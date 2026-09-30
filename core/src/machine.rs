//! The whole toy: CPU + bus, the run loop with interrupts and idle skipping,
//! the firmware shortcuts (HLE) and deep sleep.

use crate::cpu::{Bus, Cpu, Fault, FLAG_I, FLAG_T, MODE_IRQ};
use crate::sigs::{self, Pattern};
use crate::sys::{Key, System, LCD_H, LCD_W};
use crate::*;

pub const CPU_HZ: f64 = 96_000_000.0;
pub const RESET_PC: u32 = 0x2000_0048;
pub const IRQ_VECTOR: u32 = 0x2000_03C4;
/// Erased flash: where firmware calls made by the emulator stop.
pub const CALL_RETURN: u32 = 0x207F_FFF0;

/// Instructions per slice between hardware updates (~10 us).
const SLICE: u64 = 1_000;
/// Longest loop pass looked at by the idle detector.
const IDLE_PROBE: u64 = 10_000;
const IDLE_PASSES: usize = 3;
/// Slices to wait after a failed idle probe.
const IDLE_BACKOFF: u32 = 20;
/// A run that stays within one 256-byte window for this long without flash
/// or screen progress is stuck (~2 s).
const STUCK_INSNS: u64 = 200_000_000;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Outcome {
    /// Reached the instruction limit.
    Limit,
    /// The firmware cut the power (deep sleep).
    PowerOff,
    /// The game started Bluetooth (with `stop_on_bluetooth`).
    Bluetooth,
    /// The game started infrared (with `stop_on_bluetooth`).
    Infrared,
    /// The firmware hung after "BLE Initial Fail".
    BleFail,
    Crash,
    Stuck,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Hook {
    Printf,
    Spi2Transfer,
    BleFail,
    FlashWrite,
    SaveLoop,
    Rand,
    Spin,
}

pub struct Machine {
    pub cpu: Cpu,
    pub sys: System,
    /// The ROM as dumped (never written).
    pub image: std::sync::Arc<Vec<u8>>,
    /// Instructions executed (or skipped) since reset: emulated time.
    pub executed: u64,
    pub irqs: u64,
    pub idle_skip: bool,
    pub idle_skipped: u64,
    pub never_sleep: bool,
    idle_reset_at: f64,
    pub idle_counter: Option<u32>,
    clock_refresh: Option<u32>,
    clock_minute: Option<i64>,
    pub stop_on_bluetooth: bool,
    pub bluetooth_used: bool,
    pub ble_failed: bool,
    pub log: Vec<String>,
    pub print_log: bool,
    hooks: Vec<(u32, Hook)>,
    hook_pages: Vec<u64>,
    save_check: [u32; 3],
    save_loop: u32,
    pub flash_hle: Option<u32>,
    spin_skipped: u64,
    probe_wait: u32,
    /// Idle probe statistics: [tried, no repeat, memory changed, hardware written, skipped].
    pub probe_stats: [u64; 5],
}

/// Hook bitmap: a bit per halfword of RAM (16 MiB) and then SRAM (1 MiB).
const HOOK_BITS: usize = (RAM_SIZE + SRAM_SIZE) / 2;

#[inline(always)]
fn hook_bit(pc: u32) -> Option<usize> {
    if pc >> 24 == 0x20 {
        Some(((pc & 0x00FF_FFFF) >> 1) as usize)
    } else if pc.wrapping_sub(SRAM_BASE) < SRAM_SIZE as u32 {
        Some(RAM_SIZE / 2 + ((pc - SRAM_BASE) >> 1) as usize)
    } else {
        None
    }
}

impl Machine {
    /// A machine booting `flash` (the ROM, or a saved flash image) with the
    /// RTC set to `rtc_seconds` since 2007-12-31.
    pub fn new(image: std::sync::Arc<Vec<u8>>, flash: &[u8], rtc_seconds: f64) -> Machine {
        let mut sys = System::new(flash);
        sys.rtc.base_ticks = (rtc_seconds * 32768.0) as i64;
        let mut cpu = Cpu::new();
        cpu.r[15] = RESET_PC;
        let mut m = Machine {
            cpu,
            sys,
            image: image.clone(),
            executed: 0,
            irqs: 0,
            idle_skip: true,
            idle_skipped: 0,
            never_sleep: false,
            idle_reset_at: 0.0,
            idle_counter: None,
            clock_refresh: None,
            clock_minute: None,
            stop_on_bluetooth: false,
            bluetooth_used: false,
            ble_failed: false,
            log: Vec::new(),
            print_log: false,
            hooks: Vec::new(),
            hook_pages: vec![0; HOOK_BITS / 64],
            save_check: [0; 3],
            save_loop: 0,
            flash_hle: None,
            spin_skipped: 0,
            probe_wait: 0,
            probe_stats: [0; 5],
        };
        let img = &image[..];
        m.idle_counter = sigs::find_idle_counter(img);
        m.clock_refresh = sigs::find_code(img, sigs::CLOCK_REFRESH_SIG, sigs::CODE_END);
        if let Some(a) = sigs::find_code(img, sigs::PRINTF_STUB, 0x200000) {
            m.add_hook(a, Hook::Printf);
        }
        if let Some(a) = sigs::find_code(img, sigs::SPI2_XFER_SIG, sigs::CODE_END) {
            m.add_hook(a, Hook::Spi2Transfer);
        }
        if let Some(a) = sigs::find_ble_fail_loop(img) {
            m.add_hook(a, Hook::BleFail);
        }
        if let Some(a) = sigs::find_code(img, sigs::RAND_SIG, 0x200000) {
            m.add_hook(a, Hook::Rand);
        }
        for o in Pattern::parse(sigs::SPIN_LOOP).find_all(img, 0, sigs::CODE_END) {
            m.add_hook(FLASH_BASE + o as u32, Hook::Spin);
        }
        m
    }

    fn add_hook(&mut self, addr: u32, kind: Hook) {
        self.hooks.push((addr, kind));
        self.rebuild_hook_pages();
    }

    fn remove_hook(&mut self, kind: Hook) {
        self.hooks.retain(|h| h.1 != kind);
        self.rebuild_hook_pages();
    }

    fn rebuild_hook_pages(&mut self) {
        self.hook_pages.fill(0);
        for &(a, _) in &self.hooks {
            if let Some(bit) = hook_bit(a) {
                self.hook_pages[bit >> 6] |= 1 << (bit & 63);
            }
        }
    }

    /// Is there a hook at pc? (one bit per halfword of RAM and SRAM)
    #[inline(always)]
    fn hooked_page(&self, pc: u32) -> bool {
        match hook_bit(pc) {
            Some(bit) => self.hook_pages[bit >> 6] >> (bit & 63) & 1 != 0,
            None => false,
        }
    }

    pub fn now(&self) -> f64 {
        self.executed as f64 / CPU_HZ
    }

    pub fn press(&mut self, at: f64, key: Key) {
        self.sys.key_script.push((at, key));
    }

    // --- hooks ---------------------------------------------------------------

    /// Run the hook at pc, if any. Returns true if it replaced the
    /// instruction (the CPU state was changed instead of executing it).
    fn run_hook(&mut self, pc: u32) -> bool {
        let kind = match self.hooks.iter().find(|h| h.0 == pc) {
            Some(h) => h.1,
            None => return false,
        };
        match kind {
            Hook::Printf => {
                self.on_printf();
                false
            }
            Hook::Rand => {
                self.sys.rand_state = self.cpu.r[0].wrapping_add(sigs::RAND_STATE_OFF);
                self.remove_hook(Hook::Rand);
                false
            }
            Hook::Spin => {
                let n = self.cpu.r[3];
                if n > 1 {
                    self.cpu.r[3] = 1;
                    self.spin_skipped += (n as u64 - 1) * sigs::SPIN_LOOP_INSNS;
                }
                false
            }
            Hook::BleFail => {
                self.ble_failed = true;
                self.sys.stop = true;
                true
            }
            Hook::Spi2Transfer => {
                if !self.bluetooth_used {
                    // nothing in normal play touches this bus: the game is
                    // starting Bluetooth; stop so the window can undo the press
                    self.bluetooth_used = true;
                    if self.stop_on_bluetooth {
                        self.sys.stop = true;
                        return true;
                    }
                }
                let (txlen, rx, sp, lr) = (self.cpu.r[2], self.cpu.r[3], self.cpu.r[13], self.cpu.r[14]);
                let rxlen = self.sys.peek32(sp).min(txlen);
                if rx != 0 && rxlen != 0 {
                    let fill = vec![sys::SPI2_IDLE as u8; rxlen as usize];
                    self.sys.poke(rx, &fill);
                }
                self.cpu.r[0] = 0;
                self.return_to(lr);
                true
            }
            Hook::FlashWrite => {
                let (addr, buf, n, lr) = (self.cpu.r[0], self.cpu.r[1], self.cpu.r[2], self.cpu.r[14]);
                if n != 0 {
                    let off = (addr & 0xFF_FFFF) as usize % FLASH_SIZE;
                    let n = (n as usize).min(FLASH_SIZE - off);
                    self.program_flash(off, buf, n);
                    self.sys.flash.writes += 1;
                }
                self.cpu.r[0] = 0;
                self.return_to(lr);
                true
            }
            Hook::SaveLoop => self.save_loop_hle(),
        }
    }

    /// Return like `bx lr`.
    fn return_to(&mut self, lr: u32) {
        if lr & 1 != 0 {
            self.cpu.cpsr |= FLAG_T;
            self.cpu.r[15] = lr & !1;
        } else {
            self.cpu.cpsr &= !FLAG_T;
            self.cpu.r[15] = lr & !3;
        }
    }

    /// NOR programming: bits only go from 1 to 0.
    fn program_flash(&mut self, off: usize, src: u32, n: usize) {
        let mut data = vec![0u8; n];
        self.sys.read_block(src, &mut data);
        for (d, s) in self.sys.ram[off..off + n].iter_mut().zip(&data) {
            *d &= *s;
        }
        self.sys.changed = true;
    }

    fn save_loop_hle(&mut self) -> bool {
        let dst = self.cpu.r[8];
        let count = self.cpu.r[9];
        let src = self.cpu.r[11];
        let [mask, offset, limit] = self.save_check;
        let allowed = |a: u32| (a & mask).wrapping_add(offset) <= limit;
        if count == 0 || dst & 1 != 0 || !(allowed(dst) && allowed(dst.wrapping_add(2 * (count - 1)))) {
            return false; // let the firmware do (and reject) it
        }
        let n = 2 * count as usize;
        let off = (dst & 0xFF_FFFF) as usize % FLASH_SIZE;
        if off + n > FLASH_SIZE {
            return false;
        }
        self.program_flash(off, src, n);
        self.sys.flash.writes += count as u64;
        let r = &mut self.cpu.r;
        r[0] = 0;
        r[4] = dst.wrapping_add(n as u32);
        r[5] = count;
        r[6] = 0;
        r[7] = src.wrapping_sub(dst);
        self.cpu.cpsr |= FLAG_T;
        self.cpu.r[15] = self.save_loop + sigs::SAVE_LOOP_EXIT;
        true
    }

    /// Hook the SRAM copy of flash_write and the save loop, once the boot
    /// code has put them there (called on the first flash command).
    pub fn hook_flash_write(&mut self) {
        if self.flash_hle.is_some() {
            return;
        }
        let pat = Pattern::parse(sigs::FLASH_WRITE_SIG);
        let off = match pat.find(&self.sys.sram, 0, SRAM_SIZE) {
            Some(o) => o,
            None => {
                self.flash_hle = Some(0); // not there (yet): leave it emulated
                return;
            }
        };
        let addr = SRAM_BASE + off as u32;
        self.flash_hle = Some(addr);
        self.add_hook(addr, Hook::FlashWrite);
        self.hook_save_loop();
    }

    fn hook_save_loop(&mut self) {
        let pat = Pattern::parse(sigs::SAVE_LOOP_SIG);
        let off = match pat.find(&self.sys.sram, 0, SRAM_SIZE) {
            Some(o) => o,
            None => return,
        };
        let s = &self.sys.sram;
        let hi = u16::from_le_bytes([s[off + 0x10], s[off + 0x11]]);
        let lo = u16::from_le_bytes([s[off + 0x12], s[off + 0x13]]);
        let loop_addr = SRAM_BASE + off as u32;
        let func = match sigs::thumb_bl_target(hi, lo, loop_addr + 0x10) {
            Some(f) => f,
            None => return,
        };
        let mut consts = Vec::new();
        let mut pc = func;
        while pc < func + 24 {
            let ins = self.sys.peek8(pc) as u32 | (self.sys.peek8(pc + 1) as u32) << 8;
            if ins >> 11 == 0b01001 {
                let lit = ((pc + 4) & !3) + (ins & 0xFF) * 4;
                consts.push(self.sys.peek32(lit));
            }
            pc += 2;
        }
        if consts.len() < 3 {
            return;
        }
        self.save_check = [consts[0], consts[1], consts[2]];
        self.save_loop = loop_addr;
        self.add_hook(loop_addr, Hook::SaveLoop);
    }

    fn on_printf(&mut self) {
        let fmt_addr = self.cpu.r[0];
        let mut args = vec![self.cpu.r[1], self.cpu.r[2], self.cpu.r[3]];
        let sp = self.cpu.r[13];
        for i in 0..8 {
            args.push(self.sys.peek32(sp + 4 * i));
        }
        let fmt = self.read_cstr(fmt_addr, 256);
        let msg = c_printf(self, &fmt, &args);
        let msg = msg.trim_end_matches(['\r', '\n']).to_string();
        if self.print_log {
            println!("[fw] {}", msg);
        }
        if self.log.len() < 10_000 {
            self.log.push(msg);
        }
    }

    pub fn read_cstr(&mut self, addr: u32, limit: usize) -> String {
        let mut out = Vec::new();
        for i in 0..limit as u32 {
            let b = self.sys.peek8(addr.wrapping_add(i));
            if b == 0 {
                break;
            }
            out.push(b);
        }
        out.iter().map(|&b| b as char).collect()
    }

    // --- running ---------------------------------------------------------------

    fn enter_irq(&mut self) {
        self.cpu.enter_irq(IRQ_VECTOR);
        self.irqs += 1;
    }

    /// Execute up to n instructions, stopping early at `watch` (a PC) or when
    /// something needs the run loop's attention. Returns instructions run.
    #[inline]
    fn exec(&mut self, n: u64, watch: u32) -> u64 {
        let mut done = 0;
        while done < n {
            let pc = self.cpu.r[15];
            if done > 0 && pc == watch {
                break;
            }
            if self.hooked_page(pc) && self.run_hook(pc) {
                done += 1;
                if self.sys.stop {
                    break;
                }
                continue;
            }
            self.cpu.step(&mut self.sys);
            done += 1;
            if self.sys.stop || self.cpu.fault.is_some() {
                break;
            }
        }
        done
    }

    /// Run until `limit` instructions have been executed (emulated time).
    pub fn run(&mut self, limit: u64) -> Outcome {
        let mut last_window = (0u32, 0u64, 0u64);
        let mut window_since = self.executed;
        while self.executed < limit {
            let now = self.now();
            self.sys.advance(now);
            if self.never_sleep && now - self.idle_reset_at >= 1.0 {
                if let Some(a) = self.idle_counter {
                    self.sys.poke(a, &[0]); // the counter only rises once a second
                }
                self.idle_reset_at = now;
            }
            if self.sys.irq_wanted() && self.cpu.cpsr & FLAG_I == 0 {
                self.enter_irq();
            }
            let n = SLICE.min(limit - self.executed);
            let ran = self.exec(n, 1);
            self.executed += ran + self.spin_skipped;
            self.spin_skipped = 0;
            if let Some(out) = self.check_stop() {
                return out;
            }
            if self.idle_skip && self.executed < limit {
                if self.probe_wait > 0 {
                    self.probe_wait -= 1;
                } else if self.try_idle_skip(limit) {
                    window_since = self.executed;
                    if self.never_sleep {
                        self.refresh_clock();
                    }
                    continue;
                } else {
                    if let Some(out) = self.check_stop() {
                        return out;
                    }
                    self.probe_wait = IDLE_BACKOFF;
                }
            }
            // a stretch counts as progress if the flash or LCD did something,
            // so long erase loops are not mistaken for a hang
            let window = (self.cpu.r[15] & !0xFF, self.sys.flash.writes, self.sys.lcd.frames);
            if window != last_window {
                last_window = window;
                window_since = self.executed;
            } else if self.executed - window_since >= STUCK_INSNS {
                eprintln!("[?] execution parked around {:08X}", self.cpu.r[15]);
                return Outcome::Stuck;
            }
        }
        Outcome::Limit
    }

    fn check_stop(&mut self) -> Option<Outcome> {
        if let Some(f) = self.cpu.fault {
            eprintln!("[X] CPU fault {:?} lr={:08X}", f, self.cpu.r[14]);
            return Some(Outcome::Crash);
        }
        if !self.sys.stop {
            return None;
        }
        self.sys.stop = false;
        if self.sys.powered_off {
            return Some(Outcome::PowerOff);
        }
        if self.ble_failed {
            return Some(Outcome::BleFail);
        }
        if self.bluetooth_used && self.stop_on_bluetooth {
            return Some(Outcome::Bluetooth);
        }
        if self.sys.infrared_used && !self.sys.uart_is_ram {
            if self.stop_on_bluetooth {
                return Some(Outcome::Infrared);
            }
            self.sys.uart_to_ram();
        }
        if self.flash_hle.is_none() {
            self.hook_flash_write();
        }
        None
    }

    fn idle_regs(&self) -> [u32; 17] {
        let mut r = [0u32; 17];
        r[..16].copy_from_slice(&self.cpu.r);
        r[16] = self.cpu.cpsr;
        r
    }

    /// Skip ahead to the next event if the CPU is provably spinning: it goes
    /// once around its loop and comes back to the same PC with the same
    /// registers, without changing memory or touching hardware registers.
    /// Then every further pass is identical until the next interrupt or
    /// button event, so emulated time jumps straight there. (rand()'s state
    /// is left out: the main loop stirs it on every pass.)
    fn try_idle_skip(&mut self, limit: u64) -> bool {
        if self.cpu.mode() == MODE_IRQ {
            return false;
        }
        let pc = self.cpu.r[15];
        let mut want = self.idle_regs();
        self.probe_stats[0] += 1;
        for _ in 0..IDLE_PASSES {
            self.sys.track_writes();
            let writes = self.sys.mmio_writes;
            let mut budget = IDLE_PROBE;
            let found = loop {
                let ran = self.exec(budget.min(limit - self.executed), pc);
                self.executed += ran + self.spin_skipped;
                self.spin_skipped = 0;
                budget = budget.saturating_sub(ran);
                if self.sys.stop || self.cpu.fault.is_some() || self.cpu.r[15] != pc {
                    break false;
                }
                if self.idle_regs() == want {
                    break true;
                }
                if budget == 0 || self.executed >= limit {
                    break false;
                }
                // a helper inside the loop can pass this PC several times
                // per pass; keep going around
                let ran = self.exec(1, 1);
                self.executed += ran;
                budget = budget.saturating_sub(ran);
            };
            if !found {
                self.sys.writes_undone();
                self.probe_stats[1] += 1;
                return false;
            }
            let same = self.sys.writes_undone();
            if !same {
                self.probe_stats[2] += 1;
            } else if self.sys.mmio_writes != writes {
                self.probe_stats[3] += 1;
            }
            if same && self.sys.mmio_writes == writes {
                self.probe_stats[4] += 1;
                let target = (self.sys.next_event() * CPU_HZ) as u64 + 1;
                let target = target.min(limit);
                if target > self.executed {
                    self.idle_skipped += target - self.executed;
                    self.executed = target;
                }
                return true;
            }
            // the first pass after an interrupt often consumes a flag the
            // ISR set; the next pass may already be a fixed point
            want = self.idle_regs();
        }
        false
    }

    /// Run a firmware function (Thumb) to completion with all registers
    /// saved and restored; memory changes stay. True if it returned.
    pub fn call_firmware(&mut self, func: u32, arg: u32, limit: u64) -> bool {
        let saved = self.cpu.clone();
        let sp = self.cpu.r[13];
        self.cpu.r[0] = arg;
        self.cpu.r[13] = (sp.wrapping_sub(0x100)) & !7;
        self.cpu.r[14] = CALL_RETURN | 1;
        self.cpu.r[15] = func & !1;
        self.cpu.cpsr |= FLAG_T;
        let mut n = 0;
        let mut done = false;
        while n < limit {
            if self.cpu.r[15] == CALL_RETURN {
                done = true;
                break;
            }
            n += self.exec(1000.min(limit - n), CALL_RETURN);
            if self.cpu.fault.is_some() {
                break;
            }
        }
        let stop = self.sys.stop;
        self.cpu = saved;
        self.sys.stop = stop;
        done
    }

    /// With never-sleep, update the game's clock when the minute changes
    /// (on a real toy this happens at each wake-up, once a minute).
    fn refresh_clock(&mut self) {
        let func = match self.clock_refresh {
            Some(f) => f,
            None => return,
        };
        let minute = self.sys.rtc_irq.tod(self.sys.game_time) / 60;
        if Some(minute) == self.clock_minute || self.cpu.mode() == MODE_IRQ {
            return;
        }
        if self.call_firmware(func, 0, 5_000_000) {
            self.clock_minute = Some(minute);
        }
    }

    // --- sleep ---------------------------------------------------------------

    /// Deep sleep: skip to the RTC alarm or the next scripted press, then
    /// cold-boot. Returns false if nothing will wake it (or not before
    /// `limit`, in which case time moves to `limit`).
    pub fn power_cycle(&mut self, limit: u64) -> bool {
        let now = self.now();
        let turbo = self.sys.turbo;
        let mut wait: Option<f64> = None;
        if let Some(alarm) = self.sys.rtc.alarm_ticks() {
            let dt = ((alarm - self.sys.rtc.ticks(self.sys.game_time)) as f64 / 32768.0 / turbo).max(0.0);
            wait = Some(dt);
        }
        for &(t, _) in &self.sys.key_script {
            if t >= now {
                wait = Some(wait.map_or(t - now, |w| w.min(t - now)));
            }
        }
        let dt = match wait {
            Some(d) => d,
            None => return false,
        };
        if self.executed + (dt * CPU_HZ) as u64 > limit {
            // still asleep at the end
            self.sys.game_time += (limit as f64 / CPU_HZ - now) * turbo;
            self.executed = limit;
            self.sys.now = limit as f64 / CPU_HZ;
            return true;
        }
        self.cold_boot(dt);
        true
    }

    /// Restart now, like taking the batteries out and back in: flash and the
    /// clock stay, everything else starts from reset.
    pub fn restart(&mut self) {
        self.cold_boot(0.0);
    }

    /// Reboot from the current flash `dt` seconds later, keeping the RTC,
    /// button script, sound and screen counters.
    pub fn cold_boot(&mut self, dt: f64) {
        let old_now = self.now();
        let flash = self.sys.ram[..FLASH_SIZE].to_vec();
        let mut fresh = Machine::new(self.image.clone(), &flash, 0.0);
        fresh.executed = self.executed + (dt * CPU_HZ) as u64;
        fresh.irqs = self.irqs;
        fresh.log = std::mem::take(&mut self.log);
        fresh.print_log = self.print_log;
        fresh.idle_skip = self.idle_skip;
        fresh.idle_skipped = self.idle_skipped;
        fresh.never_sleep = self.never_sleep;
        fresh.stop_on_bluetooth = self.stop_on_bluetooth;
        let s = &mut self.sys;
        let q = &mut fresh.sys;
        q.flash.writes = s.flash.writes;
        q.now = fresh.executed as f64 / CPU_HZ;
        q.game_time = s.game_time + dt * s.turbo;
        q.turbo = s.turbo;
        q.key_script = std::mem::take(&mut s.key_script);
        q.live_keys = s.live_keys;
        q.hold_until = s.hold_until;
        q.sound.events = std::mem::take(&mut s.sound.events);
        if s.sound.state.0 != 0.0 {
            q.sound.events.push((old_now, 0.0, 0.5)); // power cut stops the buzzer
        }
        q.rtc = s.rtc.clone();
        q.rtc_irq.fired = (q.game_time / sys::RTC_TICK_PERIOD) as u64;
        q.lcd.frames = s.lcd.frames;
        q.lcd.last_vsync = (q.now * sys::VSYNC_HZ) as u64;
        q.trace_mmio = s.trace_mmio;
        *self = fresh;
    }

    /// Run until `limit`, sleeping and waking as the firmware does.
    pub fn run_with_sleep(&mut self, limit: u64) -> Outcome {
        loop {
            let out = self.run(limit);
            if out != Outcome::PowerOff {
                return out;
            }
            if !self.power_cycle(limit) {
                return Outcome::PowerOff;
            }
            if self.executed >= limit {
                return Outcome::Limit;
            }
        }
    }

    pub fn lcd_rgb(&self) -> Vec<u8> {
        self.sys.lcd.rgb()
    }

    pub fn screen_size() -> (usize, usize) {
        (LCD_W, LCD_H)
    }

    pub fn fault(&self) -> Option<Fault> {
        self.cpu.fault
    }
}

/// Tiny printf: %d %i %u %x %X %p %s %c with flags and width.
fn c_printf(m: &mut Machine, fmt: &str, args: &[u32]) -> String {
    let mut out = String::new();
    let mut it = args.iter().copied();
    let chars: Vec<char> = fmt.chars().collect();
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        i += 1;
        if c != '%' {
            out.push(c);
            continue;
        }
        let mut flags = String::new();
        while i < chars.len() && "-0 +#".contains(chars[i]) {
            flags.push(chars[i]);
            i += 1;
        }
        let mut width = 0usize;
        while i < chars.len() && chars[i].is_ascii_digit() {
            width = width * 10 + chars[i].to_digit(10).unwrap() as usize;
            i += 1;
        }
        while i < chars.len() && "lh".contains(chars[i]) {
            i += 1;
        }
        if i >= chars.len() {
            break;
        }
        let conv = chars[i];
        i += 1;
        let body = match conv {
            '%' => "%".to_string(),
            's' => {
                let a = it.next().unwrap_or(0);
                m.read_cstr(a, 256)
            }
            'c' => ((it.next().unwrap_or(0) & 0xFF) as u8 as char).to_string(),
            'd' | 'i' => (it.next().unwrap_or(0) as i32).to_string(),
            'u' => it.next().unwrap_or(0).to_string(),
            'x' => format!("{:x}", it.next().unwrap_or(0)),
            'X' => format!("{:X}", it.next().unwrap_or(0)),
            'p' => format!("{:08X}", it.next().unwrap_or(0)),
            _ => format!("%{}", conv),
        };
        if body.len() < width {
            let pad = width - body.len();
            if flags.contains('-') {
                out.push_str(&body);
                out.push_str(&" ".repeat(pad));
            } else if flags.contains('0') && conv != 's' {
                let (sign, digits) = if let Some(d) = body.strip_prefix('-') { ("-", d) } else { ("", &body[..]) };
                out.push_str(sign);
                out.push_str(&"0".repeat(pad));
                out.push_str(digits);
            } else {
                out.push_str(&" ".repeat(pad));
                out.push_str(&body);
            }
        } else {
            out.push_str(&body);
        }
    }
    out
}

/// Unused import guard for Bus (the trait is used through System).
#[allow(dead_code)]
fn _bus_check(s: &mut System) -> u8 {
    s.read8(0)
}
