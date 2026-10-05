"""Build the folder to hand out: tg18-emu-win with the program, a short
guide, the license, the notices of the libraries inside the program and
empty roms and saves folders.

    python tools/make_release.py [WHERE]

WHERE is the folder to put tg18-emu-win in (default: dist/ in the project,
which git ignores). An existing tg18-emu-win is updated: the program and
texts are replaced, nothing in roms or saves is ever deleted, but a warning
is printed if they hold anything besides their note (dumps and saves must
not be handed out).
"""
import hashlib
import json
import os
import re
import shutil
import subprocess
import sys

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
NAME = 'tg18-emu-win'


def crlf(src, dst):
    """Copy a text file with Windows line endings (for Notepad)."""
    text = open(src, encoding='utf-8').read().replace('\r\n', '\n')
    with open(dst, 'w', encoding='utf-8', newline='\r\n') as f:
        f.write(text)


def third_party_notices(env):
    """Licence notices of the libraries built into tg18.exe (build-time
    tools like serde_derive are not in it). Each is used under its MIT
    licence, so the notice is its MIT text with the copyright holders."""
    tree = subprocess.run(['cargo', 'tree', '-p', 'tg18-desktop', '-e', 'normal,no-proc-macro', '--prefix', 'none',
                           '--format', '{p}'], cwd=ROOT, env=env, check=True, capture_output=True, text=True).stdout
    # lines like "serde v1.0.229"; our own packages are skipped below
    used = {(l.split()[0], l.split()[1].lstrip('v')) for l in tree.splitlines() if len(l.split()) >= 2}
    meta = json.loads(subprocess.run(['cargo', 'metadata', '--format-version', '1'], cwd=ROOT, env=env, check=True,
                                     capture_output=True, text=True).stdout)
    out = ['Third-party software in tg18.exe',
           '================================',
           '',
           'tg18.exe contains these open-source libraries. Each is used under its',
           'MIT licence; their notices follow. The Rust standard library, also part',
           'of the program, is described in rust-std-licenses.html.',
           '']
    for p in sorted(meta['packages'], key=lambda p: p['name']):
        if p['source'] is None or (p['name'], p['version']) not in used:
            continue
        folder = os.path.dirname(p['manifest_path'])
        mit = [f for f in sorted(os.listdir(folder)) if f.upper().startswith('LICENSE-MIT')]
        if not mit:
            raise SystemExit('no MIT licence file in %s' % folder)
        text = open(os.path.join(folder, mit[0]), encoding='utf-8').read().strip()
        out += ['-' * 72, '%s %s  (%s)' % (p['name'], p['version'], p['license']),
                p.get('repository') or '', '']
        # some MIT files have no "Copyright ..." line: name the authors
        if not re.search(r'^\s*copyright', text, re.I | re.M):
            authors = ', '.join(a.split(' <')[0] for a in p['authors']) or 'the %s authors' % p['name']
            out += ['Copyright (c) %s' % authors, '']
        out += [text, '']
    return '\n'.join(out) + '\n'


def main():
    where = os.path.abspath(sys.argv[1]) if len(sys.argv) > 1 else os.path.join(ROOT, 'dist')
    dest = os.path.join(where, NAME)

    # a lean build (no debug data) in its own build folder, so the everyday
    # build in target/release is left alone
    env = dict(os.environ)
    env['PATH'] = os.path.join(os.path.expanduser('~'), '.cargo', 'bin') + os.pathsep + env.get('PATH', '')
    env['CARGO_PROFILE_RELEASE_DEBUG'] = '0'
    env['CARGO_PROFILE_RELEASE_STRIP'] = 'symbols'
    # source paths end up in the exe (for error messages); write them
    # without the builder's user folder, e.g. ~\.cargo\... instead of
    # C:\Users\<name>\.cargo\... (the last matching rule wins)
    home = os.path.expanduser('~')
    env['CARGO_ENCODED_RUSTFLAGS'] = '\x1f'.join([
        '--remap-path-prefix=%s=~' % home,
        '--remap-path-prefix=%s=tg18-emu' % ROOT,
    ])
    target = os.path.join(ROOT, 'target', 'dist')
    print('building ...')
    subprocess.run(['cargo', 'build', '--release', '-p', 'tg18-desktop', '--target-dir', target],
                   cwd=ROOT, env=env, check=True)
    exe = os.path.join(target, 'release', 'tg18.exe')

    for sub in ('', 'roms', 'saves'):
        os.makedirs(os.path.join(dest, sub), exist_ok=True)
    shutil.copy2(exe, os.path.join(dest, 'tg18.exe'))
    # the exe's fingerprint, for players to check their copy (also to paste
    # into the release notes); the usual "hash *file" checksum format
    digest = hashlib.sha256(open(exe, 'rb').read()).hexdigest().upper()
    with open(os.path.join(dest, 'SHA256.txt'), 'w', encoding='ascii', newline='\r\n') as f:
        f.write('%s *tg18.exe\n' % digest)
    crlf(os.path.join(ROOT, 'tools', 'release', 'README.txt'), os.path.join(dest, 'README.txt'))
    crlf(os.path.join(ROOT, 'LICENSE'), os.path.join(dest, 'LICENSE.txt'))
    # notices of the libraries inside the exe, and the Rust standard
    # library's own notice file (shipped with Rust for this purpose)
    with open(os.path.join(dest, 'THIRD-PARTY-LICENSES.txt'), 'w', encoding='utf-8', newline='\r\n') as f:
        f.write(third_party_notices(env))
    sysroot = subprocess.run(['rustc', '--print', 'sysroot'], env=env, check=True, capture_output=True,
                             text=True).stdout.strip()
    shutil.copy2(os.path.join(sysroot, 'share', 'doc', 'rust', 'COPYRIGHT-library.html'),
                 os.path.join(dest, 'rust-std-licenses.html'))
    for sub in ('roms', 'saves'):
        crlf(os.path.join(ROOT, sub, 'README.txt'), os.path.join(dest, sub, 'README.txt'))

    # nothing personal may go out with it
    extra = []
    for dirpath, _, files in os.walk(dest):
        for f in files:
            rel = os.path.relpath(os.path.join(dirpath, f), dest)
            if rel not in ('tg18.exe', 'SHA256.txt', 'README.txt', 'LICENSE.txt', 'THIRD-PARTY-LICENSES.txt',
                           'rust-std-licenses.html', os.path.join('roms', 'README.txt'),
                           os.path.join('saves', 'README.txt')):
                extra.append(rel)
    print('\n%s ready:' % dest)
    for dirpath, _, files in sorted(os.walk(dest)):
        for f in sorted(files):
            p = os.path.join(dirpath, f)
            print('  %-24s %9d bytes' % (os.path.relpath(p, dest), os.path.getsize(p)))
    print('\ntg18.exe SHA-256: %s' % digest)
    if extra:
        print('\nWARNING: not part of a clean release (dumps, saves, settings?): ' + ', '.join(extra))
        sys.exit(1)


if __name__ == '__main__':
    main()
