"""Live window for the tg18 emulator: see the screen and press A, B and C.

Usage: python tg18win.py <flash.bin> [--save FILE] [--snapshot-in FILE]

Keys: A / B / C, or Left / Down / Right. Clicking the buttons works too.
M mutes the sound.
Holding A and C together presses both, as on the real toy.
Uses only tkinter and ctypes (part of Python), no extra packages. Sound
plays through Windows' winmm; on other systems the window runs silent.
"""
import argparse
import array
import ctypes
import datetime
import sys
import time
import tkinter as tk

import tg18emu as te

AUDIO_RATE = 44100
AUDIO_LOW = 0.05              # keep at least this much sound queued (s), against dropouts
AUDIO_HIGH = 0.09             # above this, silences are shortened back to AUDIO_LOW + a chunk
AUDIO_CHUNK = 0.02            # smallest block handed to Windows (s)


class WaveOut:
    """Streams 16-bit mono samples to the speakers through Windows' winmm.

    Uses only ctypes, so no extra packages. Not available on other systems.
    """

    class WAVEHDR(ctypes.Structure):
        _fields_ = [('lpData', ctypes.c_void_p), ('dwBufferLength', ctypes.c_uint32),
                    ('dwBytesRecorded', ctypes.c_uint32), ('dwUser', ctypes.c_void_p),
                    ('dwFlags', ctypes.c_uint32), ('dwLoops', ctypes.c_uint32),
                    ('lpNext', ctypes.c_void_p), ('reserved', ctypes.c_void_p)]

    class WAVEFORMATEX(ctypes.Structure):
        _fields_ = [('wFormatTag', ctypes.c_uint16), ('nChannels', ctypes.c_uint16),
                    ('nSamplesPerSec', ctypes.c_uint32), ('nAvgBytesPerSec', ctypes.c_uint32),
                    ('nBlockAlign', ctypes.c_uint16), ('wBitsPerSample', ctypes.c_uint16),
                    ('cbSize', ctypes.c_uint16)]

    WHDR_DONE = 1

    def __init__(self, rate):
        self.winmm = ctypes.WinDLL('winmm')
        fmt = self.WAVEFORMATEX(1, 1, rate, rate * 2, 2, 16, 0)       # PCM, mono, 16 bit
        self.handle = ctypes.c_void_p()
        err = self.winmm.waveOutOpen(ctypes.byref(self.handle), ctypes.c_uint(0xFFFFFFFF),
                                     ctypes.byref(fmt), None, None, 0)
        if err:
            raise OSError('waveOutOpen failed (%d)' % err)
        self.queue = []                             # (header, buffer, samples) still playing
        self.hsize = ctypes.sizeof(self.WAVEHDR)

    def queued(self):
        """Samples written but not played yet; finished buffers are released."""
        still = []
        for hdr, buf, n in self.queue:
            if hdr.dwFlags & self.WHDR_DONE:
                self.winmm.waveOutUnprepareHeader(self.handle, ctypes.byref(hdr), self.hsize)
            else:
                still.append((hdr, buf, n))
        self.queue = still
        return sum(n for _, _, n in still)

    def write(self, samples):
        if not samples:
            return
        data = samples.tobytes()
        buf = ctypes.create_string_buffer(data, len(data))
        hdr = self.WAVEHDR(ctypes.cast(buf, ctypes.c_void_p), len(data), 0, None, 0, 0, None, None)
        self.winmm.waveOutPrepareHeader(self.handle, ctypes.byref(hdr), self.hsize)
        self.winmm.waveOutWrite(self.handle, ctypes.byref(hdr), self.hsize)
        self.queue.append((hdr, buf, len(samples)))

    def close(self):
        self.winmm.waveOutReset(self.handle)
        self.queued()
        self.winmm.waveOutClose(self.handle)

TICK_MS = 5                   # how often the window runs the emulator
MAX_STEP = 0.02               # most emulated seconds per tick, so sound and window stay smooth
MAX_BEHIND = 0.25             # most emulated time (s) the window will catch up on
MAX_CATCHUP = 10.0           # most the device clock may run ahead of the CPU when it lags
KEYMAP = {'a': 'A', 'b': 'B', 'c': 'C', 'Left': 'A', 'Down': 'B', 'Right': 'C'}

# RGB565 (as stored in the LCD framebuffer) -> 3 bytes of RGB
RGB_TABLE = [bytes(((v >> 11 & 0x1F) * 255 // 31, (v >> 5 & 0x3F) * 255 // 63,
                    (v & 0x1F) * 255 // 31)) for v in range(65536)]
PPM_HEADER = b'P6 %d %d 255\n' % (te.LCD_W, te.LCD_H)

BODY = '#f3d2e0'
BUTTON = '#fbf4f7'
BUTTON_DOWN = '#e38aac'
INK = '#5a2a3f'


class Window:
    def __init__(self, emu, args):
        self.emu = emu
        self.args = args
        self.scale = args.scale
        self.down = set()                               # keys held on keyboard or mouse
        self.frame_dirty = True
        self.was_asleep = False
        self.last_wall = time.perf_counter()
        self.behind = 0.0                               # real time not yet emulated (s)
        self.speed_window = (self.last_wall, emu.executed)
        self.stopped = None
        self.synth = te.ToneSynth(AUDIO_RATE, args.volume / 100)
        self.muted = False
        self.audio = None
        self.audio_t = None                         # emulated time rendered to sound so far
        self.pending = array.array('h')             # samples waiting to fill a chunk
        if args.volume > 0:
            try:
                self.audio = WaveOut(AUDIO_RATE)
            except (OSError, AttributeError) as e:     # no winmm (not Windows) or no device
                print('sound off: %s' % e)

        self.root = root = tk.Tk()
        root.title('tg18 emulator')
        root.configure(bg=BODY)
        root.resizable(False, False)

        w = te.LCD_W * self.scale
        self.screen = tk.Canvas(root, width=w, height=w, bg='black',
                                highlightthickness=0)
        self.screen.pack(padx=24, pady=(24, 12))
        self.small = tk.PhotoImage(width=te.LCD_W, height=te.LCD_H)
        self.big = tk.PhotoImage(width=w, height=w)
        self.screen.create_image(0, 0, image=self.big, anchor='nw')

        pad = tk.Canvas(root, width=w, height=90, bg=BODY, highlightthickness=0)
        pad.pack(padx=24)
        self.pad = pad
        self.circles = {}
        r = 28
        for i, key in enumerate('ABC'):
            cx = w * (i + 1) // 4
            cy = 30 if key == 'B' else 50                  # B sits higher, like the toy
            oval = pad.create_oval(cx - r, cy - r, cx + r, cy + r, fill=BUTTON,
                                   outline=INK, width=2)
            label = pad.create_text(cx, cy, text=key, fill=INK,
                                    font=('Segoe UI', 16, 'bold'))
            self.circles[key] = oval
            for item in (oval, label):
                pad.tag_bind(item, '<ButtonPress-1>', lambda e, k=key: self.press(k))
                pad.tag_bind(item, '<ButtonRelease-1>', lambda e, k=key: self.release(k))

        self.status = tk.Label(root, text='', bg=BODY, fg=INK, font=('Segoe UI', 9))
        self.status.pack(pady=(4, 2))
        tk.Label(root, text='Keys: A B C  or  ← ↓ →   (A+C together for both)   M: mute',
                 bg=BODY, fg=INK, font=('Segoe UI', 9)).pack(pady=(0, 12))

        root.bind('<KeyPress>', self.on_key_down)
        root.bind('<KeyRelease>', self.on_key_up)
        root.protocol('WM_DELETE_WINDOW', self.close)
        self.hook_lcd()
        root.after(TICK_MS, self.tick)

    # --- input -----------------------------------------------------------

    def on_key_down(self, event):
        if event.keysym.lower() == 'm':
            self.muted = not self.muted
            return
        key = KEYMAP.get(event.keysym) or KEYMAP.get(event.keysym.lower())
        if key:
            self.press(key)

    def on_key_up(self, event):
        key = KEYMAP.get(event.keysym) or KEYMAP.get(event.keysym.lower())
        if key:
            self.release(key)

    def press(self, key):
        if key in self.down:                            # keyboard auto-repeat
            return
        self.down.add(key)
        p = self.emu.periph
        p.live_keys.add(key)
        # also script a short press, so a quick tap is held long enough for
        # the firmware to see it and so a press wakes the device from sleep
        now = self.emu.executed / te.CPU_HZ
        p.key_script[:] = [(t, k) for t, k in p.key_script if t > now - 1.0]
        p.key_script.append((now, key))
        self.pad.itemconfigure(self.circles[key], fill=BUTTON_DOWN)

    def release(self, key):
        self.down.discard(key)
        self.emu.periph.live_keys.discard(key)
        self.pad.itemconfigure(self.circles[key], fill=BUTTON)

    # --- emulation -------------------------------------------------------

    def hook_lcd(self):
        def on_frame(lcd):
            self.frame_dirty = True
        self.emu.periph.lcd.on_frame = on_frame

    def tick(self):
        if self.stopped:
            return
        wall = time.perf_counter()
        real = wall - self.last_wall
        self.last_wall = wall
        # time a late tick missed is made up over the next ticks; only what
        # piles up beyond MAX_BEHIND is dropped
        debt = self.behind + real
        dropped = max(0.0, debt - MAX_BEHIND)
        debt -= dropped
        step = min(debt, MAX_STEP)
        self.behind = debt - step
        emu = self.emu
        t_start = emu.executed / te.CPU_HZ
        target = emu.executed + int(step * te.CPU_HZ)
        # when the CPU can't keep up at all, run the device clock faster for
        # this step, so the game's clock still follows real time
        emu.periph.turbo = min((step + dropped) / step, MAX_CATCHUP) if step > 0 else 1.0

        if emu.periph.powered_off:
            woken = te.power_cycle(emu, target)
            if woken is None:                               # no alarm: sleep until a button
                p = emu.periph
                p.game_time += (target - emu.executed) / te.CPU_HZ * p.turbo
                emu.executed = target
                p.now = target / te.CPU_HZ
            elif woken is not emu:
                self.emu = woken
                self.hook_lcd()
                self.frame_dirty = True
        else:
            outcome = emu.run(target)
            if outcome in ('crash', 'stuck'):
                self.stopped = outcome
                self.status.configure(text='Emulator stopped (%s), see the console' % outcome)
                return

        self.play_sound(t_start, self.emu.executed / te.CPU_HZ)
        asleep = self.emu.periph.powered_off
        if asleep and not self.was_asleep and self.args.save:
            te.write_save(self.emu, self.args.save)         # the firmware saved before sleeping
        self.was_asleep = asleep

        if self.frame_dirty:
            self.frame_dirty = False
            self.draw()
        self.update_status(wall)
        spent = (time.perf_counter() - wall) * 1000
        self.root.after(max(1, int(TICK_MS - spent)), self.tick)

    def play_sound(self, t0, t1):
        """Send the buzzer's sound for emulated time t0..t1 to the speakers.

        The span is rendered at its true length, so notes keep their exact
        durations. The delay to the speakers is kept between AUDIO_LOW and
        AUDIO_HIGH: when the emulator falls behind, the current sound is held
        a little longer instead of breaking up, and extra delay is only ever
        removed during silence.
        """
        events = self.emu.periph.sound.events           # shared across sleep/wake
        span, events[:] = list(events), []
        if not self.audio:
            if span:
                _, self.synth.freq, self.synth.duty = span[-1]
            return
        if self.audio_t is None:
            self.audio_t = t0
        n = max(0, int((t1 - self.audio_t) * AUDIO_RATE))
        t_end = self.audio_t + n / AUDIO_RATE           # whole samples only, so no drift
        out = array.array('h')
        self.synth.render(span, self.audio_t, t_end, n, out)
        self.audio_t = t_end

        queued = self.audio.queued() + len(self.pending)
        low, high = int(AUDIO_LOW * AUDIO_RATE), int(AUDIO_HIGH * AUDIO_RATE)
        if queued + len(out) < low:
            self.synth.tone(low - queued - len(out), out)
        elif queued + len(out) > high and not self.synth.freq:
            silent = len(out)
            while silent and not out[silent - 1]:
                silent -= 1
            excess = queued + len(out) - low - int(AUDIO_CHUNK * AUDIO_RATE)
            del out[max(silent, len(out) - excess):]
        if self.muted:
            out = array.array('h', bytes(len(out) * 2))
        self.pending.extend(out)
        if len(self.pending) >= AUDIO_CHUNK * AUDIO_RATE or self.audio.queued() < low // 2:
            self.audio.write(self.pending)
            self.pending = array.array('h')

    def draw(self):
        fb = memoryview(self.emu.periph.lcd.fb).cast('H')
        data = PPM_HEADER + b''.join([RGB_TABLE[v] for v in fb])
        self.small.configure(data=data, format='PPM')
        self.big.tk.call(self.big, 'copy', self.small, '-zoom', self.scale, self.scale)

    def update_status(self, wall):
        t0, e0 = self.speed_window
        if wall - t0 < 1.0:
            return
        speed = (self.emu.executed - e0) / te.CPU_HZ / (wall - t0)
        self.speed_window = (wall, self.emu.executed)
        if self.emu.periph.powered_off:
            text = 'Sleeping: press any button to wake it'
        else:
            text = 'Speed: %.2f× real time' % speed
            if speed < 0.95:
                text += ' (slow motion; the clock stays in sync)'
        self.status.configure(text=text)

    def close(self):
        self.stopped = self.stopped or 'closed'
        if self.audio:
            self.audio.close()
        if self.args.save:
            te.write_save(self.emu, self.args.save)
            print('flash and clock saved to %s' % self.args.save)
        if self.args.snapshot_out:
            te.save_snapshot(self.emu, self.args.snapshot_out)
            print('snapshot written to %s' % self.args.snapshot_out)
        self.root.destroy()


def main():
    ap = argparse.ArgumentParser(description='Play a tg18 flash image in a window.')
    ap.add_argument('image')
    ap.add_argument('--save', metavar='FILE',
                    help='persistent flash image (+ FILE.json for the clock); '
                         'boots from it if present, written back on sleep and on close')
    ap.add_argument('--snapshot-in', metavar='FILE',
                    help='resume from a snapshot made by tg18emu.py --snapshot-out')
    ap.add_argument('--snapshot-out', metavar='FILE',
                    help='freeze the whole machine to FILE when the window closes')
    ap.add_argument('--date', metavar='"YYYY-MM-DD HH:MM"',
                    help='RTC start time (default: now, or the saved clock)')
    ap.add_argument('--scale', type=int, default=4, help='screen zoom (default 4)')
    ap.add_argument('--volume', type=int, default=60,
                    help='buzzer volume 0-100 (default 60, 0 = no sound)')
    a = ap.parse_args()

    image = open(a.image, 'rb').read()
    if image[:4] != b'SPII':
        sys.exit('not a tg18 SPI image (missing SPII header)')
    start = datetime.datetime.strptime(a.date, '%Y-%m-%d %H:%M') if a.date else None
    saved = te.load_save(a.save, image) if a.save else None
    emu = te.Emulator(saved or image, False, start)
    if saved and not a.date:
        te.restore_rtc(emu, a.save)
    if a.snapshot_in:
        te.load_snapshot(emu, a.snapshot_in)
    Window(emu, a).root.mainloop()


if __name__ == '__main__':
    main()
