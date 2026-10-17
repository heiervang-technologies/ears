//! Replay a recording through the VAD with your config, to see why an
//! utterance was or was not picked up.
//!
//! ```bash
//! cargo run --release --example vad_replay -- recording.wav
//! ```
//!
//! The WAV must be 16 kHz mono 16-bit (what `pw-record --rate 16000
//! --channels 1 --format s16` writes). Prints one line per second of audio:
//! a bar of per-frame speech probability and the gain at its end (` ` < 0.1 < `.` < 0.25 < `:` <
//! threshold <= `#`), then each segment the VAD would hand to the
//! transcriber, and whether `min_mean_probability` would drop it.
use ears::{VadConfig, VadSegmentDetector};

fn main() -> anyhow::Result<()> {
    let path = std::env::args()
        .nth(1)
        .ok_or_else(|| anyhow::anyhow!("usage: vad_replay FILE.wav"))?;
    let bytes = std::fs::read(&path)?;
    // pw-record fills in the sizes only on a clean exit: find the data chunk.
    let data = bytes
        .windows(4)
        .position(|w| w == b"data")
        .map(|at| &bytes[at + 8..])
        .unwrap_or(&bytes[..]);
    let samples: Vec<f32> = data
        .as_chunks::<2>()
        .0
        .iter()
        .map(|b| i16::from_le_bytes(*b) as f32 / 32768.0)
        .collect();

    let settings = ears::Config::load()?.vad;
    let config = VadConfig::from_settings(&settings);
    let threshold = config.speech_threshold;
    let min_mean = config.min_mean_probability;
    println!(
        "threshold {threshold}, end {:.2}, min speech {} ms, min mean {min_mean}, auto gain {}",
        config.end_threshold(),
        config.min_speech_duration_ms,
        config.auto_gain
    );
    let mut vad = VadSegmentDetector::new(config)?;

    let frame = vad.frame_size();
    let frames_per_line = 16_000 / frame;
    let mut line = String::new();
    for (i, chunk) in samples.chunks(frame).enumerate() {
        if chunk.len() < frame {
            break;
        }
        let segment = vad.process(chunk)?;
        let p = vad.last_probability();
        line.push(match p {
            p if p >= threshold => '#',
            p if p >= 0.25 => ':',
            p if p >= 0.1 => '.',
            _ => ' ',
        });
        if (i + 1) % frames_per_line == 0 {
            println!(
                "{:>3}s |{line}| gain {:.2}",
                (i + 1) / frames_per_line - 1,
                vad.gain()
            );
            line.clear();
        }
        if let Some(s) = segment {
            let verdict = if s.samples.is_empty() || s.mean_probability >= min_mean {
                "transcribed"
            } else {
                "DROPPED"
            };
            println!(
                "      segment {:.2}-{:.2}s, mean probability {:.3}: {verdict}",
                s.start_ms as f32 / 1000.0,
                s.end_ms as f32 / 1000.0,
                s.mean_probability
            );
        }
    }
    Ok(())
}
