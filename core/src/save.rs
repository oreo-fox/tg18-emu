//! The toy's own save: the flash image plus FILE.json with the clock.
//! Same files as the Python prototype writes, so saves work in both.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::machine::Machine;
use crate::snapshot::SnapError;
use crate::*;

#[derive(Serialize, Deserialize, Clone, Debug, Default)]
pub struct SaveMeta {
    pub rtc_ticks: i64,
    /// RTC chip registers that were set (keys are register numbers).
    pub rtc_regs: BTreeMap<String, u8>,
    /// Unix time of the save.
    pub saved_at: f64,
    /// Written after the device saved itself by going to sleep.
    #[serde(default)]
    pub clean: bool,
    #[serde(default)]
    pub asleep: bool,
}

pub fn meta_path(path: &str) -> String {
    format!("{}.json", path)
}

pub fn read_meta(path: &str) -> Option<SaveMeta> {
    let text = std::fs::read_to_string(meta_path(path)).ok()?;
    serde_json::from_str(&text).ok()
}

/// Flash contents of a save, None if there is none yet.
pub fn load_flash(path: &str, image: &[u8]) -> Result<Option<Vec<u8>>, SnapError> {
    let flash = match std::fs::read(path) {
        Ok(f) => f,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(e.into()),
    };
    if flash.len() != image.len() || flash[..CODE_CHECK_LEN] != image[..CODE_CHECK_LEN] {
        return Err(SnapError::Mismatch);
    }
    Ok(Some(flash))
}

pub fn meta(m: &Machine, clean: bool, asleep: bool) -> SaveMeta {
    let mut regs = BTreeMap::new();
    // the prototype stored only registers that were written; all set ones
    // are the ones that matter (validity marker, alarm, sleep state)
    for (i, v) in m.sys.rtc.regs.iter().enumerate() {
        if *v != 0 {
            regs.insert(i.to_string(), *v);
        }
    }
    SaveMeta {
        rtc_ticks: m.sys.rtc.ticks(m.sys.game_time),
        rtc_regs: regs,
        saved_at: unix_now(),
        clean,
        asleep,
    }
}

/// Write flash + meta (safe to call from a background thread).
pub fn write(flash: &[u8], meta: &SaveMeta, path: &str) -> std::io::Result<()> {
    let tmp = format!("{}.tmp", path);
    std::fs::write(&tmp, flash)?;
    std::fs::rename(&tmp, path)?;
    let mp = meta_path(path);
    let tmp = format!("{}.tmp", mp);
    std::fs::write(&tmp, serde_json::to_string_pretty(meta).unwrap())?;
    std::fs::rename(&tmp, &mp)
}

pub fn write_machine(m: &Machine, path: &str, clean: bool, asleep: bool) -> std::io::Result<()> {
    write(&m.sys.ram[..FLASH_SIZE], &meta(m, clean, asleep), path)
}

/// Carry the RTC over from the last session. With `advance`, the real time
/// since the save is added, as on a real toy whose clock runs while it's off.
pub fn restore_rtc(m: &mut Machine, meta: &SaveMeta, advance: bool) {
    let rtc = &mut m.sys.rtc;
    rtc.regs = [0; 256];
    for (k, v) in &meta.rtc_regs {
        if let Ok(i) = k.parse::<usize>() {
            if i < 256 {
                rtc.regs[i] = *v;
            }
        }
    }
    let elapsed = if advance { (unix_now() - meta.saved_at).max(0.0) } else { 0.0 };
    rtc.base_ticks = meta.rtc_ticks + (elapsed * 32768.0) as i64;
    rtc.base_time = m.sys.game_time;
}
