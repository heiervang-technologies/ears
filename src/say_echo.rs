//! Agents' own speech coming back in through the mic.
//!
//! Agents talk with `say`, and every line they say is logged with its text
//! (`/tmp/live-subs-say.jsonl`, written for the live subtitles). Echo
//! cancellation (PipeWire `echo-cancel`) removes most of it acoustically;
//! this is the net behind it. A finished utterance is compared, as words,
//! with the lines said in the last `window_s` seconds:
//!
//! - runs of `LONG_RUN` or more words that a line also has are echo, and
//!   are cut out, as are runs of `MIN_RUN` while that line still plays;
//!   a single differing word between two runs is an ASR slip and goes with
//!   them;
//! - an utterance shorter than that is echo only if a line containing it is
//!   still playing (from its log time to its estimated end, plus slack);
//! - when (almost) nothing but echo is left, the whole utterance goes.
//!
//! Matching text instead of timing audio makes the delay irrelevant: a
//! Bluetooth speaker or headset 150 to 400 ms behind, or a line that sat in
//! the speaker queue for a while, matches all the same.

use serde::Deserialize;
use std::io::{Read, Seek, SeekFrom};
use std::path::Path;

/// `[say_echo]` in the ears config.
#[derive(Debug, Clone, Deserialize, serde::Serialize, PartialEq)]
pub struct SayEchoConfig {
    /// Drop agents' speech that the mic picked up. Default: true.
    #[serde(default = "default_enabled")]
    pub enabled: bool,
    /// The say log (JSON lines with `text` and `time`, Unix seconds).
    #[serde(default = "default_log")]
    pub log: String,
    /// How far back a said line can still come in, in seconds. Default: 120.
    #[serde(default = "default_window_s")]
    pub window_s: f64,
}

fn default_enabled() -> bool {
    true
}

fn default_log() -> String {
    "/tmp/live-subs-say.jsonl".to_string()
}

fn default_window_s() -> f64 {
    120.0
}

impl Default for SayEchoConfig {
    fn default() -> Self {
        Self {
            enabled: default_enabled(),
            log: default_log(),
            window_s: default_window_s(),
        }
    }
}

/// Shortest run of shared words that counts as echo while its line plays.
const MIN_RUN: usize = 3;
/// Shortest run that counts as echo any time in the window.
const LONG_RUN: usize = 5;
/// Speaking rate used to estimate when a line ends when the log has no
/// duration: TTS runs at about 2.5 words a second.
const SECONDS_PER_WORD: f64 = 0.4;
/// After a line's estimated end, it can still be heard for this long
/// (speaker latency, queueing, the VAD's hangover).
const PLAYING_SLACK_S: f64 = 4.0;
/// Only the tail of the log is read; it rotates at 256 KB.
const TAIL_BYTES: u64 = 64 * 1024;

/// A line an agent said.
#[derive(Debug, Clone, PartialEq)]
pub struct Said {
    pub text: String,
    /// When it was logged (Unix seconds).
    pub time: f64,
    /// Playback length, when the log knows it.
    pub duration: Option<f64>,
}

impl Said {
    fn playing_at(&self, now: f64, words: usize) -> bool {
        let length = self.duration.unwrap_or(words as f64 * SECONDS_PER_WORD);
        now >= self.time - 1.0 && now <= self.time + length + PLAYING_SLACK_S
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum Echo {
    /// Nothing heard came from a said line.
    None,
    /// All of it did.
    All,
    /// Some of it did; this is what is left.
    Part(String),
}

/// One log line. `time` is a number or, from some writers, a string;
/// the playback length comes as `audio_ms` (local playback) if at all.
fn entry(line: &str) -> Option<Said> {
    let v: serde_json::Value = serde_json::from_str(line).ok()?;
    let number = |key: &str| match &v[key] {
        serde_json::Value::Number(n) => n.as_f64(),
        serde_json::Value::String(s) => s.parse().ok(),
        _ => None,
    };
    Some(Said {
        text: v["text"].as_str()?.to_string(),
        time: number("time")?,
        duration: number("audio_ms").map(|ms| ms / 1000.0),
    })
}

/// Lines said since `since` (Unix seconds), oldest first.
pub fn recent(log: &Path, since: f64) -> Vec<Said> {
    let Ok(mut file) = std::fs::File::open(log) else {
        return Vec::new();
    };
    let len = file.metadata().map(|m| m.len()).unwrap_or(0);
    if len > TAIL_BYTES && file.seek(SeekFrom::Start(len - TAIL_BYTES)).is_err() {
        return Vec::new();
    }
    let mut tail = Vec::new();
    if file.read_to_end(&mut tail).is_err() {
        return Vec::new();
    }
    String::from_utf8_lossy(&tail)
        .lines()
        .filter_map(entry)
        .filter(|e| e.time >= since)
        .collect()
}

pub fn now() -> f64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs_f64())
        .unwrap_or(0.0)
}

/// A word as compared: lowercase letters and digits only.
fn norm(word: &str) -> String {
    word.chars()
        .filter(|c| c.is_alphanumeric())
        .flat_map(char::to_lowercase)
        .collect()
}

/// The same word, or one the ASR got a letter or two wrong ("years" for
/// "ears", "centurian" for "centurion"). Short words must match exactly.
fn alike(a: &str, b: &str) -> bool {
    if a == b {
        return true;
    }
    let (a, b): (Vec<char>, Vec<char>) = (a.chars().collect(), b.chars().collect());
    let shorter = a.len().min(b.len());
    shorter >= 4 && edit_distance(&a, &b) <= shorter / 4
}

fn edit_distance(a: &[char], b: &[char]) -> usize {
    let mut row: Vec<usize> = (0..=b.len()).collect();
    for (i, ca) in a.iter().enumerate() {
        let mut diagonal = row[0];
        row[0] = i + 1;
        for (j, cb) in b.iter().enumerate() {
            let above = row[j + 1];
            row[j + 1] = (diagonal + usize::from(ca != cb))
                .min(above + 1)
                .min(row[j] + 1);
            diagonal = above;
        }
    }
    row[b.len()]
}

fn words(text: &str) -> Vec<String> {
    text.split_whitespace()
        .map(norm)
        .filter(|w| !w.is_empty())
        .collect()
}

/// How much of `heard` came from `said` lines, as of `now`.
pub fn check(heard: &str, said: &[Said], now: f64) -> Echo {
    // Original tokens (kept as typed) and their compared forms.
    let tokens: Vec<&str> = heard
        .split_whitespace()
        .filter(|t| !norm(t).is_empty())
        .collect();
    let h: Vec<String> = tokens.iter().map(|t| norm(t)).collect();
    if h.is_empty() || said.is_empty() {
        return Echo::None;
    }
    let mut echo = vec![false; h.len()];
    for line in said {
        let s = words(&line.text);
        if s.is_empty() {
            continue;
        }
        if h.len() < MIN_RUN {
            if line.playing_at(now, s.len())
                && s.windows(h.len())
                    .any(|w| w.iter().zip(&h).all(|(a, b)| alike(a, b)))
            {
                echo.iter_mut().for_each(|e| *e = true);
            }
            continue;
        }
        // run[i][j]: shared words starting at heard i and said j.
        let mut run = vec![vec![0usize; s.len() + 1]; h.len() + 1];
        for i in (0..h.len()).rev() {
            for j in (0..s.len()).rev() {
                if alike(&h[i], &s[j]) {
                    run[i][j] = run[i + 1][j + 1] + 1;
                }
            }
        }
        // A few common words ("on the phone") are echo only while the
        // line plays; a longer run is echo whenever it comes in.
        let playing = line.playing_at(now, s.len());
        for i in 0..h.len() {
            let longest = run[i].iter().copied().max().unwrap_or(0);
            if longest >= LONG_RUN || (longest >= MIN_RUN && playing) {
                echo[i..i + longest].iter_mut().for_each(|e| *e = true);
            }
        }
    }
    // One odd word between echo is the ASR mishearing the speaker.
    for i in 1..echo.len().saturating_sub(1) {
        if !echo[i] && echo[i - 1] && echo[i + 1] {
            echo[i] = true;
        }
    }
    let echoed = echo.iter().filter(|e| **e).count();
    if echoed == 0 {
        return Echo::None;
    }
    let kept: Vec<&str> = tokens
        .iter()
        .zip(&echo)
        .filter(|(_, e)| !**e)
        .map(|(t, _)| *t)
        .collect();
    // A word or two left around a long echo is more echo, misheard.
    if kept.is_empty() || (kept.len() < 3 && kept.len() * 4 < tokens.len()) {
        return Echo::All;
    }
    Echo::Part(kept.join(" "))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn said(text: &str, time: f64) -> Said {
        Said {
            text: text.into(),
            time,
            duration: None,
        }
    }

    const LINE: &str =
        "Done. Ears sound effects are now at half volume on both centurion and hackbook.";

    #[test]
    fn whole_echo_is_dropped() {
        let lines = [said(LINE, 100.0)];
        assert_eq!(
            check("ears sound effects are now at half volume", &lines, 105.0),
            Echo::All
        );
    }

    #[test]
    fn misheard_echo_is_dropped() {
        let lines = [said(LINE, 100.0)];
        // "ears" heard as "years", "centurion" as "centurian".
        assert_eq!(
            check(
                "years sound effects are now at half volume on both centurian and hackbook",
                &lines,
                105.0
            ),
            Echo::All
        );
    }

    #[test]
    fn own_words_over_echo_are_kept() {
        let lines = [said(LINE, 100.0)];
        assert_eq!(
            check(
                "sound effects are now at half volume please open the browser for me",
                &lines,
                104.0
            ),
            Echo::Part("please open the browser for me".into())
        );
    }

    #[test]
    fn unrelated_speech_is_kept() {
        let lines = [said(LINE, 100.0)];
        assert_eq!(check("set the volume to thirty", &lines, 110.0), Echo::None);
        // Two shared words are not a run.
        assert_eq!(
            check("half volume is fine thanks", &lines, 110.0),
            Echo::None
        );
    }

    #[test]
    fn short_echo_only_while_playing() {
        let lines = [said("Should I merge it? Yes or no.", 100.0)];
        // Still playing (7 words, about 3 s, plus slack).
        assert_eq!(check("yes or no", &lines, 103.0), Echo::All);
        assert_eq!(check("Yes.", &lines, 103.0), Echo::All);
        // Long over: the user answering.
        assert_eq!(check("Yes.", &lines, 112.0), Echo::None);
    }

    #[test]
    fn common_phrase_after_the_line_is_kept() {
        let lines = [said(
            "I'll ring you now as a test. Answer on the phone.",
            100.0,
        )];
        assert_eq!(
            check("test the clanker call on the phone", &lines, 140.0),
            Echo::None
        );
    }

    #[test]
    fn delay_does_not_matter_for_long_runs() {
        let lines = [said(LINE, 100.0)];
        assert_eq!(
            check("ears sound effects are now at half volume", &lines, 190.0),
            Echo::All
        );
    }

    #[test]
    fn reads_the_log_tail() {
        let dir = tempfile::tempdir().unwrap();
        let log = dir.path().join("say.jsonl");
        std::fs::write(
            &log,
            concat!(
                r#"{"text":"old","persona":"ears","time":10.0}"#,
                "\n",
                "not json\n",
                r#"{"text":"new line","persona":"ears","time":200.5,"audio_ms":2000}"#,
                "\n",
                r#"{"text": "as text", "persona": "haios", "time": "300.25", "auto": true}"#,
                "\n"
            ),
        )
        .unwrap();
        assert_eq!(
            recent(&log, 100.0),
            vec![
                Said {
                    text: "new line".into(),
                    time: 200.5,
                    duration: Some(2.0)
                },
                Said {
                    text: "as text".into(),
                    time: 300.25,
                    duration: None
                }
            ]
        );
        assert!(recent(&dir.path().join("missing"), 0.0).is_empty());
    }
}
