//! Agents' own speech coming back in through the mic.
//!
//! Agents talk with `say`, and every line they say is logged with its text
//! (`/tmp/live-subs-say.jsonl`, written for the live subtitles). Echo
//! cancellation (PipeWire `echo-cancel`) removes most of it acoustically;
//! this is the net behind it. A finished utterance is compared, as words,
//! with the lines said in the last `window_s` seconds:
//!
//! - runs of `LONG_RUN` or more words that a line also has are echo, and
//!   are cut out, as are runs of `MIN_RUN` if an agent was playing when the
//!   utterance began; a single differing word between two runs is an ASR
//!   slip and goes with them;
//! - an utterance shorter than that is echo only if an agent was playing
//!   when it began, and a recent line contains it;
//! - when (almost) nothing but echo is left, the whole utterance goes.
//!
//! Matching text instead of timing audio makes the delay irrelevant: a
//! Bluetooth speaker or headset 150 to 400 ms behind, or a line that sat in
//! the speaker queue for a while, matches all the same.
//!
//! Whether a line is playing comes from say's talking flags
//! (`talking/<agent>.json`, there while its audio plays), checked when the
//! utterance begins: an echo begins while the line plays, while an answer
//! to it ("pick up", read out a moment ago) begins after. A flag vouches
//! only for the newest line of its own agent (the log's `persona` or
//! `name`), never for every line in the window: with many agents talking,
//! some flag is nearly always up. With flags to go by, a long run also
//! counts only while its line plays or shortly after
//! ([`LONG_RUN_AFTER_S`]): a line that waits in the speaker queue keeps
//! its flag up, so the time after that is someone quoting the agent.

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
    /// say's talking flags: `<agent>.json` with `{started, pid}` while its
    /// audio plays. Empty: `~/.local/share/agent-tools/talking`.
    #[serde(default)]
    pub talking_dir: String,
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
            talking_dir: String::new(),
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
/// A talking flag older than this is stale, whatever its pid (say's
/// contract with its consumers).
const FLAG_MAX_AGE_S: f64 = 120.0;
/// With talking flags known, how long after its line has ended a long run
/// still counts as echo (speaker latency, a slow VAD end).
const LONG_RUN_AFTER_S: f64 = 30.0;
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
    /// Who said it, as say names its talking flag: the persona and the
    /// director name, restricted to `[A-Za-z0-9._-]`.
    pub who: Vec<String>,
}

impl Said {
    fn end(&self, words: usize) -> f64 {
        self.time + self.duration.unwrap_or(words as f64 * SECONDS_PER_WORD)
    }

    fn playing_at(&self, now: f64, words: usize) -> bool {
        now >= self.time - 1.0 && now <= self.end(words) + PLAYING_SLACK_S
    }
}

/// A name as say writes it into a flag's file name.
fn flag_name(name: &str) -> String {
    name.chars()
        .filter(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-'))
        .collect()
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

impl SayEchoConfig {
    fn talking_dir(&self) -> Option<std::path::PathBuf> {
        if !self.talking_dir.is_empty() {
            return Some(self.talking_dir.clone().into());
        }
        std::env::var_os("HOME")
            .map(|h| std::path::PathBuf::from(h).join(".local/share/agent-tools/talking"))
    }

    /// The agents whose line is playing right now (their talking flags).
    /// None when there are no flags to tell (no say here): then each line's
    /// estimated playing time is used.
    pub fn playing(&self) -> Option<Vec<String>> {
        let dir = self.talking_dir()?;
        let entries = std::fs::read_dir(dir).ok()?;
        let now = std::time::SystemTime::now();
        Some(
            entries
                .flatten()
                .filter(|e| {
                    let path = e.path();
                    if path.extension().is_none_or(|x| x != "json") {
                        return false; // also skips say's `.json.tmp.PID` leftovers
                    }
                    // Cheap first: most flags are long gone.
                    let fresh = e
                        .metadata()
                        .and_then(|m| m.modified())
                        .ok()
                        .and_then(|t| now.duration_since(t).ok())
                        .is_some_and(|age| age.as_secs_f64() <= FLAG_MAX_AGE_S);
                    fresh
                        && std::fs::read_to_string(&path)
                            .ok()
                            .and_then(|s| serde_json::from_str::<serde_json::Value>(&s).ok())
                            .and_then(|v| v["pid"].as_i64())
                            .is_some_and(|pid| pid > 0 && alive(pid as i32))
                })
                .filter_map(|e| {
                    e.path()
                        .file_stem()
                        .map(|n| n.to_string_lossy().into_owned())
                })
                .collect(),
        )
    }
}

fn alive(pid: i32) -> bool {
    // SAFETY: signal 0 only checks that the process exists.
    unsafe { libc::kill(pid, 0) == 0 }
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
    let who = ["persona", "name"]
        .iter()
        .filter_map(|k| v[*k].as_str())
        .map(flag_name)
        .filter(|n| !n.is_empty())
        .collect();
    Some(Said {
        text: v["text"].as_str()?.to_string(),
        time: number("time")?,
        duration: number("audio_ms").map(|ms| ms / 1000.0),
        who,
    })
}

/// Lines said since `since` (Unix seconds), oldest first.
/// say rotates the log to `<log>.old`; just after that the new one is
/// short, so the old one's tail is read too.
pub fn recent(log: &Path, since: f64) -> Vec<Said> {
    let mut text = Vec::new();
    let len = std::fs::metadata(log).map(|m| m.len()).unwrap_or(0);
    if len < TAIL_BYTES {
        let mut old = log.as_os_str().to_owned();
        old.push(".old");
        text = tail(Path::new(&old), TAIL_BYTES - len);
        text.push(b'\n');
    }
    text.extend(tail(log, TAIL_BYTES));
    String::from_utf8_lossy(&text)
        .lines()
        .filter_map(entry)
        .filter(|e| e.time >= since)
        .collect()
}

/// The last `bytes` of a file (empty if it cannot be read).
fn tail(path: &Path, bytes: u64) -> Vec<u8> {
    let Ok(mut file) = std::fs::File::open(path) else {
        return Vec::new();
    };
    let len = file.metadata().map(|m| m.len()).unwrap_or(0);
    if len > bytes && file.seek(SeekFrom::Start(len - bytes)).is_err() {
        return Vec::new();
    }
    let mut out = Vec::new();
    if file.read_to_end(&mut out).is_err() {
        return Vec::new();
    }
    out
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

/// How much of `heard` came from `said` lines (oldest first), as of `now`.
/// `playing`: the agents whose flag was up when the utterance began (None:
/// no flags to tell, estimate from the log's times).
pub fn check(heard: &str, said: &[Said], now: f64, playing: Option<&[String]>) -> Echo {
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
    for (k, line) in said.iter().enumerate() {
        let s = words(&line.text);
        if s.is_empty() {
            continue;
        }
        // Is this line the one playing? A flag speaks for its agent's newest
        // line only; a line without a speaker goes by its estimated time.
        let flagged = playing.map(|names| {
            let up = |l: &Said| l.who.iter().any(|w| names.contains(w));
            if line.who.is_empty() {
                !names.is_empty() && line.playing_at(now, s.len())
            } else {
                up(line)
                    && !said[k + 1..]
                        .iter()
                        .any(|later| later.who.iter().any(|w| line.who.contains(w)))
            }
        });
        let playing = flagged.unwrap_or_else(|| line.playing_at(now, s.len()));
        // With flags, a long run is echo while its line plays and a little
        // after; without, any time in the window (it may have been queued).
        let long_ok = flagged.is_none() || playing || now <= line.end(s.len()) + LONG_RUN_AFTER_S;
        if h.len() < MIN_RUN {
            // Few words: only the very words, never "your hour" for "four hours".
            if playing && s.windows(h.len()).any(|w| w == &h[..]) {
                echo.iter_mut().for_each(|e| *e = true);
            }
            continue;
        }
        // run[i][j]: shared words starting at heard i and said j; exact[i][j]
        // the same without allowing for ASR slips.
        let mut run = vec![vec![0usize; s.len() + 1]; h.len() + 1];
        let mut exact = vec![vec![0usize; s.len() + 1]; h.len() + 1];
        for i in (0..h.len()).rev() {
            for j in (0..s.len()).rev() {
                if alike(&h[i], &s[j]) {
                    run[i][j] = run[i + 1][j + 1] + 1;
                }
                if h[i] == s[j] {
                    exact[i][j] = exact[i + 1][j + 1] + 1;
                }
            }
        }
        // A few common words ("on the phone") are echo only while their line
        // plays, and only said exactly; a longer run may be misheard.
        for i in 0..h.len() {
            let longest = run[i].iter().copied().max().unwrap_or(0);
            let short = exact[i].iter().copied().max().unwrap_or(0);
            if longest >= LONG_RUN && long_ok {
                echo[i..i + longest].iter_mut().for_each(|e| *e = true);
            } else if short >= MIN_RUN && playing {
                echo[i..i + short].iter_mut().for_each(|e| *e = true);
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
            who: Vec::new(),
        }
    }

    const LINE: &str =
        "Done. Ears sound effects are now at half volume on both centurion and hackbook.";

    #[test]
    fn whole_echo_is_dropped() {
        let lines = [said(LINE, 100.0)];
        assert_eq!(
            check(
                "ears sound effects are now at half volume",
                &lines,
                105.0,
                None
            ),
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
                105.0,
                None
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
                104.0,
                None
            ),
            Echo::Part("please open the browser for me".into())
        );
    }

    #[test]
    fn unrelated_speech_is_kept() {
        let lines = [said(LINE, 100.0)];
        assert_eq!(
            check("set the volume to thirty", &lines, 110.0, None),
            Echo::None
        );
        // Two shared words are not a run.
        assert_eq!(
            check("half volume is fine thanks", &lines, 110.0, None),
            Echo::None
        );
    }

    #[test]
    fn short_echo_only_while_playing() {
        let lines = [said("Should I merge it? Yes or no.", 100.0)];
        // Still playing (7 words, about 3 s, plus slack).
        assert_eq!(check("yes or no", &lines, 103.0, None), Echo::All);
        assert_eq!(check("Yes.", &lines, 103.0, None), Echo::All);
        // Long over: the user answering.
        assert_eq!(check("Yes.", &lines, 112.0, None), Echo::None);
    }

    #[test]
    fn an_answer_after_the_line_is_not_echo() {
        let lines = [said("Say answer, pick up or decline.", 100.0)];
        let none: &[String] = &[];
        let up = ["osiris".to_string()];
        // Begun after the agent stopped: the user answering.
        assert_eq!(check("Pick up.", &lines, 103.0, Some(none)), Echo::None);
        // Begun while it played: the speaker.
        assert_eq!(check("Pick up.", &lines, 103.0, Some(&up)), Echo::All);
        // A long run is echo either way.
        assert_eq!(
            check("say answer pick up or decline", &lines, 103.0, Some(none)),
            Echo::All
        );
    }

    #[test]
    fn talking_flags_need_a_live_pid() {
        let dir = tempfile::tempdir().unwrap();
        let config = SayEchoConfig {
            talking_dir: dir.path().to_string_lossy().into(),
            ..Default::default()
        };
        assert_eq!(config.playing(), Some(vec![]));
        std::fs::write(
            dir.path().join("gone.json"),
            r#"{"started":1,"pid":2147483000}"#,
        )
        .unwrap();
        std::fs::write(dir.path().join("x.json.tmp.5"), "{}").unwrap();
        assert_eq!(config.playing(), Some(vec![]));
        let me = format!(r#"{{"started":1,"pid":{}}}"#, std::process::id());
        std::fs::write(dir.path().join("moon.json"), me).unwrap();
        assert_eq!(config.playing(), Some(vec!["moon".to_string()]));
        let missing = SayEchoConfig {
            talking_dir: dir.path().join("nope").to_string_lossy().into(),
            ..Default::default()
        };
        assert_eq!(missing.playing(), None);
    }

    fn by(text: &str, time: f64, who: &str) -> Said {
        Said {
            who: vec![who.to_string()],
            ..said(text, time)
        }
    }

    #[test]
    fn another_agents_flag_does_not_make_old_lines_play() {
        // osiris spoke 100 s ago; big-dog is talking now.
        let lines = [
            by(
                "Yes, I merged it. Should I stop the build?",
                100.0,
                "osiris",
            ),
            by("Deploy is rolling out on rogue.", 195.0, "big-dog"),
        ];
        let up = ["big-dog".to_string()];
        assert_eq!(check("yes", &lines, 200.0, Some(&up)), Echo::None);
        assert_eq!(check("stop", &lines, 200.0, Some(&up)), Echo::None);
        assert_eq!(
            check("okay I merged it now go", &lines, 200.0, Some(&up)),
            Echo::None
        );
        // The line that does play is still caught.
        assert_eq!(
            check("rolling out on rogue", &lines, 200.0, Some(&up)),
            Echo::All
        );
    }

    #[test]
    fn a_flag_vouches_only_for_its_agents_newest_line() {
        let lines = [
            by("Should I stop the build?", 100.0, "osiris"),
            by("Build is green.", 190.0, "osiris"),
        ];
        let up = ["osiris".to_string()];
        assert_eq!(
            check("stop the build", &lines, 192.0, Some(&up)),
            Echo::None
        );
        assert_eq!(check("build is green", &lines, 192.0, Some(&up)), Echo::All);
    }

    #[test]
    fn short_echo_must_be_the_very_words() {
        let lines = [by("That took four hours.", 100.0, "osiris")];
        let up = ["osiris".to_string()];
        assert_eq!(check("your hour", &lines, 101.0, Some(&up)), Echo::None);
        assert_eq!(check("four hours", &lines, 101.0, Some(&up)), Echo::All);
    }

    #[test]
    fn quoting_an_agent_later_is_kept() {
        let lines = [by(
            "I think that is a good idea, let me start on it.",
            100.0,
            "osiris",
        )];
        let none: &[String] = &[];
        assert_eq!(
            check(
                "I think that is a good idea but wait until tomorrow",
                &lines,
                190.0,
                Some(none)
            ),
            Echo::None
        );
        // Right after the line, it is the speaker.
        assert!(matches!(
            check(
                "I think that is a good idea but wait until tomorrow",
                &lines,
                110.0,
                Some(none)
            ),
            Echo::Part(_)
        ));
    }

    #[test]
    fn reads_the_old_log_just_after_rotation() {
        let dir = tempfile::tempdir().unwrap();
        let log = dir.path().join("say.jsonl");
        std::fs::write(
            dir.path().join("say.jsonl.old"),
            "{\"text\":\"before the rotation\",\"time\":100,\"persona\":\"ears\"}\n",
        )
        .unwrap();
        std::fs::write(&log, "{\"text\":\"after\",\"time\":101}\n").unwrap();
        let got = recent(&log, 0.0);
        assert_eq!(got.len(), 2);
        assert_eq!(got[0].text, "before the rotation");
        assert_eq!(got[0].who, vec!["ears".to_string()]);
    }

    #[test]
    fn common_phrase_after_the_line_is_kept() {
        let lines = [said(
            "I'll ring you now as a test. Answer on the phone.",
            100.0,
        )];
        assert_eq!(
            check("test the clanker call on the phone", &lines, 140.0, None),
            Echo::None
        );
    }

    #[test]
    fn delay_does_not_matter_for_long_runs() {
        let lines = [said(LINE, 100.0)];
        assert_eq!(
            check(
                "ears sound effects are now at half volume",
                &lines,
                190.0,
                None
            ),
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
                    duration: Some(2.0),
                    who: vec!["ears".into()],
                },
                Said {
                    text: "as text".into(),
                    time: 300.25,
                    duration: None,
                    who: vec!["haios".into()],
                }
            ]
        );
        assert!(recent(&dir.path().join("missing"), 0.0).is_empty());
    }
}
