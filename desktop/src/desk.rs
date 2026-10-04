//! Desktop mode: the toy as a small shaped window that sits on the desktop.
//! No frame or title bar: it is dragged by its shell, the buttons are
//! clicked, and a right-click opens the menu.
//!
//! The shell is our own design (a rounded square, not the real toy's egg),
//! drawn in code in the window's colour theme. Windows shows it through a
//! layered window: every pixel has its own transparency, so the rounded
//! corners and the soft shadow blend into whatever is behind it.

use crate::win32::*;
use crate::Theme;

/// Where everything is in the desktop window (client pixels).
#[derive(Clone)]
pub struct Geo {
    pub w: i32,
    pub h: i32,
    /// LCD pixel size on screen.
    px: i32,
    /// Size unit: 1.0 at 2x on a 96-dpi screen.
    k: f32,
    body: [f32; 4],
    radius: f32,
    pub screen: RECT,
    bezel: [f32; 4],
    /// (centre x, centre y, radius) of A, B, C.
    pub buttons: [(f32, f32, f32); 3],
}

impl Geo {
    pub fn new(scale: u32, dpi: f64) -> Geo {
        let px = ((scale as f64 * dpi).round() as i32).max(1);
        let s = (128 * px) as f32;
        let k = s / 256.0;
        let r = |v: f32| (v * k).round();
        let margin = r(16.0);
        let pad = r(26.0);
        let bez = r(9.0);
        let body_w = s + 2.0 * (pad + bez);
        let body_h = pad + 2.0 * bez + s + r(90.0);
        let body = [margin, margin, margin + body_w, margin + body_h];
        let sx = margin + pad + bez;
        let screen = RECT { left: sx as i32, top: sx as i32, right: (sx + s) as i32, bottom: (sx + s) as i32 };
        let bezel = [sx - bez, sx - bez, sx + s + bez, sx + s + bez];
        let cx = margin + body_w / 2.0;
        let cy = bezel[3] + r(44.0);
        // B sits a little higher than A and C, like on the window and the toy
        let buttons = [
            (cx - r(74.0), cy + r(6.0), r(21.0)),
            (cx, cy - r(6.0), r(21.0)),
            (cx + r(74.0), cy + r(6.0), r(21.0)),
        ];
        Geo {
            w: (body[2] + margin) as i32,
            h: (body[3] + margin + r(6.0)) as i32,
            px,
            k,
            body,
            radius: r(46.0),
            screen,
            bezel,
            buttons,
        }
    }

    /// The button under a client point, if any.
    pub fn button_at(&self, x: i32, y: i32) -> Option<usize> {
        self.buttons.iter().position(|&(cx, cy, r)| {
            let (dx, dy) = (x as f32 + 0.5 - cx, y as f32 + 0.5 - cy);
            dx * dx + dy * dy <= r * r
        })
    }
}

// --- drawing helpers ------------------------------------------------------------

type Rgb = [f32; 3];

/// A COLORREF (0x00BBGGRR) as 0..1 RGB.
fn rgb_of(c: u32) -> Rgb {
    [(c & 0xFF) as f32 / 255.0, ((c >> 8) & 0xFF) as f32 / 255.0, ((c >> 16) & 0xFF) as f32 / 255.0]
}

fn mix(a: Rgb, b: Rgb, t: f32) -> Rgb {
    [a[0] + (b[0] - a[0]) * t, a[1] + (b[1] - a[1]) * t, a[2] + (b[2] - a[2]) * t]
}

const WHITE: Rgb = [1.0, 1.0, 1.0];

/// Signed distance from a point to a rounded rectangle (negative inside).
fn sdf_rrect(x: f32, y: f32, r: &[f32; 4], radius: f32) -> f32 {
    let (cx, cy) = ((r[0] + r[2]) / 2.0, (r[1] + r[3]) / 2.0);
    let qx = (x - cx).abs() - ((r[2] - r[0]) / 2.0 - radius);
    let qy = (y - cy).abs() - ((r[3] - r[1]) / 2.0 - radius);
    qx.max(0.0).hypot(qy.max(0.0)) + qx.max(qy).min(0.0) - radius
}

fn sdf_circle(x: f32, y: f32, c: (f32, f32, f32)) -> f32 {
    (x - c.0).hypot(y - c.1) - c.2
}

/// Coverage of a pixel by a shape at signed distance d (smooth edge).
fn cover(d: f32) -> f32 {
    (0.5 - d).clamp(0.0, 1.0)
}

fn smoothstep(a: f32, b: f32, x: f32) -> f32 {
    let t = ((x - a) / (b - a)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

/// Premultiplied RGBA picture built up layer by layer.
struct Canvas {
    w: usize,
    px: Vec<[f32; 4]>,
}

impl Canvas {
    /// Paint over every pixel: `f(x, y)` gives a colour and its coverage.
    fn layer(&mut self, f: impl Fn(f32, f32) -> (Rgb, f32)) {
        for (i, p) in self.px.iter_mut().enumerate() {
            let (x, y) = ((i % self.w) as f32 + 0.5, (i / self.w) as f32 + 0.5);
            let (c, a) = f(x, y);
            if a <= 0.0 {
                continue;
            }
            for j in 0..3 {
                p[j] = c[j] * a + p[j] * (1.0 - a);
            }
            p[3] = a + p[3] * (1.0 - a);
        }
    }

    fn to_argb(&self) -> Vec<u32> {
        self.px
            .iter()
            .map(|p| {
                let q = |v: f32| (v.clamp(0.0, 1.0) * 255.0 + 0.5) as u32;
                q(p[3]) << 24 | q(p[0]) << 16 | q(p[1]) << 8 | q(p[2])
            })
            .collect()
    }
}

/// The shell, bezel and buttons without the screen, premultiplied ARGB.
fn draw_shell(g: &Geo, t: &Theme) -> Vec<u32> {
    let (w, h) = (g.w as usize, g.h as usize);
    let mut c = Canvas { w, px: vec![[0.0; 4]; w * h] };
    let k = g.k;
    let body = rgb_of(t.body);
    let ink = rgb_of(t.ink);
    let button = rgb_of(t.button);
    let (top, bottom) = (g.body[1], g.body[3]);
    let shade = |y: f32| ((y - top) / (bottom - top)).clamp(0.0, 1.0);
    let shadow_rect = [g.body[0], g.body[1] + 5.0 * k, g.body[2], g.body[3] + 5.0 * k];

    // soft shadow on the desktop
    c.layer(|x, y| {
        let d = sdf_rrect(x, y, &shadow_rect, g.radius);
        (mix(ink, [0.0; 3], 0.5), 0.32 * (1.0 - smoothstep(-6.0 * k, 14.0 * k, d)))
    });
    // body: lighter at the top, a touch darker at the bottom
    c.layer(|x, y| {
        let d = sdf_rrect(x, y, &g.body, g.radius);
        let s = shade(y);
        (mix(mix(body, WHITE, 0.45), mix(body, ink, 0.07), s), cover(d))
    });
    // edge line and a shine along the top inside edge
    c.layer(|x, y| {
        let d = sdf_rrect(x, y, &g.body, g.radius);
        (mix(body, ink, 0.38), cover(d) - cover(d + 1.6 * k))
    });
    c.layer(|x, y| {
        let d = sdf_rrect(x, y, &g.body, g.radius);
        let ring = cover(d + 3.0 * k) - cover(d + 5.0 * k);
        (WHITE, ring * 0.7 * (1.0 - shade(y)).powi(3))
    });
    // bezel around the screen, with a lighter lip below it
    c.layer(|x, y| {
        let d = sdf_rrect(x, y - 1.5 * k, &g.bezel, 14.0 * k);
        (mix(body, WHITE, 0.6), cover(d) * 0.8)
    });
    c.layer(|x, y| {
        let d = sdf_rrect(x, y, &g.bezel, 14.0 * k);
        (mix(body, ink, 0.30), cover(d))
    });
    // buttons: shadow, face, rim
    for &b in &g.buttons {
        c.layer(|x, y| {
            let d = sdf_circle(x, y - 2.5 * k, b);
            (ink, 0.28 * cover(d / 1.5))
        });
        c.layer(|x, y| {
            let d = sdf_circle(x, y, b);
            let s = ((y - (b.1 - b.2)) / (2.0 * b.2)).clamp(0.0, 1.0);
            (mix(mix(button, WHITE, 0.5), mix(button, ink, 0.08), s), cover(d))
        });
        c.layer(|x, y| {
            let d = sdf_circle(x, y, b);
            (mix(button, ink, 0.6), cover(d) - cover(d + 1.6 * k))
        });
    }
    c.to_argb()
}

// --- the window -------------------------------------------------------------------

/// A 32-bit picture GDI can draw text into and Windows can show.
struct Surface {
    dc: HDC,
    bmp: HGDIOBJ,
    old: HGDIOBJ,
    bits: *mut u32,
    len: usize,
}

impl Surface {
    fn new(w: i32, h: i32) -> Surface {
        let mut info = BITMAPINFO::default();
        info.bmiHeader.biSize = std::mem::size_of::<BITMAPINFOHEADER>() as u32;
        info.bmiHeader.biWidth = w;
        info.bmiHeader.biHeight = -h; // top-down
        info.bmiHeader.biPlanes = 1;
        info.bmiHeader.biBitCount = 32;
        unsafe {
            let dc = CreateCompatibleDC(0);
            let mut bits: *mut std::ffi::c_void = std::ptr::null_mut();
            let bmp = CreateDIBSection(dc, &info, DIB_RGB_COLORS, &mut bits, 0, 0);
            let old = SelectObject(dc, bmp);
            Surface { dc, bmp, old, bits: bits as *mut u32, len: (w * h) as usize }
        }
    }

    fn pixels(&mut self) -> &mut [u32] {
        if self.bits.is_null() {
            return &mut [];
        }
        unsafe { std::slice::from_raw_parts_mut(self.bits, self.len) }
    }
}

impl Drop for Surface {
    fn drop(&mut self) {
        unsafe {
            SelectObject(self.dc, self.old);
            DeleteObject(self.bmp);
            DeleteDC(self.dc);
        }
    }
}

pub struct Desk {
    pub hwnd: HWND,
    pub geo: Geo,
    surface: Surface,
    /// The shell with its labels, copied under every frame.
    base: Vec<u32>,
    theme: Theme,
    /// Letters on the buttons.
    labels: bool,
    label_font: HGDIOBJ,
    hint_font: HGDIOBJ,
}

pub const CLASS: &str = "tg18desk";

impl Desk {
    /// The desktop window at (x, y), not shown yet.
    pub fn new(wndproc: WNDPROC, scale: u32, dpi: f64, theme: Theme, labels: bool, x: i32, y: i32, on_top: bool) -> Desk {
        let geo = Geo::new(scale, dpi);
        let class = wide(CLASS);
        let hwnd = unsafe {
            let inst = GetModuleHandleW(std::ptr::null());
            let wc = WNDCLASSEXW {
                cbSize: std::mem::size_of::<WNDCLASSEXW>() as u32,
                style: 0,
                lpfnWndProc: wndproc,
                cbClsExtra: 0,
                cbWndExtra: 0,
                hInstance: inst,
                hIcon: crate::icon::big_and_small().0,
                hCursor: LoadCursorW(0, IDC_ARROW),
                hbrBackground: 0,
                lpszMenuName: std::ptr::null(),
                lpszClassName: class.as_ptr(),
                hIconSm: crate::icon::big_and_small().1,
            };
            RegisterClassExW(&wc); // fails harmlessly once registered
            // no taskbar button: the tray icon stands in for it
            let ex = WS_EX_LAYERED | WS_EX_TOOLWINDOW | if on_top { WS_EX_TOPMOST } else { 0 };
            CreateWindowExW(ex, class.as_ptr(), wide("tg18").as_ptr(), WS_POPUP, x, y, geo.w, geo.h, 0, 0, inst,
                            std::ptr::null())
        };
        let k = geo.k as f64;
        let font = |h: f64, weight: i32| unsafe {
            CreateFontW(-(h.round() as i32), 0, 0, 0, weight, 0, 0, 0, 1, 0, 0, 4, 0, wide("Segoe UI").as_ptr())
        };
        let mut d = Desk {
            hwnd,
            surface: Surface::new(geo.w, geo.h),
            base: Vec::new(),
            theme,
            labels,
            label_font: font(20.0 * k, 700),
            hint_font: font(15.0 * k * 0.9, 400),
            geo,
        };
        d.build_base();
        d
    }

    pub fn set_labels(&mut self, on: bool) {
        self.labels = on;
        self.build_base();
    }

    pub fn set_theme(&mut self, theme: Theme) {
        self.theme = theme;
        self.build_base();
    }

    pub fn set_title(&self, text: &str) {
        unsafe { SetWindowTextW(self.hwnd, wide(text).as_ptr()) };
    }

    fn build_base(&mut self) {
        let shell = draw_shell(&self.geo, &self.theme);
        let g = self.geo.clone();
        let dc = self.surface.dc;
        self.surface.pixels().copy_from_slice(&shell);
        unsafe {
            SetBkMode(dc, TRANSPARENT);
            SetTextColor(dc, self.theme.ink);
            let old = SelectObject(dc, self.label_font);
            for (i, &(cx, cy, r)) in g.buttons.iter().enumerate() {
                if !self.labels {
                    break;
                }
                let mut rr = RECT { left: (cx - r) as i32, top: (cy - r) as i32, right: (cx + r) as i32, bottom: (cy + r) as i32 };
                draw_text(dc, ["A", "B", "C"][i], &mut rr, DT_CENTER | DT_VCENTER | DT_SINGLELINE);
            }
            SelectObject(dc, old);
        }
        // GDI leaves the alpha of the text at 0: the buttons are opaque
        let w = g.w as usize;
        let px = self.surface.pixels();
        for &(cx, cy, r) in &g.buttons {
            for_circle(w, (cx, cy, r - 1.0), |i| px[i] |= 0xFF00_0000);
        }
        self.base = px.to_vec();
    }

    /// Draw a frame: the LCD (0x00RRGGBB, 128x128; the sleep screen while
    /// `asleep`, then with its text), pressed buttons, and a message on the
    /// screen. The
    /// whole toy is drawn `dx` pixels to the side (the wiggle; the window
    /// has room around the shell for that).
    pub fn render(&mut self, lcd: &[u32], down: [bool; 3], asleep: bool, hint: &str, dx: i32) {
        let g = self.geo.clone();
        let w = g.w as usize;
        let down_rgb = rgb_of(self.theme.button_down);
        let dc = self.surface.dc;
        let base = std::mem::take(&mut self.base);
        let px = self.surface.pixels();
        if px.len() != base.len() {
            self.base = base;
            return;
        }
        px.copy_from_slice(&base);
        let s = &g.screen;
        for y in s.top..s.bottom {
            let ly = ((y - s.top) / g.px) as usize;
            let row = y as usize * w;
            for x in s.left..s.right {
                let lx = ((x - s.left) / g.px) as usize;
                px[row + x as usize] = if !hint.is_empty() { 0xFF00_0000 } else { 0xFF00_0000 | lcd[ly * 128 + lx] };
            }
        }
        for (i, &b) in g.buttons.iter().enumerate() {
            if down[i] {
                // tint the button towards the "pressed" colour, label included
                for_circle(w, (b.0, b.1, b.2 - 1.0), |j| {
                    let p = px[j];
                    let ch = |sh: u32, t: f32| {
                        let v = ((p >> sh) & 0xFF) as f32;
                        ((v * 0.45 + t * 255.0 * 0.55) as u32).min(255) << sh
                    };
                    px[j] = 0xFF00_0000 | ch(16, down_rgb[0]) | ch(8, down_rgb[1]) | ch(0, down_rgb[2]);
                });
            }
        }
        if asleep && hint.is_empty() {
            unsafe {
                SetBkMode(dc, TRANSPARENT);
                SetTextColor(dc, rgb(0xc9cde6));
                let old = SelectObject(dc, self.hint_font);
                let pad = (12.0 * g.k) as i32;
                let top = s.top + (s.bottom - s.top) * crate::SLEEP_TEXT_TOP / 128;
                let mut r = RECT { left: s.left + pad, top, right: s.right - pad, bottom: s.bottom - pad };
                draw_text(dc, crate::SLEEP_TEXT, &mut r, DT_CENTER | DT_WORDBREAK);
                SelectObject(dc, old);
            }
        }
        if !hint.is_empty() || asleep {
            // GDI text leaves alpha 0 behind: the screen is opaque
            let px = self.surface.pixels();
            for y in s.top..s.bottom {
                for x in s.left..s.right {
                    px[y as usize * w + x as usize] |= 0xFF00_0000;
                }
            }
        }
        if !hint.is_empty() {
            unsafe {
                SetBkMode(dc, TRANSPARENT);
                SetTextColor(dc, rgb(0xffffff));
                let old = SelectObject(dc, self.hint_font);
                let pad = (12.0 * g.k) as i32;
                let mut r = RECT { left: s.left + pad, top: s.top, right: s.right - pad, bottom: s.bottom };
                let flags = DT_CENTER | DT_WORDBREAK | DT_EDITCONTROL;
                let mut size = r;
                draw_text(dc, hint, &mut size, flags | DT_CALCRECT);
                r.top = (s.top + (s.bottom - s.top - (size.bottom - size.top)) / 2).max(s.top);
                draw_text(dc, hint, &mut r, flags);
                SelectObject(dc, old);
            }
            let px = self.surface.pixels();
            for y in s.top..s.bottom {
                for x in s.left..s.right {
                    px[y as usize * w + x as usize] |= 0xFF00_0000;
                }
            }
        }
        if dx != 0 {
            let px = self.surface.pixels();
            let d = dx.unsigned_abs() as usize;
            for row in px.chunks_exact_mut(w) {
                if dx > 0 {
                    row.copy_within(0..w - d, d);
                    row[..d].fill(0);
                } else {
                    row.copy_within(d.., 0);
                    row[w - d..].fill(0);
                }
            }
        }
        self.base = base;
        self.present();
    }

    /// How far the toy may be drawn to the side without being cut off.
    pub fn wiggle_room(&self) -> i32 {
        (10.0 * self.geo.k) as i32
    }

    /// Hand the picture to Windows (the window keeps its position).
    fn present(&self) {
        let size = SIZE { cx: self.geo.w, cy: self.geo.h };
        let src = POINT { x: 0, y: 0 };
        let blend = BLENDFUNCTION { BlendOp: 0, BlendFlags: 0, SourceConstantAlpha: 255, AlphaFormat: AC_SRC_ALPHA };
        unsafe {
            UpdateLayeredWindow(self.hwnd, 0, std::ptr::null(), &size, self.surface.dc, &src, 0, &blend, ULW_ALPHA);
        }
    }

    pub fn set_on_top(&self, on: bool) {
        unsafe {
            SetWindowPos(self.hwnd, if on { HWND_TOPMOST } else { HWND_NOTOPMOST }, 0, 0, 0, 0,
                         SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE);
        }
    }

    pub fn position(&self) -> (i32, i32) {
        let mut r = RECT::default();
        unsafe { GetWindowRect(self.hwnd, &mut r) };
        (r.left, r.top)
    }
}

impl Drop for Desk {
    fn drop(&mut self) {
        unsafe {
            DestroyWindow(self.hwnd);
            DeleteObject(self.label_font);
            DeleteObject(self.hint_font);
        }
    }
}

/// Call f with the index of every pixel inside a circle.
fn for_circle(w: usize, (cx, cy, r): (f32, f32, f32), mut f: impl FnMut(usize)) {
    let h_rows = (cy - r).floor().max(0.0) as usize..=(cy + r).ceil() as usize;
    for y in h_rows {
        for x in (cx - r).floor().max(0.0) as usize..=((cx + r).ceil() as usize).min(w - 1) {
            let (dx, dy) = (x as f32 + 0.5 - cx, y as f32 + 0.5 - cy);
            if dx * dx + dy * dy <= r * r {
                f(y * w + x);
            }
        }
    }
}

/// Where a new desktop toy goes: the bottom right of the main screen's work
/// area (above the taskbar).
pub fn default_position(w: i32, h: i32) -> (i32, i32) {
    let mut r = RECT::default();
    unsafe { SystemParametersInfoW(SPI_GETWORKAREA, 0, &mut r as *mut RECT as *mut _, 0) };
    if r.right <= r.left {
        return (100, 100);
    }
    (r.right - w - 40, r.bottom - h - 20)
}

/// True if (x, y) with this size is at least partly on a monitor.
pub fn on_screen(x: i32, y: i32, w: i32, h: i32) -> bool {
    unsafe { MonitorFromPoint(POINT { x: x + w / 2, y: y + h / 2 }, MONITOR_DEFAULTTONULL) != 0 }
}

/// Move (x, y) so the whole toy is inside the work area (the screen minus
/// the taskbar) of the monitor it is mostly on.
pub fn fit_on_screen(x: i32, y: i32, w: i32, h: i32) -> (i32, i32) {
    let mut info = MONITORINFO { cbSize: std::mem::size_of::<MONITORINFO>() as u32, ..Default::default() };
    let ok = unsafe {
        let mon = MonitorFromPoint(POINT { x: x + w / 2, y: y + h / 2 }, MONITOR_DEFAULTTONEAREST);
        GetMonitorInfoW(mon, &mut info) != 0
    };
    if !ok {
        return (x, y);
    }
    let r = info.rcWork;
    (x.min(r.right - w).max(r.left), y.min(r.bottom - h).max(r.top))
}
