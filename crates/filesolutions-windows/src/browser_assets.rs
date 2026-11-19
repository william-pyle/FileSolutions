//! Read-only shell assets for the app's browser; no shell registration.
use anyhow::{ensure, Result};
use serde_json::{json, Value};
use std::mem::size_of;
use base64::Engine as _;
use windows::{core::{w, PCWSTR}, Win32::{Foundation::{HWND, SIZE}, Graphics::Gdi::*, System::Com::*, UI::{Shell::*, WindowsAndMessaging::SW_SHOWNORMAL}}};

pub fn locations() -> Result<Value> {
    let mut result = vec![];
    for (name, icon, id) in [
        ("Desktop", "desktop", FOLDERID_Desktop), ("Downloads", "download", FOLDERID_Downloads),
        ("Documents", "documents", FOLDERID_Documents), ("Pictures", "pictures", FOLDERID_Pictures),
        ("Music", "music", FOLDERID_Music), ("Videos", "videos", FOLDERID_Videos),
    ] {
        unsafe {
            if let Ok(value) = SHGetKnownFolderPath(&id, KF_FLAG_DEFAULT, None) {
                let path = value.to_string();
                CoTaskMemFree(Some(value.0.cast()));
                if let Ok(path) = path { result.push(json!({"name":name,"icon":icon,"path":path})); }
            }
        }
    }
    Ok(json!(result))
}

pub fn open_network(hwnd: HWND) -> Result<()> {
    let result = unsafe { ShellExecuteW(Some(hwnd), w!("open"), w!("shell:NetworkPlacesFolder"), None, None, SW_SHOWNORMAL) };
    ensure!(result.0 as isize > 32, "Windows could not open Network");
    Ok(())
}

struct Apartment;
impl Drop for Apartment { fn drop(&mut self) { unsafe { CoUninitialize(); } } }
struct Bitmap(HBITMAP);
impl Drop for Bitmap { fn drop(&mut self) { unsafe { let _ = DeleteObject(HGDIOBJ(self.0.0)); } } }
struct Dc(HDC);
impl Drop for Dc { fn drop(&mut self) { unsafe { let _ = DeleteDC(self.0); } } }

pub fn icons(paths: &[String]) -> Result<Value> {
    ensure!(paths.len() <= 200, "Icon batches are limited to 200 items");
    unsafe { CoInitializeEx(None, COINIT_APARTMENTTHREADED).ok()?; }
    let _apartment = Apartment;
    let mut result = serde_json::Map::new();
    for path in paths {
        // Each failed association falls back to an ordinary file/folder icon in the UI.
        if let Ok(svg) = shell_icon(path) { result.insert(path.clone(), json!(svg)); }
    }
    Ok(Value::Object(result))
}

fn shell_icon(path: &str) -> Result<String> {
    unsafe {
        let factory: IShellItemImageFactory = SHCreateItemFromParsingName(PCWSTR(crate::wide(path).as_ptr()), None)?;
        let bitmap = Bitmap(factory.GetImage(SIZE { cx: 256, cy: 256 }, SIIGBF_ICONONLY)?);
        let mut dimensions = BITMAP::default();
        ensure!(GetObjectW(HGDIOBJ(bitmap.0.0), size_of::<BITMAP>() as i32, Some((&mut dimensions as *mut BITMAP).cast())) != 0, "Icon dimensions unavailable");
        let width = dimensions.bmWidth;
        let height = dimensions.bmHeight.abs();
        ensure!(width > 0 && height > 0 && width <= 256 && height <= 256, "Unexpected icon dimensions");
        let mut info = BITMAPINFO::default();
        info.bmiHeader = BITMAPINFOHEADER { biSize: size_of::<BITMAPINFOHEADER>() as u32, biWidth: width, biHeight: -height, biPlanes: 1, biBitCount: 32, biCompression: BI_RGB.0, ..Default::default() };
        let dc = Dc(CreateCompatibleDC(None));
        ensure!(!dc.0.0.is_null(), "Icon device context unavailable");
        let mut pixels = vec![0u8; (width * height * 4) as usize];
        ensure!(GetDIBits(dc.0, bitmap.0, 0, height as u32, Some(pixels.as_mut_ptr().cast()), &mut info, DIB_RGB_COLORS) == height, "Icon pixels unavailable");
        encode_pixels(width as u32, height as u32, pixels)
    }
}

// GDI returns premultiplied BGRA. Preserve pixels as one image rather than
// individually antialiased SVG runs, which produce visible seams when scaled.
fn encode_pixels(width: u32, height: u32, mut pixels: Vec<u8>) -> Result<String> {
    ensure!(pixels.len() == (width * height * 4) as usize, "Invalid icon pixel buffer");
    for pixel in pixels.chunks_exact_mut(4) {
        pixel.swap(0, 2);
        let alpha = pixel[3] as u32;
        for channel in &mut pixel[..3] {
            *channel = if alpha == 0 { 0 } else { ((*channel as u32 * 255 + alpha / 2) / alpha).min(255) as u8 };
        }
    }
    let mut bytes = Vec::new();
    {
        let mut encoder = png::Encoder::new(&mut bytes, width, height);
        encoder.set_color(png::ColorType::Rgba);
        encoder.set_depth(png::BitDepth::Eight);
        encoder.write_header()?.write_image_data(&pixels)?;
    }
    Ok(format!("data:image/png;base64,{}", base64::engine::general_purpose::STANDARD.encode(bytes)))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn shell_pixels_round_trip_with_straight_alpha_and_no_vector_seams() {
        let url = encode_pixels(2, 2, vec![0, 0, 255, 255, 0, 128, 0, 128, 33, 44, 55, 0, 255, 0, 0, 255]).unwrap();
        let bytes = base64::engine::general_purpose::STANDARD.decode(url.strip_prefix("data:image/png;base64,").unwrap()).unwrap();
        let mut reader = png::Decoder::new(std::io::Cursor::new(bytes)).read_info().unwrap();
        let mut pixels = vec![0; reader.output_buffer_size().unwrap()];
        let frame = reader.next_frame(&mut pixels).unwrap();
        assert_eq!((frame.width, frame.height), (2, 2));
        assert_eq!(&pixels[..frame.buffer_size()], &[255, 0, 0, 255, 0, 255, 0, 128, 0, 0, 0, 0, 0, 0, 255, 255]);
    }
}
