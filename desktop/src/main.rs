//! tg18 emulator window: see the screen, press A, B and C, hear it.
//!
//!     tg18 [flash.bin]
//!
//! With no argument it reopens the last ROM and resumes where you left off;
//! the first time it asks for the folder with your ROM dumps. A port of the
//! prototype's tg18win.py (same settings, same save folders).
//!
//! Keys: A / B / C, or Left / Down / Right; clicking the buttons works too.
//! Holding A and C together presses both. M mutes, Ctrl+S saves.
#![windows_subsystem = "windows"]

mod audio;
mod store;
mod win32;

use std::cell::RefCell;
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

const BODY: u32 = rgb(0xf3d2e0);
const BUTTON: u32 = rgb(0xfbf4f7);
const BUTTON_DOWN: u32 = rgb(0xe38aac);
const INK: u32 = rgb(0x5a2a3f);

const KEYS_HINT: &str = "Keys: A B C  or  \u{2190} \u{2193} \u{2192}   (A+C together for both)   M: mute   Ctrl+S: save";

// menu command ids
const ID_CHOOSE_FOLDER: u16 = 100;
const ID_SAVE_NOW: u16 = 101;
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
}

thread_local! {
    static EVENTS: RefCell<VecDeque<Ui>> = RefCell::new(VecDeque::new());
    static PAINT: RefCell<Paint> = RefCell::new(Paint {
        pixels: vec![0; 128 * 128],
        layout: Layout::default(),
        status: String::new(),
        hint: String::new(),
        down: [false; 3],
    });
}

fn push(ev: Ui) {
    EVENTS.with(|e| e.borrow_mut().push_back(ev));
}

unsafe extern "system" fn wndproc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    match msg {
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
        let body = CreateSolidBrush(BODY);
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
        if !p.hint.is_empty() {
            SetTextColor(mem, rgb(0xffffff));
            let of = SelectObject(mem, hint_font);
            let mut r = s;
            r.left += 20;
            r.right -= 20;
            let text: Vec<u16> = p.hint.encode_utf16().collect();
            DrawTextW(mem, text.as_ptr(), text.len() as i32, &mut r, DT_CENTER | DT_VCENTER | DT_SINGLELINE);
            SelectObject(mem, of);
        }
        let pen = CreatePen(PS_SOLID, ((2.0 * l.dpi).round() as i32).max(1), INK);
        let old_pen = SelectObject(mem, pen);
        SetTextColor(mem, INK);
        let of = SelectObject(mem, bold);
        for (i, &(cx, cy, r)) in l.buttons.iter().enumerate() {
            let brush = CreateSolidBrush(if p.down[i] { BUTTON_DOWN } else { BUTTON });
            let ob = SelectObject(mem, brush);
            Ellipse(mem, cx - r, cy - r, cx + r, cy + r);
            SelectObject(mem, ob);
            DeleteObject(brush);
            let label: Vec<u16> = ["A", "B", "C"][i].encode_utf16().collect();
            let mut rr = RECT { left: cx - r, top: cy - r, right: cx + r, bottom: cy + r };
            DrawTextW(mem, label.as_ptr(), 1, &mut rr, DT_CENTER | DT_VCENTER | DT_SINGLELINE);
        }
        SelectObject(mem, font);
        for (text, rect) in [(&p.status[..], l.status), (KEYS_HINT, l.keys)] {
            let t: Vec<u16> = text.encode_utf16().collect();
            let mut r = rect;
            DrawTextW(mem, t.as_ptr(), t.len() as i32, &mut r, DT_CENTER | DT_VCENTER | DT_SINGLELINE);
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
        PAINT.with(|p| p.borrow_mut().layout = layout.clone());
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
                hIcon: LoadIconW(0, IDI_APPLICATION),
                hCursor: LoadCursorW(0, IDC_ARROW),
                hbrBackground: 0,
                lpszMenuName: std::ptr::null(),
                lpszClassName: class.as_ptr(),
                hIconSm: 0,
            };
            RegisterClassExW(&wc);
            let (w, h) = Self::outer_size(&layout);
            CreateWindowExW(0, class.as_ptr(), wide("tg18 emulator").as_ptr(), WINDOW_STYLE,
                            CW_USEDEFAULT, CW_USEDEFAULT, w, h, 0, 0, inst, std::ptr::null())
        };
        let menus = Self::build_menus(hwnd);
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
            quit: false,
        };
        app.update_menu_checks();
        app.fill_rom_menu();
        app.fill_slot_menus();
        unsafe {
            ShowWindow(hwnd, SW_SHOW);
            SetForegroundWindow(hwnd);
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
            add(file, ID_CHOOSE_FOLDER, "Choose ROM folder\u{2026}");
            sep(file);
            add(file, ID_SAVE_NOW, "Save now\tCtrl+S");
            sub(file, save_slot, "Save to slot");
            sub(file, load_slot, "Load slot");
            add(file, ID_OPEN_SAVES, "Open saves folder");
            sep(file);
            add(file, ID_QUIT, "Quit");
            sub(bar, file, "File");

            let settings = CreatePopupMenu();
            let vol = CreatePopupMenu();
            for (i, v) in VOLUMES.iter().enumerate() {
                add(vol, ID_VOLUME + i as u16, &format!("{}%", v));
            }
            sub(settings, vol, "Volume");
            add(settings, ID_MUTE, "Mute\tM");
            let size = CreatePopupMenu();
            for (i, v) in SCALES.iter().enumerate() {
                add(size, ID_SCALE + i as u16, &format!("{}\u{d7} ({} px)", v, v * 128));
            }
            sub(settings, size, "Screen size");
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
            add(settings, ID_NEVER_SLEEP, "Never sleep (keep the screen on)");
            add(settings, ID_PAUSE_TIME, "Pause time while closed");
            sub(bar, settings, "Settings");
            SetMenu(hwnd, bar);
            Menus { rom, save_slot, load_slot, settings, bar }
        }
    }

    fn update_menu_checks(&self) {
        let s = &self.settings;
        let check = |id: u16, on: bool| unsafe {
            CheckMenuItem(self.menus.bar, id as u32, MF_BYCOMMAND | if on { MF_CHECKED } else { MF_UNCHECKED });
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
        self.roms = store::list_roms(self.settings.rom_dir.as_deref());
        unsafe {
            if self.roms.is_empty() {
                AppendMenuW(m, MF_STRING | MF_GRAYED, 0, wide("(no ROMs found: choose the ROM folder)").as_ptr());
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
                if let Some(e) = self.emu.as_mut() {
                    e.never_sleep = self.settings.never_sleep;
                }
                self.changed();
            }
            ID_PAUSE_TIME => {
                self.settings.pause_time_when_closed = !self.settings.pause_time_when_closed;
                self.changed();
            }
            _ if (ID_VOLUME..ID_VOLUME + 5).contains(&id) => {
                self.settings.volume = VOLUMES[(id - ID_VOLUME) as usize];
                self.synth.set_volume(self.settings.volume as f64 / 100.0);
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

    fn show_hint(&self, text: &str) {
        PAINT.with(|p| p.borrow_mut().hint = text.to_string());
        unsafe { InvalidateRect(self.hwnd, std::ptr::null(), 0) };
    }

    fn set_title(&self, text: &str) {
        unsafe { SetWindowTextW(self.hwnd, wide(text).as_ptr()) };
    }

    // --- ROMs ------------------------------------------------------------------

    fn start(&mut self, rom: Option<String>) {
        if let Some(r) = rom {
            let p = std::fs::canonicalize(&r).unwrap_or_else(|_| PathBuf::from(&r));
            self.open_rom(&p);
            return;
        }
        if let (Some(dir), Some(last)) = (self.settings.rom_dir.clone(), self.settings.last_rom.clone()) {
            let p = Path::new(&dir).join(last);
            if store::is_rom(&p) {
                self.open_rom(&p);
                return;
            }
        }
        self.show_hint("Choose the folder with your ROM dumps (File > Choose ROM folder)");
        if self.settings.rom_dir.is_none() {
            self.choose_folder();
        }
    }

    fn choose_folder(&mut self) {
        let folder = match pick_folder(self.hwnd, "Folder with your tg18 ROM dumps") {
            Some(f) => f,
            None => return,
        };
        let roms = store::list_roms(Some(&folder));
        if roms.is_empty() {
            message_box(self.hwnd, "No ROMs found",
                        &format!("No tg18 flash dumps (8 MiB .bin files starting with SPII) were found in\n{}", folder),
                        MB_OK | MB_ICONWARNING);
            return;
        }
        self.settings.rom_dir = Some(folder);
        self.settings.store();
        self.fill_rom_menu();
        if self.emu.is_none() {
            if roms.len() == 1 {
                self.switch_rom(&roms[0].0);
            } else {
                self.show_hint("Pick a ROM: File > Open ROM");
            }
        }
    }

    fn switch_rom(&mut self, filename: &str) {
        let dir = self.settings.rom_dir.clone().unwrap_or_default();
        let path = Path::new(&dir).join(filename);
        if self.rom_path.as_deref() == Some(path.as_path()) {
            return;
        }
        self.open_rom(&path);
    }

    fn open_rom(&mut self, path: &Path) {
        if !store::is_rom(path) {
            message_box(self.hwnd, "Not a tg18 ROM", &format!("{} is not an 8 MiB tg18 flash dump.", path.display()),
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
                message_box(self.hwnd, "Can't open the ROM", &e.to_string(), MB_OK | MB_ICONERROR);
                return;
            }
        };
        let fname = path.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
        self.say(&format!("Loading {}\u{2026}", store::display_name(&fname)), 10.0);
        let store = SaveStore::new(path);
        let legacy = store.import_legacy(&image);
        let (emu, elapsed, how) = match self.boot(&store, &image) {
            Ok(b) => b,
            Err(e) => {
                message_box(self.hwnd, "Save does not match", &format!("The save for this ROM can't be used: {}", e),
                            MB_OK | MB_ICONERROR);
                return;
            }
        };
        self.image = Some(image);
        self.store = Some(store);
        self.rom_path = Some(path.to_path_buf());
        self.set_emu(emu);
        let s = &mut self.settings;
        if s.rom_dir.is_none() {
            s.rom_dir = path.parent().map(|p| p.to_string_lossy().to_string());
        }
        if path.parent().map(|p| p.to_string_lossy().to_string()) == s.rom_dir
            || path.parent().and_then(|p| std::fs::canonicalize(p).ok())
                == s.rom_dir.as_ref().and_then(|d| std::fs::canonicalize(d).ok())
        {
            s.last_rom = Some(fname.clone());
        }
        self.settings.store();
        self.fill_rom_menu();
        self.fill_slot_menus();
        self.set_title(&format!("{} \u{2013} tg18 emulator", store::display_name(&fname)));
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
        emu.never_sleep = self.settings.never_sleep;
        emu.stop_on_bluetooth = true;
        self.emu = Some(emu);
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
        if self.force_sleep(10.0) {
            let emu = self.emu.as_mut().unwrap();
            emu.sys.rtc.base_ticks += (elapsed * 32768.0) as i64;
            let now = emu.now();
            emu.press(now, Key::B);
            let limit = emu.executed + CPU_HZ as u64;
            emu.power_cycle(limit);
            let emu = self.emu.take().unwrap();
            self.set_emu(emu);
        } else if let Some(emu) = self.emu.as_mut() {
            emu.sys.rtc.base_ticks += (elapsed * 32768.0) as i64;
        }
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
        message_box(self.hwnd, "Not supported",
                    &format!("{}\n\nThe game has been put back to just before you chose it.", text),
                    MB_OK | MB_ICONINFORMATION);
        self.reset_timing();
    }

    /// The game froze waiting for the Bluetooth chip: restart the toy.
    fn bluetooth_hang(&mut self) {
        message_box(self.hwnd, "Bluetooth is not emulated",
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
        if message_box(self.hwnd, &format!("Load slot {}", n), &q, MB_YESNO | MB_ICONQUESTION) != IDYES {
            return;
        }
        self.save_now(None, "Autosaved");
        self.writer.wait();
        let image = self.image.clone().unwrap();
        let mut emu = Machine::new(image.clone(), &image, 0.0);
        if let Err(e) = Snapshot::load(&path).and_then(|s| s.restore(&mut emu)) {
            message_box(self.hwnd, "Save does not match", &format!("Slot {} can't be loaded: {}", n, e), MB_OK | MB_ICONERROR);
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
        let due = self.rewind_at.map_or(true, |t| (now_wall - t).as_secs_f64() >= REWIND_GAP);
        if due && !emu.sys.powered_off {
            // the state just before this press: if the press starts Bluetooth
            // or infrared, the game is put back here (see connection_blocked)
            self.rewind = Some(Snapshot::capture(emu));
            self.rewind_at = Some(now_wall);
        }
        emu.sys.live_keys[i] = true;
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

    fn show_buttons(&self) {
        let down = self.down;
        PAINT.with(|p| p.borrow_mut().down = down);
        unsafe { InvalidateRect(self.hwnd, std::ptr::null(), 0) };
    }

    // --- emulation ------------------------------------------------------------------

    fn draw(&mut self) {
        if let Some(emu) = self.emu.as_ref() {
            self.frames_seen = emu.sys.lcd.frames;
            let rect = PAINT.with(|p| {
                let mut p = p.borrow_mut();
                emu.sys.lcd.argb(&mut p.pixels);
                p.layout.screen
            });
            unsafe { InvalidateRect(self.hwnd, &rect, 0) };
        }
    }

    fn tick(&mut self) {
        if self.stopped.is_some() {
            return;
        }
        let wall = Instant::now();
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
            "Sleeping: press any button to wake it".to_string()
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

fn main() {
    let rom = std::env::args().nth(1);
    let mut app = App::new(Settings::load());
    app.start(rom);
    let mut next = Instant::now();
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
            app.handle(ev);
            if app.quit {
                break;
            }
        }
        if app.quit {
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
        let now = Instant::now();
        if now >= next {
            app.tick();
            next = now + TICK;
        }
        let wait = next.saturating_duration_since(Instant::now()).as_millis() as u32;
        unsafe { MsgWaitForMultipleObjects(0, std::ptr::null(), 0, wait.max(1), QS_ALLINPUT) };
    }
}
