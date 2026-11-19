//! Provider-independent, verified backups. Only the selected destination's
//! FileSolutions directory is managed; the provider's cloud-upload status is unknown.
use crate::{model::*, staging};
use anyhow::{Context, Result};
use filesolutions_windows::filesystem;
use std::{
    fs::{File, OpenOptions},
    io::{Read, Write},
    path::{Path, PathBuf},
};

fn guard_directories(path: &Path) -> Result<Vec<File>> {
    use std::os::windows::{fs::OpenOptionsExt, io::AsRawHandle};
    use windows::Win32::{Foundation::HANDLE, Storage::FileSystem::*};
    let mut current = PathBuf::new();
    let mut handles = vec![];
    for part in path.components() {
        current.push(part);
        if matches!(part, std::path::Component::Prefix(_)) {
            continue;
        }
        if !current.try_exists()? {
            std::fs::create_dir(&current)?;
        }
        let file = OpenOptions::new()
            .access_mode(FILE_READ_ATTRIBUTES.0)
            .share_mode(FILE_SHARE_READ.0 | FILE_SHARE_WRITE.0)
            .custom_flags(FILE_FLAG_BACKUP_SEMANTICS.0 | FILE_FLAG_OPEN_REPARSE_POINT.0)
            .open(&current)?;
        let mut tag = FILE_ATTRIBUTE_TAG_INFO::default();
        unsafe {
            GetFileInformationByHandleEx(
                HANDLE(file.as_raw_handle()),
                FileAttributeTagInfo,
                (&mut tag as *mut FILE_ATTRIBUTE_TAG_INFO).cast(),
                std::mem::size_of_val(&tag) as u32,
            )?;
        }
        anyhow::ensure!(
            file.metadata()?.is_dir() && tag.ReparseTag & 0x20000000 == 0,
            "Destination contains a link or non-directory"
        );
        handles.push(file);
    }
    Ok(handles)
}

fn component(value: &str) -> Result<()> {
    let path = filesystem::safe_relative(value)?;
    anyhow::ensure!(path.components().count() == 1, "Invalid backup identifier");
    Ok(())
}

// Cloud placeholders are allowed here; links/junctions that redirect names are not.
pub fn check_path(path: &Path) -> Result<()> {
    use std::os::windows::{
        fs::{MetadataExt, OpenOptionsExt},
        io::AsRawHandle,
    };
    use windows::Win32::{Foundation::HANDLE, Storage::FileSystem::*};
    let mut current = PathBuf::new();
    for part in path.components() {
        current.push(part);
        if matches!(part, std::path::Component::Prefix(_)) {
            continue;
        }
        match std::fs::symlink_metadata(&current) {
            Ok(meta) if meta.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT.0 != 0 => {
                let file = OpenOptions::new()
                    .read(true)
                    .share_mode(FILE_SHARE_READ.0 | FILE_SHARE_WRITE.0)
                    .custom_flags(FILE_FLAG_BACKUP_SEMANTICS.0 | FILE_FLAG_OPEN_REPARSE_POINT.0)
                    .open(&current)?;
                let mut tag = FILE_ATTRIBUTE_TAG_INFO::default();
                unsafe {
                    GetFileInformationByHandleEx(
                        HANDLE(file.as_raw_handle()),
                        FileAttributeTagInfo,
                        (&mut tag as *mut FILE_ATTRIBUTE_TAG_INFO).cast(),
                        std::mem::size_of_val(&tag) as u32,
                    )?;
                }
                anyhow::ensure!(
                    tag.ReparseTag & 0x20000000 == 0,
                    "Backup destination cannot contain symbolic links or junctions"
                );
            }
            Ok(_) => {}
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => return Err(e.into()),
        }
    }
    Ok(())
}

/// Resolve a selection without initializing a destination or changing settings.
pub fn validate(path: &Path, config: &Config, state: &Path) -> Result<PathBuf> {
    anyhow::ensure!(
        path.is_absolute(),
        "Choose an absolute folder or network share path"
    );
    check_path(path)?;
    let path = std::fs::canonicalize(path)
        .context("Backup destination is unavailable. Connect the drive or network share first.")?;
    anyhow::ensure!(path.is_dir(), "Choose an existing destination folder");
    let state = std::fs::canonicalize(state).unwrap_or_else(|_| state.to_path_buf());
    for other in
        std::iter::once(state.as_path()).chain(config.roots.iter().map(|r| Path::new(&r.path)))
    {
        let resolved = std::fs::canonicalize(other).unwrap_or_else(|_| other.to_path_buf());
        anyhow::ensure!(!filesystem::contains(&resolved, &path) && !filesystem::contains(&path, &resolved),
            "The backup destination must be separate from all source folders and FileSolutions's app data");
    }
    Ok(path)
}

pub fn select(path: &Path, config: &Config, state: &Path) -> Result<Destination> {
    let path = validate(path, config, state)?;
    let identity = filesystem::directory_identity(&path)?;
    Ok(Destination {
        name: path
            .file_name()
            .unwrap_or_default()
            .to_string_lossy()
            .into(),
        path: filesystem::display(&path),
        id: id(),
        identity,
    })
}
/// Validate again at apply time. Destination content is created only during backup.
pub fn initialize(destination: &Destination, config: &Config, state: &Path) -> Result<()> {
    let path = validate(Path::new(&destination.path), config, state)?;
    anyhow::ensure!(
        filesystem::directory_identity(&path)? == destination.identity,
        "Destination changed since review; select it again"
    );
    Ok(())
}
pub fn base(config: &Config) -> Result<PathBuf> {
    let destination = config
        .destination
        .as_ref()
        .context("Choose a backup destination")?;
    let path = Path::new(&destination.path);
    anyhow::ensure!(
        filesystem::directory_identity(path)? == destination.identity,
        "Destination unavailable or replaced. Reconnect the original drive/folder."
    );
    Ok(std::fs::canonicalize(path)?.join("FileSolutions"))
}
pub fn reference(config: &Config, relative: &str) -> Result<String> {
    filesystem::safe_relative(relative)?;
    Ok(format!(
        "folder:{}:{relative}",
        config.destination.as_ref().context("No destination")?.id
    ))
}
pub fn resolve(config: &Config, reference: &str) -> Result<PathBuf> {
    let tail = reference
        .strip_prefix("folder:")
        .context("Invalid version reference")?;
    let (id, relative) = tail.split_once(':').context("Invalid version reference")?;
    let destination = config
        .destinations
        .iter()
        .chain(config.destination.iter())
        .find(|d| d.id == id)
        .context("Destination record missing from local database")?;
    let mut selected = config.clone();
    selected.destination = Some(destination.clone());
    let path = base(&selected)?.join(filesystem::safe_relative(relative)?);
    check_path(&path)?;
    Ok(path)
}
pub fn latest_relative(config: &Config, root: &Root, item: &Item) -> Result<String> {
    component(&config.device_id)?;
    component(&root.id)?;
    // The stable ID prevents equally named sources from colliding. Labels are display-only.
    let name: String = root
        .label
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' || c == '_' {
                c
            } else {
                '_'
            }
        })
        .take(48)
        .collect();
    let relative = format!(
        "{}/files/{}-{}/{}",
        config.device_id, name, root.id, item.relative_path
    );
    filesystem::safe_relative(&relative)?;
    Ok(relative)
}

/// Temp file, fsync, read-back hash, then rename on the destination filesystem.
/// Source files and existing snapshots are never removed by a failed transfer.
pub fn copy_verified(
    source: &Path,
    target: &Path,
    sha: &str,
    size: u64,
    replace: bool,
    mut checkpoint: impl FnMut(u64) -> Result<()>,
) -> Result<()> {
    check_path(target)?;
    let parent = target.parent().context("Invalid destination path")?;
    let _handles = guard_directories(parent)?;
    let partial = parent.join(format!(".filesolutions-{}.partial", id()));
    let result = (|| -> Result<()> {
        let mut input = File::open(source)?;
        let mut output = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&partial)?;
        let mut buffer = vec![0u8; 1024 * 1024];
        let mut total = 0;
        loop {
            checkpoint(total)?;
            let count = input.read(&mut buffer)?;
            if count == 0 {
                break;
            }
            output.write_all(&buffer[..count])?;
            total += count as u64;
            anyhow::ensure!(total <= size, "Snapshot size changed");
        }
        output.sync_all()?;
        drop(output);
        let (actual, _, length) = staging::hashes(File::open(&partial)?)?;
        anyhow::ensure!(
            actual == sha && length == size,
            "Copied bytes failed SHA-256 verification"
        );
        checkpoint(total)?;
        check_path(target)?;
        if replace {
            filesystem::atomic_replace(&partial, target)?;
        } else {
            filesystem::move_new(&partial, target)?;
        }
        Ok(())
    })();
    let _ = std::fs::remove_file(&partial);
    result
}
pub fn matches(config: &Config, version: &Version) -> Result<bool> {
    let path = resolve(config, &version.remote_id)?;
    if !path.try_exists()? {
        return Ok(false);
    }
    let (sha, _, size) = staging::hashes(File::open(path)?)?;
    Ok(sha == version.sha256 && size == version.size)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn interrupted_copy_and_hash_failure_keep_existing_target() {
        let t = tempfile::tempdir().unwrap();
        let source = t.path().join("source");
        let target = t.path().join("target");
        std::fs::write(&source, b"new contents").unwrap();
        std::fs::write(&target, b"original").unwrap();
        let (sha, _, size) = staging::hashes(File::open(&source).unwrap()).unwrap();
        assert!(
            copy_verified(&source, &target, &sha, size, true, |_| anyhow::bail!(
                "Paused"
            ))
            .is_err()
        );
        assert!(copy_verified(&source, &target, &"0".repeat(64), size, true, |_| Ok(())).is_err());
        std::fs::write(&source, vec![7u8; 3 * 1024 * 1024]).unwrap();
        let (sha, _, size) = staging::hashes(File::open(&source).unwrap()).unwrap();
        let mut checkpoints = 0;
        assert!(copy_verified(&source, &target, &sha, size, true, |bytes| {
            checkpoints += 1;
            anyhow::ensure!(bytes == 0, "Cancel after writing the first block");
            Ok(())
        })
        .is_err());
        assert!(checkpoints >= 2);

        assert_eq!(std::fs::read(target).unwrap(), b"original");
        assert_eq!(std::fs::read_dir(t.path()).unwrap().count(), 2);
    }
    #[test]
    fn overlaps_traversal_and_substituted_identity_are_rejected() {
        let t = tempfile::tempdir().unwrap();
        let state = t.path().join("state");
        let source = t.path().join("source");
        let dest = t.path().join("destination");
        for p in [&state, &source, &dest] {
            std::fs::create_dir(p).unwrap();
        }
        let mut c = Config::default();
        c.roots.push(Root {
            id: "root".into(),
            label: "Source".into(),
            path: filesystem::display(&source),
            volume_identity: String::new(),
            enabled: true,
            is_file: false,
        });
        assert!(validate(&source, &c, &state).is_err());
        assert!(validate(t.path(), &c, &state).is_err());
        let d = select(&dest, &c, &state).unwrap();
        initialize(&d, &c, &state).unwrap();
        c.destination = Some(d.clone());
        for reference in [
            format!("folder:{}:../escape", d.id),
            format!("folder:{}:C:/escape", d.id),
            "legacy-google-id".into(),
        ] {
            assert!(resolve(&c, &reference).is_err());
        }
        c.destination.as_mut().unwrap().identity = id();
        assert!(base(&c).is_err());
    }
}
