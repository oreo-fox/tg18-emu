//! Snapshots: the whole machine frozen to a file.
//!
//! Format: `T18SNAP1`, then a zlib stream of chunks, each
//! `[u8 name length][name][u32 data length][data]`, all little-endian.
//! `prototype/testing/to_rust_snap.py` writes the same format from the
//! Python prototype's snapshots.

use std::collections::HashMap;

use crate::machine::Machine;
use crate::*;

const MAGIC: &[u8; 8] = b"T18SNAP1";

#[derive(Debug)]
pub enum SnapError {
    Io(std::io::Error),
    Format(String),
    /// The snapshot or save belongs to a different ROM version.
    Mismatch,
}

impl std::fmt::Display for SnapError {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        match self {
            SnapError::Io(e) => write!(f, "{}", e),
            SnapError::Format(s) => write!(f, "not a valid snapshot: {}", s),
            SnapError::Mismatch => write!(f, "it belongs to a different ROM version"),
        }
    }
}

impl From<std::io::Error> for SnapError {
    fn from(e: std::io::Error) -> Self {
        SnapError::Io(e)
    }
}

#[derive(Default)]
struct W(Vec<u8>);

impl W {
    fn u8(&mut self, v: u8) -> &mut Self {
        self.0.push(v);
        self
    }
    fn u32(&mut self, v: u32) -> &mut Self {
        self.0.extend_from_slice(&v.to_le_bytes());
        self
    }
    fn u64(&mut self, v: u64) -> &mut Self {
        self.0.extend_from_slice(&v.to_le_bytes());
        self
    }
    fn i64(&mut self, v: i64) -> &mut Self {
        self.0.extend_from_slice(&v.to_le_bytes());
        self
    }
    fn f64(&mut self, v: f64) -> &mut Self {
        self.0.extend_from_slice(&v.to_le_bytes());
        self
    }
    fn bytes(&mut self, v: &[u8]) -> &mut Self {
        self.0.extend_from_slice(v);
        self
    }
}

struct R<'a>(&'a [u8], usize);

impl<'a> R<'a> {
    fn take(&mut self, n: usize) -> Result<&'a [u8], SnapError> {
        if self.1 + n > self.0.len() {
            return Err(SnapError::Format("chunk too short".into()));
        }
        let s = &self.0[self.1..self.1 + n];
        self.1 += n;
        Ok(s)
    }
    fn u8(&mut self) -> Result<u8, SnapError> {
        Ok(self.take(1)?[0])
    }
    fn u32(&mut self) -> Result<u32, SnapError> {
        Ok(u32::from_le_bytes(self.take(4)?.try_into().unwrap()))
    }
    fn u64(&mut self) -> Result<u64, SnapError> {
        Ok(u64::from_le_bytes(self.take(8)?.try_into().unwrap()))
    }
    fn i64(&mut self) -> Result<i64, SnapError> {
        Ok(i64::from_le_bytes(self.take(8)?.try_into().unwrap()))
    }
    fn f64(&mut self) -> Result<f64, SnapError> {
        Ok(f64::from_le_bytes(self.take(8)?.try_into().unwrap()))
    }
}

/// A frozen machine, in memory (cheap enough to keep one per button press
/// for the Bluetooth/infrared undo).
pub struct Snapshot {
    chunks: Vec<(String, Vec<u8>)>,
}

impl Snapshot {
    pub fn capture(m: &Machine) -> Snapshot {
        let s = &m.sys;
        let mut chunks: Vec<(String, Vec<u8>)> = Vec::new();
        let mut add = |name: &str, w: W| chunks.push((name.to_string(), w.0));

        let mut w = W::default();
        w.u64(code_hash(&m.image)).f64(unix_now());
        add("info", w);
        let mut w = W::default();
        for v in m.cpu.state() {
            w.u32(v);
        }
        add("cpu", w);
        add("ram", W(s.ram.clone()));
        add("sram", W(s.sram.clone()));
        add("gpio", W(s.gpio.clone()));
        if s.uart_is_ram {
            add("uart", W(s.uart.clone()));
        }
        let mut w = W::default();
        for (page, mem) in &s.extra {
            w.u32(*page).bytes(mem);
        }
        add("extra", w);
        let mut w = W::default();
        let mut regs: Vec<_> = s.regs.iter().collect();
        regs.sort();
        for (a, v) in regs {
            w.u32(*a).u32(*v);
        }
        add("regs", w);
        let mut w = W::default();
        w.u64(m.executed).u64(m.irqs).u32(s.rand_state);
        w.u8(m.bluetooth_used as u8).u8(s.infrared_used as u8).u8(s.infrared_armed as u8);
        add("machine", w);
        let mut w = W::default();
        w.f64(s.now).f64(s.game_time).u8(s.powered_off as u8);
        for v in s.gpio_applied {
            w.u32(v);
        }
        add("time", w);
        let mut w = W::default();
        for t in &s.timers {
            w.f64(t.start).u64(t.periods_seen).u8(t.pending as u8);
        }
        add("timers", w);
        let l = &s.lcd;
        let mut w = W::default();
        w.bytes(&l.fb).u32(l.cmd.map_or(u32::MAX, |c| c as u32)).u32(l.args.len() as u32).bytes(&l.args);
        for v in [l.x0, l.x1, l.y0, l.y1, l.x, l.y] {
            w.u32(v);
        }
        w.u32(l.half.map_or(u32::MAX, |c| c as u32)).u64(l.frames).u64(l.last_vsync);
        add("lcd", w);
        let mut w = W::default();
        w.bytes(&s.rtc.regs).i64(s.rtc.base_ticks).f64(s.rtc.base_time).u32(s.rtc.data_out);
        add("rtc", w);
        let mut w = W::default();
        w.u64(s.rtc_irq.fired).i64(s.rtc_irq.tod_base).f64(s.rtc_irq.tod_set_at);
        add("rtcirq", w);
        let mut w = W::default();
        w.u8(s.adc_converting as u8);
        add("adc", w);
        let f = &s.flash;
        let mut w = W::default();
        w.u8(f.cs as u8).u32(f.buf.len() as u32).bytes(&f.buf).u32(f.rx).u8(f.wel as u8).u64(f.writes);
        add("flash", w);
        Snapshot { chunks }
    }

    fn get(&self, name: &str) -> Option<&[u8]> {
        self.chunks.iter().find(|c| c.0 == name).map(|c| &c.1[..])
    }

    /// When the snapshot was taken (Unix time).
    pub fn saved_at(&self) -> f64 {
        self.get("info").and_then(|d| R(d, 8).f64().ok()).unwrap_or(0.0)
    }

    /// Restore into a machine made for the same ROM.
    pub fn restore(&self, m: &mut Machine) -> Result<(), SnapError> {
        let need = |n: &str| self.get(n).ok_or_else(|| SnapError::Format(format!("missing {}", n)));
        let mut r = R(need("info")?, 0);
        if r.u64()? != code_hash(&m.image) {
            return Err(SnapError::Mismatch);
        }
        let mut r = R(need("cpu")?, 0);
        let mut st = [0u32; cpu::STATE_WORDS];
        for v in st.iter_mut() {
            *v = r.u32()?;
        }
        m.cpu.set_state(&st);
        let s = &mut m.sys;
        let copy = |dst: &mut Vec<u8>, src: &[u8], what: &str| -> Result<(), SnapError> {
            if src.len() != dst.len() {
                return Err(SnapError::Format(format!("{} has the wrong size", what)));
            }
            dst.copy_from_slice(src);
            Ok(())
        };
        copy(&mut s.ram, need("ram")?, "ram")?;
        copy(&mut s.sram, need("sram")?, "sram")?;
        copy(&mut s.gpio, need("gpio")?, "gpio")?;
        s.uart_is_ram = false;
        if let Some(u) = self.get("uart") {
            copy(&mut s.uart, u, "uart")?;
            s.uart_is_ram = true;
        }
        s.extra.clear();
        let mut r = R(need("extra")?, 0);
        while r.1 < r.0.len() {
            let page = r.u32()?;
            s.extra.insert(page, r.take(0x10000)?.to_vec());
        }
        s.regs.clear();
        let mut r = R(need("regs")?, 0);
        while r.1 < r.0.len() {
            let a = r.u32()?;
            let v = r.u32()?;
            s.regs.insert(a, v);
        }
        let mut r = R(need("machine")?, 0);
        m.executed = r.u64()?;
        m.irqs = r.u64()?;
        s.rand_state = r.u32()?;
        m.bluetooth_used = r.u8()? != 0;
        s.infrared_used = r.u8()? != 0;
        s.infrared_armed = r.u8()? != 0;
        let mut r = R(need("time")?, 0);
        s.now = r.f64()?;
        s.game_time = r.f64()?;
        s.powered_off = r.u8()? != 0;
        for v in s.gpio_applied.iter_mut() {
            *v = r.u32()?;
        }
        let mut r = R(need("timers")?, 0);
        for t in s.timers.iter_mut() {
            t.start = r.f64()?;
            t.periods_seen = r.u64()?;
            t.pending = r.u8()? != 0;
        }
        s.timers_resync();
        let mut r = R(need("lcd")?, 0);
        let l = &mut s.lcd;
        let n = l.fb.len();
        l.fb.copy_from_slice(r.take(n)?);
        let c = r.u32()?;
        l.cmd = if c == u32::MAX { None } else { Some(c as u8) };
        let n = r.u32()? as usize;
        l.args = r.take(n)?.to_vec();
        l.x0 = r.u32()?;
        l.x1 = r.u32()?;
        l.y0 = r.u32()?;
        l.y1 = r.u32()?;
        l.x = r.u32()?;
        l.y = r.u32()?;
        let h = r.u32()?;
        l.half = if h == u32::MAX { None } else { Some(h as u8) };
        l.frames = r.u64()?;
        l.last_vsync = r.u64()?;
        let mut r = R(need("rtc")?, 0);
        s.rtc.regs.copy_from_slice(r.take(256)?);
        s.rtc.base_ticks = r.i64()?;
        s.rtc.base_time = r.f64()?;
        s.rtc.data_out = r.u32()?;
        let mut r = R(need("rtcirq")?, 0);
        s.rtc_irq.fired = r.u64()?;
        s.rtc_irq.tod_base = r.i64()?;
        s.rtc_irq.tod_set_at = r.f64()?;
        s.adc_converting = need("adc")?[0] != 0;
        let mut r = R(need("flash")?, 0);
        s.flash.cs = r.u8()? != 0;
        let n = r.u32()? as usize;
        s.flash.buf = r.take(n)?.to_vec();
        s.flash.rx = r.u32()?;
        s.flash.wel = r.u8()? != 0;
        s.flash.writes = r.u64()?;
        s.flash_first_use = false;
        s.stop = false;
        s.sound.state = (0.0, 0.5);
        m.hook_flash_write(); // SRAM code is already in place
        Ok(())
    }

    pub fn to_bytes(&self) -> Vec<u8> {
        let mut payload = Vec::new();
        for (name, data) in &self.chunks {
            payload.push(name.len() as u8);
            payload.extend_from_slice(name.as_bytes());
            payload.extend_from_slice(&(data.len() as u32).to_le_bytes());
            payload.extend_from_slice(data);
        }
        let mut out = MAGIC.to_vec();
        out.extend_from_slice(&miniz_oxide::deflate::compress_to_vec_zlib(&payload, 3));
        out
    }

    pub fn from_bytes(data: &[u8]) -> Result<Snapshot, SnapError> {
        if !data.starts_with(MAGIC) {
            return Err(SnapError::Format("wrong file type".into()));
        }
        let payload = miniz_oxide::inflate::decompress_to_vec_zlib(&data[8..])
            .map_err(|e| SnapError::Format(format!("{:?}", e)))?;
        let mut chunks = Vec::new();
        let mut r = R(&payload, 0);
        while r.1 < payload.len() {
            let n = r.u8()? as usize;
            let name = String::from_utf8_lossy(r.take(n)?).to_string();
            let len = r.u32()? as usize;
            chunks.push((name, r.take(len)?.to_vec()));
        }
        Ok(Snapshot { chunks })
    }

    /// Write to a file; the file is replaced only once complete.
    pub fn save(&self, path: &str) -> std::io::Result<()> {
        let tmp = format!("{}.tmp", path);
        std::fs::write(&tmp, self.to_bytes())?;
        std::fs::rename(&tmp, path)
    }

    pub fn load(path: &str) -> Result<Snapshot, SnapError> {
        Snapshot::from_bytes(&std::fs::read(path)?)
    }
}

/// Snapshot chunk names, for tools.
pub fn chunk_names(s: &Snapshot) -> HashMap<String, usize> {
    s.chunks.iter().map(|c| (c.0.clone(), c.1.len())).collect()
}
