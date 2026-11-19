//! Adapted from ContactSolutions src/window_chrome.rs, MIT (c) 2026 William Pyle.
//! Keep Tao's sizing/work-area/DPI handling. Supply only undecorated hit tests.
//! All functions and subclass state are confined to the Windows event thread.
use anyhow::{bail, ensure, Result};
use serde::Deserialize;
use windows::Win32::{
    Foundation::{HWND, LPARAM, LRESULT, POINT, RECT, WPARAM},
    Graphics::Gdi::{ClientToScreen, ScreenToClient},
    UI::{
        HiDpi::{GetDpiForWindow, GetSystemMetricsForDpi},
        Shell::{DefSubclassProc, GetWindowSubclass, RemoveWindowSubclass, SetWindowSubclass},
        WindowsAndMessaging::*,
    },
};

const SUBCLASS: usize = 0x46534348;
const SHOW_MENU: u32 = WM_APP + 0x434;

#[derive(Clone, Default, Deserialize)]
pub struct Region {
    left: f64,
    top: f64,
    right: f64,
    bottom: f64,
}
impl Region {
    fn contains(&self, x: f64, y: f64) -> bool {
        x >= self.left && x < self.right && y >= self.top && y < self.bottom
    }
    fn valid(&self) -> bool {
        [self.left, self.top, self.right, self.bottom]
            .iter()
            .all(|v| v.is_finite())
            && self.right >= self.left
            && self.bottom >= self.top
    }
}
#[derive(Default, Deserialize)]
pub struct Layout {
    toolbar: Region,
    controls: Vec<Region>,
    scale: f64,
}
impl Layout {
    fn valid(&self) -> bool {
        self.scale.is_finite()
            && self.scale > 0.
            && self.scale <= 16.
            && self.toolbar.valid()
            && self.controls.len() <= 32
            && self.controls.iter().all(Region::valid)
    }
}

pub fn install(hwnd: HWND) -> Result<()> {
    let state = Box::into_raw(Box::<Layout>::default());
    if !unsafe { SetWindowSubclass(hwnd, Some(window_proc), SUBCLASS, state as usize) }.as_bool() {
        unsafe {
            drop(Box::from_raw(state));
        }
        bail!(
            "Cannot install native window chrome: {}",
            std::io::Error::last_os_error()
        );
    }
    Ok(())
}
pub fn available(hwnd: HWND) -> bool {
    unsafe { GetWindowSubclass(hwnd, Some(window_proc), SUBCLASS, None).as_bool() }
}
pub fn set_layout(hwnd: HWND, layout: Layout) -> Result<()> {
    ensure!(layout.valid(), "Invalid window chrome layout");
    let mut data = 0usize;
    ensure!(
        unsafe { GetWindowSubclass(hwnd, Some(window_proc), SUBCLASS, Some(&mut data)) }.as_bool(),
        "Native chrome is unavailable"
    );
    unsafe {
        *(data as *mut Layout) = layout;
    }
    Ok(())
}
pub fn system_menu(hwnd: HWND) -> Result<()> {
    // Post the donor's private message so the native modal loop never holds an IPC/frame lock.
    unsafe {
        if available(hwnd) {
            PostMessageW(Some(hwnd), SHOW_MENU, WPARAM(0), LPARAM(0))?;
        } else {
            PostMessageW(
                Some(hwnd),
                WM_SYSCOMMAND,
                WPARAM(SC_KEYMENU as usize),
                LPARAM(32),
            )?;
        }
    }
    Ok(())
}
fn hit_test(
    x: i32,
    y: i32,
    width: i32,
    height: i32,
    border: i32,
    maximized: bool,
    layout: &Layout,
) -> u32 {
    if !maximized {
        match (
            x < border,
            x >= width - border,
            y < border,
            y >= height - border,
        ) {
            (true, _, true, _) => return HTTOPLEFT,
            (_, true, true, _) => return HTTOPRIGHT,
            (true, _, _, true) => return HTBOTTOMLEFT,
            (_, true, _, true) => return HTBOTTOMRIGHT,
            (true, _, _, _) => return HTLEFT,
            (_, true, _, _) => return HTRIGHT,
            (_, _, true, _) => return HTTOP,
            (_, _, _, true) => return HTBOTTOM,
            _ => {}
        }
    }
    let (x, y) = (
        x as f64 / layout.scale.max(1.0e-6),
        y as f64 / layout.scale.max(1.0e-6),
    );
    if layout.toolbar.contains(x, y) && !layout.controls.iter().any(|r| r.contains(x, y)) {
        HTCAPTION
    } else {
        HTCLIENT
    }
}
unsafe fn hit(hwnd: HWND, point: POINT, layout: &Layout) -> Result<u32> {
    let mut rect = RECT::default();
    unsafe {
        GetClientRect(hwnd, &mut rect)?;
    }
    let dpi = unsafe { GetDpiForWindow(hwnd) };
    let border = unsafe {
        GetSystemMetricsForDpi(SM_CXSIZEFRAME, dpi) + GetSystemMetricsForDpi(SM_CXPADDEDBORDER, dpi)
    };
    Ok(hit_test(
        point.x,
        point.y,
        rect.right,
        rect.bottom,
        border.max(1),
        unsafe { IsZoomed(hwnd) }.as_bool(),
        layout,
    ))
}
unsafe fn show_menu(hwnd: HWND) {
    unsafe {
        let menu = GetSystemMenu(hwnd, false);
        if menu.is_invalid() {
            return;
        }
        let maximized = IsZoomed(hwnd).as_bool();
        for (command, enabled) in [
            (SC_RESTORE, maximized),
            (SC_MAXIMIZE, !maximized),
            (SC_SIZE, !maximized),
            (SC_MOVE, !maximized),
        ] {
            let _ = EnableMenuItem(
                menu,
                command,
                MF_BYCOMMAND | if enabled { MF_ENABLED } else { MF_GRAYED },
            );
        }
        let dpi = GetDpiForWindow(hwnd) as f32 / 96.;
        let mut anchor = POINT {
            x: (8. * dpi) as i32,
            y: (48. * dpi) as i32,
        };
        if !ClientToScreen(hwnd, &mut anchor).as_bool() {
            return;
        }
        let command = TrackPopupMenu(
            menu,
            TPM_RETURNCMD | TPM_RIGHTBUTTON,
            anchor.x,
            anchor.y,
            None,
            hwnd,
            None,
        )
        .0;
        if command != 0 {
            let _ = PostMessageW(
                Some(hwnd),
                WM_SYSCOMMAND,
                WPARAM(command as usize),
                LPARAM(0),
            );
        }
    }
}
unsafe extern "system" fn window_proc(
    hwnd: HWND,
    msg: u32,
    wparam: WPARAM,
    lparam: LPARAM,
    _: usize,
    data: usize,
) -> LRESULT {
    unsafe {
        match msg {
            WM_NCHITTEST => {
                // Signed extraction supports monitors left/above the primary.
                let mut point = POINT {
                    x: lparam.0 as i16 as i32,
                    y: (lparam.0 >> 16) as i16 as i32,
                };
                if ScreenToClient(hwnd, &mut point).as_bool() {
                    if let Ok(code) = hit(hwnd, point, &*(data as *const Layout)) {
                        return LRESULT(code as isize);
                    }
                }
            }
            SHOW_MENU => {
                show_menu(hwnd);
                return LRESULT(0);
            }
            WM_NCRBUTTONUP if wparam.0 == HTCAPTION as usize => {
                show_menu(hwnd);
                return LRESULT(0);
            }
            WM_SYSCOMMAND if wparam.0 as u32 & 0xfff0 == SC_KEYMENU && lparam.0 == 32 => {
                show_menu(hwnd);
                return LRESULT(0);
            }
            WM_NCDESTROY => {
                let _ = RemoveWindowSubclass(hwnd, Some(window_proc), SUBCLASS);
                drop(Box::from_raw(data as *mut Layout));
            }
            _ => {}
        }
        DefSubclassProc(hwnd, msg, wparam, lparam)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn layout(scale: f64) -> Layout {
        Layout {
            toolbar: Region {
                left: 0.,
                top: 0.,
                right: 1000.,
                bottom: 48.,
            },
            controls: vec![
                Region {
                    left: 8.,
                    top: 8.,
                    right: 40.,
                    bottom: 40.,
                },
                Region {
                    left: 874.,
                    top: 0.,
                    right: 1000.,
                    bottom: 48.,
                },
            ],
            scale,
        }
    }
    #[test]
    fn donor_edges_corners_and_caption_exclusions_at_every_scale() {
        for scale in [1., 1.25, 1.5, 2., 2.5, 3.] {
            let layout = layout(scale);
            let hit = |x: f64, y: f64, maximized| {
                hit_test(
                    (x * scale) as i32,
                    (y * scale) as i32,
                    (1000. * scale) as i32,
                    (700. * scale) as i32,
                    (8. * scale) as i32,
                    maximized,
                    &layout,
                )
            };
            for (x, y, code) in [
                (0., 0., HTTOPLEFT),
                (999., 0., HTTOPRIGHT),
                (0., 699., HTBOTTOMLEFT),
                (999., 699., HTBOTTOMRIGHT),
                (0., 300., HTLEFT),
                (999., 300., HTRIGHT),
                (500., 0., HTTOP),
                (500., 699., HTBOTTOM),
            ] {
                assert_eq!(hit(x, y, false), code);
                assert!(![
                    HTLEFT,
                    HTRIGHT,
                    HTTOP,
                    HTBOTTOM,
                    HTTOPLEFT,
                    HTTOPRIGHT,
                    HTBOTTOMLEFT,
                    HTBOTTOMRIGHT
                ]
                .contains(&hit(x, y, true)));
            }
            assert_eq!(hit(300., 24., false), HTCAPTION);
            assert_eq!(hit(24., 24., false), HTCLIENT);
            assert_eq!(hit(900., 24., false), HTCLIENT);
            assert_eq!(hit(300., 60., false), HTCLIENT);
            assert_eq!(hit(300., 24., true), HTCAPTION);
        }
    }
    #[test]
    fn malformed_webview_layout_is_rejected() {
        assert!(layout(1.25).valid());
        assert!(!layout(0.).valid());
        assert!(!layout(f64::NAN).valid());
        let mut bad = layout(1.);
        bad.controls[0].right = f64::INFINITY;
        assert!(!bad.valid());
    }
}
