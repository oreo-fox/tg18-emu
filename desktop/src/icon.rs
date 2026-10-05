//! The program's icon, drawn in code: an egg with rainbow stripes.
//! Made at whatever size Windows asks for (title bar, taskbar, tray).

use crate::egg::egg_pixels;
use crate::win32::*;

/// A Windows icon of the egg (to be freed with DestroyIcon).
pub fn make(size: i32) -> isize {
    let px = egg_pixels(size as usize);
    let mut info = BITMAPINFO::default();
    info.bmiHeader.biSize = std::mem::size_of::<BITMAPINFOHEADER>() as u32;
    info.bmiHeader.biWidth = size;
    info.bmiHeader.biHeight = -size;
    info.bmiHeader.biPlanes = 1;
    info.bmiHeader.biBitCount = 32;
    unsafe {
        let mut bits: *mut std::ffi::c_void = std::ptr::null_mut();
        let color = CreateDIBSection(0, &info, DIB_RGB_COLORS, &mut bits, 0, 0);
        if color == 0 || bits.is_null() {
            return 0;
        }
        std::ptr::copy_nonoverlapping(px.as_ptr(), bits as *mut u32, px.len());
        let mask = CreateBitmap(size, size, 1, 1, std::ptr::null()); // unused: the colour has alpha
        let ii = ICONINFO { fIcon: 1, xHotspot: 0, yHotspot: 0, hbmMask: mask, hbmColor: color };
        let icon = CreateIconIndirect(&ii);
        DeleteObject(color);
        DeleteObject(mask);
        icon
    }
}

/// The egg in the sizes Windows uses for big and small icons (made once,
/// kept for the whole run).
pub fn big_and_small() -> (isize, isize) {
    static ICONS: std::sync::OnceLock<(isize, isize)> = std::sync::OnceLock::new();
    *ICONS.get_or_init(|| {
        let (big, small) = unsafe { (GetSystemMetrics(SM_CXICON), GetSystemMetrics(SM_CXSMICON)) };
        (make(big.max(32)), make(small.max(16)))
    })
}

#[cfg(test)]
mod tests {
    /// TG18_ICON_PREVIEW=file.png cargo test -p tg18-desktop: writes the
    /// icon at 16, 24, 32, 48 and 256 pixels side by side, for looking at.
    #[test]
    fn preview() {
        let path = match std::env::var("TG18_ICON_PREVIEW") {
            Ok(p) => p,
            Err(_) => return,
        };
        let sizes = [16usize, 24, 32, 48, 256];
        let (w, h) = (sizes.iter().map(|s| s + 8).sum::<usize>(), 256);
        let mut rgb = vec![0u8; w * h * 3];
        // checkerboard behind, to see the transparency
        for (i, p) in rgb.chunks_exact_mut(3).enumerate() {
            let v = if (i % w / 8 + i / w / 8) % 2 == 0 { 200 } else { 235 };
            p.copy_from_slice(&[v, v, v]);
        }
        let mut x0 = 0;
        for s in sizes {
            for (i, &c) in crate::egg::egg_pixels(s).iter().enumerate() {
                let (x, y) = (x0 + i % s, i / s);
                let a = (c >> 24) as f32 / 255.0;
                let p = &mut rgb[(y * w + x) * 3..(y * w + x) * 3 + 3];
                for (j, sh) in [16, 8, 0].iter().enumerate() {
                    p[j] = (((c >> sh) & 0xFF) as f32 * a + p[j] as f32 * (1.0 - a)) as u8;
                }
            }
            x0 += s + 8;
        }
        tg18::png::write(&path, w, h, &rgb, 2).unwrap();
    }
}
