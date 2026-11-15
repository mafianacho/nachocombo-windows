//! A recording as a WAV file for `POST /api/v1/dictation` (the server accepts `audio/wav`): 16-bit PCM, mono.
//! The Windows shell records with the microphone API and hands the samples here.

pub const MIME: &str = "audio/wav";
pub const FILE_NAME: &str = "dictation.wav";

/// A 44-byte RIFF header and the samples, little endian.
pub fn encode_pcm16_mono(samples: &[i16], sample_rate: u32) -> Vec<u8> {
    let data_len = (samples.len() * 2) as u32;
    let mut out = Vec::with_capacity(44 + data_len as usize);
    out.extend_from_slice(b"RIFF");
    out.extend_from_slice(&(36 + data_len).to_le_bytes());
    out.extend_from_slice(b"WAVEfmt ");
    out.extend_from_slice(&16u32.to_le_bytes());
    out.extend_from_slice(&1u16.to_le_bytes()); // PCM
    out.extend_from_slice(&1u16.to_le_bytes()); // mono
    out.extend_from_slice(&sample_rate.to_le_bytes());
    out.extend_from_slice(&(sample_rate * 2).to_le_bytes()); // bytes per second
    out.extend_from_slice(&2u16.to_le_bytes()); // block align
    out.extend_from_slice(&16u16.to_le_bytes()); // bits per sample
    out.extend_from_slice(b"data");
    out.extend_from_slice(&data_len.to_le_bytes());
    for s in samples { out.extend_from_slice(&s.to_le_bytes()); }
    out
}

/// Seconds of audio.
pub fn seconds(samples: usize, sample_rate: u32) -> f64 { if sample_rate == 0 { 0.0 } else { samples as f64 / sample_rate as f64 } }

/// Loudness of the last block as 0…1, for the level shown while recording (RMS against full scale, eased so quiet
/// speech still moves it).
pub fn level(block: &[i16]) -> f64 {
    if block.is_empty() { return 0.0; }
    let sum: f64 = block.iter().map(|s| { let v = *s as f64 / 32768.0; v * v }).sum();
    (sum / block.len() as f64).sqrt().sqrt().min(1.0)
}

/// Whether anything was said: a recording that never rises above this is treated as silence (nothing is sent).
pub fn has_sound(samples: &[i16]) -> bool {
    samples.iter().any(|s| s.unsigned_abs() > 600)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn header_and_data_are_a_valid_wav() {
        let w = encode_pcm16_mono(&[0, 1, -1, 32767], 16000);
        assert_eq!(&w[0..4], b"RIFF");
        assert_eq!(u32::from_le_bytes(w[4..8].try_into().unwrap()), 36 + 8);
        assert_eq!(&w[8..16], b"WAVEfmt ");
        assert_eq!(u16::from_le_bytes(w[20..22].try_into().unwrap()), 1, "PCM");
        assert_eq!(u32::from_le_bytes(w[24..28].try_into().unwrap()), 16000);
        assert_eq!(u32::from_le_bytes(w[28..32].try_into().unwrap()), 32000);
        assert_eq!(&w[36..40], b"data");
        assert_eq!(u32::from_le_bytes(w[40..44].try_into().unwrap()), 8);
        assert_eq!(i16::from_le_bytes(w[48..50].try_into().unwrap()), -1);
        assert_eq!(w.len(), 44 + 8);
    }

    #[test]
    fn seconds_level_and_silence() {
        assert_eq!(seconds(32000, 16000), 2.0);
        assert_eq!(seconds(10, 0), 0.0);
        assert_eq!(level(&[]), 0.0);
        assert!(level(&[0; 160]) == 0.0 && level(&[20000; 160]) > 0.7 && level(&[i16::MAX; 8]) <= 1.0);
        assert!(!has_sound(&[0, 100, -300, 500]));
        assert!(has_sound(&[0, 100, -3000]));
    }
}
