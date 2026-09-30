//! Firmware routines found by their code patterns, so the same emulator
//! works on all nine known images (see the Python prototype for how each
//! was found).

use crate::FLASH_BASE;

/// A byte pattern; `None` matches any byte.
pub struct Pattern(Vec<Option<u8>>);

impl Pattern {
    /// Hex bytes, spaces ignored, `??` = any byte.
    pub fn parse(text: &str) -> Pattern {
        let s: Vec<char> = text.chars().filter(|c| !c.is_whitespace()).collect();
        let mut out = Vec::with_capacity(s.len() / 2);
        for pair in s.chunks(2) {
            if pair == ['?', '?'] {
                out.push(None);
            } else {
                let hex: String = pair.iter().collect();
                out.push(Some(u8::from_str_radix(&hex, 16).expect("bad hex in pattern")));
            }
        }
        Pattern(out)
    }

    pub fn len(&self) -> usize {
        self.0.len()
    }

    pub fn matches_at(&self, hay: &[u8], i: usize) -> bool {
        i + self.0.len() <= hay.len()
            && self.0.iter().zip(&hay[i..]).all(|(p, b)| p.map_or(true, |p| p == *b))
    }

    /// Offset of the first match in hay[start..end].
    pub fn find(&self, hay: &[u8], start: usize, end: usize) -> Option<usize> {
        let end = end.min(hay.len());
        let first = self.0[0]?;
        let mut i = start;
        while i + self.0.len() <= end {
            match hay[i..end].iter().position(|&b| b == first) {
                None => return None,
                Some(p) => {
                    i += p;
                    if self.matches_at(hay, i) && i + self.0.len() <= end {
                        return Some(i);
                    }
                    i += 1;
                }
            }
        }
        None
    }

    pub fn find_all(&self, hay: &[u8], start: usize, end: usize) -> Vec<usize> {
        let mut out = Vec::new();
        let mut i = start;
        while let Some(p) = self.find(hay, i, end) {
            out.push(p);
            i = p + 1;
        }
        out
    }
}

/// Code is searched in the first part of the image.
pub const CODE_END: usize = 0x220000;

/// newlib rand(): the 64-bit LCG step, from `ldr lr,[r0,#0xa8]` on.
pub const RAND_SIG: &str = "a8e090e5910e03e02cc09fe5ac1090e59c3121e09e2c83e0012092e2033081e00030a3e2a82080e5ac3080e5";
pub const RAND_STATE_OFF: u32 = 0xA8;

/// flash_write(cpu_addr, buf, len) of the SPI flash driver, copied to SRAM at boot.
pub const FLASH_WRITE_SIG: &str =
    "f0472de90070a0e10180a0e1ff4000e2023084e0ff5083e22554a0e1014c64e2020054e10240a021010c54e34000000aff0053e33400009a022080e0ff6002e2";

/// The game's halfword save loop (Thumb, in SRAM); ?? = the bl to write_halfword.
pub const SAVE_LOOP_SIG: &str = "444600255b461f1b395b802292052000 ???????? 061e0fd102340135a945f3d1";
pub const SAVE_LOOP_EXIT: u32 = 0x20;

/// Software delay loop (Thumb): mov r8,r8; subs r3,#1; cmp r3,#0; bne back.
pub const SPIN_LOOP: &str = "c046013b002bfbd1";
pub const SPIN_LOOP_INSNS: u64 = 4;

/// The game's clock refresh (Thumb).
pub const CLOCK_REFRESH_SIG: &str = "70b58ab0060006a8 ?? 4b ???????? 099b019303a91800 ?? 4b ???????? 049d ?? 4c 2573 0599 6173 089a a273 079b e373";

/// spi_transfer(dev, tx, txlen, rx, rxlen on stack) of the second SPI master (Bluetooth).
pub const SPI2_XFER_SIG: &str =
    "f0412de908d04de20050a0e10160a0e10270a0e10380a0e1c6fcffeb000050e3100000ba44409fe5003094e540209fe5202083e520309de500308de50830a0e1";

/// printf is a stub `push {r0-r3}; add sp,#0x10; bx lr` in release builds.
pub const PRINTF_STUB: &str = "0f002de9 10d08de2 1eff2fe1";

fn u16_at(b: &[u8], i: usize) -> u16 {
    u16::from_le_bytes([b[i], b[i + 1]])
}

pub fn u32_at(b: &[u8], i: usize) -> u32 {
    u32::from_le_bytes([b[i], b[i + 1], b[i + 2], b[i + 3]])
}

/// CPU address of the first match of a pattern in the code area.
pub fn find_code(image: &[u8], sig: &str, end: usize) -> Option<u32> {
    Pattern::parse(sig).find(image, 0, end).map(|o| FLASH_BASE + o as u32)
}

/// Address of the firmware's idle-seconds counter (see the prototype's
/// find_idle_counter): `ldrb r3,[rN,#k]; cmp r3,#limit; bhi`, then
/// `movs r2,#2 ... strb r2,[r3]`; the counter is field k of the struct
/// loaded by `ldr rN,[pc,#imm]` just before.
pub fn find_idle_counter(image: &[u8]) -> Option<u32> {
    let code = &image[..CODE_END.min(image.len())];
    let mut found: Vec<u32> = Vec::new();
    for i in 18..code.len().saturating_sub(16) {
        if !(code[i] >= 0xC0 && (code[i + 1] == 0x78 || code[i + 1] == 0x79) && code[i + 3] == 0x2B && code[i + 5] == 0xD8) {
            continue;
        }
        let ins = u16_at(code, i);
        if ins >> 11 != 0b01111 || ins & 7 != 3 {
            continue;
        }
        let rn = (ins >> 3) & 7;
        let k = ((ins >> 6) & 31) as u32;
        let tail = &code[i + 6..i + 16];
        let j = match tail.windows(2).position(|w| w == [0x02, 0x22]) {
            Some(j) => j,
            None => continue,
        };
        let rest = &tail[j..(j + 8).min(tail.len())];
        if !rest.windows(2).any(|w| w == [0x1A, 0x70]) {
            continue;
        }
        let mut back = 2;
        while back < 18 {
            let ld = u16_at(code, i - back);
            if ld >> 11 == 0b01001 && (ld >> 8) & 7 == rn {
                let lit = ((i - back + 4) & !3) + (ld & 0xFF) as usize * 4;
                let base = u32_at(image, lit);
                if (crate::SRAM_BASE..crate::SRAM_BASE + crate::SRAM_SIZE as u32).contains(&base) {
                    if !found.contains(&(base + k)) {
                        found.push(base + k);
                    }
                }
                break;
            }
            back += 2;
        }
    }
    if found.len() == 1 { Some(found[0]) } else { None }
}

/// Address of the `while (1) delay(500)` after "BLE Initial Fail".
pub fn find_ble_fail_loop(image: &[u8]) -> Option<u32> {
    let s = image.windows(16).position(|w| w == b"BLE Initial Fail")?;
    let pool = (FLASH_BASE + s as u32).to_le_bytes();
    let code = &image[..CODE_END.min(image.len())];
    let mut p = 0;
    while let Some(q) = code[p..].windows(4).position(|w| w == pool) {
        let at = p + q;
        p = at + 1;
        let mut back = 8;
        while back < 0x1000 && back <= at {
            let ins = u32_at(image, at - back);
            if ins & 0xFFFF_F000 == 0xE59F_0000 && (ins & 0xFFF) as usize == back - 8 {
                let i = at - back;
                let mut j = i;
                while j < i + 0x40 {
                    if u32_at(image, j) == 0xEAFF_FFFB {
                        return Some(FLASH_BASE + j as u32);
                    }
                    j += 4;
                }
                break;
            }
            back += 4;
        }
    }
    None
}

/// Target of a Thumb BL pair at `at` (address `addr`).
pub fn thumb_bl_target(hi: u16, lo: u16, addr: u32) -> Option<u32> {
    if hi >> 11 != 0b11110 || lo >> 11 != 0b11111 {
        return None;
    }
    let rel = ((hi as u32 & 0x7FF) << 12) | ((lo as u32 & 0x7FF) << 1);
    let rel = if rel & (1 << 22) != 0 { rel.wrapping_sub(1 << 23) } else { rel };
    Some(addr.wrapping_add(4).wrapping_add(rel))
}
