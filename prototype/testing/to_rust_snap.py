"""Convert a prototype snapshot (.snap, Python pickle) to the Rust format (.t18s).

    python to_rust_snap.py ROM IN.snap OUT.t18s

The format is described in core/src/snapshot.rs.
"""
import struct
import sys
import zlib

sys.path.insert(0, r'C:\Users\Anna\tg18-emu\prototype')
import tg18emu as te
from unicorn.arm_const import (UC_ARM_REG_CPSR, UC_ARM_REG_SPSR, UC_ARM_REG_R8, UC_ARM_REG_R9,
                               UC_ARM_REG_R10, UC_ARM_REG_R11, UC_ARM_REG_R12, UC_ARM_REG_R13,
                               UC_ARM_REG_R14)
from unicorn import arm_const

te.print = lambda *a, **k: None

MODES = [0x10, 0x11, 0x12, 0x13, 0x17, 0x1B]          # bank order of the Rust Cpu
R = [getattr(arm_const, 'UC_ARM_REG_R%d' % i) for i in range(13)] + [
    UC_ARM_REG_R13, UC_ARM_REG_R14, arm_const.UC_ARM_REG_PC]


def code_hash(image):
    h = 0xCBF29CE484222325
    for b in image[:te.CODE_CHECK_LEN]:
        h = ((h ^ b) * 0x100000001B3) & 0xFFFFFFFFFFFFFFFF
    return h


def cpu_state(uc):
    """The 45 words of Cpu::state(): r0-r15, CPSR, r13/r14 per bank, user
    r8-r12, FIQ r8-r12, SPSR per bank."""
    cpsr = uc.reg_read(UC_ARM_REG_CPSR)
    r = [uc.reg_read(x) for x in R]
    thumb = cpsr & 0x20
    r[15] = r[15] & ~1                                  # the PC is kept without the Thumb bit
    banked, spsr = [], []
    fiq_r8_12 = usr_r8_12 = None
    for mode in MODES:
        uc.reg_write(UC_ARM_REG_CPSR, (cpsr & ~0x1F) | mode)
        banked += [uc.reg_read(UC_ARM_REG_R13), uc.reg_read(UC_ARM_REG_R14)]
        spsr.append(uc.reg_read(UC_ARM_REG_SPSR) if mode != 0x10 else 0)
        hi = [uc.reg_read(x) for x in (UC_ARM_REG_R8, UC_ARM_REG_R9, UC_ARM_REG_R10,
                                       UC_ARM_REG_R11, UC_ARM_REG_R12)]
        if mode == 0x11:
            fiq_r8_12 = hi
        elif mode == 0x10:
            usr_r8_12 = hi
    uc.reg_write(UC_ARM_REG_CPSR, cpsr)
    assert (uc.reg_read(UC_ARM_REG_CPSR) & 0x3F) == (cpsr & 0x3F)
    return r + [cpsr] + banked + usr_r8_12 + fiq_r8_12 + spsr


def convert(rom, src, dst):
    image = open(rom, 'rb').read()
    emu = te.Emulator(image, False, None)
    state = te.load_snapshot(emu, src)
    write_t18s(emu, image, dst, state.get('saved_at', 0.0))


def write_t18s(emu, image, dst, saved_at=0.0):
    """Write the live prototype machine emu in the Rust snapshot format."""
    p = emu.periph
    chunks = []
    add = lambda name, data: chunks.append((name, data))
    add('info', struct.pack('<Qd', code_hash(image), saved_at))
    add('cpu', struct.pack('<45I', *cpu_state(emu.uc)))
    add('ram', bytes(emu.uc.mem_read(te.FLASH_BASE, te.RAM_SIZE)))
    add('sram', bytes(emu.uc.mem_read(te.SRAM_BASE, te.SRAM_SIZE)))
    add('gpio', bytes(emu.uc.mem_read(*te.GPIO_PAGE)))
    if emu.uart_is_ram:
        add('uart', bytes(emu.uc.mem_read(*te.UART_PAGE)))
    known = [(te.FLASH_BASE, te.RAM_SIZE), (te.SRAM_BASE, te.SRAM_SIZE), te.GPIO_PAGE, te.UART_PAGE]
    known += te.MMIO_REGIONS
    extra = b''
    for begin, end, _ in emu.uc.mem_regions():
        if any(b <= begin < b + s for b, s in known):
            continue
        for page in range(begin, end + 1, 0x10000):
            extra += struct.pack('<I', page) + bytes(emu.uc.mem_read(page, 0x10000))
    add('extra', extra)
    add('regs', b''.join(struct.pack('<II', a & 0xFFFFFFFF, v & 0xFFFFFFFF)
                         for a, v in sorted(p.regs.items())))
    add('machine', struct.pack('<QQIBBB', emu.executed, emu.irqs,
                               (emu.rand_state + te.SRAM_BASE) if emu.rand_state is not None else 1,
                               emu.bluetooth_used, emu.infrared_used, p.on_infrared is not None))
    add('time', struct.pack('<ddB8I', p.now, p.game_time, p.powered_off, *p.gpio_applied))
    add('timers', b''.join(struct.pack('<dQB', t.start, t.periods_seen, t.pending) for t in p.timers))
    l = p.lcd
    none = 0xFFFFFFFF
    add('lcd', bytes(l.fb) + struct.pack('<II', none if l.cmd is None else l.cmd, len(l.args))
        + bytes(l.args) + struct.pack('<6IIQQ', l.x0, l.x1, l.y0, l.y1, l.x, l.y,
                                      none if l.half is None else l.half, l.frames, l.last_vsync))
    regs = bytearray(256)
    for k, v in p.rtc.regs.items():
        regs[k] = v
    add('rtc', bytes(regs) + struct.pack('<qdI', p.rtc.base_ticks, p.rtc.base_time, p.rtc.data_out))
    add('rtcirq', struct.pack('<qqd', p.rtc_irq.fired[2], int(p.rtc_irq.tod_base), p.rtc_irq.tod_set_at))
    add('adc', struct.pack('<B', p.adc.converting))
    f = emu.flash
    add('flash', struct.pack('<BI', f.cs, len(f.buf)) + bytes(f.buf) + struct.pack('<IBQ', f.rx, f.wel, f.writes))
    payload = b''.join(struct.pack('<B', len(n)) + n.encode() + struct.pack('<I', len(d)) + d
                       for n, d in chunks)
    with open(dst, 'wb') as out:
        out.write(b'T18SNAP1' + zlib.compress(payload, 3))


if __name__ == '__main__':
    convert(*sys.argv[1:4])
