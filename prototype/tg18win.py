"""Live window for the tg18 emulator: see the screen, press A, B and C, hear it.

Usage: python tg18win.py [flash.bin]

With no argument it reopens the last ROM and resumes where you left off; the
first time it asks for the folder with your ROM dumps. File > Open ROM
switches between them, each with its own saves (see tg18saves.py).

Keys: A / B / C, or Left / Down / Right. Clicking the buttons works too.
Holding A and C together presses both, as on the real toy. M mutes,
Ctrl+S saves. Uses only tkinter and ctypes (part of Python), no extra
packages. Sound plays through Windows' winmm; elsewhere the window is silent.
"""
import array
import ctypes
import os
import sys
import time
import tkinter as tk
from tkinter import filedialog, messagebox

import tg18emu as te
import tg18saves as ts

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
REWIND_GAP = 1.0              # at most one undo snapshot per second of button presses
MAX_CATCHUP = 10.0           # most the device clock may run ahead of the CPU when it lags
MIN_HOLD = 0.05               # emulated seconds a quick tap is held (firmware debounce: 44 ms)
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
    def __init__(self, settings, rom=None):
        self.settings = settings
        self.writer = ts.Writer()
        self.emu = self.image = self.store = self.rom_path = None
        self.down = set()                               # keys held on keyboard or mouse
        self.message = None                             # (text, until) shown in the status line
        self.stopped = None
        self.status = None
        self.rewind_point, self.rewind_at = None, 0.0   # undo point for Bluetooth, see press()
        self.synth = te.ToneSynth(AUDIO_RATE, settings['volume'] / 100)
        self.audio = None
        try:
            self.audio = WaveOut(AUDIO_RATE)
        except (OSError, AttributeError) as e:         # no winmm (not Windows) or no device
            print('sound off: %s' % e)
        self.reset_timing()

        self.root = root = tk.Tk()
        root.title('tg18 emulator')
        root.configure(bg=BODY)
        root.resizable(False, False)
        self.build_menu()
        self.frame = None
        self.layout()
        self.status = tk.Label(root, text='', bg=BODY, fg=INK, font=('Segoe UI', 9))
        self.status.pack(pady=(4, 2))
        tk.Label(root, text='Keys: A B C  or  ← ↓ →   (A+C together for both)   '
                 'M: mute   Ctrl+S: save', bg=BODY, fg=INK, font=('Segoe UI', 9)).pack(pady=(0, 12))

        root.bind('<KeyPress>', self.on_key_down)
        root.bind('<KeyRelease>', self.on_key_up)
        root.bind('<Control-s>', lambda e: self.save_now())
        root.protocol('WM_DELETE_WINDOW', self.close)
        root.after(50, lambda: self.start(rom))
        root.after(TICK_MS, self.tick)

    # --- menus and settings ------------------------------------------------

    def build_menu(self):
        s = self.settings
        bar = tk.Menu(self.root)
        self.var = {
            'volume': tk.IntVar(value=s['volume']),
            'muted': tk.BooleanVar(value=s['muted']),
            'scale': tk.IntVar(value=s['scale']),
            'autosave_minutes': tk.IntVar(value=s['autosave_minutes']),
            'never_sleep': tk.BooleanVar(value=s['never_sleep']),
            'pause_time_when_closed': tk.BooleanVar(value=s['pause_time_when_closed']),
            'rom': tk.StringVar(value=''),
        }
        filem = tk.Menu(bar, tearoff=False)
        self.rom_menu = tk.Menu(filem, tearoff=False, postcommand=self.fill_rom_menu)
        filem.add_cascade(label='Open ROM', menu=self.rom_menu)
        filem.add_command(label='Choose ROM folder…', command=self.choose_folder)
        filem.add_separator()
        filem.add_command(label='Save now', accelerator='Ctrl+S', command=self.save_now)
        self.save_slot_menu = tk.Menu(filem, tearoff=False,
                                      postcommand=lambda: self.fill_slot_menu(True))
        self.load_slot_menu = tk.Menu(filem, tearoff=False,
                                      postcommand=lambda: self.fill_slot_menu(False))
        filem.add_cascade(label='Save to slot', menu=self.save_slot_menu)
        filem.add_cascade(label='Load slot', menu=self.load_slot_menu)
        filem.add_command(label='Open saves folder', command=self.open_saves_folder)
        filem.add_separator()
        filem.add_command(label='Quit', command=self.close)
        bar.add_cascade(label='File', menu=filem)

        setm = tk.Menu(bar, tearoff=False)
        vol = tk.Menu(setm, tearoff=False)
        for v in (20, 40, 60, 80, 100):
            vol.add_radiobutton(label='%d%%' % v, value=v, variable=self.var['volume'],
                                command=lambda: self.changed('volume'))
        setm.add_cascade(label='Volume', menu=vol)
        setm.add_checkbutton(label='Mute', accelerator='M', variable=self.var['muted'],
                             command=lambda: self.changed('muted'))
        size = tk.Menu(setm, tearoff=False)
        for v in (2, 3, 4, 5, 6):
            size.add_radiobutton(label='%d× (%d px)' % (v, v * te.LCD_W), value=v,
                                 variable=self.var['scale'], command=lambda: self.changed('scale'))
        setm.add_cascade(label='Screen size', menu=size)
        auto = tk.Menu(setm, tearoff=False)
        for v in (0, 1, 2, 5, 10):
            label = 'Off' if not v else 'Every %d minute%s' % (v, 's' if v > 1 else '')
            auto.add_radiobutton(label=label, value=v, variable=self.var['autosave_minutes'],
                                 command=lambda: self.changed('autosave_minutes'))
        setm.add_cascade(label='Autosave', menu=auto)
        setm.add_separator()
        setm.add_checkbutton(label='Never sleep (keep the screen on)',
                             variable=self.var['never_sleep'],
                             command=lambda: self.changed('never_sleep'))
        setm.add_checkbutton(label='Pause time while closed',
                             variable=self.var['pause_time_when_closed'],
                             command=lambda: self.changed('pause_time_when_closed'))
        bar.add_cascade(label='Settings', menu=setm)
        self.root.configure(menu=bar)

    def changed(self, name):
        value = self.var[name].get()
        self.settings[name] = value
        ts.store_settings(self.settings)
        if name == 'volume':
            self.synth.amp = int(12000 * value / 100)
        elif name == 'scale':
            self.layout()
        elif name == 'never_sleep' and self.emu:
            self.emu.never_sleep = value
        elif name == 'autosave_minutes':
            self.next_autosave = time.time() + value * 60

    def fill_rom_menu(self):
        m = self.rom_menu
        m.delete(0, 'end')
        roms = ts.list_roms(self.settings['rom_dir'])
        if not roms:
            m.add_command(label='(no ROMs found: choose the ROM folder)', state='disabled')
        for filename, name in roms:
            m.add_radiobutton(label=name, value=filename, variable=self.var['rom'],
                              command=lambda f=filename: self.switch_rom(f))

    def fill_slot_menu(self, saving):
        m = self.save_slot_menu if saving else self.load_slot_menu
        m.delete(0, 'end')
        for n in range(1, ts.SLOTS + 1):
            when = self.store.slot_time(n) if self.store else None
            label = 'Slot %d: %s' % (n, time.strftime('%d %b %Y %H:%M', time.localtime(when))
                                     if when else 'empty')
            if saving:
                m.add_command(label=label, command=lambda n=n: self.save_slot(n),
                              state='normal' if self.emu else 'disabled')
            else:
                m.add_command(label=label, command=lambda n=n: self.load_slot(n),
                              state='normal' if when else 'disabled')

    def open_saves_folder(self):
        folder = self.store.dir if self.store else ts.SAVES_DIR
        os.makedirs(folder, exist_ok=True)
        if hasattr(os, 'startfile'):
            os.startfile(folder)

    def say(self, text, seconds=3.0):
        self.message = (text, time.time() + seconds)
        if self.status:
            self.status.configure(text=text)
            self.root.update_idletasks()

    # --- screen ------------------------------------------------------------

    def layout(self):
        """(Re)build the screen and buttons for the current screen size."""
        if self.frame:
            self.frame.destroy()
        self.scale = self.settings['scale']
        self.frame = frame = tk.Frame(self.root, bg=BODY)
        if self.status:
            frame.pack(before=self.status)
        else:
            frame.pack()
        w = te.LCD_W * self.scale
        self.screen = tk.Canvas(frame, width=w, height=w, bg='black', highlightthickness=0)
        self.screen.pack(padx=24, pady=(24, 12))
        self.small = tk.PhotoImage(width=te.LCD_W, height=te.LCD_H)
        self.big = tk.PhotoImage(width=w, height=w)
        self.screen.create_image(0, 0, image=self.big, anchor='nw')
        self.hint = self.screen.create_text(w // 2, w // 2, text='', fill='white', width=w - 40,
                                            font=('Segoe UI', 11), justify='center')
        pad = tk.Canvas(frame, width=w, height=90, bg=BODY, highlightthickness=0)
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
        self.frame_dirty = True

    def show_hint(self, text):
        self.screen.itemconfigure(self.hint, text=text)

    def draw(self):
        fb = memoryview(self.emu.periph.lcd.fb).cast('H')
        data = PPM_HEADER + b''.join([RGB_TABLE[v] for v in fb])
        self.small.configure(data=data, format='PPM')
        self.big.tk.call(self.big, 'copy', self.small, '-zoom', self.scale, self.scale)

    # --- ROMs ----------------------------------------------------------------

    def start(self, rom):
        """Open the ROM given on the command line, else the last one used."""
        s = self.settings
        if rom:
            self.open_rom(os.path.abspath(rom))
            return
        if s['rom_dir'] and s['last_rom'] and ts.is_rom(os.path.join(s['rom_dir'], s['last_rom'])):
            self.open_rom(os.path.join(s['rom_dir'], s['last_rom']))
            return
        self.show_hint('Choose the folder with your ROM dumps\n(File > Choose ROM folder)')
        if not s['rom_dir']:
            self.choose_folder()

    def choose_folder(self):
        folder = filedialog.askdirectory(parent=self.root, title='Folder with your tg18 ROM dumps',
                                         initialdir=self.settings['rom_dir'] or os.path.expanduser('~'))
        if not folder:
            return
        roms = ts.list_roms(folder)
        if not roms:
            messagebox.showwarning('No ROMs found', 'No tg18 flash dumps (8 MiB .bin files '
                                   'starting with SPII) were found in\n%s' % folder, parent=self.root)
            return
        self.settings['rom_dir'] = folder
        ts.store_settings(self.settings)
        if not self.emu:
            self.show_hint('Pick a ROM: File > Open ROM')
            if len(roms) == 1:
                self.switch_rom(roms[0][0])
            else:
                self.pick_rom(roms)

    def pick_rom(self, roms):
        """Small list dialog for choosing the first ROM."""
        top = tk.Toplevel(self.root)
        top.title('Open ROM')
        top.transient(self.root)
        lb = tk.Listbox(top, height=len(roms), width=34, font=('Segoe UI', 10), activestyle='none')
        for _, name in roms:
            lb.insert('end', name)
        lb.pack(padx=12, pady=12)
        lb.selection_set(0)

        def ok(event=None):
            sel = lb.curselection()
            top.destroy()
            if sel:
                self.switch_rom(roms[sel[0]][0])
        lb.bind('<Double-Button-1>', ok)
        lb.bind('<Return>', ok)
        tk.Button(top, text='Open', command=ok, width=10).pack(pady=(0, 12))
        lb.focus_set()

    def switch_rom(self, filename):
        path = os.path.join(self.settings['rom_dir'], filename)
        if self.rom_path and os.path.abspath(path) == os.path.abspath(self.rom_path):
            return
        self.open_rom(path)

    def open_rom(self, path):
        if not ts.is_rom(path):
            messagebox.showerror('Not a tg18 ROM', '%s is not an 8 MiB tg18 flash dump.' % path,
                                 parent=self.root)
            return
        if self.emu:
            self.shutdown_game()
        self.emu = None
        image = open(path, 'rb').read()
        self.say('Loading %s…' % ts.display_name(os.path.basename(path)), 10)
        store = ts.SaveStore(path)
        legacy = store.import_legacy(image)
        try:
            emu, elapsed, how = self.boot(store, image)
        except te.SaveMismatch as e:
            messagebox.showerror('Save does not match', str(e), parent=self.root)
            return
        self.image, self.store, self.rom_path = image, store, path
        self.set_emu(emu)
        s = self.settings
        s['rom_dir'] = s['rom_dir'] or os.path.dirname(path)
        if os.path.abspath(os.path.dirname(path)) == os.path.abspath(s['rom_dir']):
            s['last_rom'] = os.path.basename(path)
        ts.store_settings(s)
        self.var['rom'].set(os.path.basename(path))
        self.root.title('%s – tg18 emulator' % ts.display_name(os.path.basename(path)))
        self.show_hint('')
        if elapsed:
            self.advance_clock(elapsed)
        note = ' (imported %s)' % os.path.basename(legacy) if legacy else ''
        self.say('%s%s' % (how, note), 4)

    def boot(self, store, image):
        """A new Emulator for this ROM's newest save: (emu, seconds to catch up, message)."""
        pause = self.settings['pause_time_when_closed']
        kind, meta = store.newest()
        if kind == 'flash':
            emu = te.Emulator(te.load_save(store.flash, image), False, None)
            emu.image = image
            if meta:
                te.restore_rtc(emu, store.flash, advance=not pause)
            if meta and meta.get('asleep'):
                # it went to sleep when the window closed: wake it with a button
                # (the clock has moved on, the pet hasn't: see advance_clock)
                emu.periph.key_script.append((0.0, 'B'))
                away = time.time() - meta['saved_at']
                if pause or away < 60:
                    return emu, 0, 'Resumed from the last session'
                return emu, 0, 'Resumed; clock moved forward by %s' % describe(away)
            return emu, 0, 'Booted from the flash save'
        if kind == 'snapshot':
            emu = te.Emulator(image, False, None)
            state = te.load_snapshot(emu, store.autosave)
            elapsed = 0 if pause else max(0.0, time.time() - state.get('saved_at', time.time()))
            return emu, elapsed, 'Resumed from the autosave'
        return te.Emulator(image, False, None), 0, 'New game'

    def set_emu(self, emu):
        emu.never_sleep = self.settings['never_sleep']
        emu.stop_on_bluetooth = True
        self.rewind_point, self.rewind_at = None, 0.0
        self.emu = emu
        self.hook_lcd()
        self.reset_timing()
        self.next_autosave = time.time() + self.settings['autosave_minutes'] * 60

    def reset_timing(self):
        self.last_wall = time.perf_counter()
        self.behind = 0.0                               # real time not yet emulated (s)
        self.speed_window = (self.last_wall, self.emu.executed if self.emu else 0)
        self.audio_t = None                             # emulated time rendered to sound so far
        self.pending = array.array('h')                 # samples waiting to fill a chunk
        self.frame_dirty = True
        self.was_asleep = False

    # --- sleeping, saving, loading --------------------------------------------

    def force_sleep(self, limit=10.0):
        """Make the firmware go to sleep now, as it would after the idle timeout.

        On the way down it saves everything to flash, like the real toy.
        Sets the idle counter past any limit (30 or 180 s) and runs until the
        power goes off. Returns False where the firmware doesn't sleep (for
        example during the first setup) or the counter wasn't found.
        """
        emu = self.emu
        if emu.periph.powered_off:
            return True
        if emu.idle_counter is None:
            return False
        never, emu.never_sleep = emu.never_sleep, False
        end = emu.executed + int(limit * te.CPU_HZ)
        while not emu.periph.powered_off and emu.executed < end:
            emu.uc.mem_write(emu.idle_counter, b'\xfe')
            if emu.run(min(end, emu.executed + te.CPU_HZ // 10)) in ('crash', 'stuck', 'ble_fail', 'bluetooth', 'infrared'):
                break
        emu.never_sleep = never
        emu.periph.sound.events[:] = []
        return emu.periph.powered_off

    def advance_clock(self, elapsed):
        """Move the device clock forward by the time the window was closed.

        Puts the device to sleep, moves the clock and wakes it with a button,
        so the firmware reads the new time the way it does on a real wake-up.
        The pet itself doesn't change, by design (like a toy with the
        batteries out): the firmware only counts time through the alarm
        wake-ups it makes about once a minute while asleep, and those are
        not replayed (30 minutes would cost ~45 s).
        """
        self.say('Clock moved forward by %s' % describe(elapsed), 5)
        if self.force_sleep():
            p = self.emu.periph
            p.rtc.base_ticks += int(elapsed * 32768)
            p.key_script.append((self.emu.executed / te.CPU_HZ, 'B'))
            self.set_emu(te.power_cycle(self.emu, self.emu.executed + te.CPU_HZ) or self.emu)
        else:                                           # no sleep possible: just move the clock
            self.emu.periph.rtc.base_ticks += int(elapsed * 32768)

    def connection_blocked(self, kind):
        """The game wants Bluetooth or infrared: undo the press that led here and explain."""
        for key in list(self.down):
            self.release(key)
        if self.rewind_point is None:                   # nothing to go back to
            self.bluetooth_hang()
            return
        emu = te.Emulator(self.image, False, None)
        te.restore_snapshot(emu, self.rewind_point)
        self.set_emu(emu)
        self.draw()
        what = {'bluetooth': "Bluetooth functions (the Tamagotchi app, the camera, downloads "
                             "from the app) are not supported by the emulator.",
                'infrared': "Infrared connections with another Tamagotchi (playdates, gifts, "
                            "marrying, downloads) are not supported by the emulator."}[kind]
        messagebox.showinfo('Not supported', what + "\n\nThe game has been put back to "
                            "just before you chose it.", parent=self.root)
        self.reset_timing()

    def bluetooth_hang(self):
        """The game froze waiting for the Bluetooth chip: restart the toy."""
        messagebox.showinfo(
            'Bluetooth is not emulated',
            "This needs Bluetooth (it talks to the Tamagotchi phone app), which the "
            "emulator can't do yet. The game froze waiting for the Bluetooth chip, "
            "so the toy is restarted, like taking the batteries out.\n\n"
            "Choose CONTINUE to carry on from the game's last own save.",
            parent=self.root)
        self.set_emu(te.restart(self.emu))
        self.say('Restarted after the Bluetooth freeze', 5)

    def save_now(self, path=None, label='Saved'):
        """Snapshot the machine; the writing happens in the background."""
        if not self.emu or self.stopped:
            return
        emu = self.emu
        state = te.capture_snapshot(emu)
        self.writer.submit(te.write_snapshot, state, path or self.store.autosave)
        if path is None:                                # also refresh the flash fallback
            flash = state['ram'][:te.SpiFlash.SIZE]
            self.writer.submit(te.write_save_data, flash, te.save_meta(emu, clean=False),
                               self.store.flash)
        self.next_autosave = time.time() + self.settings['autosave_minutes'] * 60
        self.say('%s at %s' % (label, time.strftime('%H:%M')))

    def save_slot(self, n):
        self.save_now(self.store.slot(n), 'Saved to slot %d' % n)

    def load_slot(self, n):
        when = self.store.slot_time(n)
        if not when or not messagebox.askyesno(
                'Load slot %d' % n, 'Load the save from %s?\n\nThe game you are playing now is '
                'autosaved first.' % time.strftime('%d %b %Y %H:%M', time.localtime(when)),
                parent=self.root):
            return
        self.save_now(label='Autosaved')
        self.writer.wait()
        emu = te.Emulator(self.image, False, None)
        try:
            te.load_snapshot(emu, self.store.slot(n))
        except te.SaveMismatch as e:
            messagebox.showerror('Save does not match', str(e), parent=self.root)
            return
        self.set_emu(emu)
        self.say('Loaded slot %d' % n)

    def shutdown_game(self):
        """Store the current game before closing it (window closed, other ROM)."""
        self.say('Saving…', 10)
        if self.force_sleep():
            te.write_save(self.emu, self.store.flash, clean=True, asleep=True)
        else:                                           # no sleep: snapshot it instead
            self.save_now(label='Saved')
        self.writer.wait()
        for e in self.writer.errors:
            print('save error: %s' % e)

    def close(self):
        if self.stopped != 'closed':
            if self.emu and self.stopped is None:
                self.shutdown_game()
            self.stopped = 'closed'
        if self.audio:
            self.audio.close()
        ts.store_settings(self.settings)
        self.root.destroy()

    # --- input -----------------------------------------------------------

    def on_key_down(self, event):
        if event.state & 0x4:                           # Ctrl held: shortcuts only
            return
        if event.keysym.lower() == 'm':
            self.var['muted'].set(not self.var['muted'].get())
            self.changed('muted')
            return
        key = KEYMAP.get(event.keysym) or KEYMAP.get(event.keysym.lower())
        if key:
            self.press(key)

    def on_key_up(self, event):
        key = KEYMAP.get(event.keysym) or KEYMAP.get(event.keysym.lower())
        if key:
            self.release(key)

    def press(self, key):
        if key in self.down or not self.emu:            # keyboard auto-repeat
            return
        self.down.add(key)
        now_wall = time.perf_counter()
        if now_wall - self.rewind_at >= REWIND_GAP and not self.emu.periph.powered_off:
            # the state just before this press: if the press starts Bluetooth,
            # the game is put back here (see connection_blocked)
            self.rewind_point, self.rewind_at = te.capture_snapshot(self.emu), now_wall
        p = self.emu.periph
        p.live_keys.add(key)
        now = self.emu.executed / te.CPU_HZ
        # a quick tap stays held for at least MIN_HOLD, long enough for the
        # firmware's 44 ms debounce; no longer, or fast taps in the mini
        # games (8 a second) run together into one long press
        p.hold_until[key] = now + MIN_HOLD
        if p.powered_off:                               # a scripted press wakes it from sleep
            p.key_script[:] = [(t, k) for t, k in p.key_script if t > now - 1.0]
            p.key_script.append((now, key))
        self.pad.itemconfigure(self.circles[key], fill=BUTTON_DOWN)

    def release(self, key):
        self.down.discard(key)
        if self.emu:
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
        if not self.emu:
            self.last_wall = wall
            self.root.after(50, self.tick)
            return
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
            p = emu.periph
            now = emu.executed / te.CPU_HZ
            if emu.never_sleep and not any(t >= now for t, _ in p.key_script):
                # it went to sleep anyway (e.g. another sleep path): wake it
                p.key_script.append((now, 'B'))
            woken = te.power_cycle(emu, target)
            if woken is None:                               # no alarm: sleep until a button
                p.game_time += (target - emu.executed) / te.CPU_HZ * p.turbo
                emu.executed = target
                p.now = target / te.CPU_HZ
            elif woken is not emu:
                self.emu = woken
                self.hook_lcd()
                self.frame_dirty = True
        else:
            outcome = emu.run(target)
            if outcome in ('bluetooth', 'infrared'):
                self.connection_blocked(outcome)
                self.root.after(TICK_MS, self.tick)
                return
            if outcome == 'ble_fail':
                self.bluetooth_hang()
                self.root.after(TICK_MS, self.tick)
                return
            if outcome in ('crash', 'stuck'):
                self.stopped = outcome
                self.status.configure(text='Emulator stopped (%s), see the console' % outcome)
                return

        self.play_sound(t_start, self.emu.executed / te.CPU_HZ)
        asleep = self.emu.periph.powered_off
        if asleep and not self.was_asleep:              # the firmware saved before sleeping
            flash = bytes(self.emu.uc.mem_read(te.FLASH_BASE, te.SpiFlash.SIZE))
            self.writer.submit(te.write_save_data, flash,
                               te.save_meta(self.emu, clean=True, asleep=True), self.store.flash)
        self.was_asleep = asleep
        minutes = self.settings['autosave_minutes']
        if minutes and not asleep and time.time() >= self.next_autosave:
            self.save_now(label='Autosaved')

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
        if self.settings['muted']:
            out = array.array('h', bytes(len(out) * 2))
        self.pending.extend(out)
        if len(self.pending) >= AUDIO_CHUNK * AUDIO_RATE or self.audio.queued() < low // 2:
            self.audio.write(self.pending)
            self.pending = array.array('h')

    def update_status(self, wall):
        if self.message and time.time() < self.message[1]:
            return
        self.message = None
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


def describe(seconds):
    """'3 days', '5 hours', '12 minutes' ..."""
    for unit, size in (('day', 86400), ('hour', 3600), ('minute', 60)):
        if seconds >= size:
            n = int(seconds // size)
            return '%d %s%s' % (n, unit, 's' if n > 1 else '')
    return '%d seconds' % seconds


def main():
    rom = sys.argv[1] if len(sys.argv) > 1 else None
    if rom in ('-h', '--help'):
        print(__doc__)
        return
    Window(ts.load_settings(), rom).root.mainloop()


if __name__ == '__main__':
    main()
