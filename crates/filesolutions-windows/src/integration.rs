use anyhow::Result;
use std::path::Path;
use winreg::{enums::*, RegKey};
pub const OVERLAY_KEY: &str =
    r"SOFTWARE\Microsoft\Windows\CurrentVersion\Explorer\ShellIconOverlayIdentifiers";
pub const CLSIDS: [&str; 5] = [
    "{7CD04097-A70E-4AAD-A1B1-D90ED264F601}",
    "{7CD04097-A70E-4AAD-A1B1-D90ED264F602}",
    "{7CD04097-A70E-4AAD-A1B1-D90ED264F603}",
    "{7CD04097-A70E-4AAD-A1B1-D90ED264F604}",
    "{7CD04097-A70E-4AAD-A1B1-D90ED264F605}",
];
pub const COMMAND_CLSIDS: [&str; 3] = [
    "{7CD04097-A70E-4AAD-A1B1-D90ED264F611}",
    "{7CD04097-A70E-4AAD-A1B1-D90ED264F612}",
    "{7CD04097-A70E-4AAD-A1B1-D90ED264F613}",
];
pub const BADGES: [&str; 5] = ["Backed up", "Syncing", "Pending", "Excluded", "Issue"];
/// Re-query status for changed items; never flush all image caches or restart Explorer.
pub fn refresh_overlays(paths: &[std::path::PathBuf]) {
    use windows::{
        core::PCWSTR,
        Win32::{
            System::Com::{CoInitializeEx, CoTaskMemFree, CoUninitialize, COINIT_MULTITHREADED},
            UI::Shell::{
                SHChangeNotify, SHParseDisplayName, SHCNE_UPDATEITEM, SHCNF_FLUSHNOWAIT,
                SHCNF_IDLIST, SHCNF_PATHW,
            },
        },
    };
    struct Com(bool);
    impl Drop for Com {
        fn drop(&mut self) {
            if self.0 {
                unsafe { CoUninitialize() }
            }
        }
    }
    let _com = Com(unsafe { CoInitializeEx(None, COINIT_MULTITHREADED).is_ok() });
    for path in paths {
        let wide = crate::wide(path);
        // PATH notifications have a MAX_PATH contract. Resolve longer paths on
        // this background thread and notify by Shell item ID instead.
        if wide.len() <= 260 {
            unsafe {
                SHChangeNotify(
                    SHCNE_UPDATEITEM,
                    SHCNF_PATHW | SHCNF_FLUSHNOWAIT,
                    Some(wide.as_ptr().cast()),
                    None,
                );
            }
        } else {
            unsafe {
                let mut item = std::ptr::null_mut();
                if SHParseDisplayName(PCWSTR(wide.as_ptr()), None, &mut item, 0, None).is_ok()
                    && !item.is_null()
                {
                    SHChangeNotify(
                        SHCNE_UPDATEITEM,
                        SHCNF_IDLIST | SHCNF_FLUSHNOWAIT,
                        Some(item.cast()),
                        None,
                    );
                }
                if !item.is_null() {
                    CoTaskMemFree(Some(item.cast()));
                }
            }
        }
    }
}
pub fn startup(enabled: bool, exe: &Path) -> Result<()> {
    let key = RegKey::predef(HKEY_CURRENT_USER)
        .create_subkey(r"Software\Microsoft\Windows\CurrentVersion\Run")?
        .0;
    if enabled {
        key.set_value("FileSolutions", &format!("\"{}\" --tray", exe.display()))?;
    } else {
        match key.delete_value("FileSolutions") {
            Ok(_) => {}
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => return Err(e.into()),
        }
    }
    Ok(())
}
pub fn open_url(url: &str) -> Result<()> {
    use windows::{
        core::PCWSTR,
        Win32::UI::{Shell::ShellExecuteW, WindowsAndMessaging::SW_SHOWNORMAL},
    };
    let allowed = url.starts_with("https://accounts.google.com/")
        || url.starts_with("https://drive.google.com/")
        || url.starts_with("https://one.google.com/");
    anyhow::ensure!(allowed, "Unsupported external URL");
    unsafe {
        let result = ShellExecuteW(
            None,
            windows::core::w!("open"),
            PCWSTR(crate::wide(url).as_ptr()),
            PCWSTR::null(),
            PCWSTR::null(),
            SW_SHOWNORMAL,
        );
        anyhow::ensure!(result.0 as isize > 32, "Could not open the system browser");
    }
    Ok(())
}
