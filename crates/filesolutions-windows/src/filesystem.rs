use anyhow::{bail, Context, Result};
use std::io::Write;
use std::{
    fs::{File, OpenOptions},
    os::windows::{
        fs::{MetadataExt, OpenOptionsExt},
        io::AsRawHandle,
    },
    path::{Path, PathBuf},
};
use windows::{
    core::{BOOL, PCWSTR},
    Win32::{Foundation::HANDLE, Storage::FileSystem::*, UI::WindowsAndMessaging::*},
};

pub fn display(path: &Path) -> String {
    let value = path.to_string_lossy();
    if let Some(unc) = value.strip_prefix(r"\\?\UNC\") {
        format!(r"\\{unc}")
    } else {
        value.trim_start_matches(r"\\?\").to_string()
    }
}
pub fn key(path: &Path) -> String {
    display(path)
        .replace('/', "\\")
        .trim_end_matches('\\')
        .to_uppercase()
}
pub fn contains(parent: &Path, child: &Path) -> bool {
    let p = key(parent);
    let c = key(child);
    c == p || c.starts_with(&(p + "\\"))
}
pub fn no_reparse(path: &Path) -> Result<()> {
    let mut current = PathBuf::new();
    for component in path.components() {
        current.push(component);
        if let Ok(meta) = std::fs::symlink_metadata(&current) {
            if meta.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT.0 != 0 {
                bail!("Links, junctions and cloud placeholders are not supported backup roots");
            }
        }
    }
    Ok(())
}
pub fn eligible(path: &Path) -> Result<()> {
    let meta = std::fs::symlink_metadata(path)?;
    let attributes = meta.file_attributes();
    if attributes
        & (FILE_ATTRIBUTE_REPARSE_POINT.0 | FILE_ATTRIBUTE_OFFLINE.0 | FILE_ATTRIBUTE_ENCRYPTED.0)
        != 0
    {
        bail!("Link, offline placeholder or EFS file is not supported");
    }
    if meta.is_dir() {
        let file = OpenOptions::new()
            .read(true)
            .share_mode(FILE_SHARE_READ.0 | FILE_SHARE_WRITE.0 | FILE_SHARE_DELETE.0)
            .custom_flags(FILE_FLAG_BACKUP_SEMANTICS.0 | FILE_FLAG_OPEN_REPARSE_POINT.0)
            .open(path)?;
        ensure_case_insensitive(&file)?;
    }
    Ok(())
}
pub fn validate_root(path: &Path, state: &Path) -> Result<(PathBuf, String, bool)> {
    if !path.is_absolute() {
        bail!("Choose an absolute local path");
    }
    no_reparse(path)?;
    let path = std::fs::canonicalize(path).context("Folder is unavailable")?;
    if contains(state, &path) || contains(&path, state) {
        bail!("Choose a folder that does not contain FileSolutions's own state directory");
    }
    let text = key(&path);
    if text.starts_with("UNC\\") || text.starts_with("\\\\") {
        bail!("Network roots are not supported");
    }
    unsafe {
        let mut volume = [0u16; 32768];
        GetVolumePathNameW(PCWSTR(crate::wide(&path).as_ptr()), &mut volume)?;
        if GetDriveTypeW(PCWSTR(volume.as_ptr())) != 3 {
            bail!("Release 1 supports fixed local NTFS volumes");
        }
        let mut fs_name = [0u16; 32];
        let mut serial = 0;
        GetVolumeInformationW(
            PCWSTR(volume.as_ptr()),
            None,
            Some(&mut serial),
            None,
            None,
            Some(&mut fs_name),
        )?;
        let fs = String::from_utf16_lossy(
            &fs_name[..fs_name
                .iter()
                .position(|n| *n == 0)
                .unwrap_or(fs_name.len())],
        );
        if fs != "NTFS" {
            bail!("Choose a folder on NTFS");
        }
        let handle = OpenOptions::new()
            .read(true)
            .share_mode(FILE_SHARE_READ.0 | FILE_SHARE_WRITE.0 | FILE_SHARE_DELETE.0)
            .custom_flags(FILE_FLAG_BACKUP_SEMANTICS.0 | FILE_FLAG_OPEN_REPARSE_POINT.0)
            .open(&path)?;
        let mut info = BY_HANDLE_FILE_INFORMATION::default();
        GetFileInformationByHandle(HANDLE(handle.as_raw_handle()), &mut info)?;
        if handle.metadata()?.is_dir() {
            ensure_case_insensitive(&handle)?;
        }
        Ok((
            path,
            format!(
                "{serial:08X}-{:08X}{:08X}",
                info.nFileIndexHigh, info.nFileIndexLow
            ),
            handle.metadata()?.is_file(),
        ))
    }
}
fn ensure_case_insensitive(file: &File) -> Result<()> {
    unsafe {
        let mut flags = FILE_CASE_SENSITIVE_INFO::default();
        GetFileInformationByHandleEx(
            HANDLE(file.as_raw_handle()),
            FileCaseSensitiveInfo,
            (&mut flags as *mut FILE_CASE_SENSITIVE_INFO).cast(),
            std::mem::size_of_val(&flags) as u32,
        )?;
        if flags.Flags & 1 != 0 {
            bail!("Case-sensitive folders are not supported");
        }
        Ok(())
    }
}
pub fn locked_source(path: &Path, root: &Path) -> Result<File> {
    no_reparse(path)?;
    eligible(path)?;
    let file = OpenOptions::new()
        .read(true)
        .share_mode(FILE_SHARE_READ.0)
        .custom_flags(FILE_FLAG_OPEN_REPARSE_POINT.0 | FILE_FLAG_SEQUENTIAL_SCAN.0)
        .open(path)
        .context("File is open for writing or cannot be read")?;
    unsafe {
        let mut result = vec![0u16; 32768];
        let len = GetFinalPathNameByHandleW(
            HANDLE(file.as_raw_handle()),
            &mut result,
            GETFINALPATHNAMEBYHANDLE_FLAGS(FILE_NAME_NORMALIZED.0 | VOLUME_NAME_DOS.0),
        );
        if len == 0 || len as usize >= result.len() {
            bail!("Cannot verify the opened file location");
        }
        let actual = PathBuf::from(String::from_utf16_lossy(&result[..len as usize]));
        if !contains(root, &actual) {
            bail!("File moved outside its selected root");
        }
    }
    Ok(file)
}
pub fn free_space(path: &Path) -> Result<u64> {
    unsafe {
        let mut available = 0;
        GetDiskFreeSpaceExW(
            PCWSTR(crate::wide(path).as_ptr()),
            Some(&mut available),
            None,
            None,
        )?;
        Ok(available)
    }
}
pub fn atomic_replace(from: &Path, to: &Path) -> Result<()> {
    unsafe {
        MoveFileExW(
            PCWSTR(crate::wide(from).as_ptr()),
            PCWSTR(crate::wide(to).as_ptr()),
            MOVEFILE_REPLACE_EXISTING | MOVEFILE_WRITE_THROUGH,
        )?;
    }
    Ok(())
}
pub fn write_durable(path: &Path, bytes: &[u8]) -> Result<()> {
    let mut file = OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .open(path)?;
    file.write_all(bytes)?;
    file.sync_all()?;
    Ok(())
}
/// Publish a restore without ever replacing a destination that appeared meanwhile.
pub fn move_new(from: &Path, to: &Path) -> Result<()> {
    unsafe {
        MoveFileExW(
            PCWSTR(crate::wide(from).as_ptr()),
            PCWSTR(crate::wide(to).as_ptr()),
            MOVEFILE_WRITE_THROUGH,
        )?;
    }
    Ok(())
}
/// Hold no-delete handles to every parent while creating/writing a restore.
/// Opening reparse points themselves lets us reject junctions before following them.
pub fn guarded_directories(path: &Path) -> Result<Vec<File>> {
    let mut held = Vec::new();
    let mut current = PathBuf::new();
    for component in path.components() {
        current.push(component);
        if matches!(component, std::path::Component::Prefix(_)) {
            continue;
        }
        if !current.has_root() {
            continue;
        }
        if !current.exists() {
            std::fs::create_dir(&current)?;
        }
        let file = OpenOptions::new()
            .access_mode(FILE_READ_ATTRIBUTES.0)
            .share_mode(FILE_SHARE_READ.0 | FILE_SHARE_WRITE.0)
            .custom_flags(FILE_FLAG_BACKUP_SEMANTICS.0 | FILE_FLAG_OPEN_REPARSE_POINT.0)
            .open(&current)
            .with_context(|| format!("Cannot protect restore directory {}", display(&current)))?;
        let metadata = file.metadata()?;
        anyhow::ensure!(
            metadata.is_dir() && metadata.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT.0 == 0,
            "Restore path contains a link or non-directory"
        );
        ensure_case_insensitive(&file)?;
        held.push(file);
    }
    Ok(held)
}
pub fn reduced_motion() -> bool {
    unsafe {
        let mut enabled = BOOL(1);
        SystemParametersInfoW(
            SPI_GETCLIENTAREAANIMATION,
            0,
            Some((&mut enabled as *mut BOOL).cast()),
            SYSTEM_PARAMETERS_INFO_UPDATE_FLAGS(0),
        )
        .is_ok()
            && !enabled.as_bool()
    }
}
pub fn safe_relative(value: &str) -> Result<PathBuf> {
    let value = value.replace('\\', "/");
    if value.is_empty() || value.starts_with('/') || value.contains(':') {
        bail!("Unsafe restore path");
    }
    for part in value.split('/') {
        let stem = part.split('.').next().unwrap_or("").to_uppercase();
        if part.is_empty()
            || part == "."
            || part == ".."
            || part.ends_with(['.', ' '])
            || part.chars().any(|c| c < ' ' || "<>\"|?*".contains(c))
            || ["CON", "PRN", "AUX", "NUL"].contains(&stem.as_str())
            || (stem.len() == 4
                && (stem.starts_with("COM") || stem.starts_with("LPT"))
                && stem.as_bytes()[3].is_ascii_digit())
        {
            bail!("Unsafe Windows filename in restore catalog");
        }
    }
    Ok(PathBuf::from(value.replace('/', "\\")))
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn containment() {
        assert!(contains(Path::new("C:/Data"), Path::new("c:/data/a")));
        assert!(!contains(Path::new("C:/Data"), Path::new("C:/Database/a")));
    }
    #[test]
    fn restore_paths() {
        for p in [
            "../file",
            "C:/file",
            "x:secret",
            "dir/CON.txt",
            "a/../b",
            "dir/a.",
            "a//b",
        ] {
            assert!(safe_relative(p).is_err(), "{p}");
        }
        assert!(safe_relative("Photos/Family.jpg").is_ok());
    }
}

/// OS volume + 128-bit directory identity; no marker is written to a destination.
/// Filesystems that cannot supply this identity fail closed.
pub fn directory_identity(path: &Path) -> Result<String> {
    no_reparse(path)?;
    let file = OpenOptions::new()
        .access_mode(FILE_READ_ATTRIBUTES.0)
        .share_mode(FILE_SHARE_READ.0 | FILE_SHARE_WRITE.0)
        .custom_flags(FILE_FLAG_BACKUP_SEMANTICS.0 | FILE_FLAG_OPEN_REPARSE_POINT.0)
        .open(path)?;
    anyhow::ensure!(file.metadata()?.is_dir(), "Destination is not a directory");
    let mut info = FILE_ID_INFO::default();
    unsafe {
        GetFileInformationByHandleEx(
            HANDLE(file.as_raw_handle()),
            FileIdInfo,
            (&mut info as *mut FILE_ID_INFO).cast(),
            std::mem::size_of_val(&info) as u32,
        )?;
    }
    anyhow::ensure!(
        info.FileId.Identifier.iter().any(|b| *b != 0),
        "Filesystem supplied no stable directory identity"
    );
    Ok(format!(
        "{:016x}:{:02x?}",
        info.VolumeSerialNumber, info.FileId.Identifier
    ))
}

/// Avoid silently losing alternate streams when the verified transfer pipeline
/// copies the ordinary data stream. The shell remains available for these files.
pub fn ordinary_stream_only(path: &Path) -> Result<()> {
    eligible(path)?;
    let mut data = WIN32_FIND_STREAM_DATA::default();
    unsafe {
        let search = FindFirstStreamW(
            PCWSTR(crate::wide(path).as_ptr()),
            FindStreamInfoStandard,
            (&mut data as *mut WIN32_FIND_STREAM_DATA).cast(),
            None,
        )?;
        struct Search(HANDLE);
        impl Drop for Search {
            fn drop(&mut self) {
                unsafe {
                    let _ = FindClose(self.0);
                }
            }
        }
        let _search = Search(search);
        loop {
            let len = data
                .cStreamName
                .iter()
                .position(|c| *c == 0)
                .unwrap_or(data.cStreamName.len());
            anyhow::ensure!(
                String::from_utf16_lossy(&data.cStreamName[..len]) == "::$DATA",
                "File has alternate data streams; use Windows shell operations to preserve them"
            );
            if let Err(error) =
                FindNextStreamW(search, (&mut data as *mut WIN32_FIND_STREAM_DATA).cast())
            {
                anyhow::ensure!(
                    error.code()
                        == windows::core::HRESULT::from_win32(
                            windows::Win32::Foundation::ERROR_HANDLE_EOF.0
                        ),
                    "Cannot enumerate all file streams: {error}"
                );
                break;
            }
        }
    }
    Ok(())
}
