//! File transfers publish complete trees before a cross-volume move deletes any source.
use crate::{folder, model::id, staging};
use anyhow::{Context, Result};
use filesolutions_windows::filesystem;
use std::os::windows::{fs::OpenOptionsExt, io::AsRawHandle};
use std::{
    fs::{File, OpenOptions},
    path::{Path, PathBuf},
    sync::atomic::{AtomicBool, Ordering},
};
use windows::Win32::{Foundation::HANDLE, Storage::FileSystem::*};

pub fn transfer(
    sources: &[String],
    destination: &Path,
    moving: bool,
    cancel: &AtomicBool,
) -> Result<Vec<String>> {
    anyhow::ensure!(!sources.is_empty(), "Select at least one item");
    filesystem::no_reparse(destination)?;
    let destination = std::fs::canonicalize(destination)?;
    anyhow::ensure!(destination.is_dir(), "Destination is not a folder");
    let _destination_guard = filesystem::guarded_directories(&destination)?;
    let mut names = std::collections::HashSet::new();
    let mut prepared: Vec<(PathBuf, PathBuf)> = vec![];
    for source in sources {
        filesystem::no_reparse(Path::new(source))?;
        let source = std::fs::canonicalize(source)?;
        let name = source
            .file_name()
            .context("A drive root cannot be transferred")?;
        anyhow::ensure!(
            names.insert(name.to_string_lossy().to_lowercase()),
            "Selected items have the same name"
        );
        let target = destination.join(name);
        anyhow::ensure!(
            !target.exists(),
            "Name collision: {}. Rename the existing item or choose another folder.",
            filesystem::display(&target)
        );
        anyhow::ensure!(
            !filesystem::contains(&source, &destination),
            "Cannot copy a folder into itself"
        );
        for (other, _) in &prepared {
            anyhow::ensure!(
                !filesystem::contains(other, &source) && !filesystem::contains(&source, other),
                "Selected paths overlap"
            );
        }
        prepared.push((source, target));
    }
    let checkpoint = || -> Result<()> {
        anyhow::ensure!(
            !cancel.load(Ordering::SeqCst),
            "Transfer cancelled; original files retained"
        );
        Ok(())
    };
    let mut completed = vec![];
    for (source, target) in prepared {
        checkpoint()?;
        let stage = destination.join(format!(".filesolutions-transfer-{}", id()));
        let is_dir = source.is_dir();
        let mut files: Vec<(PathBuf, File, PathBuf)> = vec![];
        let mut directories: Vec<PathBuf> = vec![];
        let result = (|| -> Result<()> {
            let paths = if is_dir {
                walkdir::WalkDir::new(&source)
                    .follow_links(false)
                    .into_iter()
                    .map(|e| Ok(e?.into_path()))
                    .collect::<Result<Vec<_>>>()?
            } else {
                vec![source.clone()]
            };
            // Lock every source file against changes and replacement before copying.
            for path in paths {
                checkpoint()?;
                filesystem::no_reparse(&path)?;
                let relative = if is_dir {
                    path.strip_prefix(&source)?.to_path_buf()
                } else {
                    PathBuf::new()
                };
                let output = if relative.as_os_str().is_empty() {
                    stage.clone()
                } else {
                    stage.join(relative)
                };
                if path.is_dir() {
                    directories.push(path);
                    std::fs::create_dir_all(&output)?;
                } else {
                    filesystem::ordinary_stream_only(&path)?;
                    let file = OpenOptions::new()
                        .access_mode(FILE_GENERIC_READ.0 | if moving { DELETE.0 } else { 0 })
                        .share_mode(FILE_SHARE_READ.0)
                        .custom_flags(FILE_FLAG_OPEN_REPARSE_POINT.0)
                        .open(&path)?;
                    files.push((path, file, output));
                }
            }
            for (path, file, output) in &files {
                let (hash, _, size) = staging::hashes(file.try_clone()?)?;
                folder::copy_verified(path, output, &hash, size, false, |_| checkpoint())?;
                let metadata = file.metadata()?;
                OpenOptions::new().write(true).open(output)?.set_times(
                    std::fs::FileTimes::new()
                        .set_modified(metadata.modified()?)
                        .set_accessed(metadata.accessed()?),
                )?;
            }
            checkpoint()?;
            filesystem::move_new(&stage, &target)?;
            // Complete verified destination exists before any deletion. Cancellation
            // after publication keeps both copies; deletion never precedes publication.
            if moving {
                checkpoint()?;
                for (_, file, _) in &files {
                    let disposition = FILE_DISPOSITION_INFO {
                        DeleteFile: true.into(),
                    };
                    unsafe {
                        SetFileInformationByHandle(
                            HANDLE(file.as_raw_handle()),
                            FileDispositionInfo,
                            (&disposition as *const FILE_DISPOSITION_INFO).cast(),
                            std::mem::size_of_val(&disposition) as u32,
                        )?;
                    }
                }
                files.clear();
                for dir in directories.iter().rev() {
                    std::fs::remove_dir(dir)?;
                }
            }
            Ok(())
        })();
        if stage.exists() {
            // Only our random unpublished staging path, never a selected path.
            anyhow::ensure!(
                stage.parent() == Some(destination.as_path()),
                "Invalid staging path"
            );
            if stage.is_dir() {
                std::fs::remove_dir_all(&stage)?;
            } else {
                std::fs::remove_file(&stage)?;
            }
        }
        result.with_context(|| format!("Transfer of {} stopped. {} earlier item(s) completed; a published copy is retained if source cleanup failed", filesystem::display(&source), completed.len()))?;
        completed.push(filesystem::display(&target));
    }
    Ok(completed)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn alternate_stream_move_is_refused_without_losing_source() {
        let t = tempfile::tempdir().unwrap();
        let source = t.path().join("source.txt");
        let destination = t.path().join("destination");
        std::fs::create_dir(&destination).unwrap();
        std::fs::write(&source, b"ordinary data").unwrap();
        let stream = format!("{}:extra", source.display());
        std::fs::write(&stream, b"must not disappear").unwrap();
        let result = transfer(
            &[filesystem::display(&source)],
            &destination,
            true,
            &AtomicBool::new(false),
        );
        assert!(result.is_err());
        assert_eq!(std::fs::read(&stream).unwrap(), b"must not disappear");
        assert_eq!(std::fs::read(&source).unwrap(), b"ordinary data");
        assert_eq!(std::fs::read_dir(destination).unwrap().count(), 0);
    }
    #[test]
    fn copy_move_collision_and_cancel_preserve_bytes() {
        let t = tempfile::tempdir().unwrap();
        let a = t.path().join("a");
        let b = t.path().join("b");
        std::fs::create_dir(&a).unwrap();
        std::fs::create_dir(&b).unwrap();
        let source = a.join("folder");
        std::fs::create_dir(&source).unwrap();
        std::fs::write(source.join("file.txt"), b"verified contents").unwrap();
        let paths = vec![filesystem::display(&source)];
        let cancel = AtomicBool::new(true);
        assert!(transfer(&paths, &b, true, &cancel).is_err());
        assert!(source.join("file.txt").exists());
        cancel.store(false, Ordering::SeqCst);
        transfer(&paths, &b, false, &cancel).unwrap();
        assert!(transfer(&paths, &b, true, &cancel).is_err());
        assert!(source.exists());
        let c = t.path().join("c");
        std::fs::create_dir(&c).unwrap();
        transfer(&paths, &c, true, &cancel).unwrap();
        assert!(!source.exists());
        assert_eq!(
            std::fs::read(c.join("folder/file.txt")).unwrap(),
            b"verified contents"
        );
        assert_eq!(std::fs::read_dir(&c).unwrap().count(), 1);
    }
}
