//! The audio format ears captures and sends: mono 16 kHz signed 16-bit PCM.
//!
//! Levels, WAV files and the `pw-record` command line live here so every
//! pipeline measures and writes audio the same way.

use std::path::Path;
use std::process::Command;

/// Capture and transcription sample rate.
pub const SAMPLE_RATE: u32 = 16_000;

/// Size of the canonical PCM WAV header written by [`wav_header`].
pub const WAV_HEADER_LEN: usize = 44;

/// Samples at or above this magnitude count as clipped.
pub const CLIP_LEVEL: f32 = 0.99;

/// `pw-record` capturing `device` as mono 16 kHz s16 into `output` (`-`:
/// stdout). The caller adds stdio settings or wraps it.
pub fn pw_record(device: &str, output: impl AsRef<std::ffi::OsStr>) -> Command {
    let mut cmd = Command::new("pw-record");
    cmd.args(pw_record_args(device)).arg(output);
    cmd
}

/// The `pw-record` arguments before the output path.
pub fn pw_record_args(device: &str) -> [String; 8] {
    [
        "--target".into(),
        device.into(),
        "--rate".into(),
        SAMPLE_RATE.to_string(),
        "--channels".into(),
        "1".into(),
        "--format".into(),
        "s16".into(),
    ]
}

/// Header of a mono 16-bit PCM WAV holding `data_len` bytes of samples.
pub fn wav_header(data_len: u32, sample_rate: u32) -> [u8; WAV_HEADER_LEN] {
    let mut h = [0u8; WAV_HEADER_LEN];
    h[0..4].copy_from_slice(b"RIFF");
    h[4..8].copy_from_slice(&(36 + data_len).to_le_bytes());
    h[8..16].copy_from_slice(b"WAVEfmt ");
    h[16..20].copy_from_slice(&16u32.to_le_bytes());
    h[20..22].copy_from_slice(&1u16.to_le_bytes()); // PCM
    h[22..24].copy_from_slice(&1u16.to_le_bytes()); // mono
    h[24..28].copy_from_slice(&sample_rate.to_le_bytes());
    h[28..32].copy_from_slice(&(sample_rate * 2).to_le_bytes());
    h[32..34].copy_from_slice(&2u16.to_le_bytes());
    h[34..36].copy_from_slice(&16u16.to_le_bytes());
    h[36..40].copy_from_slice(b"data");
    h[40..44].copy_from_slice(&data_len.to_le_bytes());
    h
}

/// Little-endian PCM16 bytes wrapped as a WAV in memory.
pub fn wav_from_bytes(pcm: &[u8], sample_rate: u32) -> Vec<u8> {
    let mut out = Vec::with_capacity(WAV_HEADER_LEN + pcm.len());
    out.extend_from_slice(&wav_header(pcm.len() as u32, sample_rate));
    out.extend_from_slice(pcm);
    out
}

/// 16 kHz PCM16 samples as a WAV in memory.
pub fn wav_bytes(pcm: &[i16]) -> Vec<u8> {
    let bytes: Vec<u8> = pcm.iter().flat_map(|s| s.to_le_bytes()).collect();
    wav_from_bytes(&bytes, SAMPLE_RATE)
}

/// Write 16 kHz float samples (-1.0..1.0, clamped) as a PCM16 WAV file.
pub fn write_wav(path: &Path, samples: &[f32]) -> std::io::Result<()> {
    let pcm: Vec<i16> = samples.iter().map(|&s| to_i16(s)).collect();
    std::fs::write(path, wav_bytes(&pcm))
}

/// Float sample to PCM16, clamped.
pub fn to_i16(sample: f32) -> i16 {
    (sample.clamp(-1.0, 1.0) * 32767.0) as i16
}

/// PCM16 sample to float.
pub fn to_f32(sample: i16) -> f32 {
    sample as f32 / 32768.0
}

/// PCM16 samples from little-endian bytes (a trailing odd byte is dropped).
pub fn samples_i16(bytes: &[u8]) -> Vec<i16> {
    let (pairs, _) = bytes.as_chunks::<2>();
    pairs.iter().map(|&b| i16::from_le_bytes(b)).collect()
}

/// Float samples from little-endian PCM16 bytes.
pub fn samples_f32(bytes: &[u8]) -> Vec<f32> {
    let (pairs, _) = bytes.as_chunks::<2>();
    pairs
        .iter()
        .map(|&b| to_f32(i16::from_le_bytes(b)))
        .collect()
}

/// RMS of float samples (0.0 for none).
pub fn rms(samples: &[f32]) -> f32 {
    if samples.is_empty() {
        return 0.0;
    }
    (samples.iter().map(|&s| f64::from(s).powi(2)).sum::<f64>() / samples.len() as f64).sqrt()
        as f32
}

/// RMS of PCM16 samples, on the same -1.0..1.0 scale as [`rms`].
pub fn rms_i16(samples: &[i16]) -> f32 {
    if samples.is_empty() {
        return 0.0;
    }
    let energy: f64 = samples.iter().map(|&s| f64::from(s).powi(2)).sum();
    ((energy / samples.len() as f64).sqrt() / 32768.0) as f32
}

/// Frame length for level statistics: 32 ms, one Silero frame.
pub const LEVEL_FRAME: usize = 512;

/// RMS of each [`LEVEL_FRAME`], loudest first.
pub fn frame_levels(samples: &[f32]) -> Vec<f32> {
    let mut levels: Vec<f32> = samples.chunks(LEVEL_FRAME).map(rms).collect();
    levels.sort_by(|a, b| b.total_cmp(a));
    levels
}

/// Speech level: the power mean of the louder half of the frames, so pauses
/// between words do not drag it down.
pub fn speech_rms(samples: &[f32]) -> f32 {
    let levels = frame_levels(samples);
    power_mean(&levels[..levels.len().div_ceil(2)])
}

/// Background level: the power mean of the quieter half of the frames, so a
/// click or a breath does not lift it.
pub fn noise_rms(samples: &[f32]) -> f32 {
    let levels = frame_levels(samples);
    power_mean(&levels[levels.len() / 2..])
}

fn power_mean(levels: &[f32]) -> f32 {
    if levels.is_empty() {
        return 0.0;
    }
    (levels.iter().map(|l| l * l).sum::<f32>() / levels.len() as f32).sqrt()
}

/// Largest magnitude.
pub fn peak(samples: &[f32]) -> f32 {
    samples.iter().fold(0.0f32, |p, s| p.max(s.abs()))
}

/// Samples at or above [`CLIP_LEVEL`].
pub fn clipped(samples: &[f32]) -> usize {
    samples.iter().filter(|s| s.abs() >= CLIP_LEVEL).count()
}

/// Linear level to dBFS, floored at -120.
pub fn dbfs(level: f32) -> f32 {
    20.0 * level.max(1e-6).log10()
}

/// dBFS to linear level.
pub fn from_dbfs(db: f32) -> f32 {
    10f32.powf(db / 20.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wav_bytes_is_a_valid_header() {
        let wav = wav_bytes(&[1, 2, 3]);
        assert_eq!(&wav[0..4], b"RIFF");
        assert_eq!(u32::from_le_bytes(wav[4..8].try_into().unwrap()), 42);
        assert_eq!(u32::from_le_bytes(wav[24..28].try_into().unwrap()), 16_000);
        assert_eq!(u32::from_le_bytes(wav[40..44].try_into().unwrap()), 6);
        assert_eq!(samples_i16(&wav[WAV_HEADER_LEN..]), vec![1, 2, 3]);
    }

    #[test]
    fn levels_share_one_scale() {
        let f = [0.5f32, -0.5, 0.5, -0.5];
        let i: Vec<i16> = f.iter().map(|&s| to_i16(s)).collect();
        assert!((rms(&f) - 0.5).abs() < 1e-6);
        assert!((rms_i16(&i) - 0.5).abs() < 1e-3);
        assert!((dbfs(0.1) + 20.0).abs() < 1e-4);
        assert!((from_dbfs(-20.0) - 0.1).abs() < 1e-6);
        assert_eq!(dbfs(0.0), -120.0);
        assert_eq!(rms(&[]), 0.0);
    }

    #[test]
    fn speech_and_noise_ignore_the_other_half() {
        let mut s = vec![0.1f32; LEVEL_FRAME * 2];
        s.extend(vec![0.001f32; LEVEL_FRAME * 2]);
        assert!((speech_rms(&s) - 0.1).abs() < 1e-5);
        assert!((noise_rms(&s) - 0.001).abs() < 1e-6);
        assert_eq!(speech_rms(&[]), 0.0);
    }

    #[test]
    fn clipping_counts_full_scale_samples() {
        assert_eq!(clipped(&[0.2, 1.0, -0.995, 0.98]), 2);
        assert_eq!(peak(&[0.2, -0.7]), 0.7);
    }

    #[test]
    fn pw_record_asks_for_mono_16k_s16() {
        let cmd = pw_record("mic", "-");
        let args: Vec<_> = cmd
            .get_args()
            .map(|a| a.to_string_lossy().into_owned())
            .collect();
        assert_eq!(
            args,
            [
                "--target",
                "mic",
                "--rate",
                "16000",
                "--channels",
                "1",
                "--format",
                "s16",
                "-"
            ]
        );
    }
}
