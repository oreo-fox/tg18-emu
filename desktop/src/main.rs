//! tg18 emulator window: see the screen, press A, B and C, hear it.
//!
//!     tg18 [flash.bin]
//!
//! With no argument it reopens the last ROM and resumes where you left off.
//! ROM dumps go in the `roms` folder next to the program (another folder can
//! be chosen in the File menu), saves in `saves`. A port of the
//! prototype's tg18win.py (same settings, same save folders).
//!
//! Keys: A / B / C, or Left / Down / Right; clicking the buttons works too.
//! Holding A and C together presses both. M mutes, Ctrl+S saves.
#![windows_subsystem = "windows"]

mod audio;
mod desk;
mod egg;
mod icon;
mod tray;
mod store;
mod win32;

use std::cell::{Cell, RefCell};
use std::collections::VecDeque;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use tg18::save;
use tg18::snapshot::{SnapError, Snapshot};
use tg18::sound::ToneSynth;
use tg18::{Key, Machine, Outcome, CPU_HZ};

use store::{Newest, SaveStore, Settings, Writer};
use win32::*;

const AUDIO_RATE: u32 = 44100;
/// Keep at least this much sound queued (s), against dropouts.
const AUDIO_LOW: f64 = 0.05;
/// Above this, silences are shortened back to AUDIO_LOW + a chunk.
const AUDIO_HIGH: f64 = 0.09;
/// Smallest block handed to Windows (s).
const AUDIO_CHUNK: f64 = 0.02;
/// How often the emulator runs.
const TICK: Duration = Duration::from_millis(4);
/// Most emulated seconds per tick, so sound and window stay smooth.
const MAX_STEP: f64 = 0.02;
/// Most emulated time (s) the window will catch up on.
const MAX_BEHIND: f64 = 0.25;
/// At most one undo snapshot per second of button presses.
const REWIND_GAP: f64 = 1.0;
/// Most the device clock may run ahead of the CPU when it lags.
const MAX_CATCHUP: f64 = 10.0;
/// Emulated seconds a quick tap is held (firmware debounce: 44 ms).
const MIN_HOLD: f64 = 0.05;
/// How often the ROM folder is looked at for new dumps.
const ROM_SCAN: Duration = Duration::from_secs(2);
/// A sound counts as the toy calling if no button was pressed for this long.
const CALL_QUIET: Duration = Duration::from_secs(3);
/// The desktop toy's wiggle when it calls: length, swings per second, and
/// at most one wiggle per this many seconds.
const WIGGLE_LEN: f64 = 1.0;
const WIGGLE_HZ: f64 = 6.0;
const WIGGLE_GAP: Duration = Duration::from_secs(4);

/// Colours of the window: the toy's body, buttons, pressed buttons, text.
#[derive(Clone, Copy)]
struct Theme {
    /// Name in settings.json.
    key: &'static str,
    label: &'static str,
    body: u32,
    button: u32,
    button_down: u32,
    ink: u32,
}

const THEMES: [Theme; 5] = [
    Theme { key: "pink", label: "Pastel pink", body: rgb(0xf3d2e0), button: rgb(0xfbf4f7), button_down: rgb(0xe38aac), ink: rgb(0x5a2a3f) },
    Theme { key: "blue", label: "Pastel blue", body: rgb(0xcfe2f6), button: rgb(0xf4f8fd), button_down: rgb(0x8bb6e4), ink: rgb(0x26405e) },
    Theme { key: "green", label: "Pastel green", body: rgb(0xd3ecd5), button: rgb(0xf4fbf4), button_down: rgb(0x8cc99a), ink: rgb(0x2b4d34) },
    Theme { key: "yellow", label: "Pastel yellow", body: rgb(0xf7edc3), button: rgb(0xfdfaee), button_down: rgb(0xe2c46a), ink: rgb(0x5a4718) },
    Theme { key: "lilac", label: "Pastel lilac", body: rgb(0xe3d7f3), button: rgb(0xfaf7fe), button_down: rgb(0xb59ae0), ink: rgb(0x46305f) },
];

/// The theme saved in the settings (pink if unknown).
fn theme(key: &str) -> Theme {
    THEMES.iter().copied().find(|t| t.key == key).unwrap_or(THEMES[0])
}

/// Shown on the screen while the toy sleeps, under the moon.
pub const SLEEP_TEXT: &str = "Screen sleeping\u{2026}\nPress a button to wake it";
/// Where that text starts, in LCD pixels from the top (the moon is above).
pub const SLEEP_TEXT_TOP: i32 = 74;

/// The screen while the toy sleeps: night sky with a crescent moon and a
/// few stars, as pixel art at the LCD's own 128x128 (0x00RRGGBB).
pub fn sleep_screen() -> &'static [u32] {
    static SCREEN: std::sync::OnceLock<Vec<u32>> = std::sync::OnceLock::new();
    SCREEN.get_or_init(|| {
        let mut px = vec![0x000a0c1au32; 128 * 128];
        let (cx, cy, r) = (64.0f32, 46.0f32, 14.0f32);
        for y in 0..128 {
            for x in 0..128 {
                let (fx, fy) = (x as f32 + 0.5, y as f32 + 0.5);
                let d = (fx - cx).hypot(fy - cy);
                let bite = (fx - cx - 6.0).hypot(fy - cy + 4.0);
                if d <= r && bite > r * 0.82 {
                    // the crescent, a little darker along its outer edge
                    px[y * 128 + x] = if d > r - 1.5 { 0x00e0b850 } else { 0x00f6df8a };
                }
            }
        }
        for &(x, y, big) in &[(36usize, 30usize, true), (92, 26, false), (96, 58, true), (30, 60, false), (82, 40, false)] {
            px[y * 128 + x] = 0x00dfe6ff;
            if big {
                for (dx, dy) in [(1i32, 0i32), (-1, 0), (0, 1), (0, -1)] {
                    px[(y as i32 + dy) as usize * 128 + (x as i32 + dx) as usize] = 0x006a7299;
                }
            }
        }
        px
    })
}

const KEYS_HINT: &str = "Keys: A B C  or  \u{2190} \u{2193} \u{2192}   (A+C together for both)   M: mute   Ctrl+S: save";

// menu command ids
const ID_CHOOSE_FOLDER: u16 = 100;
const ID_SAVE_NOW: u16 = 101;
const ID_OPEN_ROMS: u16 = 102;
const ID_SAVE_SLOT: u16 = 110;
const ID_LOAD_SLOT: u16 = 120;
const ID_OPEN_SAVES: u16 = 130;
const ID_QUIT: u16 = 131;
const ID_VOLUME: u16 = 200;
const ID_MUTE: u16 = 210;
const ID_SCALE: u16 = 220;
const ID_AUTOSAVE: u16 = 230;
const ID_NEVER_SLEEP: u16 = 250;
const ID_PAUSE_TIME: u16 = 251;
const ID_THEME: u16 = 260;
const ID_DESK_MODE: u16 = 270;
const ID_DESK_TOP: u16 = 271;
const ID_DESK_NEVER_SLEEP: u16 = 272;
const ID_DESK_SCALE: u16 = 280;
const DESK_SCALES: [u32; 3] = [2, 3, 4];
const ID_TRAY_TOGGLE: u16 = 273;
const ID_LABELS: u16 = 290;
const ID_SYNC_CLOCK: u16 = 291;
const ID_ROM: u16 = 1000;
const VOLUMES: [u32; 5] = [20, 40, 60, 80, 100];
const SCALES: [u32; 5] = [2, 3, 4, 5, 6];
const AUTOSAVES: [u32; 5] = [0, 1, 2, 5, 10];

const WINDOW_STYLE: u32 = WS_OVERLAPPED | WS_CAPTION | WS_SYSMENU | WS_MINIMIZEBOX;

/// Input from the window procedure, handled in the main loop.
enum Ui {
    KeyDown(usize),
    KeyUp(usize),
    Mouse(bool, i32, i32),
    Command(u16),
    FocusLost,
    Close,
    /// Left button on the desktop toy (down?, client x, y).
    DeskMouse(bool, i32, i32),
    /// Right-click on the desktop toy (screen x, y).
    DeskMenu(i32, i32),
    /// The desktop toy was dragged somewhere else.
    DeskMoved,
    /// The tray icon was clicked / right-clicked.
    TrayClick,
    TrayMenu,
    /// Explorer restarted: the tray icon has to be added again.
    TrayLost,
}

#[derive(Clone, Default)]
struct Layout {
    dpi: f64,
    width: i32,
    height: i32,
    screen: RECT,
    buttons: [(i32, i32, i32); 3],
    status: RECT,
    keys: RECT,
}

impl Layout {
    fn new(scale: u32, dpi: f64) -> Layout {
        let d = |v: f64| (v * dpi).round() as i32;
        let pixel = ((scale as f64 * dpi).round() as i32).max(1);
        let s = 128 * pixel;
        let pad = d(24.0);
        let width = s + 2 * pad;
        let screen = RECT { left: pad, top: pad, right: pad + s, bottom: pad + s };
        let top = screen.bottom + d(12.0);
        let mut buttons = [(0, 0, 0); 3];
        for (i, b) in buttons.iter_mut().enumerate() {
            let cy = top + if i == 1 { d(30.0) } else { d(50.0) }; // B sits higher, like the toy
            *b = (pad + s * (i as i32 + 1) / 4, cy, d(28.0));
        }
        let st = top + d(90.0) + d(4.0);
        let status = RECT { left: 0, top: st, right: width, bottom: st + d(20.0) };
        let keys = RECT { left: 0, top: status.bottom, right: width, bottom: status.bottom + d(20.0) };
        Layout { dpi, width, height: keys.bottom + d(12.0), screen, buttons, status, keys }
    }
}

/// What the window procedure needs to paint.
struct Paint {
    pixels: Vec<u32>,
    layout: Layout,
    status: String,
    hint: String,
    down: [bool; 3],
    theme: Theme,
    /// Letters on the buttons.
    labels: bool,
    /// The toy is asleep: `pixels` holds the sleep screen.
    asleep: bool,
}

/// A right-click menu to show from the main loop (not from inside the
/// app, so the emulator can keep running while it is open).
struct Popup {
    menu: HMENU,
    owner: HWND,
    x: i32,
    y: i32,
    /// A menu made just for this (freed afterwards).
    temporary: bool,
}

/// Timer that keeps the emulator running while Windows runs its own loop:
/// a menu is open, or a window is being dragged.
const TIMER_ID: usize = 7;
const TIMER_MS: u32 = 10;

thread_local! {
    /// The app, reachable from the window procedure for the timer.
    static APP: RefCell<Option<App>> = RefCell::new(None);
    /// Shape of the desktop toy, for the window procedure's hit test.
    static DESK_GEO: RefCell<Option<desk::Geo>> = RefCell::new(None);
    /// Message Explorer sends when the taskbar is (re)created.
    static TASKBAR_CREATED: Cell<u32> = Cell::new(u32::MAX);
    static EVENTS: RefCell<VecDeque<Ui>> = RefCell::new(VecDeque::new());
    static PAINT: RefCell<Paint> = RefCell::new(Paint {
        pixels: vec![0; 128 * 128],
        layout: Layout::default(),
        status: String::new(),
        hint: String::new(),
        down: [false; 3],
        theme: THEMES[0],
        labels: true,
        asleep: false,
    });
}

fn push(ev: Ui) {
    EVENTS.with(|e| e.borrow_mut().push_back(ev));
}

unsafe extern "system" fn wndproc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    if msg == tray::WM_TRAY {
        match (lp & 0xFFFF) as u32 {
            WM_LBUTTONUP => push(Ui::TrayClick),
            WM_RBUTTONUP => push(Ui::TrayMenu),
            _ => {}
        }
        return 0;
    }
    if msg == TASKBAR_CREATED.with(|c| c.get()) {
        push(Ui::TrayLost);
        return 0;
    }
    match msg {
        WM_TIMER if wp == TIMER_ID => {
            // only does something while the main loop is held up (a menu or
            // a drag); a busy app (e.g. showing a message) is left alone
            APP.with(|a| {
                if let Ok(mut a) = a.try_borrow_mut() {
                    if let Some(app) = a.as_mut() {
                        app.tick_if_due();
                    }
                }
            });
            0
        }
        WM_PAINT => {
            paint(hwnd);
            0
        }
        WM_ERASEBKGND => 1,
        WM_KEYDOWN => {
            if lp & (1 << 30) == 0 {
                push(Ui::KeyDown(wp)); // not keyboard auto-repeat
            }
            0
        }
        WM_KEYUP => {
            push(Ui::KeyUp(wp));
            0
        }
        WM_LBUTTONDOWN | WM_LBUTTONUP => {
            let x = (lp & 0xFFFF) as i16 as i32;
            let y = ((lp >> 16) & 0xFFFF) as i16 as i32;
            push(Ui::Mouse(msg == WM_LBUTTONDOWN, x, y));
            if msg == WM_LBUTTONDOWN {
                SetCapture(hwnd);
            } else {
                ReleaseCapture();
            }
            0
        }
        WM_COMMAND => {
            push(Ui::Command((wp & 0xFFFF) as u16));
            0
        }
        WM_KILLFOCUS => {
            push(Ui::FocusLost);
            0
        }
        WM_CLOSE => {
            push(Ui::Close);
            0
        }
        WM_DESTROY => {
            PostQuitMessage(0);
            0
        }
        _ => DefWindowProcW(hwnd, msg, wp, lp),
    }
}

/// The desktop toy's window: dragged by its shell (Windows moves it when
/// the shell counts as a title bar), buttons clicked, right-click menu.
unsafe extern "system" fn desk_wndproc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    let xy = |lp: LPARAM| ((lp & 0xFFFF) as i16 as i32, ((lp >> 16) & 0xFFFF) as i16 as i32);
    match msg {
        WM_NCHITTEST => {
            let (sx, sy) = xy(lp);
            let mut r = RECT::default();
            GetWindowRect(hwnd, &mut r);
            let on_button =
                DESK_GEO.with(|g| g.borrow().as_ref().and_then(|g| g.button_at(sx - r.left, sy - r.top)).is_some());
            if on_button { HTCLIENT } else { HTCAPTION }
        }
        WM_LBUTTONDOWN | WM_LBUTTONUP => {
            let (x, y) = xy(lp);
            push(Ui::DeskMouse(msg == WM_LBUTTONDOWN, x, y));
            if msg == WM_LBUTTONDOWN {
                SetCapture(hwnd);
            } else {
                ReleaseCapture();
            }
            0
        }
        WM_NCRBUTTONUP | WM_CONTEXTMENU => {
            let (mut x, mut y) = xy(lp);
            if x == -1 && y == -1 {
                // from the keyboard: at the toy's corner
                let mut r = RECT::default();
                GetWindowRect(hwnd, &mut r);
                (x, y) = (r.left + 20, r.top + 20);
            }
            push(Ui::DeskMenu(x, y));
            0
        }
        WM_EXITSIZEMOVE => {
            push(Ui::DeskMoved);
            0
        }
        WM_NCLBUTTONDBLCLK => 0, // no maximising by double-click
        WM_PAINT => {
            // drawn with UpdateLayeredWindow; nothing to paint here
            let mut ps: PAINTSTRUCT = std::mem::zeroed();
            BeginPaint(hwnd, &mut ps);
            EndPaint(hwnd, &ps);
            0
        }
        WM_DESTROY => 0, // only the main window ends the program
        WM_KEYDOWN | WM_KEYUP | WM_COMMAND | WM_KILLFOCUS | WM_CLOSE => wndproc(hwnd, msg, wp, lp),
        _ => DefWindowProcW(hwnd, msg, wp, lp),
    }
}

unsafe fn paint(hwnd: HWND) {
    let mut ps: PAINTSTRUCT = std::mem::zeroed();
    let hdc = BeginPaint(hwnd, &mut ps);
    PAINT.with(|p| {
        let p = p.borrow();
        let l = &p.layout;
        let (w, h) = (l.width, l.height);
        let mem = CreateCompatibleDC(hdc);
        let bmp = CreateCompatibleBitmap(hdc, w, h);
        let old_bmp = SelectObject(mem, bmp);
        let th = p.theme;
        let body = CreateSolidBrush(th.body);
        FillRect(mem, &RECT { left: 0, top: 0, right: w, bottom: h }, body);
        DeleteObject(body);

        let mut info = BITMAPINFO::default();
        info.bmiHeader.biSize = std::mem::size_of::<BITMAPINFOHEADER>() as u32;
        info.bmiHeader.biWidth = 128;
        info.bmiHeader.biHeight = -128; // top-down
        info.bmiHeader.biPlanes = 1;
        info.bmiHeader.biBitCount = 32;
        SetStretchBltMode(mem, COLORONCOLOR);
        let s = l.screen;
        StretchDIBits(mem, s.left, s.top, s.right - s.left, s.bottom - s.top, 0, 0, 128, 128,
                      p.pixels.as_ptr() as *const _, &info, DIB_RGB_COLORS, SRCCOPY);

        let px = |v: f64| -((v * l.dpi).round() as i32);
        let font = CreateFontW(px(12.0), 0, 0, 0, 400, 0, 0, 0, 1, 0, 0, 5, 0, wide("Segoe UI").as_ptr());
        let bold = CreateFontW(px(21.0), 0, 0, 0, 700, 0, 0, 0, 1, 0, 0, 5, 0, wide("Segoe UI").as_ptr());
        let hint_font = CreateFontW(px(15.0), 0, 0, 0, 400, 0, 0, 0, 1, 0, 0, 5, 0, wide("Segoe UI").as_ptr());
        SetBkMode(mem, TRANSPARENT);
        if p.asleep && p.hint.is_empty() {
            SetTextColor(mem, rgb(0xc9cde6));
            let of = SelectObject(mem, hint_font);
            let pad = (12.0 * l.dpi) as i32;
            let top = s.top + (s.bottom - s.top) * SLEEP_TEXT_TOP / 128;
            let mut r = RECT { left: s.left + pad, top, right: s.right - pad, bottom: s.bottom - pad };
            draw_text(mem, SLEEP_TEXT, &mut r, DT_CENTER | DT_WORDBREAK);
            SelectObject(mem, of);
        }
        if !p.hint.is_empty() {
            SetTextColor(mem, rgb(0xffffff));
            let of = SelectObject(mem, hint_font);
            // several lines, wrapped and centred on the screen
            let mut r = s;
            r.left += 20;
            r.right -= 20;
            let flags = DT_CENTER | DT_WORDBREAK | DT_EDITCONTROL;
            let mut size = r;
            draw_text(mem, &p.hint, &mut size, flags | DT_CALCRECT);
            r.top = (s.top + (s.bottom - s.top - (size.bottom - size.top)) / 2).max(s.top);
            draw_text(mem, &p.hint, &mut r, flags);
            SelectObject(mem, of);
        }
        let pen = CreatePen(PS_SOLID, ((2.0 * l.dpi).round() as i32).max(1), th.ink);
        let old_pen = SelectObject(mem, pen);
        SetTextColor(mem, th.ink);
        let of = SelectObject(mem, bold);
        for (i, &(cx, cy, r)) in l.buttons.iter().enumerate() {
            let brush = CreateSolidBrush(if p.down[i] { th.button_down } else { th.button });
            let ob = SelectObject(mem, brush);
            Ellipse(mem, cx - r, cy - r, cx + r, cy + r);
            SelectObject(mem, ob);
            DeleteObject(brush);
            if p.labels {
                let mut rr = RECT { left: cx - r, top: cy - r, right: cx + r, bottom: cy + r };
                draw_text(mem, ["A", "B", "C"][i], &mut rr, DT_CENTER | DT_VCENTER | DT_SINGLELINE);
            }
        }
        SelectObject(mem, font);
        for (text, rect) in [(&p.status[..], l.status), (KEYS_HINT, l.keys)] {
            let mut r = rect;
            draw_text(mem, text, &mut r, DT_CENTER | DT_VCENTER | DT_SINGLELINE);
        }
        SelectObject(mem, of);
        SelectObject(mem, old_pen);
        DeleteObject(pen);
        DeleteObject(font);
        DeleteObject(bold);
        DeleteObject(hint_font);
        BitBlt(hdc, 0, 0, w, h, mem, 0, 0, SRCCOPY);
        SelectObject(mem, old_bmp);
        DeleteObject(bmp);
        DeleteDC(mem);
    });
    EndPaint(hwnd, &ps);
}

fn key_for(vk: usize) -> Option<Key> {
    match vk {
        0x41 | VK_LEFT => Some(Key::A),
        0x42 | VK_DOWN => Some(Key::B),
        0x43 | VK_RIGHT => Some(Key::C),
        _ => None,
    }
}

fn log(msg: &str) {
    use std::io::Write;
    if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(store::app_dir().join("tg18.log")) {
        let (y, mo, d, h, mi, s, _) = tg18::local_time();
        let _ = writeln!(f, "{}-{:02}-{:02} {:02}:{:02}:{:02}  {}", y, mo, d, h, mi, s, msg);
    }
}

/// '3 days', '5 hours', '12 minutes' ...
fn describe(seconds: f64) -> String {
    for (unit, size) in [("day", 86400.0), ("hour", 3600.0), ("minute", 60.0)] {
        if seconds >= size {
            let n = (seconds / size) as u64;
            return format!("{} {}{}", n, unit, if n > 1 { "s" } else { "" });
        }
    }
    format!("{} seconds", seconds as u64)
}

fn clock_hhmm() -> String {
    let (_, _, _, h, m, _, _) = tg18::local_time();
    format!("{:02}:{:02}", h, m)
}

struct Menus {
    /// Right-click menu of the desktop toy.
    desk: HMENU,
    rom: HMENU,
    save_slot: HMENU,
    load_slot: HMENU,
    settings: HMENU,
    bar: HMENU,
}

struct App {
    hwnd: HWND,
    settings: Settings,
    writer: Writer,
    menus: Menus,
    roms: Vec<(String, String)>,
    emu: Option<Machine>,
    image: Option<Arc<Vec<u8>>>,
    store: Option<SaveStore>,
    rom_path: Option<PathBuf>,
    down: [bool; 3],
    mouse_key: Option<Key>,
    message_until: Option<Instant>,
    stopped: Option<&'static str>,
    rewind: Option<Snapshot>,
    rewind_at: Option<Instant>,
    synth: ToneSynth,
    audio: Option<audio::WaveOut>,
    last_wall: Instant,
    behind: f64,
    speed_window: (Instant, u64),
    audio_t: Option<f64>,
    pending: Vec<i16>,
    frames_seen: u64,
    was_asleep: bool,
    next_autosave: Instant,
    /// When to look in the ROM folder again for new or removed dumps.
    next_rom_scan: Instant,
    /// The toy on the desktop, in desktop mode.
    desk: Option<desk::Desk>,
    /// Hidden from the tray icon (still running).
    desk_hidden: bool,
    tray: Option<tray::Tray>,
    last_press: Instant,
    /// When the desktop toy last started to wiggle, and its offset now.
    wiggle: Option<Instant>,
    wiggle_dx: i32,
    /// Next emulator step (main loop or timer).
    next_tick: Instant,
    popup: Option<Popup>,
    title: String,
    quit: bool,
}

impl App {
    fn new(settings: Settings) -> App {
        unsafe {
            SetProcessDPIAware();
            CoInitializeEx(std::ptr::null(), COINIT_APARTMENTTHREADED);
            timeBeginPeriod(1);
        }
        let dpi = unsafe {
            let dc = GetDC(0);
            let v = GetDeviceCaps(dc, LOGPIXELSX);
            ReleaseDC(0, dc);
            if v > 0 { v as f64 / 96.0 } else { 1.0 }
        };
        let layout = Layout::new(settings.scale, dpi);
        PAINT.with(|p| {
            let mut p = p.borrow_mut();
            p.layout = layout.clone();
            p.theme = theme(&settings.theme);
            p.labels = settings.button_labels;
        });
        let class = wide("tg18emu");
        let hwnd = unsafe {
            let inst = GetModuleHandleW(std::ptr::null());
            let wc = WNDCLASSEXW {
                cbSize: std::mem::size_of::<WNDCLASSEXW>() as u32,
                style: CS_HREDRAW | CS_VREDRAW,
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
            RegisterClassExW(&wc);
            let (w, h) = Self::outer_size(&layout);
            CreateWindowExW(0, class.as_ptr(), wide("tg18 emulator").as_ptr(), WINDOW_STYLE,
                            CW_USEDEFAULT, CW_USEDEFAULT, w, h, 0, 0, inst, std::ptr::null())
        };
        let menus = Self::build_menus(hwnd);
        unsafe { SetTimer(hwnd, TIMER_ID, TIMER_MS, std::ptr::null()) };
        let tc = unsafe { RegisterWindowMessageW(wide("TaskbarCreated").as_ptr()) };
        TASKBAR_CREATED.with(|c| c.set(tc));
        let synth = ToneSynth::new(AUDIO_RATE, settings.volume as f64 / 100.0);
        let audio = audio::WaveOut::open(AUDIO_RATE);
        let now = Instant::now();
        let mut app = App {
            hwnd,
            settings,
            writer: Writer::new(),
            menus,
            roms: Vec::new(),
            emu: None,
            image: None,
            store: None,
            rom_path: None,
            down: [false; 3],
            mouse_key: None,
            message_until: None,
            stopped: None,
            rewind: None,
            rewind_at: None,
            synth,
            audio,
            last_wall: now,
            behind: 0.0,
            speed_window: (now, 0),
            audio_t: None,
            pending: Vec::new(),
            frames_seen: u64::MAX,
            was_asleep: false,
            next_autosave: now,
            next_rom_scan: now + ROM_SCAN,
            desk: None,
            desk_hidden: false,
            tray: None,
            last_press: now,
            wiggle: None,
            wiggle_dx: 0,
            next_tick: now,
            popup: None,
            title: "tg18 emulator".to_string(),
            quit: false,
        };
        app.update_menu_checks();
        app.fill_rom_menu();
        app.fill_slot_menus();
        if app.settings.desk_mode {
            app.enter_desk();
        } else {
            unsafe {
                ShowWindow(hwnd, SW_SHOW);
                SetForegroundWindow(hwnd);
            }
        }
        app
    }

    fn outer_size(l: &Layout) -> (i32, i32) {
        let mut r = RECT { left: 0, top: 0, right: l.width, bottom: l.height };
        unsafe { AdjustWindowRectEx(&mut r, WINDOW_STYLE, 1, 0) };
        (r.right - r.left, r.bottom - r.top)
    }

    // --- menus and settings ----------------------------------------------------

    fn build_menus(hwnd: HWND) -> Menus {
        unsafe {
            let bar = CreateMenu();
            let file = CreatePopupMenu();
            let rom = CreatePopupMenu();
            let save_slot = CreatePopupMenu();
            let load_slot = CreatePopupMenu();
            let add = |m: HMENU, id: u16, text: &str| AppendMenuW(m, MF_STRING, id as usize, wide(text).as_ptr());
            let sub = |m: HMENU, s: HMENU, text: &str| AppendMenuW(m, MF_POPUP, s as usize, wide(text).as_ptr());
            let sep = |m: HMENU| AppendMenuW(m, MF_SEPARATOR, 0, std::ptr::null());
            sub(file, rom, "Open ROM");
            add(file, ID_OPEN_ROMS, "Open ROM folder");
            add(file, ID_CHOOSE_FOLDER, "Choose ROM folder\u{2026}");
            sep(file);
            add(file, ID_SAVE_NOW, "Save now\tCtrl+S");
            sub(file, save_slot, "Save to slot");
            sub(file, load_slot, "Load slot");
            add(file, ID_OPEN_SAVES, "Open saves folder");
            sep(file);
            add(file, ID_QUIT, "Quit");
            sub(bar, file, "File");

            // a Settings menu for each mode, with only what applies there
            let settings = Self::build_settings(false);
            sub(bar, settings, "Settings");
            SetMenu(hwnd, bar);

            // the desktop toy's right-click menu shares File
            let desk = CreatePopupMenu();
            sub(desk, file, "File");
            sub(desk, Self::build_settings(true), "Settings");
            sep(desk);
            add(desk, ID_DESK_TOP, "Always on top");
            add(desk, ID_DESK_MODE, "Back to the window");
            add(desk, ID_QUIT, "Quit");
            Menus { desk, rom, save_slot, load_slot, settings, bar }
        }
    }

    /// The Settings menu of the window (`desk` false) or the desktop toy.
    fn build_settings(desk: bool) -> HMENU {
        unsafe {
            let add = |m: HMENU, id: u16, text: &str| AppendMenuW(m, MF_STRING, id as usize, wide(text).as_ptr());
            let sub = |m: HMENU, s: HMENU, text: &str| AppendMenuW(m, MF_POPUP, s as usize, wide(text).as_ptr());
            let sep = |m: HMENU| AppendMenuW(m, MF_SEPARATOR, 0, std::ptr::null());
            let settings = CreatePopupMenu();
            let vol = CreatePopupMenu();
            for (i, v) in VOLUMES.iter().enumerate() {
                add(vol, ID_VOLUME + i as u16, &format!("{}%", v));
            }
            sub(settings, vol, "Volume");
            add(settings, ID_MUTE, "Mute\tM");
            let size = CreatePopupMenu();
            if desk {
                for (i, v) in DESK_SCALES.iter().enumerate() {
                    add(size, ID_DESK_SCALE + i as u16, &format!("{}\u{d7}", v));
                }
                sub(settings, size, "Size");
            } else {
                for (i, v) in SCALES.iter().enumerate() {
                    add(size, ID_SCALE + i as u16, &format!("{}\u{d7} ({} px)", v, v * 128));
                }
                sub(settings, size, "Screen size");
            }
            let colour = CreatePopupMenu();
            for (i, t) in THEMES.iter().enumerate() {
                add(colour, ID_THEME + i as u16, t.label);
            }
            sub(settings, colour, "Colour");
            add(settings, ID_LABELS, "Show A, B, C on the buttons");
            let auto = CreatePopupMenu();
            for (i, v) in AUTOSAVES.iter().enumerate() {
                let label = match v {
                    0 => "Off".to_string(),
                    1 => "Every minute".to_string(),
                    v => format!("Every {} minutes", v),
                };
                add(auto, ID_AUTOSAVE + i as u16, &label);
            }
            sub(settings, auto, "Autosave");
            sep(settings);
            // each mode has its own never-sleep setting
            add(settings, if desk { ID_DESK_NEVER_SLEEP } else { ID_NEVER_SLEEP }, "Never sleep (keep the screen on)");
            add(settings, ID_PAUSE_TIME, "Stop the toy's clock while closed");
            add(settings, ID_SYNC_CLOCK, "Set the toy's clock to Windows time now");
            if !desk {
                sep(settings);
                add(settings, ID_DESK_MODE, "Put the toy on the desktop");
            }
            settings
        }
    }

    fn update_menu_checks(&self) {
        let s = &self.settings;
        let check = |id: u16, on: bool| unsafe {
            let flag = MF_BYCOMMAND | if on { MF_CHECKED } else { MF_UNCHECKED };
            CheckMenuItem(self.menus.bar, id as u32, flag);
            CheckMenuItem(self.menus.desk, id as u32, flag);
        };
        for (i, v) in VOLUMES.iter().enumerate() {
            check(ID_VOLUME + i as u16, *v == s.volume);
        }
        for (i, v) in SCALES.iter().enumerate() {
            check(ID_SCALE + i as u16, *v == s.scale);
        }
        for (i, v) in AUTOSAVES.iter().enumerate() {
            check(ID_AUTOSAVE + i as u16, *v == s.autosave_minutes);
        }
        check(ID_MUTE, s.muted);
        check(ID_NEVER_SLEEP, s.never_sleep);
        check(ID_PAUSE_TIME, s.pause_time_when_closed);
        check(ID_LABELS, s.button_labels);
        check(ID_DESK_TOP, s.desk_on_top);
        check(ID_DESK_NEVER_SLEEP, s.desk_never_sleep);
        for (i, v) in DESK_SCALES.iter().enumerate() {
            check(ID_DESK_SCALE + i as u16, *v == s.desk_scale);
        }

        let current = theme(&s.theme).key;
        for (i, t) in THEMES.iter().enumerate() {
            check(ID_THEME + i as u16, t.key == current);
        }
        let current = self.rom_path.as_ref().and_then(|p| p.file_name()).map(|n| n.to_string_lossy().to_string());
        for (i, (f, _)) in self.roms.iter().enumerate() {
            check(ID_ROM + i as u16, Some(f) == current.as_ref());
        }
        let _ = self.menus.settings;
    }

    fn clear_menu(m: HMENU) {
        unsafe {
            while GetMenuItemCount(m) > 0 {
                DeleteMenu(m, 0, MF_BYPOSITION);
            }
        }
    }

    fn fill_rom_menu(&mut self) {
        let m = self.menus.rom;
        Self::clear_menu(m);
        self.roms = store::list_roms(&self.settings.rom_folder());
        unsafe {
            if self.roms.is_empty() {
                AppendMenuW(m, MF_STRING | MF_GRAYED, 0, wide("(no ROMs: put your dumps in the ROM folder)").as_ptr());
            }
            for (i, (_, name)) in self.roms.iter().enumerate() {
                AppendMenuW(m, MF_STRING, (ID_ROM + i as u16) as usize, wide(name).as_ptr());
            }
        }
        self.update_menu_checks();
    }

    fn fill_slot_menus(&self) {
        for (saving, m) in [(true, self.menus.save_slot), (false, self.menus.load_slot)] {
            Self::clear_menu(m);
            for n in 1..=store::SLOTS {
                let when = self.store.as_ref().and_then(|s| s.slot_time(n));
                let label = format!("Slot {}: {}", n, when.map_or("empty".to_string(), format_time));
                let enabled = if saving { self.emu.is_some() } else { when.is_some() };
                let id = if saving { ID_SAVE_SLOT } else { ID_LOAD_SLOT } + n as u16;
                unsafe {
                    AppendMenuW(m, MF_STRING | if enabled { MF_ENABLED } else { MF_GRAYED }, id as usize, wide(&label).as_ptr());
                }
            }
        }
    }

    fn command(&mut self, id: u16) {
        match id {
            ID_CHOOSE_FOLDER => self.choose_folder(),
            ID_OPEN_ROMS => {
                let dir = self.settings.rom_folder();
                let _ = std::fs::create_dir_all(&dir);
                open_in_explorer(&dir.to_string_lossy());
            }
            ID_SAVE_NOW => self.save_now(None, "Saved"),
            ID_OPEN_SAVES => {
                let dir = self.store.as_ref().map(|s| s.dir.clone()).unwrap_or_else(store::saves_dir);
                let _ = std::fs::create_dir_all(&dir);
                open_in_explorer(&dir.to_string_lossy());
            }
            ID_QUIT => self.close(),
            ID_MUTE => {
                self.settings.muted = !self.settings.muted;
                self.changed();
            }
            ID_NEVER_SLEEP => {
                self.settings.never_sleep = !self.settings.never_sleep;
                self.apply_never_sleep();
                self.changed();
            }
            ID_DESK_NEVER_SLEEP => {
                self.settings.desk_never_sleep = !self.settings.desk_never_sleep;
                self.apply_never_sleep();
                self.changed();
            }
            ID_TRAY_TOGGLE => self.toggle_desk_hidden(),
            ID_DESK_MODE => {
                if self.desk.is_some() {
                    self.leave_desk();
                } else {
                    self.enter_desk();
                }
            }
            ID_DESK_TOP => {
                self.settings.desk_on_top = !self.settings.desk_on_top;
                if let Some(d) = self.desk.as_ref() {
                    d.set_on_top(self.settings.desk_on_top);
                }
                self.changed();
            }
            _ if (ID_DESK_SCALE..ID_DESK_SCALE + DESK_SCALES.len() as u16).contains(&id) => {
                self.settings.desk_scale = DESK_SCALES[(id - ID_DESK_SCALE) as usize];
                if self.desk.is_some() {
                    self.save_desk_position();
                    self.desk = None;
                    self.enter_desk(); // rebuilt at the new size, same place
                }
                self.changed();
            }
            ID_LABELS => {
                self.settings.button_labels = !self.settings.button_labels;
                let on = self.settings.button_labels;
                PAINT.with(|p| p.borrow_mut().labels = on);
                unsafe { InvalidateRect(self.hwnd, std::ptr::null(), 0) };
                if let Some(d) = self.desk.as_mut() {
                    d.set_labels(on);
                }
                self.render_desk();
                self.changed();
            }
            ID_SYNC_CLOCK => self.sync_clock(),
            ID_PAUSE_TIME => {
                self.settings.pause_time_when_closed = !self.settings.pause_time_when_closed;
                self.changed();
            }
            _ if (ID_VOLUME..ID_VOLUME + 5).contains(&id) => {
                self.settings.volume = VOLUMES[(id - ID_VOLUME) as usize];
                self.synth.set_volume(self.settings.volume as f64 / 100.0);
                self.changed();
            }
            _ if (ID_THEME..ID_THEME + THEMES.len() as u16).contains(&id) => {
                let t = THEMES[(id - ID_THEME) as usize];
                self.settings.theme = t.key.to_string();
                PAINT.with(|p| p.borrow_mut().theme = t);
                unsafe { InvalidateRect(self.hwnd, std::ptr::null(), 0) };
                if let Some(d) = self.desk.as_mut() {
                    d.set_theme(t);
                }
                self.render_desk();
                self.changed();
            }
            _ if (ID_SCALE..ID_SCALE + 5).contains(&id) => {
                self.settings.scale = SCALES[(id - ID_SCALE) as usize];
                self.relayout();
                self.changed();
            }
            _ if (ID_AUTOSAVE..ID_AUTOSAVE + 5).contains(&id) => {
                self.settings.autosave_minutes = AUTOSAVES[(id - ID_AUTOSAVE) as usize];
                self.next_autosave = Instant::now() + Duration::from_secs(self.settings.autosave_minutes as u64 * 60);
                self.changed();
            }
            _ if id > ID_SAVE_SLOT && id <= ID_SAVE_SLOT + store::SLOTS as u16 => {
                let n = (id - ID_SAVE_SLOT) as usize;
                if let Some(path) = self.store.as_ref().map(|s| s.slot(n)) {
                    self.save_now(Some(path), &format!("Saved to slot {}", n));
                    self.writer.wait();
                    self.fill_slot_menus();
                }
            }
            _ if id > ID_LOAD_SLOT && id <= ID_LOAD_SLOT + store::SLOTS as u16 => self.load_slot((id - ID_LOAD_SLOT) as usize),
            _ if id >= ID_ROM && ((id - ID_ROM) as usize) < self.roms.len() => {
                let file = self.roms[(id - ID_ROM) as usize].0.clone();
                self.switch_rom(&file);
            }
            _ => {}
        }
    }

    fn changed(&mut self) {
        self.settings.store();
        self.update_menu_checks();
    }

    fn relayout(&mut self) {
        let dpi = PAINT.with(|p| p.borrow().layout.dpi);
        let layout = Layout::new(self.settings.scale, dpi);
        let (w, h) = Self::outer_size(&layout);
        PAINT.with(|p| p.borrow_mut().layout = layout);
        unsafe {
            SetWindowPos(self.hwnd, 0, 0, 0, w, h, 0x0002 | 0x0004); // no move, no z-order
            InvalidateRect(self.hwnd, std::ptr::null(), 0);
        }
    }

    fn say(&mut self, text: &str, seconds: f64) {
        self.message_until = Some(Instant::now() + Duration::from_secs_f64(seconds));
        self.set_status(text);
        unsafe { UpdateWindow(self.hwnd) };
    }

    fn set_status(&self, text: &str) {
        let rect = PAINT.with(|p| {
            let mut p = p.borrow_mut();
            p.status = text.to_string();
            p.layout.status
        });
        unsafe { InvalidateRect(self.hwnd, &rect, 0) };
    }

    fn show_hint(&mut self, text: &str) {
        PAINT.with(|p| p.borrow_mut().hint = text.to_string());
        unsafe { InvalidateRect(self.hwnd, std::ptr::null(), 0) };
        self.render_desk();
    }

    fn set_title(&mut self, text: &str) {
        self.title = text.to_string();
        unsafe { SetWindowTextW(self.hwnd, wide(text).as_ptr()) };
        if let Some(d) = self.desk.as_ref() {
            d.set_title(text);
        }
        if let Some(t) = self.tray.as_ref() {
            t.update(text);
        }
    }

    /// The window dialogs belong to: the desktop toy in desktop mode.
    fn owner(&self) -> HWND {
        self.desk.as_ref().map_or(self.hwnd, |d| d.hwnd)
    }

    // --- desktop mode ----------------------------------------------------------

    /// Put the toy on the desktop (where it was left last time) and hide the window.
    fn enter_desk(&mut self) {
        self.release_all();
        let dpi = PAINT.with(|p| p.borrow().layout.dpi);
        let s = &self.settings;
        let geo = desk::Geo::new(s.desk_scale, dpi);
        let (x, y) = match (s.desk_x, s.desk_y) {
            // where it was left, moved in if a bigger size or another screen
            // setup would put part of it off the screen
            (Some(x), Some(y)) if desk::on_screen(x, y, geo.w, geo.h) => desk::fit_on_screen(x, y, geo.w, geo.h),
            _ => desk::default_position(geo.w, geo.h),
        };
        let d = desk::Desk::new(desk_wndproc, s.desk_scale, dpi, theme(&s.theme), s.button_labels, x, y, s.desk_on_top);
        d.set_title(&self.title);
        DESK_GEO.with(|g| *g.borrow_mut() = Some(d.geo.clone()));
        let hwnd = d.hwnd;
        self.desk = Some(d);
        self.desk_hidden = false;
        if self.tray.is_none() {
            self.tray = Some(tray::Tray::add(self.hwnd, &self.title));
        }
        self.render_desk();
        unsafe {
            ShowWindow(hwnd, SW_SHOW);
            SetForegroundWindow(hwnd);
            ShowWindow(self.hwnd, SW_HIDE);
        }
        self.settings.desk_mode = true;
        self.apply_never_sleep();
        self.changed();
    }

    /// Back to the normal window.
    fn leave_desk(&mut self) {
        self.release_all();
        self.save_desk_position();
        self.desk = None;
        self.tray = None;
        DESK_GEO.with(|g| *g.borrow_mut() = None);
        unsafe {
            ShowWindow(self.hwnd, SW_SHOW);
            SetForegroundWindow(self.hwnd);
        }
        self.settings.desk_mode = false;
        self.apply_never_sleep();
        self.changed();
        self.draw();
    }

    fn save_desk_position(&mut self) {
        if let Some(d) = self.desk.as_ref() {
            let (x, y) = d.position();
            self.settings.desk_x = Some(x);
            self.settings.desk_y = Some(y);
        }
    }

    /// Never-sleep has its own setting for each mode.
    fn apply_never_sleep(&mut self) {
        let s = &self.settings;
        let on = if self.desk.is_some() { s.desk_never_sleep } else { s.never_sleep };
        if let Some(e) = self.emu.as_mut() {
            e.never_sleep = on;
        }
    }

    /// Redraw the desktop toy (screen, buttons, message), if it is shown.
    fn render_desk(&mut self) {
        let down = self.down;
        let dx = self.wiggle_dx;
        if let Some(d) = self.desk.as_mut() {
            PAINT.with(|p| {
                let p = p.borrow();
                d.render(&p.pixels, down, p.asleep, &p.hint, dx);
            });
        }
    }

    /// Show or hide the desktop toy (it keeps running while hidden).
    fn toggle_desk_hidden(&mut self) {
        let hwnd = match self.desk.as_ref() {
            Some(d) => d.hwnd,
            None => return,
        };
        self.release_all();
        self.desk_hidden = !self.desk_hidden;
        unsafe {
            if self.desk_hidden {
                ShowWindow(hwnd, SW_HIDE);
            } else {
                ShowWindow(hwnd, SW_SHOW);
                SetForegroundWindow(hwnd);
            }
        }
        self.render_desk();
    }

    fn tray_menu(&mut self) {
        let mut pt = POINT::default();
        unsafe {
            GetCursorPos(&mut pt);
            let m = CreatePopupMenu();
            let toggle = if self.desk_hidden { "Show the toy" } else { "Hide the toy" };
            AppendMenuW(m, MF_STRING, ID_TRAY_TOGGLE as usize, wide(toggle).as_ptr());
            AppendMenuW(m, MF_STRING, ID_DESK_MODE as usize, wide("Back to the window").as_ptr());
            AppendMenuW(m, MF_SEPARATOR, 0, std::ptr::null());
            AppendMenuW(m, MF_STRING, ID_QUIT as usize, wide("Quit").as_ptr());
            self.popup = Some(Popup { menu: m, owner: self.hwnd, x: pt.x, y: pt.y, temporary: true });
        }
    }

    /// Run the emulator if its next step is due.
    fn tick_if_due(&mut self) {
        let now = Instant::now();
        if now >= self.next_tick {
            self.next_tick = now + TICK;
            self.tick();
        }
    }

    /// A sound nobody asked for (no button pressed for a while) means the
    /// toy is calling: the desktop toy wiggles.
    fn notice_call(&mut self) {
        let emu = match self.emu.as_ref() {
            Some(e) => e,
            None => return,
        };
        let sound = emu.sys.sound.events.iter().any(|e| e.1 > 0.0);
        if sound
            && self.desk.is_some()
            && !self.desk_hidden
            && self.last_press.elapsed() >= CALL_QUIET
            && self.wiggle.map_or(true, |w| w.elapsed() >= WIGGLE_GAP)
        {
            self.wiggle = Some(Instant::now());
        }
    }

    fn update_wiggle(&mut self) {
        let start = match self.wiggle {
            Some(s) if self.desk.is_some() => s,
            _ => return,
        };
        let t = start.elapsed().as_secs_f64();
        let room = self.desk.as_ref().map_or(0, |d| d.wiggle_room()) as f64;
        let dx = if t >= WIGGLE_LEN {
            0
        } else {
            (room * 0.6 * (std::f64::consts::TAU * WIGGLE_HZ * t).sin() * (1.0 - t / WIGGLE_LEN)).round() as i32
        };
        if dx != self.wiggle_dx {
            self.wiggle_dx = dx;
            self.render_desk();
        }
    }

    fn desk_menu(&mut self, x: i32, y: i32) {
        let hwnd = match self.desk.as_ref() {
            Some(d) => d.hwnd,
            None => return,
        };
        self.release_all();
        self.fill_slot_menus();
        self.popup = Some(Popup { menu: self.menus.desk, owner: hwnd, x, y, temporary: false });
    }

    // --- ROMs ------------------------------------------------------------------

    fn start(&mut self, rom: Option<String>) {
        let _ = std::fs::create_dir_all(store::roms_dir());
        let _ = std::fs::create_dir_all(store::saves_dir());
        if let Some(r) = rom {
            let p = std::fs::canonicalize(&r).unwrap_or_else(|_| PathBuf::from(&r));
            self.open_rom(&p);
            return;
        }
        if let Some(last) = self.settings.last_rom.clone() {
            let p = self.settings.rom_folder().join(last);
            if store::is_rom(&p) {
                self.open_rom(&p);
                return;
            }
        }
        self.no_game_yet();
    }

    /// Nothing is running: open the only ROM there is, or say what to do.
    fn no_game_yet(&mut self) {
        let folder = self.settings.rom_folder();
        let _ = std::fs::create_dir_all(&folder);
        match self.roms.len() {
            0 => self.show_hint(&format!(
                "Put your tg18 ROM dumps (8 MiB .bin files) into this folder:\n\n{}\n\n\
                 (File > Open ROM folder)",
                folder.display()
            )),
            1 => {
                let file = self.roms[0].0.clone();
                self.switch_rom(&file);
            }
            _ => self.show_hint("Pick a ROM: File > Open ROM"),
        }
    }

    /// Notice dumps added to or removed from the ROM folder.
    fn rescan_roms(&mut self) {
        let names: Vec<String> = store::list_roms(&self.settings.rom_folder()).into_iter().map(|r| r.0).collect();
        if names != self.roms.iter().map(|r| r.0.clone()).collect::<Vec<_>>() {
            self.fill_rom_menu();
            if self.emu.is_none() {
                self.no_game_yet();
            }
        }
    }

    fn choose_folder(&mut self) {
        let folder = match pick_folder(self.owner(), "Folder with your tg18 ROM dumps") {
            Some(f) => f,
            None => return,
        };
        let roms = store::list_roms(Path::new(&folder));
        if roms.is_empty() {
            message_box(self.owner(), "No ROMs found",
                        &format!("No tg18 flash dumps (8 MiB .bin files starting with SPII) were found in\n{}", folder),
                        MB_OK | MB_ICONWARNING);
            return;
        }
        // the roms folder next to the program is the default: not stored, so
        // the whole folder can be moved or copied elsewhere
        self.settings.rom_dir = if store::same_dir(Path::new(&folder), &store::roms_dir()) { None } else { Some(folder) };
        self.settings.store();
        self.fill_rom_menu();
        if self.emu.is_none() {
            self.no_game_yet();
        }
    }

    fn switch_rom(&mut self, filename: &str) {
        let path = self.settings.rom_folder().join(filename);
        if self.rom_path.as_deref() == Some(path.as_path()) {
            return;
        }
        self.open_rom(&path);
    }

    fn open_rom(&mut self, path: &Path) {
        if !store::is_rom(path) {
            message_box(self.owner(), "Not a tg18 ROM", &format!("{} is not an 8 MiB tg18 flash dump.", path.display()),
                        MB_OK | MB_ICONERROR);
            return;
        }
        if self.emu.is_some() {
            self.shutdown_game();
        }
        self.emu = None;
        let image = match std::fs::read(path) {
            Ok(i) => Arc::new(i),
            Err(e) => {
                message_box(self.owner(), "Can't open the ROM", &e.to_string(), MB_OK | MB_ICONERROR);
                return;
            }
        };
        let fname = path.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
        let (shown, save_name) = store::identity(&fname, &image);
        self.say(&format!("Loading {}\u{2026}", shown), 10.0);
        let store = SaveStore::new(&save_name);
        let legacy = store.import_legacy(&image);
        let (emu, elapsed, how) = match self.boot(&store, &image) {
            Ok(b) => b,
            Err(e) => {
                message_box(self.owner(), "Save does not match", &format!("The save for this ROM can't be used: {}", e),
                            MB_OK | MB_ICONERROR);
                return;
            }
        };
        self.image = Some(image);
        self.store = Some(store);
        self.rom_path = Some(path.to_path_buf());
        self.set_emu(emu);
        // remembered for next time if it is in the ROM folder (not one opened
        // from elsewhere on the command line)
        if path.parent().map_or(false, |p| store::same_dir(p, &self.settings.rom_folder())) {
            self.settings.last_rom = Some(fname.clone());
        }
        self.settings.store();
        self.fill_rom_menu();
        self.fill_slot_menus();
        self.set_title(&format!("{} \u{2013} tg18 emulator", shown));
        self.show_hint("");
        if elapsed > 0.0 {
            self.advance_clock(elapsed);
        }
        let note = legacy.map(|l| format!(" (imported {})", l)).unwrap_or_default();
        self.say(&format!("{}{}", how, note), 4.0);
    }

    /// A machine for this ROM's newest save: (machine, seconds to catch up, message).
    fn boot(&self, store: &SaveStore, image: &Arc<Vec<u8>>) -> Result<(Machine, f64, String), SnapError> {
        let pause = self.settings.pause_time_when_closed;
        match store.newest() {
            Newest::Flash(meta) => {
                let flash = save::load_flash(&store.flash, image)?.unwrap_or_else(|| image.to_vec());
                let mut m = Machine::new(image.clone(), &flash, tg18::rtc_seconds_now());
                if let Some(meta) = &meta {
                    save::restore_rtc(&mut m, meta, !pause);
                    if meta.asleep {
                        // it went to sleep when the window closed: wake it with a
                        // button (the clock has moved on, the pet hasn't)
                        m.press(0.0, Key::B);
                        let away = tg18::unix_now() - meta.saved_at;
                        if pause || away < 60.0 {
                            return Ok((m, 0.0, "Resumed from the last session".into()));
                        }
                        return Ok((m, 0.0, format!("Resumed; clock moved forward by {}", describe(away))));
                    }
                }
                Ok((m, 0.0, "Booted from the flash save".into()))
            }
            Newest::Snapshot => {
                let mut m = Machine::new(image.clone(), image, 0.0);
                let snap = Snapshot::load(&store.autosave)?;
                snap.restore(&mut m)?;
                let elapsed = if pause { 0.0 } else { (tg18::unix_now() - snap.saved_at()).max(0.0) };
                Ok((m, elapsed, "Resumed from the autosave".into()))
            }
            Newest::Nothing => Ok((Machine::new(image.clone(), image, tg18::rtc_seconds_now()), 0.0, "New game".into())),
        }
    }

    fn set_emu(&mut self, mut emu: Machine) {
        emu.stop_on_bluetooth = true;
        self.emu = Some(emu);
        self.apply_never_sleep();
        self.rewind = None;
        self.rewind_at = None;
        self.reset_timing();
        self.next_autosave = Instant::now() + Duration::from_secs(self.settings.autosave_minutes as u64 * 60);
    }

    fn reset_timing(&mut self) {
        let now = Instant::now();
        self.last_wall = now;
        self.behind = 0.0;
        self.speed_window = (now, self.emu.as_ref().map_or(0, |e| e.executed));
        self.audio_t = None;
        self.pending.clear();
        self.frames_seen = u64::MAX;
        self.was_asleep = false;
    }

    // --- sleeping, saving, loading ------------------------------------------------

    /// Make the firmware go to sleep now, as after the idle timeout; on the
    /// way down it saves everything to flash, like the real toy. False where
    /// it doesn't sleep (e.g. during the first setup).
    fn force_sleep(&mut self, limit: f64) -> bool {
        let emu = match self.emu.as_mut() {
            Some(e) => e,
            None => return false,
        };
        if emu.sys.powered_off {
            return true;
        }
        let counter = match emu.idle_counter {
            Some(c) => c,
            None => return false,
        };
        let never = emu.never_sleep;
        emu.never_sleep = false;
        let end = emu.executed + (limit * CPU_HZ) as u64;
        while !emu.sys.powered_off && emu.executed < end {
            emu.sys.poke(counter, &[0xFE]);
            let out = emu.run(end.min(emu.executed + CPU_HZ as u64 / 10));
            if !matches!(out, Outcome::Limit | Outcome::PowerOff) {
                break;
            }
        }
        emu.never_sleep = never;
        emu.sys.sound.events.clear();
        emu.sys.powered_off
    }

    /// Move the device clock forward by the time the window was closed: put
    /// it to sleep, move the clock and wake it with a button, so the firmware
    /// reads the new time the way it does on a real wake-up. The pet itself
    /// doesn't change, by design (like a toy with the batteries out).
    fn advance_clock(&mut self, elapsed: f64) {
        self.say(&format!("Clock moved forward by {}", describe(elapsed)), 5.0);
        let add = (elapsed * 32768.0) as i64;
        if !self.change_clock(|m| m.sys.rtc.ticks(m.sys.game_time) + add) {
            if let Some(emu) = self.emu.as_mut() {
                emu.sys.rtc.base_ticks += add; // read at the next wake-up
            }
        }
    }

    /// Set the toy's clock to the computer's time, as if its batteries had
    /// been out and the clock was set again; the pet doesn't change.
    fn sync_clock(&mut self) {
        if self.emu.is_none() {
            return;
        }
        self.say("Setting the clock\u{2026}", 5.0);
        if self.change_clock(|_| (tg18::rtc_seconds_now() * 32768.0) as i64) {
            self.say(&format!("Clock set to Windows time ({})", clock_hhmm()), 4.0);
        } else {
            message_box(self.owner(), "Clock not set",
                        "The toy can't take a new time right now (for example while the pet is calling, \
                         or during the first setup). Please try again in a moment.",
                        MB_OK | MB_ICONINFORMATION);
            self.reset_timing();
        }
    }

    /// Change the toy's clock the way the real toy gets a new time: it goes
    /// to sleep (and saves itself), its clock chip is set to `ticks(machine)`
    /// (32768 per second since 2007-12-31), and a button wakes it, so the
    /// game reads the time as after any wake-up. False if it won't sleep now.
    fn change_clock(&mut self, ticks: impl Fn(&Machine) -> i64) -> bool {
        if !self.force_sleep(10.0) {
            return false;
        }
        let emu = self.emu.as_mut().unwrap();
        let shift = ticks(emu) - emu.sys.rtc.ticks(emu.sys.game_time);
        emu.sys.rtc.base_ticks += shift;
        let now = emu.now();
        emu.press(now, Key::B);
        let limit = emu.executed + CPU_HZ as u64;
        emu.power_cycle(limit);
        let emu = self.emu.take().unwrap();
        self.set_emu(emu);
        true
    }

    /// The game wants Bluetooth or infrared: undo the press that led here and explain.
    fn connection_blocked(&mut self, what: Outcome) {
        self.release_all();
        let snap = match self.rewind.take() {
            Some(s) => s,
            None => return self.bluetooth_hang(),
        };
        let image = self.image.clone().unwrap();
        let mut emu = Machine::new(image.clone(), &image, 0.0);
        if snap.restore(&mut emu).is_err() {
            return self.bluetooth_hang();
        }
        self.set_emu(emu);
        self.draw();
        let text = if what == Outcome::Bluetooth {
            "Bluetooth functions (the Tamagotchi app, the camera, downloads from the app) are not supported by the emulator."
        } else {
            "Infrared connections with another Tamagotchi (playdates, gifts, marrying, downloads) are not supported by the emulator."
        };
        message_box(self.owner(), "Not supported",
                    &format!("{}\n\nThe game has been put back to just before you chose it.", text),
                    MB_OK | MB_ICONINFORMATION);
        self.reset_timing();
    }

    /// The game froze waiting for the Bluetooth chip: restart the toy.
    fn bluetooth_hang(&mut self) {
        message_box(self.owner(), "Bluetooth is not emulated",
                    "This needs Bluetooth (it talks to the Tamagotchi phone app), which the emulator can't do yet. \
                     The game froze waiting for the Bluetooth chip, so the toy is restarted, like taking the \
                     batteries out.\n\nChoose CONTINUE to carry on from the game's last own save.",
                    MB_OK | MB_ICONINFORMATION);
        if let Some(mut emu) = self.emu.take() {
            emu.restart();
            self.set_emu(emu);
        }
        self.say("Restarted after the Bluetooth freeze", 5.0);
    }

    /// Snapshot the machine; the writing happens in the background.
    fn save_now(&mut self, path: Option<String>, label: &str) {
        if self.stopped.is_some() {
            return;
        }
        let (emu, store) = match (self.emu.as_ref(), self.store.as_ref()) {
            (Some(e), Some(s)) => (e, s),
            _ => return,
        };
        let snap = Snapshot::capture(emu);
        let dest = path.clone().unwrap_or_else(|| store.autosave.clone());
        self.writer.submit(move || snap.save(&dest).map_err(|e| format!("{}: {}", dest, e)));
        if path.is_none() {
            // also refresh the flash fallback
            let flash = emu.sys.ram[..tg18::FLASH_SIZE].to_vec();
            let meta = save::meta(emu, false, false);
            let dest = store.flash.clone();
            self.writer.submit(move || save::write(&flash, &meta, &dest).map_err(|e| format!("{}: {}", dest, e)));
        }
        self.next_autosave = Instant::now() + Duration::from_secs(self.settings.autosave_minutes as u64 * 60);
        self.say(&format!("{} at {}", label, clock_hhmm()), 3.0);
    }

    fn load_slot(&mut self, n: usize) {
        let store = match self.store.as_ref() {
            Some(s) => s,
            None => return,
        };
        let when = match store.slot_time(n) {
            Some(w) => w,
            None => return,
        };
        let path = store.slot(n);
        let q = format!("Load the save from {}?\n\nThe game you are playing now is autosaved first.", format_time(when));
        if message_box(self.owner(), &format!("Load slot {}", n), &q, MB_YESNO | MB_ICONQUESTION) != IDYES {
            return;
        }
        self.save_now(None, "Autosaved");
        self.writer.wait();
        let image = self.image.clone().unwrap();
        let mut emu = Machine::new(image.clone(), &image, 0.0);
        if let Err(e) = Snapshot::load(&path).and_then(|s| s.restore(&mut emu)) {
            message_box(self.owner(), "Save does not match", &format!("Slot {} can't be loaded: {}", n, e), MB_OK | MB_ICONERROR);
            return;
        }
        self.set_emu(emu);
        self.say(&format!("Loaded slot {}", n), 3.0);
    }

    /// Store the current game before closing it (window closed, other ROM).
    fn shutdown_game(&mut self) {
        self.say("Saving\u{2026}", 10.0);
        if self.force_sleep(10.0) {
            let emu = self.emu.as_ref().unwrap();
            let path = self.store.as_ref().unwrap().flash.clone();
            if let Err(e) = save::write_machine(emu, &path, true, true) {
                log(&format!("save error: {}", e));
            }
        } else {
            self.save_now(None, "Saved"); // no sleep: snapshot it instead
        }
        self.writer.wait();
        for e in self.writer.errors() {
            log(&format!("save error: {}", e));
        }
    }

    fn close(&mut self) {
        if self.stopped != Some("closed") {
            if self.emu.is_some() && self.stopped.is_none() {
                self.shutdown_game();
            }
            self.stopped = Some("closed");
        }
        self.audio = None;
        self.save_desk_position();
        self.desk = None;
        self.tray = None;
        self.settings.store();
        unsafe {
            timeEndPeriod(1);
            DestroyWindow(self.hwnd);
        }
        self.quit = true;
    }

    // --- input -------------------------------------------------------------------

    fn handle(&mut self, ev: Ui) {
        match ev {
            Ui::KeyDown(vk) => {
                let ctrl = unsafe { GetKeyState(VK_CONTROL) } < 0;
                if ctrl {
                    if vk == 0x53 {
                        self.save_now(None, "Saved"); // Ctrl+S
                    }
                    return;
                }
                if vk == 0x4D {
                    self.settings.muted = !self.settings.muted;
                    self.changed();
                    return;
                }
                if let Some(k) = key_for(vk) {
                    self.press(k);
                }
            }
            Ui::KeyUp(vk) => {
                if let Some(k) = key_for(vk) {
                    self.release(k);
                }
            }
            Ui::Mouse(down, x, y) => {
                if down {
                    let layout = PAINT.with(|p| p.borrow().layout.clone());
                    for (i, &(cx, cy, r)) in layout.buttons.iter().enumerate() {
                        if (x - cx).pow(2) + (y - cy).pow(2) <= r * r {
                            let k = Key::all()[i];
                            self.mouse_key = Some(k);
                            self.press(k);
                        }
                    }
                } else if let Some(k) = self.mouse_key.take() {
                    self.release(k);
                }
            }
            Ui::DeskMouse(down, x, y) => {
                if down {
                    let hit = self.desk.as_ref().and_then(|d| d.geo.button_at(x, y));
                    if let Some(i) = hit {
                        let k = Key::all()[i];
                        self.mouse_key = Some(k);
                        self.press(k);
                    }
                } else if let Some(k) = self.mouse_key.take() {
                    self.release(k);
                }
            }
            Ui::DeskMenu(x, y) => self.desk_menu(x, y),
            Ui::DeskMoved => {
                self.save_desk_position();
                self.settings.store();
            }
            Ui::TrayClick => self.toggle_desk_hidden(),
            Ui::TrayMenu => self.tray_menu(),
            Ui::TrayLost => {
                if let Some(t) = self.tray.as_ref() {
                    t.update(&self.title);
                }
            }
            Ui::Command(id) => self.command(id),
            Ui::FocusLost => self.release_all(),
            Ui::Close => self.close(),
        }
    }

    fn press(&mut self, key: Key) {
        let i = key.index();
        if self.down[i] {
            return;
        }
        let emu = match self.emu.as_mut() {
            Some(e) => e,
            None => return,
        };
        self.down[i] = true;
        let now_wall = Instant::now();
        self.last_press = now_wall;
        let due = self.rewind_at.map_or(true, |t| (now_wall - t).as_secs_f64() >= REWIND_GAP);
        if due && !emu.sys.powered_off {
            // the state just before this press: if the press starts Bluetooth
            // or infrared, the game is put back here (see connection_blocked)
            self.rewind = Some(Snapshot::capture(emu));
            self.rewind_at = Some(now_wall);
        }
        emu.sys.live_keys[i] = true;
        emu.alarm_wake = false; // a button turns the screen on
        let now = emu.now();
        // a quick tap stays held for at least MIN_HOLD, long enough for the
        // firmware's 44 ms debounce; no longer, or fast taps in the mini
        // games (8 a second) run together into one long press
        emu.sys.hold_until[i] = now + MIN_HOLD;
        if emu.sys.powered_off {
            // a scripted press wakes it from sleep
            emu.sys.key_script.retain(|&(t, _)| t > now - 1.0);
            emu.press(now, key);
        }
        self.show_buttons();
    }

    fn release(&mut self, key: Key) {
        self.down[key.index()] = false;
        if let Some(e) = self.emu.as_mut() {
            e.sys.live_keys[key.index()] = false;
        }
        self.show_buttons();
    }

    fn release_all(&mut self) {
        for k in Key::all() {
            if self.down[k.index()] {
                self.release(k);
            }
        }
        self.mouse_key = None;
    }

    fn show_buttons(&mut self) {
        let down = self.down;
        PAINT.with(|p| p.borrow_mut().down = down);
        unsafe { InvalidateRect(self.hwnd, std::ptr::null(), 0) };
        self.render_desk();
    }

    // --- emulation ------------------------------------------------------------------

    fn draw(&mut self) {
        if let Some(emu) = self.emu.as_ref() {
            self.frames_seen = emu.sys.lcd.frames;
            // asleep, or awake for a moment with the backlight off
            let asleep = emu.sys.powered_off || emu.alarm_wake;
            let rect = PAINT.with(|p| {
                let mut p = p.borrow_mut();
                if asleep {
                    p.pixels.copy_from_slice(sleep_screen());
                } else {
                    emu.sys.lcd.argb(&mut p.pixels);
                }
                p.asleep = asleep;
                p.layout.screen
            });
            unsafe { InvalidateRect(self.hwnd, &rect, 0) };
            self.render_desk();
        }
    }

    fn tick(&mut self) {
        if self.stopped.is_some() {
            return;
        }
        let wall = Instant::now();
        if wall >= self.next_rom_scan {
            self.next_rom_scan = wall + ROM_SCAN;
            self.rescan_roms();
        }
        if self.emu.is_none() {
            self.last_wall = wall;
            return;
        }
        let real = (wall - self.last_wall).as_secs_f64();
        self.last_wall = wall;
        // time a late tick missed is made up over the next ticks; only what
        // piles up beyond MAX_BEHIND is dropped
        let mut debt = self.behind + real;
        let dropped = (debt - MAX_BEHIND).max(0.0);
        debt -= dropped;
        let step = debt.min(MAX_STEP);
        self.behind = debt - step;
        let emu = self.emu.as_mut().unwrap();
        let t_start = emu.now();
        let target = emu.executed + (step * CPU_HZ) as u64;
        // when the CPU can't keep up at all, run the device clock faster for
        // this step, so the game's clock still follows real time
        emu.sys.turbo = if step > 0.0 { ((step + dropped) / step).min(MAX_CATCHUP) } else { 1.0 };

        if emu.sys.powered_off {
            let now = emu.now();
            if emu.never_sleep && !emu.sys.key_script.iter().any(|&(t, _)| t >= now) {
                // it went to sleep anyway (e.g. another sleep path): wake it
                emu.press(now, Key::B);
            }
            if !emu.power_cycle(target) {
                // no alarm: sleep until a button
                emu.sys.game_time += (target - emu.executed) as f64 / CPU_HZ * emu.sys.turbo;
                emu.executed = target;
                emu.sys.now = target as f64 / CPU_HZ;
            }
        } else {
            match emu.run(target) {
                out @ (Outcome::Bluetooth | Outcome::Infrared) => return self.connection_blocked(out),
                Outcome::BleFail => return self.bluetooth_hang(),
                out @ (Outcome::Crash | Outcome::Stuck) => {
                    self.stopped = Some(if out == Outcome::Crash { "crash" } else { "stuck" });
                    let msg = format!("Emulator stopped ({:?}) at pc={:08X}: {:?}", out, emu.cpu.pc(), emu.fault());
                    log(&msg);
                    self.set_status(&format!("Emulator stopped ({:?}); details in tg18.log", out));
                    return;
                }
                _ => {}
            }
        }
        let t_end = self.emu.as_ref().unwrap().now();
        self.notice_call();
        self.update_wiggle();
        self.play_sound(t_start, t_end);
        let emu = self.emu.as_ref().unwrap();
        let asleep = emu.sys.powered_off;
        if asleep && !self.was_asleep {
            // the firmware saved before sleeping
            if let Some(store) = self.store.as_ref() {
                let flash = emu.sys.ram[..tg18::FLASH_SIZE].to_vec();
                let meta = save::meta(emu, true, true);
                let dest = store.flash.clone();
                self.writer.submit(move || save::write(&flash, &meta, &dest).map_err(|e| format!("{}: {}", dest, e)));
            }
        }
        if asleep != self.was_asleep {
            self.draw(); // the sleep screen, or the game again
        }
        self.was_asleep = asleep;
        if self.settings.autosave_minutes > 0 && !asleep && Instant::now() >= self.next_autosave {
            self.save_now(None, "Autosaved");
        }
        if self.emu.as_ref().unwrap().sys.lcd.frames != self.frames_seen {
            self.draw();
        }
        self.update_status(wall);
        for e in self.writer.errors() {
            log(&format!("save error: {}", e));
        }
    }

    /// Send the buzzer's sound for emulated time t0..t1 to the speakers,
    /// rendered at its true length. The delay to the speakers is kept
    /// between AUDIO_LOW and AUDIO_HIGH: when the emulator falls behind, the
    /// current sound is held a little longer instead of breaking up, and
    /// extra delay is only ever removed during silence.
    fn play_sound(&mut self, t0: f64, t1: f64) {
        let events: Vec<(f64, f64, f64)> = std::mem::take(&mut self.emu.as_mut().unwrap().sys.sound.events);
        let audio = match self.audio.as_mut() {
            Some(a) => a,
            None => {
                if let Some(last) = events.last() {
                    self.synth.freq = last.1;
                    self.synth.duty = last.2;
                }
                return;
            }
        };
        let rate = AUDIO_RATE as f64;
        let start = *self.audio_t.get_or_insert(t0);
        let n = ((t1 - start) * rate).max(0.0) as usize;
        let t_end = start + n as f64 / rate; // whole samples only, so no drift
        let mut out = Vec::with_capacity(n + 4096);
        self.synth.render(&events, start, t_end, n, &mut out);
        self.audio_t = Some(t_end);

        let queued = audio.queued() + self.pending.len();
        let (low, high) = ((AUDIO_LOW * rate) as usize, (AUDIO_HIGH * rate) as usize);
        if queued + out.len() < low {
            let fill = low - queued - out.len();
            self.synth.tone(fill, &mut out);
        } else if queued + out.len() > high && self.synth.freq == 0.0 {
            let mut silent = out.len();
            while silent > 0 && out[silent - 1] == 0 {
                silent -= 1;
            }
            let excess = (queued + out.len()).saturating_sub(low + (AUDIO_CHUNK * rate) as usize);
            let keep = silent.max(out.len().saturating_sub(excess));
            out.truncate(keep);
        }
        if self.settings.muted {
            out.iter_mut().for_each(|s| *s = 0);
        }
        self.pending.extend_from_slice(&out);
        if self.pending.len() as f64 >= AUDIO_CHUNK * rate || audio.queued() < low / 2 {
            audio.write(std::mem::take(&mut self.pending));
        }
    }

    fn update_status(&mut self, wall: Instant) {
        if let Some(until) = self.message_until {
            if Instant::now() < until {
                return;
            }
            self.message_until = None;
        }
        let (t0, e0) = self.speed_window;
        let secs = (wall - t0).as_secs_f64();
        if secs < 1.0 {
            return;
        }
        let emu = self.emu.as_ref().unwrap();
        let speed = (emu.executed.saturating_sub(e0)) as f64 / CPU_HZ / secs;
        self.speed_window = (wall, emu.executed);
        let text = if emu.sys.powered_off {
            "Screen sleeping: press any button to wake it".to_string()
        } else if speed < 0.95 {
            format!("Speed: {:.2}\u{d7} real time (slow motion; the clock stays in sync)", speed)
        } else {
            format!("Speed: {:.2}\u{d7} real time", speed)
        };
        self.set_status(&text);
    }
}

fn format_time(unix: f64) -> String {
    // local time of a file: shift by the current UTC offset
    let (y, mo, d, h, mi, s, _) = tg18::local_time();
    let local_now = tg18::days_from_civil(y, mo, d) as f64 * 86400.0 + (h * 3600 + mi * 60 + s) as f64;
    let offset = (local_now - tg18::unix_now()) / 900.0;
    let t = unix + offset.round() * 900.0;
    let days = (t / 86400.0).floor() as i64;
    let (yy, mm, dd) = tg18::civil_from_days(days);
    let secs = (t - days as f64 * 86400.0) as u32;
    const MONTHS: [&str; 12] = ["Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec"];
    format!("{:02} {} {} {:02}:{:02}", dd, MONTHS[(mm - 1) as usize], yy, secs / 3600, secs / 60 % 60)
}

/// Do something with the app (it lives in APP so the timer can reach it).
fn with_app<R>(f: impl FnOnce(&mut App) -> R) -> R {
    APP.with(|a| f(a.borrow_mut().as_mut().expect("app")))
}

/// Show a right-click menu; the app is not borrowed meanwhile, so the
/// timer keeps the emulator running. Its choice arrives as WM_COMMAND.
fn show_popup(p: Popup) {
    unsafe {
        // the menu needs a foreground window to close when clicking elsewhere
        SetForegroundWindow(p.owner);
        TrackPopupMenu(p.menu, TPM_RIGHTBUTTON, p.x, p.y, 0, p.owner, std::ptr::null());
        PostMessageW(p.owner, WM_NULL, 0, 0);
        if p.temporary {
            DestroyMenu(p.menu);
        }
    }
}

fn main() {
    // there is no console to show a crash message: keep it in tg18.log
    std::panic::set_hook(Box::new(|info| log(&format!("crash: {}", info))));
    let rom = std::env::args().nth(1);
    let app = App::new(Settings::load());
    APP.with(|a| *a.borrow_mut() = Some(app));
    with_app(|app| app.start(rom));
    loop {
        let mut msg = MSG::default();
        unsafe {
            while PeekMessageW(&mut msg, 0, 0, 0, PM_REMOVE) != 0 {
                if msg.message == WM_QUIT {
                    return;
                }
                TranslateMessage(&msg);
                DispatchMessageW(&msg);
            }
        }
        while let Some(ev) = EVENTS.with(|e| e.borrow_mut().pop_front()) {
            if with_app(|app| {
                app.handle(ev);
                app.quit
            }) {
                break;
            }
            if let Some(p) = with_app(|app| app.popup.take()) {
                show_popup(p);
            }
        }
        if with_app(|app| app.quit) {
            // let the window finish closing
            unsafe {
                while PeekMessageW(&mut msg, 0, 0, 0, PM_REMOVE) != 0 {
                    if msg.message == WM_QUIT {
                        return;
                    }
                    DispatchMessageW(&msg);
                }
            }
            return;
        }
        let next = with_app(|app| {
            app.tick_if_due();
            app.next_tick
        });
        let wait = next.saturating_duration_since(Instant::now()).as_millis() as u32;
        unsafe { MsgWaitForMultipleObjects(0, std::ptr::null(), 0, wait.max(1), QS_ALLINPUT) };
    }
}
