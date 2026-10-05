//! Buzzer sound: turns timer-4 events into 16-bit samples.

/// Square-wave synthesiser that keeps the phase smooth across notes.
pub struct ToneSynth {
    pub rate: f64,
    pub amp: i32,
    pub freq: f64,
    pub duty: f64,
    phase: f64,
}

impl ToneSynth {
    pub fn new(rate: u32, volume: f64) -> ToneSynth {
        ToneSynth { rate: rate as f64, amp: (12000.0 * volume) as i32, freq: 0.0, duty: 0.5, phase: 0.0 }
    }

    pub fn set_volume(&mut self, volume: f64) {
        self.amp = (12000.0 * volume) as i32;
    }

    /// Append n samples of the current tone. Each sample is the average of
    /// the square wave over the sample's time (not a point sample), which
    /// removes most of the aliasing that adds off-key overtones to high notes.
    pub fn tone(&mut self, n: usize, out: &mut Vec<i16>) {
        if self.freq == 0.0 || self.amp == 0 || self.freq >= self.rate / 2.0 {
            out.extend(std::iter::repeat(0).take(n));
            return;
        }
        let step = self.freq / self.rate;
        let (mut ph, duty, amp) = (self.phase, self.duty, self.amp as f64);
        let scale = 2.0 * amp / step;
        for _ in 0..n {
            let end = ph + step;
            let high = (end.min(duty) - ph).max(0.0) + (end.min(1.0 + duty) - ph.max(1.0)).max(0.0);
            out.push((high * scale - amp) as i16);
            ph = if end >= 1.0 { end - 1.0 } else { end };
        }
        self.phase = ph;
    }

    /// Append n samples covering emulated time t0..t1; events are this
    /// span's (time, freq, duty) changes in order. If n doesn't match the
    /// span's length, the span is stretched to fit (slow motion keeps pitch).
    pub fn render(&mut self, events: &[(f64, f64, f64)], t0: f64, t1: f64, n: usize, out: &mut Vec<i16>) {
        let span = t1 - t0;
        let mut done = 0;
        for &(t, freq, duty) in events {
            let upto = if span <= 0.0 { n } else { n.min(((t - t0) / span * n as f64).round().max(0.0) as usize) };
            if upto > done {
                self.tone(upto - done, out);
                done = upto;
            }
            self.freq = freq;
            self.duty = duty;
        }
        if n > done {
            self.tone(n - done, out);
        }
    }
}

/// Silences longer than this are shortened in WAV files (deep sleep).
pub const MAX_GAP: f64 = 2.0;

/// Render buzzer events between t0 and t1 into a 16-bit mono WAV file.
pub fn write_wav(path: &str, events: &[(f64, f64, f64)], t0: f64, t1: f64, rate: u32) -> std::io::Result<()> {
    let mut synth = ToneSynth::new(rate, 0.5);
    let mut out: Vec<i16> = Vec::new();
    for e in events {
        if e.0 <= t0 {
            synth.freq = e.1;
            synth.duty = e.2;
        }
    }
    let inside: Vec<_> = events.iter().filter(|e| t0 < e.0 && e.0 < t1).copied().collect();
    let mut edges: Vec<f64> = inside.iter().map(|e| e.0).collect();
    edges.push(t1);
    let mut pending = inside.into_iter().peekable();
    let mut t = t0;
    for edge in edges {
        let mut span = edge - t;
        if synth.freq == 0.0 {
            span = span.min(MAX_GAP);
        }
        synth.render(&[], t, edge, (span * rate as f64).round() as usize, &mut out);
        while let Some(e) = pending.peek() {
            if e.0 > edge {
                break;
            }
            synth.freq = e.1;
            synth.duty = e.2;
            pending.next();
        }
        t = edge;
    }
    let mut f = Vec::with_capacity(44 + out.len() * 2);
    let data_len = (out.len() * 2) as u32;
    f.extend_from_slice(b"RIFF");
    f.extend_from_slice(&(36 + data_len).to_le_bytes());
    f.extend_from_slice(b"WAVEfmt ");
    f.extend_from_slice(&16u32.to_le_bytes());
    f.extend_from_slice(&1u16.to_le_bytes());
    f.extend_from_slice(&1u16.to_le_bytes());
    f.extend_from_slice(&rate.to_le_bytes());
    f.extend_from_slice(&(rate * 2).to_le_bytes());
    f.extend_from_slice(&2u16.to_le_bytes());
    f.extend_from_slice(&16u16.to_le_bytes());
    f.extend_from_slice(b"data");
    f.extend_from_slice(&data_len.to_le_bytes());
    for s in out {
        f.extend_from_slice(&s.to_le_bytes());
    }
    std::fs::write(path, f)
}

/// Recognises the pet's care call: the 5-note tune F7 D7 F7 G7 C8
/// (64, 256, 64, 64 and 128 ms) the toy plays when the pet needs
/// something, awake or asleep. Same tune in every version measured (EN
/// Fairy, Magic, Wonder Garden). Other sounds the toy makes by itself
/// (events, animations ending) don't match it.
#[derive(Default)]
pub struct CallDetector {
    /// Recent notes as (pitch Hz, start, end), oldest first.
    notes: std::collections::VecDeque<(f64, f64, f64)>,
    /// The note playing now: (pitch, start).
    cur: Option<(f64, f64)>,
}

/// The care call as (pitch Hz, length s).
const CALL_TUNE: [(f64, f64); 5] = [(2793.0, 0.064), (2350.0, 0.256), (2793.0, 0.064), (3136.0, 0.064), (4187.0, 0.128)];

impl CallDetector {
    /// Feed the buzzer's (time, freq, duty) changes in order; true if the
    /// call tune just finished.
    pub fn feed(&mut self, events: &[(f64, f64, f64)]) -> bool {
        let mut heard = false;
        for &(t, freq, _) in events {
            if let Some((f, start)) = self.cur {
                if f == freq {
                    continue;
                }
                self.notes.push_back((f, start, t));
                if self.notes.len() > CALL_TUNE.len() {
                    self.notes.pop_front();
                }
                heard |= self.is_call();
            }
            self.cur = (freq > 0.0).then_some((freq, t));
        }
        heard
    }

    fn is_call(&self) -> bool {
        if self.notes.len() < CALL_TUNE.len() {
            return false;
        }
        let close = |a: f64, b: f64, tol: f64| (a - b).abs() <= tol;
        let tune_ok = self.notes.iter().zip(CALL_TUNE.iter()).all(|(&(f, s, e), &(want_f, want_len))| {
            close(f, want_f, want_f * 0.03) && close(e - s, want_len, 0.02)
        });
        // all within about a second (the tune lasts 0.96 s with its pauses)
        let span = self.notes.back().unwrap().2 - self.notes.front().unwrap().1;
        tune_ok && span < 1.3
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The call as recorded from EN Fairy, with its pauses (freq 0).
    fn call_at(t0: f64) -> Vec<(f64, f64, f64)> {
        let mut ev = Vec::new();
        let mut t = t0;
        for (f, len, gap) in [(2793.0, 0.064, 0.128), (2350.0, 0.256, 0.192), (2793.0, 0.064, 0.032), (3136.0, 0.064, 0.032), (4187.0, 0.128, 0.0)] {
            ev.push((t, f, 0.5));
            t += len;
            ev.push((t, 0.0, 0.5));
            t += gap;
        }
        ev
    }

    #[test]
    fn hears_the_call() {
        let mut d = CallDetector::default();
        assert!(d.feed(&call_at(1.0)));
        // fed in small pieces, as the window does
        let mut d = CallDetector::default();
        let ev = call_at(5.0);
        let heard = ev.chunks(1).map(|c| d.feed(c)).filter(|&h| h).count();
        assert_eq!(heard, 1);
    }

    #[test]
    fn ignores_other_sounds() {
        let mut d = CallDetector::default();
        // a falling "bling" (EN Magic, by itself) and a button beep
        let ev = [(1.0, 4187.0, 0.5), (1.064, 2093.0, 0.5), (1.128, 1047.0, 0.5), (1.16, 0.0, 0.5), (2.0, 1865.0, 0.5), (2.032, 0.0, 0.5)];
        assert!(!d.feed(&ev));
        // the call's notes but far too slow
        let slow: Vec<_> = call_at(10.0).into_iter().map(|(t, f, d)| (10.0 + (t - 10.0) * 2.0, f, d)).collect();
        assert!(!d.feed(&slow));
    }
}
