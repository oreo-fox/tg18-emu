//! tg18 emulator core: Tamagotchi Meets / On (GeneralPlus ARM926 SoC).
//!
//! Rust port of the Python prototype in `prototype/`. No window or sound
//! output here, so it can be used by the desktop app, tools and (later)
//! Android alike.

pub mod cpu;
pub mod machine;
pub mod png;
pub mod roms;
pub mod save;
pub mod sigs;
pub mod snapshot;
pub mod sound;
pub mod sys;

pub use machine::{Machine, Outcome, CPU_HZ};
pub use sys::Key;

/// Flash image, executing in place (plus 8 MiB of RAM above it).
pub const FLASH_BASE: u32 = 0x2000_0000;
pub const FLASH_SIZE: usize = 0x80_0000;
pub const RAM_SIZE: usize = 0x100_0000;
/// Internal SRAM: .data/.bss, stacks, routines copied there at boot.
pub const SRAM_BASE: u32 = 0xF800_0000;
pub const SRAM_SIZE: usize = 0x10_0000;

/// Saves and snapshots must come from the same ROM version: the first part
/// of the image (code) is compared.
pub const CODE_CHECK_LEN: usize = 0x1E_0000;

/// True if this looks like a tg18 SPI flash dump.
pub fn is_image(data: &[u8]) -> bool {
    data.len() == FLASH_SIZE && data.starts_with(b"SPII")
}

/// FNV-1a hash of the ROM's code part, to recognise its saves.
pub fn code_hash(image: &[u8]) -> u64 {
    let mut h: u64 = 0xCBF2_9CE4_8422_2325;
    for &b in &image[..CODE_CHECK_LEN.min(image.len())] {
        h ^= b as u64;
        h = h.wrapping_mul(0x100_0000_01B3);
    }
    h
}

/// Seconds from the RTC's epoch (2007-12-31 00:00) to a date and time.
pub fn rtc_seconds(year: i64, month: u32, day: u32, hour: u32, minute: u32, second: u32) -> f64 {
    let days = days_from_civil(year, month, day) - days_from_civil(2007, 12, 31);
    (days * 86400 + hour as i64 * 3600 + minute as i64 * 60 + second as i64) as f64
}

/// RTC seconds for the local time now.
pub fn rtc_seconds_now() -> f64 {
    let (y, mo, d, h, mi, s, ms) = local_time();
    rtc_seconds(y, mo, d, h, mi, s) + ms as f64 / 1000.0
}

/// Local date and time: (year, month, day, hour, minute, second, ms).
#[cfg(windows)]
pub fn local_time() -> (i64, u32, u32, u32, u32, u32, u32) {
    #[repr(C)]
    #[derive(Default)]
    struct SystemTime {
        year: u16,
        month: u16,
        day_of_week: u16,
        day: u16,
        hour: u16,
        minute: u16,
        second: u16,
        ms: u16,
    }
    #[link(name = "kernel32")]
    extern "system" {
        fn GetLocalTime(t: *mut SystemTime);
    }
    let mut t = SystemTime::default();
    unsafe { GetLocalTime(&mut t) };
    (t.year as i64, t.month as u32, t.day as u32, t.hour as u32, t.minute as u32, t.second as u32, t.ms as u32)
}

/// Local date and time (UTC where the time zone is not known yet).
#[cfg(not(windows))]
pub fn local_time() -> (i64, u32, u32, u32, u32, u32, u32) {
    let t = unix_now();
    let days = (t / 86400.0).floor() as i64;
    let secs = (t - days as f64 * 86400.0) as u32;
    let (y, m, d) = civil_from_days(days);
    (y, m, d, secs / 3600, secs / 60 % 60, secs % 60, ((t.fract()) * 1000.0) as u32)
}

/// Inverse of days_from_civil.
pub fn civil_from_days(z: i64) -> (i64, u32, u32) {
    let z = z + 719468;
    let era = z.div_euclid(146097);
    let doe = z - era * 146097;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    (yoe + era * 400 + if m <= 2 { 1 } else { 0 }, m, d)
}

/// Unix time now (seconds, UTC).
pub fn unix_now() -> f64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs_f64())
        .unwrap_or(0.0)
}

/// Days since 1970-01-01 of a proleptic Gregorian date.
pub fn days_from_civil(y: i64, m: u32, d: u32) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let m = m as i64;
    let doy = (153 * (if m > 2 { m - 3 } else { m + 9 }) + 2) / 5 + d as i64 - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146097 + doe - 719468
}
