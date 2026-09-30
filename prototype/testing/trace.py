"""Record a lockstep trace with the prototype (Unicorn) for the Rust core.

    python trace.py ROM OUTDIR [--snap IN.snap] [--date "YYYY-MM-DD HH:MM"]
                    [--warmup S] [--insns N] [--press T:KEY ...]

Press times count from the start, warmup included.

Runs the prototype (from a fresh boot or a snapshot), optionally for
--warmup emulated seconds first, then records N instructions:

  OUTDIR/start.t18s  the machine at the start of the trace (Rust format)
  OUTDIR/regs.bin    zlib stream; per instruction 18 x u64: r0-r15, CPSR
                     before it runs, and the address Unicorn reported
  OUTDIR/events.bin  zlib stream of records <Q index, B kind, ...>: things
                     done to the machine outside of instructions, and the
                     values hardware reads returned:
                       1 IRQ entry
                       2 register write   <B reg (0-15, 16 = CPSR), I value>
                       3 memory write     <I addr, I length, bytes>
                       4 hardware read    <I addr, B size, I value>

`cargo run --release --bin lockstep -- ROM OUTDIR` replays it on the Rust
core and compares the registers after every instruction.
"""
import argparse
import datetime
import os
import struct
import sys
import zlib

sys.path.insert(0, os.path.join(os.path.dirname(os.path.abspath(__file__)), '..'))
import tg18emu as te
from unicorn import UC_HOOK_CODE, arm_const
from to_rust_snap import write_t18s

te.print = lambda *a, **k: None

REGS = [getattr(arm_const, 'UC_ARM_REG_R%d' % i) for i in range(13)] + [
    arm_const.UC_ARM_REG_SP, arm_const.UC_ARM_REG_LR, arm_const.UC_ARM_REG_PC,
    arm_const.UC_ARM_REG_CPSR]
REG_INDEX = {r: i for i, r in enumerate(REGS)}
REG_INDEX[arm_const.UC_ARM_REG_R13] = 13
REG_INDEX[arm_const.UC_ARM_REG_R14] = 14
REG_INDEX[arm_const.UC_ARM_REG_R15] = 15

IRQ, REG, MEM, MMIO = 1, 2, 3, 4


class Tracer:
    def __init__(self, outdir):
        self.on = False
        self.count = 0
        self.limit = 0
        self.last = None
        self.events_since = False
        self.suppress = False
        self.regs_out = open(os.path.join(outdir, 'regs.bin'), 'wb')
        self.ev_out = open(os.path.join(outdir, 'events.bin'), 'wb')
        self.rz = zlib.compressobj(1)
        self.ez = zlib.compressobj(6)
        self.buf = bytearray()
        self.ev = bytearray()

    def event(self, kind, payload):
        if self.on and not self.suppress:
            self.ev += struct.pack('<QB', self.count, kind) + payload
            self.events_since = True

    def flush(self, final=False):
        self.regs_out.write(self.rz.compress(bytes(self.buf)))
        self.ev_out.write(self.ez.compress(bytes(self.ev)))
        self.buf.clear()
        self.ev.clear()
        if final:
            self.regs_out.write(self.rz.flush())
            self.ev_out.write(self.ez.flush())
            self.regs_out.close()
            self.ev_out.close()


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument('rom')
    ap.add_argument('outdir')
    ap.add_argument('--snap')
    ap.add_argument('--date', default='2018-01-01 12:00')
    ap.add_argument('--warmup', type=float, default=0.0)
    ap.add_argument('--insns', type=int, default=2_000_000)
    ap.add_argument('--press', action='append', default=[])
    a = ap.parse_args()
    os.makedirs(a.outdir, exist_ok=True)
    tr = Tracer(a.outdir)

    # hardware reads: wrap the MMIO callbacks before the machine is built
    orig_read = te.Peripherals.read

    def read(self, base):
        cb = orig_read(self, base)

        def wrapped(uc, off, size, data):
            v = cb(uc, off, size, data)
            tr.event(MMIO, struct.pack('<IBI', (base + off) & 0xFFFFFFFF, size, v & 0xFFFFFFFF))
            return v
        return wrapped
    te.Peripherals.read = read

    image = open(a.rom, 'rb').read()
    start = datetime.datetime.strptime(a.date, '%Y-%m-%d %H:%M')
    emu = te.Emulator(image, False, start)
    if a.snap:
        te.load_snapshot(emu, a.snap)
    t_start = emu.executed / te.CPU_HZ
    for spec in a.press:                       # times from the start, warmup included
        t, key = spec.split(':')
        emu.periph.key_script.append((t_start + float(t), key.upper()))
    if a.warmup:
        end = emu.executed + int(a.warmup * te.CPU_HZ)
        while emu.executed < end:
            out = emu.run(end)
            if out == 'poweroff':
                emu = te.power_cycle(emu, end)
            elif out != 'limit':
                sys.exit('warmup stopped: %s' % out)
    emu.never_sleep = False                    # its clock refresh swaps CPU contexts
    t0 = emu.executed / te.CPU_HZ
    write_t18s(emu, image, os.path.join(a.outdir, 'start.t18s'))

    uc = emu.uc
    orig_reg_write, orig_mem_write = uc.reg_write, uc.mem_write

    def reg_write(reg, value):
        if reg in REG_INDEX:
            tr.event(REG, struct.pack('<BI', REG_INDEX[reg], value & 0xFFFFFFFF))
        return orig_reg_write(reg, value)

    def mem_write(addr, data):
        data = bytes(data)
        tr.event(MEM, struct.pack('<II', addr & 0xFFFFFFFF, len(data)) + data)
        return orig_mem_write(addr, data)
    uc.reg_write, uc.mem_write = reg_write, mem_write

    orig_irq = emu.enter_irq

    def enter_irq():
        tr.event(IRQ, b'')
        tr.suppress = True
        try:
            orig_irq()
        finally:
            tr.suppress = False
    emu.enter_irq = enter_irq

    reader = te.RegReader(uc, REGS)

    def on_insn(uc, addr, size, _):
        if not tr.on:
            return
        rec = reader.read() + struct.pack('<Q', addr)
        if rec == tr.last and not tr.events_since:
            return                              # stopped before it ran, now resumed
        if len(tr.buf) >= 144 * 50_000:
            tr.flush()                          # (keeps the newest entry in buf, see emu_start)
        tr.last = rec
        tr.events_since = False
        tr.buf += rec
        tr.count += 1
        if tr.count >= tr.limit:
            tr.on = False
            emu.periph.powered_off = True       # makes run() return after this slice
            uc.emu_stop()
    uc.hook_add(UC_HOOK_CODE, on_insn)

    orig_start = uc.emu_start

    def emu_start(begin, until, timeout=0, count=0):
        # Unicorn calls the code hook for the instruction at which a count
        # limit or emu_stop() ends the run, without running it: drop it
        try:
            return orig_start(begin, until, timeout, count)
        finally:
            if tr.buf and tr.count:
                last_pc = struct.unpack_from('<Q', tr.buf, len(tr.buf) - 144 + 15 * 8)[0]
                if (last_pc & ~1) == (uc.reg_read(arm_const.UC_ARM_REG_PC) & ~1) and not tr.events_since:
                    del tr.buf[-144:]
                    tr.count -= 1
                    tr.last = None
    uc.emu_start = emu_start

    tr.limit = a.insns
    tr.on = True
    end = emu.executed + int(3600 * te.CPU_HZ)
    out = emu.run(end)
    tr.on = False
    tr.flush(final=True)
    print('traced %d instructions (%s), emulated %.3f s' % (tr.count, out, emu.executed / te.CPU_HZ - t0))


if __name__ == '__main__':
    main()
