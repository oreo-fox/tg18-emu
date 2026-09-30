//! Streams 16-bit mono samples to the speakers through Windows' winmm.

use crate::win32::*;

pub struct WaveOut {
    handle: isize,
    /// Buffers still playing: (header, samples). Boxed so they stay put.
    queue: Vec<(Box<WAVEHDR>, Vec<i16>)>,
}

impl WaveOut {
    pub fn open(rate: u32) -> Option<WaveOut> {
        let fmt = WAVEFORMATEX {
            wFormatTag: 1,
            nChannels: 1,
            nSamplesPerSec: rate,
            nAvgBytesPerSec: rate * 2,
            nBlockAlign: 2,
            wBitsPerSample: 16,
            cbSize: 0,
        };
        let mut h = 0isize;
        let err = unsafe { waveOutOpen(&mut h, WAVE_MAPPER, &fmt, 0, 0, 0) };
        if err != 0 {
            return None;
        }
        Some(WaveOut { handle: h, queue: Vec::new() })
    }

    /// Samples written but not played yet; finished buffers are released.
    pub fn queued(&mut self) -> usize {
        let size = std::mem::size_of::<WAVEHDR>() as u32;
        let h = self.handle;
        self.queue.retain_mut(|(hdr, _)| {
            let done = unsafe { std::ptr::read_volatile(&hdr.dwFlags) } & WHDR_DONE != 0;
            if done {
                unsafe { waveOutUnprepareHeader(h, &mut **hdr, size) };
            }
            !done
        });
        self.queue.iter().map(|(_, s)| s.len()).sum()
    }

    pub fn write(&mut self, samples: Vec<i16>) {
        if samples.is_empty() {
            return;
        }
        let mut samples = samples;
        let mut hdr = Box::new(WAVEHDR {
            lpData: samples.as_mut_ptr() as *mut u8,
            dwBufferLength: (samples.len() * 2) as u32,
            dwBytesRecorded: 0,
            dwUser: 0,
            dwFlags: 0,
            dwLoops: 0,
            lpNext: std::ptr::null_mut(),
            reserved: 0,
        });
        let size = std::mem::size_of::<WAVEHDR>() as u32;
        unsafe {
            waveOutPrepareHeader(self.handle, &mut *hdr, size);
            waveOutWrite(self.handle, &mut *hdr, size);
        }
        self.queue.push((hdr, samples));
    }
}

impl Drop for WaveOut {
    fn drop(&mut self) {
        unsafe {
            waveOutReset(self.handle);
        }
        self.queued();
        unsafe {
            waveOutClose(self.handle);
        }
    }
}
