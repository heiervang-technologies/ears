//! `ears mic-check`: how good is the microphone for transcription?
//!
//! Records a few seconds of silence, then a read sentence, and measures the
//! background level, the speech level, clipping and whether the audio is
//! narrow band (8 kHz Bluetooth headset audio upsampled to 16 kHz, which
//! looks like normal audio until you check what is above 4 kHz). A
//! transcript of the sentence is scored against what was read.

use crate::pcm;
use anyhow::{Context, Result};
use serde::Serialize;
use std::io::Read;
use std::process::Stdio;
use std::time::Duration;

/// The sentence to read. Its s, sh and f sounds put energy above 4 kHz,
/// which is what tells wide band from narrow band.
pub const SENTENCE: &str = "The quick brown fox jumps over the lazy dog, and six sheep follow.";

/// Background above this is loud enough to matter (fans, hiss, a noisy room).
pub const NOISY_DBFS: f32 = -45.0;
/// Speech below this is quiet enough to hurt recognition.
pub const QUIET_DBFS: f32 = -40.0;
/// Background at or below this is digital silence: a muted or dead source.
pub const DEAD_DBFS: f32 = -100.0;
/// Speech must stand this far above the background (dB).
pub const MIN_SNR_DB: f32 = 15.0;
/// Speech this close to the background was probably not heard at all.
pub const NO_SPEECH_SNR_DB: f32 = 6.0;
/// Transcripts worse than this word error rate get flagged.
pub const MAX_WORD_ERROR_RATE: f32 = 0.25;
/// Seconds of silence, then of speech, recorded by [`run`].
pub const SILENCE_SECONDS: f32 = 3.0;
pub const SPEECH_SECONDS: f32 = 6.0;
/// Fraction of clipped speech samples that is audible distortion.
pub const MAX_CLIPPED: f32 = 0.001;
/// Speech energy above [`HIGH_BAND_HZ`] relative to all of it, below which
/// the audio is narrow band (dB). Wide band speech sits around -15 to -30.
pub const NARROW_BAND_DB: f32 = -40.0;
/// Narrow band audio has nothing above about 4 kHz.
pub const HIGH_BAND_HZ: f32 = 4_500.0;

/// Measurements and verdict.
#[derive(Debug, Clone, Serialize)]
pub struct MicReport {
    pub device: String,
    pub noise_dbfs: f32,
    pub speech_dbfs: f32,
    pub snr_db: f32,
    pub peak_dbfs: f32,
    /// Clipped speech samples, as a fraction.
    pub clipped: f32,
    /// Speech energy above [`HIGH_BAND_HZ`] relative to all of it (dB).
    pub high_band_db: f32,
    pub narrow_band: bool,
    pub transcript: Option<String>,
    /// Word error rate of the transcript against [`SENTENCE`] (0.0 = exact).
    pub word_error_rate: Option<f32>,
    pub issues: Vec<Issue>,
}

/// One problem, with what to do about it.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Issue {
    pub code: &'static str,
    pub advice: String,
}

impl MicReport {
    pub fn verdict(&self) -> &'static str {
        match self.issues.first() {
            None => "good",
            Some(issue) => issue.code,
        }
    }
}

/// Measure a silence recording and a speech recording from `device`.
pub fn analyze(device: &str, silence: &[f32], speech: &[f32]) -> MicReport {
    let noise = pcm::noise_rms(silence);
    let level = pcm::speech_rms(speech);
    let noise_dbfs = pcm::dbfs(noise);
    let speech_dbfs = pcm::dbfs(level);
    let clipped = if speech.is_empty() {
        0.0
    } else {
        pcm::clipped(speech) as f32 / speech.len() as f32
    };
    let high_band_db = high_band_db(speech);
    let mut report = MicReport {
        device: device.to_string(),
        noise_dbfs,
        speech_dbfs,
        snr_db: speech_dbfs - noise_dbfs,
        peak_dbfs: pcm::dbfs(pcm::peak(speech)),
        clipped,
        high_band_db,
        narrow_band: false,
        transcript: None,
        word_error_rate: None,
        issues: Vec::new(),
    };
    report.issues = issues(&mut report);
    report
}

fn issues(r: &mut MicReport) -> Vec<Issue> {
    let mut out = Vec::new();
    let level_reported = r.speech_dbfs < QUIET_DBFS || r.noise_dbfs > NOISY_DBFS;
    let mut add = |code, advice: &str| {
        out.push(Issue {
            code,
            advice: advice.to_string(),
        })
    };
    if r.noise_dbfs <= DEAD_DBFS && r.speech_dbfs <= DEAD_DBFS {
        add(
            "silent",
            "The microphone delivers digital silence: it is muted, or the wrong device is selected (ears select).",
        );
        return out;
    }
    if r.snr_db < NO_SPEECH_SNR_DB {
        add(
            "no speech",
            "Speech was barely louder than the silence: check that this is the microphone you spoke into (ears select).",
        );
        return out;
    }
    // Narrow band is only judged on speech that was clearly heard.
    r.narrow_band = r.high_band_db < NARROW_BAND_DB;
    if r.narrow_band {
        add(
            "narrow band",
            "The audio stops near 4 kHz: a Bluetooth headset in its call profile, or a phone line. Use a wired or USB microphone, or a headset that supports wide band (mSBC or LC3) speech.",
        );
    }
    if r.clipped > MAX_CLIPPED {
        add(
            "clipping",
            "Speech hits full scale and distorts: lower the input volume (pavucontrol or wpctl set-volume).",
        );
    }
    if r.speech_dbfs < QUIET_DBFS {
        add(
            "too quiet",
            "Speech is quiet: move closer to the microphone or raise its input volume.",
        );
    }
    if r.noise_dbfs > NOISY_DBFS {
        add(
            "noisy",
            "The background is loud: a fan, hiss or a noisy room. Lower it, or use a closer microphone.",
        );
    }
    if r.snr_db < MIN_SNR_DB && !level_reported {
        add(
            "low signal to noise",
            "Speech is not far enough above the background: move closer to the microphone.",
        );
    }
    out
}

/// Energy above [`HIGH_BAND_HZ`] relative to all energy, over the louder
/// half of the frames (dB). Hann-windowed spectra keep leakage from the
/// strong low band below the narrow band threshold.
pub fn high_band_db(samples: &[f32]) -> f32 {
    const N: usize = pcm::LEVEL_FRAME;
    let mut frames: Vec<&[f32; N]> = samples.as_chunks::<N>().0.iter().collect();
    frames.sort_by(|a, b| pcm::rms(&b[..]).total_cmp(&pcm::rms(&a[..])));
    frames.truncate(frames.len().div_ceil(2));
    let window: Vec<f32> = (0..N)
        .map(|i| 0.5 - 0.5 * (2.0 * std::f32::consts::PI * i as f32 / N as f32).cos())
        .collect();
    let first_high = (HIGH_BAND_HZ / pcm::SAMPLE_RATE as f32 * N as f32).ceil() as usize;
    let (mut high, mut all) = (0f64, 0f64);
    for frame in frames {
        let mut re: Vec<f32> = frame.iter().zip(&window).map(|(s, w)| s * w).collect();
        let mut im = vec![0.0; N];
        fft(&mut re, &mut im);
        // Skip DC; bins above N/2 mirror the ones below.
        for k in 1..=N / 2 {
            let power = f64::from(re[k] * re[k] + im[k] * im[k]);
            all += power;
            if k >= first_high {
                high += power;
            }
        }
    }
    if all <= 0.0 {
        return 0.0;
    }
    (10.0 * (high / all).max(1e-12).log10()) as f32
}

/// In-place radix-2 FFT; the length must be a power of two.
fn fft(re: &mut [f32], im: &mut [f32]) {
    let n = re.len();
    let mut j = 0;
    for i in 1..n {
        let mut bit = n >> 1;
        while j & bit != 0 {
            j ^= bit;
            bit >>= 1;
        }
        j |= bit;
        if i < j {
            re.swap(i, j);
            im.swap(i, j);
        }
    }
    let mut len = 2;
    while len <= n {
        let angle = -2.0 * std::f32::consts::PI / len as f32;
        for start in (0..n).step_by(len) {
            for k in 0..len / 2 {
                let (sin, cos) = (angle * k as f32).sin_cos();
                let (a, b) = (start + k, start + k + len / 2);
                let tr = re[b] * cos - im[b] * sin;
                let ti = re[b] * sin + im[b] * cos;
                re[b] = re[a] - tr;
                im[b] = im[a] - ti;
                re[a] += tr;
                im[a] += ti;
            }
        }
        len <<= 1;
    }
}

/// Word error rate of `heard` against `reference`, ignoring case and
/// punctuation.
pub fn word_error_rate(reference: &str, heard: &str) -> f32 {
    let words = |s: &str| -> Vec<String> {
        s.split_whitespace()
            .map(|w| {
                w.chars()
                    .filter(|c| c.is_alphanumeric() || *c == '\'')
                    .flat_map(char::to_lowercase)
                    .collect::<String>()
            })
            .filter(|w| !w.is_empty())
            .collect()
    };
    let (r, h) = (words(reference), words(heard));
    if r.is_empty() {
        return if h.is_empty() { 0.0 } else { 1.0 };
    }
    let mut prev: Vec<usize> = (0..=h.len()).collect();
    for (i, rw) in r.iter().enumerate() {
        let mut cur = vec![i + 1; h.len() + 1];
        for (j, hw) in h.iter().enumerate() {
            let sub = prev[j] + usize::from(rw != hw);
            cur[j + 1] = sub.min(prev[j + 1] + 1).min(cur[j] + 1);
        }
        prev = cur;
    }
    prev[h.len()] as f32 / r.len() as f32
}

/// Record `seconds` of audio from `device`. Fails if the device delivers
/// nothing, rather than waiting forever.
pub fn record(device: &str, seconds: f32) -> Result<Vec<f32>> {
    let mut child = pcm::pw_record(device, "-")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .context("Failed to start pw-record")?;
    let mut stdout = child
        .stdout
        .take()
        .context("pw-record stdout unavailable")?;
    let want = (seconds * pcm::SAMPLE_RATE as f32) as usize * 2;
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let mut buf = vec![0u8; want];
        let mut got = 0;
        while got < want {
            match stdout.read(&mut buf[got..]) {
                Ok(0) | Err(_) => break,
                Ok(n) => got += n,
            }
        }
        buf.truncate(got);
        let _ = tx.send(buf);
    });
    let result = rx.recv_timeout(Duration::from_secs_f32(seconds + 3.0));
    let _ = child.kill();
    let _ = child.wait();
    let bytes = result
        .map_err(|_| anyhow::anyhow!("No audio from {device} within {:.0} s", seconds + 3.0))?;
    if bytes.len() < want / 2 {
        anyhow::bail!(
            "{device} delivered only {:.1} s of audio: is it connected?",
            bytes.len() as f32 / 2.0 / pcm::SAMPLE_RATE as f32
        );
    }
    Ok(pcm::samples_f32(&bytes))
}

/// Record, measure and (unless `transcribe` is false) transcribe from
/// `device` or the configured one. Prompts go to stderr so `json` output on
/// stdout stays clean.
pub async fn run(
    config: &crate::Config,
    device: Option<String>,
    transcribe: bool,
    json: bool,
) -> Result<MicReport> {
    let device = device.unwrap_or_else(|| config.device.clone());
    eprintln!("Checking microphone {device}");
    eprintln!("Stay quiet for {SILENCE_SECONDS:.0} seconds…");
    // Let the key press that started the check die away.
    tokio::time::sleep(Duration::from_millis(700)).await;
    let d = device.clone();
    let silence = tokio::task::spawn_blocking(move || record(&d, SILENCE_SECONDS)).await??;
    eprintln!("Now read aloud, at your normal distance and volume:\n\n  {SENTENCE}\n");
    let d = device.clone();
    let speech = tokio::task::spawn_blocking(move || record(&d, SPEECH_SECONDS)).await??;
    let mut report = analyze(&device, &silence, &speech);

    if transcribe
        && !report
            .issues
            .iter()
            .any(|i| i.code == "silent" || i.code == "no speech")
    {
        eprintln!("Transcribing…");
        match transcribe_speech(config, &speech).await {
            Ok(text) => {
                let wer = word_error_rate(SENTENCE, &text);
                if wer > MAX_WORD_ERROR_RATE {
                    report.issues.push(Issue {
                        code: "misheard",
                        advice: format!(
                            "{:.0}% of the words came out wrong. If the levels above look fine, check the server and language (ears test).",
                            wer * 100.0
                        ),
                    });
                }
                report.transcript = Some(text);
                report.word_error_rate = Some(wer);
            }
            Err(e) => eprintln!("Transcription failed: {e:#}"),
        }
    }

    if json {
        println!("{}", serde_json::to_string_pretty(&report)?);
    } else {
        print!("{}", render(&report));
    }
    Ok(report)
}

async fn transcribe_speech(config: &crate::Config, speech: &[f32]) -> Result<String> {
    let language = crate::KeyboardLayout::detect_language().or_else(|| config.language.clone());
    let (server_url, model) = config.resolve_server(language.as_deref());
    let client = crate::WhisperClient::new(server_url.to_string())
        .with_language(language)
        .with_api_key(config.api_key.clone())
        .with_model(model)
        .with_silence_filter(config.filter_silence_artifacts)
        .with_prompt(config.prompt.clone());
    let file = tempfile::Builder::new()
        .prefix("ears-mic-check-")
        .suffix(".wav")
        .tempfile()?;
    pcm::write_wav(file.path(), speech)?;
    Ok(client.transcribe(file.path()).await?)
}

/// The human-readable report.
pub fn render(r: &MicReport) -> String {
    let mut out = format!("{:<14}{}\n", "Device", r.device);
    let mut line = |label: &str, value: String, note: &str| {
        out.push_str(&format!("{label:<14}{value:<12}{note}\n"));
    };
    line(
        "Background",
        format!("{:.0} dBFS", r.noise_dbfs),
        &format!("(under {NOISY_DBFS:.0} is quiet)"),
    );
    line(
        "Speech",
        format!("{:.0} dBFS", r.speech_dbfs),
        &format!("(over {QUIET_DBFS:.0} is loud enough)"),
    );
    line(
        "Signal/noise",
        format!("{:.0} dB", r.snr_db),
        &format!("(over {MIN_SNR_DB:.0} is good)"),
    );
    line(
        "Peak",
        format!("{:.1} dBFS", r.peak_dbfs),
        &format!("{:.2}% clipped", r.clipped * 100.0),
    );
    line(
        "Above 4.5k",
        format!("{:.0} dB", r.high_band_db),
        if r.narrow_band {
            "narrow band"
        } else {
            "wide band"
        },
    );
    if let (Some(text), Some(wer)) = (&r.transcript, r.word_error_rate) {
        line(
            "Transcript",
            format!("{:.0}% wrong", wer * 100.0),
            &format!("\"{text}\""),
        );
    }
    out.push('\n');
    if r.issues.is_empty() {
        out.push_str("Verdict: good\n");
    }
    for issue in &r.issues {
        out.push_str(&format!("Verdict: {}. {}\n", issue.code, issue.advice));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tone(hz: f32, amp: f32, seconds: f32) -> Vec<f32> {
        (0..(seconds * 16_000.0) as usize)
            .map(|i| amp * (2.0 * std::f32::consts::PI * hz * i as f32 / 16_000.0).sin())
            .collect()
    }

    /// Deterministic white-ish noise.
    fn noise(amp: f32, seconds: f32) -> Vec<f32> {
        let mut x = 0x1234_5678u32;
        (0..(seconds * 16_000.0) as usize)
            .map(|_| {
                x ^= x << 13;
                x ^= x >> 17;
                x ^= x << 5;
                amp * (x as f32 / u32::MAX as f32 * 2.0 - 1.0)
            })
            .collect()
    }

    fn mix(a: &[f32], b: &[f32]) -> Vec<f32> {
        a.iter().zip(b).map(|(x, y)| x + y).collect()
    }

    #[test]
    fn wide_band_speech_in_a_quiet_room_is_good() {
        let speech = mix(&tone(300.0, 0.1, 2.0), &noise(0.05, 2.0));
        let r = analyze("mic", &noise(0.0005, 1.0), &speech);
        assert_eq!(r.issues, vec![], "{r:?}");
        assert_eq!(r.verdict(), "good");
        assert!(r.snr_db > 30.0);
    }

    #[test]
    fn nothing_above_4k_is_narrow_band() {
        let speech = mix(&tone(300.0, 0.1, 2.0), &tone(3_000.0, 0.05, 2.0));
        let r = analyze("bluez_input.headset", &noise(0.0005, 1.0), &speech);
        assert!(r.narrow_band, "{r:?}");
        assert_eq!(r.verdict(), "narrow band");
    }

    #[test]
    fn zeros_are_a_dead_microphone() {
        let r = analyze("mic", &[0.0; 16_000], &[0.0; 16_000]);
        assert_eq!(r.verdict(), "silent");
        assert_eq!(r.issues.len(), 1);
    }

    #[test]
    fn speech_no_louder_than_silence_was_not_heard() {
        let r = analyze("mic", &noise(0.01, 1.0), &noise(0.01, 1.0));
        assert_eq!(r.verdict(), "no speech");
        assert!(!r.narrow_band, "not judged without speech");
    }

    #[test]
    fn quiet_noisy_and_clipped_are_each_reported() {
        let quiet = analyze("mic", &noise(0.0001, 1.0), &noise(0.004, 1.0));
        assert_eq!(quiet.verdict(), "too quiet");

        let noisy = analyze("mic", &noise(0.02, 1.0), &noise(0.5, 1.0));
        assert!(noisy.issues.iter().any(|i| i.code == "noisy"), "{noisy:?}");

        let clipped = analyze("mic", &noise(0.0005, 1.0), &noise(1.2, 1.0));
        assert!(clipped.issues.iter().any(|i| i.code == "clipping"));
    }

    #[test]
    fn fft_finds_a_tone_in_its_bin() {
        let mut re = tone(1_000.0, 1.0, 512.0 / 16_000.0);
        let mut im = vec![0.0; 512];
        fft(&mut re, &mut im);
        let power: Vec<f32> = (0..=256).map(|k| re[k].hypot(im[k])).collect();
        let max = (0..=256)
            .max_by(|&a, &b| power[a].total_cmp(&power[b]))
            .unwrap();
        assert_eq!(max, 32, "1 kHz is bin 32 of 512 at 16 kHz");
    }

    #[test]
    fn high_band_separates_wide_from_narrow() {
        assert!(
            high_band_db(&noise(0.1, 1.0)) > -5.0,
            "white noise is mostly high band"
        );
        assert!(high_band_db(&tone(3_800.0, 0.5, 1.0)) < NARROW_BAND_DB);
        assert_eq!(high_band_db(&[]), 0.0);
    }

    #[test]
    fn render_names_every_issue() {
        let r = analyze("mic", &noise(0.02, 1.0), &noise(1.2, 1.0));
        let text = render(&r);
        assert!(text.contains("Verdict: clipping."), "{text}");
        assert!(text.contains("Verdict: noisy."), "{text}");
        let good = analyze("mic", &noise(0.0005, 1.0), &noise(0.1, 1.0));
        assert!(render(&good).ends_with("Verdict: good\n"));
    }

    #[test]
    fn word_error_rate_ignores_case_and_punctuation() {
        assert_eq!(word_error_rate(SENTENCE, &SENTENCE.to_uppercase()), 0.0);
        assert_eq!(word_error_rate("a b c d", "a x c d"), 0.25);
        assert_eq!(word_error_rate("a b c d", "a b c d e"), 0.25);
        assert_eq!(word_error_rate("a b", ""), 1.0);
        assert_eq!(word_error_rate("", ""), 0.0);
    }
}
