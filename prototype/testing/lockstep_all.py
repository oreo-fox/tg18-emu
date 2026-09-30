"""Record and replay lockstep traces for many situations (see trace.py).

    python -u lockstep_all.py WORKDIR [NAME ...]  > log

Each case: record with the prototype, replay on the Rust core
(target/release/lockstep.exe), print OK or the first difference. The trace
files of passing cases are deleted.
"""
import os
import shutil
import subprocess
import sys
import time

HERE = os.path.dirname(os.path.abspath(__file__))
ROOT = os.path.abspath(os.path.join(HERE, '..', '..'))
DUMPS = r'C:\Users\Anna\Downloads\tamagotchi-meets-on-spi-flash-dumps\tg18_fw_dump'
ROMS = {
    'fairy': 'fw_tg18_en_v057_2019-05-13_fairy.bin',
    'magic': 'fw_tg18_en_v058_2019-05-13_magic.bin',
    'wg': 'fw_tg18_en_v063_2020-02-13_wondergarden.bin',
    'jfairy': 'fw_tg18_jp_v030_2018-10-19_fairy.bin',
    'jmagic': 'fw_tg18_jp_v031_2018-10-19_magic.bin',
    'jfantasy': 'fw_tg18_jp_v047_2019-05-28_fantasy.bin',
    'jpastel': 'fw_tg18_jp_v055_2019-03-10_pastel.bin',
    'jsanrio': 'fw_tg18_jp_v056_2019-04-24_sanrio.bin',
    'jsweets': 'fw_tg18_jp_v062_2019-09-12_sweets.bin',
}
F = 'fairy'
# name: (rom, snapshot or None, warmup seconds, instructions, presses)
CASES = {
    'boot_' + r: (r, None, 0, 2_000_000, []) for r in ROMS
}
CASES.update({
    'boot_wg_3s': ('wg', None, 3.0, 2_000_000, []),
    'boot_wg_format': ('wg', None, 6.0, 2_000_000, []),
    'boot_jfairy_4s': ('jfairy', None, 4.0, 2_000_000, []),
    'boot_jsanrio_5s': ('jsanrio', None, 5.0, 2_000_000, []),
    'name_screen': (F, 'fairy/f01_clock_set_name_screen.snap', 0.5, 2_000_000, ['0.2:B']),
    'egg': (F, 'fairy/f03_egg.snap', 1.0, 2_000_000, []),
    'baby_meal': (F, 'fairy/g01_baby_hatched.snap', 3.0, 2_000_000, ['0.3:A', '1.3:A', '2.3:B']),
    'child_room': (F, 'fairy/c04_child_cared.snap', 1.0, 2_000_000, []),
    'surfing': (F, 'fairy/t03.snap', 5.0, 2_000_000, ['0.3:B'] + ['%.3f:A' % (3.0 + i / 8) for i in range(16)]),
    'after_surf': (F, 'fairy/s02.snap', 1.0, 2_000_000, []),
    'resort': (F, 'fairy/h01.snap', 2.0, 2_000_000, ['0.5:B']),
    'hotel_home': (F, 'fairy/h05.snap', 1.0, 2_000_000, []),
    'adult_menu': (F, 'fairy/grow3/stage1.snap', 2.5, 2_000_000, ['0.5:A', '1.5:A', '2.0:A']),
    'wedding': (F, 'fairy/wed/p14.snap', 5.0, 2_000_000, []),
    'gen2_name': (F, 'fairy/wed/p15.snap', 1.0, 2_000_000, ['0.3:B']),
    'grim': (F, 'fairy/grim/m39.snap', 1.0, 2_000_000, []),
    'grave': (F, 'fairy/grim/end.snap', 1.0, 2_000_000, []),
    'new_egg': (F, 'fairy/grim/ac.snap', 1.0, 2_000_000, []),
    'sick': (F, 'fairy/sick4/sick10.snap', 1.0, 2_000_000, []),
    'clock_screen': ('wg', r'..\..\saves\wg_baby.snap', 1.0, 2_000_000, ['0.3:B']),
})


def main():
    work = sys.argv[1]
    names = sys.argv[2:] or list(CASES)
    lockstep = os.path.join(ROOT, 'target', 'release', 'lockstep.exe')
    failed = []
    for name in names:
        rom, snap, warmup, insns, presses = CASES[name]
        rom_path = os.path.join(DUMPS, ROMS[rom])
        out = os.path.join(work, name)
        cmd = [sys.executable, os.path.join(HERE, 'trace.py'), rom_path, out,
               '--warmup', str(warmup), '--insns', str(insns)]
        if snap:
            if not os.path.exists(os.path.join(HERE, snap)):
                print('%-16s SKIPPED (no %s)' % (name, snap))
                continue
            cmd += ['--snap', os.path.join(HERE, snap)]
        for p in presses:
            cmd += ['--press', p]
        t = time.time()
        rec = subprocess.run(cmd, cwd=HERE, capture_output=True, text=True, timeout=3600)
        if rec.returncode:
            print('%-16s RECORD FAILED\n%s' % (name, rec.stderr[-2000:]))
            failed.append(name)
            continue
        rep = subprocess.run([lockstep, rom_path, out], capture_output=True, text=True, timeout=3600)
        last = rep.stdout.strip().splitlines()[-1] if rep.stdout.strip() else rep.stderr
        if rep.returncode == 0:
            print('%-16s %s  (%.0f s)' % (name, last, time.time() - t))
            shutil.rmtree(out, ignore_errors=True)
        else:
            print('%-16s FAILED\n%s' % (name, rep.stdout[-3000:] + rep.stderr[-1000:]))
            failed.append(name)
    print('\n%d of %d cases passed%s' % (len(names) - len(failed), len(names),
                                         '; failed: ' + ' '.join(failed) if failed else ''))


if __name__ == '__main__':
    main()
