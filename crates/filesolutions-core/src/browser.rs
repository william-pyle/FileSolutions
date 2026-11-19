use crate::{model::Config, rules::Matcher};
use anyhow::{Context, Result};
use filesolutions_windows::filesystem;
use serde::Serialize;
use std::os::windows::fs::MetadataExt;
use std::path::Path;

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Entry {
    name: String,
    path: String,
    directory: bool,
    hidden: bool,
    size: u64,
    modified: u64,
    kind: String,
    backup: String,
    reason: String,
    destination: String,
}
pub fn list(path: &str, config: &Config) -> Result<serde_json::Value> {
    if path.is_empty() {
        let mut drives = vec![];
        let mask = unsafe { windows::Win32::Storage::FileSystem::GetLogicalDrives() };
        for n in 0..26 {
            if mask & (1 << n) != 0 {
                drives.push(format!("{}:\\", (b'A' + n) as char));
            }
        }
        return Ok(serde_json::json!({"path":"","parent":null,"entries":[],"drives":drives}));
    }
    let path = std::fs::canonicalize(path).context("Folder is unavailable or access was denied")?;
    anyhow::ensure!(path.is_dir(), "Choose a directory");
    let matcher = Matcher::new(&config.rules)?;
    let mut entries = vec![];
    for entry in std::fs::read_dir(&path)? {
        let entry = entry?;
        let meta = entry.metadata()?;
        let full = entry.path();
        let name = entry.file_name().to_string_lossy().into_owned();
        let mut backup = "Not selected".to_string();
        let mut reason = "No backup source includes this item".to_string();
        let mut destination = String::new();
        if let Some(root) = config
            .roots
            .iter()
            .find(|r| filesystem::contains(Path::new(&r.path), &full))
        {
            let root_path = std::fs::canonicalize(&root.path)?;
            let relative = if root.is_file {
                name.clone()
            } else {
                full.strip_prefix(&root_path)?
                    .to_string_lossy()
                    .replace('\\', "/")
            };
            let decision = if relative.is_empty() {
                crate::rules::Decision {
                    included: true,
                    winning_rule: None,
                    reason: "Explicit source selection".into(),
                }
            } else {
                matcher.decide(&root.id, &relative)?
            };
            backup = if !root.enabled {
                "Paused source"
            } else if decision.included {
                "Included"
            } else {
                "Excluded"
            }
            .into();
            reason = decision.reason;
            destination = config
                .for_root(&root.id)
                .destination
                .map(|d| d.name)
                .unwrap_or_else(|| "Unassigned".into());
        }
        entries.push(Entry {
            name,
            path: filesystem::display(&full),
            directory: meta.is_dir(),
            hidden: meta.file_attributes() & 2 != 0,
            size: meta.len(),
            modified: meta
                .modified()?
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_secs(),
            kind: if meta.is_dir() {
                "Folder".into()
            } else {
                full.extension()
                    .unwrap_or_default()
                    .to_string_lossy()
                    .to_uppercase()
            },
            backup,
            reason,
            destination,
        });
    }
    Ok(
        serde_json::json!({"path":filesystem::display(&path),"parent":path.parent().map(filesystem::display),"entries":entries,"drives":[]}),
    )
}
