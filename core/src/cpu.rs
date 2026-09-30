//! ARMv5TE processor (the ARM926EJ-S core of the tg18 SoC), as an interpreter.
//!
//! Covers the ARM and Thumb instruction sets of ARMv5TE: data processing,
//! multiplies (incl. the DSP ones), loads/stores, load/store multiple, swaps,
//! branches with interworking, PSR transfers and CLZ. The firmware uses no
//! MMU, caches, coprocessors or software interrupts; those are reported as
//! faults instead (coprocessor accesses are ignored).
//!
//! Between steps `r[15]` holds the address of the next instruction. While an
//! instruction executes, reading r15 gives its address + 8 (ARM) or + 4
//! (Thumb), as on the hardware.

/// Memory as the CPU sees it. Addresses of 16- and 32-bit accesses are
/// aligned by the CPU before the call.
pub trait Bus {
    fn read8(&mut self, addr: u32) -> u8;
    fn read16(&mut self, addr: u32) -> u16;
    fn read32(&mut self, addr: u32) -> u32;
    fn write8(&mut self, addr: u32, val: u8);
    fn write16(&mut self, addr: u32, val: u16);
    fn write32(&mut self, addr: u32, val: u32);
    fn fetch32(&mut self, addr: u32) -> u32 {
        self.read32(addr)
    }
    fn fetch16(&mut self, addr: u32) -> u16 {
        self.read16(addr)
    }
}

pub const MODE_USR: u32 = 0x10;
pub const MODE_FIQ: u32 = 0x11;
pub const MODE_IRQ: u32 = 0x12;
pub const MODE_SVC: u32 = 0x13;
pub const MODE_ABT: u32 = 0x17;
pub const MODE_UND: u32 = 0x1B;
pub const MODE_SYS: u32 = 0x1F;

pub const FLAG_N: u32 = 1 << 31;
pub const FLAG_Z: u32 = 1 << 30;
pub const FLAG_C: u32 = 1 << 29;
pub const FLAG_V: u32 = 1 << 28;
pub const FLAG_Q: u32 = 1 << 27;
pub const FLAG_I: u32 = 1 << 7;
pub const FLAG_F: u32 = 1 << 6;
pub const FLAG_T: u32 = 1 << 5;

/// Why the CPU could not go on.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Fault {
    Undefined { addr: u32, op: u32, thumb: bool },
    SoftwareInterrupt { addr: u32 },
    Breakpoint { addr: u32 },
}

/// Register bank of a processor mode: 0 user/system, 1 FIQ, 2 IRQ,
/// 3 supervisor, 4 abort, 5 undefined.
fn bank(mode: u32) -> usize {
    match mode {
        MODE_FIQ => 1,
        MODE_IRQ => 2,
        MODE_SVC => 3,
        MODE_ABT => 4,
        MODE_UND => 5,
        _ => 0,
    }
}

/// Number of u32 words in `Cpu::state()`.
pub const STATE_WORDS: usize = 45;

#[derive(Clone)]
pub struct Cpu {
    /// The registers of the current mode.
    pub r: [u32; 16],
    pub cpsr: u32,
    /// r13/r14 of each bank while that bank is not the current one.
    banked: [[u32; 2]; 6],
    /// r8-r12 of the non-FIQ modes while in FIQ mode, and of FIQ otherwise.
    usr_r8_12: [u32; 5],
    fiq_r8_12: [u32; 5],
    /// SPSR per bank (index 0 unused).
    spsr: [u32; 6],
    /// Where execution continues after the current instruction.
    next: u32,
    pub fault: Option<Fault>,
}

impl Default for Cpu {
    fn default() -> Self {
        Self::new()
    }
}

impl Cpu {
    pub fn new() -> Self {
        Cpu {
            r: [0; 16],
            cpsr: MODE_SVC | FLAG_I | FLAG_F,
            banked: [[0; 2]; 6],
            usr_r8_12: [0; 5],
            fiq_r8_12: [0; 5],
            spsr: [0; 6],
            next: 0,
            fault: None,
        }
    }

    #[inline]
    pub fn pc(&self) -> u32 {
        self.r[15]
    }

    #[inline]
    pub fn thumb(&self) -> bool {
        self.cpsr & FLAG_T != 0
    }

    #[inline]
    pub fn mode(&self) -> u32 {
        self.cpsr & 0x1F
    }

    // --- modes and PSRs ----------------------------------------------------

    /// Switch the visible registers to another mode's bank (the mode bits of
    /// CPSR change too; nothing else does).
    pub fn set_mode(&mut self, mode: u32) {
        let old = bank(self.mode());
        let new = bank(mode);
        if old != new {
            self.banked[old] = [self.r[13], self.r[14]];
            if old == 1 {
                self.fiq_r8_12.copy_from_slice(&self.r[8..13]);
                self.r[8..13].copy_from_slice(&self.usr_r8_12);
            } else if new == 1 {
                self.usr_r8_12.copy_from_slice(&self.r[8..13]);
                self.r[8..13].copy_from_slice(&self.fiq_r8_12);
            }
            self.r[13] = self.banked[new][0];
            self.r[14] = self.banked[new][1];
        }
        self.cpsr = (self.cpsr & !0x1F) | (mode & 0x1F);
    }

    /// Write the whole CPSR, switching register banks if the mode changes.
    pub fn write_cpsr(&mut self, val: u32) {
        if val & 0x1F != self.cpsr & 0x1F {
            self.set_mode(val & 0x1F);
        }
        self.cpsr = val;
    }

    pub fn spsr(&self) -> u32 {
        match bank(self.mode()) {
            0 => self.cpsr,
            b => self.spsr[b],
        }
    }

    pub fn set_spsr(&mut self, val: u32) {
        let b = bank(self.mode());
        if b != 0 {
            self.spsr[b] = val;
        }
    }

    /// Take an IRQ: the next instruction is where the interrupted code resumes.
    pub fn enter_irq(&mut self, vector: u32) {
        let old = self.cpsr;
        let ret = self.r[15].wrapping_add(4);
        self.set_mode(MODE_IRQ);
        self.spsr[bank(MODE_IRQ)] = old;
        self.cpsr = (self.cpsr & !FLAG_T) | FLAG_I;
        self.r[14] = ret;
        self.r[15] = vector;
    }

    /// All registers of all modes, for snapshots: r0-r15, CPSR, r13/r14 of
    /// the six banks (current bank's copy is live in r), user r8-r12, FIQ
    /// r8-r12 (current copy live), SPSRs of the six banks.
    pub fn state(&self) -> [u32; STATE_WORDS] {
        let mut c = self.clone();
        let cur = bank(c.mode());
        c.banked[cur] = [c.r[13], c.r[14]];
        if cur == 1 {
            c.fiq_r8_12.copy_from_slice(&c.r[8..13]);
        } else {
            c.usr_r8_12.copy_from_slice(&c.r[8..13]);
        }
        let mut s = [0u32; STATE_WORDS];
        s[..16].copy_from_slice(&c.r);
        s[16] = c.cpsr;
        for b in 0..6 {
            s[17 + 2 * b] = c.banked[b][0];
            s[18 + 2 * b] = c.banked[b][1];
        }
        s[29..34].copy_from_slice(&c.usr_r8_12);
        s[34..39].copy_from_slice(&c.fiq_r8_12);
        s[39..45].copy_from_slice(&c.spsr);
        s
    }

    pub fn set_state(&mut self, s: &[u32]) {
        self.r.copy_from_slice(&s[..16]);
        self.cpsr = s[16];
        for b in 0..6 {
            self.banked[b] = [s[17 + 2 * b], s[18 + 2 * b]];
        }
        self.usr_r8_12.copy_from_slice(&s[29..34]);
        self.fiq_r8_12.copy_from_slice(&s[34..39]);
        self.spsr.copy_from_slice(&s[39..45]);
        self.fault = None;
    }

    // --- helpers -----------------------------------------------------------

    #[inline]
    fn carry(&self) -> u32 {
        (self.cpsr >> 29) & 1
    }

    #[inline]
    fn set_nz(&mut self, res: u32) {
        self.cpsr = (self.cpsr & !(FLAG_N | FLAG_Z)) | (res & FLAG_N) | if res == 0 { FLAG_Z } else { 0 };
    }

    #[inline]
    fn set_nzc(&mut self, res: u32, c: bool) {
        self.cpsr = (self.cpsr & !(FLAG_N | FLAG_Z | FLAG_C))
            | (res & FLAG_N)
            | if res == 0 { FLAG_Z } else { 0 }
            | if c { FLAG_C } else { 0 };
    }

    #[inline]
    fn set_nzcv(&mut self, res: u32, c: bool, v: bool) {
        self.cpsr = (self.cpsr & !(FLAG_N | FLAG_Z | FLAG_C | FLAG_V))
            | (res & FLAG_N)
            | if res == 0 { FLAG_Z } else { 0 }
            | if c { FLAG_C } else { 0 }
            | if v { FLAG_V } else { 0 };
    }

    /// a + b + carry_in, with carry and overflow out.
    #[inline]
    fn adc(a: u32, b: u32, cin: u32) -> (u32, bool, bool) {
        let wide = a as u64 + b as u64 + cin as u64;
        let res = wide as u32;
        (res, wide >> 32 != 0, ((a ^ res) & (b ^ res)) >> 31 != 0)
    }

    #[inline]
    fn cond(&self, c: u32) -> bool {
        let f = self.cpsr >> 28;
        let (n, z, cf, v) = (f & 8 != 0, f & 4 != 0, f & 2 != 0, f & 1 != 0);
        match c {
            0x0 => z,
            0x1 => !z,
            0x2 => cf,
            0x3 => !cf,
            0x4 => n,
            0x5 => !n,
            0x6 => v,
            0x7 => !v,
            0x8 => cf && !z,
            0x9 => !cf || z,
            0xA => n == v,
            0xB => n != v,
            0xC => !z && n == v,
            0xD => z || n != v,
            _ => true,
        }
    }

    /// Write a register; r15 is a branch that stays in the current state.
    #[inline]
    fn set_reg(&mut self, n: usize, val: u32) {
        if n == 15 {
            self.next = if self.thumb() { val & !1 } else { val & !3 };
        } else {
            self.r[n] = val;
        }
    }

    /// Branch with interworking: bit 0 selects Thumb.
    #[inline]
    fn bx(&mut self, val: u32) {
        if val & 1 != 0 {
            self.cpsr |= FLAG_T;
            self.next = val & !1;
        } else {
            self.cpsr &= !FLAG_T;
            self.next = val & !3;
        }
    }

    /// Return from an exception: CPSR = SPSR, then jump in the restored state.
    fn exception_return(&mut self, target: u32) {
        let spsr = self.spsr();
        self.write_cpsr(spsr);
        self.next = if self.thumb() { target & !1 } else { target & !3 };
    }

    fn user_reg(&self, n: usize) -> u32 {
        let b = bank(self.mode());
        match n {
            8..=12 if b == 1 => self.usr_r8_12[n - 8],
            13 | 14 if b != 0 => self.banked[0][n - 13],
            _ => self.r[n],
        }
    }

    fn set_user_reg(&mut self, n: usize, val: u32) {
        let b = bank(self.mode());
        match n {
            8..=12 if b == 1 => self.usr_r8_12[n - 8] = val,
            13 | 14 if b != 0 => self.banked[0][n - 13] = val,
            15 => self.set_reg(15, val),
            _ => self.r[n] = val,
        }
    }

    /// Barrel shifter with an immediate amount (encoding rules: LSR/ASR #0
    /// mean #32, ROR #0 means RRX). Returns (value, carry out).
    #[inline]
    fn shift_imm(&self, val: u32, typ: u32, amt: u32) -> (u32, bool) {
        let c = self.carry() != 0;
        match typ {
            0 => {
                if amt == 0 {
                    (val, c)
                } else {
                    (val << amt, (val >> (32 - amt)) & 1 != 0)
                }
            }
            1 => {
                if amt == 0 {
                    (0, val >> 31 != 0)
                } else {
                    (val >> amt, (val >> (amt - 1)) & 1 != 0)
                }
            }
            2 => {
                if amt == 0 {
                    (((val as i32) >> 31) as u32, val >> 31 != 0)
                } else {
                    (((val as i32) >> amt) as u32, (val >> (amt - 1)) & 1 != 0)
                }
            }
            _ => {
                if amt == 0 {
                    (((c as u32) << 31) | (val >> 1), val & 1 != 0)
                } else {
                    (val.rotate_right(amt), (val >> (amt - 1)) & 1 != 0)
                }
            }
        }
    }

    /// Barrel shifter with a register amount (bottom byte, 0..255).
    #[inline]
    fn shift_reg(&self, val: u32, typ: u32, amt: u32) -> (u32, bool) {
        let c = self.carry() != 0;
        if amt == 0 {
            return (val, c);
        }
        match typ {
            0 => match amt {
                1..=31 => (val << amt, (val >> (32 - amt)) & 1 != 0),
                32 => (0, val & 1 != 0),
                _ => (0, false),
            },
            1 => match amt {
                1..=31 => (val >> amt, (val >> (amt - 1)) & 1 != 0),
                32 => (0, val >> 31 != 0),
                _ => (0, false),
            },
            2 => {
                if amt < 32 {
                    (((val as i32) >> amt) as u32, (val >> (amt - 1)) & 1 != 0)
                } else {
                    (((val as i32) >> 31) as u32, val >> 31 != 0)
                }
            }
            _ => {
                let a = amt & 31;
                if a == 0 {
                    (val, val >> 31 != 0)
                } else {
                    (val.rotate_right(a), (val >> (a - 1)) & 1 != 0)
                }
            }
        }
    }

    // --- execution ---------------------------------------------------------

    /// Execute one instruction.
    #[inline]
    pub fn step<B: Bus>(&mut self, bus: &mut B) {
        if self.cpsr & FLAG_T != 0 {
            let pc = self.r[15];
            let op = bus.fetch16(pc);
            self.r[15] = pc.wrapping_add(4);
            self.next = pc.wrapping_add(2);
            self.exec_thumb(bus, op, pc);
        } else {
            let pc = self.r[15];
            let op = bus.fetch32(pc);
            self.r[15] = pc.wrapping_add(8);
            self.next = pc.wrapping_add(4);
            let c = op >> 28;
            if c == 0xE || self.cond(c) {
                self.exec_arm(bus, op, pc);
            } else if c == 0xF {
                self.exec_arm_uncond(bus, op, pc);
            }
        }
        self.r[15] = self.next;
    }

    fn undefined(&mut self, op: u32, pc: u32) {
        self.fault = Some(Fault::Undefined { addr: pc, op, thumb: self.thumb() });
        self.next = pc;
    }

    fn exec_arm_uncond<B: Bus>(&mut self, _bus: &mut B, op: u32, pc: u32) {
        if op & 0x0E00_0000 == 0x0A00_0000 {
            // BLX immediate: always to Thumb, H bit adds a halfword
            let off = (((op & 0x00FF_FFFF) << 8) as i32 >> 6) as u32;
            let target = pc.wrapping_add(8).wrapping_add(off) | ((op >> 23) & 2);
            self.r[14] = pc.wrapping_add(4);
            self.cpsr |= FLAG_T;
            self.next = target;
        } else if op & 0x0D70_F000 == 0x0550_F000 {
            // PLD: a cache hint, nothing to do
        } else {
            self.undefined(op, pc);
        }
    }

    fn exec_arm<B: Bus>(&mut self, bus: &mut B, op: u32, pc: u32) {
        match (op >> 25) & 7 {
            0 => {
                if op & 0x90 == 0x90 {
                    if op & 0x60 == 0 {
                        if op & 0x0F80_0000 == 0x0000_0000 {
                            self.arm_mul(op);
                        } else if op & 0x0F80_0000 == 0x0080_0000 {
                            self.arm_mul_long(op);
                        } else if op & 0x0FB0_0FF0 == 0x0100_0090 {
                            self.arm_swap(bus, op);
                        } else {
                            self.undefined(op, pc);
                        }
                    } else {
                        self.arm_extra_load_store(bus, op, pc);
                    }
                } else if op & 0x0190_0000 == 0x0100_0000 {
                    self.arm_misc(op, pc);
                } else {
                    self.arm_data(op);
                }
            }
            1 => {
                if op & 0x0190_0000 == 0x0100_0000 {
                    if op & 0x0020_0000 != 0 {
                        // MSR immediate
                        let imm = (op & 0xFF).rotate_right(((op >> 8) & 15) * 2);
                        self.msr(op & (1 << 22) != 0, (op >> 16) & 15, imm);
                    } else {
                        self.undefined(op, pc);
                    }
                } else {
                    self.arm_data(op);
                }
            }
            2 => self.arm_load_store(bus, op),
            3 => {
                if op & 0x10 != 0 {
                    self.undefined(op, pc);
                } else {
                    self.arm_load_store(bus, op);
                }
            }
            4 => self.arm_block(bus, op),
            5 => {
                let off = (((op & 0x00FF_FFFF) << 8) as i32 >> 6) as u32;
                if op & (1 << 24) != 0 {
                    self.r[14] = pc.wrapping_add(4);
                }
                self.next = pc.wrapping_add(8).wrapping_add(off);
            }
            6 => {} // LDC/STC: no coprocessors with memory access
            _ => {
                if op & (1 << 24) != 0 {
                    self.fault = Some(Fault::SoftwareInterrupt { addr: pc });
                    self.next = pc;
                } else if op & 0x10 != 0 && op & (1 << 20) != 0 {
                    // MRC: read as 0, except the CP15 main ID register
                    let rd = ((op >> 12) & 15) as usize;
                    let cp = (op >> 8) & 15;
                    let crn = (op >> 16) & 15;
                    let val = if cp == 15 && crn == 0 { 0x4106_9265 } else { 0 };
                    if rd == 15 {
                        self.cpsr = (self.cpsr & 0x0FFF_FFFF) | (val & 0xF000_0000);
                    } else {
                        self.r[rd] = val;
                    }
                }
                // MCR / CDP: cache and MMU maintenance, ignored
            }
        }
    }

    fn arm_data(&mut self, op: u32) {
        let s = op & (1 << 20) != 0;
        let rn = ((op >> 16) & 15) as usize;
        let rd = ((op >> 12) & 15) as usize;
        let (b, sc, a) = if op & (1 << 25) != 0 {
            let rot = ((op >> 8) & 15) * 2;
            let v = (op & 0xFF).rotate_right(rot);
            let c = if rot == 0 { self.carry() != 0 } else { v >> 31 != 0 };
            (v, c, self.r[rn])
        } else if op & 0x10 != 0 {
            // register-specified shift: r15 as an operand reads 12 ahead
            let rm = (op & 15) as usize;
            let rs = ((op >> 8) & 15) as usize;
            let pc12 = |cpu: &Cpu, n: usize| if n == 15 { cpu.r[15].wrapping_add(4) } else { cpu.r[n] };
            let amt = pc12(self, rs) & 0xFF;
            let (v, c) = self.shift_reg(pc12(self, rm), (op >> 5) & 3, amt);
            (v, c, pc12(self, rn))
        } else {
            let (v, c) = self.shift_imm(self.r[(op & 15) as usize], (op >> 5) & 3, (op >> 7) & 31);
            (v, c, self.r[rn])
        };
        let cin = self.carry();
        let opc = (op >> 21) & 15;
        let (res, write) = match opc {
            0x0 | 0x8 => (a & b, opc == 0x0),
            0x1 | 0x9 => (a ^ b, opc == 0x1),
            0xC => (a | b, true),
            0xD => (b, true),
            0xE => (a & !b, true),
            0xF => (!b, true),
            _ => {
                let (res, c, v) = match opc {
                    0x2 | 0xA => Self::adc(a, !b, 1),
                    0x3 => Self::adc(b, !a, 1),
                    0x4 | 0xB => Self::adc(a, b, 0),
                    0x5 => Self::adc(a, b, cin),
                    0x6 => Self::adc(a, !b, cin),
                    _ => Self::adc(b, !a, cin), // RSC
                };
                if s && rd != 15 {
                    self.set_nzcv(res, c, v);
                } else if s && opc >= 0x8 {
                    self.set_nzcv(res, c, v);
                }
                if opc < 0x8 {
                    self.write_data_result(rd, res, s);
                }
                return;
            }
        };
        if s && (rd != 15 || !write) {
            self.set_nzc(res, sc);
        }
        if write {
            self.write_data_result(rd, res, s);
        }
    }

    #[inline]
    fn write_data_result(&mut self, rd: usize, res: u32, s: bool) {
        if rd == 15 {
            if s {
                self.exception_return(res);
            } else {
                self.next = res & !3;
            }
        } else {
            self.r[rd] = res;
        }
    }

    fn arm_mul(&mut self, op: u32) {
        let rd = ((op >> 16) & 15) as usize;
        let rn = ((op >> 12) & 15) as usize;
        let rs = ((op >> 8) & 15) as usize;
        let rm = (op & 15) as usize;
        let mut res = self.r[rm].wrapping_mul(self.r[rs]);
        if op & (1 << 21) != 0 {
            res = res.wrapping_add(self.r[rn]);
        }
        if op & (1 << 20) != 0 {
            self.set_nz(res);
        }
        self.set_reg(rd, res);
    }

    fn arm_mul_long(&mut self, op: u32) {
        let hi = ((op >> 16) & 15) as usize;
        let lo = ((op >> 12) & 15) as usize;
        let rs = ((op >> 8) & 15) as usize;
        let rm = (op & 15) as usize;
        let signed = op & (1 << 22) != 0;
        let mut res = if signed {
            (self.r[rm] as i32 as i64).wrapping_mul(self.r[rs] as i32 as i64) as u64
        } else {
            (self.r[rm] as u64) * (self.r[rs] as u64)
        };
        if op & (1 << 21) != 0 {
            res = res.wrapping_add(((self.r[hi] as u64) << 32) | self.r[lo] as u64);
        }
        if op & (1 << 20) != 0 {
            self.cpsr = (self.cpsr & !(FLAG_N | FLAG_Z))
                | ((res >> 32) as u32 & FLAG_N)
                | if res == 0 { FLAG_Z } else { 0 };
        }
        self.set_reg(lo, res as u32);
        self.set_reg(hi, (res >> 32) as u32);
    }

    fn arm_swap<B: Bus>(&mut self, bus: &mut B, op: u32) {
        let rn = ((op >> 16) & 15) as usize;
        let rd = ((op >> 12) & 15) as usize;
        let rm = (op & 15) as usize;
        let addr = self.r[rn];
        let new = self.r[rm];
        if op & (1 << 22) != 0 {
            let old = bus.read8(addr);
            bus.write8(addr, new as u8);
            self.set_reg(rd, old as u32);
        } else {
            let old = bus.read32(addr & !3).rotate_right((addr & 3) * 8);
            bus.write32(addr & !3, new);
            self.set_reg(rd, old);
        }
    }

    /// Status register transfers, BX/BLX, CLZ and the DSP multiplies.
    fn arm_misc(&mut self, op: u32, pc: u32) {
        let rd = ((op >> 12) & 15) as usize;
        let rm = (op & 15) as usize;
        if op & 0x0FBF_0FFF == 0x010F_0000 {
            let v = if op & (1 << 22) != 0 { self.spsr() } else { self.cpsr };
            self.set_reg(rd, v);
        } else if op & 0x0FB0_FFF0 == 0x0120_F000 {
            self.msr(op & (1 << 22) != 0, (op >> 16) & 15, self.r[rm]);
        } else if op & 0x0FFF_FFF0 == 0x012F_FF10 {
            self.bx(self.r[rm]);
        } else if op & 0x0FFF_FFF0 == 0x012F_FF30 {
            let target = self.r[rm];
            self.r[14] = pc.wrapping_add(4);
            self.bx(target);
        } else if op & 0x0FFF_0FF0 == 0x016F_0F10 {
            self.set_reg(rd, self.r[rm].leading_zeros());
        } else if op & 0x0FF0_00F0 == 0x0120_0070 {
            self.fault = Some(Fault::Breakpoint { addr: pc });
            self.next = pc;
        } else if op & 0x0F90_0FF0 == 0x0100_0050 {
            self.arm_qadd(op);
        } else if op & 0x0F90_0090 == 0x0100_0080 {
            self.arm_dsp_mul(op);
        } else {
            self.undefined(op, pc);
        }
    }

    fn msr(&mut self, to_spsr: bool, fields: u32, val: u32) {
        let mut mask = 0u32;
        for (i, m) in [0xFFu32, 0xFF00, 0xFF_0000, 0xFF00_0000].iter().enumerate() {
            if fields & (1 << i) != 0 {
                mask |= m;
            }
        }
        if to_spsr {
            if bank(self.mode()) != 0 {
                let s = self.spsr();
                self.set_spsr((s & !mask) | (val & mask));
            }
        } else {
            if self.mode() == MODE_USR {
                mask &= 0xFF00_0000;
            }
            mask &= !FLAG_T; // the state bit only changes by branching
            let new = (self.cpsr & !mask) | (val & mask);
            self.write_cpsr(new);
        }
    }

    fn sat_add(&mut self, a: i32, b: i32) -> i32 {
        match a.checked_add(b) {
            Some(v) => v,
            None => {
                self.cpsr |= FLAG_Q;
                if b < 0 { i32::MIN } else { i32::MAX }
            }
        }
    }

    fn sat_sub(&mut self, a: i32, b: i32) -> i32 {
        match a.checked_sub(b) {
            Some(v) => v,
            None => {
                self.cpsr |= FLAG_Q;
                if b > 0 { i32::MIN } else { i32::MAX }
            }
        }
    }

    /// QADD, QSUB, QDADD, QDSUB
    fn arm_qadd(&mut self, op: u32) {
        let rn = ((op >> 16) & 15) as usize;
        let rd = ((op >> 12) & 15) as usize;
        let rm = (op & 15) as usize;
        let a = self.r[rm] as i32;
        let mut b = self.r[rn] as i32;
        if op & (1 << 22) != 0 {
            b = self.sat_add(b, b);
        }
        let res = if op & (1 << 21) != 0 { self.sat_sub(a, b) } else { self.sat_add(a, b) };
        self.set_reg(rd, res as u32);
    }

    /// SMLAxy, SMLAWy, SMULWy, SMLALxy, SMULxy
    fn arm_dsp_mul(&mut self, op: u32) {
        let rd = ((op >> 16) & 15) as usize;
        let rn = ((op >> 12) & 15) as usize;
        let rs = ((op >> 8) & 15) as usize;
        let rm = (op & 15) as usize;
        let x = (op >> 5) & 1;
        let y = (op >> 6) & 1;
        let half = |v: u32, top: u32| (if top != 0 { v >> 16 } else { v }) as u16 as i16 as i32;
        match (op >> 21) & 3 {
            0 => {
                let p = half(self.r[rm], x) * half(self.r[rs], y);
                let acc = self.r[rn] as i32;
                let (res, ov) = p.overflowing_add(acc);
                if ov {
                    self.cpsr |= FLAG_Q;
                }
                self.set_reg(rd, res as u32);
            }
            1 => {
                let p = ((self.r[rm] as i32 as i64 * half(self.r[rs], y) as i64) >> 16) as i32;
                if x == 0 {
                    let (res, ov) = p.overflowing_add(self.r[rn] as i32);
                    if ov {
                        self.cpsr |= FLAG_Q;
                    }
                    self.set_reg(rd, res as u32);
                } else {
                    self.set_reg(rd, p as u32);
                }
            }
            2 => {
                let p = (half(self.r[rm], x) * half(self.r[rs], y)) as i64;
                let acc = (((self.r[rd] as u64) << 32) | self.r[rn] as u64) as i64;
                let res = acc.wrapping_add(p) as u64;
                self.set_reg(rn, res as u32);
                self.set_reg(rd, (res >> 32) as u32);
            }
            _ => {
                let p = half(self.r[rm], x) * half(self.r[rs], y);
                self.set_reg(rd, p as u32);
            }
        }
    }

    fn arm_load_store<B: Bus>(&mut self, bus: &mut B, op: u32) {
        let rn = ((op >> 16) & 15) as usize;
        let rd = ((op >> 12) & 15) as usize;
        let off = if op & (1 << 25) == 0 {
            op & 0xFFF
        } else {
            self.shift_imm(self.r[(op & 15) as usize], (op >> 5) & 3, (op >> 7) & 31).0
        };
        let base = self.r[rn];
        let moved = if op & (1 << 23) != 0 { base.wrapping_add(off) } else { base.wrapping_sub(off) };
        let pre = op & (1 << 24) != 0;
        let addr = if pre { moved } else { base };
        let writeback = !pre || op & (1 << 21) != 0;
        let byte = op & (1 << 22) != 0;
        if op & (1 << 20) != 0 {
            let val = if byte {
                bus.read8(addr) as u32
            } else {
                bus.read32(addr & !3).rotate_right((addr & 3) * 8)
            };
            if writeback {
                self.set_reg(rn, moved);
            }
            if rd == 15 {
                self.bx(val);
            } else {
                self.r[rd] = val;
            }
        } else {
            let val = self.r[rd];
            if byte {
                bus.write8(addr, val as u8);
            } else {
                bus.write32(addr & !3, val);
            }
            if writeback {
                self.set_reg(rn, moved);
            }
        }
    }

    fn arm_extra_load_store<B: Bus>(&mut self, bus: &mut B, op: u32, pc: u32) {
        let rn = ((op >> 16) & 15) as usize;
        let rd = ((op >> 12) & 15) as usize;
        let off = if op & (1 << 22) != 0 {
            ((op >> 4) & 0xF0) | (op & 0xF)
        } else {
            self.r[(op & 15) as usize]
        };
        let base = self.r[rn];
        let moved = if op & (1 << 23) != 0 { base.wrapping_add(off) } else { base.wrapping_sub(off) };
        let pre = op & (1 << 24) != 0;
        let addr = if pre { moved } else { base };
        let writeback = !pre || op & (1 << 21) != 0;
        let load = op & (1 << 20) != 0;
        match (load, (op >> 5) & 3) {
            (true, sh) => {
                let val = match sh {
                    1 => bus.read16(addr & !1) as u32,
                    2 => bus.read8(addr) as i8 as i32 as u32,
                    _ => bus.read16(addr & !1) as i16 as i32 as u32,
                };
                if writeback {
                    self.set_reg(rn, moved);
                }
                self.set_reg(rd, val);
            }
            (false, 1) => {
                bus.write16(addr & !1, self.r[rd] as u16);
                if writeback {
                    self.set_reg(rn, moved);
                }
            }
            (false, 2) => {
                if rd & 1 != 0 {
                    return self.undefined(op, pc);
                }
                let lo = bus.read32(addr & !3);
                let hi = bus.read32((addr & !3).wrapping_add(4));
                if writeback {
                    self.set_reg(rn, moved);
                }
                self.set_reg(rd, lo);
                self.set_reg(rd + 1, hi);
            }
            _ => {
                if rd & 1 != 0 {
                    return self.undefined(op, pc);
                }
                bus.write32(addr & !3, self.r[rd]);
                bus.write32((addr & !3).wrapping_add(4), self.r[rd + 1]);
                if writeback {
                    self.set_reg(rn, moved);
                }
            }
        }
    }

    fn arm_block<B: Bus>(&mut self, bus: &mut B, op: u32) {
        let rn = ((op >> 16) & 15) as usize;
        let list = op & 0xFFFF;
        let n = list.count_ones();
        let base = self.r[rn];
        let up = op & (1 << 23) != 0;
        let pre = op & (1 << 24) != 0;
        let (start, end) = match (pre, up) {
            (false, true) => (base, base.wrapping_add(4 * n)),
            (true, true) => (base.wrapping_add(4), base.wrapping_add(4 * n)),
            (false, false) => (base.wrapping_sub(4 * n).wrapping_add(4), base.wrapping_sub(4 * n)),
            (true, false) => (base.wrapping_sub(4 * n), base.wrapping_sub(4 * n)),
        };
        let s = op & (1 << 22) != 0;
        let writeback = op & (1 << 21) != 0;
        let mut addr = start & !3;
        if op & (1 << 20) != 0 {
            let user = s && list & 0x8000 == 0;
            if writeback {
                self.set_reg(rn, end);
            }
            for i in 0..16 {
                if list & (1 << i) == 0 {
                    continue;
                }
                let v = bus.read32(addr);
                addr = addr.wrapping_add(4);
                if i == 15 {
                    if s {
                        self.exception_return(v);
                    } else {
                        self.bx(v);
                    }
                } else if user {
                    self.set_user_reg(i, v);
                } else {
                    self.r[i] = v;
                }
            }
        } else {
            for i in 0..16 {
                if list & (1 << i) == 0 {
                    continue;
                }
                let v = if s { self.user_reg(i) } else { self.r[i] };
                bus.write32(addr, v);
                addr = addr.wrapping_add(4);
            }
            if writeback {
                self.set_reg(rn, end);
            }
        }
    }

    // --- Thumb -------------------------------------------------------------

    fn exec_thumb<B: Bus>(&mut self, bus: &mut B, op: u16, pc: u32) {
        let op = op as u32;
        match op >> 11 {
            0x00..=0x02 => {
                // LSL/LSR/ASR by immediate
                let rd = (op & 7) as usize;
                let (v, c) = self.shift_imm(self.r[((op >> 3) & 7) as usize], op >> 11, (op >> 6) & 31);
                self.r[rd] = v;
                self.set_nzc(v, c);
            }
            0x03 => {
                let rd = (op & 7) as usize;
                let a = self.r[((op >> 3) & 7) as usize];
                let n = (op >> 6) & 7;
                let b = if op & (1 << 10) != 0 { n } else { self.r[n as usize] };
                let (res, c, v) = if op & (1 << 9) != 0 { Self::adc(a, !b, 1) } else { Self::adc(a, b, 0) };
                self.r[rd] = res;
                self.set_nzcv(res, c, v);
            }
            0x04..=0x07 => {
                let rd = ((op >> 8) & 7) as usize;
                let imm = op & 0xFF;
                match (op >> 11) & 3 {
                    0 => {
                        self.r[rd] = imm;
                        self.set_nz(imm);
                    }
                    1 => {
                        let (res, c, v) = Self::adc(self.r[rd], !imm, 1);
                        self.set_nzcv(res, c, v);
                    }
                    2 => {
                        let (res, c, v) = Self::adc(self.r[rd], imm, 0);
                        self.r[rd] = res;
                        self.set_nzcv(res, c, v);
                    }
                    _ => {
                        let (res, c, v) = Self::adc(self.r[rd], !imm, 1);
                        self.r[rd] = res;
                        self.set_nzcv(res, c, v);
                    }
                }
            }
            0x08 => {
                if op & 0x400 == 0 {
                    self.thumb_alu(op);
                } else {
                    self.thumb_hi(op, pc);
                }
            }
            0x09 => {
                let rd = ((op >> 8) & 7) as usize;
                let addr = (self.r[15] & !3).wrapping_add((op & 0xFF) * 4);
                self.r[rd] = bus.read32(addr);
            }
            0x0A | 0x0B => {
                let rd = (op & 7) as usize;
                let addr = self.r[((op >> 3) & 7) as usize].wrapping_add(self.r[((op >> 6) & 7) as usize]);
                match (op >> 9) & 7 {
                    0 => bus.write32(addr & !3, self.r[rd]),
                    1 => bus.write16(addr & !1, self.r[rd] as u16),
                    2 => bus.write8(addr, self.r[rd] as u8),
                    3 => self.r[rd] = bus.read8(addr) as i8 as i32 as u32,
                    4 => self.r[rd] = bus.read32(addr & !3).rotate_right((addr & 3) * 8),
                    5 => self.r[rd] = bus.read16(addr & !1) as u32,
                    6 => self.r[rd] = bus.read8(addr) as u32,
                    _ => self.r[rd] = bus.read16(addr & !1) as i16 as i32 as u32,
                }
            }
            0x0C..=0x0F => {
                let rd = (op & 7) as usize;
                let base = self.r[((op >> 3) & 7) as usize];
                let imm = (op >> 6) & 31;
                let byte = op & (1 << 12) != 0;
                let addr = base.wrapping_add(if byte { imm } else { imm * 4 });
                match (byte, op & (1 << 11) != 0) {
                    (false, false) => bus.write32(addr & !3, self.r[rd]),
                    (false, true) => self.r[rd] = bus.read32(addr & !3).rotate_right((addr & 3) * 8),
                    (true, false) => bus.write8(addr, self.r[rd] as u8),
                    (true, true) => self.r[rd] = bus.read8(addr) as u32,
                }
            }
            0x10 | 0x11 => {
                let rd = (op & 7) as usize;
                let addr = self.r[((op >> 3) & 7) as usize].wrapping_add(((op >> 6) & 31) * 2);
                if op & (1 << 11) != 0 {
                    self.r[rd] = bus.read16(addr & !1) as u32;
                } else {
                    bus.write16(addr & !1, self.r[rd] as u16);
                }
            }
            0x12 | 0x13 => {
                let rd = ((op >> 8) & 7) as usize;
                let addr = self.r[13].wrapping_add((op & 0xFF) * 4);
                if op & (1 << 11) != 0 {
                    self.r[rd] = bus.read32(addr & !3).rotate_right((addr & 3) * 8);
                } else {
                    bus.write32(addr & !3, self.r[rd]);
                }
            }
            0x14 | 0x15 => {
                let rd = ((op >> 8) & 7) as usize;
                let base = if op & (1 << 11) != 0 { self.r[13] } else { self.r[15] & !3 };
                self.r[rd] = base.wrapping_add((op & 0xFF) * 4);
            }
            0x16 | 0x17 => self.thumb_misc(bus, op, pc),
            0x18 | 0x19 => {
                let rb = ((op >> 8) & 7) as usize;
                let list = op & 0xFF;
                let mut addr = self.r[rb];
                let end = addr.wrapping_add(4 * list.count_ones());
                if op & (1 << 11) != 0 {
                    for i in 0..8 {
                        if list & (1 << i) != 0 {
                            self.r[i] = bus.read32(addr & !3);
                            addr = addr.wrapping_add(4);
                        }
                    }
                    if list & (1 << rb) == 0 {
                        self.r[rb] = end;
                    }
                } else {
                    for i in 0..8 {
                        if list & (1 << i) != 0 {
                            bus.write32(addr & !3, self.r[i]);
                            addr = addr.wrapping_add(4);
                        }
                    }
                    self.r[rb] = end;
                }
            }
            0x1A | 0x1B => {
                let c = (op >> 8) & 15;
                if c == 0xF {
                    self.fault = Some(Fault::SoftwareInterrupt { addr: pc });
                    self.next = pc;
                } else if c == 0xE {
                    self.undefined(op, pc);
                } else if self.cond(c) {
                    let off = ((op & 0xFF) as u8 as i8 as i32 * 2) as u32;
                    self.next = self.r[15].wrapping_add(off);
                }
            }
            0x1C => {
                let off = ((((op & 0x7FF) << 21) as i32) >> 20) as u32;
                self.next = self.r[15].wrapping_add(off);
            }
            0x1D => self.thumb_bl_suffix(op, pc),
            0x1E => {
                // BL/BLX prefix; with its suffix right behind it, the pair
                // runs as one instruction (as in Unicorn/QEMU)
                let off = ((((op & 0x7FF) << 21) as i32) >> 9) as u32;
                self.r[14] = self.r[15].wrapping_add(off);
                let next = bus.fetch16(pc.wrapping_add(2)) as u32;
                if next >> 11 == 0x1F || (next >> 11 == 0x1D && next & 1 == 0) {
                    self.thumb_bl_suffix(next, pc.wrapping_add(2));
                }
            }
            _ => self.thumb_bl_suffix(op, pc),
        }
    }

    /// Second half of BL (0x1F) or BLX (0x1D), at address pc.
    fn thumb_bl_suffix(&mut self, op: u32, pc: u32) {
        let target = self.r[14].wrapping_add((op & 0x7FF) << 1);
        if op >> 11 == 0x1D {
            if op & 1 != 0 {
                return self.undefined(op, pc);
            }
            self.cpsr &= !FLAG_T;
            self.next = target & !3;
        } else {
            self.next = target & !1;
        }
        self.r[14] = pc.wrapping_add(2) | 1;
    }

    fn thumb_alu(&mut self, op: u32) {
        let rd = (op & 7) as usize;
        let rm = ((op >> 3) & 7) as usize;
        let a = self.r[rd];
        let b = self.r[rm];
        match (op >> 6) & 15 {
            0x0 => {
                let r = a & b;
                self.r[rd] = r;
                self.set_nz(r);
            }
            0x1 => {
                let r = a ^ b;
                self.r[rd] = r;
                self.set_nz(r);
            }
            opc @ (0x2 | 0x3 | 0x4 | 0x7) => {
                let typ = match opc {
                    0x2 => 0,
                    0x3 => 1,
                    0x4 => 2,
                    _ => 3,
                };
                let (r, c) = self.shift_reg(a, typ, b & 0xFF);
                self.r[rd] = r;
                self.set_nzc(r, c);
            }
            0x5 => {
                let (r, c, v) = Self::adc(a, b, self.carry());
                self.r[rd] = r;
                self.set_nzcv(r, c, v);
            }
            0x6 => {
                let (r, c, v) = Self::adc(a, !b, self.carry());
                self.r[rd] = r;
                self.set_nzcv(r, c, v);
            }
            0x8 => self.set_nz(a & b),
            0x9 => {
                let (r, c, v) = Self::adc(0, !b, 1);
                self.r[rd] = r;
                self.set_nzcv(r, c, v);
            }
            0xA => {
                let (r, c, v) = Self::adc(a, !b, 1);
                self.set_nzcv(r, c, v);
            }
            0xB => {
                let (r, c, v) = Self::adc(a, b, 0);
                self.set_nzcv(r, c, v);
            }
            0xC => {
                let r = a | b;
                self.r[rd] = r;
                self.set_nz(r);
            }
            0xD => {
                let r = a.wrapping_mul(b);
                self.r[rd] = r;
                self.set_nz(r);
            }
            0xE => {
                let r = a & !b;
                self.r[rd] = r;
                self.set_nz(r);
            }
            _ => {
                let r = !b;
                self.r[rd] = r;
                self.set_nz(r);
            }
        }
    }

    fn thumb_hi(&mut self, op: u32, pc: u32) {
        let rd = ((op & 7) | ((op >> 4) & 8)) as usize;
        let rm = ((op >> 3) & 15) as usize;
        let b = self.r[rm];
        match (op >> 8) & 3 {
            0 => {
                let r = self.r[rd].wrapping_add(b);
                self.set_reg(rd, r);
            }
            1 => {
                let (r, c, v) = Self::adc(self.r[rd], !b, 1);
                self.set_nzcv(r, c, v);
            }
            2 => self.set_reg(rd, b),
            _ => {
                if op & 0x80 != 0 {
                    self.r[14] = pc.wrapping_add(2) | 1;
                }
                self.bx(b);
            }
        }
    }

    fn thumb_misc<B: Bus>(&mut self, bus: &mut B, op: u32, pc: u32) {
        if op & 0xFF00 == 0xB000 {
            let imm = (op & 0x7F) * 4;
            self.r[13] = if op & 0x80 != 0 { self.r[13].wrapping_sub(imm) } else { self.r[13].wrapping_add(imm) };
        } else if op & 0xF600 == 0xB400 {
            let list = op & 0xFF;
            let extra = op & 0x100 != 0;
            let n = list.count_ones() + extra as u32;
            if op & (1 << 11) != 0 {
                // POP {list, pc}
                let mut addr = self.r[13];
                for i in 0..8 {
                    if list & (1 << i) != 0 {
                        self.r[i] = bus.read32(addr & !3);
                        addr = addr.wrapping_add(4);
                    }
                }
                if extra {
                    let v = bus.read32(addr & !3);
                    self.bx(v);
                }
                self.r[13] = self.r[13].wrapping_add(4 * n);
            } else {
                // PUSH {list, lr}
                let start = self.r[13].wrapping_sub(4 * n);
                let mut addr = start;
                for i in 0..8 {
                    if list & (1 << i) != 0 {
                        bus.write32(addr & !3, self.r[i]);
                        addr = addr.wrapping_add(4);
                    }
                }
                if extra {
                    bus.write32(addr & !3, self.r[14]);
                }
                self.r[13] = start;
            }
        } else if op & 0xFF00 == 0xBE00 {
            self.fault = Some(Fault::Breakpoint { addr: pc });
            self.next = pc;
        } else {
            self.undefined(op, pc);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 64 KiB of RAM at address 0.
    struct Ram(Vec<u8>);

    impl Bus for Ram {
        fn read8(&mut self, a: u32) -> u8 {
            self.0[a as usize]
        }
        fn read16(&mut self, a: u32) -> u16 {
            u16::from_le_bytes([self.0[a as usize], self.0[a as usize + 1]])
        }
        fn read32(&mut self, a: u32) -> u32 {
            u32::from_le_bytes(self.0[a as usize..a as usize + 4].try_into().unwrap())
        }
        fn write8(&mut self, a: u32, v: u8) {
            self.0[a as usize] = v;
        }
        fn write16(&mut self, a: u32, v: u16) {
            self.0[a as usize..a as usize + 2].copy_from_slice(&v.to_le_bytes());
        }
        fn write32(&mut self, a: u32, v: u32) {
            self.0[a as usize..a as usize + 4].copy_from_slice(&v.to_le_bytes());
        }
    }

    fn arm(code: &[u32]) -> (Cpu, Ram) {
        let mut ram = Ram(vec![0; 0x10000]);
        for (i, w) in code.iter().enumerate() {
            ram.write32(i as u32 * 4, *w);
        }
        (Cpu::new(), ram)
    }

    #[test]
    fn arm_add_and_flags() {
        // mov r0,#1; mvn r1,#0; adds r2,r0,r1 -> 0 with carry and zero
        let (mut cpu, mut ram) = arm(&[0xE3A00001, 0xE3E01000, 0xE0902001]);
        for _ in 0..3 {
            cpu.step(&mut ram);
        }
        assert_eq!(cpu.r[2], 0);
        assert_eq!(cpu.cpsr & (FLAG_Z | FLAG_C | FLAG_N | FLAG_V), FLAG_Z | FLAG_C);
        assert_eq!(cpu.pc(), 12);
    }

    #[test]
    fn arm_branch_link_and_bx_to_thumb() {
        // bl +8 -> at 0x10: add r0,pc,#1 (r0 = 0x19); bx r0 -> Thumb at 0x18: movs r1,#5
        let mut code = vec![0xEB000002, 0, 0, 0, 0xE28F0001, 0xE12FFF10];
        code.push(0x0000_2105); // movs r1,#5 at 0x18
        let (mut cpu, mut ram) = arm(&code);
        cpu.step(&mut ram);
        assert_eq!(cpu.pc(), 0x10);
        assert_eq!(cpu.r[14], 4);
        cpu.step(&mut ram);
        assert_eq!(cpu.r[0], 0x19);
        cpu.step(&mut ram);
        assert!(cpu.thumb());
        assert_eq!(cpu.pc(), 0x18);
        cpu.step(&mut ram);
        assert_eq!(cpu.r[1], 5);
    }

    #[test]
    fn thumb_push_pop_and_bl() {
        let mut ram = Ram(vec![0; 0x10000]);
        // 0: bl 0x100 (prefix+suffix) ; 0x100: push {r4,lr}; movs r4,#7; pop {r4,pc}
        ram.write16(0, 0xF000);
        ram.write16(2, 0xF87E);
        ram.write16(0x100, 0xB510);
        ram.write16(0x102, 0x2407);
        ram.write16(0x104, 0xBD10);
        let mut cpu = Cpu::new();
        cpu.cpsr |= FLAG_T;
        cpu.r[13] = 0x8000;
        cpu.r[4] = 0x1234;
        cpu.step(&mut ram); // the BL pair is one step
        assert_eq!(cpu.pc(), 0x100);
        assert_eq!(cpu.r[14], 5);
        for _ in 0..3 {
            cpu.step(&mut ram);
        }
        assert_eq!(cpu.pc(), 4);
        assert!(cpu.thumb());
        assert_eq!(cpu.r[4], 0x1234);
        assert_eq!(cpu.r[13], 0x8000);
    }

    #[test]
    fn irq_entry_and_return() {
        // IRQ handler at 0x40: subs pc, lr, #4
        let mut code = vec![0xE1A00000; 16];
        code.push(0xE25EF004);
        let (mut cpu, mut ram) = arm(&code);
        cpu.set_mode(MODE_SVC);
        cpu.r[13] = 0x1000;
        cpu.r[15] = 0x20;
        cpu.enter_irq(0x40);
        assert_eq!(cpu.mode(), MODE_IRQ);
        assert_eq!(cpu.r[13], 0);
        cpu.step(&mut ram);
        assert_eq!(cpu.mode(), MODE_SVC);
        assert_eq!(cpu.r[13], 0x1000);
        assert_eq!(cpu.pc(), 0x20);
    }
}
