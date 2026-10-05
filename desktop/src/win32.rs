//! The few Windows functions the app uses, declared by hand (like the
//! prototype's ctypes calls), so no extra packages are needed.
#![allow(non_snake_case, non_camel_case_types, clippy::upper_case_acronyms, dead_code)]

use std::ffi::c_void;

pub type HWND = isize;
pub type HMENU = isize;
pub type HDC = isize;
pub type HINSTANCE = isize;
pub type HGDIOBJ = isize;
pub type WPARAM = usize;
pub type LPARAM = isize;
pub type LRESULT = isize;
pub type WNDPROC = unsafe extern "system" fn(HWND, u32, WPARAM, LPARAM) -> LRESULT;

#[repr(C)]
pub struct WNDCLASSEXW {
    pub cbSize: u32,
    pub style: u32,
    pub lpfnWndProc: WNDPROC,
    pub cbClsExtra: i32,
    pub cbWndExtra: i32,
    pub hInstance: HINSTANCE,
    pub hIcon: isize,
    pub hCursor: isize,
    pub hbrBackground: isize,
    pub lpszMenuName: *const u16,
    pub lpszClassName: *const u16,
    pub hIconSm: isize,
}

#[repr(C)]
#[derive(Default, Clone, Copy)]
pub struct POINT {
    pub x: i32,
    pub y: i32,
}

#[repr(C)]
#[derive(Default, Clone, Copy)]
pub struct RECT {
    pub left: i32,
    pub top: i32,
    pub right: i32,
    pub bottom: i32,
}

#[repr(C)]
#[derive(Default)]
pub struct MSG {
    pub hwnd: HWND,
    pub message: u32,
    pub wParam: WPARAM,
    pub lParam: LPARAM,
    pub time: u32,
    pub pt: POINT,
    pub lPrivate: u32,
}

#[repr(C)]
pub struct PAINTSTRUCT {
    pub hdc: HDC,
    pub fErase: i32,
    pub rcPaint: RECT,
    pub fRestore: i32,
    pub fIncUpdate: i32,
    pub rgbReserved: [u8; 32],
}

#[repr(C)]
#[derive(Default)]
pub struct BITMAPINFOHEADER {
    pub biSize: u32,
    pub biWidth: i32,
    pub biHeight: i32,
    pub biPlanes: u16,
    pub biBitCount: u16,
    pub biCompression: u32,
    pub biSizeImage: u32,
    pub biXPelsPerMeter: i32,
    pub biYPelsPerMeter: i32,
    pub biClrUsed: u32,
    pub biClrImportant: u32,
}

#[repr(C)]
#[derive(Default)]
pub struct BITMAPINFO {
    pub bmiHeader: BITMAPINFOHEADER,
    pub bmiColors: [u32; 1],
}

#[repr(C)]
pub struct BROWSEINFOW {
    pub hwndOwner: HWND,
    pub pidlRoot: *const c_void,
    pub pszDisplayName: *mut u16,
    pub lpszTitle: *const u16,
    pub ulFlags: u32,
    pub lpfn: *const c_void,
    pub lParam: LPARAM,
    pub iImage: i32,
}

#[repr(C)]
pub struct OPENFILENAMEW {
    pub lStructSize: u32,
    pub hwndOwner: HWND,
    pub hInstance: HINSTANCE,
    pub lpstrFilter: *const u16,
    pub lpstrCustomFilter: *mut u16,
    pub nMaxCustFilter: u32,
    pub nFilterIndex: u32,
    pub lpstrFile: *mut u16,
    pub nMaxFile: u32,
    pub lpstrFileTitle: *mut u16,
    pub nMaxFileTitle: u32,
    pub lpstrInitialDir: *const u16,
    pub lpstrTitle: *const u16,
    pub Flags: u32,
    pub nFileOffset: u16,
    pub nFileExtension: u16,
    pub lpstrDefExt: *const u16,
    pub lCustData: LPARAM,
    pub lpfnHook: *const c_void,
    pub lpTemplateName: *const u16,
    pub pvReserved: *mut c_void,
    pub dwReserved: u32,
    pub FlagsEx: u32,
}

#[repr(C)]
pub struct WAVEFORMATEX {
    pub wFormatTag: u16,
    pub nChannels: u16,
    pub nSamplesPerSec: u32,
    pub nAvgBytesPerSec: u32,
    pub nBlockAlign: u16,
    pub wBitsPerSample: u16,
    pub cbSize: u16,
}

#[repr(C)]
pub struct WAVEHDR {
    pub lpData: *mut u8,
    pub dwBufferLength: u32,
    pub dwBytesRecorded: u32,
    pub dwUser: usize,
    pub dwFlags: u32,
    pub dwLoops: u32,
    pub lpNext: *mut c_void,
    pub reserved: usize,
}

#[repr(C)]
#[derive(Default, Clone, Copy)]
pub struct SIZE {
    pub cx: i32,
    pub cy: i32,
}

#[repr(C)]
pub struct BLENDFUNCTION {
    pub BlendOp: u8,
    pub BlendFlags: u8,
    pub SourceConstantAlpha: u8,
    pub AlphaFormat: u8,
}

#[repr(C)]
pub struct ICONINFO {
    pub fIcon: i32,
    pub xHotspot: u32,
    pub yHotspot: u32,
    pub hbmMask: isize,
    pub hbmColor: isize,
}

#[repr(C)]
pub struct NOTIFYICONDATAW {
    pub cbSize: u32,
    pub hWnd: HWND,
    pub uID: u32,
    pub uFlags: u32,
    pub uCallbackMessage: u32,
    pub hIcon: isize,
    pub szTip: [u16; 128],
    pub dwState: u32,
    pub dwStateMask: u32,
    pub szInfo: [u16; 256],
    pub uVersion: u32,
    pub szInfoTitle: [u16; 64],
    pub dwInfoFlags: u32,
    pub guidItem: [u32; 4],
    pub hBalloonIcon: isize,
}

impl Default for NOTIFYICONDATAW {
    fn default() -> Self {
        // all zero is a valid empty record
        unsafe { std::mem::zeroed() }
    }
}

pub const NIM_ADD: u32 = 0;
pub const NIM_MODIFY: u32 = 1;
pub const NIM_DELETE: u32 = 2;
pub const NIF_MESSAGE: u32 = 1;
pub const NIF_ICON: u32 = 2;
pub const NIF_TIP: u32 = 4;
pub const WM_APP: u32 = 0x8000;
pub const WM_RBUTTONUP: u32 = 0x0205;
pub const WM_ENTERSIZEMOVE: u32 = 0x0231;
pub const WS_EX_TOOLWINDOW: u32 = 0x0000_0080;

pub const SM_CXICON: i32 = 11;
pub const SM_CXSMICON: i32 = 49;
pub const WM_SETICON: u32 = 0x0080;
pub const ICON_SMALL: usize = 0;
pub const ICON_BIG: usize = 1;

pub const AC_SRC_ALPHA: u8 = 1;
pub const ULW_ALPHA: u32 = 2;
pub const WS_POPUP: u32 = 0x8000_0000;
pub const WS_EX_LAYERED: u32 = 0x0008_0000;
pub const WS_EX_TOPMOST: u32 = 0x0000_0008;
pub const HWND_TOPMOST: HWND = -1;
pub const HWND_NOTOPMOST: HWND = -2;
pub const SWP_NOSIZE: u32 = 0x0001;
pub const SWP_NOMOVE: u32 = 0x0002;
pub const SWP_NOZORDER: u32 = 0x0004;
pub const SWP_NOACTIVATE: u32 = 0x0010;
pub const SW_HIDE: i32 = 0;
pub const WM_NULL: u32 = 0x0000;
pub const WM_TIMER: u32 = 0x0113;
pub const WM_NCHITTEST: u32 = 0x0084;
pub const WM_NCLBUTTONDBLCLK: u32 = 0x00A3;
pub const WM_NCRBUTTONUP: u32 = 0x00A5;
pub const WM_CONTEXTMENU: u32 = 0x007B;
pub const WM_EXITSIZEMOVE: u32 = 0x0232;
pub const HTCLIENT: LRESULT = 1;
pub const HTCAPTION: LRESULT = 2;
pub const TPM_RIGHTBUTTON: u32 = 0x0002;
pub const SPI_GETWORKAREA: u32 = 0x0030;
pub const MONITOR_DEFAULTTONULL: u32 = 0;
pub const MONITOR_DEFAULTTONEAREST: u32 = 2;

#[repr(C)]
#[derive(Default)]
pub struct MONITORINFO {
    pub cbSize: u32,
    pub rcMonitor: RECT,
    pub rcWork: RECT,
    pub dwFlags: u32,
}

pub const WS_OVERLAPPED: u32 = 0;
pub const WS_CAPTION: u32 = 0x00C0_0000;
pub const WS_SYSMENU: u32 = 0x0008_0000;
pub const WS_MINIMIZEBOX: u32 = 0x0002_0000;
pub const WS_VISIBLE: u32 = 0x1000_0000;
pub const CW_USEDEFAULT: i32 = 0x8000_0000u32 as i32;
pub const SW_SHOW: i32 = 5;
pub const SW_SHOWNORMAL: i32 = 1;
pub const CS_HREDRAW: u32 = 2;
pub const CS_VREDRAW: u32 = 1;
pub const IDC_ARROW: usize = 32512;
pub const IDI_APPLICATION: usize = 32512;

pub const WM_DESTROY: u32 = 0x0002;
pub const WM_PAINT: u32 = 0x000F;
pub const WM_CLOSE: u32 = 0x0010;
pub const WM_QUIT: u32 = 0x0012;
pub const WM_ERASEBKGND: u32 = 0x0014;
pub const WM_ACTIVATE: u32 = 0x0006;
pub const WM_KILLFOCUS: u32 = 0x0008;
pub const WM_KEYDOWN: u32 = 0x0100;
pub const WM_KEYUP: u32 = 0x0101;
pub const WM_SYSKEYDOWN: u32 = 0x0104;
pub const WM_COMMAND: u32 = 0x0111;
pub const WM_INITMENUPOPUP: u32 = 0x0117;
pub const WM_LBUTTONDOWN: u32 = 0x0201;
pub const WM_LBUTTONUP: u32 = 0x0202;
pub const WM_ENTERMENULOOP: u32 = 0x0211;
pub const WM_EXITMENULOOP: u32 = 0x0212;
pub const PM_REMOVE: u32 = 1;
pub const QS_ALLINPUT: u32 = 0x04FF;

pub const MF_STRING: u32 = 0;
pub const MF_POPUP: u32 = 0x10;
pub const MF_SEPARATOR: u32 = 0x800;
pub const MF_CHECKED: u32 = 8;
pub const MF_UNCHECKED: u32 = 0;
pub const MF_GRAYED: u32 = 1;
pub const MF_ENABLED: u32 = 0;
pub const MF_BYCOMMAND: u32 = 0;
pub const MF_BYPOSITION: u32 = 0x400;

pub const MB_OK: u32 = 0;
pub const MB_YESNO: u32 = 4;
pub const MB_ICONINFORMATION: u32 = 0x40;
pub const MB_ICONWARNING: u32 = 0x30;
pub const MB_ICONERROR: u32 = 0x10;
pub const MB_ICONQUESTION: u32 = 0x20;
pub const IDYES: i32 = 6;

pub const VK_CONTROL: i32 = 0x11;
pub const VK_LEFT: usize = 0x25;
pub const VK_DOWN: usize = 0x28;
pub const VK_RIGHT: usize = 0x27;

pub const DT_CENTER: u32 = 1;
pub const DT_VCENTER: u32 = 4;
pub const DT_SINGLELINE: u32 = 0x20;
pub const DT_WORDBREAK: u32 = 0x10;
pub const DT_CALCRECT: u32 = 0x400;
/// Long words (paths) are broken too instead of running off the edge.
pub const DT_EDITCONTROL: u32 = 0x2000;
pub const TRANSPARENT: i32 = 1;
pub const COLORONCOLOR: i32 = 3;
pub const SRCCOPY: u32 = 0x00CC_0020;
pub const DIB_RGB_COLORS: u32 = 0;
pub const PS_SOLID: i32 = 0;
pub const LOGPIXELSX: i32 = 88;

pub const BIF_RETURNONLYFSDIRS: u32 = 1;
pub const BIF_NEWDIALOGSTYLE: u32 = 0x40;
pub const COINIT_APARTMENTTHREADED: u32 = 2;

pub const WAVE_MAPPER: u32 = 0xFFFF_FFFF;
pub const WHDR_DONE: u32 = 1;

#[link(name = "user32")]
extern "system" {
    pub fn RegisterClassExW(wc: *const WNDCLASSEXW) -> u16;
    pub fn CreateWindowExW(
        ex: u32,
        class: *const u16,
        title: *const u16,
        style: u32,
        x: i32,
        y: i32,
        w: i32,
        h: i32,
        parent: HWND,
        menu: HMENU,
        inst: HINSTANCE,
        param: *const c_void,
    ) -> HWND;
    pub fn DefWindowProcW(h: HWND, m: u32, w: WPARAM, l: LPARAM) -> LRESULT;
    pub fn DestroyWindow(h: HWND) -> i32;
    pub fn ShowWindow(h: HWND, cmd: i32) -> i32;
    pub fn PeekMessageW(msg: *mut MSG, h: HWND, min: u32, max: u32, remove: u32) -> i32;
    pub fn TranslateMessage(msg: *const MSG) -> i32;
    pub fn DispatchMessageW(msg: *const MSG) -> LRESULT;
    pub fn PostQuitMessage(code: i32);
    pub fn MsgWaitForMultipleObjects(n: u32, handles: *const isize, all: i32, ms: u32, mask: u32) -> u32;
    pub fn CreateMenu() -> HMENU;
    pub fn CreatePopupMenu() -> HMENU;
    pub fn AppendMenuW(m: HMENU, flags: u32, id: usize, text: *const u16) -> i32;
    pub fn SetMenu(h: HWND, m: HMENU) -> i32;
    pub fn DestroyMenu(m: HMENU) -> i32;
    pub fn DeleteMenu(m: HMENU, pos: u32, flags: u32) -> i32;
    pub fn GetMenuItemCount(m: HMENU) -> i32;
    pub fn CheckMenuItem(m: HMENU, id: u32, flags: u32) -> u32;
    pub fn EnableMenuItem(m: HMENU, id: u32, flags: u32) -> i32;
    pub fn DrawMenuBar(h: HWND) -> i32;
    pub fn InvalidateRect(h: HWND, r: *const RECT, erase: i32) -> i32;
    pub fn UpdateWindow(h: HWND) -> i32;
    pub fn BeginPaint(h: HWND, ps: *mut PAINTSTRUCT) -> HDC;
    pub fn EndPaint(h: HWND, ps: *const PAINTSTRUCT) -> i32;
    pub fn GetDC(h: HWND) -> HDC;
    pub fn ReleaseDC(h: HWND, dc: HDC) -> i32;
    pub fn FillRect(dc: HDC, r: *const RECT, brush: isize) -> i32;
    pub fn DrawTextW(dc: HDC, text: *const u16, n: i32, r: *mut RECT, fmt: u32) -> i32;
    pub fn SetWindowPos(h: HWND, after: HWND, x: i32, y: i32, w: i32, hgt: i32, flags: u32) -> i32;
    pub fn AdjustWindowRectEx(r: *mut RECT, style: u32, menu: i32, ex: u32) -> i32;
    pub fn SetWindowTextW(h: HWND, text: *const u16) -> i32;
    pub fn MessageBoxW(h: HWND, text: *const u16, caption: *const u16, t: u32) -> i32;
    pub fn LoadCursorW(inst: HINSTANCE, name: usize) -> isize;
    pub fn LoadIconW(inst: HINSTANCE, name: usize) -> isize;
    pub fn GetKeyState(vk: i32) -> i16;
    pub fn SetCapture(h: HWND) -> HWND;
    pub fn ReleaseCapture() -> i32;
    pub fn SetProcessDPIAware() -> i32;
    pub fn SetForegroundWindow(h: HWND) -> i32;
    pub fn UpdateLayeredWindow(
        h: HWND,
        dst_dc: HDC,
        dst_pos: *const POINT,
        size: *const SIZE,
        src_dc: HDC,
        src_pos: *const POINT,
        key: u32,
        blend: *const BLENDFUNCTION,
        flags: u32,
    ) -> i32;
    pub fn GetWindowRect(h: HWND, r: *mut RECT) -> i32;
    pub fn SystemParametersInfoW(action: u32, param: u32, pv: *mut c_void, ini: u32) -> i32;
    pub fn MonitorFromPoint(pt: POINT, flags: u32) -> isize;
    pub fn GetMonitorInfoW(mon: isize, info: *mut MONITORINFO) -> i32;
    pub fn CreateIconIndirect(info: *const ICONINFO) -> isize;
    pub fn DestroyIcon(icon: isize) -> i32;
    pub fn GetSystemMetrics(index: i32) -> i32;
    pub fn SendMessageW(h: HWND, m: u32, w: WPARAM, l: LPARAM) -> LRESULT;
    pub fn RegisterWindowMessageW(name: *const u16) -> u32;
    pub fn GetCursorPos(p: *mut POINT) -> i32;
    pub fn IsWindowVisible(h: HWND) -> i32;
    pub fn SetTimer(h: HWND, id: usize, ms: u32, f: *const c_void) -> usize;
    pub fn TrackPopupMenu(m: HMENU, flags: u32, x: i32, y: i32, reserved: i32, h: HWND, r: *const RECT) -> i32;
    pub fn PostMessageW(h: HWND, m: u32, w: WPARAM, l: LPARAM) -> i32;
}

#[link(name = "gdi32")]
extern "system" {
    pub fn StretchDIBits(
        dc: HDC,
        xd: i32,
        yd: i32,
        wd: i32,
        hd: i32,
        xs: i32,
        ys: i32,
        ws: i32,
        hs: i32,
        bits: *const c_void,
        info: *const BITMAPINFO,
        usage: u32,
        rop: u32,
    ) -> i32;
    pub fn SetStretchBltMode(dc: HDC, mode: i32) -> i32;
    pub fn CreateSolidBrush(color: u32) -> isize;
    pub fn CreatePen(style: i32, width: i32, color: u32) -> isize;
    pub fn SelectObject(dc: HDC, obj: HGDIOBJ) -> HGDIOBJ;
    pub fn DeleteObject(obj: HGDIOBJ) -> i32;
    pub fn Ellipse(dc: HDC, l: i32, t: i32, r: i32, b: i32) -> i32;
    pub fn SetBkMode(dc: HDC, mode: i32) -> i32;
    pub fn SetTextColor(dc: HDC, color: u32) -> u32;
    pub fn CreateFontW(
        h: i32,
        w: i32,
        esc: i32,
        orient: i32,
        weight: i32,
        italic: u32,
        underline: u32,
        strike: u32,
        charset: u32,
        out_prec: u32,
        clip_prec: u32,
        quality: u32,
        pitch: u32,
        face: *const u16,
    ) -> isize;
    pub fn CreateCompatibleDC(dc: HDC) -> HDC;
    pub fn CreateCompatibleBitmap(dc: HDC, w: i32, h: i32) -> isize;
    pub fn BitBlt(dc: HDC, x: i32, y: i32, w: i32, h: i32, src: HDC, xs: i32, ys: i32, rop: u32) -> i32;
    pub fn DeleteDC(dc: HDC) -> i32;
    pub fn GetDeviceCaps(dc: HDC, index: i32) -> i32;
    pub fn CreateBitmap(w: i32, h: i32, planes: u32, bits_per_pixel: u32, bits: *const c_void) -> isize;
    pub fn CreateDIBSection(dc: HDC, info: *const BITMAPINFO, usage: u32, bits: *mut *mut c_void, section: isize,
                            offset: u32) -> isize;
}

#[link(name = "kernel32")]
extern "system" {
    pub fn GetModuleHandleW(name: *const u16) -> HINSTANCE;
}

#[link(name = "shell32")]
extern "system" {
    pub fn SHBrowseForFolderW(bi: *const BROWSEINFOW) -> *mut c_void;
    pub fn SHGetPathFromIDListW(pidl: *const c_void, path: *mut u16) -> i32;
    pub fn Shell_NotifyIconW(what: u32, data: *const NOTIFYICONDATAW) -> i32;
    pub fn ShellExecuteW(h: HWND, op: *const u16, file: *const u16, params: *const u16, dir: *const u16, show: i32) -> isize;
}

#[link(name = "comdlg32")]
extern "system" {
    pub fn GetOpenFileNameW(ofn: *mut OPENFILENAMEW) -> i32;
}

#[link(name = "ole32")]
extern "system" {
    pub fn CoInitializeEx(reserved: *const c_void, flags: u32) -> i32;
    pub fn CoTaskMemFree(p: *mut c_void);
}

#[link(name = "winmm")]
extern "system" {
    pub fn waveOutOpen(h: *mut isize, dev: u32, fmt: *const WAVEFORMATEX, cb: usize, inst: usize, flags: u32) -> u32;
    pub fn waveOutPrepareHeader(h: isize, hdr: *mut WAVEHDR, size: u32) -> u32;
    pub fn waveOutUnprepareHeader(h: isize, hdr: *mut WAVEHDR, size: u32) -> u32;
    pub fn waveOutWrite(h: isize, hdr: *mut WAVEHDR, size: u32) -> u32;
    pub fn waveOutReset(h: isize) -> u32;
    pub fn waveOutClose(h: isize) -> u32;
    pub fn timeBeginPeriod(ms: u32) -> u32;
    pub fn timeEndPeriod(ms: u32) -> u32;
}

/// A Rust string as a NUL-terminated UTF-16 buffer.
pub fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

pub fn from_wide(buf: &[u16]) -> String {
    let n = buf.iter().position(|&c| c == 0).unwrap_or(buf.len());
    String::from_utf16_lossy(&buf[..n])
}

/// COLORREF from #rrggbb.
pub const fn rgb(hex: u32) -> u32 {
    ((hex & 0xFF) << 16) | (hex & 0xFF00) | ((hex >> 16) & 0xFF)
}

/// DrawTextW with a NUL-terminated copy of the text. (An empty Rust string
/// has no real buffer behind it, and Windows reads the pointer even when
/// told the length is 0: that crashed the app when no ROM was loaded.)
pub fn draw_text(dc: HDC, text: &str, r: &mut RECT, fmt: u32) -> i32 {
    unsafe { DrawTextW(dc, wide(text).as_ptr(), -1, r, fmt) }
}

pub fn message_box(owner: HWND, title: &str, text: &str, flags: u32) -> i32 {
    unsafe { MessageBoxW(owner, wide(text).as_ptr(), wide(title).as_ptr(), flags) }
}

/// Windows' folder picker; None if cancelled.
pub fn pick_folder(owner: HWND, title: &str) -> Option<String> {
    let mut name = [0u16; 260];
    let title = wide(title);
    let bi = BROWSEINFOW {
        hwndOwner: owner,
        pidlRoot: std::ptr::null(),
        pszDisplayName: name.as_mut_ptr(),
        lpszTitle: title.as_ptr(),
        ulFlags: BIF_RETURNONLYFSDIRS | BIF_NEWDIALOGSTYLE,
        lpfn: std::ptr::null(),
        lParam: 0,
        iImage: 0,
    };
    unsafe {
        let pidl = SHBrowseForFolderW(&bi);
        if pidl.is_null() {
            return None;
        }
        let mut path = [0u16; 1024];
        let ok = SHGetPathFromIDListW(pidl, path.as_mut_ptr());
        CoTaskMemFree(pidl);
        if ok != 0 { Some(from_wide(&path)) } else { None }
    }
}

/// Windows' Open dialog for one existing file; None if cancelled. `filter`
/// is pairs of (description, patterns like "*.a;*.b").
pub fn pick_file(owner: HWND, title: &str, start_dir: &str, filter: &[(&str, &str)]) -> Option<String> {
    const OFN_HIDEREADONLY: u32 = 0x4;
    const OFN_NOCHANGEDIR: u32 = 0x8;
    const OFN_PATHMUSTEXIST: u32 = 0x800;
    const OFN_FILEMUSTEXIST: u32 = 0x1000;
    // "description\0patterns\0...\0\0"
    let mut filt: Vec<u16> = Vec::new();
    for (desc, pats) in filter {
        filt.extend(desc.encode_utf16().chain([0]));
        filt.extend(pats.encode_utf16().chain([0]));
    }
    filt.push(0);
    let mut file = [0u16; 1024];
    let title = wide(title);
    let dir = wide(start_dir);
    let mut ofn = OPENFILENAMEW {
        lStructSize: std::mem::size_of::<OPENFILENAMEW>() as u32,
        hwndOwner: owner,
        hInstance: 0,
        lpstrFilter: filt.as_ptr(),
        lpstrCustomFilter: std::ptr::null_mut(),
        nMaxCustFilter: 0,
        nFilterIndex: 1,
        lpstrFile: file.as_mut_ptr(),
        nMaxFile: file.len() as u32,
        lpstrFileTitle: std::ptr::null_mut(),
        nMaxFileTitle: 0,
        lpstrInitialDir: dir.as_ptr(),
        lpstrTitle: title.as_ptr(),
        Flags: OFN_HIDEREADONLY | OFN_NOCHANGEDIR | OFN_PATHMUSTEXIST | OFN_FILEMUSTEXIST,
        nFileOffset: 0,
        nFileExtension: 0,
        lpstrDefExt: std::ptr::null(),
        lCustData: 0,
        lpfnHook: std::ptr::null(),
        lpTemplateName: std::ptr::null(),
        pvReserved: std::ptr::null_mut(),
        dwReserved: 0,
        FlagsEx: 0,
    };
    if unsafe { GetOpenFileNameW(&mut ofn) } != 0 { Some(from_wide(&file)) } else { None }
}

pub fn open_in_explorer(path: &str) {
    unsafe {
        ShellExecuteW(0, wide("open").as_ptr(), wide(path).as_ptr(), std::ptr::null(), std::ptr::null(), SW_SHOWNORMAL);
    }
}
