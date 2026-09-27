"""Settings, ROM folder and per-ROM saves for the tg18 window.

Everything lives next to the scripts: settings.json, and saves/<rom name>/
with one folder per ROM:

  game.flash (+ .json)   the toy's own flash save and clock; written when the
                         window closes (after the device has saved itself by
                         going to sleep) and as a fallback with each autosave
  autosave.snap          snapshot of the whole machine, every few minutes
  slot1-3.snap           manual save slots

Snapshots are Python pickles: only load ones you made yourself.
"""
import glob
import json
import os
import queue
import re
import shutil
import threading
import time

import tg18emu as te

APP_DIR = os.path.dirname(os.path.abspath(__file__))
SETTINGS_PATH = os.path.join(APP_DIR, 'settings.json')
SAVES_DIR = os.path.join(APP_DIR, 'saves')
SLOTS = 3

DEFAULTS = {
    'rom_dir': None,
    'last_rom': None,             # file name inside rom_dir
    'volume': 60,
    'muted': False,
    'scale': 4,
    'autosave_minutes': 1,        # 0 = off
    'never_sleep': True,
    'pause_time_when_closed': False,
}


def load_settings():
    settings = dict(DEFAULTS)
    try:
        with open(SETTINGS_PATH) as f:
            settings.update(json.load(f))
    except (OSError, ValueError):
        pass
    return settings


def store_settings(settings):
    tmp = SETTINGS_PATH + '.tmp'
    with open(tmp, 'w') as f:
        json.dump(settings, f, indent=1)
    os.replace(tmp, SETTINGS_PATH)


def is_rom(path):
    try:
        with open(path, 'rb') as f:
            return os.path.getsize(path) == te.SpiFlash.SIZE and f.read(4) == b'SPII'
    except OSError:
        return False


def list_roms(folder):
    """(file name, display name) of every tg18 image in folder, sorted by name."""
    if not folder or not os.path.isdir(folder):
        return []
    roms = [os.path.basename(p) for p in glob.glob(os.path.join(folder, '*.bin')) if is_rom(p)]
    return sorted(((f, display_name(f)) for f in roms), key=lambda r: r[1].lower())


NICE_NAMES = {'wondergarden': 'Wonder Garden'}


def display_name(filename):
    """'fw_tg18_en_v063_2020-02-13_wondergarden.bin' -> 'Wonder Garden (EN v063)'."""
    stem = os.path.splitext(filename)[0]
    m = re.match(r'fw_tg18_([a-z]+)_v(\d+)_[\d-]+_(.+)$', stem, re.I)
    if not m:
        return stem
    name = NICE_NAMES.get(m.group(3).lower(), m.group(3).replace('_', ' ').title())
    return '%s (%s v%s)' % (name, m.group(1).upper(), m.group(2))


class SaveStore:
    """The save files of one ROM."""

    def __init__(self, rom_path):
        self.name = os.path.splitext(os.path.basename(rom_path))[0]
        self.dir = os.path.join(SAVES_DIR, self.name)
        os.makedirs(self.dir, exist_ok=True)
        self.flash = os.path.join(self.dir, 'game.flash')
        self.autosave = os.path.join(self.dir, 'autosave.snap')

    def slot(self, n):
        return os.path.join(self.dir, 'slot%d.snap' % n)

    def slot_time(self, n):
        """When slot n was saved, or None if it's empty."""
        path = self.slot(n)
        return os.path.getmtime(path) if os.path.exists(path) else None

    def import_legacy(self, image):
        """Adopt a save made before per-ROM folders (saves/*.flash) if it fits."""
        if os.path.exists(self.flash) or os.path.exists(self.autosave):
            return None
        for path in sorted(glob.glob(os.path.join(SAVES_DIR, '*.flash'))):
            if te.save_matches(path, image):
                shutil.copyfile(path, self.flash)
                if os.path.exists(path + '.json'):
                    shutil.copyfile(path + '.json', self.flash + '.json')
                return path
        return None

    def newest(self):
        """Which state to resume: ('flash', meta), ('snapshot', None) or (None, None).

        A clean flash save (written after the device went to sleep on close)
        and the autosave snapshot compete by age; a flash save written while
        the device was awake is only used if there's nothing else, as the
        game may not have stored its latest state in flash yet.
        """
        meta = te.read_save_meta(self.flash) if os.path.exists(self.flash) else None
        snap = os.path.getmtime(self.autosave) if os.path.exists(self.autosave) else None
        clean = meta is not None and meta.get('clean')
        if clean and (snap is None or meta['saved_at'] >= snap):
            return 'flash', meta
        if snap is not None:
            return 'snapshot', None
        if meta is not None or os.path.exists(self.flash):
            return 'flash', meta
        return None, None


class Writer:
    """Writes saves on a background thread, one at a time, in order."""

    def __init__(self):
        self.jobs = queue.Queue()
        self.errors = []
        threading.Thread(target=self._run, daemon=True).start()

    def submit(self, fn, *args):
        self.jobs.put((fn, args))

    @property
    def busy(self):
        return self.jobs.unfinished_tasks

    def _run(self):
        while True:
            fn, args = self.jobs.get()
            try:
                fn(*args)
            except Exception as e:          # reported by the window, never fatal
                self.errors.append(str(e))
            finally:
                self.jobs.task_done()

    def wait(self):
        self.jobs.join()
