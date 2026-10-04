//! The egg icon in the notification area (the tray, next to the clock)
//! while the toy is on the desktop: click to show or hide the toy,
//! right-click for a small menu. Messages arrive at the main window as
//! WM_TRAY.

use crate::win32::*;

/// Message the tray icon sends to the main window.
pub const WM_TRAY: u32 = WM_APP + 1;

pub struct Tray {
    hwnd: HWND,
}

impl Tray {
    /// Show the icon; `tip` is the text when the mouse rests on it.
    pub fn add(hwnd: HWND, tip: &str) -> Tray {
        let t = Tray { hwnd };
        t.send(NIM_ADD, tip);
        t
    }

    /// Show it again after Explorer restarted (the taskbar is rebuilt
    /// without it), or with a new tip.
    pub fn update(&self, tip: &str) {
        if !self.send(NIM_MODIFY, tip) {
            self.send(NIM_ADD, tip);
        }
    }

    fn send(&self, what: u32, tip: &str) -> bool {
        let mut d = NOTIFYICONDATAW::default();
        d.cbSize = std::mem::size_of::<NOTIFYICONDATAW>() as u32;
        d.hWnd = self.hwnd;
        d.uID = 1;
        d.uFlags = NIF_MESSAGE | NIF_ICON | NIF_TIP;
        d.uCallbackMessage = WM_TRAY;
        d.hIcon = crate::icon::big_and_small().1;
        for (o, c) in d.szTip.iter_mut().zip(tip.encode_utf16().take(127)) {
            *o = c;
        }
        unsafe { Shell_NotifyIconW(what, &d) != 0 }
    }
}

impl Drop for Tray {
    fn drop(&mut self) {
        let mut d = NOTIFYICONDATAW::default();
        d.cbSize = std::mem::size_of::<NOTIFYICONDATAW>() as u32;
        d.hWnd = self.hwnd;
        d.uID = 1;
        unsafe { Shell_NotifyIconW(NIM_DELETE, &d) };
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn struct_size_matches_windows() {
        // NOTIFYICONDATAW on 64-bit Windows
        assert_eq!(std::mem::size_of::<crate::win32::NOTIFYICONDATAW>(), 976);
    }
}
