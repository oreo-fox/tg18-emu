//! The program's icon, drawn in code: an egg with rainbow stripes.
//! Made at whatever size Windows asks for (title bar, taskbar, tray).

use crate::win32::*;

const STRIPES: [[f32; 3]; 6] = [
    [1.00, 0.42, 0.45], // red
    [1.00, 0.66, 0.30], // orange
    [1.00, 0.88, 0.40], // yellow
    [0.42, 0.85, 0.50], // green
    [0.30, 0.67, 0.97], // blue
    [0.70, 0.59, 0.99], // purple
];
const RIM: [f32; 3] = [0.35, 0.16, 0.25];

/// The egg as size x size pixels, 0xAARRGGBB (not premultiplied).
pub fn egg_pixels(size: usize) -> Vec<u32> {
    let n = size as f32;
    // outline width in egg units: about one pixel, a bit more when large
    let rim = (1.1 + n / 80.0) / n;
    const SUB: usize = 4; // 4x4 samples per pixel for smooth edges
    let mut out = Vec::with_capacity(size * size);
    for py in 0..size {
        for px in 0..size {
            let mut acc = [0.0f32; 4];
            for sy in 0..SUB {
                for sx in 0..SUB {
                    let x = (px as f32 + (sx as f32 + 0.5) / SUB as f32) / n;
                    let y = (py as f32 + (sy as f32 + 0.5) / SUB as f32) / n;
                    if let Some(c) = egg_at(x, y, rim) {
                        for j in 0..3 {
                            acc[j] += c[j];
                        }
                        acc[3] += 1.0;
                    }
                }
            }
            let a = acc[3] / (SUB * SUB) as f32;
            if acc[3] == 0.0 {
                out.push(0);
                continue;
            }
            let q = |v: f32| ((v / acc[3]).clamp(0.0, 1.0) * 255.0 + 0.5) as u32;
            out.push(((a * 255.0 + 0.5) as u32) << 24 | q(acc[0]) << 16 | q(acc[1]) << 8 | q(acc[2]));
        }
    }
    out
}

/// Colour of the egg at (x, y) in 0..1 coordinates, None outside it.
fn egg_at(x: f32, y: f32, rim: f32) -> Option<[f32; 3]> {
    let (cx, cy, half_h) = (0.5, 0.53, 0.45);
    let t = (y - cy) / half_h; // -1 top .. 1 bottom
    let half_w = 0.335 * (1.0 + 0.16 * t); // narrower at the top, like an egg
    let u = (x - cx) / half_w;
    let r = (u * u + t * t).sqrt();
    if r > 1.0 {
        return None;
    }
    if r > 1.0 - rim / half_h.min(half_w) {
        return Some(RIM);
    }
    // stripes curve a little around the egg
    let band = ((t + 0.18 * u * u + 1.0) / 2.0 * STRIPES.len() as f32).floor() as isize;
    let mut c = STRIPES[band.clamp(0, STRIPES.len() as isize - 1) as usize];
    // light from the top left: a shine spot and a slightly darker lower right
    let (hx, hy) = (u + 0.42, t + 0.52);
    let shine = (1.0 - (hx * hx + hy * hy).sqrt() / 0.38).clamp(0.0, 1.0);
    let shade = ((u + t) * 0.5).clamp(0.0, 1.0) * 0.18;
    for v in c.iter_mut() {
        *v = *v * (1.0 - shade) + (1.0 - *v) * shine * 0.85;
    }
    Some(c)
}

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
            for (i, &c) in super::egg_pixels(s).iter().enumerate() {
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
