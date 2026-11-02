//! Decisions (the red mode): an agent asks a question with a few discrete
//! options (`ears decide`), ears turns red and shows them, and you read one
//! out. Nothing is typed while a decision is open: every utterance is
//! matched against the options, and the one you said goes back to the asker.
//!
//! The asker writes `decision.json` in the runtime dir and waits for
//! `decision-answer.json` with the same id. A decision whose asker died is
//! ignored, so a killed `ears decide` never leaves ears red.
//!
//! What was heard is matched by its words first and, when they are unclear
//! or ASR is down, by embeddinggemma2 (the audio against each option's
//! text). A decision may leave room for an answer of your own (`other`):
//! anything that is not an option is returned as said. Every outcome is
//! noted in `decision-last.json` for the overlay's feedback.

use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant, SystemTime};

/// The open decision.
pub const FILE: &str = "decision.json";
/// The answer to it.
pub const ANSWER: &str = "decision-answer.json";
/// The last thing heard for a decision, for the overlay.
pub const LAST: &str = "decision-last.json";

/// By embedding, the nearest option must lead the next by this much
/// (cosine): text against audio sits in a narrow band.
pub const EMBED_MARGIN: f32 = 0.04;

/// How often the engine looks for a decision (a stat per look).
const LOOK_EVERY: Duration = Duration::from_millis(200);

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Decision {
    pub id: String,
    pub question: String,
    pub options: Vec<String>,
    /// The asker; the decision lapses when it exits.
    pub pid: u32,
    /// A quick time event: opened at (unix ms), answer within (ms). The
    /// overlay drains a bar over this time.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub opened_ms: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub timeout_ms: Option<u64>,
    /// Room for an answer of your own: what is not an option is returned.
    #[serde(default)]
    pub other: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Answer {
    pub id: String,
    /// The option chosen; None when the decision was cancelled.
    pub choice: Option<String>,
    pub index: Option<usize>,
    /// An answer of your own (decisions with `other`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub custom: Option<String>,
}

/// What became of one utterance, for the overlay.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Feedback {
    pub id: String,
    /// "chose", "custom", "cancel" or "unclear".
    pub outcome: String,
    pub index: Option<usize>,
    pub heard: String,
    /// "words" or "embedding".
    pub via: String,
}

/// Pick an option by embedding scores: the best, if it leads clearly.
pub fn pick_by_scores(scores: &[f32]) -> Option<usize> {
    let mut order: Vec<usize> = (0..scores.len()).collect();
    order.sort_by(|&a, &b| scores[b].total_cmp(&scores[a]));
    match order[..] {
        [best] => Some(best),
        [best, next, ..] if scores[best] - scores[next] >= EMBED_MARGIN => Some(best),
        _ => None,
    }
}

/// What an utterance means for the open decision.
#[derive(Debug, Clone, PartialEq)]
pub enum Heard {
    Chose(usize),
    Cancel,
    /// Not one of the options (or more than one fits).
    Unclear,
}

const CANCEL: &[&str] = &["cancel", "none", "none of these", "none of them", "skip"];
const ORDINALS: &[&[&str]] = &[
    &[
        "1",
        "one",
        "first",
        "option one",
        "number one",
        "the first one",
    ],
    &[
        "2",
        "two",
        "second",
        "option two",
        "number two",
        "the second one",
    ],
    &[
        "3",
        "three",
        "third",
        "option three",
        "number three",
        "the third one",
    ],
    &[
        "4",
        "four",
        "fourth",
        "option four",
        "number four",
        "the fourth one",
    ],
    &[
        "5",
        "five",
        "fifth",
        "option five",
        "number five",
        "the fifth one",
    ],
    &[
        "6",
        "six",
        "sixth",
        "option six",
        "number six",
        "the sixth one",
    ],
];

impl Decision {
    /// Match what was said against the options: the option itself, its
    /// number, the option within a sentence, or enough of its start.
    pub fn hear(&self, transcript: &str) -> Heard {
        let said = crate::commands::normalize(transcript);
        if said.is_empty() {
            return Heard::Unclear;
        }
        let options: Vec<String> = self
            .options
            .iter()
            .map(|o| crate::commands::normalize(o))
            .collect();
        if let Some(i) = options.iter().position(|o| *o == said) {
            return Heard::Chose(i);
        }
        if CANCEL.contains(&said.as_str()) {
            return Heard::Cancel;
        }
        if let Some(i) = ORDINALS
            .iter()
            .take(options.len())
            .position(|words| words.contains(&said.as_str()))
        {
            return Heard::Chose(i);
        }
        let padded = format!(" {said} ");
        let within: Vec<usize> = (0..options.len())
            .filter(|&i| !options[i].is_empty() && padded.contains(&format!(" {} ", options[i])))
            .collect();
        if let [i] = within[..] {
            return Heard::Chose(i);
        }
        if within.len() > 1 {
            // "yes" inside "yes but later": an option said only as part of
            // a longer one that was said does not count. Two distinct ones
            // ("red or blue") are unclear.
            let contains = |big: &str, small: &str| {
                big != small && format!(" {big} ").contains(&format!(" {small} "))
            };
            let kept: Vec<usize> = within
                .iter()
                .copied()
                .filter(|&i| !within.iter().any(|&j| contains(&options[j], &options[i])))
                .collect();
            if let [i] = kept[..] {
                return Heard::Chose(i);
            }
            return Heard::Unclear;
        }
        // The start of one option only ("deploy" for "deploy now").
        let starts: Vec<usize> = (0..options.len())
            .filter(|&i| said.len() >= 3 && options[i].starts_with(&format!("{said} ")))
            .collect();
        if let [i] = starts[..] {
            return Heard::Chose(i);
        }
        Heard::Unclear
    }
}

/// Where decisions live, and the one open now (looked up at most every
/// [`LOOK_EVERY`]).
#[derive(Debug)]
pub struct Decisions {
    dir: PathBuf,
    open: Option<Decision>,
    stamp: Option<SystemTime>,
    looked: Option<Instant>,
}

impl Decisions {
    pub fn new(state_dir: &Path) -> Self {
        Self {
            dir: state_dir.to_path_buf(),
            open: None,
            stamp: None,
            looked: None,
        }
    }

    /// The open decision as last looked up (no lookup).
    pub fn current(&self) -> Option<&Decision> {
        self.open.as_ref()
    }

    /// The decision waiting for an answer, if any.
    pub fn open(&mut self) -> Option<&Decision> {
        if self.looked.is_none_or(|t| t.elapsed() >= LOOK_EVERY) {
            self.looked = Some(Instant::now());
            self.refresh();
        }
        self.open.as_ref()
    }

    fn refresh(&mut self) {
        let path = self.dir.join(FILE);
        let stamp = std::fs::metadata(&path).and_then(|m| m.modified()).ok();
        if stamp != self.stamp {
            self.stamp = stamp;
            self.open = stamp
                .and_then(|_| std::fs::read_to_string(&path).ok())
                .and_then(|s| serde_json::from_str::<Decision>(&s).ok())
                .filter(|d| !d.options.is_empty());
        }
        if self.open.as_ref().is_some_and(|d| !alive(d.pid)) {
            self.open = None;
        }
    }

    /// Note what became of an utterance (the overlay shows it).
    pub fn note(&self, feedback: &Feedback) {
        let tmp = self.dir.join(format!("{LAST}.tmp"));
        if std::fs::write(&tmp, serde_json::to_vec(feedback).unwrap_or_default()).is_ok() {
            let _ = std::fs::rename(&tmp, self.dir.join(LAST));
        }
    }

    /// Answer the open decision and close it: an option, an answer of
    /// your own, or neither (cancelled).
    pub fn answer(
        &mut self,
        choice: Option<usize>,
        custom: Option<String>,
    ) -> std::io::Result<Option<String>> {
        let Some(decision) = self.open.take() else {
            return Ok(None);
        };
        let chosen = choice.and_then(|i| decision.options.get(i).cloned());
        let answer = Answer {
            id: decision.id.clone(),
            choice: chosen.clone(),
            index: choice,
            custom,
        };
        let tmp = self.dir.join(format!("{ANSWER}.tmp"));
        std::fs::write(&tmp, serde_json::to_vec(&answer).unwrap_or_default())?;
        std::fs::rename(&tmp, self.dir.join(ANSWER))?;
        let _ = std::fs::remove_file(self.dir.join(FILE));
        self.stamp = None;
        Ok(chosen)
    }
}

fn alive(pid: u32) -> bool {
    // SAFETY: signal 0 only checks that the process exists.
    pid > 0 && unsafe { libc::kill(pid as i32, 0) == 0 }
}

/// The asking side (`ears decide`): open a decision, wait for the answer.
/// Waits for an earlier open decision to close first. None on timeout.
pub async fn ask(
    state_dir: &Path,
    question: &str,
    options: &[String],
    other: bool,
    timeout: Option<Duration>,
) -> std::io::Result<Option<Answer>> {
    let started = Instant::now();
    let expired = || timeout.is_some_and(|t| started.elapsed() >= t);
    let file = state_dir.join(FILE);
    let answer_file = state_dir.join(ANSWER);
    // One decision at a time: a live asker's stays until it is answered.
    let mut waiting = Decisions::new(state_dir);
    while waiting.open().is_some() {
        if expired() {
            return Ok(None);
        }
        tokio::time::sleep(LOOK_EVERY).await;
        waiting.looked = None;
    }
    let id = format!(
        "{}-{}",
        std::process::id(),
        SystemTime::now()
            .duration_since(SystemTime::UNIX_EPOCH)
            .map(|d| d.as_millis())
            .unwrap_or(0)
    );
    // The clock for the bar starts when the decision shows, not when it
    // was queued behind another.
    let left = timeout.map(|t| t.saturating_sub(started.elapsed()));
    let decision = Decision {
        id: id.clone(),
        question: question.to_string(),
        options: options.to_vec(),
        pid: std::process::id(),
        opened_ms: left.map(|_| {
            SystemTime::now()
                .duration_since(SystemTime::UNIX_EPOCH)
                .map(|d| d.as_millis() as u64)
                .unwrap_or(0)
        }),
        timeout_ms: left.map(|t| t.as_millis() as u64),
        other,
    };
    std::fs::create_dir_all(state_dir)?;
    let tmp = state_dir.join(format!("{FILE}.tmp"));
    std::fs::write(&tmp, serde_json::to_vec(&decision).unwrap_or_default())?;
    std::fs::rename(&tmp, &file)?;
    let close = |file: &Path| {
        // Only our own: never take down a decision someone else opened.
        let ours = std::fs::read_to_string(file)
            .ok()
            .and_then(|s| serde_json::from_str::<Decision>(&s).ok())
            .is_some_and(|d| d.id == id);
        if ours {
            let _ = std::fs::remove_file(file);
        }
    };
    loop {
        if let Some(answer) = std::fs::read_to_string(&answer_file)
            .ok()
            .and_then(|s| serde_json::from_str::<Answer>(&s).ok())
            .filter(|a| a.id == decision.id)
        {
            let _ = std::fs::remove_file(&answer_file);
            return Ok(Some(answer));
        }
        if expired() {
            close(&file);
            return Ok(None);
        }
        tokio::select! {
            _ = tokio::time::sleep(Duration::from_millis(100)) => {}
            _ = tokio::signal::ctrl_c() => {
                close(&file);
                std::process::exit(130);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn decision(options: &[&str]) -> Decision {
        Decision {
            id: "t".into(),
            question: "?".into(),
            options: options.iter().map(|o| o.to_string()).collect(),
            pid: std::process::id(),
            opened_ms: None,
            timeout_ms: None,
            other: false,
        }
    }

    #[test]
    fn hears_the_option_its_number_or_its_start() {
        let d = decision(&["Deploy now", "Wait for review", "Roll back"]);
        assert_eq!(d.hear("Deploy now."), Heard::Chose(0));
        assert_eq!(d.hear("two"), Heard::Chose(1));
        assert_eq!(d.hear("The third one"), Heard::Chose(2));
        assert_eq!(d.hear("let's roll back please"), Heard::Chose(2));
        assert_eq!(d.hear("deploy"), Heard::Chose(0));
        assert_eq!(d.hear("wait"), Heard::Chose(1));
        assert_eq!(d.hear("ro"), Heard::Unclear, "too short to be a start");
        assert_eq!(d.hear("cancel"), Heard::Cancel);
        assert_eq!(d.hear("banana"), Heard::Unclear);
        assert_eq!(d.hear("four"), Heard::Unclear, "only three options");
    }

    #[test]
    fn the_longest_option_said_wins_a_tie_is_unclear() {
        let d = decision(&["yes", "yes but later"]);
        assert_eq!(d.hear("yes but later please"), Heard::Chose(1));
        let d = decision(&["red", "blue"]);
        assert_eq!(d.hear("red or blue"), Heard::Unclear);
    }

    #[test]
    fn an_option_named_cancel_is_an_option() {
        let d = decision(&["Proceed", "Cancel"]);
        assert_eq!(d.hear("cancel"), Heard::Chose(1));
    }

    #[tokio::test]
    async fn ask_and_answer_round_trip() {
        let dir = tempfile::tempdir().unwrap();
        let state = dir.path().to_path_buf();
        let asker = {
            let state = state.clone();
            tokio::spawn(async move {
                ask(
                    &state,
                    "Ship it?",
                    &["yes".into(), "no".into()],
                    false,
                    Some(Duration::from_secs(5)),
                )
                .await
                .unwrap()
            })
        };
        let mut decisions = Decisions::new(&state);
        let open = loop {
            decisions.looked = None;
            if let Some(d) = decisions.open() {
                break d.clone();
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        };
        assert_eq!(open.question, "Ship it?");
        let i = match open.hear("no") {
            Heard::Chose(i) => i,
            other => panic!("{other:?}"),
        };
        assert_eq!(
            decisions.answer(Some(i), None).unwrap().as_deref(),
            Some("no")
        );
        let answer = asker.await.unwrap().unwrap();
        assert_eq!(answer.choice.as_deref(), Some("no"));
        assert_eq!(answer.index, Some(1));
        assert!(!state.join(FILE).exists());
        assert!(!state.join(ANSWER).exists());
    }

    #[tokio::test]
    async fn a_timeout_closes_only_its_own_decision() {
        let dir = tempfile::tempdir().unwrap();
        let answer = ask(
            dir.path(),
            "?",
            &["a".into(), "b".into()],
            false,
            Some(Duration::from_millis(150)),
        )
        .await
        .unwrap();
        assert!(answer.is_none());
        assert!(!dir.path().join(FILE).exists());
    }

    #[test]
    fn embedding_picks_only_a_clear_lead() {
        assert_eq!(pick_by_scores(&[0.70, 0.78, 0.72]), Some(1));
        assert_eq!(pick_by_scores(&[0.70, 0.73]), None);
        assert_eq!(pick_by_scores(&[0.5]), Some(0));
    }

    #[test]
    fn a_dead_askers_decision_lapses() {
        let dir = tempfile::tempdir().unwrap();
        let mut d = decision(&["a", "b"]);
        d.pid = 999_999_999;
        std::fs::write(dir.path().join(FILE), serde_json::to_vec(&d).unwrap()).unwrap();
        let mut decisions = Decisions::new(dir.path());
        assert!(decisions.open().is_none());
    }
}
