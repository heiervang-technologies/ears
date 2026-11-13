//! `ears cs build`: probe the configured ASR with synthesized speech of every
//! canonical command phrase and write what it hears as compensating
//! scaffolding (`cs/<model>/commands.toml`, see
//! `docs/rfc/0002-compensating-scaffolding.md`).
//!
//! For each canonical phrase the probe synthesizes speech with several
//! voices at a few speaking rates (one TTS request per phrase, voice and
//! rate), mixes each clip with synthetic noise at a few signal-to-noise
//! ratios, sends every mix to the ASR over the same HTTP transcription path
//! dictation uses ([`WhisperClient`]), and normalizes the result with
//! [`commands::normalize`]. Per phrase it then counts the distinct hearings.
//! A hearing that differs from the canonical phrase and makes up at least
//! `--min-fraction` of the phrase's trials becomes an entry.
//!
//! Audio:
//! - TTS: an OpenAI-shaped `/v1/audio/speech` endpoint (`--tts-url`, e.g. a
//!   vLLM-omni Qwen3-TTS deployment; asked for `response_format = "wav"` and
//!   `speed`), or `espeak-ng` (rate = 175 words per minute times the speed).
//!   Never `say`: that plays on the speakers.
//! - Every clip is resampled to 16 kHz mono (windowed-sinc), set to a speech
//!   level of -20 dBFS and padded with 250 ms of silence before and 350 ms
//!   after, roughly what a VAD segment looks like.
//! - Noise is synthetic: white (uniform) or pink (Paul Kellet's filter over
//!   white), seeded from the clip, scaled to the requested SNR against the
//!   speech level ([`pcm::speech_rms`]) and laid over the whole clip.
//!
//! Load: the ASR also serves live voice typing, so at most two requests are
//! in flight, the run refuses to start when it would need more than
//! `--max-requests` ASR (or TTS) requests, and `--dry-run` prints the plan.
//! Synthesized clips and transcripts are cached (`--cache-dir`), so a re-run
//! with the same setup costs no requests at all.

use crate::command_mode::{parse_actions, Action, Effect};
use crate::commands::{normalize, VoiceCommands};
use crate::cs::risk::{self, Lexicon, Verdict};
use crate::cs::{assess_entry, migrate, model_key, Entry, FailureMode, Meta, Source, Status};
use crate::pcm;
use crate::whisper::{WhisperClient, WhisperError};
use anyhow::{bail, Context, Result};
use futures_util::{stream, StreamExt};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Write as _;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

/// The actions file shipped as the default for command mode.
const DEFAULT_ACTIONS: &str = include_str!("../../contrib/actions.toml");

/// Most requests in flight at once, whatever `--concurrency` says: the ASR
/// also serves live voice typing.
pub const MAX_CONCURRENCY: usize = 2;

/// Speech level clips are set to before noise is added.
const SPEECH_DBFS: f32 = -20.0;
/// Silence before and after the speech, in samples at 16 kHz.
const PAD_BEFORE: usize = 4_000;
const PAD_AFTER: usize = 5_600;
/// espeak-ng's default rate, words per minute.
const ESPEAK_WPM: f32 = 175.0;

// ---------------------------------------------------------------------------
// Phrases
// ---------------------------------------------------------------------------

/// A phrase some target is matched by, for collision checks.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Known {
    pub target: String,
    /// Normalized.
    pub text: String,
}

/// A canonical phrase to probe.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Canonical {
    pub target: String,
    /// As configured.
    pub meant: String,
}

/// Everything the matcher knows: the canonical phrase of each target and
/// every phrase of every target.
#[derive(Debug, Clone, Default)]
pub struct Phrases {
    pub canonical: Vec<Canonical>,
    pub known: Vec<Known>,
}

/// The `[commands]` phrase lists, by target name.
pub fn command_lists(commands: &VoiceCommands) -> Vec<(&'static str, &[String])> {
    vec![
        ("wake", &commands.wake),
        ("sleep", &commands.sleep),
        ("standby", &commands.standby),
        ("enter", &commands.enter),
        ("sign_off", &commands.sign_off),
        ("new_line", &commands.new_line),
        ("undo", &commands.undo),
        ("back", &commands.back),
        ("backtrack", &commands.backtrack),
        ("literal", &commands.literal),
    ]
}

/// The phrase of `list` to probe, by `ears cs migrate`'s split: the first
/// built-in phrase (`seeds`), else the first phrase the split keeps
/// canonical. Variants it recognizes (hand-added mishearings) are never
/// probed as phrases of their own.
fn canonical_of(target: &str, seeds: &[String], list: &[String]) -> Option<String> {
    if let Some(seed) = seeds.iter().find(|s| !normalize(s).is_empty()) {
        return Some(seed.clone());
    }
    migrate::split_list(target, &[], list)
        .canonical
        .into_iter()
        .next()
}

/// The built-in phrases of a `[commands]` list.
fn seeds(target: &str) -> Vec<String> {
    command_lists(&VoiceCommands::default())
        .into_iter()
        .find(|(name, _)| *name == target)
        .map(|(_, list)| list.to_vec())
        .unwrap_or_default()
}

/// Canonical and known phrases from the commands config and command-mode
/// actions. The built-in chatter action `none` is not a command.
pub fn collect_phrases(commands: &VoiceCommands, actions: &[Action]) -> Phrases {
    let mut phrases = Phrases::default();
    let mut add = |target: String, list: &[String]| {
        if let Some(meant) = canonical_of(&target, &seeds(&target), list) {
            phrases.canonical.push(Canonical {
                target: target.clone(),
                meant,
            });
        }
        for p in list {
            let text = normalize(p);
            if !text.is_empty() {
                phrases.known.push(Known {
                    target: target.clone(),
                    text,
                });
            }
        }
    };
    for (name, list) in command_lists(commands) {
        add(name.to_string(), list);
    }
    for run in commands.run.resolved() {
        add(format!("run:{}", run.name), &run.phrases);
    }
    for action in actions {
        if action.effect == Effect::Nothing {
            continue;
        }
        add(format!("action:{}", action.name), &action.text);
    }
    // Key phrases are not probed (no spec target yet), but a hearing that
    // equals one would press its key.
    for phrase in commands.keys.keys() {
        let text = normalize(phrase);
        if !text.is_empty() {
            phrases.known.push(Known {
                target: format!("key:{text}"),
                text,
            });
        }
    }
    phrases
}

/// The canonical phrases `selectors` asks for, in their order. A selector is
/// a target (`wake`, `run:centurion`, `action:close`), `lists` (every
/// `[commands]` list), `run:*` or `action:*`.
///
/// With `skip_missing`, a target the phrases do not have is skipped and
/// returned in the second list instead of failing.
pub fn select<'a>(
    phrases: &'a Phrases,
    selectors: &[String],
    skip_missing: bool,
) -> Result<(Vec<&'a Canonical>, Vec<String>)> {
    let mut missing = Vec::new();
    let lists: BTreeSet<&str> = command_lists(&VoiceCommands::default())
        .into_iter()
        .map(|(n, _)| n)
        .collect();
    let mut out: Vec<&Canonical> = Vec::new();
    for selector in selectors {
        let selector = selector.trim();
        if selector.is_empty() {
            continue;
        }
        let matched: Vec<&Canonical> = phrases
            .canonical
            .iter()
            .filter(|c| match selector {
                "lists" => lists.contains(c.target.as_str()),
                "run:*" => c.target.starts_with("run:"),
                "action:*" => c.target.starts_with("action:"),
                exact => c.target == exact,
            })
            .collect();
        if matched.is_empty() {
            if skip_missing {
                missing.push(selector.to_string());
                continue;
            }
            bail!("no canonical phrase for target {selector:?}");
        }
        for c in matched {
            if !out.iter().any(|o| o.target == c.target) {
                out.push(c);
            }
        }
    }
    Ok((out, missing))
}

// ---------------------------------------------------------------------------
// Conditions and plan
// ---------------------------------------------------------------------------

/// Background noise mixed into a clip.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Noise {
    Clean,
    /// SNR in dB.
    White(f32),
    /// SNR in dB.
    Pink(f32),
}

impl Noise {
    /// `clean`, `white:SNR` or `pink:SNR` (SNR in dB).
    pub fn parse(s: &str) -> Result<Noise> {
        let s = s.trim();
        if s == "clean" {
            return Ok(Noise::Clean);
        }
        let (kind, snr) = s
            .split_once(':')
            .with_context(|| format!("noise {s:?}: expected clean, white:SNR or pink:SNR"))?;
        let snr: f32 = snr
            .parse()
            .with_context(|| format!("noise {s:?}: SNR is not a number"))?;
        match kind {
            "white" => Ok(Noise::White(snr)),
            "pink" => Ok(Noise::Pink(snr)),
            _ => bail!("noise {s:?}: kind must be white or pink"),
        }
    }

    pub fn label(&self) -> String {
        match self {
            Noise::Clean => "clean".into(),
            Noise::White(snr) => format!("white:{snr}"),
            Noise::Pink(snr) => format!("pink:{snr}"),
        }
    }
}

/// One synthesis: a phrase in one voice at one rate.
#[derive(Debug, Clone, PartialEq)]
pub struct Synthesis {
    pub phrase: usize,
    pub voice: String,
    pub speed: f32,
}

/// What a run will do.
#[derive(Debug, Clone)]
pub struct Plan {
    pub phrases: Vec<Canonical>,
    /// Only for phrases whose text no earlier phrase has: the same words
    /// for two targets ("enter" for `enter` and `action:enter`) are spoken
    /// and heard once.
    pub syntheses: Vec<Synthesis>,
    pub noises: Vec<Noise>,
    /// (phrase, earlier phrase with the same words): it gets that one's
    /// hearings.
    pub aliases: Vec<(usize, usize)>,
}

impl Plan {
    pub fn new(
        phrases: Vec<Canonical>,
        voices: &[String],
        speeds: &[f32],
        noises: &[Noise],
    ) -> Plan {
        let mut syntheses = Vec::new();
        let mut aliases = Vec::new();
        for phrase in 0..phrases.len() {
            let text = normalize(&phrases[phrase].meant);
            if let Some(first) = phrases[..phrase]
                .iter()
                .position(|p| normalize(&p.meant) == text)
            {
                aliases.push((phrase, first));
                continue;
            }
            for voice in voices {
                for &speed in speeds {
                    syntheses.push(Synthesis {
                        phrase,
                        voice: voice.clone(),
                        speed,
                    });
                }
            }
        }
        Plan {
            phrases,
            syntheses,
            noises: noises.to_vec(),
            aliases,
        }
    }

    /// Trials that send audio to the ASR (aliases not counted).
    pub fn trials(&self) -> usize {
        self.syntheses.len() * self.noises.len()
    }
}

// ---------------------------------------------------------------------------
// TTS
// ---------------------------------------------------------------------------

/// Where speech comes from.
#[derive(Debug, Clone)]
pub enum Tts {
    /// OpenAI-shaped `POST {url}/v1/audio/speech`.
    Http {
        url: String,
        model: Option<String>,
        client: reqwest::Client,
    },
    /// `espeak-ng --stdout`.
    Espeak { program: String },
}

impl Tts {
    pub fn http(url: &str, model: Option<String>) -> Tts {
        Tts::Http {
            url: url.trim_end_matches('/').to_string(),
            model,
            client: reqwest::Client::builder()
                .connect_timeout(Duration::from_secs(3))
                .timeout(Duration::from_secs(90))
                .build()
                .expect("Failed to build HTTP client"),
        }
    }

    pub fn espeak() -> Tts {
        Tts::Espeak {
            program: "espeak-ng".into(),
        }
    }

    /// Names the backend in cache keys and the file header.
    pub fn describe(&self) -> String {
        match self {
            Tts::Http { url, model, .. } => {
                format!("http {url} model {}", model.as_deref().unwrap_or("default"))
            }
            Tts::Espeak { program } => program.clone(),
        }
    }

    /// Speech for `text`, 16 kHz mono.
    pub async fn synthesize(&self, text: &str, voice: &str, speed: f32) -> Result<Vec<f32>> {
        let wav = match self {
            Tts::Http { url, model, client } => {
                let mut body = serde_json::json!({
                    "input": text,
                    "voice": voice,
                    "response_format": "wav",
                    "speed": speed,
                });
                if let Some(model) = model {
                    body["model"] = serde_json::Value::String(model.clone());
                }
                let response = client
                    .post(format!("{url}/v1/audio/speech"))
                    .json(&body)
                    .send()
                    .await
                    .context("TTS request")?;
                let status = response.status();
                if !status.is_success() {
                    let detail = response.text().await.unwrap_or_default();
                    bail!(
                        "TTS returned {status}: {}",
                        detail.chars().take(200).collect::<String>()
                    );
                }
                response.bytes().await.context("TTS response")?.to_vec()
            }
            Tts::Espeak { program } => {
                let wpm = (ESPEAK_WPM * speed).round().clamp(80.0, 450.0) as u32;
                let out = tokio::process::Command::new(program)
                    .args(["-v", voice, "-s", &wpm.to_string(), "--stdout", text])
                    .output()
                    .await
                    .with_context(|| format!("running {program}"))?;
                if !out.status.success() {
                    bail!(
                        "{program} failed: {}",
                        String::from_utf8_lossy(&out.stderr).trim()
                    );
                }
                out.stdout
            }
        };
        let (samples, rate) = parse_wav(&wav)?;
        Ok(resample(&samples, rate, pcm::SAMPLE_RATE))
    }
}

/// Mono float samples and the sample rate of a PCM16 or float32 WAV. A
/// streamed WAV's unknown data length (0 or 0xFFFFFFFF) reads to the end.
pub fn parse_wav(bytes: &[u8]) -> Result<(Vec<f32>, u32)> {
    if bytes.len() < 12 || &bytes[0..4] != b"RIFF" || &bytes[8..12] != b"WAVE" {
        bail!("not a WAV file");
    }
    let u16_at = |i: usize| u16::from_le_bytes([bytes[i], bytes[i + 1]]);
    let u32_at =
        |i: usize| u32::from_le_bytes([bytes[i], bytes[i + 1], bytes[i + 2], bytes[i + 3]]);
    let mut pos = 12;
    let mut fmt: Option<(u16, u16, u32, u16)> = None;
    while pos + 8 <= bytes.len() {
        let id = &bytes[pos..pos + 4];
        let len = u32_at(pos + 4) as usize;
        let start = pos + 8;
        if id == b"fmt " {
            if start + 16 > bytes.len() {
                bail!("truncated fmt chunk");
            }
            fmt = Some((
                u16_at(start),
                u16_at(start + 2),
                u32_at(start + 4),
                u16_at(start + 14),
            ));
        } else if id == b"data" {
            let (format, channels, rate, bits) = fmt.context("data before fmt chunk")?;
            let end = if len == 0 || len == u32::MAX as usize {
                bytes.len()
            } else {
                (start + len).min(bytes.len())
            };
            let data = &bytes[start..end];
            let channels = usize::from(channels.max(1));
            let frames: Vec<f32> = match (format, bits) {
                (1, 16) => pcm::samples_f32(data),
                (3, 32) => data
                    .as_chunks::<4>()
                    .0
                    .iter()
                    .map(|&b| f32::from_le_bytes(b))
                    .collect(),
                // WAVE_FORMAT_EXTENSIBLE: assume the common PCM16 case.
                (0xFFFE, 16) => pcm::samples_f32(data),
                _ => bail!("unsupported WAV format {format} with {bits} bits"),
            };
            let mono = frames
                .chunks(channels)
                .map(|f| f.iter().sum::<f32>() / f.len() as f32)
                .collect();
            return Ok((mono, rate));
        }
        pos = start + len + (len & 1);
    }
    bail!("WAV has no data chunk")
}

/// Windowed-sinc resampling (Hann window, 16 zero crossings each side, cut
/// off just under the lower Nyquist frequency).
pub fn resample(input: &[f32], from: u32, to: u32) -> Vec<f32> {
    if from == to || input.is_empty() || from == 0 || to == 0 {
        return input.to_vec();
    }
    const HALF: f64 = 16.0;
    let ratio = f64::from(to) / f64::from(from);
    let cutoff = ratio.min(1.0) * 0.95;
    let out_len = ((input.len() as f64) * ratio).round() as usize;
    let half_width = HALF / cutoff;
    let mut out = Vec::with_capacity(out_len);
    for n in 0..out_len {
        let t = n as f64 / ratio;
        let lo = (t - half_width).ceil().max(0.0) as usize;
        let hi = ((t + half_width).floor() as usize).min(input.len() - 1);
        let mut acc = 0.0f64;
        let mut norm = 0.0f64;
        for (k, &x) in input.iter().enumerate().take(hi + 1).skip(lo) {
            let d = t - k as f64;
            let arg = d * cutoff;
            let sinc = if arg.abs() < 1e-9 {
                1.0
            } else {
                (std::f64::consts::PI * arg).sin() / (std::f64::consts::PI * arg)
            };
            let w = 0.5 + 0.5 * (std::f64::consts::PI * d / half_width).cos();
            let h = sinc * w;
            acc += f64::from(x) * h;
            norm += h;
        }
        out.push(if norm.abs() > 1e-9 {
            (acc / norm) as f32
        } else {
            0.0
        });
    }
    out
}

/// A probe clip: `speech` (16 kHz) at -20 dBFS speech level, padded, with
/// `noise` laid over it, seeded by `seed`.
pub fn mix(speech: &[f32], noise: Noise, seed: u64) -> Vec<f32> {
    let level = pcm::speech_rms(speech);
    let gain = if level > 1e-6 {
        let g = pcm::from_dbfs(SPEECH_DBFS) / level;
        // Keep the peak under full scale.
        let peak = pcm::peak(speech) * g;
        if peak > 0.95 {
            g * 0.95 / peak
        } else {
            g
        }
    } else {
        1.0
    };
    let mut clip = vec![0.0f32; PAD_BEFORE];
    clip.extend(speech.iter().map(|s| s * gain));
    clip.extend(std::iter::repeat_n(0.0, PAD_AFTER));
    let snr = match noise {
        Noise::Clean => return clip,
        Noise::White(snr) | Noise::Pink(snr) => snr,
    };
    let mut rng = fastrand::Rng::with_seed(seed);
    let white: Vec<f32> = (0..clip.len()).map(|_| rng.f32() * 2.0 - 1.0).collect();
    let noise_samples = match noise {
        Noise::Pink(_) => pink(&white),
        _ => white,
    };
    let target = level * gain / 10f32.powf(snr / 20.0);
    let scale = target / pcm::rms(&noise_samples).max(1e-9);
    for (c, n) in clip.iter_mut().zip(noise_samples) {
        *c = (*c + n * scale).clamp(-1.0, 1.0);
    }
    clip
}

/// Pink noise from white: Paul Kellet's refined filter.
fn pink(white: &[f32]) -> Vec<f32> {
    let (mut b0, mut b1, mut b2, mut b3, mut b4, mut b5, mut b6) =
        (0.0f32, 0.0f32, 0.0f32, 0.0f32, 0.0f32, 0.0f32, 0.0f32);
    white
        .iter()
        .map(|&w| {
            b0 = 0.99886 * b0 + w * 0.0555179;
            b1 = 0.99332 * b1 + w * 0.0750759;
            b2 = 0.96900 * b2 + w * 0.153_852;
            b3 = 0.86650 * b3 + w * 0.3104856;
            b4 = 0.55000 * b4 + w * 0.5329522;
            b5 = -0.7616 * b5 - w * 0.0168980;
            let out = b0 + b1 + b2 + b3 + b4 + b5 + b6 + w * 0.5362;
            b6 = w * 0.115926;
            out
        })
        .collect()
}

// ---------------------------------------------------------------------------
// Classification and risk
// ---------------------------------------------------------------------------

/// Sound distances ([`risk::phonetic_distance`]) at most this are the same
/// phrase misheard as other words: about one changed sound in a short phrase
/// ("ears off"/"years off" 0, "tab"/"tap" 0.06, "ears vad"/"ears bad" 0.29).
const HOMOPHONE_DISTANCE: f32 = 0.30;
/// A changed name word this close to the one meant is the same name drifting.
const NAME_DRIFT_DISTANCE: f32 = 0.45;
/// At least this far apart, with no word in common, is a hallucination
/// ("ears on"/"adsense" 0.50).
const HALLUCINATION_DISTANCE: f32 = 0.5;

/// How `heard` fails `meant` (both normalized or not; normalized here).
/// First match wins:
/// 1. `spelled`: two or more single letters in a row ("v a d").
/// 2. `split`: the same letters, spaced differently ("back track",
///    "backtrack").
/// 3. `truncation`: fewer words, all of them from `meant`, in order
///    (more words with all of `meant` in order is `other`).
/// 4. `name_drift`: the words that changed are, on both sides, words the
///    risk lexicon does not know (names like "tmux", "clanker"), and sound
///    alike (distance at most 0.45): "tmux detach" -> "timux detach".
/// 5. `homophone`: sounds the same ([`risk::phonetic_distance`] at most 0.30).
/// 6. `hallucination`: no word in common and sounds unrelated (0.5 or more).
/// 7. `other`.
pub fn classify(meant: &str, heard: &str, lexicon: &Lexicon) -> FailureMode {
    let meant = normalize(meant);
    let heard = normalize(heard);
    let m: Vec<&str> = meant.split(' ').collect();
    let h: Vec<&str> = heard.split(' ').collect();
    let single = |w: &&str| w.chars().count() == 1 && w.chars().all(char::is_alphabetic);
    if h.windows(2).any(|p| single(&p[0]) && single(&p[1]))
        && !m.windows(2).any(|p| single(&p[0]) && single(&p[1]))
    {
        return FailureMode::Spelled;
    }
    if meant.replace(' ', "") == heard.replace(' ', "") {
        return FailureMode::Split;
    }
    if h.len() < m.len() && is_subsequence(&h, &m) {
        return FailureMode::Truncation;
    }
    // The whole phrase with words added: not a mishearing of its sound.
    if h.len() > m.len() && is_subsequence(&m, &h) {
        return FailureMode::Other;
    }
    if let Some((was, now)) = changed_words(&m, &h) {
        let unknown =
            |words: &[&str]| !words.is_empty() && words.iter().all(|w| !lexicon.contains(w));
        if unknown(&was)
            && unknown(&now)
            && risk::phonetic_distance(&was.join(" "), &now.join(" ")) <= NAME_DRIFT_DISTANCE
        {
            return FailureMode::NameDrift;
        }
    }
    let distance = risk::phonetic_distance(&meant, &heard);
    if distance <= HOMOPHONE_DISTANCE {
        return FailureMode::Homophone;
    }
    if distance >= HALLUCINATION_DISTANCE && !h.iter().any(|w| m.contains(w)) {
        return FailureMode::Hallucination;
    }
    FailureMode::Other
}

fn is_subsequence(short: &[&str], long: &[&str]) -> bool {
    let mut it = long.iter();
    short.iter().all(|w| it.any(|l| l == w))
}

/// The words of `m` and `h` between their common first and last words:
/// what changed. `None` when nothing in common frames the change.
fn changed_words<'a>(m: &[&'a str], h: &[&'a str]) -> Option<(Vec<&'a str>, Vec<&'a str>)> {
    let prefix = m.iter().zip(h).take_while(|(a, b)| a == b).count();
    let suffix = m[prefix..]
        .iter()
        .rev()
        .zip(h[prefix..].iter().rev())
        .take_while(|(a, b)| a == b)
        .count();
    if prefix + suffix == 0 {
        // A one-word phrase: the whole word changed.
        return (m.len() == 1 && h.len() == 1).then(|| (m.to_vec(), h.to_vec()));
    }
    Some((
        m[prefix..m.len() - suffix].to_vec(),
        h[prefix..h.len() - suffix].to_vec(),
    ))
}

/// Risk, reason and status of adding `heard` to `target` (as a way of
/// saying `meant`), from [`crate::cs::assess_entry`] (the rule `ears cs
/// check` uses) against every phrase the matcher knows.
pub fn assess(
    heard: &str,
    meant: &str,
    target: &str,
    known: &[Known],
    lexicon: &Lexicon,
) -> (f32, String, Status) {
    let others: Vec<risk::Phrase<'_>> = known
        .iter()
        .map(|k| risk::Phrase {
            text: &k.text,
            target: &k.target,
        })
        .collect();
    let r = assess_entry(heard, meant, target, &others, lexicon);
    let status = match r.verdict {
        Verdict::Ok => Status::Active,
        Verdict::Flag => Status::Flagged,
        Verdict::Reject => Status::Rejected,
    };
    // Two decimals, as written, so a file reads back equal.
    ((r.score * 100.0).round() / 100.0, r.reason, status)
}

// ---------------------------------------------------------------------------
// Aggregation and entries
// ---------------------------------------------------------------------------

/// What the ASR made of one canonical phrase.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Tally {
    pub target: String,
    pub meant: String,
    /// Trials with an answer (an empty transcript counts, as "").
    pub trials: u32,
    /// Trials the ASR request failed for (not counted in `trials`).
    pub errors: u32,
    /// Normalized hearing -> count. "" is silence: nothing came back.
    pub hearings: BTreeMap<String, u32>,
}

impl Tally {
    pub fn new(canonical: &Canonical) -> Tally {
        Tally {
            target: canonical.target.clone(),
            meant: canonical.meant.clone(),
            trials: 0,
            errors: 0,
            hearings: BTreeMap::new(),
        }
    }

    pub fn add(&mut self, heard: Option<&str>) {
        match heard {
            Some(text) => {
                self.trials += 1;
                *self.hearings.entry(normalize(text)).or_default() += 1;
            }
            None => self.errors += 1,
        }
    }

    /// Trials heard exactly as the canonical phrase.
    pub fn hits(&self) -> u32 {
        self.hearings
            .get(&normalize(&self.meant))
            .copied()
            .unwrap_or(0)
    }

    /// Hearings, most frequent first (ties alphabetically).
    pub fn ranked(&self) -> Vec<(&str, u32)> {
        let mut v: Vec<(&str, u32)> = self
            .hearings
            .iter()
            .map(|(h, &c)| (h.as_str(), c))
            .collect();
        v.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(b.0)));
        v
    }
}

/// Count `results` (phrase index, hearing or `None` for a failed request)
/// per phrase.
pub fn aggregate(phrases: &[Canonical], results: &[(usize, Option<String>)]) -> Vec<Tally> {
    let mut tallies: Vec<Tally> = phrases.iter().map(Tally::new).collect();
    for (i, heard) in results {
        if let Some(t) = tallies.get_mut(*i) {
            t.add(heard.as_deref());
        }
    }
    tallies
}

/// Whether `heard` is written in the script `meant` is: a Latin phrase heard
/// in Chinese or Thai is the ASR guessing the language wrong (with no
/// language pinned), not a way the phrase sounds; with English pinned the
/// model never writes it.
pub fn same_script(meant: &str, heard: &str) -> bool {
    let latin = |c: char| c.is_ascii() || ('\u{00C0}'..='\u{024F}').contains(&c);
    !meant.chars().all(latin) || heard.chars().all(latin)
}

/// Whether `target`'s phrases are matched while dictating: every
/// `[commands]` list but `wake` (heard only in standby), and run commands.
/// Command-mode actions and decide options are heard only in their modes.
pub fn fires_during_dictation(target: &str) -> bool {
    target.starts_with("run:")
        || (target != "wake"
            && command_lists(&VoiceCommands::default())
                .iter()
                .any(|(name, _)| *name == target))
}

/// Failure modes that are the phrase itself, misheard: the sound or the
/// letters are still there. Anything else (ordinary words in place of the
/// phrase) needs review before it can fire mid-dictation.
fn sounds_like_the_phrase(mode: FailureMode) -> bool {
    matches!(
        mode,
        FailureMode::Homophone | FailureMode::Split | FailureMode::Spelled | FailureMode::NameDrift
    )
}

/// Entries for the hearings that differ from their canonical phrase, are
/// not empty, are in its script ([`same_script`]), and make up at least
/// `min_fraction` of its trials. Ordered by phrase, then count.
///
/// Status is [`risk::assess`]'s, except that a hearing for a target that
/// fires during dictation ([`fires_during_dictation`]) which does not sound
/// like the phrase ([`sounds_like_the_phrase`]) is at most `flagged`:
/// "here s that" for standby would put VAD in standby mid-sentence.
pub fn propose(tallies: &[Tally], known: &[Known], min_fraction: f32) -> Vec<Entry> {
    let lexicon = Lexicon::builtin();
    let mut entries = Vec::new();
    for tally in tallies {
        if tally.trials == 0 {
            continue;
        }
        let meant = normalize(&tally.meant);
        for (heard, count) in tally.ranked() {
            if heard.is_empty() || heard == meant || !same_script(&meant, heard) {
                continue;
            }
            if (count as f32) < min_fraction * tally.trials as f32 {
                continue;
            }
            let (risk, mut risk_reason, mut status) =
                assess(heard, &tally.meant, &tally.target, known, &lexicon);
            let mode = classify(&tally.meant, heard, &lexicon);
            if status == Status::Active
                && fires_during_dictation(&tally.target)
                && !sounds_like_the_phrase(mode)
            {
                status = Status::Flagged;
                risk_reason.push_str(&format!(
                    "; flagged: {} is heard while dictating and this does not sound like {:?}",
                    tally.target, tally.meant
                ));
            }
            entries.push(Entry {
                meant: tally.meant.clone(),
                target: tally.target.clone(),
                heard: heard.to_string(),
                mode,
                source: Source::Probe,
                count,
                trials: tally.trials,
                risk,
                risk_reason,
                status,
            });
        }
    }
    entries
}

fn quoted(s: &str) -> String {
    toml::Value::String(s.to_string()).to_string()
}

/// The cs file: `meta`, then the entries grouped by phrase, each group
/// introduced by a comment with the phrase's hit rate. `header` lines go
/// first, as comments.
pub fn render(meta: &Meta, header: &[String], tallies: &[Tally], entries: &[Entry]) -> String {
    let mut out = String::new();
    for line in header {
        let _ = writeln!(out, "# {line}");
    }
    if !header.is_empty() {
        out.push('\n');
    }
    out.push_str("[meta]\n");
    let _ = writeln!(out, "model = {}", quoted(&meta.model));
    let _ = writeln!(out, "domain = {}", quoted(&meta.domain));
    let _ = writeln!(out, "generated = {}", quoted(&meta.generated));
    let _ = writeln!(out, "generator = {}", quoted(&meta.generator));
    for tally in tallies {
        let group: Vec<&Entry> = entries
            .iter()
            .filter(|e| e.target == tally.target && e.meant == tally.meant)
            .collect();
        if group.is_empty() {
            continue;
        }
        let _ = writeln!(
            out,
            "\n# {} {:?}: heard exactly {}/{}",
            tally.target,
            tally.meant,
            tally.hits(),
            tally.trials
        );
        for e in group {
            out.push_str("\n[[entry]]\n");
            let _ = writeln!(out, "meant = {}", quoted(&e.meant));
            let _ = writeln!(out, "target = {}", quoted(&e.target));
            let _ = writeln!(out, "heard = {}", quoted(&e.heard));
            let _ = writeln!(out, "mode = {}", quoted(e.mode.as_str()));
            out.push_str("source = \"probe\"\n");
            let _ = writeln!(out, "count = {}", e.count);
            let _ = writeln!(out, "trials = {}", e.trials);
            let _ = writeln!(out, "risk = {:.2}", e.risk);
            let _ = writeln!(out, "risk_reason = {}", quoted(&e.risk_reason));
            let _ = writeln!(out, "status = {}", quoted(e.status.as_str()));
        }
    }
    out
}

/// Plain-text summary: per phrase the hit rate and its hearings.
pub fn report(tallies: &[Tally], entries: &[Entry]) -> String {
    let mut out = String::new();
    for t in tallies {
        let pct = if t.trials > 0 {
            100.0 * t.hits() as f32 / t.trials as f32
        } else {
            0.0
        };
        let _ = write!(
            out,
            "{:<22} {:<22} {:>2}/{:<2} {:>4.0}%",
            t.target,
            format!("{:?}", t.meant),
            t.hits(),
            t.trials,
            pct
        );
        if t.errors > 0 {
            let _ = write!(out, " ({} failed)", t.errors);
        }
        let misses: Vec<String> = t
            .ranked()
            .into_iter()
            .filter(|(h, _)| *h != normalize(&t.meant))
            .map(|(h, c)| {
                let script = same_script(&normalize(&t.meant), h);
                let h = if h.is_empty() { "(nothing)" } else { h };
                let mark = entries
                    .iter()
                    .find(|e| e.target == t.target && e.heard == h)
                    .map_or(if script { "" } else { " [other script]" }, |e| {
                        match e.status {
                            Status::Rejected => " [rejected]",
                            Status::Flagged => " [flagged]",
                            Status::Active => " [entry]",
                        }
                    });
                format!("{h:?} x{c}{mark}")
            })
            .collect();
        if !misses.is_empty() {
            let _ = write!(out, "  {}", misses.join(", "));
        }
        out.push('\n');
    }
    out
}

/// Today as YYYY-MM-DD (UTC).
pub fn today() -> String {
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs());
    civil_date((secs / 86_400) as i64)
}

/// Days since 1970-01-01 as YYYY-MM-DD (Howard Hinnant's civil_from_days).
fn civil_date(days: i64) -> String {
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + i64::from(m <= 2);
    format!("{y:04}-{m:02}-{d:02}")
}

// ---------------------------------------------------------------------------
// Running
// ---------------------------------------------------------------------------

/// The ASR side of a run.
pub struct Asr {
    pub client: WhisperClient,
    /// Model id the transcripts are cached under.
    pub model: String,
    /// Language and prompt sent, part of the transcript cache key.
    pub language: Option<String>,
    pub prompt: Option<String>,
}

/// First model id of the server's `/v1/models`.
pub async fn server_model(server: &str, api_key: Option<&str>) -> Result<String> {
    let url = format!("{}/v1/models", server.trim_end_matches('/'));
    let client = reqwest::Client::builder()
        .connect_timeout(Duration::from_secs(2))
        .timeout(Duration::from_secs(5))
        .build()?;
    let mut request = client.get(&url);
    if let Some(key) = api_key {
        request = request.bearer_auth(key);
    }
    let response = request.send().await.with_context(|| format!("GET {url}"))?;
    if !response.status().is_success() {
        bail!("GET {url}: {}", response.status());
    }
    let models: serde_json::Value = response.json().await?;
    models["data"][0]["id"]
        .as_str()
        .map(str::to_string)
        .with_context(|| format!("{url} lists no model"))
}

/// Run settings.
pub struct Run {
    pub tts: Tts,
    pub asr: Asr,
    pub cache: PathBuf,
    pub concurrency: usize,
    /// Most uncached ASR requests (and, separately, TTS requests) allowed.
    pub max_requests: usize,
}

fn digest(parts: &[&str]) -> String {
    let mut h = Sha256::new();
    for p in parts {
        h.update(p.as_bytes());
        h.update([0u8]);
    }
    h.finalize()
        .iter()
        .take(16)
        .map(|b| format!("{b:02x}"))
        .collect()
}

impl Run {
    fn clip_path(&self, plan: &Plan, s: &Synthesis) -> PathBuf {
        let key = digest(&[
            &self.tts.describe(),
            &s.voice,
            &format!("{:.3}", s.speed),
            &plan.phrases[s.phrase].meant,
        ]);
        self.cache.join("clips").join(format!("{key}.wav"))
    }

    fn transcript_path(&self, clip: &Path, noise: Noise) -> PathBuf {
        let key = digest(&[
            &self.asr.model,
            self.asr.language.as_deref().unwrap_or(""),
            self.asr.prompt.as_deref().unwrap_or(""),
            &clip.to_string_lossy(),
            &noise.label(),
        ]);
        self.cache.join("asr").join(format!("{key}.txt"))
    }

    /// Uncached (TTS, ASR) requests the plan needs.
    pub fn requests_needed(&self, plan: &Plan) -> (usize, usize) {
        let mut tts = 0;
        let mut asr = 0;
        for s in &plan.syntheses {
            let clip = self.clip_path(plan, s);
            if !clip.exists() {
                tts += 1;
            }
            for &noise in &plan.noises {
                if !self.transcript_path(&clip, noise).exists() {
                    asr += 1;
                }
            }
        }
        (tts, asr)
    }

    /// Run the plan; returns (phrase index, hearing) per trial, `None` where
    /// a request failed.
    pub async fn execute(&self, plan: &Plan) -> Result<Vec<(usize, Option<String>)>> {
        let (tts_needed, asr_needed) = self.requests_needed(plan);
        if asr_needed > self.max_requests || tts_needed > self.max_requests {
            bail!(
                "the plan needs {asr_needed} ASR and {tts_needed} TTS requests, over the cap of {} \
                 (--max-requests); probe fewer phrases or conditions",
                self.max_requests
            );
        }
        std::fs::create_dir_all(self.cache.join("clips"))
            .with_context(|| format!("creating {}", self.cache.display()))?;
        std::fs::create_dir_all(self.cache.join("asr"))?;
        let work = tempfile::Builder::new().prefix("ears-cs-probe").tempdir()?;
        let tts_sent = AtomicUsize::new(0);
        let asr_sent = AtomicUsize::new(0);
        let done = AtomicUsize::new(0);
        let total = plan.syntheses.len();
        let concurrency = self.concurrency.clamp(1, MAX_CONCURRENCY);
        let counters = (&tts_sent, &asr_sent, &done, work.path());
        let results: Vec<Vec<(usize, Option<String>)>> = stream::iter(plan.syntheses.iter())
            .map(|s| self.one_synthesis(plan, s, counters, total))
            .buffer_unordered(concurrency)
            .collect()
            .await;
        eprintln!(
            "probe: {} TTS and {} ASR requests sent",
            tts_sent.load(Ordering::Relaxed),
            asr_sent.load(Ordering::Relaxed)
        );
        let mut results: Vec<(usize, Option<String>)> = results.into_iter().flatten().collect();
        for &(alias, first) in &plan.aliases {
            let copies: Vec<_> = results
                .iter()
                .filter(|(i, _)| *i == first)
                .map(|(_, heard)| (alias, heard.clone()))
                .collect();
            results.extend(copies);
        }
        Ok(results)
    }

    async fn one_synthesis(
        &self,
        plan: &Plan,
        s: &Synthesis,
        (tts_sent, asr_sent, done, work): (&AtomicUsize, &AtomicUsize, &AtomicUsize, &Path),
        total: usize,
    ) -> Vec<(usize, Option<String>)> {
        let phrase = &plan.phrases[s.phrase];
        let failed = || plan.noises.iter().map(|_| (s.phrase, None)).collect();
        let clip_path = self.clip_path(plan, s);
        let speech = match std::fs::read(&clip_path) {
            Ok(bytes) => match parse_wav(&bytes) {
                Ok((samples, _)) => samples,
                Err(e) => {
                    eprintln!("probe: bad cached clip {}: {e:#}", clip_path.display());
                    return failed();
                }
            },
            Err(_) => {
                if tts_sent.fetch_add(1, Ordering::SeqCst) >= self.max_requests {
                    return failed();
                }
                match self.tts.synthesize(&phrase.meant, &s.voice, s.speed).await {
                    Ok(samples) if !samples.is_empty() => {
                        let tmp = clip_path.with_extension("wav.tmp");
                        if let Err(e) = pcm::write_wav(&tmp, &samples)
                            .and_then(|_| std::fs::rename(&tmp, &clip_path))
                        {
                            eprintln!("probe: caching {}: {e}", clip_path.display());
                        }
                        samples
                    }
                    Ok(_) => {
                        eprintln!(
                            "probe: TTS gave no audio for {:?} ({})",
                            phrase.meant, s.voice
                        );
                        return failed();
                    }
                    Err(e) => {
                        eprintln!("probe: TTS {:?} ({}): {e:#}", phrase.meant, s.voice);
                        return failed();
                    }
                }
            }
        };
        let mut out = Vec::new();
        for &noise in &plan.noises {
            let cached = self.transcript_path(&clip_path, noise);
            if let Ok(text) = std::fs::read_to_string(&cached) {
                out.push((s.phrase, Some(text)));
                continue;
            }
            if asr_sent.fetch_add(1, Ordering::SeqCst) >= self.max_requests {
                out.push((s.phrase, None));
                continue;
            }
            let seed = u64::from_str_radix(
                &digest(&[&clip_path.to_string_lossy(), &noise.label()])[..16],
                16,
            )
            .unwrap_or(0);
            let clip = mix(&speech, noise, seed);
            let file = work.join(format!(
                "{}.wav",
                digest(&[&clip_path.to_string_lossy(), &noise.label()])
            ));
            if let Err(e) = pcm::write_wav(&file, &clip) {
                eprintln!("probe: writing {}: {e}", file.display());
                out.push((s.phrase, None));
                continue;
            }
            let heard = match self.asr.client.transcribe(&file).await {
                Ok(text) => Some(normalize(&text)),
                Err(WhisperError::EmptyTranscription) => Some(String::new()),
                Err(e) => {
                    eprintln!("probe: ASR {:?}: {e}", phrase.meant);
                    None
                }
            };
            let _ = std::fs::remove_file(&file);
            if let Some(text) = &heard {
                let _ = std::fs::write(&cached, text);
            }
            out.push((s.phrase, heard));
        }
        let n = done.fetch_add(1, Ordering::SeqCst) + 1;
        if n % 10 == 0 || n == total {
            eprintln!("probe: {n}/{total} syntheses done");
        }
        out
    }
}

// ---------------------------------------------------------------------------
// `ears cs build`
// ---------------------------------------------------------------------------

/// TTS backend for `ears cs build`.
#[derive(clap::ValueEnum, Clone, Copy, Debug, PartialEq, Eq)]
pub enum TtsBackend {
    /// `--tts-url` if given, else espeak-ng.
    Auto,
    /// OpenAI-shaped `/v1/audio/speech` at `--tts-url`.
    Http,
    /// Local espeak-ng.
    Espeak,
}

/// `ears cs build` options.
#[derive(clap::Args, Debug, Clone)]
pub struct BuildArgs {
    /// Targets to probe, comma separated: a target (`wake`, `run:centurion`,
    /// `action:close`), `lists` (every [commands] list), `run:*`, `action:*`.
    #[arg(long, value_delimiter = ',', default_value = "lists,run:*")]
    pub targets: Vec<String>,
    /// TTS backend.
    #[arg(long, value_enum, default_value = "auto")]
    pub tts: TtsBackend,
    /// Base URL of an OpenAI-shaped TTS server (POST /v1/audio/speech).
    #[arg(long, env = "EARS_CS_TTS_URL")]
    pub tts_url: Option<String>,
    /// TTS model name sent with each request (default: the server's).
    #[arg(long)]
    pub tts_model: Option<String>,
    /// Voices, comma separated (default: alloy,echo,nova for http;
    /// en-us,en-gb,en-us+f3 for espeak-ng).
    #[arg(long, value_delimiter = ',')]
    pub voices: Vec<String>,
    /// Speaking rates, comma separated (1.0 = normal).
    #[arg(long, value_delimiter = ',', default_value = "0.9,1.15")]
    pub speeds: Vec<f32>,
    /// Noise conditions, comma separated: clean, white:SNR, pink:SNR (dB).
    #[arg(long, value_delimiter = ',', default_value = "clean,pink:10")]
    pub noise: Vec<String>,
    /// A hearing becomes an entry when it is at least this fraction of the
    /// phrase's trials.
    #[arg(long, default_value_t = 0.1)]
    pub min_fraction: f32,
    /// Requests in flight (at most 2).
    #[arg(long, default_value_t = 2)]
    pub concurrency: usize,
    /// Most uncached ASR requests (and TTS requests) the run may send; it
    /// refuses to start when the plan needs more.
    #[arg(long, default_value_t = 600)]
    pub max_requests: usize,
    /// Where clips and transcripts are cached (default: the ears cache
    /// directory).
    #[arg(long)]
    pub cache_dir: Option<PathBuf>,
    /// ASR model id (default: the first of the server's /v1/models).
    #[arg(long)]
    pub model: Option<String>,
    /// Transcription language (default: as dictation picks it).
    #[arg(long)]
    pub language: Option<String>,
    /// Output file (default: cs/<model>/commands.toml).
    #[arg(long, short)]
    pub out: Option<PathBuf>,
    /// Also write rejected hearings, so later tools do not propose them.
    #[arg(long)]
    pub keep_rejected: bool,
    /// Probe and check against the built-in phrases only (built-in
    /// `[commands]` lists and run commands, the shipped actions.toml), not
    /// this desk's config: for the defaults shipped in `cs/<model>/`.
    /// Targets only this desk has are skipped.
    #[arg(long)]
    pub builtin: bool,
    /// Print the plan and request counts, send nothing.
    #[arg(long)]
    pub dry_run: bool,
}

fn default_voices(tts: &Tts) -> Vec<String> {
    let v: &[&str] = match tts {
        Tts::Http { .. } => &["alloy", "echo", "nova"],
        Tts::Espeak { .. } => &["en-us", "en-gb", "en-us+f3"],
    };
    v.iter().map(|s| s.to_string()).collect()
}

/// The actions command mode would load, without writing the default file
/// when it is missing (as `command_mode::load_actions` does).
fn read_actions(path: &Path) -> Result<Vec<Action>> {
    let text = match std::fs::read_to_string(path) {
        Ok(text) => text,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => DEFAULT_ACTIONS.to_string(),
        Err(e) => return Err(e).with_context(|| format!("reading {}", path.display())),
    };
    let (_, actions) =
        parse_actions(&text).map_err(|e| anyhow::anyhow!("{}: {e}", path.display()))?;
    Ok(actions)
}

/// The phrases every install has: the built-in `[commands]` lists and run
/// commands, and the shipped command-mode actions.
pub fn builtin_phrases() -> Phrases {
    let (_, actions) = parse_actions(DEFAULT_ACTIONS).expect("shipped actions.toml parses");
    collect_phrases(&VoiceCommands::default(), &actions)
}

/// `ears cs build`.
pub async fn build(config: &crate::Config, args: &BuildArgs) -> Result<()> {
    let phrases = if args.builtin {
        builtin_phrases()
    } else {
        let actions_path = crate::command_mode::Paths::new(&config.state_dir).actions;
        collect_phrases(&config.commands, &read_actions(&actions_path)?)
    };
    let (selected, missing) = select(&phrases, &args.targets, args.builtin)?;
    let selected: Vec<Canonical> = selected.into_iter().cloned().collect();
    if !missing.is_empty() {
        println!("Not built in, skipped: {}", missing.join(", "));
    }

    let tts = match (args.tts, &args.tts_url) {
        (TtsBackend::Http, None) => bail!("--tts http needs --tts-url"),
        (TtsBackend::Http | TtsBackend::Auto, Some(url)) => Tts::http(url, args.tts_model.clone()),
        (TtsBackend::Espeak, _) | (TtsBackend::Auto, None) => Tts::espeak(),
    };
    let voices = if args.voices.is_empty() {
        default_voices(&tts)
    } else {
        args.voices.clone()
    };
    let noises = args
        .noise
        .iter()
        .map(|n| Noise::parse(n))
        .collect::<Result<Vec<_>>>()?;
    if args.speeds.iter().any(|s| !(0.25..=4.0).contains(s)) {
        bail!("speeds must be between 0.25 and 4");
    }
    let plan = Plan::new(selected, &voices, &args.speeds, &noises);

    let language = args
        .language
        .clone()
        .or_else(|| crate::KeyboardLayout::detect_language().or_else(|| config.language.clone()));
    let (server, server_model_name) = config.resolve_server(language.as_deref());
    let model = match &args.model {
        Some(m) => m.clone(),
        None => server_model(server.as_str(), config.api_key.as_deref())
            .await
            .context("asking the ASR server for its model (or pass --model)")?,
    };
    // One attempt per clip, no no-speech double check: one request a trial.
    let client = WhisperClient::with_retry_config(server.to_string(), 0, 100, 1000)
        .with_language(language.clone())
        .with_api_key(config.api_key.clone())
        .with_model(server_model_name)
        .with_silence_filter(false)
        .with_prompt(config.prompt.clone());
    let cache = match &args.cache_dir {
        Some(dir) => dir.clone(),
        None => directories::ProjectDirs::from("com", "heiervang", "ears")
            .map(|d| d.cache_dir().join("cs-probe"))
            .context("no cache directory; pass --cache-dir")?,
    };
    let run = Run {
        tts,
        asr: Asr {
            client,
            model: model.clone(),
            language: language.clone(),
            prompt: config.prompt.clone(),
        },
        cache,
        concurrency: args.concurrency,
        max_requests: args.max_requests,
    };

    let (tts_needed, asr_needed) = run.requests_needed(&plan);
    println!(
        "Probe of {} ({}) at {server}, language {}",
        model,
        model_key(&model),
        language.as_deref().unwrap_or("auto")
    );
    println!("TTS: {}; voices {}", run.tts.describe(), voices.join(", "));
    println!(
        "Speeds {:?}; noise {}",
        args.speeds,
        noises
            .iter()
            .map(Noise::label)
            .collect::<Vec<_>>()
            .join(", ")
    );
    println!(
        "{} phrases ({} with distinct words) x {} syntheses x {} noise conditions = {} trials",
        plan.phrases.len(),
        plan.phrases.len() - plan.aliases.len(),
        voices.len() * args.speeds.len(),
        noises.len(),
        plan.trials()
    );
    println!(
        "Requests needed (uncached): {tts_needed} TTS, {asr_needed} ASR (cap {}); cache {}",
        args.max_requests,
        run.cache.display()
    );
    if args.dry_run {
        for c in &plan.phrases {
            println!("  {:<24} {:?}", c.target, c.meant);
        }
        return Ok(());
    }

    let results = run.execute(&plan).await?;
    let tallies = aggregate(&plan.phrases, &results);
    let mut entries = propose(&tallies, &phrases.known, args.min_fraction);
    print!("{}", report(&tallies, &entries));
    if !args.keep_rejected {
        entries.retain(|e| e.status != Status::Rejected);
    }

    let slug = model_key(&model);
    let meta = Meta {
        model: slug.clone(),
        domain: "commands".into(),
        generated: today(),
        generator: "ears cs build".into(),
    };
    let header = vec![
        format!(
            "Generated by `ears cs build` against ASR model {model}, language {}.",
            language.as_deref().unwrap_or("auto")
        ),
        format!(
            "TTS: {}; voices: {}.",
            tts_label(&run.tts),
            voices.join(", ")
        ),
        format!(
            "Speeds: {}; noise: {} (synthetic, SNR in dB against the speech level).",
            args.speeds
                .iter()
                .map(|s| s.to_string())
                .collect::<Vec<_>>()
                .join(", "),
            noises
                .iter()
                .map(Noise::label)
                .collect::<Vec<_>>()
                .join(", ")
        ),
        format!(
            "Phrases and collision checks: {}.",
            if args.builtin {
                "built-in lists, run commands and shipped actions"
            } else {
                "this desk's config and actions.toml"
            }
        ),
        format!(
            "{} trials per phrase; a hearing is listed from {:.0}% of them.",
            voices.len() * args.speeds.len() * noises.len(),
            args.min_fraction * 100.0
        ),
    ];
    let text = render(&meta, &header, &tallies, &entries);
    let out = args
        .out
        .clone()
        .unwrap_or_else(|| PathBuf::from("cs").join(&slug).join("commands.toml"));
    if let Some(dir) = out.parent().filter(|d| !d.as_os_str().is_empty()) {
        std::fs::create_dir_all(dir)?;
    }
    std::fs::write(&out, text).with_context(|| format!("writing {}", out.display()))?;
    println!("Wrote {} entries to {}", entries.len(), out.display());
    Ok(())
}

/// The TTS backend for the file header, without the server address.
fn tts_label(tts: &Tts) -> String {
    match tts {
        Tts::Http { model, .. } => format!(
            "OpenAI-shaped /v1/audio/speech, model {}",
            model.as_deref().unwrap_or("server default")
        ),
        Tts::Espeak { program } => program.clone(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, Request, ResponseTemplate};

    fn canonical(target: &str, meant: &str) -> Canonical {
        Canonical {
            target: target.into(),
            meant: meant.into(),
        }
    }

    fn known(target: &str, text: &str) -> Known {
        Known {
            target: target.into(),
            text: text.into(),
        }
    }

    #[test]
    fn classifies_the_failure_modes_in_the_rfc() {
        let lx = Lexicon::builtin();
        let c = |m: &str, h: &str| classify(m, h, &lx);
        assert_eq!(c("ears vad", "ears v a d"), FailureMode::Spelled);
        assert_eq!(c("backtrack", "back track"), FailureMode::Split);
        assert_eq!(c("back track", "backtrack"), FailureMode::Split);
        assert_eq!(c("ears off", "years off"), FailureMode::Homophone);
        assert_eq!(c("tab", "tap"), FailureMode::Homophone);
        assert_eq!(c("ears on", "ears"), FailureMode::Truncation);
        assert_eq!(c("ears on", "adsense"), FailureMode::Hallucination);
        assert_eq!(c("over and out", "over and out now"), FailureMode::Other);
        assert_eq!(c("tmux detach", "timux detach"), FailureMode::NameDrift);
        assert_eq!(c("hey clanker", "hey klinker"), FailureMode::NameDrift);
        // A known word heard instead of a name is not a name drifting.
        assert_ne!(c("ears vad", "ears bad"), FailureMode::NameDrift);
    }

    #[test]
    fn spelled_needs_letters_the_phrase_does_not_have() {
        // Already spelled in the phrase: not a spelling failure.
        let lx = Lexicon::builtin();
        assert_ne!(
            classify("ears v a d", "ears v a d o", &lx),
            FailureMode::Spelled
        );
    }

    #[test]
    fn changed_words_frames_the_difference() {
        let w = |s: &'static str| s.split(' ').collect::<Vec<_>>();
        assert_eq!(
            changed_words(&w("tmux new window"), &w("smokes new window")),
            Some((w("tmux"), w("smokes")))
        );
        assert_eq!(
            changed_words(&w("halt"), &w("hi")),
            Some((w("halt"), w("hi")))
        );
        assert_eq!(changed_words(&w("ears on"), &w("adsense now")), None);
    }

    #[test]
    fn aggregates_hearings_per_phrase() {
        let phrases = vec![canonical("wake", "ears on"), canonical("sleep", "ears off")];
        let results = vec![
            (0, Some("Ears on.".to_string())),
            (0, Some("years on".to_string())),
            (0, Some("Years on!".to_string())),
            (0, None),
            (1, Some("ears off".to_string())),
            (1, Some(String::new())),
        ];
        let t = aggregate(&phrases, &results);
        assert_eq!(t[0].trials, 3);
        assert_eq!(t[0].errors, 1);
        assert_eq!(t[0].hits(), 1);
        assert_eq!(t[0].ranked(), vec![("years on", 2), ("ears on", 1)]);
        assert_eq!(t[1].trials, 2);
        assert_eq!(t[1].hits(), 1);
        assert_eq!(t[1].hearings.get(""), Some(&1));
    }

    #[test]
    fn proposes_frequent_differing_hearings_and_rejects_collisions() {
        let phrases = vec![canonical("wake", "ears on")];
        let mut results: Vec<(usize, Option<String>)> = Vec::new();
        for h in ["ears on"; 5]
            .into_iter()
            .chain(["years on"; 3])
            .chain(["ears off"; 2])
            .chain(["adsense"])
            .chain([""; 2])
            .chain(["什么"; 3])
        {
            results.push((0, Some(h.to_string())));
        }
        let tallies = aggregate(&phrases, &results);
        let known = vec![known("wake", "ears on"), known("sleep", "ears off")];
        let entries = propose(&tallies, &known, 0.1);
        let heard: Vec<&str> = entries.iter().map(|e| e.heard.as_str()).collect();
        // "adsense" is 1/16, under 10%; silence and another script never
        // become entries.
        assert_eq!(heard, vec!["years on", "ears off"]);
        assert_eq!(entries[0].status, Status::Active);
        assert_eq!(entries[0].count, 3);
        assert_eq!(entries[0].trials, 16);
        assert_eq!(entries[0].mode, FailureMode::Homophone);
        assert_eq!(entries[1].status, Status::Rejected);
        assert!(entries[1].risk_reason.contains("sleep"));
        assert_eq!(entries[1].risk, 1.0);
    }

    #[test]
    fn a_target_sharing_the_meant_words_is_no_collision() {
        let known = vec![
            known("wake", "ears on"),
            known("run:regular", "ears on"),
            known("sleep", "ears off"),
        ];
        let lx = Lexicon::builtin();
        let (_, _, wake) = assess("years on", "ears on", "wake", &known, &lx);
        assert_ne!(wake, Status::Rejected);
        // Meant "ears off", heard like the wake phrase: a real collision.
        let (risk, reason, sleep) = assess("years on", "ears off", "sleep", &known, &lx);
        assert_eq!(sleep, Status::Rejected, "{risk} {reason}");
    }

    #[test]
    fn ordinary_words_for_a_dictation_target_are_never_active() {
        let tally = |target: &str, meant: &str, heard: &str| Tally {
            target: target.into(),
            meant: meant.into(),
            trials: 16,
            errors: 0,
            hearings: BTreeMap::from([(heard.into(), 4)]),
        };
        let known = builtin_phrases().known;
        let standby = propose(&[tally("standby", "ears vad", "here s that")], &known, 0.1);
        assert_eq!(standby[0].mode, FailureMode::Other);
        assert_ne!(standby[0].status, Status::Active, "{:?}", standby[0]);
        // A sound-alike for standby, or anything for a command-mode action,
        // keeps the scorer's verdict.
        let bad = propose(&[tally("standby", "ears vad", "ears bad")], &known, 0.1);
        assert_eq!(bad[0].status, Status::Active, "{:?}", bad[0]);
        let man = propose(&[tally("action:leave", "command", "a man")], &known, 0.1);
        assert_eq!(man[0].status, Status::Active, "{:?}", man[0]);
        assert!(fires_during_dictation("sleep") && fires_during_dictation("run:friend-wake"));
        assert!(!fires_during_dictation("wake") && !fires_during_dictation("action:tab"));
    }

    #[test]
    fn builtin_phrases_have_no_desk_targets() {
        let p = builtin_phrases();
        let has = |t: &str| p.canonical.iter().any(|c| c.target == t);
        assert!(has("wake") && has("run:friend-wake") && has("action:tab") && has("action:leave"));
        assert!(!has("run:regular") && !has("action:halt") && !has("action:tmux-detach"));
    }

    #[test]
    fn canonical_phrase_comes_from_the_migrate_split() {
        let list: Vec<String> = ["hey clanker", "hay clanker", "listen clanker"]
            .iter()
            .map(|s| s.to_string())
            .collect();
        assert_eq!(
            canonical_of("run:x", &[], &list).as_deref(),
            Some("hey clanker")
        );
        let wake: Vec<String> = vec!["ears on".into(), "years on".into()];
        assert_eq!(
            canonical_of("wake", &seeds("wake"), &wake).as_deref(),
            Some("ears on")
        );
    }

    #[test]
    fn render_writes_the_spec_format() {
        let tally = Tally {
            target: "wake".into(),
            meant: "ears on".into(),
            trials: 16,
            errors: 0,
            hearings: BTreeMap::from([("ears on".into(), 12), ("years on".into(), 4)]),
        };
        let entries = propose(std::slice::from_ref(&tally), &[], 0.1);
        let meta = Meta {
            model: "qwen3-asr-1.7b".into(),
            domain: "commands".into(),
            generated: "2026-11-13".into(),
            generator: "ears cs build".into(),
        };
        let text = render(&meta, &["a header".into()], &[tally], &entries);
        assert!(text.starts_with("# a header\n"));
        assert!(text.contains("# wake \"ears on\": heard exactly 12/16"));
        let parsed = crate::cs::CsFile::parse(&text).unwrap();
        assert_eq!(parsed.entry, entries, "core reads back what was written");
        let value: toml::Value = toml::from_str(&text).unwrap();
        assert_eq!(value["meta"]["model"].as_str(), Some("qwen3-asr-1.7b"));
        assert_eq!(value["meta"]["generator"].as_str(), Some("ears cs build"));
        let e = &value["entry"][0];
        assert_eq!(e["meant"].as_str(), Some("ears on"));
        assert_eq!(e["target"].as_str(), Some("wake"));
        assert_eq!(e["heard"].as_str(), Some("years on"));
        assert_eq!(e["mode"].as_str(), Some("homophone"));
        assert_eq!(e["source"].as_str(), Some("probe"));
        assert_eq!(e["count"].as_integer(), Some(4));
        assert_eq!(e["trials"].as_integer(), Some(16));
        let risk = e["risk"].as_float().unwrap();
        assert!((risk - f64::from(entries[0].risk)).abs() < 0.006, "{risk}");
        assert_eq!(
            e["risk_reason"].as_str(),
            Some(entries[0].risk_reason.as_str())
        );
        assert_eq!(e["status"].as_str(), Some(entries[0].status.as_str()));
    }

    #[test]
    fn render_quotes_awkward_text() {
        let tally = Tally {
            target: "action:quote".into(),
            meant: "say \"hi\"".into(),
            trials: 2,
            errors: 0,
            hearings: BTreeMap::from([("say hi there".into(), 2)]),
        };
        let entries = propose(std::slice::from_ref(&tally), &[], 0.1);
        let text = render(
            &Meta {
                model: "m".into(),
                domain: "commands".into(),
                generated: "2026-11-13".into(),
                generator: "ears cs build".into(),
            },
            &[],
            &[tally],
            &entries,
        );
        let value: toml::Value = toml::from_str(&text).unwrap();
        assert_eq!(value["entry"][0]["meant"].as_str(), Some("say \"hi\""));
    }

    #[test]
    fn collects_canonical_phrases_from_config_and_actions() {
        let commands: VoiceCommands = toml::from_str(
            r#"
            wake = ["hey clanker"]
            [keys]
            "clear all" = "ctrl+u*40"
            [run.centurion]
            phrases = ["centurion", "centurian"]
            command = ["true"]
            "#,
        )
        .unwrap();
        let (_, actions) = parse_actions(DEFAULT_ACTIONS).unwrap();
        let p = collect_phrases(&commands, &actions);
        let find = |t: &str| {
            p.canonical
                .iter()
                .find(|c| c.target == t)
                .map(|c| c.meant.as_str())
        };
        assert_eq!(find("wake"), Some("ears on"));
        assert_eq!(find("standby"), Some("ears vad"));
        assert_eq!(find("run:centurion"), Some("centurion"));
        assert_eq!(find("run:friend-wake"), Some("hey friend"));
        assert_eq!(find("action:close"), Some("close"));
        assert_eq!(find("action:leave"), Some("command"));
        assert_eq!(find("action:none"), None);
        assert!(p.known.contains(&known("wake", "hey clanker")));
        assert!(p.known.contains(&known("key:clear all", "clear all")));

        let (picked, _) =
            select(&p, &["wake".into(), "run:*".into(), "wake".into()], false).unwrap();
        assert_eq!(picked[0].target, "wake");
        assert!(picked.iter().skip(1).all(|c| c.target.starts_with("run:")));
        assert!(select(&p, &["nope".into()], false).is_err());
        let (some, missing) = select(&p, &["wake".into(), "nope".into()], true).unwrap();
        assert_eq!((some.len(), missing), (1, vec!["nope".to_string()]));
        assert_eq!(select(&p, &["lists".into()], false).unwrap().0.len(), 10);
    }

    #[test]
    fn noise_parses_and_mixes_at_the_requested_snr() {
        assert_eq!(Noise::parse("clean").unwrap(), Noise::Clean);
        assert_eq!(Noise::parse("pink:10").unwrap(), Noise::Pink(10.0));
        assert!(Noise::parse("brown:3").is_err());
        let speech: Vec<f32> = (0..16_000)
            .map(|i| (i as f32 * 440.0 * std::f32::consts::TAU / 16_000.0).sin() * 0.3)
            .collect();
        let clean = mix(&speech, Noise::Clean, 1);
        assert_eq!(clean.len(), PAD_BEFORE + speech.len() + PAD_AFTER);
        assert!(
            (pcm::dbfs(pcm::speech_rms(&clean[PAD_BEFORE..PAD_BEFORE + 16_000])) - SPEECH_DBFS)
                .abs()
                < 0.5
        );
        for noise in [Noise::White(10.0), Noise::Pink(10.0)] {
            let noisy = mix(&speech, noise, 7);
            let n: Vec<f32> = noisy.iter().zip(&clean).map(|(a, b)| a - b).collect();
            let snr = pcm::dbfs(pcm::speech_rms(&clean)) - pcm::dbfs(pcm::rms(&n));
            assert!((snr - 10.0).abs() < 0.5, "{noise:?}: {snr}");
            assert_eq!(noisy, mix(&speech, noise, 7), "seeded");
        }
    }

    #[test]
    fn resamples_and_reads_wav() {
        let tone: Vec<f32> = (0..24_000)
            .map(|i| (i as f32 * 300.0 * std::f32::consts::TAU / 24_000.0).sin() * 0.5)
            .collect();
        let pcm16: Vec<u8> = tone
            .iter()
            .flat_map(|&s| pcm::to_i16(s).to_le_bytes())
            .collect();
        let wav = pcm::wav_from_bytes(&pcm16, 24_000);
        let (samples, rate) = parse_wav(&wav).unwrap();
        assert_eq!(rate, 24_000);
        let out = resample(&samples, rate, 16_000);
        assert_eq!(out.len(), 16_000);
        assert!((pcm::rms(&out[1000..15_000]) - pcm::rms(&tone)).abs() < 0.01);
        assert!(parse_wav(b"nope").is_err());

        // 32-bit float stereo: channels are averaged.
        let mut float = pcm::wav_header(16, 8_000).to_vec();
        float[20..22].copy_from_slice(&3u16.to_le_bytes());
        float[22..24].copy_from_slice(&2u16.to_le_bytes());
        float[34..36].copy_from_slice(&32u16.to_le_bytes());
        for s in [0.5f32, 0.25, -1.0, 0.0] {
            float.extend_from_slice(&s.to_le_bytes());
        }
        assert_eq!(parse_wav(&float).unwrap(), (vec![0.375, -0.5], 8_000));
    }

    #[test]
    fn model_slug_and_dates() {
        assert_eq!(model_key("Qwen/Qwen3-ASR-1.7B"), "qwen3-asr-1.7b");
        assert_eq!(model_key(" whisper-Large-V3 "), "whisper-large-v3");
        assert_eq!(civil_date(0), "1970-01-01");
        assert_eq!(civil_date(20_770), "2026-11-13");
    }

    fn tts_wav() -> Vec<u8> {
        let pcm16: Vec<u8> = (0..12_000)
            .flat_map(|i| {
                let s = (i as f32 * 200.0 * std::f32::consts::TAU / 24_000.0).sin() * 0.4;
                pcm::to_i16(s).to_le_bytes()
            })
            .collect();
        pcm::wav_from_bytes(&pcm16, 24_000)
    }

    #[tokio::test]
    async fn probe_runs_against_mock_tts_and_asr_and_caches() {
        let tts_server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/v1/audio/speech"))
            .respond_with(ResponseTemplate::new(200).set_body_bytes(tts_wav()))
            .expect(2)
            .mount(&tts_server)
            .await;
        let asr_server = MockServer::start().await;
        let calls = Arc::new(AtomicUsize::new(0));
        let c = calls.clone();
        Mock::given(method("POST"))
            .and(path("/v1/audio/transcriptions"))
            .respond_with(move |_: &Request| {
                let n = c.fetch_add(1, Ordering::SeqCst);
                let text = if n % 4 == 3 { "Years on." } else { "Ears on." };
                ResponseTemplate::new(200).set_body_json(serde_json::json!({ "text": text }))
            })
            .expect(4)
            .mount(&asr_server)
            .await;
        Mock::given(method("GET"))
            .and(path("/v1/models"))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_body_json(serde_json::json!({"data": [{"id": "Org/Mock-ASR"}]})),
            )
            .mount(&asr_server)
            .await;

        let model = server_model(&asr_server.uri(), None).await.unwrap();
        assert_eq!(model, "Org/Mock-ASR");
        let cache = tempfile::tempdir().unwrap();
        let run = Run {
            tts: Tts::http(&tts_server.uri(), Some("mock-tts".into())),
            asr: Asr {
                client: WhisperClient::with_retry_config(asr_server.uri(), 0, 1, 1)
                    .with_silence_filter(false),
                model,
                language: Some("en".into()),
                prompt: None,
            },
            cache: cache.path().to_path_buf(),
            concurrency: 1,
            max_requests: 10,
        };
        let plan = Plan::new(
            vec![
                canonical("wake", "ears on"),
                canonical("run:regular", "Ears on"),
            ],
            &["a".into(), "b".into()],
            &[1.0],
            &[Noise::Clean, Noise::Pink(10.0)],
        );
        assert_eq!(plan.trials(), 4);
        assert_eq!(run.requests_needed(&plan), (2, 4));
        let results = run.execute(&plan).await.unwrap();
        let tallies = aggregate(&plan.phrases, &results);
        assert_eq!(tallies[0].trials, 4);
        assert_eq!(tallies[0].hits(), 3);
        assert_eq!(tallies[0].hearings.get("years on"), Some(&1));
        assert_eq!(tallies[1].hearings, tallies[0].hearings, "alias heard once");

        // Everything is cached now: a re-run sends nothing (the mocks'
        // `expect` counts are checked when they drop).
        assert_eq!(run.requests_needed(&plan), (0, 0));
        let again = run.execute(&plan).await.unwrap();
        assert_eq!(aggregate(&plan.phrases, &again), tallies);
        assert_eq!(calls.load(Ordering::SeqCst), 4);
    }

    #[test]
    fn the_same_words_for_two_targets_are_probed_once() {
        let plan = Plan::new(
            vec![
                canonical("enter", "enter"),
                canonical("action:tab", "tab"),
                canonical("action:enter", "Enter."),
            ],
            &["a".into(), "b".into()],
            &[1.0],
            &[Noise::Clean],
        );
        assert_eq!(plan.syntheses.len(), 4);
        assert_eq!(plan.aliases, vec![(2, 0)]);
        assert_eq!(plan.trials(), 4);
    }

    #[tokio::test]
    async fn probe_refuses_a_plan_over_the_cap() {
        let cache = tempfile::tempdir().unwrap();
        let run = Run {
            tts: Tts::http("http://127.0.0.1:9", None),
            asr: Asr {
                client: WhisperClient::new("http://127.0.0.1:9"),
                model: "m".into(),
                language: None,
                prompt: None,
            },
            cache: cache.path().to_path_buf(),
            concurrency: 2,
            max_requests: 3,
        };
        let plan = Plan::new(
            vec![canonical("wake", "ears on"), canonical("sleep", "ears off")],
            &["a".into()],
            &[1.0],
            &[Noise::Clean, Noise::White(5.0)],
        );
        let err = run.execute(&plan).await.unwrap_err().to_string();
        assert!(err.contains("4 ASR"), "{err}");
    }

    #[tokio::test]
    async fn failed_requests_count_as_errors_not_trials() {
        let tts_server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/v1/audio/speech"))
            .respond_with(ResponseTemplate::new(500))
            .mount(&tts_server)
            .await;
        let cache = tempfile::tempdir().unwrap();
        let run = Run {
            tts: Tts::http(&tts_server.uri(), None),
            asr: Asr {
                client: WhisperClient::new("http://127.0.0.1:9"),
                model: "m".into(),
                language: None,
                prompt: None,
            },
            cache: cache.path().to_path_buf(),
            concurrency: 2,
            max_requests: 10,
        };
        let plan = Plan::new(
            vec![canonical("wake", "ears on")],
            &["a".into()],
            &[1.0],
            &[Noise::Clean],
        );
        let results = run.execute(&plan).await.unwrap();
        let t = aggregate(&plan.phrases, &results);
        assert_eq!((t[0].trials, t[0].errors), (0, 1));
        assert!(propose(&t, &[], 0.1).is_empty());
    }
}
