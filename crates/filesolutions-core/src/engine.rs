use crate::{folder, model::*, rules::Matcher, staging, store::Store};
use anyhow::{Context, Result};
use filesolutions_windows::{filesystem, security, status::Cache};
use notify::Watcher;
use serde_json::Value;
use std::{
    collections::{HashMap, HashSet},
    fs::File,
    path::{Path, PathBuf},
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, Mutex,
    },
    time::Duration,
};

struct PendingPreview {
    config: Config,
    expires: i64,
    paths: Vec<(String, String)>,
}
struct StatusService {
    task: tokio::task::JoinHandle<()>,
    cache: Arc<Cache>,
}
impl Drop for StatusService {
    fn drop(&mut self) {
        self.task.abort();
        self.cache.invalidate();
    }
}
pub struct Engine {
    pub state: PathBuf,
    store: Mutex<Store>,
    progress: Mutex<Progress>,
    summary: Mutex<Counts>,
    previews: Mutex<HashMap<String, PendingPreview>>,
    wake: tokio::sync::Notify,
    dirty: AtomicBool,
    force: AtomicBool,
    stopping: AtomicBool,
    applying: AtomicBool,
    cache: Arc<Cache>,
    watcher: Mutex<Option<notify::RecommendedWatcher>>,
    watched: Mutex<Vec<String>>,
    scan_lock: Mutex<()>,
    run_lock: tokio::sync::Mutex<()>,
}
impl Engine {
    /// Browser status uses this profile's existing cache, including isolated profiles.
    pub fn browse(&self, path: &str) -> Result<Value> {
        let mut listing = crate::browser::list(path, &self.config()?)?;
        if let Some(entries) = listing["entries"].as_array_mut() {
            for entry in entries {
                let status = entry["path"].as_str().and_then(|p| self.cache.lookup(Path::new(p)));
                if let Some(status) = status {
                    entry["status"] = serde_json::json!(match status {
                        1 => "backed_up", 2 => "syncing", 3 => "pending",
                        4 => "excluded", 5 => "issue", _ => "",
                    });
                }
            }
        }
        Ok(listing)
    }

    /// One canonical review for a source and its explicitly chosen destination.
    /// No backup is performed here; apply retains the existing review transaction.
    pub fn backup_selection_preview(&self, source: Option<&Path>, destination_id: Option<&str>, new_destination: Option<(String, String)>) -> Result<Preview> {
        let mut config = self.config()?;
        let destination = if let Some((name, path)) = new_destination {
            anyhow::ensure!(!name.trim().is_empty(), "Name the backup location");
            let mut destination = folder::select(Path::new(&path), &config, &self.state)?;
            if let Some(existing) = config.destinations.iter().find(|d| filesystem::key(Path::new(&d.path)) == filesystem::key(Path::new(&destination.path))) {
                existing.clone()
            } else {
                destination.name = name.trim().into();
                config.destinations.push(destination.clone());
                destination
            }
        } else {
            config.destinations.iter().find(|d| Some(d.id.as_str()) == destination_id).cloned().context("Choose a backup location")?
        };
        if config.destination.is_none() { config.destination = Some(destination.clone()); }
        if let Some(path) = source {
            filesystem::no_reparse(path)?;
            let path = std::fs::canonicalize(path)?;
            if let Some(root) = config.roots.iter_mut().find(|r| filesystem::key(Path::new(&r.path)) == filesystem::key(&path)) {
                root.enabled = true;
                config.assignments.insert(root.id.clone(), destination.id.clone());
            } else {
                anyhow::ensure!(!config.roots.iter().any(|r| filesystem::contains(Path::new(&r.path), &path)), "This item inherits its parent source's backup location. Use its parent backup rule or change the parent's location.");
                let root_id = id();
                config.roots.push(Root { id: root_id.clone(), label: path.file_name().unwrap_or_default().to_string_lossy().into(), path: filesystem::display(&path), volume_identity: String::new(), enabled: true, is_file: path.is_file() });
                config.assignments.insert(root_id, destination.id.clone());
            }
        }
        self.preview(config, format!("{} {}; source files and retained backups are kept", if source.is_some() { "Back up selection to" } else { "Add backup location" }, destination.name))
    }

    pub fn open(state: PathBuf) -> Result<Arc<Self>> {
        Self::open_with_cache(
            state,
            if std::env::var_os("FILESOLUTIONS_PROFILE").is_some() {
                Cache::isolated()?
            } else {
                Cache::open(true)?
            },
        )
    }
    fn open_with_cache(state: PathBuf, cache: Cache) -> Result<Arc<Self>> {
        security::private_directory(&state, false)?;
        let store = Store::open(&state.join("state.sqlite"))?;
        // 0.1.0 did not persist coverage per item. Republish its known versions once;
        // this does not upload file contents again or change source configuration.
        if store.items()?.iter().any(|i| {
            i.current
                .as_ref()
                .is_some_and(|v| i.cataloged_remote_id.as_deref() != Some(&v.remote_id))
        }) {
            store.set("catalogDirty", &true)?;
        }
        let last_backup = store.setting::<Option<i64>>("lastBackup")?.flatten();
        Ok(Arc::new(Self {
            state,
            store: Mutex::new(store),
            progress: Mutex::new(Progress {
                phase: "starting".into(),
                last_backup,
                ..Default::default()
            }),
            summary: Mutex::new(Counts::default()),
            previews: Mutex::new(HashMap::new()),
            wake: tokio::sync::Notify::new(),
            dirty: AtomicBool::new(true),
            force: AtomicBool::new(false),
            stopping: AtomicBool::new(false),
            applying: AtomicBool::new(false),
            cache: Arc::new(cache),
            watcher: Mutex::new(None),
            watched: Mutex::new(vec![]),
            scan_lock: Mutex::new(()),
            run_lock: tokio::sync::Mutex::new(()),
        }))
    }
    pub fn browser_state(&self) -> Result<Value> {
        Ok(self
            .store
            .lock()
            .unwrap()
            .setting::<Value>("browserState")?
            .unwrap_or(Value::Null))
    }
    pub fn save_browser_state(&self, state: &Value) -> Result<()> {
        anyhow::ensure!(
            serde_json::to_vec(state)?.len() < 128 * 1024,
            "Too many browser tabs"
        );
        self.store.lock().unwrap().set("browserState", state)
    }
    pub fn config(&self) -> Result<Config> {
        self.store.lock().unwrap().config()
    }
    pub fn items(&self) -> Result<Vec<Item>> {
        self.store.lock().unwrap().items()
    }
    pub async fn snapshot(self: &Arc<Self>) -> Result<Snapshot> {
        let engine = self.clone();
        tokio::task::spawn_blocking(move || engine.snapshot_local()).await?
    }
    fn snapshot_local(&self) -> Result<Snapshot> {
        let config = self.config()?;
        let mut recent = self.display_items()?;
        let mut counts = Counts::default();
        for item in &recent {
            match item.status {
                Status::BackedUp => counts.backed_up += 1,
                Status::Pending | Status::Syncing => counts.pending += 1,
                Status::Issue => counts.issues += 1,
                Status::Excluded => counts.excluded += 1,
                Status::Retained => counts.retained += 1,
            }
        }
        *self.summary.lock().unwrap() = counts.clone();
        recent.sort_by_key(|i| std::cmp::Reverse(i.last_seen));
        recent.truncate(1000);
        let mut progress = self.progress.lock().unwrap().clone();
        if config.paused() {
            progress.phase = "paused".into();
            progress.message = "Backup is paused. Existing backup copies are retained.".into();
        }
        Ok(Snapshot {
            config,
            progress,
            counts,
            recent,
            version: env!("CARGO_PKG_VERSION").into(),
        })
    }
    pub fn display_items(&self) -> Result<Vec<Item>> {
        let mut items = self.items()?;
        for item in &mut items {
            if item.status != item.effective_status() {
                item.status = Status::Pending;
                item.reason = "Committing local recovery checkpoint".into();
            }
        }
        Ok(items)
    }
    pub fn tray_status(&self) -> Result<(bool, String, Counts)> {
        let paused = self.config()?.paused();
        let phase = if paused {
            "paused".into()
        } else {
            self.progress.lock().unwrap().phase.clone()
        };
        Ok((paused, phase, self.summary.lock().unwrap().clone()))
    }
    fn phase(&self, phase: &str, message: &str) {
        let mut p = self.progress.lock().unwrap();
        p.phase = phase.into();
        p.message = message.into();
    }
    pub fn rescan(&self) {
        self.dirty.store(true, Ordering::SeqCst);
        self.cache.mark_pending(None);
        self.wake.notify_one();
    }
    pub fn sync_now(&self) -> Result<()> {
        anyhow::ensure!(
            !self.config()?.paused(),
            "Resume backup before selecting Sync now"
        );
        self.force.store(true, Ordering::SeqCst);
        self.rescan();
        Ok(())
    }
    pub fn pause(&self, seconds: Option<i64>) -> Result<()> {
        let mut s = self.store.lock().unwrap();
        let mut c = s.config()?;
        let expected = c.revision;
        c.revision += 1;
        c.paused_until = seconds.map(|n| if n <= 0 { 0 } else { now() + n });
        s.save_config(&c, expected)?;
        drop(s);
        self.wake.notify_one();
        Ok(())
    }
    pub fn shutdown(&self) {
        self.stopping.store(true, Ordering::SeqCst);
        self.cache.invalidate();
        self.wake.notify_one();
    }
    pub async fn stop_when_idle(&self) {
        self.shutdown();
        let _guard = self.run_lock.lock().await;
    }
    fn permitted(&self) -> Result<()> {
        anyhow::ensure!(
            !self.applying.load(Ordering::SeqCst),
            "Applying reviewed settings at a safe checkpoint"
        );
        anyhow::ensure!(
            !self.dirty.load(Ordering::SeqCst),
            "Checking changed source files before continuing backup"
        );
        anyhow::ensure!(
            !self.stopping.load(Ordering::SeqCst),
            "FileSolutions is closing"
        );
        anyhow::ensure!(
            !self.config()?.paused(),
            "Backup paused at a safe checkpoint"
        );
        Ok(())
    }
    pub fn preview(&self, mut config: Config, description: String) -> Result<Preview> {
        let current = self.config()?;
        anyhow::ensure!(
            config.revision == current.revision,
            "Configuration changed; open this review again"
        );
        // Device identity and security limits are controlled by the host, never imported from a web view.
        config.device_id = current.device_id;
        config.revision = current.revision + 1;
        anyhow::ensure!(
            config.staging_budget_bytes >= 1024 * 1024
                && config.free_space_reserve_bytes >= 1024 * 1024,
            "Storage limits must be at least 1 MiB"
        );
        anyhow::ensure!(
            config.upload_limit_kib != Some(0),
            "Upload limit must be positive or empty"
        );
        anyhow::ensure!(
            ["system", "light", "dark"].contains(&config.theme.as_str()),
            "Unknown theme"
        );
        let mut paths = vec![];
        let mut ids = HashSet::new();
        for root in &mut config.roots {
            anyhow::ensure!(ids.insert(root.id.clone()), "Duplicate folder ID");
            let (path, identity, is_file) =
                filesystem::validate_root(Path::new(&root.path), &self.state)?;
            for (p, _) in &paths {
                anyhow::ensure!(
                    !filesystem::contains(Path::new(p), &path)
                        && !filesystem::contains(&path, Path::new(p)),
                    "Selected folders must not overlap"
                );
            }
            root.path = filesystem::display(&path);
            root.volume_identity = identity.clone();
            root.is_file = is_file;
            anyhow::ensure!(!root.label.trim().is_empty(), "A folder needs a name");
            paths.push((root.path.clone(), identity));
        }
        // Retain registered destinations so old versions remain restorable.
        for old in &current.destinations {
            anyhow::ensure!(config.destinations.iter().any(|d| d == old), "Existing destination records cannot be removed or changed while versions are retained");
        }
        if let Some(d) = config.destination.clone() {
            let selected = if let Some(old) = current
                .destinations
                .iter()
                .find(|old| old.id == d.id && old.path == d.path)
            {
                old.clone()
            } else {
                let mut selected = folder::select(Path::new(&d.path), &config, &self.state)?;
                // A new source/location review already assigned this freshly validated ID.
                // Preserve it when normalizing the first default, rather than duplicate it.
                if let Some(candidate) = config.destinations.iter().find(|candidate| candidate.id == d.id && candidate.path == selected.path && candidate.identity == selected.identity) {
                    selected.id = candidate.id.clone();
                }
                if !d.name.trim().is_empty() {
                    selected.name = d.name.clone();
                }
                selected
            };
            if !config.destinations.iter().any(|d| d.id == selected.id) {
                config.destinations.push(selected.clone());
            }
            config.destination = Some(selected);
        }
        let mut destination_ids = HashSet::new();
        for (i, d) in config.destinations.iter().enumerate() {
            anyhow::ensure!(destination_ids.insert(&d.id), "Duplicate destination ID");
            folder::validate(Path::new(&d.path), &config, &self.state)?;
            for other in config.destinations.iter().take(i) {
                anyhow::ensure!(
                    !filesystem::contains(Path::new(&d.path), Path::new(&other.path))
                        && !filesystem::contains(Path::new(&other.path), Path::new(&d.path)),
                    "Backup destinations must not overlap"
                );
            }
            folder::initialize(d, &config, &self.state)?;
        }
        for (root, destination) in &config.assignments {
            anyhow::ensure!(
                ids.contains(root) && destination_ids.contains(destination),
                "Invalid source-to-destination assignment"
            );
        }
        for rule in &config.rules {
            anyhow::ensure!(
                rule.scope == "global" || ids.contains(&rule.scope),
                "Rule refers to a missing folder"
            );
        }
        let matcher = Matcher::new(&config.rules)?;
        let mut included = 0;
        let mut excluded = 0;
        let mut bytes = 0;
        let mut examples = vec![];
        let mut warnings = vec![];
        if config.destination != current.destination {
            warnings.push("Unassigned sources use this default destination. Explicit assignments and earlier versions remain.".into());
        }
        for root in config.roots.iter().filter(|r| r.enabled) {
            walk_root(root, |path, relative| {
                match filesystem::eligible(path) {
                    Ok(()) => {
                        let decision = matcher.decide(&root.id, relative)?;
                        if decision.included {
                            included += 1;
                            bytes += std::fs::metadata(path)?.len();
                        } else {
                            excluded += 1;
                        }
                        if examples.len() < 12 {
                            examples.push(format!(
                                "{} / {} — {}",
                                root.label, relative, decision.reason
                            ));
                        }
                    }
                    Err(e) => {
                        excluded += 1;
                        if warnings.len() < 12 {
                            warnings.push(format!("{relative}: {e}"));
                        }
                    }
                }
                Ok(())
            })?;
        }
        let token = id();
        let preview = Preview {
            token: token.clone(),
            revision: current.revision,
            description,
            included,
            excluded,
            bytes,
            examples,
            warnings,
        };
        let mut pending = self.previews.lock().unwrap();
        pending.retain(|_, p| p.expires > now());
        pending.insert(
            token,
            PendingPreview {
                config,
                expires: now() + 600,
                paths,
            },
        );
        Ok(preview)
    }
    pub fn apply_preview(&self, token: &str) -> Result<()> {
        let _run = self.run_lock.try_lock().map_err(|_| {
            anyhow::anyhow!("Backup is finishing a file; try applying again in a moment")
        })?;
        self.apply_locked(token)
    }
    pub async fn apply_when_idle(self: &Arc<Self>, token: String) -> Result<()> {
        anyhow::ensure!(
            !self.applying.swap(true, Ordering::SeqCst),
            "Another configuration review is being applied"
        );
        struct Reset<'a>(&'a AtomicBool);
        impl Drop for Reset<'_> {
            fn drop(&mut self) {
                self.0.store(false, Ordering::SeqCst);
            }
        }
        let _reset = Reset(&self.applying);
        let _guard = self.run_lock.lock().await;
        let engine = self.clone();
        tokio::task::spawn_blocking(move || engine.apply_locked(&token)).await?
    }
    fn apply_locked(&self, token: &str) -> Result<()> {
        let _scan = self.scan_lock.lock().unwrap();
        let preview = self
            .previews
            .lock()
            .unwrap()
            .remove(token)
            .context("Review expired. Preview the change again.")?;
        anyhow::ensure!(preview.expires > now(), "Review expired");
        for (path, expected) in preview.paths {
            let (_, identity, _) = filesystem::validate_root(Path::new(&path), &self.state)?;
            anyhow::ensure!(
                identity == expected,
                "A selected folder was replaced since the preview"
            );
        }
        anyhow::ensure!(
            self.config()?.revision == preview.config.revision - 1,
            "Configuration changed. Preview again."
        );
        for destination in &preview.config.destinations {
            folder::initialize(destination, &preview.config, &self.state)?;
        }
        let mut s = self.store.lock().unwrap();
        s.save_config(&preview.config, preview.config.revision - 1)?;
        self.progress.lock().unwrap().last_backup =
            s.setting::<Option<i64>>("lastBackup")?.flatten();
        drop(s);
        self.force.store(true, Ordering::SeqCst);
        self.rescan();
        Ok(())
    }
    pub fn undo_preview(&self) -> Result<Preview> {
        let current = self.config()?;
        let mut previous = self
            .store
            .lock()
            .unwrap()
            .previous_config(current.revision)?
            .context("No earlier configuration is available")?;
        previous.revision = current.revision;
        previous.paused_until = current.paused_until;
        self.preview(previous, "Undo the last saved configuration".into())
    }
    pub fn shell_preview(&self, path: &Path, include: bool) -> Result<Preview> {
        let mut c = self.config()?;
        filesystem::no_reparse(path)?;
        let path = std::fs::canonicalize(path)?;
        let root = c
            .roots
            .iter()
            .find(|r| filesystem::contains(Path::new(&r.path), &path))
            .cloned();
        if let Some(root) = root {
            let same = filesystem::key(Path::new(&root.path)) == filesystem::key(&path);
            if same && !root.is_file {
                c.roots
                    .iter_mut()
                    .find(|r| r.id == root.id)
                    .unwrap()
                    .enabled = include;
            } else {
                let relative = if root.is_file {
                    Path::new(&root.path)
                        .file_name()
                        .unwrap()
                        .to_string_lossy()
                        .into_owned()
                } else {
                    filesystem::display(&path)
                        .strip_prefix(&format!("{}\\", root.path))
                        .context("Cannot resolve relative path")?
                        .replace('\\', "/")
                };
                let order = c.rules.iter().map(|r| r.order).max().unwrap_or(0) + 1;
                c.rules.push(Rule {
                    id: id(),
                    scope: root.id,
                    kind: if path.is_dir() {
                        RuleKind::Subtree
                    } else {
                        RuleKind::ExactFile
                    },
                    action: if include {
                        Action::Include
                    } else {
                        Action::Exclude
                    },
                    value: relative,
                    order,
                });
            }
        } else {
            anyhow::ensure!(include, "This item is already outside your backup folders");
            c.roots.push(Root {
                id: id(),
                label: path
                    .file_name()
                    .unwrap_or_default()
                    .to_string_lossy()
                    .into(),
                path: filesystem::display(&path),
                volume_identity: String::new(),
                enabled: true,
                is_file: path.is_file(),
            });
        }
        self.preview(
            c,
            if include {
                "Include selected item"
            } else {
                "Exclude selected item; keep backup copies"
            }
            .into(),
        )
    }
    pub fn inherit_preview(&self, path: &Path) -> Result<Preview> {
        let mut config = self.config()?;
        let path = std::fs::canonicalize(path)?;
        let root = config
            .roots
            .iter()
            .find(|r| filesystem::contains(Path::new(&r.path), &path))
            .cloned()
            .context("No explicit backup selection")?;
        if filesystem::key(Path::new(&root.path)) == filesystem::key(&path) {
            config.roots.retain(|r| r.id != root.id);
            config.rules.retain(|r| r.scope != root.id);
            config.assignments.remove(&root.id);
        } else {
            let base = std::fs::canonicalize(&root.path)?;
            let relative = path
                .strip_prefix(base)?
                .to_string_lossy()
                .replace('\\', "/");
            config.rules.retain(|r| {
                !(r.scope == root.id
                    && r.value.eq_ignore_ascii_case(&relative)
                    && matches!(r.kind, RuleKind::ExactFile | RuleKind::Subtree))
            });
        }
        self.preview(
            config,
            "Inherit parent selection; preserve all retained backup versions".into(),
        )
    }
    fn scan(&self) -> Result<()> {
        let _scan = self.scan_lock.lock().unwrap();
        let scan_epoch = self.cache.epoch();
        self.phase("scanning", "Checking selected folders");
        let config = self.config()?;
        let matcher = Matcher::new(&config.rules)?;
        let epoch = now();
        let mut failures = vec![];
        for root in &config.roots {
            if !root.enabled {
                for mut item in self.items()?.into_iter().filter(|i| i.root_id == root.id) {
                    item.status = Status::Excluded;
                    item.reason = "Folder backup is disabled; backup copies retained".into();
                    self.store.lock().unwrap().put_item(&item)?;
                }
                continue;
            }
            let result = (|| -> Result<()> {
                let (_, identity, _) =
                    filesystem::validate_root(Path::new(&root.path), &self.state)?;
                anyhow::ensure!(
                    identity == root.volume_identity,
                    "Folder identity changed; select the folder again"
                );
                let mut seen = HashSet::new();
                walk_root(root, |path, relative| {
                    seen.insert(relative.to_lowercase());
                    let meta = std::fs::symlink_metadata(path)?;
                    let changed_at = staging::modified(&meta);
                    let previous = self.store.lock().unwrap().find(&root.id, relative)?;
                    let mut item = previous.unwrap_or(Item {
                        id: id(),
                        root_id: root.id.clone(),
                        relative_path: relative.into(),
                        size: meta.len(),
                        modified: changed_at,
                        generation: 1,
                        status: Status::Pending,
                        reason: String::new(),
                        winning_rule: None,
                        current: None,
                        cataloged_remote_id: None,
                        history: vec![],
                        last_seen: epoch,
                    });
                    if item.size != meta.len() || item.modified != changed_at {
                        item.generation += 1;
                        item.size = meta.len();
                        item.modified = changed_at;
                        item.status = Status::Pending;
                    }
                    item.last_seen = epoch;
                    let decision = matcher.decide(&root.id, relative)?;
                    item.winning_rule = decision.winning_rule;
                    if let Err(error) = filesystem::eligible(path) {
                        item.status = Status::Excluded;
                        item.reason = error.to_string();
                    } else if !decision.included {
                        item.status = Status::Excluded;
                        item.reason = decision.reason;
                    } else if item.status != Status::Issue
                        || !item.reason.starts_with("Backup conflict:")
                    {
                        // Hash the stable source on every reconciliation; timestamps alone never establish green.
                        match filesystem::locked_source(path, Path::new(&root.path))
                            .and_then(staging::hashes)
                        {
                            Ok((sha, _, size)) => {
                                let same = item
                                    .current
                                    .as_ref()
                                    .is_some_and(|v| v.sha256 == sha && v.size == size);
                                if same {
                                    item.status = Status::BackedUp;
                                    item.reason = "Verified backup matches local content".into();
                                } else {
                                    let staged = self.store.lock().unwrap().job(&item.id)?;
                                    if item.status == Status::BackedUp
                                        || staged.is_some_and(|j| {
                                            j.generation == item.generation && j.sha256 != sha
                                        })
                                    {
                                        item.generation += 1;
                                    }
                                    item.status = Status::Pending;
                                    item.reason = "Waiting for verified backup".into();
                                }
                            }
                            Err(e) => {
                                item.status = Status::Issue;
                                item.reason = e.to_string();
                            }
                        }
                    }
                    self.store.lock().unwrap().put_item(&item)?;
                    Ok(())
                })?;
                for mut item in self.items()?.into_iter().filter(|i| {
                    i.root_id == root.id && !seen.contains(&i.relative_path.to_lowercase())
                }) {
                    item.status = Status::Retained;
                    item.reason = "Local item is absent; backup versions retained".into();
                    self.store.lock().unwrap().put_item(&item)?;
                }
                Ok(())
            })();
            if let Err(e) = result {
                failures.push(format!("{}: {e}", root.label));
                for mut item in self.items()?.into_iter().filter(|i| i.root_id == root.id) {
                    item.status = Status::Issue;
                    item.reason="Backup folder unavailable or scan incomplete; no remote deletion performed".into();
                    self.store.lock().unwrap().put_item(&item)?;
                }
            }
        }
        for mut item in self
            .items()?
            .into_iter()
            .filter(|i| !config.roots.iter().any(|r| r.id == i.root_id))
        {
            item.status = Status::Retained;
            item.reason = "Folder removed from configuration; backup versions retained".into();
            self.store.lock().unwrap().put_item(&item)?;
        }
        self.store.lock().unwrap().set("scanErrors", &failures)?;
        self.publish_cache_scanned(Some(scan_epoch))?;
        if !failures.is_empty() {
            self.phase("issue", &failures.join("; "));
        }
        Ok(())
    }
    fn publish_cache(&self) -> Result<()> {
        self.publish_cache_scanned(None)
    }
    fn publish_cache_scanned(&self, scanned_through: Option<u64>) -> Result<()> {
        if self.stopping.load(Ordering::SeqCst) {
            return Ok(());
        }
        let config = self.config()?;
        let items = self.items()?;
        let mut paths = vec![];
        let mut folders: HashMap<PathBuf, u32> = HashMap::new();
        let mut summary = Counts::default();
        for item in &items {
            match item.effective_status() {
                Status::BackedUp => summary.backed_up += 1,
                Status::Pending | Status::Syncing => summary.pending += 1,
                Status::Excluded => summary.excluded += 1,
                Status::Issue => summary.issues += 1,
                Status::Retained => summary.retained += 1,
            }
        }
        *self.summary.lock().unwrap() = summary;
        for root in &config.roots {
            for item in items.iter().filter(|i| i.root_id == root.id) {
                let state = item.effective_status().overlay();
                let path = if root.is_file {
                    PathBuf::from(&root.path)
                } else {
                    Path::new(&root.path).join(filesystem::safe_relative(&item.relative_path)?)
                };
                if state == 0 {
                    continue;
                }
                paths.push((path.clone(), state));
                if !root.is_file {
                    let mut parent = path.parent();
                    while let Some(directory) = parent {
                        if !filesystem::contains(Path::new(&root.path), directory) {
                            break;
                        }
                        let current = folders.entry(directory.to_path_buf()).or_insert(0);
                        *current = aggregate(*current, if !root.enabled { 4 } else { state });
                        parent = directory.parent();
                    }
                }
            }
        }
        paths.extend(folders);
        self.cache.publish_scanned(&paths, scanned_through)?;
        Ok(())
    }
    fn update_watchers(self: &Arc<Self>) -> Result<()> {
        let roots = self
            .config()?
            .roots
            .into_iter()
            .filter(|r| r.enabled)
            .map(|r| r.path)
            .collect::<Vec<_>>();
        if *self.watched.lock().unwrap() == roots && self.watcher.lock().unwrap().is_some() {
            return Ok(());
        }
        let weak = Arc::downgrade(self);
        let mut watcher =
            notify::recommended_watcher(move |event: notify::Result<notify::Event>| {
                if let Some(engine) = weak.upgrade() {
                    match event {
                        Ok(event) if matches!(event.kind, notify::EventKind::Access(_)) => {}
                        Ok(event) if !event.paths.is_empty() && !event.need_rescan() => {
                            engine.cache.mark_pending(Some(&event.paths));
                            engine.dirty.store(true, Ordering::SeqCst);
                            engine.wake.notify_one();
                        }
                        _ => engine.rescan(),
                    }
                }
            })?;
        for root in &roots {
            watcher.watch(Path::new(root), notify::RecursiveMode::Recursive)?;
        }
        *self.watcher.lock().unwrap() = Some(watcher);
        *self.watched.lock().unwrap() = roots;
        Ok(())
    }
    pub async fn run(self: Arc<Self>) {
        // This task never scans or waits for a destination transfer. Its lifetime follows the engine.
        let weak = Arc::downgrade(&self);
        let status_task = tokio::spawn(async move {
            let mut notification: Option<tokio::task::JoinHandle<()>> = None;
            loop {
                tokio::time::sleep(Duration::from_millis(500)).await;
                let Some(engine) = weak.upgrade() else {
                    break;
                };
                if engine.stopping.load(Ordering::SeqCst) {
                    break;
                }
                engine.cache.heartbeat();
                if notification
                    .as_ref()
                    .is_some_and(|task| !task.is_finished())
                {
                    continue;
                }
                let paths = engine.cache.drain_changes(256);
                if !paths.is_empty() {
                    notification = Some(tokio::task::spawn_blocking(move || {
                        filesolutions_windows::integration::refresh_overlays(&paths)
                    }));
                }
            }
        });
        // Cancellation and unexpected worker exit must also stop the heartbeat.
        let _status = StatusService {
            task: status_task,
            cache: self.cache.clone(),
        };
        let mut reconciled = 0;
        let mut destination_checked = 0;
        loop {
            if self.stopping.load(Ordering::SeqCst) {
                break;
            }
            let _guard = self.run_lock.lock().await;
            let forced = self.force.swap(false, Ordering::SeqCst);
            let config = match self.config() {
                Ok(c) => c,
                Err(e) => {
                    self.phase("issue", &e.to_string());
                    break;
                }
            };
            if self.dirty.swap(false, Ordering::SeqCst) || now() - reconciled >= 86400 || forced {
                let e = self.clone();
                match tokio::task::spawn_blocking(move || e.scan()).await {
                    Ok(Ok(())) => {
                        reconciled = now();
                        if let Err(e) = self.update_watchers() {
                            self.phase("issue", &format!("Folder watching unavailable: {e}"));
                            self.dirty.store(true, Ordering::SeqCst);
                        }
                    }
                    Ok(Err(e)) => self.phase("issue", &e.to_string()),
                    Err(_) => self.phase("issue", "Folder scan stopped unexpectedly"),
                }
            }
            if config.paused() {
                self.phase("paused", "Backup is paused");
            } else if config.destination.is_none() {
                self.phase(
                    "disconnected",
                    "Choose a backup destination. Your saved folders and exclusions are ready.",
                );
            } else if config.continuous || forced {
                let result = async {
                    folder::base(&config)?;
                    if now() - destination_checked >= 300 || forced {
                        self.check_destination().await?;
                        destination_checked = now();
                    }
                    self.save_catalog().await?;
                    self.backup_pending().await?;
                    self.save_catalog().await?;
                    Ok::<_, anyhow::Error>(())
                }
                .await;
                if let Err(e) = result {
                    if forced && self.dirty.load(Ordering::SeqCst)
                        && !self.stopping.load(Ordering::SeqCst) && !self.config().is_ok_and(|c| c.paused())
                    {
                        // Source reconciliation is a checkpoint, not completion of Sync now.
                        // Retain that one request across a watcher/Refresh interruption.
                        self.force.store(true, Ordering::SeqCst);
                        self.wake.notify_one();
                        self.phase("waiting", "Checking changed source files before continuing backup");
                    } else {
                        self.phase("issue", &e.to_string());
                    }
                } else {
                    let items = self.items().unwrap_or_default();
                    let errors = self
                        .store
                        .lock()
                        .unwrap()
                        .setting::<Vec<String>>("scanErrors")
                        .ok()
                        .flatten()
                        .unwrap_or_default();
                    let issues =
                        items.iter().filter(|i| i.status == Status::Issue).count() + errors.len();
                    let pending = items
                        .iter()
                        .any(|i| i.status == Status::Pending || i.status == Status::Syncing);
                    self.phase(
                        if issues > 0 {
                            "issue"
                        } else if pending {
                            "waiting"
                        } else {
                            "idle"
                        },
                        if issues > 0 {
                            "Some files or folders need attention"
                        } else if pending {
                            "Pending files will retry after their saved backoff"
                        } else {
                            "All selected changes are backed up"
                        },
                    );
                }
            } else {
                self.phase(
                    "idle",
                    "Automatic backup is off. Select Sync now to run backup.",
                );
            }
            let _ = self.publish_cache();
            drop(_guard);
            tokio::select! {_=self.wake.notified()=>tokio::time::sleep(Duration::from_millis(750)).await,_=tokio::time::sleep(Duration::from_secs(5))=>{}}
        }
    }
    async fn check_destination(self: &Arc<Self>) -> Result<()> {
        let engine = self.clone();
        tokio::task::spawn_blocking(move || {
            let config = engine.config()?;
            for mut item in engine.items()?.into_iter().filter(|i| i.current.is_some() && !matches!(i.status, Status::Retained | Status::Excluded)) {
                engine.permitted()?;
                if !folder::matches(&config, item.current.as_ref().unwrap())? {
                    item.status = Status::Issue;
                    item.reason = "Backup conflict: a saved version changed or disappeared. Review it, then acknowledge to create a new snapshot.".into();
                    engine.store.lock().unwrap().put_item(&item)?;
                } else if let Some(root) = config.roots.iter().find(|r|r.id==item.root_id) {
                    let selected = config.for_root(&root.id);
                    let readable=folder::resolve(&selected,&folder::reference(&selected,&folder::latest_relative(&selected,root,&item)?)?)?;
                    if !readable.try_exists()? {
                        item.status=Status::Pending;
                        item.reason="Readable copy is missing; rebuilding it from the source".into();
                        engine.store.lock().unwrap().put_item(&item)?;
                    } else {
                        let (sha,_,size)=staging::hashes(File::open(readable)?)?;
                        let version=item.current.as_ref().unwrap();
                        if sha!=version.sha256 || size!=version.size {
                            item.status=Status::Issue;
                            item.reason="Backup conflict: the readable destination file changed. Acknowledge to preserve it separately and create a fresh backup.".into();
                            engine.store.lock().unwrap().put_item(&item)?;
                        }
                    }
                }
            }
            Ok::<_, anyhow::Error>(())
        }).await?
    }
    pub fn acknowledge_conflict(&self, item_id: &str) -> Result<()> {
        let mut item = self
            .store
            .lock()
            .unwrap()
            .item(item_id)?
            .context("Unknown file")?;
        anyhow::ensure!(
            item.reason.starts_with("Backup conflict:"),
            "No backup conflict to acknowledge"
        );
        if let Some(previous) = item.current.take() {
            item.history.push(previous);
        }
        item.status = Status::Pending;
        item.reason = "Backup conflict acknowledged; a new separate backup will be created".into();
        self.store.lock().unwrap().put_item(&item)?;
        self.rescan();
        Ok(())
    }
    async fn backup_pending(self: &Arc<Self>) -> Result<()> {
        let mut checkpoint = std::time::Instant::now();
        let mut completed = 0;
        for mut item in self
            .items()?
            .into_iter()
            .filter(|i| i.status == Status::Pending || i.status == Status::Syncing)
        {
            self.permitted()?;
            if completed > 0
                && (item.size >= 8 * 1024 * 1024 || checkpoint.elapsed() >= Duration::from_secs(5))
            {
                self.save_catalog().await?;
                completed = 0;
                checkpoint = std::time::Instant::now();
            }
            let config = self.config()?;
            if item.reason == "Copy verified; committing local recovery checkpoint" {
                continue;
            }
            let Some(root) = config
                .roots
                .iter()
                .find(|r| r.enabled && r.id == item.root_id)
            else {
                continue;
            };
            if !Matcher::new(&config.rules)?
                .decide(&root.id, &item.relative_path)?
                .included
            {
                continue;
            }
            let existing = self.store.lock().unwrap().job(&item.id)?;
            let existing = match existing {
                Some(job) if job.generation != item.generation => {
                    self.discard_job(&job)?;
                    None
                }
                job => job,
            };
            let mut job = if let Some(job) = existing {
                if job.retry_at > now() {
                    continue;
                }
                job
            } else {
                let e = self.clone_for_capture(root, &item, &config);
                match tokio::task::spawn_blocking(move || staging::capture(&e.0, &e.1, &e.2, &e.3))
                    .await?
                {
                    Ok(job) => {
                        self.store.lock().unwrap().put_job(&job)?;
                        job
                    }
                    Err(error) => {
                        item.status = Status::Issue;
                        item.reason = error.to_string();
                        self.store.lock().unwrap().put_item(&item)?;
                        continue;
                    }
                }
            };
            item.status = Status::Syncing;
            item.reason = "Copying a verified snapshot".into();
            self.store.lock().unwrap().put_item(&item)?;
            self.phase("syncing", "Backing up changes");
            {
                let mut p = self.progress.lock().unwrap();
                p.current_file = item.relative_path.clone();
                p.sent = job.offset;
                p.total = job.size;
            }
            self.publish_cache()?;
            let worker = self.clone();
            let copy_config = config.clone();
            let copy_root = root.clone();
            let copy_item = item.clone();
            let copy_job = job.clone();
            let result = tokio::task::spawn_blocking(move || {
                worker.copy_item(&copy_config, &copy_root, &copy_item, &copy_job)
            })
            .await?;
            match result {
                Ok(version) => {
                    let mut latest = self
                        .store
                        .lock()
                        .unwrap()
                        .item(&item.id)?
                        .context("File record disappeared")?;
                    if let Some(old) = latest.current.replace(version) {
                        latest.history.push(old);
                    }
                    latest.status = Status::Pending;
                    latest.reason = "Copy verified; committing local recovery checkpoint".into();
                    self.store.lock().unwrap().commit_verified(&latest)?;
                    self.remove_stage(&job);
                    completed += 1;
                    // Bound progress latency without rewriting a full catalog after every tiny file.
                    if completed >= 8 || checkpoint.elapsed() >= Duration::from_secs(5) {
                        self.save_catalog().await?;
                        completed = 0;
                        checkpoint = std::time::Instant::now();
                    }
                }
                Err(error) => {
                    if self.dirty.load(Ordering::SeqCst) {
                        // The rescan validates the saved generation before reuse. Do not
                        // penalize an intentional source-check interruption as an IO failure.
                        job.retry_at = 0;
                    } else {
                        job.attempts = job.attempts.saturating_add(1);
                        job.retry_at =
                            now() + (5_i64.saturating_mul(2_i64.pow(job.attempts.min(9)))).min(1800);
                    }
                    self.store.lock().unwrap().put_job(&job)?;
                    item.status = if error.to_string().starts_with("Backup conflict:") {
                        Status::Issue
                    } else {
                        Status::Pending
                    };
                    item.reason = error.to_string();
                    self.store.lock().unwrap().put_item(&item)?;
                    return Err(error);
                }
            }
        }
        Ok(())
    }
    fn clone_for_capture(
        &self,
        root: &Root,
        item: &Item,
        config: &Config,
    ) -> (Root, Item, PathBuf, Config) {
        (
            root.clone(),
            item.clone(),
            self.state.clone(),
            config.clone(),
        )
    }
    fn remove_stage(&self, job: &Job) {
        if filesystem::contains(&self.state.join("staging"), Path::new(&job.stage)) {
            let _ = std::fs::remove_file(&job.stage);
        }
    }
    fn discard_job(&self, job: &Job) -> Result<()> {
        self.store.lock().unwrap().delete_job(&job.item_id)?;
        self.remove_stage(job);
        Ok(())
    }
    fn copy_item(&self, config: &Config, root: &Root, item: &Item, job: &Job) -> Result<Version> {
        let selected = config.for_root(&root.id);
        let config = &selected;
        staging::verify_stage(job, &self.state)?;
        let relative = format!(
            "{}/versions/{}/{}/{}/{}",
            config.device_id,
            root.id,
            item.id,
            job.id,
            Path::new(&item.relative_path)
                .file_name()
                .context("Invalid source filename")?
                .to_string_lossy()
        );
        let reference = folder::reference(config, &relative)?;
        let target = folder::resolve(config, &reference)?;
        let version = Version {
            remote_id: reference,
            sha256: job.sha256.clone(),
            md5: job.md5.clone(),
            size: job.size,
            verified_at: now(),
        };
        if !target.try_exists()? {
            anyhow::ensure!(
                filesystem::free_space(Path::new(
                    &config.destination.as_ref().context("No destination")?.path
                ))? >= job
                    .size
                    .saturating_mul(2)
                    .saturating_add(config.free_space_reserve_bytes),
                "Destination needs more free space for the snapshot and readable copy"
            );
            let start = std::time::Instant::now();
            folder::copy_verified(
                Path::new(&job.stage),
                &target,
                &job.sha256,
                job.size,
                false,
                |sent| {
                    self.permitted()?;
                    self.progress.lock().unwrap().sent = sent;
                    if let Some(limit) = config.upload_limit_kib {
                        let expected =
                            Duration::from_secs_f64(sent as f64 / (limit as f64 * 1024.0));
                        while start.elapsed() < expected {
                            self.permitted()?;
                            std::thread::sleep(
                                Duration::from_millis(50)
                                    .min(expected.saturating_sub(start.elapsed())),
                            );
                        }
                    }
                    Ok(())
                },
            )?;
        } else {
            anyhow::ensure!(
                folder::matches(config, &version)?,
                "Backup conflict: an interrupted snapshot was changed"
            );
        }
        let latest = folder::resolve(
            config,
            &folder::reference(config, &folder::latest_relative(config, root, item)?)?,
        )?;
        if latest.try_exists()? {
            let (sha, _, size) = staging::hashes(File::open(&latest)?)?;
            if sha == job.sha256 && size == job.size {
                return Ok(version);
            }
            if let Some(previous) = &item.current {
                anyhow::ensure!(sha==previous.sha256 && size==previous.size,"Backup conflict: the readable destination file changed. Review and acknowledge it first.");
            } else {
                // After acknowledgment, preserve an independently edited readable file.
                let relative = format!(
                    "{}/conflicts/{}/{}/{}",
                    config.device_id,
                    item.id,
                    id(),
                    latest.file_name().unwrap().to_string_lossy()
                );
                let retained = folder::resolve(config, &folder::reference(config, &relative)?)?;
                folder::copy_verified(&latest, &retained, &sha, size, false, |_| self.permitted())?;
            }
        }
        folder::copy_verified(&target, &latest, &job.sha256, job.size, true, |_| {
            self.permitted()
        })?;
        Ok(version)
    }
    async fn save_catalog(self: &Arc<Self>) -> Result<()> {
        if !self
            .store
            .lock()
            .unwrap()
            .setting::<bool>("catalogDirty")?
            .unwrap_or(false)
        {
            return Ok(());
        }
        self.permitted()?;
        let engine = self.clone();
        tokio::task::spawn_blocking(move || {
            let config = engine.config()?;
            let catalog = Catalog {
                schema_version: 2,
                device_id: config.device_id.clone(),
                committed_at: now(),
                roots: config
                    .roots
                    .iter()
                    .map(|r| CatalogRoot {
                        id: r.id.clone(),
                        label: r.label.clone(),
                        is_file: r.is_file,
                    })
                    .collect(),
                items: engine.items()?,
            };
            let reference = id();
            engine
                .store
                .lock()
                .unwrap()
                .commit_catalog(&catalog, &reference, now())?;
            engine.progress.lock().unwrap().last_backup = Some(now());
            engine.reconcile_cataloged()
        })
        .await?
    }
    fn reconcile_cataloged(&self) -> Result<()> {
        let config = self.config()?;
        let matcher = Matcher::new(&config.rules)?;
        // Recheck only newly uploaded sources, not the entire tree on an async thread.
        for mut item in self.items()?.into_iter().filter(|i| {
            i.status == Status::Pending
                && i.reason == "Copy verified; committing local recovery checkpoint"
        }) {
            let Some(version) = &item.current else {
                continue;
            };
            if item.cataloged_remote_id.as_deref() != Some(&version.remote_id) {
                continue;
            }
            let Some(root) = config
                .roots
                .iter()
                .find(|r| r.id == item.root_id && r.enabled)
            else {
                continue;
            };
            if !matcher.decide(&root.id, &item.relative_path)?.included {
                continue;
            }
            let path = if root.is_file {
                PathBuf::from(&root.path)
            } else {
                Path::new(&root.path).join(filesystem::safe_relative(&item.relative_path)?)
            };
            let result = (|| -> Result<bool> {
                let (_, identity, _) =
                    filesystem::validate_root(Path::new(&root.path), &self.state)?;
                anyhow::ensure!(identity == root.volume_identity, "Source root changed");
                let (sha, _, size) =
                    staging::hashes(filesystem::locked_source(&path, Path::new(&root.path))?)?;
                Ok(sha == version.sha256 && size == version.size)
            })();
            match result {
                Ok(true) => {
                    item.status = Status::BackedUp;
                    item.reason = "Verified backup matches local content".into();
                }
                Ok(false) => {
                    item.generation += 1;
                    item.reason = "Local content changed; waiting for verified backup".into();
                }
                Err(e) => {
                    item.status = Status::Issue;
                    item.reason = e.to_string();
                }
            }
            self.store.lock().unwrap().put_item(&item)?;
        }
        self.publish_cache()
    }
    pub async fn catalogs(&self) -> Result<Value> {
        Ok(serde_json::to_value(
            self.store.lock().unwrap().catalogs()?,
        )?)
    }
    pub async fn read_catalog(&self, id: &str) -> Result<Catalog> {
        self.store.lock().unwrap().read_catalog(id)
    }
    pub fn restore(
        &self,
        version: Version,
        relative: String,
        destination: PathBuf,
    ) -> Result<String> {
        filesystem::no_reparse(&destination)?;
        let destination = std::fs::canonicalize(&destination)?;
        for root in self.config()?.roots {
            anyhow::ensure!(
                !filesystem::contains(Path::new(&root.path), &destination),
                "Choose a restore location outside every selected backup folder"
            );
        }
        anyhow::ensure!(
            !filesystem::contains(&self.state, &destination),
            "Cannot restore into application state"
        );
        for backup in self.config()?.destinations {
            anyhow::ensure!(
                !filesystem::contains(Path::new(&backup.path), &destination),
                "Choose a restore location outside the backup destination"
            );
        }
        let relative = filesystem::safe_relative(&relative)?;
        let target = destination.join(relative);
        anyhow::ensure!(
            filesystem::contains(&destination, &target),
            "Restore escaped its destination"
        );
        let parent_owned = target
            .parent()
            .context("Invalid restore path")?
            .to_path_buf();
        let parent = parent_owned.as_path();
        for root in self.config()?.roots {
            anyhow::ensure!(
                !filesystem::contains(Path::new(&root.path), &target),
                "Restore target lies inside a protected folder"
            );
        }
        let _parent_handles = filesystem::guarded_directories(parent)?;
        let original_name = target
            .file_stem()
            .unwrap_or_default()
            .to_string_lossy()
            .into_owned();
        let extension = target
            .extension()
            .map(|e| format!(".{}", e.to_string_lossy()))
            .unwrap_or_default();
        let mut target = target;
        let mut n = 1;
        while target.exists() {
            target = parent.join(format!("{original_name} (restored {n}){extension}"));
            n += 1;
            anyhow::ensure!(n < 1000, "Too many conflicting restore names");
        }
        let partial = parent.join(format!(".filesolutions-{}.partial", id()));
        let result = (|| -> Result<String> {
            let config = self.config()?;
            anyhow::ensure!(
                folder::matches(&config, &version)?,
                "Backup bytes changed; restore stopped"
            );
            folder::copy_verified(
                &folder::resolve(&config, &version.remote_id)?,
                &partial,
                &version.sha256,
                version.size,
                false,
                |_| Ok(()),
            )?;
            let (sha, md5, size) = staging::hashes(File::open(&partial)?)?;
            anyhow::ensure!(
                sha == version.sha256 && md5 == version.md5 && size == version.size,
                "Restored bytes failed integrity verification"
            );
            filesystem::no_reparse(parent)?;
            anyhow::ensure!(
                !target.exists(),
                "Restore destination appeared during download; try again"
            );
            filesystem::move_new(&partial, &target)?;
            Ok(filesystem::display(&target))
        })();
        let _ = std::fs::remove_file(partial);
        result
    }
}
fn walk_root(root: &Root, mut visit: impl FnMut(&Path, &str) -> Result<()>) -> Result<()> {
    if root.is_file {
        let path = Path::new(&root.path);
        return visit(
            path,
            &path
                .file_name()
                .context("Invalid filename")?
                .to_string_lossy(),
        );
    }
    for entry in walkdir::WalkDir::new(&root.path)
        .follow_links(false)
        .into_iter()
        .filter_entry(|e| {
            e.depth() == 0 || !e.file_type().is_dir() || filesystem::eligible(e.path()).is_ok()
        })
    {
        let entry = entry?;
        if entry.depth() == 0
            || entry.file_type().is_dir() && filesystem::eligible(entry.path()).is_ok()
        {
            continue;
        }
        let relative = entry
            .path()
            .strip_prefix(&root.path)?
            .to_string_lossy()
            .replace('\\', "/");
        visit(entry.path(), &relative)?;
    }
    Ok(())
}
fn aggregate(current: u32, next: u32) -> u32 {
    fn priority(state: u32) -> u8 {
        match state {
            5 => 5,
            2 => 4,
            3 => 3,
            1 => 2,
            4 => 1,
            _ => 0,
        }
    }
    if priority(next) > priority(current) {
        next
    } else {
        current
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    fn choose_destination(engine: &Engine, path: &Path) {
        std::fs::create_dir_all(path).unwrap();
        let mut config = engine.config().unwrap();
        config.free_space_reserve_bytes = 1024 * 1024;
        config.destination = Some(Destination {
            name: String::new(),
            identity: String::new(),
            path: filesystem::display(path),
            id: String::new(),
        });
        let preview = engine.preview(config, "Change destination".into()).unwrap();
        engine.apply_preview(&preview.token).unwrap();
    }
    async fn copy_pass(engine: &Arc<Engine>) {
        engine.scan().unwrap();
        engine.dirty.store(false, Ordering::SeqCst);
        engine.backup_pending().await.unwrap();
        engine.save_catalog().await.unwrap();
    }
    #[tokio::test]
    async fn folder_backup_restore_versions_exclusions_switch_and_restart() {
        let (temp, engine, source) = fixture();
        select(&engine, &source);
        let selections = serde_json::to_value(engine.config().unwrap().roots).unwrap();
        let rules = serde_json::to_value(engine.config().unwrap().rules).unwrap();
        choose_destination(&engine, &temp.path().join("destination-a"));
        copy_pass(&engine).await;
        let config = engine.config().unwrap();
        let item = engine
            .items()
            .unwrap()
            .into_iter()
            .find(|i| i.relative_path == "keep.txt")
            .unwrap();
        let first = item.current.clone().unwrap();
        assert_eq!(engine.snapshot().await.unwrap().counts.backed_up, 1);
        let readable = folder::resolve(
            &config,
            &folder::reference(
                &config,
                &folder::latest_relative(&config, &config.roots[0], &item).unwrap(),
            )
            .unwrap(),
        )
        .unwrap();
        assert_eq!(std::fs::read(&readable).unwrap(), b"original");
        assert!(!readable.with_file_name("skip.tmp").exists());
        std::fs::write(source.join("keep.txt"), b"next version").unwrap();
        copy_pass(&engine).await;
        let current = engine
            .items()
            .unwrap()
            .into_iter()
            .find(|i| i.id == item.id)
            .unwrap();
        assert_eq!(current.history.len(), 1);
        assert_eq!(std::fs::read(&readable).unwrap(), b"next version");
        assert!(folder::matches(&config, &first).unwrap());
        let restore = temp.path().join("restored");
        std::fs::create_dir(&restore).unwrap();
        std::fs::write(restore.join("keep.txt"), b"retain me").unwrap();
        let restored = engine
            .restore(first.clone(), "keep.txt".into(), restore.clone())
            .unwrap();
        assert_eq!(std::fs::read(restored).unwrap(), b"original");
        assert_eq!(
            std::fs::read(restore.join("keep.txt")).unwrap(),
            b"retain me"
        );
        let catalogs = engine.catalogs().await.unwrap();
        let catalog = engine
            .read_catalog(catalogs[0]["id"].as_str().unwrap())
            .await
            .unwrap();
        assert_eq!(catalog.schema_version, 2);
        choose_destination(&engine, &temp.path().join("destination-b"));
        assert_eq!(engine.snapshot().await.unwrap().counts.backed_up, 0);
        assert_eq!(
            serde_json::to_value(engine.config().unwrap().roots).unwrap(),
            selections
        );
        assert_eq!(
            serde_json::to_value(engine.config().unwrap().rules).unwrap(),
            rules
        );
        copy_pass(&engine).await;
        assert_eq!(engine.snapshot().await.unwrap().counts.backed_up, 1);
        assert!(folder::matches(&config, &first).unwrap());
        assert!(folder::matches(&engine.config().unwrap(), &first).unwrap());
        let new_destination = engine.config().unwrap().destination;
        drop(engine);
        let reopened =
            Engine::open_with_cache(temp.path().join("state"), Cache::isolated().unwrap()).unwrap();
        assert_eq!(reopened.config().unwrap().destination, new_destination);
        assert_eq!(reopened.snapshot().await.unwrap().counts.backed_up, 1);
    }
    #[tokio::test]
    async fn missing_drive_and_changed_snapshot_never_report_success() {
        let (temp, engine, source) = fixture();
        select(&engine, &source);
        choose_destination(&engine, &temp.path().join("destination"));
        copy_pass(&engine).await;
        let config = engine.config().unwrap();
        let item = engine
            .items()
            .unwrap()
            .into_iter()
            .find(|i| i.current.is_some())
            .unwrap();
        let snapshot = folder::resolve(&config, &item.current.as_ref().unwrap().remote_id).unwrap();
        std::fs::write(snapshot, b"tampered").unwrap();
        engine.check_destination().await.unwrap();
        assert_eq!(engine.snapshot().await.unwrap().counts.issues, 1);
        std::fs::rename(
            temp.path().join("destination"),
            temp.path().join("unplugged"),
        )
        .unwrap();
        assert!(folder::base(&config).is_err());
        assert!(!temp.path().join("destination").exists());
        assert_eq!(std::fs::read(source.join("keep.txt")).unwrap(), b"original");
    }
    #[tokio::test]
    async fn multiple_destinations_have_content_only_and_local_recovery_survives_restart() {
        let (temp, engine, source) = fixture();
        std::fs::write(source.join("keep.txt"), b"first source").unwrap();
        select(&engine, &source);
        choose_destination(&engine, &temp.path().join("disk-a"));
        let first_root = engine.config().unwrap().roots[0].id.clone();
        let first_destination = engine.config().unwrap().destination.unwrap().id;
        choose_destination(&engine, &temp.path().join("disk-b"));
        let second = temp.path().join("second");
        std::fs::create_dir(&second).unwrap();
        std::fs::write(second.join("second.txt"), b"second source").unwrap();
        let review = engine.shell_preview(&second, true).unwrap();
        engine.apply_preview(&review.token).unwrap();
        let mut config = engine.config().unwrap();
        config
            .assignments
            .insert(first_root.clone(), first_destination);
        config.rules.push(Rule {
            id: id(),
            scope: first_root,
            kind: RuleKind::ExactFile,
            action: Action::Exclude,
            value: "private.txt".into(),
            order: 1,
        });
        std::fs::write(source.join("private.txt"), b"private").unwrap();
        let review = engine
            .preview(config, "Assign separate destinations".into())
            .unwrap();
        engine.apply_preview(&review.token).unwrap();
        copy_pass(&engine).await;
        let items = engine.items().unwrap();
        assert_eq!(
            items
                .iter()
                .filter(|i| i.effective_status() == Status::BackedUp)
                .count(),
            2
        );
        for item in items.iter().filter(|i| i.current.is_some()) {
            let path = folder::resolve(
                &engine.config().unwrap(),
                &item.current.as_ref().unwrap().remote_id,
            )
            .unwrap();
            assert!(path
                .to_string_lossy()
                .contains(if item.relative_path == "keep.txt" {
                    "disk-a"
                } else {
                    "disk-b"
                }));
        }
        for disk in ["disk-a", "disk-b"] {
            let files: Vec<_> = walkdir::WalkDir::new(temp.path().join(disk))
                .into_iter()
                .map(|e| e.unwrap())
                .filter(|e| e.file_type().is_file())
                .collect();
            assert_eq!(
                files.len(),
                2,
                "Only readable copy and immutable content snapshot"
            );
            for file in files {
                let text = file.path().to_string_lossy();
                assert!(text.contains("\\files\\") || text.contains("\\versions\\"));
                assert!(!text.contains("private.txt"));
                println!(
                    "Destination content only: {}",
                    file.path().strip_prefix(temp.path()).unwrap().display()
                );
            }
        }
        let catalog_count = engine.catalogs().await.unwrap().as_array().unwrap().len();
        assert!(catalog_count > 0);
        engine
            .save_browser_state(&serde_json::json!({"tabs":["a","b"]}))
            .unwrap();
        drop(engine);
        let reopened =
            Engine::open_with_cache(temp.path().join("state"), Cache::isolated().unwrap()).unwrap();
        assert_eq!(reopened.config().unwrap().destinations.len(), 2);
        assert_eq!(reopened.config().unwrap().assignments.len(), 1);
        assert_eq!(
            reopened.catalogs().await.unwrap().as_array().unwrap().len(),
            catalog_count
        );
        assert_eq!(
            reopened.browser_state().unwrap()["tabs"]
                .as_array()
                .unwrap()
                .len(),
            2
        );
    }
    #[test]
    fn folder_badges_prefer_actionable_states_and_do_not_let_exclusions_hide_backups() {
        assert_eq!(aggregate(1, 4), 1);
        assert_eq!(aggregate(4, 3), 3);
        assert_eq!(aggregate(3, 2), 2);
        assert_eq!(aggregate(2, 5), 5);
    }
    #[tokio::test]
    async fn explorer_destination_chooser_is_transactional_and_keeps_retained_content() {
        let (temp, engine, source) = fixture();
        let archive = temp.path().join("archive-a");
        let second = temp.path().join("archive-b");
        std::fs::create_dir(&archive).unwrap();
        std::fs::create_dir(&second).unwrap();
        let review = engine.backup_selection_preview(Some(&source), None,
            Some(("Office archive".into(), filesystem::display(&archive)))).unwrap();
        assert!(engine.config().unwrap().roots.is_empty(), "Preview must not apply selection");
        assert_eq!(std::fs::read_dir(&archive).unwrap().count(), 0, "Preview must not write destination metadata/content");
        engine.apply_preview(&review.token).unwrap();
        let config = engine.config().unwrap();
        assert_eq!(config.destinations.len(), 1, "First default and assignment share one ID");
        assert_eq!(config.assignments[&config.roots[0].id], config.destinations[0].id);
        let review = engine.backup_selection_preview(None, None,
            Some(("Travel archive".into(), filesystem::display(&second)))).unwrap();
        engine.apply_preview(&review.token).unwrap();
        let target_id = engine.config().unwrap().destinations[1].id.clone();
        let file = temp.path().join("single-file.txt");
        std::fs::write(&file, b"single-file-content").unwrap();
        let review = engine.backup_selection_preview(Some(&file), Some(&target_id), None).unwrap();
        engine.apply_preview(&review.token).unwrap();
        let exclude = engine.shell_preview(&source.join("skip.tmp"), false).unwrap();
        engine.apply_preview(&exclude.token).unwrap();
        copy_pass(&engine).await;
        let before = engine.items().unwrap();
        assert_eq!(before.iter().filter(|i| i.effective_status() == Status::BackedUp).count(), 2);
        let listing = engine.browse(&filesystem::display(&source)).unwrap();
        assert!(listing["entries"].as_array().unwrap().iter().any(|i| i["name"] == "skip.tmp" && i["backup"] == "Excluded"));
        let copies: Vec<_> = before.iter().filter_map(|i| i.current.as_ref()).map(|v| folder::resolve(&engine.config().unwrap(), &v.remote_id).unwrap()).collect();
        let remove = engine.inherit_preview(&source).unwrap();
        engine.apply_preview(&remove.token).unwrap();
        assert_eq!(std::fs::read(source.join("keep.txt")).unwrap(), b"original");
        for copy in &copies { assert!(copy.is_file(), "Removing selection must retain verified content"); }
        let state = serde_json::json!({"tabs":[{"id":"first","path":filesystem::display(&source),"view":"tiles"}],"active":"first","preferences":{"columns":["name","destination"],"widths":{"destination":310},"preview":false}});
        engine.save_browser_state(&state).unwrap();
        drop(engine);
        let reopened = Engine::open_with_cache(temp.path().join("state"), Cache::isolated().unwrap()).unwrap();
        assert_eq!(reopened.browser_state().unwrap(), state);
        assert_eq!(reopened.config().unwrap().destinations.len(), 2);
        assert!(reopened.config().unwrap().roots[0].is_file);
        for disk in [&archive, &second] {
            let files: Vec<_> = walkdir::WalkDir::new(disk).into_iter().map(|e| e.unwrap()).filter(|e| e.file_type().is_file()).collect();
            assert_eq!(files.len(), 2, "Only current content and retained content snapshot");
            for entry in files { let relative = entry.path().strip_prefix(disk).unwrap(); let text=relative.to_string_lossy(); assert!(text.contains("\\files\\") || text.contains("\\versions\\")); println!("Chooser destination content only: {}", relative.display()); }
        }
    }
    #[tokio::test]
    async fn manual_run_completes_folder_and_file_destinations() {
        let (temp, engine, source) = fixture();
        let file = temp.path().join("standalone.txt");
        std::fs::write(&file, b"separate file source").unwrap();
        for (selected, name) in [(&source, "archive-a"), (&file, "archive-b")] {
            let destination = temp.path().join(name);
            std::fs::create_dir(&destination).unwrap();
            let review = engine.backup_selection_preview(Some(selected), None,
                Some((name.into(), filesystem::display(&destination)))).unwrap();
            engine.apply_preview(&review.token).unwrap();
        }
        let mut config = engine.config().unwrap();
        config.continuous = false;
        config.free_space_reserve_bytes = 1024 * 1024;
        let review = engine.preview(config, "Manual fixture backup".into()).unwrap();
        engine.apply_preview(&review.token).unwrap();
        engine.sync_now().unwrap();
        let worker = tokio::spawn(engine.clone().run());
        let outcome = tokio::time::timeout(Duration::from_secs(10), async {
            loop {
                if engine.items().unwrap().iter().filter(|i| i.effective_status() == Status::BackedUp).count() == 3 { break; }
                tokio::time::sleep(Duration::from_millis(100)).await;
            }
        }).await;
        engine.shutdown();
        worker.await.unwrap();
        assert!(outcome.is_ok(), "Manual request left unfinished items: {:?}; {:?}", engine.items().unwrap(), engine.progress.lock().unwrap());
    }
    #[tokio::test]
    async fn manual_run_survives_source_rescan_at_safe_checkpoint() {
        let (temp, engine, source) = fixture();
        std::fs::write(source.join("keep.txt"), vec![7u8; 256 * 1024]).unwrap();
        select(&engine, &source);
        choose_destination(&engine, &temp.path().join("archive"));
        let mut config = engine.config().unwrap();
        config.continuous = false;
        config.upload_limit_kib = Some(512);
        let review = engine.preview(config, "Manual interrupted fixture".into()).unwrap();
        engine.apply_preview(&review.token).unwrap();
        engine.sync_now().unwrap();
        let worker = tokio::spawn(engine.clone().run());
        let outcome = tokio::time::timeout(Duration::from_secs(5), async {
            while engine.progress.lock().unwrap().phase != "syncing" {
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
            // Source changes during a staged copy must invalidate and recapture that job.
            std::fs::write(source.join("keep.txt"), vec![9u8; 256 * 1024]).unwrap();
            engine.rescan();
            loop {
                if engine.items().unwrap().iter().any(|i| i.effective_status() == Status::BackedUp) { break; }
                tokio::time::sleep(Duration::from_millis(50)).await;
            }
        }).await;
        engine.shutdown();
        worker.await.unwrap();
        assert!(outcome.is_ok(), "Source reconciliation must not consume the manual request: {:?}", engine.progress.lock().unwrap());
    }
    pub(super) fn fixture() -> (tempfile::TempDir, Arc<Engine>, PathBuf) {
        let temp = tempfile::tempdir().unwrap();
        let source = temp.path().join("source");
        std::fs::create_dir(&source).unwrap();
        std::fs::write(source.join("keep.txt"), b"original").unwrap();
        std::fs::write(source.join("skip.tmp"), b"temporary").unwrap();
        let engine =
            Engine::open_with_cache(temp.path().join("state"), Cache::isolated().unwrap()).unwrap();
        (temp, engine, source)
    }
    pub(super) fn select(engine: &Engine, source: &Path) {
        let mut config = engine.config().unwrap();
        config.roots.push(Root {
            id: "root".into(),
            label: "Test source".into(),
            path: filesystem::display(source),
            volume_identity: String::new(),
            enabled: true,
            is_file: false,
        });
        config.rules.push(Rule {
            id: "rule".into(),
            scope: "global".into(),
            kind: RuleKind::FilenameGlob,
            action: Action::Exclude,
            value: "*.tmp".into(),
            order: 1,
        });
        let preview = engine.preview(config, "Test selection".into()).unwrap();
        assert_eq!((preview.included, preview.excluded), (1, 1));
        engine.apply_preview(&preview.token).unwrap();
    }
    #[tokio::test]
    async fn pause_is_persistent_and_sync_now_cannot_bypass_it() {
        let (temp, engine, _) = fixture();
        engine.pause(Some(0)).unwrap();
        assert!(engine.sync_now().is_err());
        assert_eq!(engine.snapshot().await.unwrap().progress.phase, "paused");
        drop(engine);
        let reopened =
            Engine::open_with_cache(temp.path().join("state"), Cache::isolated().unwrap()).unwrap();
        assert!(reopened.config().unwrap().paused());
        reopened.pause(None).unwrap();
        assert!(reopened.sync_now().is_ok());
    }
    #[test]
    fn stale_preview_does_not_overwrite_newer_configuration() {
        let (_temp, engine, source) = fixture();
        select(&engine, &source);
        let preview = engine
            .preview(engine.config().unwrap(), "Old review".into())
            .unwrap();
        engine.pause(Some(0)).unwrap();
        assert!(engine.apply_preview(&preview.token).is_err());
        assert!(engine.config().unwrap().paused());
    }
    #[test]
    fn scan_verifies_content_even_when_size_and_timestamp_are_preserved() {
        let (_temp, engine, source) = fixture();
        select(&engine, &source);
        engine.scan().unwrap();
        let file = source.join("keep.txt");
        let modified = std::fs::metadata(&file).unwrap().modified().unwrap();
        let mut item = engine
            .items()
            .unwrap()
            .into_iter()
            .find(|i| i.relative_path == "keep.txt")
            .unwrap();
        let (sha, md5, size) = staging::hashes(File::open(&file).unwrap()).unwrap();
        item.current = Some(Version {
            remote_id: "test-id".into(),
            sha256: sha,
            md5,
            size,
            verified_at: now(),
        });
        item.status = Status::BackedUp;
        engine.store.lock().unwrap().put_item(&item).unwrap();
        std::fs::write(&file, b"modified").unwrap();
        File::options()
            .write(true)
            .open(&file)
            .unwrap()
            .set_times(std::fs::FileTimes::new().set_modified(modified))
            .unwrap();
        engine.scan().unwrap();
        let changed = engine
            .store
            .lock()
            .unwrap()
            .item(&item.id)
            .unwrap()
            .unwrap();
        assert_eq!(changed.status, Status::Pending);
        assert!(changed.generation > item.generation);
        assert_eq!(
            engine
                .items()
                .unwrap()
                .iter()
                .find(|i| i.relative_path == "skip.tmp")
                .unwrap()
                .status,
            Status::Excluded
        );
    }
    #[test]
    fn absent_local_files_are_retained() {
        let (_temp, engine, source) = fixture();
        select(&engine, &source);
        engine.scan().unwrap();
        std::fs::remove_file(source.join("keep.txt")).unwrap();
        engine.scan().unwrap();
        let item = engine
            .items()
            .unwrap()
            .into_iter()
            .find(|i| i.relative_path == "keep.txt")
            .unwrap();
        assert_eq!(item.status, Status::Retained);
    }
    #[tokio::test]
    async fn catalog_checkpoints_advance_counts_without_demoting_covered_files() {
        let (_temp, engine, source) = fixture();
        select(&engine, &source);
        std::fs::write(source.join("second.txt"), b"second").unwrap();
        engine.scan().unwrap();
        let mut items = engine
            .items()
            .unwrap()
            .into_iter()
            .filter(|i| i.status != Status::Excluded)
            .collect::<Vec<_>>();
        for (index, item) in items.iter_mut().enumerate() {
            let (sha, md5, size) =
                staging::hashes(File::open(source.join(&item.relative_path)).unwrap()).unwrap();
            item.current = Some(Version {
                remote_id: format!("remote-{index}"),
                sha256: sha,
                md5,
                size,
                verified_at: now(),
            });
            item.status = Status::Pending;
            item.reason = "Copy verified; committing local recovery checkpoint".into();
            engine.store.lock().unwrap().commit_verified(item).unwrap();
        }
        let mut catalog = Catalog {
            schema_version: 1,
            device_id: engine.config().unwrap().device_id,
            committed_at: now(),
            roots: vec![],
            items: vec![items[0].clone()],
        };
        engine
            .store
            .lock()
            .unwrap()
            .commit_catalog(&catalog, "catalog-1", now())
            .unwrap();
        engine.reconcile_cataloged().unwrap();
        let counts = engine.snapshot().await.unwrap().counts;
        assert_eq!((counts.backed_up, counts.pending), (1, 1));
        assert!(engine
            .store
            .lock()
            .unwrap()
            .setting::<bool>("catalogDirty")
            .unwrap()
            .unwrap());
        catalog.items = items.clone();
        engine
            .store
            .lock()
            .unwrap()
            .commit_catalog(&catalog, "catalog-2", now())
            .unwrap();
        engine.reconcile_cataloged().unwrap();
        let counts = engine.snapshot().await.unwrap().counts;
        assert_eq!((counts.backed_up, counts.pending), (2, 0));
        // A resumed old catalog cannot cover a newer version uploaded since it was staged.
        let mut newer = engine
            .store
            .lock()
            .unwrap()
            .item(&items[1].id)
            .unwrap()
            .unwrap();
        newer.current.as_mut().unwrap().remote_id = "newer-copy".into();
        newer.status = Status::Pending;
        engine
            .store
            .lock()
            .unwrap()
            .commit_verified(&newer)
            .unwrap();
        engine
            .store
            .lock()
            .unwrap()
            .commit_catalog(&catalog, "old-resumed-catalog", now())
            .unwrap();
        assert_eq!(engine.snapshot().await.unwrap().counts.backed_up, 1);
        assert!(engine
            .store
            .lock()
            .unwrap()
            .setting::<bool>("catalogDirty")
            .unwrap()
            .unwrap());
    }
    #[test]
    fn cleanup_commits_only_the_selected_version_and_survives_restart() {
        let (temp, engine, source) = fixture();
        select(&engine, &source);
        engine.scan().unwrap();
        let mut item = engine
            .items()
            .unwrap()
            .into_iter()
            .find(|i| i.relative_path == "skip.tmp")
            .unwrap();
        let version = Version {
            remote_id: "current".into(),
            sha256: "hash".into(),
            md5: "md5".into(),
            size: 5,
            verified_at: now(),
        };
        item.current = Some(version.clone());
        item.cataloged_remote_id = Some("current".into());
        let mut older = version;
        older.remote_id = "history".into();
        item.history.push(older);
        {
            let mut store = engine.store.lock().unwrap();
            store.put_item(&item).unwrap();
            store
                .record_cleanup("run", &item.id, "history", "requested")
                .unwrap();
            store.commit_cleanup("run", &item.id, "history").unwrap();
        }
        drop(engine);
        let store = Store::open(&temp.path().join("state/state.sqlite")).unwrap();
        let saved = store.item(&item.id).unwrap().unwrap();
        assert_eq!(saved.current.unwrap().remote_id, "current");
        assert!(saved.history.is_empty());
        assert!(store.setting::<bool>("catalogDirty").unwrap().unwrap());
        assert!(source.join("skip.tmp").exists());
    }
    #[test]
    fn restore_publish_never_overwrites_existing_destination() {
        let (temp, _engine, _) = fixture();
        let original = temp.path().join("original.txt");
        let downloaded = temp.path().join("download.tmp");
        filesystem::write_durable(&original, b"keep me").unwrap();
        filesystem::write_durable(&downloaded, b"restore").unwrap();
        assert!(filesystem::move_new(&downloaded, &original).is_err());
        assert_eq!(std::fs::read(original).unwrap(), b"keep me");
    }
}
