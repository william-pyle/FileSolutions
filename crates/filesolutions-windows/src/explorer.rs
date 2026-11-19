//! Windows shell integration without registration or Explorer restarts.
use anyhow::{Context, Result};
use std::{ffi::CString, path::Path};
use windows::{
    core::{Interface, PCSTR, PCWSTR},
    Win32::{
        Foundation::*,
        System::{Com::*, DataExchange::*, Memory::*},
        UI::{Shell::*, WindowsAndMessaging::*},
    },
};

struct Pidl(*mut Common::ITEMIDLIST);
impl Drop for Pidl {
    fn drop(&mut self) {
        unsafe {
            CoTaskMemFree(Some(self.0.cast()));
        }
    }
}
struct Menu(HMENU);
impl Drop for Menu {
    fn drop(&mut self) {
        unsafe {
            let _ = DestroyMenu(self.0);
        }
    }
}

unsafe extern "system" fn menu_proc(
    hwnd: HWND,
    msg: u32,
    wp: WPARAM,
    lp: LPARAM,
    _: usize,
    data: usize,
) -> LRESULT {
    if [WM_INITMENUPOPUP, WM_DRAWITEM, WM_MEASUREITEM, WM_MENUCHAR].contains(&msg) {
        let menu = &*(data as *const IContextMenu3);
        let mut result = LRESULT(0);
        if menu.HandleMenuMsg2(msg, wp, lp, Some(&mut result)).is_ok() {
            return result;
        }
    }
    DefSubclassProc(hwnd, msg, wp, lp)
}

/// Must execute on the app's STA event thread, not a Tokio worker.
pub fn context(hwnd: HWND, paths: &[String], verb: Option<&str>) -> Result<()> {
    anyhow::ensure!(!paths.is_empty(), "Select an item first");
    let parent = Path::new(&paths[0])
        .parent()
        .context("Drive root context menu is unavailable")?;
    anyhow::ensure!(
        paths.iter().all(|p| Path::new(p).parent() == Some(parent)),
        "Shell selection must share a parent folder"
    );
    unsafe {
        let mut pidls = vec![];
        let mut children = vec![];
        let mut folder: Option<IShellFolder> = None;
        for path in paths {
            let mut pidl = std::ptr::null_mut();
            SHParseDisplayName(PCWSTR(crate::wide(path).as_ptr()), None, &mut pidl, 0, None)?;
            let pidl = Pidl(pidl);
            let mut child = std::ptr::null_mut();
            let bound: IShellFolder = SHBindToParent(pidl.0, Some(&mut child))?;
            if folder.is_none() {
                folder = Some(bound);
            }
            children.push(child as *const Common::ITEMIDLIST);
            pidls.push(pidl);
        }
        let context: IContextMenu = folder.unwrap().GetUIObjectOf(hwnd, &children, None)?;
        let menu = Menu(CreatePopupMenu()?);
        context
            .QueryContextMenu(menu.0, 0, 1, 0x7fff, CMF_NORMAL)
            .ok()?;
        let literal = verb.map(CString::new).transpose()?;
        let command = if let Some(ref verb) = literal {
            PCSTR(verb.as_ptr().cast())
        } else {
            let menu3 = context.cast::<IContextMenu3>().ok();
            if let Some(ref menu3) = menu3 {
                let _ = SetWindowSubclass(
                    hwnd,
                    Some(menu_proc),
                    0x4653,
                    (menu3 as *const IContextMenu3) as usize,
                );
            }
            let mut point = POINT::default();
            GetCursorPos(&mut point)?;
            let chosen = TrackPopupMenuEx(
                menu.0,
                (TPM_RETURNCMD | TPM_RIGHTBUTTON).0,
                point.x,
                point.y,
                hwnd,
                None,
            )
            .0;
            if menu3.is_some() {
                let _ = RemoveWindowSubclass(hwnd, Some(menu_proc), 0x4653);
            }
            if chosen == 0 {
                return Ok(());
            }
            PCSTR((chosen as usize - 1) as *const u8)
        };
        let info = CMINVOKECOMMANDINFO {
            cbSize: std::mem::size_of::<CMINVOKECOMMANDINFO>() as u32,
            hwnd,
            lpVerb: command,
            nShow: SW_SHOWNORMAL.0,
            ..Default::default()
        };
        context.InvokeCommand(&info)?;
    }
    Ok(())
}
pub fn system_menu(hwnd: HWND) -> Result<()> {
    unsafe {
        let menu = GetSystemMenu(hwnd, false);
        let mut point = POINT::default();
        GetCursorPos(&mut point)?;
        let command = TrackPopupMenuEx(
            menu,
            (TPM_RETURNCMD | TPM_RIGHTBUTTON).0,
            point.x,
            point.y,
            hwnd,
            None,
        )
        .0;
        if command != 0 {
            PostMessageW(
                Some(hwnd),
                WM_SYSCOMMAND,
                WPARAM(command as usize),
                LPARAM(0),
            )?;
        }
    }
    Ok(())
}
pub fn clipboard(hwnd: HWND) -> Result<serde_json::Value> {
    unsafe {
        OpenClipboard(Some(hwnd))?;
        struct Close;
        impl Drop for Close {
            fn drop(&mut self) {
                unsafe {
                    let _ = CloseClipboard();
                }
            }
        }
        let _close = Close;
        let drop = GetClipboardData(15).context("Clipboard contains no files")?;
        let count = DragQueryFileW(HDROP(drop.0), u32::MAX, None);
        let mut paths = vec![];
        for n in 0..count {
            let len = DragQueryFileW(HDROP(drop.0), n, None);
            let mut value = vec![0u16; len as usize + 1];
            DragQueryFileW(HDROP(drop.0), n, Some(&mut value));
            paths.push(String::from_utf16_lossy(&value[..len as usize]));
        }
        let format = RegisterClipboardFormatW(windows::core::w!("Preferred DropEffect"));
        let moving = if let Ok(handle) = GetClipboardData(format) {
            let memory = GlobalLock(HGLOBAL(handle.0));
            if memory.is_null() {
                false
            } else {
                let value = *(memory as *const u32);
                let _ = GlobalUnlock(HGLOBAL(handle.0));
                value & 2 != 0
            }
        } else {
            false
        };
        Ok(serde_json::json!({"paths":paths,"moving":moving}))
    }
}
/// Shell's reversible Delete command shows Windows confirmation/error UI.
pub fn recycle(hwnd: HWND, paths: &[String]) -> Result<()> {
    unsafe {
        let op: IFileOperation = CoCreateInstance(&FileOperation, None, CLSCTX_INPROC_SERVER)?;
        op.SetOwnerWindow(hwnd)?;
        op.SetOperationFlags(FOF_ALLOWUNDO | FOFX_RECYCLEONDELETE | FOF_WANTNUKEWARNING)?;
        for path in paths {
            let item: IShellItem =
                SHCreateItemFromParsingName(PCWSTR(crate::wide(path).as_ptr()), None)?;
            op.DeleteItem(&item, None)?;
        }
        op.PerformOperations()?;
        anyhow::ensure!(
            !op.GetAnyOperationsAborted()?.as_bool(),
            "Delete cancelled; some items may already be in the Recycle Bin"
        );
    }
    Ok(())
}

use windows::Win32::System::{
    Ole::*,
    SystemServices::{MK_LBUTTON, MODIFIERKEYS_FLAGS},
};
#[windows::core::implement(IDropSource)]
struct DragSource;
impl IDropSource_Impl for DragSource_Impl {
    fn QueryContinueDrag(
        &self,
        escape: windows::core::BOOL,
        keys: MODIFIERKEYS_FLAGS,
    ) -> windows::core::HRESULT {
        if escape.as_bool() {
            DRAGDROP_S_CANCEL
        } else if !keys.contains(MK_LBUTTON) {
            DRAGDROP_S_DROP
        } else {
            windows::core::HRESULT(0)
        }
    }
    fn GiveFeedback(&self, _: DROPEFFECT) -> windows::core::HRESULT {
        DRAGDROP_S_USEDEFAULTCURSORS
    }
}
/// Outbound drag advertises copy only. External apps must never remove sources
/// before FileSolutions can verify their destination; internal moves use Rust.
pub fn drag_copy(hwnd: HWND, paths: &[String]) -> Result<()> {
    anyhow::ensure!(!paths.is_empty(), "Select files to drag");
    unsafe {
        let mut pidls = vec![];
        let mut children = vec![];
        let mut parent: Option<IShellFolder> = None;
        let folder = Path::new(&paths[0]).parent();
        for path in paths {
            anyhow::ensure!(
                Path::new(path).parent() == folder,
                "Drag selection must share a parent folder"
            );
            let mut pidl = std::ptr::null_mut();
            SHParseDisplayName(PCWSTR(crate::wide(path).as_ptr()), None, &mut pidl, 0, None)?;
            let pidl = Pidl(pidl);
            let mut child = std::ptr::null_mut();
            let bound: IShellFolder = SHBindToParent(pidl.0, Some(&mut child))?;
            if parent.is_none() {
                parent = Some(bound);
            }
            children.push(child as *const Common::ITEMIDLIST);
            pidls.push(pidl);
        }
        let data: IDataObject = parent.unwrap().GetUIObjectOf(hwnd, &children, None)?;
        let source: IDropSource = DragSource.into();
        let mut effect = DROPEFFECT_NONE;
        DoDragDrop(&data, &source, DROPEFFECT_COPY, &mut effect).ok()?;
    }
    Ok(())
}
