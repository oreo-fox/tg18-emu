"""Drive the Rust window app for tests: find it, press keys, capture it, close it.

    python winctl.py shot OUT.png          window contents (client area) as PNG
    python winctl.py keys "B x A A B" [gap] press keys (x = wait), gap seconds apart
    python winctl.py menu ID               send a menu command
    python winctl.py close                 close it like the X button
"""
import ctypes
import os
import struct
import sys
import time
from ctypes import wintypes

sys.path.insert(0, os.path.join(os.path.dirname(os.path.abspath(__file__)), '..'))
import tg18emu as te

user32 = ctypes.WinDLL('user32', use_last_error=True)
gdi32 = ctypes.WinDLL('gdi32')
user32.FindWindowW.restype = wintypes.HWND
user32.GetDC.restype = wintypes.HDC
user32.GetDC.argtypes = [wintypes.HWND]
user32.PostMessageW.argtypes = [wintypes.HWND, wintypes.UINT, wintypes.WPARAM, wintypes.LPARAM]
user32.PrintWindow.argtypes = [wintypes.HWND, wintypes.HDC, wintypes.UINT]
gdi32.CreateCompatibleDC.restype = wintypes.HDC
gdi32.CreateCompatibleDC.argtypes = [wintypes.HDC]
gdi32.CreateCompatibleBitmap.restype = wintypes.HBITMAP
gdi32.CreateCompatibleBitmap.argtypes = [wintypes.HDC, ctypes.c_int, ctypes.c_int]
gdi32.SelectObject.argtypes = [wintypes.HDC, wintypes.HGDIOBJ]
gdi32.GetDIBits.argtypes = [wintypes.HDC, wintypes.HBITMAP, wintypes.UINT, wintypes.UINT,
                            ctypes.c_void_p, ctypes.c_void_p, wintypes.UINT]

WM_KEYDOWN, WM_KEYUP, WM_COMMAND, WM_CLOSE = 0x100, 0x101, 0x111, 0x10


def window():
    h = user32.FindWindowW('tg18emu', None)
    if not h:
        sys.exit('window not found')
    return h


def shot(path):
    h = window()
    r = wintypes.RECT()
    user32.GetClientRect(h, ctypes.byref(r))
    w, hgt = r.right, r.bottom
    dc = user32.GetDC(h)
    mem = gdi32.CreateCompatibleDC(dc)
    bmp = gdi32.CreateCompatibleBitmap(dc, w, hgt)
    gdi32.SelectObject(mem, bmp)
    user32.PrintWindow(h, mem, 1)                   # PW_CLIENTONLY
    hdr = struct.pack('<IiiHHIIiiII', 40, w, -hgt, 1, 32, 0, 0, 0, 0, 0, 0)
    info = ctypes.create_string_buffer(hdr + b'\0' * 16)
    buf = ctypes.create_string_buffer(w * hgt * 4)
    gdi32.GetDIBits(mem, bmp, 0, hgt, buf, info, 0)
    raw = buf.raw
    rgb = bytearray(w * hgt * 3)
    rgb[0::3], rgb[1::3], rgb[2::3] = raw[2::4], raw[1::4], raw[0::4]
    te.write_png(path, w, hgt, bytes(rgb), 1)
    print('%s: %dx%d' % (path, w, hgt))


def keys(spec, gap=1.0):
    h = window()
    for k in spec.split():
        if k.lower() != 'x':
            vk = ord(k.upper())
            user32.PostMessageW(h, WM_KEYDOWN, vk, 0)
            time.sleep(0.08)
            user32.PostMessageW(h, WM_KEYUP, vk, 0xC0000001)
        time.sleep(gap)


if __name__ == '__main__':
    cmd = sys.argv[1]
    if cmd == 'shot':
        shot(sys.argv[2])
    elif cmd == 'keys':
        keys(sys.argv[2], float(sys.argv[3]) if len(sys.argv) > 3 else 1.0)
    elif cmd == 'menu':
        user32.PostMessageW(window(), WM_COMMAND, int(sys.argv[2]), 0)
    elif cmd == 'close':
        user32.PostMessageW(window(), WM_CLOSE, 0, 0)
