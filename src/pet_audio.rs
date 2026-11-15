//! The microphone for dictation: 16 kHz, 16-bit, mono through the classic wave-in API (no extra DLLs, no COM). One
//! recording is one worker thread that owns the device; the UI thread only starts, stops or cancels it and reads the
//! level. The result goes to the core as a WAV (`core::pet::wav`).
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::Arc;
use windows::Win32::Media::Audio::*;
use windows::Win32::Media::MMSYSERR_NOERROR;

pub const SAMPLE_RATE: u32 = 16_000;
const BLOCK_SAMPLES: usize = 1600; // 100 ms
const BLOCKS: usize = 4;

/// How a recording ended.
pub enum RecordEnd {
    /// The samples (stopped by the user or by the length limit).
    Finished(Vec<i16>),
    /// The microphone could not be opened (no device, taken by another program, or blocked in Windows' privacy settings).
    Denied,
    /// Thrown away on request.
    Canceled,
}

pub struct Recorder {
    stop: Arc<AtomicBool>,
    cancel: Arc<AtomicBool>,
    level: Arc<AtomicU32>,
}

impl Recorder {
    /// Starts recording at once. `done` runs on the worker thread, exactly once; the shell hops to its UI thread.
    pub fn start(max_seconds: u32, done: Box<dyn FnOnce(RecordEnd) + Send>) -> Recorder {
        let r = Recorder { stop: Arc::default(), cancel: Arc::default(), level: Arc::default() };
        let (stop, cancel, level) = (r.stop.clone(), r.cancel.clone(), r.level.clone());
        std::thread::spawn(move || done(record(max_seconds, &stop, &cancel, &level)));
        r
    }

    /// Ends the recording and hands the audio over.
    pub fn stop(&self) { self.stop.store(true, Ordering::SeqCst); }

    /// Ends the recording and throws the audio away.
    pub fn cancel(&self) {
        self.cancel.store(true, Ordering::SeqCst);
        self.stop.store(true, Ordering::SeqCst);
    }

    /// 0…1, the loudness of the last block (for the ring and the bars).
    pub fn level(&self) -> f64 { f32::from_bits(self.level.load(Ordering::Relaxed)) as f64 }
}

fn record(max_seconds: u32, stop: &AtomicBool, cancel: &AtomicBool, level: &AtomicU32) -> RecordEnd {
    unsafe {
        let fmt = WAVEFORMATEX { wFormatTag: WAVE_FORMAT_PCM as u16, nChannels: 1, nSamplesPerSec: SAMPLE_RATE, nAvgBytesPerSec: SAMPLE_RATE * 2, nBlockAlign: 2, wBitsPerSample: 16, cbSize: 0 };
        let mut h = HWAVEIN::default();
        if waveInOpen(Some(&mut h), WAVE_MAPPER, &fmt, None, None, CALLBACK_NULL) != MMSYSERR_NOERROR { return RecordEnd::Denied; }
        let mut bufs: Vec<Vec<i16>> = (0..BLOCKS).map(|_| vec![0i16; BLOCK_SAMPLES]).collect();
        let mut hdrs: Vec<WAVEHDR> = bufs.iter_mut().map(|b| WAVEHDR { lpData: windows::core::PSTR(b.as_mut_ptr() as *mut u8), dwBufferLength: (BLOCK_SAMPLES * 2) as u32, ..Default::default() }).collect();
        let size = std::mem::size_of::<WAVEHDR>() as u32;
        for hdr in hdrs.iter_mut() {
            waveInPrepareHeader(h, hdr, size);
            waveInAddBuffer(h, hdr, size);
        }
        let mut samples: Vec<i16> = Vec::new();
        let max = (max_seconds as usize).max(1) * SAMPLE_RATE as usize;
        let mut ok = waveInStart(h) == MMSYSERR_NOERROR;
        let mut next = 0usize;
        while ok && !stop.load(Ordering::SeqCst) && samples.len() < max {
            if hdrs[next].dwFlags & WHDR_DONE != 0 {
                let got = hdrs[next].dwBytesRecorded as usize / 2;
                let block = &bufs[next][..got.min(BLOCK_SAMPLES)];
                level.store((crate::core::pet::wav::level(block) as f32).to_bits(), Ordering::Relaxed);
                samples.extend_from_slice(block);
                hdrs[next].dwFlags &= !WHDR_DONE;
                waveInAddBuffer(h, &mut hdrs[next], size);
                next = (next + 1) % BLOCKS;
            } else {
                std::thread::sleep(std::time::Duration::from_millis(15));
            }
        }
        ok = waveInStop(h) == MMSYSERR_NOERROR && ok;
        waveInReset(h);
        // The block that was filling when it stopped holds the last words.
        if hdrs[next].dwBytesRecorded > 0 && samples.len() < max {
            let got = (hdrs[next].dwBytesRecorded as usize / 2).min(BLOCK_SAMPLES);
            samples.extend_from_slice(&bufs[next][..got]);
        }
        for hdr in hdrs.iter_mut() { waveInUnprepareHeader(h, hdr, size); }
        waveInClose(h);
        level.store(0, Ordering::Relaxed);
        samples.truncate(max);
        if cancel.load(Ordering::SeqCst) { RecordEnd::Canceled } else if !ok && samples.is_empty() { RecordEnd::Denied } else { RecordEnd::Finished(samples) }
    }
}
