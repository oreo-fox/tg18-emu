//! Memory map and hardware of the tg18 SoC: what the CPU sees on its bus.
//!
//! A port of the Python prototype's peripherals (prototype/tg18emu.py); the
//! register descriptions there and in the README apply.

use std::collections::HashMap;
use std::hash::{BuildHasherDefault, Hasher};

use crate::cpu::Bus;
use crate::*;

/// Identity-style hasher for register addresses (fast, keys are u32).
#[derive(Default)]
pub struct AddrHasher(u64);

impl Hasher for AddrHasher {
    fn finish(&self) -> u64 {
        self.0
    }
    fn write(&mut self, bytes: &[u8]) {
        for b in bytes {
            self.0 = (self.0 << 8 | *b as u64).wrapping_mul(0x9E37_79B9_7F4A_7C15);
        }
    }
    fn write_u32(&mut self, n: u32) {
        self.0 = (n as u64).wrapping_mul(0x9E37_79B9_7F4A_7C15);
    }
}

pub type RegMap = HashMap<u32, u32, BuildHasherDefault<AddrHasher>>;

// --- register addresses ------------------------------------------------------

pub const GPIO_BASE: u32 = 0xC000_0000;
pub const GPIO_SIZE: usize = 0x1000;
pub const UART_BASE: u32 = 0xC006_0000;
pub const UART_SIZE: usize = 0x1000;
pub const UART_IR_REGS: [u32; 4] = [0x00, 0x10, 0x14, 0x20];
pub const TIMER_BASE: u32 = 0xC002_0000;
pub const TIMER_COUNT: usize = 6;
pub const TIMER_IRQ: u32 = 8;
pub const SOUND_TIMER: usize = 4;
pub const RTC_IRQ_BASE: u32 = 0xC004_0000;
pub const RTC_IRQ: u32 = 1;
pub const SPI2_BASE: u32 = 0xC008_0000;
pub const SPI2_IDLE: u32 = 0x00;
pub const RTC_BUS: u32 = 0xC009_0000;
pub const ADC_BASE: u32 = 0xC00C_0000;
pub const ADC_IRQ: u32 = 29;
pub const ADC_BATTERY: u32 = 0xC000;
pub const SPIFC_BASE: u32 = 0xC015_0000;
pub const POWER_REG: u32 = 0xD000_0078;
pub const CLOCK_SWITCH: u32 = 0xD000_001C;
pub const CLOCK_STATUS: u32 = 0xD000_003C;
pub const INTC_PENDING: u32 = 0xD010_0028;
pub const INTC_PENDING_FIQ: u32 = 0xD010_002C;
pub const INTC_MASK: u32 = 0xD010_0030;
pub const INTC_DISABLE: u32 = 0xD010_0038;
pub const LCD_BASE: u32 = 0xD050_0000;
pub const LCD_IRQ: u32 = 27;
pub const LCD_W: usize = 128;
pub const LCD_H: usize = 128;
pub const VSYNC_HZ: f64 = 60.0;
/// Firmware variable holding the system clock (timers run at half of it).
pub const SYSCLK_VAR: u32 = 0xF800_90A4;

/// Buttons: GPIO pin (port * 16 + bit), active high.
pub fn key_pin(key: Key) -> u32 {
    match key {
        Key::A => 0x15,
        Key::B => 0x16,
        Key::C => 0x17,
    }
}

/// Seconds a scripted press is held.
pub const KEY_HOLD: f64 = 0.15;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Key {
    A,
    B,
    C,
}

impl Key {
    pub fn parse(s: &str) -> Option<Key> {
        match s.trim().to_ascii_uppercase().as_str() {
            "A" => Some(Key::A),
            "B" => Some(Key::B),
            "C" => Some(Key::C),
            _ => None,
        }
    }
    pub fn all() -> [Key; 3] {
        [Key::A, Key::B, Key::C]
    }
    pub fn index(self) -> usize {
        self as usize
    }
}

// --- timers ------------------------------------------------------------------

/// Six timers at C0020000 + n*0x20: +0 ctrl (b15 pending, b14 IRQ enable,
/// b13 run, b0 sysclk/256 instead of sysclk/2), +8 reload = -(period),
/// +0x10 counter.
#[derive(Clone, Default)]
pub struct Timer {
    pub base: u32,
    pub start: f64,
    pub periods_seen: u64,
    pub pending: bool,
}

// --- LCD ---------------------------------------------------------------------

/// TFT interface at D0500000 plus an ST7735-like panel behind it (see the
/// prototype's Lcd class).
#[derive(Clone)]
pub struct Lcd {
    pub fb: Vec<u8>,
    pub cmd: Option<u8>,
    pub args: Vec<u8>,
    pub x0: u32,
    pub x1: u32,
    pub y0: u32,
    pub y1: u32,
    pub x: u32,
    pub y: u32,
    pub half: Option<u8>,
    pub frames: u64,
    pub last_vsync: u64,
}

impl Default for Lcd {
    fn default() -> Self {
        Lcd {
            fb: vec![0; LCD_W * LCD_H * 2],
            cmd: None,
            args: Vec::new(),
            x0: 0,
            x1: LCD_W as u32 - 1,
            y0: 0,
            y1: LCD_H as u32 - 1,
            x: 0,
            y: 0,
            half: None,
            frames: 0,
            last_vsync: 0,
        }
    }
}

impl Lcd {
    fn command(&mut self, c: u8) {
        self.cmd = Some(c);
        self.args.clear();
        self.half = None;
        if c == 0x2C {
            self.x = self.x0;
            self.y = self.y0;
        }
    }

    fn data(&mut self, b: u8) {
        if self.cmd == Some(0x2C) {
            match self.half.take() {
                None => self.half = Some(b),
                Some(h) => self.pixel((h as u16) << 8 | b as u16),
            }
            return;
        }
        self.args.push(b);
        if self.args.len() == 4 && matches!(self.cmd, Some(0x2A) | Some(0x2B)) {
            let lo = (self.args[0] as u32) << 8 | self.args[1] as u32;
            let hi = (self.args[2] as u32) << 8 | self.args[3] as u32;
            if self.cmd == Some(0x2A) {
                self.x0 = lo;
                self.x1 = hi;
            } else {
                self.y0 = lo;
                self.y1 = hi;
            }
        }
    }

    fn pixel(&mut self, rgb565: u16) {
        if (self.x as usize) < LCD_W && (self.y as usize) < LCD_H {
            let i = (self.y as usize * LCD_W + self.x as usize) * 2;
            self.fb[i..i + 2].copy_from_slice(&rgb565.to_le_bytes());
        }
        self.x += 1;
        if self.x > self.x1 {
            self.x = self.x0;
            self.y += 1;
            if self.y > self.y1 {
                self.y = self.y0;
                self.frames += 1;
            }
        }
    }

    /// The screen as 8-bit RGB triples.
    pub fn rgb(&self) -> Vec<u8> {
        let mut out = Vec::with_capacity(LCD_W * LCD_H * 3);
        for px in self.fb.chunks_exact(2) {
            let v = u16::from_le_bytes([px[0], px[1]]) as u32;
            out.push(((v >> 11 & 0x1F) * 255 / 31) as u8);
            out.push(((v >> 5 & 0x3F) * 255 / 63) as u8);
            out.push(((v & 0x1F) * 255 / 31) as u8);
        }
        out
    }

    /// The screen as 0x00RRGGBB pixels (for windows).
    pub fn argb(&self, out: &mut [u32]) {
        for (px, o) in self.fb.chunks_exact(2).zip(out.iter_mut()) {
            let v = u16::from_le_bytes([px[0], px[1]]) as u32;
            let r = (v >> 11 & 0x1F) * 255 / 31;
            let g = (v >> 5 & 0x3F) * 255 / 63;
            let b = (v & 0x1F) * 255 / 31;
            *o = r << 16 | g << 8 | b;
        }
    }
}

// --- RTC chip ----------------------------------------------------------------

/// Low-power RTC behind the register bus at C0090000: 48-bit 32768 Hz
/// counter (0x30..0x35), staging (0x10..0x15), alarm (0x20..0x25), enables
/// (0x40, 0x50), battery-backed memory (0x80..).
#[derive(Clone)]
pub struct Rtc {
    pub regs: [u8; 256],
    pub base_ticks: i64,
    pub base_time: f64,
    pub data_out: u32,
}

impl Default for Rtc {
    fn default() -> Self {
        Rtc { regs: [0; 256], base_ticks: 0, base_time: 0.0, data_out: 0 }
    }
}

impl Rtc {
    pub fn alarm_ticks(&self) -> Option<i64> {
        if self.regs[0x40] & 6 == 0 {
            return None;
        }
        Some((0..6).map(|i| (self.regs[0x20 + i] as i64) << (8 * i)).sum())
    }

    pub fn ticks(&self, game_time: f64) -> i64 {
        self.base_ticks + ((game_time - self.base_time) * 32768.0) as i64
    }
}

// --- SoC clock ---------------------------------------------------------------

/// The SoC's own clock at C0040000: +0/+4/+8 seconds/minutes/hours (counting),
/// +0x54 status (write 1 clears), +0x58 enable; bit 1 = 1 Hz game tick.
#[derive(Clone, Default)]
pub struct RtcIrq {
    /// Game-time seconds count of the 1 Hz source already signalled.
    pub fired: u64,
    pub tod_base: i64,
    pub tod_set_at: f64,
}

pub const RTC_TICK_BIT: u32 = 0x2;
pub const RTC_TICK_PERIOD: f64 = 1.0;

impl RtcIrq {
    pub fn tod(&self, game_time: f64) -> i64 {
        ((self.tod_base as f64 + game_time - self.tod_set_at) as i64).rem_euclid(86400)
    }
}

// --- sound -------------------------------------------------------------------

/// Buzzer changes: (emulated time, frequency Hz (0 = off), duty).
#[derive(Clone)]
pub struct Sound {
    pub state: (f64, f64),
    pub events: Vec<(f64, f64, f64)>,
}

impl Default for Sound {
    fn default() -> Self {
        Sound { state: (0.0, 0.5), events: Vec::new() }
    }
}

// --- SPI flash ---------------------------------------------------------------

/// 8 MiB W25Q64-style NOR flash; its storage is the first 8 MiB of `ram`.
#[derive(Clone, Default)]
pub struct SpiFlash {
    pub cs: bool,
    pub buf: Vec<u8>,
    pub rx: u32,
    pub wel: bool,
    pub writes: u64,
}

pub const FLASH_JEDEC: [u8; 3] = [0xEF, 0x40, 0x17];

// --- the system --------------------------------------------------------------

/// Everything on the CPU's bus.
pub struct System {
    pub ram: Vec<u8>,
    pub sram: Vec<u8>,
    pub gpio: Vec<u8>,
    pub uart: Vec<u8>,
    pub uart_is_ram: bool,
    /// Zero pages mapped on demand for stray accesses (64 KiB each).
    pub extra: HashMap<u32, Vec<u8>>,
    pub unmapped: Vec<(u32, u32)>,
    pub regs: RegMap,

    pub now: f64,
    pub game_time: f64,
    pub turbo: f64,
    pub powered_off: bool,
    pub timers: Vec<Timer>,
    pub lcd: Lcd,
    pub rtc: Rtc,
    pub rtc_irq: RtcIrq,
    pub adc_converting: bool,
    pub sound: Sound,
    pub flash: SpiFlash,

    /// Scripted presses (time, key), each held KEY_HOLD.
    pub key_script: Vec<(f64, Key)>,
    /// Keys held down right now (window).
    pub live_keys: [bool; 3],
    /// A quick tap stays held until this time.
    pub hold_until: [f64; 3],
    pub gpio_applied: [u32; 8],

    pub mmio_writes: u64,
    /// Set by memory writes that change a value (idle detection).
    pub changed: bool,
    tracking: bool,
    write_log: Vec<(u32, u32)>,
    /// rand()'s 64-bit state (excluded from `changed`), as a CPU address.
    pub rand_state: u32,
    /// The run loop should stop after this instruction.
    pub stop: bool,
    pub infrared_armed: bool,
    pub infrared_used: bool,
    pub flash_first_use: bool,
    pub trace_mmio: bool,
    /// Lockstep replay: hardware reads return these recorded (addr, value)s.
    pub replay_reads: Option<std::collections::VecDeque<(u32, u32)>>,
    pub replay_error: Option<String>,
    seen_mmio: std::collections::HashSet<(u32, bool)>,
}

impl System {
    pub fn new(image: &[u8]) -> System {
        let mut ram = vec![0u8; RAM_SIZE];
        ram[..image.len()].copy_from_slice(image);
        System {
            ram,
            sram: vec![0; SRAM_SIZE],
            gpio: vec![0; GPIO_SIZE],
            uart: vec![0; UART_SIZE],
            uart_is_ram: false,
            extra: HashMap::new(),
            unmapped: Vec::new(),
            regs: RegMap::default(),
            now: 0.0,
            game_time: 0.0,
            turbo: 1.0,
            powered_off: false,
            timers: (0..TIMER_COUNT)
                .map(|n| Timer { base: TIMER_BASE + n as u32 * 0x20, ..Default::default() })
                .collect(),
            lcd: Lcd::default(),
            rtc: Rtc::default(),
            rtc_irq: RtcIrq::default(),
            adc_converting: false,
            sound: Sound::default(),
            flash: SpiFlash::default(),
            key_script: Vec::new(),
            live_keys: [false; 3],
            hold_until: [0.0; 3],
            gpio_applied: [0; 8],
            mmio_writes: 0,
            changed: false,
            tracking: false,
            write_log: Vec::new(),
            rand_state: 1,
            stop: false,
            infrared_armed: true,
            infrared_used: false,
            flash_first_use: true,
            trace_mmio: false,
            replay_reads: None,
            replay_error: None,
            seen_mmio: Default::default(),
        }
    }

    #[inline]
    pub fn reg(&self, addr: u32) -> u32 {
        self.regs.get(&addr).copied().unwrap_or(0)
    }

    pub fn sram_u32(&self, addr: u32) -> u32 {
        let o = (addr - SRAM_BASE) as usize;
        u32::from_le_bytes(self.sram[o..o + 4].try_into().unwrap())
    }

    pub fn sysclk(&self) -> f64 {
        match self.sram_u32(SYSCLK_VAR) {
            0 => 24_000_000.0,
            c => c as f64,
        }
    }

    // --- timers ---

    fn timer_rate(&self, n: usize) -> f64 {
        let clk = self.sysclk();
        if self.reg(self.timers[n].base) & 1 != 0 { clk / 256.0 } else { clk / 2.0 }
    }

    pub fn timer_period(&self, n: usize) -> u64 {
        let reload = self.reg(self.timers[n].base + 8) & 0xFFFF;
        if reload != 0 { 0x10000 - reload as u64 } else { 0x10000 }
    }

    fn timer_elapsed(&self, n: usize) -> u64 {
        let t = (self.now - self.timers[n].start) * self.timer_rate(n);
        if t > 0.0 { t as u64 } else { 0 }
    }

    fn timer_counter(&self, n: usize) -> u32 {
        let base = self.timers[n].base;
        if self.reg(base) & 0x2000 == 0 {
            return self.reg(base + 0x10);
        }
        let period = self.timer_period(n);
        ((0x10000 - period + self.timer_elapsed(n) % period) & 0xFFFF) as u32
    }

    fn timer_write_ctrl(&mut self, n: usize, val: u32, old: u32) {
        if val & 0x8000 != 0 {
            self.timers[n].pending = false;
        }
        if val & 0x2000 != 0 && old & 0x2000 == 0 {
            self.timers[n].start = self.now;
            self.timers[n].periods_seen = 0;
        }
        self.regs.insert(self.timers[n].base, val & !0x8000);
    }

    fn timer_advance(&mut self, n: usize) {
        let ctrl = self.reg(self.timers[n].base);
        if ctrl & 0x2000 == 0 {
            return;
        }
        let periods = self.timer_elapsed(n) / self.timer_period(n);
        if periods > self.timers[n].periods_seen {
            self.timers[n].periods_seen = periods;
            if ctrl & 0x4000 != 0 {
                self.timers[n].pending = true;
            }
        }
    }

    /// Recompute the timers' period counts after a snapshot restore.
    pub fn timers_resync(&mut self) {
        for n in 0..TIMER_COUNT {
            self.timers[n].periods_seen = if self.reg(self.timers[n].base) & 0x2000 != 0 {
                self.timer_elapsed(n) / self.timer_period(n)
            } else {
                0
            };
        }
    }

    fn sound_update(&mut self) {
        let base = self.timers[SOUND_TIMER].base;
        let state = if self.reg(base) & 0x2000 != 0 {
            let period = self.timer_period(SOUND_TIMER) as f64;
            let high = (0x10000 - (self.reg(base + 0x0C) & 0xFFFF)) as f64;
            (self.timer_rate(SOUND_TIMER) / period, (high / period).clamp(0.05, 0.95))
        } else {
            (0.0, 0.5)
        };
        if state != self.sound.state {
            self.sound.state = state;
            self.sound.events.push((self.now, state.0, state.1));
        }
    }

    // --- LCD ---

    fn lcd_write_ctrl(&mut self, val: u32) {
        if val & 1 == 0 {
            return;
        }
        let data = self.reg(LCD_BASE + 0x16C);
        match val & 0xF0 {
            0x80 => self.lcd.command(data as u8),
            0xA0 => {
                if val & 0x2000 != 0 {
                    self.lcd.data(data as u8);
                } else {
                    self.lcd.data((data >> 8) as u8);
                    self.lcd.data(data as u8);
                }
            }
            0xD0 => self.lcd_dma(self.reg(LCD_BASE + 0x33C)),
            _ => {}
        }
        self.regs.insert(LCD_BASE + 0x140, val & !1);
    }

    fn lcd_dma(&mut self, addr: u32) {
        if addr == 0 {
            return;
        }
        let n = self.lcd.fb.len();
        let mut fb = std::mem::take(&mut self.lcd.fb);
        if !self.read_block(addr, &mut fb[..n]) {
            eprintln!("[lcd] DMA from bad address {:08X}", addr);
        }
        self.lcd.fb = fb;
        self.regs.insert(LCD_BASE + 0x18C, self.reg(LCD_BASE + 0x18C) | 0x40);
        self.lcd.frames += 1;
    }

    fn lcd_irq_line(&self) -> bool {
        self.reg(LCD_BASE + 0x18C) & self.reg(LCD_BASE + 0x188) != 0
    }

    // --- RTC chip ---

    fn rtc_command(&mut self, cmd: u32) {
        let addr = (self.reg(RTC_BUS + 4) & 0xFF) as usize;
        if cmd & 1 != 0 {
            let data = (self.reg(RTC_BUS + 8) & 0xFF) as u8;
            self.rtc.regs[addr] = data;
            if addr == 0x15 {
                self.rtc.base_ticks = (0..6).map(|i| (self.rtc.regs[0x10 + i] as i64) << (8 * i)).sum();
                self.rtc.base_time = self.game_time;
            }
        } else if cmd & 2 != 0 {
            self.rtc.data_out = if (0x30..=0x35).contains(&addr) {
                ((self.rtc.ticks(self.game_time) >> ((addr - 0x30) * 8)) & 0xFF) as u32
            } else if addr == 0 {
                (self.rtc.regs[0] & !0x10) as u32
            } else {
                self.rtc.regs[addr] as u32
            };
        }
    }

    // --- SoC clock ---

    fn tod_read(&self, field: u32) -> u32 {
        let t = self.rtc_irq.tod(self.game_time);
        (match field {
            0 => t % 60,
            1 => t / 60 % 60,
            _ => t / 3600,
        }) as u32
    }

    fn tod_write(&mut self, field: u32, value: u32) {
        let t = self.rtc_irq.tod(self.game_time);
        let mut parts = [t % 60, t / 60 % 60, t / 3600];
        parts[field as usize] = value as i64;
        self.rtc_irq.tod_base = (parts[2] % 24) * 3600 + (parts[1] % 60) * 60 + parts[0] % 60;
        self.rtc_irq.tod_set_at = self.game_time;
    }

    fn rtc_irq_advance(&mut self) {
        let n = (self.game_time / RTC_TICK_PERIOD) as u64;
        if n > self.rtc_irq.fired {
            self.rtc_irq.fired = n;
            let s = self.reg(RTC_IRQ_BASE + 0x54) | RTC_TICK_BIT;
            self.regs.insert(RTC_IRQ_BASE + 0x54, s);
        }
    }

    fn rtc_irq_line(&self) -> bool {
        self.reg(RTC_IRQ_BASE + 0x54) & self.reg(RTC_IRQ_BASE + 0x58) != 0
    }

    // --- ADC ---

    fn adc_write_ctrl(&mut self, new: u32, old: u32) {
        let mut stored = new & !0x80B0;
        if old & 0x8000 != 0 && new & 0x8000 == 0 {
            stored |= old & 0x8080;
        }
        if new & 0x4000 != 0 && old & 0x4000 == 0 {
            self.adc_converting = true;
        }
        self.regs.insert(ADC_BASE + 4, stored);
    }

    fn adc_advance(&mut self) {
        if self.adc_converting {
            self.adc_converting = false;
            self.regs.insert(ADC_BASE + 8, ADC_BATTERY);
            let c = self.reg(ADC_BASE + 4) | 0x8080;
            self.regs.insert(ADC_BASE + 4, c);
        }
    }

    fn adc_irq_line(&self) -> bool {
        self.reg(ADC_BASE + 4) & 0x8040 == 0x8040
    }

    // --- SPI flash ---

    fn flash_addr(&self) -> usize {
        let b = &self.flash.buf;
        (((b[1] as usize) << 16) | ((b[2] as usize) << 8) | b[3] as usize) % FLASH_SIZE
    }

    fn flash_select(&mut self, on: bool) {
        if self.flash_first_use {
            self.flash_first_use = false;
            self.stop = true; // let the machine hook flash_write (see Machine::hook_flash_write)
        }
        if on && !self.flash.cs {
            self.flash.buf.clear();
        } else if !on && self.flash.cs {
            self.flash_finish();
        }
        self.flash.cs = on;
    }

    fn flash_xfer(&mut self, byte: u8) {
        self.flash.buf.push(byte);
        let cmd = self.flash.buf[0];
        let i = self.flash.buf.len() - 1;
        let rx: u8 = if i == 0 {
            0xFF
        } else {
            match cmd {
                0x9F => *FLASH_JEDEC.get(i - 1).unwrap_or(&0xFF),
                0x90 if i >= 4 => [0xEF, 0x16][(i - 4) & 1],
                0xAB if i >= 4 => 0x16,
                0x05 => {
                    if self.flash.wel { 0x02 } else { 0 }
                }
                0x35 | 0x15 => 0,
                0x03 if i >= 4 => self.ram[(self.flash_addr() + i - 4) % FLASH_SIZE],
                0x0B if i >= 5 => self.ram[(self.flash_addr() + i - 5) % FLASH_SIZE],
                0x4B if i >= 5 => 0x5A,
                _ => 0xFF,
            }
        };
        self.flash.rx = rx as u32;
    }

    fn flash_finish(&mut self) {
        if self.flash.buf.is_empty() {
            return;
        }
        let cmd = self.flash.buf[0];
        match cmd {
            0x06 => self.flash.wel = true,
            0x04 => self.flash.wel = false,
            0x02 if self.flash.buf.len() > 4 && self.flash.wel => {
                let addr = self.flash_addr();
                let page = addr & !0xFF;
                let mut off = addr & 0xFF;
                for k in 4..self.flash.buf.len() {
                    let b = self.flash.buf[k];
                    self.ram[page + off] &= b; // NOR program: 1 -> 0 only
                    off = (off + 1) & 0xFF;
                }
                self.flash.writes += 1;
                self.flash.wel = false;
            }
            0x20 | 0x52 | 0xD8 if self.flash.buf.len() >= 4 && self.flash.wel => {
                let size = match cmd {
                    0x20 => 0x1000,
                    0x52 => 0x8000,
                    _ => 0x10000,
                };
                let start = self.flash_addr() & !(size - 1);
                self.ram[start..start + size].fill(0xFF);
                self.flash.writes += 1;
                self.flash.wel = false;
            }
            0xC7 | 0x60 if self.flash.wel => {
                self.ram[..FLASH_SIZE].fill(0xFF);
                self.flash.wel = false;
            }
            _ => {}
        }
    }

    // --- interrupts, time, buttons ---

    fn irq_lines(&self) -> [bool; 4] {
        [
            self.rtc_irq_line(),
            self.timers.iter().any(|t| t.pending),
            self.lcd_irq_line(),
            self.adc_irq_line(),
        ]
    }

    /// Highest-priority unmasked pending IRQ number (1..32), 0 = none.
    pub fn pending_irq(&self) -> u32 {
        let mask = self.reg(INTC_MASK);
        let lines = self.irq_lines();
        for (n, on) in [RTC_IRQ, TIMER_IRQ, LCD_IRQ, ADC_IRQ].iter().zip(lines) {
            if on && (mask >> (32 - n)) & 1 == 0 {
                return *n;
            }
        }
        0
    }

    pub fn irq_wanted(&self) -> bool {
        self.reg(INTC_DISABLE) & 1 == 0 && self.pending_irq() != 0
    }

    /// Bring the hardware up to emulated time `now`.
    pub fn advance(&mut self, now: f64) {
        self.game_time += (now - self.now) * self.turbo;
        self.now = now;
        for n in 0..TIMER_COUNT {
            self.timer_advance(n);
        }
        let v = (now * VSYNC_HZ) as u64;
        if v > self.lcd.last_vsync {
            self.lcd.last_vsync = v;
            let s = self.reg(LCD_BASE + 0x18C) | 0x2000;
            self.regs.insert(LCD_BASE + 0x18C, s);
        }
        self.adc_advance();
        self.rtc_irq_advance();
        self.sync_gpio();
    }

    /// Which keys are down at `now` (scripted, live, or a tap still held).
    pub fn keys_held(&self) -> [bool; 3] {
        let now = self.now;
        let mut held = self.live_keys;
        for k in Key::all() {
            if now < self.hold_until[k.index()] {
                held[k.index()] = true;
            }
        }
        for &(t, k) in &self.key_script {
            if t <= now && now < t + KEY_HOLD {
                held[k.index()] = true;
            }
        }
        held
    }

    fn sync_gpio(&mut self) {
        let held = self.keys_held();
        let mut want = [0u32; 8];
        for k in Key::all() {
            if held[k.index()] {
                let pin = key_pin(k);
                want[(pin >> 4) as usize] |= 1 << (pin & 15);
            }
        }
        for port in 0..8 {
            if want[port] == self.gpio_applied[port] {
                continue;
            }
            let o = port * 0x20;
            let val = u32::from_le_bytes(self.gpio[o..o + 4].try_into().unwrap());
            let val = (val & !self.gpio_applied[port]) | want[port];
            self.gpio[o..o + 4].copy_from_slice(&val.to_le_bytes());
            self.gpio_applied[port] = want[port];
        }
    }

    /// Emulated time of the next thing that can change what the CPU sees.
    pub fn next_event(&self) -> f64 {
        let now = self.now;
        if self.adc_converting || self.irq_wanted() {
            return now;
        }
        let mut t = (self.lcd.last_vsync + 1) as f64 / VSYNC_HZ;
        for n in 0..TIMER_COUNT {
            let tm = &self.timers[n];
            if self.reg(tm.base) & 0x6000 == 0x6000 {
                if tm.pending {
                    return now;
                }
                let next = tm.start + (tm.periods_seen + 1) as f64 * self.timer_period(n) as f64 / self.timer_rate(n);
                t = t.min(next);
            }
        }
        let game_next = (self.rtc_irq.fired + 1) as f64 * RTC_TICK_PERIOD;
        t = t.min(now + (game_next - self.game_time) / self.turbo);
        for &(kt, _) in &self.key_script {
            for edge in [kt, kt + KEY_HOLD] {
                if edge > now {
                    t = t.min(edge);
                }
            }
        }
        for &h in &self.hold_until {
            if h > now {
                t = t.min(h);
            }
        }
        t
    }

    // --- MMIO dispatch ---

    fn is_mmio(addr: u32) -> bool {
        (0xC000_1000..0xC100_0000).contains(&addr)
            || (0xD000_0000..0xD100_0000).contains(&addr)
            || (0xFF00_0000..0xFFFF_F000).contains(&addr)
    }

    fn uart_ir_access(&mut self, addr: u32) {
        if self.infrared_armed && (UART_BASE..UART_BASE + UART_SIZE as u32).contains(&addr)
            && UART_IR_REGS.contains(&((addr - UART_BASE) & !3))
        {
            self.infrared_armed = false;
            self.infrared_used = true;
            self.stop = true;
        }
    }

    fn log_mmio(&mut self, addr: u32, write: bool, val: u32) {
        if self.seen_mmio.insert((addr, write)) {
            println!("  [mmio] {} {:08X} = {:08X}", if write { 'W' } else { 'R' }, addr, val);
        }
    }

    fn mmio_read(&mut self, addr: u32, size: u32) -> u32 {
        if let Some(q) = self.replay_reads.as_mut() {
            return match q.pop_front() {
                Some((a, v)) if a == addr => v,
                other => {
                    if self.replay_error.is_none() {
                        self.replay_error = Some(format!("hardware read of {:08X}, trace has {:X?}", addr, other));
                    }
                    0
                }
            };
        }
        self.uart_ir_access(addr);
        let word = addr & !3;
        let old = self.reg(word);
        let val = match word {
            CLOCK_STATUS => {
                if self.reg(CLOCK_SWITCH) & 0x8000 != 0 { 2 } else { 1 }
            }
            0xC015_0004 => old & !0x8,
            0xC015_000C => self.flash.rx,
            INTC_PENDING => self.pending_irq(),
            INTC_PENDING_FIQ => 0,
            0xC008_0010 => {
                let c = self.reg(SPI2_BASE + 0x0C) & !7;
                self.regs.insert(SPI2_BASE + 0x0C, c);
                old
            }
            0xC004_0000 | 0xC004_0004 | 0xC004_0008 => self.tod_read((word - RTC_IRQ_BASE) / 4),
            0xC009_0010 => 1,
            0xC009_0014 => self.rtc.data_out,
            _ => {
                let off = word.wrapping_sub(TIMER_BASE);
                if off < (TIMER_COUNT as u32) * 0x20 && off % 0x20 == 0 {
                    let n = (off / 0x20) as usize;
                    (old & !0x8000) | if self.timers[n].pending { 0x8000 } else { 0 }
                } else if off < (TIMER_COUNT as u32) * 0x20 && off % 0x20 == 0x10 {
                    self.timer_counter((off / 0x20) as usize)
                } else {
                    old
                }
            }
        };
        let val = (val >> ((addr & 3) * 8)) & if size == 4 { u32::MAX } else { (1 << (size * 8)) - 1 };
        if self.trace_mmio {
            self.log_mmio(addr, false, val);
        }
        val
    }

    fn mmio_write(&mut self, addr: u32, size: u32, val: u32) {
        self.uart_ir_access(addr);
        if self.trace_mmio {
            self.log_mmio(addr, true, val);
        }
        let word = addr & !3;
        let sh = (addr & 3) * 8;
        let mask = if size == 4 { u32::MAX } else { ((1u32 << (size * 8)) - 1) << sh };
        self.mmio_writes += 1;
        let old = self.reg(word);
        let new = (old & !mask) | ((val << sh) & mask);
        let off = word.wrapping_sub(TIMER_BASE);
        if off < (TIMER_COUNT as u32) * 0x20 && off % 0x20 == 0 {
            let n = (off / 0x20) as usize;
            self.timer_write_ctrl(n, new, old);
            if n == SOUND_TIMER {
                self.sound_update();
            }
            return;
        }
        match word {
            0xD050_018C => {
                self.regs.insert(word, old & !new);
                return;
            }
            0xC00C_0004 => {
                self.adc_write_ctrl(new, old);
                return;
            }
            0xC004_0054 => {
                self.regs.insert(word, old & !((val << sh) & mask));
                return;
            }
            0xC004_0000 | 0xC004_0004 | 0xC004_0008 => {
                self.tod_write((word - RTC_IRQ_BASE) / 4, new);
                return;
            }
            _ => {}
        }
        self.regs.insert(word, new);
        match word {
            POWER_REG => {
                if new & 1 == 0 {
                    self.powered_off = true;
                    self.stop = true;
                }
            }
            0xC015_0000 => self.flash_select(new & 0x40 != 0),
            0xC015_0008 => self.flash_xfer(new as u8),
            0xC008_0008 => {
                self.regs.insert(SPI2_BASE + 0x10, SPI2_IDLE);
                let c = self.reg(SPI2_BASE + 0x0C) | 1;
                self.regs.insert(SPI2_BASE + 0x0C, c);
            }
            0xD050_0140 => self.lcd_write_ctrl(new),
            0xC009_000C => self.rtc_command(new),
            0xC002_0088 | 0xC002_008C => self.sound_update(),
            _ => {}
        }
    }

    // --- plain memory ---

    /// The backing bytes for a plain-memory address, if it is one.
    #[inline]
    fn plain(&mut self, addr: u32) -> Option<(&mut Vec<u8>, usize)> {
        match addr >> 24 {
            0x20 => Some((&mut self.ram, (addr - FLASH_BASE) as usize)),
            0xF8 if addr < SRAM_BASE + SRAM_SIZE as u32 => Some((&mut self.sram, (addr - SRAM_BASE) as usize)),
            0xC0 if addr < GPIO_BASE + GPIO_SIZE as u32 => Some((&mut self.gpio, (addr - GPIO_BASE) as usize)),
            0xC0 if self.uart_is_ram && (UART_BASE..UART_BASE + UART_SIZE as u32).contains(&addr) => {
                Some((&mut self.uart, (addr - UART_BASE) as usize))
            }
            _ => {
                if Self::is_mmio(addr) {
                    return None;
                }
                let page = addr & !0xFFFF;
                if !self.extra.contains_key(&page) {
                    if self.unmapped.len() < 64 {
                        eprintln!("[!] unmapped access {:08X}, mapping zero page", addr);
                    }
                    self.unmapped.push((addr, 0));
                    self.extra.insert(page, vec![0; 0x10000]);
                }
                Some((self.extra.get_mut(&page).unwrap(), (addr & 0xFFFF) as usize))
            }
        }
    }

    /// Copy memory into buf; false if part of it is not plain memory.
    pub fn read_block(&mut self, addr: u32, buf: &mut [u8]) -> bool {
        match self.plain(addr) {
            Some((mem, o)) if o + buf.len() <= mem.len() => {
                buf.copy_from_slice(&mem[o..o + buf.len()]);
                true
            }
            _ => {
                for (i, b) in buf.iter_mut().enumerate() {
                    *b = self.peek8(addr.wrapping_add(i as u32));
                }
                false
            }
        }
    }

    /// Read without side effects (MMIO reads as its stored value).
    pub fn peek8(&mut self, addr: u32) -> u8 {
        match self.plain(addr) {
            Some((m, o)) => m[o],
            None => (self.reg(addr & !3) >> ((addr & 3) * 8)) as u8,
        }
    }

    pub fn peek32(&mut self, addr: u32) -> u32 {
        u32::from_le_bytes([self.peek8(addr), self.peek8(addr + 1), self.peek8(addr + 2), self.peek8(addr + 3)])
    }

    pub fn poke(&mut self, addr: u32, data: &[u8]) {
        for (i, b) in data.iter().enumerate() {
            if let Some((m, o)) = self.plain(addr.wrapping_add(i as u32)) {
                m[o] = *b;
            }
        }
    }

    /// Make the infrared port plain memory ("idle, nothing received").
    pub fn uart_to_ram(&mut self) {
        if self.uart_is_ram {
            return;
        }
        for off in (0..0x40u32).step_by(4) {
            if !UART_IR_REGS.contains(&off) {
                let v = self.reg(UART_BASE + off);
                self.uart[off as usize..off as usize + 4].copy_from_slice(&v.to_le_bytes());
            }
        }
        self.uart_is_ram = true;
        self.infrared_armed = false;
    }

    /// Start logging memory writes (idle detection): the old value of each
    /// written word is kept, so `writes_undone` can tell whether memory
    /// ends up as it was, however often it changed in between.
    pub fn track_writes(&mut self) {
        self.write_log.clear();
        self.changed = false;
        self.tracking = true;
    }

    /// True if every word written since `track_writes` holds its old value
    /// again (rand()'s state excepted). Stops tracking.
    pub fn writes_undone(&mut self) -> bool {
        self.tracking = false;
        if self.changed || self.write_log.len() >= WRITE_LOG_MAX {
            return false;
        }
        let log = std::mem::take(&mut self.write_log);
        let mut seen: Vec<u32> = Vec::with_capacity(log.len());
        let mut same = true;
        for &(addr, old) in &log {
            if seen.contains(&addr) {
                continue; // the first entry holds the value before the pass
            }
            seen.push(addr);
            if addr.wrapping_sub(self.rand_state & !3) < 8 {
                continue;
            }
            if self.peek_word(addr) != old {
                same = false;
                break;
            }
        }
        self.write_log = log;
        same
    }

    fn peek_word(&mut self, addr: u32) -> u32 {
        match self.plain(addr) {
            Some((m, o)) => u32::from_le_bytes([m[o], m[o + 1], m[o + 2], m[o + 3]]),
            None => 0,
        }
    }

    #[inline(never)]
    fn log_write(&mut self, addr: u32) {
        if self.tracking && self.write_log.len() < WRITE_LOG_MAX {
            let w = addr & !3;
            let old = self.peek_word(w);
            self.write_log.push((w, old));
        }
    }
}

/// Most writes logged in one idle probe; a pass writing more is not idle.
const WRITE_LOG_MAX: usize = 4096;

const SRAM_END: u32 = SRAM_BASE + SRAM_SIZE as u32;

impl System {
    #[inline(never)]
    fn slow_read(&mut self, addr: u32, size: u32) -> u32 {
        match self.plain(addr) {
            Some((m, o)) if o + size as usize <= m.len() => {
                let mut v = 0u32;
                for i in (0..size as usize).rev() {
                    v = v << 8 | m[o + i] as u32;
                }
                v
            }
            Some(_) => 0,
            None => self.mmio_read(addr, size),
        }
    }

    #[inline(never)]
    fn slow_write(&mut self, addr: u32, size: u32, val: u32) {
        match self.plain(addr) {
            Some((m, o)) if o + size as usize <= m.len() => {
                for i in 0..size as usize {
                    m[o + i] = (val >> (8 * i)) as u8;
                }
            }
            Some(_) => {}
            None => self.mmio_write(addr, size, val),
        }
    }

    /// RAM or SRAM: the backing bytes and offset (the fast path).
    #[inline(always)]
    fn fast(&mut self, addr: u32) -> Option<(&mut [u8], usize)> {
        if addr >> 24 == 0x20 {
            Some((&mut self.ram[..], (addr & 0x00FF_FFFF) as usize))
        } else if (SRAM_BASE..SRAM_END).contains(&addr) {
            Some((&mut self.sram[..], (addr - SRAM_BASE) as usize))
        } else {
            None
        }
    }
}

impl Bus for System {
    #[inline(always)]
    fn read8(&mut self, addr: u32) -> u8 {
        match self.fast(addr) {
            Some((m, o)) => m[o],
            None => self.slow_read(addr, 1) as u8,
        }
    }

    #[inline(always)]
    fn read16(&mut self, addr: u32) -> u16 {
        match self.fast(addr) {
            Some((m, o)) if o + 2 <= m.len() => u16::from_le_bytes([m[o], m[o + 1]]),
            _ => self.slow_read(addr, 2) as u16,
        }
    }

    #[inline(always)]
    fn read32(&mut self, addr: u32) -> u32 {
        match self.fast(addr) {
            Some((m, o)) if o + 4 <= m.len() => u32::from_le_bytes([m[o], m[o + 1], m[o + 2], m[o + 3]]),
            _ => self.slow_read(addr, 4),
        }
    }

    #[inline(always)]
    fn write8(&mut self, addr: u32, val: u8) {
        if self.tracking {
            self.log_write(addr);
        }
        match self.fast(addr) {
            Some((m, o)) => m[o] = val,
            None => self.slow_write(addr, 1, val as u32),
        }
    }

    #[inline(always)]
    fn write16(&mut self, addr: u32, val: u16) {
        if self.tracking {
            self.log_write(addr);
        }
        match self.fast(addr) {
            Some((m, o)) if o + 2 <= m.len() => m[o..o + 2].copy_from_slice(&val.to_le_bytes()),
            _ => self.slow_write(addr, 2, val as u32),
        }
    }

    #[inline(always)]
    fn write32(&mut self, addr: u32, val: u32) {
        if self.tracking {
            self.log_write(addr);
        }
        match self.fast(addr) {
            Some((m, o)) if o + 4 <= m.len() => m[o..o + 4].copy_from_slice(&val.to_le_bytes()),
            _ => self.slow_write(addr, 4, val),
        }
    }
}
