//! Print the speech segments ears would cut from WAV files (16-bit mono,
//! 16 kHz) under the old fixed threshold, with hysteresis, and with
//! hysteresis plus speech-gated gain. See docs/VOICE_ISOLATION.md.
use ears::vad::{VadConfig, VadSegmentDetector};
fn read(path: &str) -> Vec<f32> {
    let b = std::fs::read(path).unwrap();
    let mut i = 12;
    while &b[i..i + 4] != b"data" {
        let n = u32::from_le_bytes(b[i + 4..i + 8].try_into().unwrap()) as usize;
        i += 8 + n;
    }
    b[i + 8..]
        .as_chunks::<2>()
        .0
        .iter()
        .map(|c| i16::from_le_bytes(*c) as f32 / 32768.0)
        .collect()
}
fn main() {
    for path in std::env::args().skip(1) {
        let audio = read(&path);
        for (name, end, gain) in [
            ("old", Some(0.5), false),
            ("hyst", None, false),
            ("hyst+agc", None, true),
        ] {
            let mut det = VadSegmentDetector::new(VadConfig {
                min_speech_duration_ms: std::env::var("MIN_SPEECH_MS")
                    .ok()
                    .and_then(|v| v.parse().ok())
                    .unwrap_or(300),
                max_silence_duration_ms: 700,
                end_threshold: end,
                auto_gain: gain,
                ..VadConfig::default()
            })
            .unwrap();
            let mut segs = vec![];
            for chunk in audio.chunks(1600) {
                if let Some(s) = det.process(chunk).unwrap() {
                    segs.push(s);
                }
            }
            if let Some(s) = det.process(&vec![0.0; 16000]).unwrap() {
                segs.push(s);
            }
            let d: Vec<String> = segs
                .iter()
                .map(|s| format!("{}-{}ms p={:.2}", s.start_ms, s.end_ms, s.mean_probability))
                .collect();
            println!(
                "{:<28} {:<9} {} segs: {}",
                path.rsplit('/').next().unwrap(),
                name,
                segs.len(),
                d.join(", ")
            );
        }
    }
}
