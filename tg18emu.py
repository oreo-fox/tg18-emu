"""Feasibility harness for Tamagotchi Meets/On (tg18) SPI flash images.

Boots a flash image on Unicorn's ARM core, stubs every peripheral as a plain
register file, prints the firmware's (compiled-out) printf output, and reports
which hardware registers were touched and where execution got stuck.

Usage: python tg18emu.py <flash.bin> [--insns N] [--trace-mmio]
"""
import argparse
import collections
import re
import struct
import sys

from unicorn import (Uc, UcError, UC_ARCH_ARM, UC_MODE_ARM, UC_HOOK_CODE,
                     UC_HOOK_MEM_UNMAPPED, UC_PROT_ALL)
from unicorn import arm_const
from unicorn.arm_const import (UC_ARM_REG_PC, UC_ARM_REG_SP, UC_ARM_REG_LR,
                               UC_ARM_REG_R0, UC_ARM_REG_R1, UC_ARM_REG_R2,
                               UC_ARM_REG_R3, UC_ARM_REG_CPSR, UC_ARM_REG_SPSR,
                               UC_CPU_ARM_926)

# Memory map, from the scatter-loader at the reset vector
FLASH_BASE = 0x20000000       # image executes from here (external RAM / XIP)
RAM_SIZE = 0x01000000         # 16 MiB covers the image plus 0x20Fxxxxx refs
SRAM_BASE = 0xF8000000        # internal SRAM: .data, .bss, stacks
SRAM_SIZE = 0x00100000
# The GPIO page is plain memory, not MMIO: the firmware polls it about a
# million times a second, and leaving the CPU core for each read dominated
# the run time. Button bits are written into it when they change.
GPIO_PAGE = (0xC0000000, 0x1000)
MMIO_REGIONS = [(0xC0001000, 0x00FFF000),
                (0xD0000000, 0x01000000),
                (0xFF000000, 0x00FFF000)]
RESET_PC = 0x20000048
IDLE_REGS = [getattr(arm_const, 'UC_ARM_REG_R%d' % i) for i in range(13)] + [
    arm_const.UC_ARM_REG_SP, arm_const.UC_ARM_REG_LR, arm_const.UC_ARM_REG_CPSR]
IRQ_VECTOR = 0x200003C4       # target of the IRQ entry in the vector table

CHUNK = 20_000                # instructions per run slice (~0.2 ms emulated)
IDLE_PROBE = 10_000           # max instructions for one idle-loop pass
IDLE_BACKOFF = 3              # slices to wait after a failed idle probe
IDLE_PASSES = 3               # loop passes tried before giving up on a probe
IDLE_SLICE = 1_500            # short slice after a skip: let the ISR run, re-probe
IDLE_SRAM_CHECK = 0x20000     # .data/.bss/stacks live in the first 128 KiB
# newlib rand(): the 64-bit LCG step after __getreent(), from `ldr lr,[r0,#0xa8]`
# to `str r3,[r0,#0xac]`; found once in each of the nine known images
RAND_SIG = bytes.fromhex('a8e090e5910e03e02cc09fe5ac1090e59c3121e0'
                         '9e2c83e0012092e2033081e00030a3e2a82080e5ac3080e5')
RAND_STATE_OFF = 0xA8         # state offset in the reent struct
STUCK_SLICES = 10_000         # ~2 s in one 256-byte window with no flash/LCD progress = stuck

# Registers whose reads must not simply echo the last write. Each entry is
# found by letting the firmware get stuck and reading the polling loop.
READ_QUIRKS = {
    # clock-source status: 1 = slow clock, 2 = PLL, after the switch bit
    # D000001C[15] is written (see 20013F80)
    0xD000003C: lambda p, old: 2 if p.regs.get(0xD000001C, 0) & 0x8000 else 1,
    # SPIFC status/config: bit 3 = transfer busy (driver at F8001ED0)
    0xC0150004: lambda p, old: old & ~0x8,
    # SPIFC RX byte from the last manual-mode transfer
    0xC015000C: lambda p, old: p.flash.rx,
    # interrupt controller: number of the highest-priority pending IRQ
    # (1..32, 0 = none) and FIQ (1..4); see dispatcher at F80011AC
    0xD0100028: lambda p, old: p.pending_irq(),
    0xD010002C: lambda p, old: 0,
}

# Registers with side effects on write: addr -> fn(periph, value)
WRITE_HOOKS = {
    # power control: bit 0 keeps the system powered; the firmware clears it
    # to enter deep sleep after arming the RTC alarm (see 200075EC)
    0xD0000078: lambda p, v: p.power_write(v),
    # SPIFC control: bit 6 = manual mode, holds chip-select low
    0xC0150000: lambda p, v: p.flash.select(bool(v & 0x40)),
    # SPIFC TX byte: clocks one byte through the flash chip
    0xC0150008: lambda p, v: p.flash.xfer(v & 0xFF),
}

# Buttons: GPIO pins (port*16 + bit) from the key table at 201AD200, read
# active-high through the port data register C0000000 + port*0x20
GPIO_BASE = 0xC0000000
KEY_PINS = {'A': 0x15, 'B': 0x16, 'C': 0x17}
KEY_HOLD = 0.15               # seconds a scripted press is held

CPU_HZ = 96_000_000          # PLL: 96 (from the firmware's own log); ~1 IPC

# Six timers at C0020000 + n*0x20 (driver at 2000A1CC, ISR at F8002F48):
#   +0x00 ctrl: bit15 IRQ pending (write 1 clears), bit14 IRQ enable,
#               bit13 run, bit0 clock = sysclk/256 instead of sysclk/2
#   +0x08 reload: -(period), 16 bit        +0x10 counter, counts up
TIMER_BASE = 0xC0020000
TIMER_COUNT = 6
TIMER_IRQ = 8                 # one of the lines wired to the timer ISR
INTC_MASK = 0xD0100030        # bit (32-n) set = IRQ n masked
INTC_DISABLE = 0xD0100038     # bit 0 = all IRQs off


LCD_BASE = 0xD0500000
LCD_IRQ = 27                  # ISR at F80041B8
LCD_W = LCD_H = 128           # D050036C = 0x00800080
VSYNC_HZ = 60


class Lcd:
    """TFT interface at D0500000 plus the panel behind it.

    +0x140 ctrl: bit0 start, bits 4-7 op (0x80 cmd, 0xA0 data, 0xD0 DMA
                 the framebuffer at +0x33C), bit13 = 8-bit bus
    +0x16C data byte/halfword for cmd/data ops
    +0x188 IRQ enable, +0x18C IRQ status (write 1 clears):
                 0x2000 vsync, 0x40 DMA done
    The panel speaks MIPI DCS (CASET 2A, RASET 2B, RAMWR 2C), RGB565
    big-endian over the 8-bit bus, like an ST7735.
    """

    def __init__(self, periph):
        self.p = periph
        self.fb = bytearray(LCD_W * LCD_H * 2)
        self.cmd = None
        self.args = []
        self.x0, self.x1, self.y0, self.y1 = 0, LCD_W - 1, 0, LCD_H - 1
        self.x = self.y = 0
        self.half = None
        self.frames = 0
        self.last_vsync = 0
        self.on_frame = None

    def reg(self, off):
        return self.p.regs.get(LCD_BASE + off, 0)

    def write_ctrl(self, val):
        if not val & 1:
            return
        op = val & 0xF0
        data = self.reg(0x16C)
        if op == 0x80:
            self._command(data & 0xFF)
        elif op == 0xA0:
            if val & 0x2000:
                self._data(data & 0xFF)
            else:
                self._data(data >> 8 & 0xFF)
                self._data(data & 0xFF)
        elif op == 0xD0:
            self._dma(self.reg(0x33C))
        self.p.regs[LCD_BASE + 0x140] = val & ~1       # transfer done at once

    def write_status(self, val, old):
        self.p.regs[LCD_BASE + 0x18C] = old & ~val      # write-1-to-clear

    def irq_line(self):
        return bool(self.reg(0x18C) & self.reg(0x188))

    def raise_status(self, bits):
        self.p.regs[LCD_BASE + 0x18C] = self.reg(0x18C) | bits

    def advance(self, now):
        n = int(now * VSYNC_HZ)
        if n > self.last_vsync:
            self.last_vsync = n
            self.raise_status(0x2000)

    def _command(self, c):
        self.cmd, self.args, self.half = c, [], None
        if c == 0x2C:
            self.x, self.y = self.x0, self.y0

    def _data(self, b):
        if self.cmd == 0x2C:
            if self.half is None:
                self.half = b
                return
            self._pixel((self.half << 8) | b)
            self.half = None
            return
        self.args.append(b)
        if len(self.args) == 4 and self.cmd in (0x2A, 0x2B):
            lo = self.args[0] << 8 | self.args[1]
            hi = self.args[2] << 8 | self.args[3]
            if self.cmd == 0x2A:
                self.x0, self.x1 = lo, hi
            else:
                self.y0, self.y1 = lo, hi

    def _pixel(self, rgb565):
        if self.x < LCD_W and self.y < LCD_H:
            i = (self.y * LCD_W + self.x) * 2
            self.fb[i:i + 2] = rgb565.to_bytes(2, 'little')
        self.x += 1
        if self.x > self.x1:
            self.x = self.x0
            self.y += 1
            if self.y > self.y1:
                self.y = self.y0
                self._frame_done()

    def _dma(self, addr):
        if not addr:                                    # no framebuffer set yet
            return
        try:
            src = bytes(self.p.uc.mem_read(addr, len(self.fb)))
        except UcError:
            print('[lcd] DMA from bad address %08X' % addr)
            return
        self.fb[:] = src                                # already little-endian RGB565
        self.raise_status(0x40)
        self._frame_done()

    def _frame_done(self):
        self.frames += 1
        if self.on_frame:
            self.on_frame(self)

    def rgb(self):
        out = bytearray(LCD_W * LCD_H * 3)
        for i in range(LCD_W * LCD_H):
            v = self.fb[2 * i] | self.fb[2 * i + 1] << 8
            out[3 * i] = (v >> 11 & 0x1F) * 255 // 31
            out[3 * i + 1] = (v >> 5 & 0x3F) * 255 // 63
            out[3 * i + 2] = (v & 0x1F) * 255 // 31
        return bytes(out)


def write_png(path, width, height, rgb, scale=1):
    import zlib
    rows = []
    for y in range(height):
        line = rgb[y * width * 3:(y + 1) * width * 3]
        if scale > 1:
            line = b''.join(line[i:i + 3] * scale for i in range(0, len(line), 3))
        rows.extend([b'\0' + line] * scale)
    def chunk(tag, data):
        c = tag + data
        return struct.pack('>I', len(data)) + c + struct.pack('>I', zlib.crc32(c))
    png = (b'\x89PNG\r\n\x1a\n'
           + chunk(b'IHDR', struct.pack('>IIBBBBB', width * scale, height * scale, 8, 2, 0, 0, 0))
           + chunk(b'IDAT', zlib.compress(b''.join(rows), 9))
           + chunk(b'IEND', b''))
    with open(path, 'wb') as f:
        f.write(png)


RTC_BUS = 0xC0090000
RTC_EPOCH = (2007, 12, 31)    # counter value 0 reads back as this date


class Rtc:
    """Low-power RTC reached through a byte-wide register bus (driver 20007DF8).

    Bus: +0x04 register address, +0x08 write data, +0x0C command
         (1 = write, 2 = read), +0x10 bit0 ready, +0x14 read data.
    RTC registers (from the driver at 20008270 and the boot sequence):
      0x00        control; bit 4 = busy
      0x10..0x15  staging for a new counter value, LSB first; writing
                  0x15 loads it (boot writes 2018-01-01 here after reset)
      0x20..0x25  alarm compare value, LSB first (wakes from deep sleep)
      0x30..0x35  48-bit counter at 32768 Hz, LSB first, read twice
      0x40, 0x50  alarm enables (bits 1-2), checked at boot for RTC wake
      0x80..0x83  backup RAM, 'XYZ[' marks the clock as valid
    Everything else is plain storage.
    """

    def __init__(self, periph, start_seconds=0):
        self.p = periph
        self.regs = {}
        self.base_ticks = int(start_seconds * 32768)
        self.base_time = 0.0
        self.data_out = 0

    def alarm_ticks(self):
        """Alarm time in counter ticks, or None if the alarm is not armed."""
        if not self.regs.get(0x40, 0) & 6:
            return None
        return sum(self.regs.get(0x20 + i, 0) << (8 * i) for i in range(6))

    def ticks(self):
        return self.base_ticks + int((self.p.game_time - self.base_time) * 32768)

    def command(self, cmd):
        addr = self.p.regs.get(RTC_BUS + 4, 0) & 0xFF
        if cmd & 1:
            data = self.p.regs.get(RTC_BUS + 8, 0) & 0xFF
            self.regs[addr] = data
            if addr == 0x15:
                self.base_ticks = sum(self.regs.get(0x10 + i, 0) << (8 * i)
                                      for i in range(6))
                self.base_time = self.p.game_time
        elif cmd & 2:
            if 0x30 <= addr <= 0x35:
                self.data_out = self.ticks() >> ((addr - 0x30) * 8) & 0xFF
            elif addr == 0x00:                       # bit 4 = busy, done at once
                self.data_out = self.regs.get(addr, 0) & ~0x10
            else:
                self.data_out = self.regs.get(addr, 0)


ADC_BASE = 0xC00C0000
ADC_IRQ = 29                  # ISR at F80000D4
ADC_BATTERY = 0xC000          # battery reading; below 0x592F shows LOW BATTERY


class Adc:
    """Single-shot ADC used for the battery check (loop at 2012DFA8).

    +0x04 ctrl: bit14 start, bit15 done (write 1 clears), bit7 result
          valid, bits 4-5 error flags (write 1 clears), bit6 IRQ enable
    +0x08 result (16 bit)
    """

    def __init__(self, periph):
        self.p = periph
        self.converting = False

    @property
    def ctrl(self):
        return self.p.regs.get(ADC_BASE + 4, 0)

    def write_ctrl(self, new, old):
        stored = new & ~0x80B0
        if old & 0x8000 and not new & 0x8000:           # done not acknowledged
            stored |= old & 0x8080
        if new & 0x4000 and not old & 0x4000:
            self.converting = True
        self.p.regs[ADC_BASE + 4] = stored

    def advance(self):
        # finish one slice after the start so the driver's read-modify-write
        # sequence does not acknowledge the result before the ISR sees it
        if self.converting:
            self.converting = False
            self.p.regs[ADC_BASE + 8] = ADC_BATTERY
            self.p.regs[ADC_BASE + 4] = self.ctrl | 0x8080

    def irq_line(self):
        return self.ctrl & 0x8040 == 0x8040


RTC_IRQ_BASE = 0xC0040000
RTC_IRQ = 1                   # ISR at F80013B0
# periodic RTC interrupt sources: status bit -> period in game seconds.
# Bit 1 drives the game-time routine F8005098 via callback F80042B0;
# once per second is a working assumption.
RTC_PERIODIC = {0x2: 1.0}


class RtcIrq:
    """Periodic interrupts of the RTC block at C0040000 (ISR F80013B0).

    +0x54 status (write 1 clears), +0x58 enable.
    """

    def __init__(self, periph):
        self.p = periph
        self.fired = {bit: 0 for bit in RTC_PERIODIC}

    def advance(self):
        status = self.p.regs.get(RTC_IRQ_BASE + 0x54, 0)
        for bit, period in RTC_PERIODIC.items():
            n = int(self.p.game_time / period)
            if n > self.fired[bit]:
                self.fired[bit] = n
                status |= bit
        self.p.regs[RTC_IRQ_BASE + 0x54] = status

    def write_status(self, new, old, written):
        self.p.regs[RTC_IRQ_BASE + 0x54] = old & ~written

    def irq_line(self):
        return bool(self.p.regs.get(RTC_IRQ_BASE + 0x54, 0)
                    & self.p.regs.get(RTC_IRQ_BASE + 0x58, 0))


class Timer:
    def __init__(self, periph, n):
        self.p = periph
        self.base = TIMER_BASE + n * 0x20
        self.start = 0.0
        self.periods_seen = 0
        self.pending = False

    @property
    def ctrl(self):
        return self.p.regs.get(self.base, 0)

    def _rate(self):
        # the driver (2000A1CC) computes reloads from sysclk/2, or sysclk/256
        # with the prescaler bit; timer_start(0, 1000 Hz) gives -12000
        clk = self.p.sysclk()
        return clk / 256 if self.ctrl & 1 else clk / 2

    def _period(self):
        reload = self.p.regs.get(self.base + 8, 0) & 0xFFFF
        return 0x10000 - reload if reload else 0x10000

    def _elapsed_ticks(self):
        return int((self.p.now - self.start) * self._rate())

    def read_ctrl(self, stored):
        return (stored & ~0x8000) | (0x8000 if self.pending else 0)

    def write_ctrl(self, val, old):
        if val & 0x8000:
            self.pending = False
        if val & 0x2000 and not old & 0x2000:
            self.start, self.periods_seen = self.p.now, 0
        self.p.regs[self.base] = val & ~0x8000

    def counter(self):
        if not self.ctrl & 0x2000:
            return self.p.regs.get(self.base + 0x10, 0)
        period = self._period()
        return (0x10000 - period + self._elapsed_ticks() % period) & 0xFFFF

    def advance(self):
        if not self.ctrl & 0x2000:
            return
        periods = self._elapsed_ticks() // self._period()
        if periods > self.periods_seen:
            self.periods_seen = periods
            if self.ctrl & 0x4000:
                self.pending = True


SOUND_TIMER = 4              # timer 4 in PWM mode drives the buzzer
SOUND_MAX_GAP = 2.0           # --wav shortens silences longer than this (sleep)


class Sound:
    """The buzzer: timer 4 produces a square wave (driver at 2000A83C).

    ctrl bit13 runs it; +0x08 reload = -(period) and +0x0C = -(high time),
    in the same 12 MHz ticks as the other timers. The boot jingle plays
    D6 E6 F6 G6 this way, one reprogramming per note.
    """

    def __init__(self, periph):
        self.p = periph
        self.state = (0.0, 0.5)                  # (frequency Hz, duty); 0 Hz = off
        self.events = []                         # (emulated time, freq, duty)

    def update(self):
        t = self.p.timers[SOUND_TIMER]
        if t.ctrl & 0x2000:
            period = t._period()
            high = 0x10000 - (self.p.regs.get(t.base + 0x0C, 0) & 0xFFFF)
            state = (t._rate() / period, min(max(high / period, 0.05), 0.95))
        else:
            state = (0.0, 0.5)
        if state != self.state:
            self.state = state
            self.events.append((self.p.now, state[0], state[1]))


class ToneSynth:
    """Turns buzzer events into 16-bit mono samples, keeping the phase smooth."""

    def __init__(self, rate=44100, volume=0.5):
        self.rate = rate
        self.amp = int(12000 * volume)
        self.freq, self.duty = 0.0, 0.5
        self.phase = 0.0

    def tone(self, n, out):
        """Append n samples of the current tone to the array out."""
        if not self.freq or not self.amp or self.freq >= self.rate / 2:   # off or inaudible
            out.extend([0] * n)
            return
        # each sample is the average of the square wave over the sample's
        # time, not a point sample: this removes most of the aliasing that
        # otherwise adds off-key overtones to high notes
        step = self.freq / self.rate
        ph, duty, amp = self.phase, self.duty, self.amp
        scale = 2 * amp / step
        for _ in range(n):
            end = ph + step
            high = max(0.0, min(end, duty) - ph) + max(0.0, min(end, 1.0 + duty) - max(ph, 1.0))
            out.append(int(high * scale) - amp)
            ph = end - 1.0 if end >= 1.0 else end
        self.phase = ph

    def render(self, events, t0, t1, n, out):
        """Append n samples covering emulated time t0..t1 to out.

        events are this span's (time, freq, duty) changes in order. When n
        doesn't match the span's length, the span is stretched to fit, so
        slow emulation plays in slow motion at the right pitch.
        """
        span = t1 - t0
        done = 0
        for t, freq, duty in events:
            upto = n if span <= 0 else min(n, round((t - t0) / span * n))
            if upto > done:
                self.tone(upto - done, out)
                done = upto
            self.freq, self.duty = freq, duty
        self.tone(n - done, out)


def write_wav(path, events, t0, t1, rate=44100):
    """Render buzzer events to a WAV file, with long silences shortened."""
    import array
    import wave
    synth = ToneSynth(rate)
    out = array.array('h')
    edges = [t for t, _, _ in events if t0 < t < t1] + [t1]
    pending = [e for e in events if t0 < e[0] < t1]
    for e in events:                         # state already in effect at t0
        if e[0] <= t0:
            synth.freq, synth.duty = e[1], e[2]
    t = t0
    for edge in edges:
        span = edge - t
        if not synth.freq:
            span = min(span, SOUND_MAX_GAP)
        synth.render([], t, edge, round(span * rate), out)
        while pending and pending[0][0] <= edge:
            _, synth.freq, synth.duty = pending.pop(0)
        t = edge
    with wave.open(path, 'wb') as w:
        w.setnchannels(1)
        w.setsampwidth(2)
        w.setframerate(rate)
        w.writeframes(out.tobytes())


class SpiFlash:
    """Generic 8 MiB SPI NOR flash (Winbond W25Q64-style command set).

    Its storage is the CPU-visible XIP window at FLASH_BASE, so programmed
    data shows up in memory-mapped reads straight away.
    """
    JEDEC_ID = (0xEF, 0x40, 0x17)
    SIZE = 0x800000

    def __init__(self, uc):
        self.uc = uc
        self.cs = False
        self.buf = []
        self.rx = 0xFF
        self.wel = False
        self.unknown = set()
        self.writes = 0

    def _addr(self):
        b = self.buf
        return ((b[1] << 16) | (b[2] << 8) | b[3]) % self.SIZE

    def _read(self, addr, n=1):
        return bytes(self.uc.mem_read(FLASH_BASE + addr, n))

    def select(self, on):
        if on and not self.cs:
            self.buf = []
        elif not on and self.cs:
            self._finish()
        self.cs = on

    def xfer(self, byte):
        self.buf.append(byte)
        cmd, i = self.buf[0], len(self.buf) - 1
        rx = 0xFF
        if i == 0:
            rx = 0xFF
        elif cmd == 0x9F:                          # JEDEC ID
            rx = self.JEDEC_ID[i - 1] if i <= 3 else 0xFF
        elif cmd == 0x90 and i >= 4:               # manufacturer/device ID
            rx = (0xEF, 0x16)[(i - 4) & 1]
        elif cmd == 0xAB and i >= 4:               # release power-down / ID
            rx = 0x16
        elif cmd in (0x05, 0x35, 0x15):            # status registers
            rx = (0x02 if self.wel else 0) if cmd == 0x05 else 0
        elif cmd == 0x03 and i >= 4:               # read
            rx = self._read((self._addr() + i - 4) % self.SIZE)[0]
        elif cmd == 0x0B and i >= 5:               # fast read (1 dummy byte)
            rx = self._read((self._addr() + i - 5) % self.SIZE)[0]
        elif cmd == 0x4B and i >= 5:               # unique ID
            rx = 0x5A
        elif cmd not in (0x06, 0x04, 0x02, 0x20, 0x52, 0xD8, 0xC7, 0x60,
                         0xB9, 0xFF, 0x66, 0x99, 0x01, 0x50):
            if cmd not in self.unknown:
                self.unknown.add(cmd)
                print('[flash] unhandled command %02X' % cmd)
        self.rx = rx
        return rx

    def _finish(self):
        if not self.buf:
            return
        cmd = self.buf[0]
        if cmd == 0x06:
            self.wel = True
        elif cmd == 0x04:
            self.wel = False
        elif cmd == 0x02 and len(self.buf) > 4 and self.wel:
            addr = self._addr()
            page, off = addr & ~0xFF, addr & 0xFF
            for b in self.buf[4:]:                  # NOR program: 1 -> 0 only
                a = page + off
                self.uc.mem_write(FLASH_BASE + a, bytes([self._read(a)[0] & b]))
                off = (off + 1) & 0xFF
            self.writes += 1
            self.wel = False
        elif cmd in (0x20, 0x52, 0xD8) and len(self.buf) >= 4 and self.wel:
            size = {0x20: 0x1000, 0x52: 0x8000, 0xD8: 0x10000}[cmd]
            start = self._addr() & ~(size - 1)
            self.uc.mem_write(FLASH_BASE + start, b'\xff' * size)
            self.writes += 1
            self.wel = False
        elif cmd in (0xC7, 0x60) and self.wel:
            self.uc.mem_write(FLASH_BASE, b'\xff' * self.SIZE)
            self.wel = False


class Peripherals:
    """Every MMIO register behaves like plain memory; accesses are logged."""

    def __init__(self, uc, trace, flash):
        self.uc = uc
        self.regs = {}
        self.seen = collections.OrderedDict()   # (addr, 'R'/'W') -> [count, first_pc, last_val]
        self.trace = trace
        self.flash = flash
        self.pc = 0
        self.now = 0.0                          # emulated seconds since reset
        self.timers = [Timer(self, n) for n in range(TIMER_COUNT)]
        self.lcd = Lcd(self)
        self.rtc = Rtc(self)
        self.adc = Adc(self)
        self.rtc_irq = RtcIrq(self)
        self.sound = Sound(self)
        self.turbo = 1.0                       # game clock speed-up
        self.game_time = 0.0                    # RTC seconds elapsed, scaled
        self.powered_off = False
        self.reads = dict(READ_QUIRKS)
        self.writes = dict(WRITE_HOOKS)
        self.writes[LCD_BASE + 0x140] = lambda p, v: p.lcd.write_ctrl(v)
        self.reads[RTC_BUS + 0x10] = lambda p, old: 1
        self.reads[RTC_BUS + 0x14] = lambda p, old: p.rtc.data_out
        self.writes[RTC_BUS + 0x0C] = lambda p, v: p.rtc.command(v)
        self.held = set()                       # pins currently pressed
        self.key_script = []                    # (time, key) presses
        self.live_keys = set()                  # keys held down right now (window)
        self.gpio_applied = [0] * 8             # button bits currently in RAM
        self.mmio_writes = 0
        for t in self.timers:
            self.reads[t.base] = lambda p, old, t=t: t.read_ctrl(old)
            self.reads[t.base + 0x10] = lambda p, old, t=t: t.counter()
        buzzer = self.timers[SOUND_TIMER].base
        for off in (0x08, 0x0C):                # pitch or duty changed
            self.writes[buzzer + off] = lambda p, v: p.sound.update()

    def sysclk(self):
        """Timer input clock, from the firmware's own clock variable."""
        clk = struct.unpack('<I', bytes(self.uc.mem_read(0xF80090A4, 4)))[0]
        return clk or 24_000_000

    def power_write(self, v):
        # boot only ever sets bit 0; clearing it is the deep-sleep request
        if not v & 1:
            self.powered_off = True

    def sync_gpio(self):
        """Write changed button bits into the GPIO data registers (RAM)."""
        for port in range(8):
            want = self.gpio_input(port)
            if want == self.gpio_applied[port]:
                continue
            addr = GPIO_BASE + port * 0x20
            val = struct.unpack('<I', bytes(self.uc.mem_read(addr, 4)))[0]
            val = (val & ~self.gpio_applied[port]) | want
            self.uc.mem_write(addr, struct.pack('<I', val))
            self.gpio_applied[port] = want

    def gpio_input(self, port):
        return sum(1 << (pin & 15) for pin in self.held if pin >> 4 == port)

    def advance(self, now):
        self.game_time += (now - self.now) * self.turbo
        self.now = now
        for t in self.timers:
            t.advance()
        self.lcd.advance(now)
        self.adc.advance()
        self.rtc_irq.advance()
        self.held = {KEY_PINS[k] for t, k in self.key_script
                     if t <= now < t + KEY_HOLD}
        self.held |= {KEY_PINS[k] for k in self.live_keys}
        self.sync_gpio()

    def next_event(self):
        """Emulated time of the next thing that can change what the CPU sees."""
        now = self.now
        if self.adc.converting or self.irq_wanted():
            return now
        times = [(self.lcd.last_vsync + 1) / VSYNC_HZ]
        for t in self.timers:
            if t.ctrl & 0x6000 == 0x6000:               # running with IRQ enabled
                if t.pending:
                    return now
                times.append(t.start + (t.periods_seen + 1) * t._period() / t._rate())
        for bit, period in RTC_PERIODIC.items():
            game_next = (self.rtc_irq.fired[bit] + 1) * period
            times.append(now + (game_next - self.game_time) / self.turbo)
        for t, _ in self.key_script:
            times.extend(edge for edge in (t, t + KEY_HOLD) if edge > now)
        return min(times)

    def irq_lines(self):
        lines = set()
        if any(t.pending for t in self.timers):
            lines.add(TIMER_IRQ)
        if self.lcd.irq_line():
            lines.add(LCD_IRQ)
        if self.adc.irq_line():
            lines.add(ADC_IRQ)
        if self.rtc_irq.irq_line():
            lines.add(RTC_IRQ)
        return lines

    def pending_irq(self):
        mask = self.regs.get(INTC_MASK, 0)
        for n in sorted(self.irq_lines()):
            if not (mask >> (32 - n)) & 1:
                return n
        return 0

    def irq_wanted(self):
        return not self.regs.get(INTC_DISABLE, 0) & 1 and self.pending_irq() != 0

    def _log(self, addr, kind, val):
        key = (addr, kind)
        ent = self.seen.get(key)
        if ent is None:
            self.seen[key] = [1, self.pc, val]
            if self.trace:
                print('  [mmio] %s %08X = %08X  pc=%08X' % (kind, addr, val, self.pc))
        else:
            ent[0] += 1
            ent[2] = val

    def read(self, base):
        def cb(uc, off, size, _):
            addr = base + off
            self.pc = uc.reg_read(UC_ARM_REG_PC)
            val = self.regs.get(addr & ~3, 0)
            quirk = self.reads.get(addr & ~3)
            if quirk:
                val = quirk(self, val)
            val = (val >> ((addr & 3) * 8)) & ((1 << (size * 8)) - 1)
            self._log(addr, 'R', val)
            return val
        return cb

    def write(self, base):
        def cb(uc, off, size, val, _):
            addr = base + off
            self.pc = uc.reg_read(UC_ARM_REG_PC)
            word = addr & ~3
            sh = (addr & 3) * 8
            mask = ((1 << (size * 8)) - 1) << sh
            self.mmio_writes += 1
            old = self.regs.get(word, 0)
            new = (old & ~mask) | ((val << sh) & mask)
            self._log(addr, 'W', val)
            timer = self._timer_ctrl(word)
            if timer:
                timer.write_ctrl(new, old)
                if timer is self.timers[SOUND_TIMER]:
                    self.sound.update()
                return
            if word == LCD_BASE + 0x18C:
                self.lcd.write_status(new, old)
                return
            if word == ADC_BASE + 4:
                self.adc.write_ctrl(new, old)
                return
            if word == RTC_IRQ_BASE + 0x54:
                self.rtc_irq.write_status(new, old, (val << sh) & mask)
                return
            self.regs[word] = new
            hook = self.writes.get(word)
            if hook:
                hook(self, new)
        return cb

    def _timer_ctrl(self, word):
        off = word - TIMER_BASE
        if 0 <= off < TIMER_COUNT * 0x20 and off % 0x20 == 0:
            return self.timers[off // 0x20]
        return None


def read_cstr(uc, addr, limit=256):
    try:
        raw = bytes(uc.mem_read(addr, limit))
    except UcError:
        return '<bad ptr %08X>' % addr
    return raw.split(b'\0', 1)[0].decode('latin-1')


def c_printf(uc, fmt, args):
    """Tiny printf: enough for %d %u %x %X %p %s %c with flags/width."""
    it = iter(args)
    def sub(m):
        flags, width, conv = m.group(1), m.group(2), m.group(3)
        if conv == '%':
            return '%'
        v = next(it, 0)
        if conv == 's':
            return read_cstr(uc, v)
        if conv == 'c':
            return chr(v & 0xFF)
        if conv in 'di':
            v = v - (1 << 32) if v & 0x80000000 else v
            conv = 'd'
        if conv == 'p':
            conv, flags = 'X', '0'
            width = width or '8'
        if conv == 'u':
            conv = 'd'
        return ('%' + flags + width + conv) % v
    return re.sub(r'%([-0 +#]*)(\d*)(?:l|h|hh|ll)?([diuxXpsc%])', sub, fmt)


class RegReader:
    """Reads a fixed set of registers with one direct call into Unicorn's C API.

    Unicorn's Python reg_read_batch rebuilds its ctypes arrays on every call
    (~35 us); idle detection reads the registers tens of thousands of times a
    second, so the arrays are built once here. Returns the values as bytes.
    """

    def __init__(self, uc, regs):
        import ctypes
        from unicorn.unicorn_py3.unicorn import uclib
        n = len(regs)
        self.call = uclib.uc_reg_read_batch
        self.handle = uc._uch
        self.ids = (ctypes.c_int * n)(*regs)
        self.vals = (ctypes.c_uint64 * n)()             # 32-bit registers, upper half stays 0
        base = ctypes.addressof(self.vals)
        self.ptrs = (ctypes.c_void_p * n)(*(base + 8 * i for i in range(n)))
        self.n = n

    def read(self):
        if self.call(self.handle, self.ids, self.ptrs, self.n):
            raise UcError(self.call(self.handle, self.ids, self.ptrs, self.n))
        return bytes(self.vals)


class Emulator:
    def __init__(self, image, trace_mmio=False, start_time=None):
        self.image = image
        self.uc = uc = Uc(UC_ARCH_ARM, UC_MODE_ARM, UC_CPU_ARM_926)
        uc.mem_map(FLASH_BASE, RAM_SIZE, UC_PROT_ALL)
        uc.mem_write(FLASH_BASE, image)
        uc.mem_map(SRAM_BASE, SRAM_SIZE, UC_PROT_ALL)

        self.flash = SpiFlash(uc)
        self.periph = Peripherals(uc, trace_mmio, self.flash)
        import datetime
        start_time = start_time or datetime.datetime.now()
        rtc_seconds = (start_time - datetime.datetime(*RTC_EPOCH)).total_seconds()
        self.periph.rtc.base_ticks = int(rtc_seconds * 32768)
        uc.mem_map(*GPIO_PAGE, UC_PROT_ALL)
        for base, size in MMIO_REGIONS:
            uc.mmio_map(base, size, self.periph.read(base), None,
                        self.periph.write(base), None)

        self.unmapped = []
        uc.hook_add(UC_HOOK_MEM_UNMAPPED, self._on_unmapped)

        self.log = []
        self.printf_addr = self._find_printf()
        if self.printf_addr is not None:
            uc.hook_add(UC_HOOK_CODE, self._on_printf,
                        begin=self.printf_addr, end=self.printf_addr)

        uc.reg_write(UC_ARM_REG_PC, RESET_PC)
        self.executed = 0
        self.irqs = 0
        self.idle_skip = True
        self.idle_skipped = 0
        self.pc_hist = collections.Counter()
        self.rand_state = None                          # SRAM offset of rand()'s 64-bit state
        self.idle_anchor = None                         # PC where the last idle skip succeeded
        self.idle_regs = RegReader(uc, IDLE_REGS)
        self.lr_reg = RegReader(uc, [UC_ARM_REG_LR])
        self._find_rand()

    def _find_printf(self):
        """printf is a stub `push {r0-r3}; add sp,#0x10; bx lr` in release builds."""
        stub = bytes.fromhex('0f002de9 10d08de2 1eff2fe1')
        off = self.image.find(stub, 0, 0x200000)
        return FLASH_BASE + off if off >= 0 else None

    def _on_printf(self, uc, addr, size, _):
        r = [uc.reg_read(x) for x in (UC_ARM_REG_R0, UC_ARM_REG_R1,
                                      UC_ARM_REG_R2, UC_ARM_REG_R3)]
        sp = uc.reg_read(UC_ARM_REG_SP)
        stack = struct.unpack('<8I', bytes(uc.mem_read(sp, 32)))
        try:
            msg = c_printf(uc, read_cstr(uc, r[0]), r[1:] + list(stack))
        except Exception as e:  # malformed format: keep going
            msg = '<printf error %s: %r>' % (e, read_cstr(uc, r[0]))
        msg = msg.rstrip('\r\n')
        self.log.append(msg)
        print('[fw] ' + msg)

    def _on_unmapped(self, uc, access, addr, size, value, _):
        pc = uc.reg_read(UC_ARM_REG_PC)
        self.unmapped.append((addr, pc, access))
        print('[!] unmapped access %08X at pc=%08X, mapping zero page' % (addr, pc))
        page = addr & ~0xFFFF
        try:
            uc.mem_map(page, 0x10000, UC_PROT_ALL)
        except UcError:
            return False
        return len(self.unmapped) < 64

    def enter_irq(self):
        """ARM IRQ exception entry; Unicorn has no API for raising one."""
        uc = self.uc
        pc = uc.reg_read(UC_ARM_REG_PC)
        cpsr = uc.reg_read(UC_ARM_REG_CPSR)
        uc.reg_write(UC_ARM_REG_CPSR, (cpsr & ~0x3F) | 0x80 | 0x12)  # IRQ mode, I set, ARM
        uc.reg_write(UC_ARM_REG_SPSR, cpsr)
        uc.reg_write(UC_ARM_REG_LR, pc + 4)
        uc.reg_write(UC_ARM_REG_PC, IRQ_VECTOR)
        self.irqs += 1

    def _find_rand(self):
        """Hook rand() once to learn where its state lives (see _idle_state)."""
        off = self.image.find(RAND_SIG, 0, 0x200000)
        if off < 0:
            return
        def got_state(uc, addr, size, _):             # r0 = reent struct, after __getreent()
            self.rand_state = uc.reg_read(UC_ARM_REG_R0) + RAND_STATE_OFF - SRAM_BASE
            uc.hook_del(self._rand_hook)
        pc = FLASH_BASE + off                           # first instruction of the signature
        self._rand_hook = self.uc.hook_add(UC_HOOK_CODE, got_state, begin=pc, end=pc)

    def _idle_state(self):
        uc = self.uc
        regs = self.idle_regs.read()
        ram = uc.mem_read(SRAM_BASE, IDLE_SRAM_CHECK)             # compared with memcmp
        r = self.rand_state
        if r is not None and 0 <= r <= IDLE_SRAM_CHECK - 8:
            ram[r:r + 8] = bytes(8)
        return regs, bytes(ram), self.periph.mmio_writes

    def _run_to(self, addr):
        """Run until the CPU reaches addr; False if not within IDLE_PROBE."""
        uc = self.uc
        reached = [False]
        def at(uc, a, size, _):
            reached[0] = True
            uc.emu_stop()
        pc = uc.reg_read(UC_ARM_REG_PC)
        thumb = uc.reg_read(UC_ARM_REG_CPSR) & 0x20
        h = uc.hook_add(UC_HOOK_CODE, at, begin=addr, end=addr)
        try:
            uc.emu_start(pc | (1 if thumb else 0), 0xFFFFFFFF, count=IDLE_PROBE)
        finally:
            uc.hook_del(h)
        return reached[0]

    def try_idle_skip(self, max_insns):
        """Skip ahead to the next event if the CPU is provably spinning.

        Runs the loop the CPU is in until it is back at the same PC with the
        same registers (the PC may be inside a helper called several times
        per pass, so that can take a few hits). If internal RAM and MMIO
        writes are unchanged too, every further pass is identical, and nothing
        can change until the next interrupt or button event, so emulated time
        jumps straight there. rand()'s state is left out of the comparison:
        the main loop calls rand() every pass and throws the result away, only
        to stir the sequence. Returns True if time was skipped.
        """
        uc = self.uc
        anchor = self.idle_anchor
        if anchor is not None and uc.reg_read(UC_ARM_REG_PC) != anchor:
            # measure from where the last skip succeeded: usually the loop
            # itself, passed once per pass, rather than a helper inside it
            if not self._run_to(anchor):
                self.idle_anchor = None
                self.executed += IDLE_PROBE
                return False
        pc = uc.reg_read(UC_ARM_REG_PC)
        thumb = uc.reg_read(UC_ARM_REG_CPSR) & 0x20
        before = self._idle_state()
        for _ in range(IDLE_PASSES):
            want = before[0]
            i = IDLE_REGS.index(arm_const.UC_ARM_REG_LR) * 8
            want_lr = want[i:i + 8]
            hits = [0, False]
            read_lr, read_regs = self.lr_reg.read, self.idle_regs.read
            def at_pc(uc, addr, size, _):
                hits[0] += 1
                # the return address alone rules out most calls of a helper
                # from other places, and is cheaper to read than all registers
                if hits[0] > 1 and read_lr() == want_lr and read_regs() == want:
                    hits[1] = True
                    uc.emu_stop()
            h = uc.hook_add(UC_HOOK_CODE, at_pc, begin=pc, end=pc)
            try:
                uc.emu_start(pc | (1 if thumb else 0), 0xFFFFFFFF, count=IDLE_PROBE)
            finally:
                uc.hook_del(h)
            if not hits[1]:                             # no repeat within the probe
                self.executed += IDLE_PROBE
                return False
            after = self._idle_state()
            if after == before:
                break
            # the first pass after an interrupt often consumes a flag the ISR
            # set; the next pass may already be a fixed point
            before = after
        else:
            return False
        self.idle_anchor = pc
        target = int(self.periph.next_event() * CPU_HZ) + 1
        target = min(target, max_insns)
        if target <= self.executed:
            return False
        self.idle_skipped += target - self.executed
        self.executed = target
        return True

    def run(self, max_insns):
        uc = self.uc
        stuck_slices = 0
        last_window = None
        probe_wait = 0
        chunk = CHUNK
        while self.executed < max_insns:
            self.periph.advance(self.executed / CPU_HZ)
            if self.periph.irq_wanted() and not uc.reg_read(UC_ARM_REG_CPSR) & 0x80:
                self.enter_irq()
            pc = uc.reg_read(UC_ARM_REG_PC)
            thumb = uc.reg_read(UC_ARM_REG_CPSR) & 0x20
            self.periph.pc = pc
            try:
                uc.emu_start(pc | (1 if thumb else 0), 0xFFFFFFFF, count=chunk)
            except UcError as e:
                pc = uc.reg_read(UC_ARM_REG_PC)
                print('[X] CPU exception %s at pc=%08X lr=%08X'
                      % (e, pc, uc.reg_read(UC_ARM_REG_LR)))
                return 'crash'
            self.executed += chunk
            chunk = CHUNK
            if self.periph.powered_off:
                return 'poweroff'
            if self.idle_skip and self.executed < max_insns:
                if probe_wait:
                    probe_wait -= 1
                elif self.try_idle_skip(max_insns):
                    stuck_slices = 0                    # waiting is not hanging
                    chunk = IDLE_SLICE
                    continue
                else:
                    probe_wait = IDLE_BACKOFF
            pc = uc.reg_read(UC_ARM_REG_PC)
            self.pc_hist[pc & ~0xFF] += 1
            # a slice counts as progress if the flash or LCD did something,
            # so long erase loops are not mistaken for a hang
            window = (pc & ~0xFF, self.flash.writes, self.periph.lcd.frames)
            stuck_slices = stuck_slices + 1 if window == last_window else 0
            last_window = window
            if stuck_slices >= STUCK_SLICES:
                print('[?] execution parked around %08X for %d slices'
                      % (pc, stuck_slices))
                return 'stuck'
        return 'limit'

    def report(self, outcome):
        pc = self.uc.reg_read(UC_ARM_REG_PC)
        print('\n=== result: %s after ~%d instructions, pc=%08X ==='
              % (outcome, self.executed, pc))
        print('idle time skipped: %.3f s' % (self.idle_skipped / CPU_HZ))
        print('emulated time %.3f s, IRQs delivered: %d, printf lines: %d, '
              'unmapped accesses: %d, flash writes: %d'
              % (self.executed / CPU_HZ, self.irqs, len(self.log),
                 len(self.unmapped), self.flash.writes))
        blocks = collections.Counter(a & ~0xFFF for a, _ in self.periph.seen)
        print('\nperipheral blocks touched (4K granularity):')
        for b, n in sorted(blocks.items()):
            print('  %08X  %3d registers' % (b, n))
        print('\nregisters read most often (likely polling / status):')
        reads = [(v[0], a, v[1], v[2]) for (a, k), v in self.periph.seen.items() if k == 'R']
        for n, a, first_pc, val in sorted(reads, reverse=True)[:15]:
            print('  %08X  x%-8d first pc=%08X  last=%08X' % (a, n, first_pc, val))
        print('\nhottest code (256-byte windows, sampled per slice):')
        for w, n in self.pc_hist.most_common(8):
            print('  %08X  %d' % (w, n))
        if outcome != 'limit':
            self.disasm_around(pc)

    def disasm_around(self, pc, before=0x30, after=0x20):
        from capstone import Cs, CS_ARCH_ARM, CS_MODE_ARM, CS_MODE_THUMB
        thumb = self.uc.reg_read(UC_ARM_REG_CPSR) & 0x20
        md = Cs(CS_ARCH_ARM, CS_MODE_THUMB if thumb else CS_MODE_ARM)
        md.skipdata = True
        start = (pc - before) & ~3
        code = bytes(self.uc.mem_read(start, before + after))
        print('\ncode around pc (%s):' % ('thumb' if thumb else 'arm'))
        for ins in md.disasm(code, start):
            mark = '>>' if ins.address == pc else '  '
            print('  %s %08X  %-7s %s' % (mark, ins.address, ins.mnemonic, ins.op_str))


def power_cycle(emu, limit):
    """Deep sleep: skip to the RTC alarm or the next button press, then cold-boot.

    Flash, the RTC (counter, alarm, battery-backed registers) and elapsed time
    survive; the CPU, SRAM and other peripherals start from reset, as on the
    real chip. Returns the emulator to continue with (a new one after a wake).
    """
    p = emu.periph
    now = emu.executed / CPU_HZ
    waits = []
    alarm = p.rtc.alarm_ticks()
    if alarm is not None:
        waits.append((max(0.0, (alarm - p.rtc.ticks()) / 32768 / p.turbo), 'RTC alarm'))
    presses = [t for t, _ in p.key_script if t >= now]
    if presses:
        waits.append((min(presses) - now, 'button'))
    if not waits:
        print('[pwr] powered off with no wake source at %.3f s' % now)
        return None
    dt, reason = min(waits)
    if emu.executed + int(dt * CPU_HZ) > limit:           # still asleep at the end
        p.game_time += (limit / CPU_HZ - now) * p.turbo
        emu.executed = limit
        p.now = limit / CPU_HZ
        return emu
    print('[pwr] asleep at %.3f s, %s wakes it after %.1f game seconds'
          % (now, reason, dt * p.turbo))
    flash = bytes(emu.uc.mem_read(FLASH_BASE, SpiFlash.SIZE))
    new = Emulator(flash, p.trace)
    new.image = emu.image
    new.executed = emu.executed + int(dt * CPU_HZ)
    new.irqs, new.log = emu.irqs, emu.log
    new.idle_skip, new.idle_skipped = emu.idle_skip, emu.idle_skipped
    new.flash.writes = emu.flash.writes
    q = new.periph
    q.now = new.executed / CPU_HZ
    q.game_time = p.game_time + dt * p.turbo
    q.turbo, q.key_script, q.live_keys = p.turbo, p.key_script, p.live_keys
    q.sound.events = p.sound.events
    if p.sound.state[0]:                                  # power cut stops the buzzer
        p.sound.events.append((now, 0.0, 0.5))
    q.rtc, q.rtc.p = p.rtc, q
    q.rtc_irq.fired = {bit: int(q.game_time / per) for bit, per in RTC_PERIODIC.items()}
    q.lcd.frames, q.lcd.on_frame = p.lcd.frames, p.lcd.on_frame
    q.lcd.last_vsync = int(q.now * VSYNC_HZ)
    return new


CODE_CHECK_LEN = 0x1E0000     # compared to make sure a save matches its ROM

# Peripheral fields captured in snapshots, per object
SNAP_FIELDS = {
    'periph': ('regs', 'now', 'game_time', 'powered_off', 'gpio_applied'),
    'rtc_irq': ('fired',),
    'lcd': ('fb', 'cmd', 'args', 'x0', 'x1', 'y0', 'y1', 'x', 'y', 'half',
            'frames', 'last_vsync'),
    'rtc': ('regs', 'base_ticks', 'base_time', 'data_out'),
    'adc': ('converting',),
    'flash': ('cs', 'buf', 'rx', 'wel', 'writes'),
}


def _snap_objects(emu):
    p = emu.periph
    return {'periph': p, 'lcd': p.lcd, 'rtc': p.rtc, 'adc': p.adc,
            'rtc_irq': p.rtc_irq, 'flash': emu.flash}


def save_snapshot(emu, path):
    """Freeze the whole machine (CPU, memory, peripherals) to a file.

    Snapshots are Python pickles: only load ones you made yourself.
    """
    import gzip
    import pickle
    state = {
        'cpu': emu.uc.context_save(),
        'ram': bytes(emu.uc.mem_read(FLASH_BASE, RAM_SIZE)),
        'sram': bytes(emu.uc.mem_read(SRAM_BASE, SRAM_SIZE)),
        'gpio': bytes(emu.uc.mem_read(*GPIO_PAGE)),
        'executed': emu.executed,
        'irqs': emu.irqs,
        'timers': [(t.start, t.periods_seen, t.pending) for t in emu.periph.timers],
        'code_check': emu.image[:CODE_CHECK_LEN],
    }
    for name, obj in _snap_objects(emu).items():
        state[name] = {f: getattr(obj, f) for f in SNAP_FIELDS[name]}
    with gzip.open(path, 'wb', compresslevel=3) as f:
        pickle.dump(state, f)


def load_snapshot(emu, path):
    import gzip
    import pickle
    with gzip.open(path, 'rb') as f:
        state = pickle.load(f)
    if state['code_check'] != emu.image[:CODE_CHECK_LEN]:
        sys.exit('snapshot %s belongs to a different ROM version' % path)
    emu.uc.context_restore(state['cpu'])
    emu.uc.mem_write(FLASH_BASE, state['ram'])
    emu.uc.mem_write(SRAM_BASE, state['sram'])
    emu.executed, emu.irqs = state['executed'], state['irqs']
    for t, (start, periods, pending) in zip(emu.periph.timers, state['timers']):
        t.start, t.pending = start, pending
    emu.periph.now = state['periph']['now']
    for t in emu.periph.timers:                     # recompute under the current timer model
        t.periods_seen = t._elapsed_ticks() // t._period() if t.ctrl & 0x2000 else 0
    for name, obj in _snap_objects(emu).items():
        for f, v in state.get(name, {}).items():
            setattr(obj, f, v)
    p = emu.periph
    if 'gpio' in state:
        emu.uc.mem_write(GPIO_PAGE[0], state['gpio'])
    else:                                               # older snapshot: GPIO was MMIO
        for addr, val in p.regs.items():
            if GPIO_PAGE[0] <= addr < GPIO_PAGE[0] + GPIO_PAGE[1]:
                emu.uc.mem_write(addr, struct.pack('<I', val & 0xFFFFFFFF))
    if 'game_time' not in state['periph']:              # older snapshot
        p.game_time = p.now
    if 'rtc_irq' not in state:
        p.rtc_irq.fired = {bit: int(p.game_time / per) for bit, per in RTC_PERIODIC.items()}


def load_save(path, image):
    """Flash contents from a save file, or None if there is no save yet."""
    import os
    if not os.path.exists(path):
        return None
    flash = open(path, 'rb').read()
    if len(flash) != len(image) or flash[:CODE_CHECK_LEN] != image[:CODE_CHECK_LEN]:
        sys.exit('save file %s belongs to a different ROM version' % path)
    return flash


def restore_rtc(emu, path):
    """Carry the RTC over from the last session, adding the real time since."""
    import json
    import os
    import time
    meta = path + '.json'
    if not os.path.exists(meta):
        return False
    state = json.load(open(meta))
    rtc = emu.periph.rtc
    rtc.regs = {int(k): v for k, v in state['rtc_regs'].items()}
    elapsed = max(0.0, time.time() - state['saved_at'])
    rtc.base_ticks = state['rtc_ticks'] + int(elapsed * 32768)
    return True


def write_save(emu, path):
    import json
    import os
    import time
    flash = bytes(emu.uc.mem_read(FLASH_BASE, SpiFlash.SIZE))
    tmp = path + '.tmp'
    with open(tmp, 'wb') as f:
        f.write(flash)
    os.replace(tmp, path)
    state = {'rtc_ticks': emu.periph.rtc.ticks(),
             'rtc_regs': emu.periph.rtc.regs,
             'saved_at': time.time()}
    with open(path + '.json', 'w') as f:
        json.dump(state, f, indent=1)


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument('image')
    ap.add_argument('--insns', type=int, default=50_000_000,
                    help='instructions to run in this session')
    ap.add_argument('--seconds', type=float,
                    help='emulated seconds to run (overrides --insns)')
    ap.add_argument('--trace-mmio', action='store_true')
    ap.add_argument('--frames', metavar='DIR',
                    help='save every distinct LCD frame as a PNG in DIR')
    ap.add_argument('--screenshot', metavar='PNG', default='screen.png',
                    help='where to save the final LCD contents')
    ap.add_argument('--press', metavar='SECONDS:KEY', action='append', default=[],
                    help='press button A, B or C at T seconds into this run, '
                         'e.g. 4.5:B; A+C presses both')
    ap.add_argument('--date', metavar='"YYYY-MM-DD HH:MM"',
                    help='RTC start time (default: now, or the saved clock)')
    ap.add_argument('--save', metavar='FILE',
                    help='persistent flash image (+ FILE.json for the clock); '
                         'boots from it if present and writes it back on exit')
    ap.add_argument('--no-idle-skip', action='store_true',
                    help='always execute idle loops instead of skipping them')
    ap.add_argument('--turbo', type=float, default=1.0,
                    help='run the device clock N times faster (fast-forward)')
    ap.add_argument('--snapshot-in', metavar='FILE',
                    help='resume from a snapshot made by --snapshot-out')
    ap.add_argument('--snapshot-out', metavar='FILE',
                    help='freeze the whole machine to FILE at the end of the run')
    ap.add_argument('--wav', metavar='FILE',
                    help='write the buzzer sound of this run to a WAV file '
                         '(silences over %g s, e.g. sleep, are shortened)' % SOUND_MAX_GAP)
    a = ap.parse_args()
    image = open(a.image, 'rb').read()
    if image[:4] != b'SPII':
        sys.exit('not a tg18 SPI image (missing SPII header)')
    import datetime
    start = datetime.datetime.strptime(a.date, '%Y-%m-%d %H:%M') if a.date else None
    saved = load_save(a.save, image) if a.save else None
    emu = Emulator(saved or image, a.trace_mmio, start)
    if saved:
        resumed = not a.date and restore_rtc(emu, a.save)
        print('booting from save %s%s' % (a.save, ' (clock resumed)' if resumed else ''))
    if a.snapshot_in:
        load_snapshot(emu, a.snapshot_in)
        print('resumed snapshot %s at %.3f s' % (a.snapshot_in, emu.executed / CPU_HZ))
    emu.periph.turbo = a.turbo
    emu.idle_skip = not a.no_idle_skip
    t0 = emu.executed / CPU_HZ
    for spec in a.press:
        t, keys = spec.split(':')
        for key in keys.upper().split('+'):
            if key not in KEY_PINS:
                sys.exit('unknown key %r, use A, B or C' % key)
            emu.periph.key_script.append((t0 + float(t), key))
    print('printf stub at %s' % (hex(emu.printf_addr) if emu.printf_addr else 'not found'))

    lcd = emu.periph.lcd
    if a.frames:
        import hashlib
        import os
        os.makedirs(a.frames, exist_ok=True)
        seen = set()
        def save_frame(l):
            h = hashlib.md5(l.fb).digest()
            if h in seen:
                return
            seen.add(h)
            path = os.path.join(a.frames, 'frame_%05d_%07.3fs.png' % (l.frames, emu.periph.now))
            write_png(path, LCD_W, LCD_H, l.rgb(), scale=3)
            print('[lcd] new frame -> %s' % path)
        lcd.on_frame = save_frame

    insns = int(a.seconds * CPU_HZ) if a.seconds else a.insns
    end = emu.executed + insns
    while True:
        outcome = emu.run(end)
        if outcome != 'poweroff':
            break
        woken = power_cycle(emu, end)
        if woken is None or woken is emu:
            outcome = 'asleep'
            break
        emu = woken
    emu.report(outcome)
    lcd = emu.periph.lcd                                # may be a new machine after a wake
    print('\nLCD frames completed: %d' % lcd.frames)
    write_png(a.screenshot, LCD_W, LCD_H, lcd.rgb(), scale=3)
    print('final screen saved to %s' % a.screenshot)
    if a.save:
        write_save(emu, a.save)
        print('flash and clock saved to %s' % a.save)
    if a.snapshot_out:
        save_snapshot(emu, a.snapshot_out)
        print('snapshot written to %s' % a.snapshot_out)
    if a.wav:
        events = emu.periph.sound.events
        write_wav(a.wav, events, t0, emu.executed / CPU_HZ)
        print('%d buzzer changes, sound written to %s' % (len(events), a.wav))


if __name__ == '__main__':
    main()
