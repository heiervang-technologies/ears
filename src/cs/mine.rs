//! Opt-in mining of compensating scaffolding (RFC 0002): a local journal of
//! command-like utterances, and `ears cs mine`, which reads it and proposes
//! cs entries for review.
//!
//! # The journal
//!
//! With `[cs] journal = true` (default off) VAD appends one JSON line per
//! finished utterance to `$XDG_STATE_HOME/ears/cs-journal.jsonl`
//! (`~/.local/state/ears/` without it): when, in which mode, the words
//! (normalized with [`crate::commands::normalize`]), what they matched (a
//! target and the phrase, see [`Line`]) and, in command mode, the routing
//! decision also written to `command-last.json`.
//!
//! **Privacy.** Dictation is personal, so the journal only takes:
//!
//! - utterances in command mode and an open decision (`ears decide`),
//!   matched or not;
//! - in standby, utterances that matched, and unmatched ones only when
//!   they sound like a try at a phrase standby listens for (wake, sleep,
//!   standby, run commands, "command"): phonetic distance at most
//!   `mine_max_distance` ([`nearest_standby_phrase`]). Standby hears the
//!   room; everything else it hears is overheard or private and dropped;
//! - in dictation and vision, only utterances that *are* a voice command
//!   (wake, sleep, "command", a colour, a run command, enter, undo...). Of
//!   a run command that takes the rest of the utterance (`prefix`), and of
//!   a command closing a dictated sentence ("... over"), only the command's
//!   own words are kept. `literal ...` is dictation and is never kept, nor
//!   is an answer of your own to a decision.
//!
//! Ordinary dictated text never reaches the journal. The file is created
//! with mode 0600, stays below `journal_max_kb` (rotated once to
//! `cs-journal.jsonl.1`) and loses lines older than `journal_max_days`.
//!
//! Writing never holds up the audio path: [`Journal`] hands each line to a
//! writer thread through a bounded queue and drops it when the queue is
//! full, counting the drops (the next written line carries the count).
//!
//! # The miner
//!
//! `ears cs mine` reads the journal and looks for evidence that a phrase was
//! misheard ([`Evidence`]):
//!
//! - **retry**: an unmatched utterance followed within `mine_window_s` by a
//!   matched one in the same mode, with nothing matched in between: the
//!   first was most likely a failed try at the second;
//! - **routed**: command mode or a decision fired by sound or meaning while
//!   the words were something else;
//! - **chant**: the other readings of one chanted breath (ears#264) that
//!   fired an action.
//!
//! Each pair must sound alike (`cs::risk::phonetic_distance` at most
//! `mine_max_distance`), so two unrelated things said seconds apart do not
//! pair. Evidence is summed per (target, heard); at least `mine_min_count`
//! makes a proposal, scored with `cs::risk::assess` and classified by
//! failure mode. A (target, heard) any built-in or cs.d file already has,
//! in any status and for any model, is not proposed again. Proposals go to
//! a review file with `source = "mined"` and `status = "flagged"`: nothing
//! mined is ever applied by itself. Running it again keeps what was
//! reviewed in it (any status but `flagged`) as it is.

use crate::commands::{normalize, Command, VoiceCommands};
use crate::cs::risk::{assess, phonetic_distance, phonetic_key, Lexicon, Phrase};
use crate::cs::{CsFile, Entry, FailureMode, Loaded, Meta, Source, Status};
use crate::vad_mode::ModeKind;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::io::Write as _;
use std::os::unix::fs::OpenOptionsExt as _;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{sync_channel, Receiver, SyncSender, TrySendError};
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

/// The journal's file name in the ears state dir.
pub const JOURNAL: &str = "cs-journal.jsonl";
/// Lines waiting for the writer; more are dropped.
pub const QUEUE: usize = 64;

// ---------------------------------------------------------------------------
// Configuration

/// The `[cs]` config section (journal and miner keys included).
pub use crate::config::CsConfig;

/// `$XDG_STATE_HOME/ears/cs-journal.jsonl`, else under `~/.local/state`.
pub fn journal_path() -> Option<PathBuf> {
    let base = std::env::var_os("XDG_STATE_HOME")
        .map(PathBuf::from)
        .filter(|p| p.is_absolute())
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".local/state")))?;
    Some(base.join("ears").join(JOURNAL))
}

fn rotated(path: &Path) -> PathBuf {
    let mut name = path.as_os_str().to_os_string();
    name.push(".1");
    PathBuf::from(name)
}

fn now() -> f64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs_f64()
}

// ---------------------------------------------------------------------------
// Journal lines

/// One finished utterance.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Line {
    /// Unix seconds.
    pub time: f64,
    /// `standby`, `dictation`, `command`, `vision` or `decide`.
    pub mode: String,
    /// The words, normalized. Of a tail or prefix command, its words only.
    pub text: String,
    /// The list it matched (a cs target: `wake`, `enter`, `run:<name>`,
    /// `action:<name>`, `decide:<option>`, and `command` for the command
    /// mode switch, `colour:<name>` for a colour), or none.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub target: Option<String>,
    /// The phrase it matched (normalized): the canonical phrase the miner
    /// proposes `text` of an earlier try as a variant of.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub phrase: Option<String>,
    /// How it matched when not by its words: `voice`, `both`, `meaning`
    /// (command mode), `embedding` (a decision).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub via: Option<String>,
    /// A command that closed a dictated sentence ("... over").
    #[serde(default, skip_serializing_if = "is_false")]
    pub tail: bool,
    /// Other readings of the same utterance: a chanted breath's folded
    /// repeats (chant mode, ears#264).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub alts: Vec<String>,
    /// Command mode: the routing decision (as in `command-last.json`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub routing: Option<serde_json::Value>,
    /// Lines dropped on a full queue since the journal started, when any.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dropped: Option<u64>,
}

fn is_false(b: &bool) -> bool {
    !*b
}

impl Line {
    fn new(mode: &str, text: String) -> Self {
        Self {
            time: now(),
            mode: mode.to_string(),
            text,
            target: None,
            phrase: None,
            via: None,
            tail: false,
            alts: Vec::new(),
            routing: None,
            dropped: None,
        }
    }

    fn matched(mut self, target: impl Into<String>, phrase: impl Into<String>) -> Self {
        self.target = Some(target.into());
        self.phrase = Some(phrase.into());
        self
    }

    pub fn is_matched(&self) -> bool {
        self.target.is_some()
    }
}

/// What the engine knows when a transcript is finished (after the text
/// filters, before anything acts on it).
pub struct Context<'a> {
    pub commands: &'a VoiceCommands,
    pub mode: ModeKind,
    /// Command mode is available ("command" switches it on).
    pub command_mode: bool,
    /// Vision mode is available (pink is a colour word).
    pub vision: bool,
    /// Bash grammar decoding: a shell command, dictation.
    pub guided: bool,
    /// Unmatched standby speech is kept only this close (phonetic
    /// distance) to a phrase standby listens for (`[cs] mine_max_distance`).
    pub max_distance: f32,
}

/// The journal line for a transcript outside command mode and decisions,
/// or `None` when it is not to be kept. Mirrors the order the engine
/// matches in (`StreamingEngine::process_segment`).
pub fn transcript_line(ctx: &Context, transcript: &str) -> Option<Line> {
    if ctx.guided {
        return None;
    }
    let said = normalize(transcript);
    if said.is_empty() {
        return None;
    }
    let mode = ctx.mode.name();
    let line = |text: &str| Line::new(mode, text.to_string());
    if ctx.command_mode && crate::command_mode::ENTER_PHRASES.contains(&said.as_str()) {
        return Some(line(&said).matched("command", said.clone()));
    }
    if let Some(colour) = crate::colour_mode::Colour::parse(transcript)
        .filter(|c| *c != crate::colour_mode::Colour::Pink || ctx.vision)
    {
        return Some(line(&said).matched(format!("colour:{}", colour.name()), said.clone()));
    }
    let run = match ctx.commands.parse(transcript) {
        Some(Command::Run { name, .. }) => run_phrase(ctx.commands, &name, &said),
        _ => None,
    };
    if let Some((name, phrase, true)) = &run {
        return Some(line(phrase).matched(format!("run:{name}"), phrase.clone()));
    }
    if ctx.commands.enabled {
        let lists = [
            ("standby", &ctx.commands.standby),
            ("wake", &ctx.commands.wake),
            ("sleep", &ctx.commands.sleep),
        ];
        for (target, list) in lists {
            if list.iter().any(|p| normalize(p) == said) {
                return Some(line(&said).matched(target, said.clone()));
            }
        }
    }
    match ctx.mode {
        ModeKind::Standby => match run {
            Some((name, phrase, _)) => Some(line(&phrase).matched(format!("run:{name}"), phrase)),
            // Standby hears the room: only what sounds like a try at one of
            // its phrases is kept, not conversation.
            None => (nearest_standby_phrase(ctx, &said) <= ctx.max_distance).then(|| line(&said)),
        },
        // Utterances go to `ears look` before commands are parsed.
        ModeKind::Vision => None,
        // Command mode does not come here; treat it as dictation if it does.
        ModeKind::Dictation | ModeKind::Command => {
            dictation_line(ctx.commands, transcript, &said, run, mode)
        }
    }
}

/// The phonetic distance from `said` to the nearest phrase standby listens
/// for: wake, sleep and standby phrases (with commands on), run commands,
/// and the command mode switch. 1 when there are none.
pub fn nearest_standby_phrase(ctx: &Context, said: &str) -> f32 {
    let mut phrases: Vec<String> = Vec::new();
    if ctx.commands.enabled {
        for list in [
            &ctx.commands.standby,
            &ctx.commands.wake,
            &ctx.commands.sleep,
        ] {
            phrases.extend(list.iter().cloned());
        }
    }
    for run in ctx.commands.run.resolved() {
        phrases.extend(run.phrases);
    }
    if ctx.command_mode {
        phrases.extend(
            crate::command_mode::ENTER_PHRASES
                .iter()
                .map(|p| p.to_string()),
        );
    }
    phrases
        .iter()
        .map(|p| normalize(p))
        .filter(|p| !p.is_empty())
        .map(|p| phonetic_distance(said, &p))
        .fold(1.0, f32::min)
}

/// A dictated utterance: kept only if it is a voice command, and then only
/// the command's words.
fn dictation_line(
    commands: &VoiceCommands,
    transcript: &str,
    said: &str,
    run: Option<(String, String, bool)>,
    mode: &str,
) -> Option<Line> {
    match commands.parse(transcript) {
        Some(Command::Literal(_)) => None,
        Some(Command::Run { .. }) => {
            let (name, phrase, _) = run?;
            Some(Line::new(mode, phrase.clone()).matched(format!("run:{name}"), phrase))
        }
        Some(command) => Some(Line::new(mode, said.to_string()).matched(target_of(&command), said)),
        None => match commands.parse_tail(transcript) {
            None | Some((_, Command::Literal(_))) => None,
            Some((body, command)) => {
                let body_words = normalize(&body).split(' ').count();
                let words: Vec<&str> = said.split(' ').collect();
                let phrase = words.get(body_words..)?.join(" ");
                if phrase.is_empty() {
                    return None;
                }
                let mut line = Line::new(mode, phrase.clone()).matched(target_of(&command), phrase);
                line.tail = true;
                Some(line)
            }
        },
    }
}

/// The cs target a parsed command extends.
fn target_of(command: &Command) -> String {
    match command {
        Command::Run { name, .. } => format!("run:{name}"),
        other => other.name().to_string(),
    }
}

/// The run command `name`'s phrase `said` matched: (name, phrase, resume).
/// For a prefix match the phrase alone, without what followed it.
fn run_phrase(commands: &VoiceCommands, name: &str, said: &str) -> Option<(String, String, bool)> {
    let run = commands
        .run
        .resolved()
        .into_iter()
        .find(|r| r.name == name)?;
    let phrase = run
        .phrases
        .iter()
        .map(|p| normalize(p))
        .filter(|p| !p.is_empty())
        .filter(|p| said == p || (run.prefix && said.starts_with(&format!("{p} "))))
        .max_by_key(|p| p.len())?;
    Some((run.name, phrase, run.resume))
}

/// The journal line for a command-mode utterance. Taught and forgotten
/// recordings are not voice matches and are not kept.
pub fn command_line(
    outcome: &crate::command_mode::Outcome,
    hearing: &crate::command_mode::Hearing,
) -> Option<Line> {
    use crate::command_mode::Outcome;
    let said = normalize(&hearing.words);
    let mut line = Line::new(ModeKind::Command.name(), said.clone());
    line.routing = serde_json::to_value(hearing).ok();
    match outcome {
        Outcome::Fire { name, .. } => {
            let by_words = hearing.by.is_none_or(|by| by == "words");
            let phrase = if by_words && !said.is_empty() {
                said
            } else {
                action_phrase(name)
            };
            line = line.matched(format!("action:{name}"), phrase);
            if !by_words {
                line.via = hearing.by.map(str::to_string);
            }
        }
        Outcome::Nothing if !said.is_empty() => {}
        _ => return None,
    }
    Some(line)
}

/// An action's name as words: "close-window" -> "close window".
fn action_phrase(name: &str) -> String {
    normalize(&name.replace(['-', '_'], " "))
}

/// The journal line for an utterance heard by an open decision. `outcome`
/// is `chose`, `cancel`, `custom` or `unclear` (as noted by the engine). An
/// answer of your own is personal and not kept.
pub fn decision_line(
    options: &[String],
    choice: Option<usize>,
    outcome: &str,
    said: &str,
    via: &str,
) -> Option<Line> {
    let said = normalize(said);
    let line = Line::new("decide", said.clone());
    let mut line = match (outcome, choice.and_then(|i| options.get(i))) {
        ("chose", Some(option)) => {
            let option = normalize(option);
            line.matched(format!("decide:{option}"), option)
        }
        ("cancel", _) if !said.is_empty() => line.matched("decide:cancel", said.clone()),
        ("unclear", _) if !said.is_empty() => line,
        _ => return None,
    };
    if via != "words" && line.is_matched() {
        line.via = Some(via.to_string());
    }
    Some(line)
}

// ---------------------------------------------------------------------------
// The journal writer

/// The journal's write end, held by the engine. Cheap to call: lines go to
/// a writer thread and are dropped (counted) when it falls behind.
pub struct Journal {
    tx: SyncSender<Line>,
    dropped: Arc<AtomicU64>,
}

impl Journal {
    /// A journal at the default path, if `[cs] journal` is on. Logs and
    /// returns `None` when it cannot start.
    pub fn from_config(config: &CsConfig) -> Option<Journal> {
        if !config.journal {
            return None;
        }
        let Some(path) = journal_path() else {
            tracing::warn!("cs journal: no state dir (HOME unset)");
            return None;
        };
        match Journal::start(&path, config) {
            Ok(journal) => {
                tracing::info!("cs journal: {}", path.display());
                Some(journal)
            }
            Err(e) => {
                tracing::warn!("cs journal: cannot start at {}: {}", path.display(), e);
                None
            }
        }
    }

    /// Start a writer thread for `path`.
    pub fn start(path: &Path, config: &CsConfig) -> std::io::Result<Journal> {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        let (tx, rx) = sync_channel(QUEUE);
        let dropped = Arc::new(AtomicU64::new(0));
        let mut writer = Writer::new(path, config, dropped.clone());
        writer.prune(now());
        std::thread::Builder::new()
            .name("cs-journal".into())
            .spawn(move || writer.run(rx))?;
        Ok(Journal { tx, dropped })
    }

    /// A journal whose lines go to `tx` (tests).
    pub fn with_sender(tx: SyncSender<Line>) -> Journal {
        Journal {
            tx,
            dropped: Arc::new(AtomicU64::new(0)),
        }
    }

    /// Queue `line`; never blocks.
    pub fn send(&self, line: Line) {
        match self.tx.try_send(line) {
            Ok(()) => {}
            Err(TrySendError::Full(_)) => {
                if self.dropped.fetch_add(1, Ordering::Relaxed) == 0 {
                    tracing::warn!("cs journal: writer behind, dropping lines");
                }
            }
            Err(TrySendError::Disconnected(_)) => {
                self.dropped.fetch_add(1, Ordering::Relaxed);
            }
        }
    }

    /// Lines dropped so far.
    pub fn dropped(&self) -> u64 {
        self.dropped.load(Ordering::Relaxed)
    }

    /// A finished transcript outside command mode and decisions.
    pub fn transcript(&self, ctx: &Context, transcript: &str) {
        if let Some(line) = transcript_line(ctx, transcript) {
            self.send(line);
        }
    }

    /// A command-mode utterance and how it was routed.
    pub fn command(
        &self,
        outcome: &crate::command_mode::Outcome,
        hearing: &crate::command_mode::Hearing,
    ) {
        if let Some(line) = command_line(outcome, hearing) {
            self.send(line);
        }
    }

    /// An utterance heard by an open decision.
    pub fn decision(
        &self,
        options: &[String],
        choice: Option<usize>,
        outcome: &str,
        said: &str,
        via: &str,
    ) {
        if let Some(line) = decision_line(options, choice, outcome, said, via) {
            self.send(line);
        }
    }
}

/// How often the writer drops lines past the age cap while running.
const PRUNE_EVERY: Duration = Duration::from_secs(3600);

/// The writer thread's side: appends, rotates and prunes.
struct Writer {
    path: PathBuf,
    max_bytes: u64,
    max_age_s: f64,
    dropped: Arc<AtomicU64>,
    last_prune: f64,
}

impl Writer {
    fn new(path: &Path, config: &CsConfig, dropped: Arc<AtomicU64>) -> Self {
        Self {
            path: path.to_path_buf(),
            max_bytes: config.journal_max_kb.max(1) * 1024,
            max_age_s: config.journal_max_days.max(1) as f64 * 86400.0,
            dropped,
            last_prune: 0.0,
        }
    }

    fn run(mut self, rx: Receiver<Line>) {
        for line in rx {
            if let Err(e) = self.append(line) {
                tracing::warn!("cs journal: cannot write {}: {}", self.path.display(), e);
            }
        }
    }

    fn append(&mut self, mut line: Line) -> std::io::Result<()> {
        let dropped = self.dropped.load(Ordering::Relaxed);
        line.dropped = (dropped > 0).then_some(dropped);
        let mut text = serde_json::to_string(&line).map_err(std::io::Error::other)?;
        text.push('\n');
        let mut file = open_private(&self.path, true)?;
        file.write_all(text.as_bytes())?;
        let size = file.metadata()?.len();
        drop(file);
        if size > self.max_bytes {
            std::fs::rename(&self.path, rotated(&self.path))?;
        }
        if line.time - self.last_prune >= PRUNE_EVERY.as_secs_f64() {
            self.prune(line.time);
        }
        Ok(())
    }

    /// Drop lines older than the age cap, and the rotated file once all of
    /// it is.
    fn prune(&mut self, now: f64) {
        self.last_prune = now;
        let cutoff = now - self.max_age_s;
        let old = rotated(&self.path);
        if let Ok(meta) = std::fs::metadata(&old) {
            let modified = meta
                .modified()
                .ok()
                .and_then(|m| m.duration_since(UNIX_EPOCH).ok())
                .map_or(0.0, |d| d.as_secs_f64());
            if modified < cutoff {
                let _ = std::fs::remove_file(&old);
            }
        }
        let Ok(text) = std::fs::read_to_string(&self.path) else {
            return;
        };
        let keep: Vec<&str> = text
            .lines()
            .filter(|l| {
                serde_json::from_str::<Line>(l)
                    .map(|line| line.time >= cutoff)
                    .unwrap_or(false)
            })
            .collect();
        if keep.len() == text.lines().count() {
            return;
        }
        let tmp = self.path.with_extension("jsonl.tmp");
        let result = open_private(&tmp, false).and_then(|mut f| {
            for l in &keep {
                writeln!(f, "{l}")?;
            }
            Ok(())
        });
        if let Err(e) = result.and_then(|()| std::fs::rename(&tmp, &self.path)) {
            tracing::warn!("cs journal: cannot prune {}: {}", self.path.display(), e);
            let _ = std::fs::remove_file(&tmp);
        }
    }
}

/// Open for writing with mode 0600 (also on a file that exists).
fn open_private(path: &Path, append: bool) -> std::io::Result<std::fs::File> {
    use std::os::unix::fs::PermissionsExt as _;
    let mut options = std::fs::OpenOptions::new();
    options.create(true).mode(0o600);
    if append {
        options.append(true);
    } else {
        options.write(true).truncate(true);
    }
    let file = options.open(path)?;
    let meta = file.metadata()?;
    if meta.permissions().mode() & 0o077 != 0 {
        file.set_permissions(std::fs::Permissions::from_mode(0o600))?;
    }
    Ok(file)
}

/// The journal's lines, the rotated file's first, oldest first. Lines that
/// do not parse are skipped.
pub fn read_journal(path: &Path) -> Vec<Line> {
    let mut lines = Vec::new();
    for file in [rotated(path), path.to_path_buf()] {
        if let Ok(text) = std::fs::read_to_string(&file) {
            lines.extend(parse_lines(&text));
        }
    }
    lines.sort_by(|a, b| a.time.total_cmp(&b.time));
    lines
}

pub fn parse_lines(text: &str) -> Vec<Line> {
    text.lines()
        .filter_map(|l| serde_json::from_str(l).ok())
        .collect()
}

// ---------------------------------------------------------------------------
// The miner

/// Why the miner thinks `heard` was a mishearing of a phrase.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Evidence {
    /// Unmatched, then matched seconds later in the same mode.
    Retry,
    /// Fired by sound or meaning while the words were these.
    Routed,
    /// Another reading of a chanted breath that fired.
    Chant,
}

impl Evidence {
    fn name(self) -> &'static str {
        match self {
            Evidence::Retry => "retry",
            Evidence::Routed => "routed",
            Evidence::Chant => "chant",
        }
    }
}

/// How the miner pairs and counts.
#[derive(Debug, Clone)]
pub struct MineParams {
    pub window_s: f64,
    pub min_count: u32,
    pub max_distance: f32,
    /// Only lines at or after this Unix time.
    pub since: Option<f64>,
}

impl MineParams {
    pub fn from_config(config: &CsConfig) -> Self {
        Self {
            window_s: config.mine_window_s,
            min_count: config.mine_min_count.max(1),
            max_distance: config.mine_max_distance,
            since: None,
        }
    }
}

/// One proposed entry, before it is scored.
#[derive(Debug, Clone, PartialEq)]
pub struct Proposal {
    pub target: String,
    pub heard: String,
    /// The phrase it was taken for most often.
    pub meant: String,
    pub count: u32,
    /// Evidence kinds and how often each was seen.
    pub evidence: BTreeMap<&'static str, u32>,
}

/// Targets the miner does not propose for: colour words are fixed.
fn proposable(target: &str) -> bool {
    !target.starts_with("colour:") && target != "decide:cancel"
}

/// Find evidence in `lines` (oldest first) and sum it per (target, heard).
pub fn mine(lines: &[Line], params: &MineParams) -> Vec<Proposal> {
    #[derive(Default)]
    struct Tally {
        count: u32,
        meant: BTreeMap<String, u32>,
        evidence: BTreeMap<&'static str, u32>,
    }
    let lines: Vec<&Line> = lines
        .iter()
        .filter(|l| params.since.is_none_or(|s| l.time >= s))
        .collect();
    let mut tallies: BTreeMap<(String, String), Tally> = BTreeMap::new();
    let mut note = |target: &str, heard: &str, meant: &str, evidence: Evidence| {
        if heard.is_empty() || heard == meant || !proposable(target) {
            return;
        }
        if phonetic_distance(heard, meant) > params.max_distance {
            return;
        }
        let tally = tallies
            .entry((target.to_string(), heard.to_string()))
            .or_default();
        tally.count += 1;
        *tally.meant.entry(meant.to_string()).or_default() += 1;
        *tally.evidence.entry(evidence.name()).or_default() += 1;
    };
    for (i, line) in lines.iter().enumerate() {
        if let (Some(target), Some(phrase)) = (&line.target, &line.phrase) {
            if line.via.is_some() {
                note(target, &line.text, phrase, Evidence::Routed);
            }
            for alt in &line.alts {
                note(target, &normalize(alt), phrase, Evidence::Chant);
            }
            continue;
        }
        // Unmatched: the next matched line in the same mode, in time.
        for next in &lines[i + 1..] {
            if next.time - line.time > params.window_s || next.mode != line.mode {
                break;
            }
            if let (Some(target), Some(phrase)) = (&next.target, &next.phrase) {
                note(target, &line.text, phrase, Evidence::Retry);
                break;
            }
        }
    }
    tallies
        .into_iter()
        .filter(|(_, t)| t.count >= params.min_count)
        .map(|((target, heard), t)| {
            let meant = t
                .meant
                .iter()
                .max_by_key(|(_, n)| **n)
                .map(|(m, _)| m.clone())
                .unwrap_or_default();
            Proposal {
                target,
                heard,
                meant,
                count: t.count,
                evidence: t.evidence,
            }
        })
        .collect()
}

/// The failure mode a (heard, meant) pair looks like. A heuristic: the
/// reviewer corrects it.
pub fn classify(heard: &str, meant: &str) -> FailureMode {
    let h: Vec<&str> = heard.split(' ').collect();
    let m: Vec<&str> = meant.split(' ').collect();
    if h.iter().filter(|w| w.chars().count() == 1).count() >= 2 {
        return FailureMode::Spelled;
    }
    if h.len() != m.len() && heard.replace(' ', "") == meant.replace(' ', "") {
        return FailureMode::Split;
    }
    let contains = |long: &[&str], short: &[&str]| {
        short.len() < long.len() && long.windows(short.len()).any(|w| w == short)
    };
    if contains(&m, &h) {
        return FailureMode::Truncation;
    }
    if h.len() == m.len() {
        let differ: Vec<(&str, &str)> = h
            .iter()
            .zip(&m)
            .filter(|(a, b)| a != b)
            .map(|(a, b)| (*a, *b))
            .collect();
        if let [(a, b)] = differ[..] {
            if phonetic_key(a) == phonetic_key(b) && b.chars().count() <= 5 {
                return FailureMode::Homophone;
            }
            if phonetic_distance(a, b) < 0.45 {
                return FailureMode::NameDrift;
            }
        }
    }
    let d = phonetic_distance(heard, meant);
    if d <= 0.2 {
        FailureMode::Homophone
    } else if d >= 0.45 {
        FailureMode::Hallucination
    } else {
        FailureMode::Other
    }
}

/// Every phrase a list already holds, as (target, phrase), for collision
/// checks.
pub fn known_phrases(commands: &VoiceCommands) -> Vec<(String, String)> {
    let mut out = Vec::new();
    let mut add = |target: &str, list: &[String]| {
        for p in list {
            let p = normalize(p);
            if !p.is_empty() {
                out.push((target.to_string(), p));
            }
        }
    };
    add("enter", &commands.enter);
    add("sign_off", &commands.sign_off);
    add("new_line", &commands.new_line);
    add("undo", &commands.undo);
    add("back", &commands.back);
    add("backtrack", &commands.backtrack);
    add("standby", &commands.standby);
    add("wake", &commands.wake);
    add("sleep", &commands.sleep);
    add("literal", &commands.literal);
    add("steer", &commands.steer);
    let keys: Vec<String> = commands.keys.keys().cloned().collect();
    add("keys", &keys);
    for run in commands.run.resolved() {
        add(&format!("run:{}", run.name), &run.phrases);
    }
    let enter: Vec<String> = crate::command_mode::ENTER_PHRASES
        .iter()
        .map(|s| s.to_string())
        .collect();
    add("command", &enter);
    out
}

/// Score and classify proposals into cs entries, always `flagged`.
pub fn entries(proposals: &[Proposal], known: &[(String, String)]) -> Vec<Entry> {
    let lexicon = Lexicon::builtin();
    proposals
        .iter()
        .map(|p| {
            let others: Vec<Phrase> = known
                .iter()
                .filter(|(t, _)| *t != p.target)
                .map(|(t, text)| Phrase {
                    text: text.as_str(),
                    target: t.as_str(),
                })
                .collect();
            let risk = assess(&p.heard, &p.target, &others, &lexicon);
            let evidence = p
                .evidence
                .iter()
                .map(|(k, n)| format!("{k} {n}"))
                .collect::<Vec<_>>()
                .join(", ");
            let reason = match risk.reason.is_empty() {
                true => format!("mined: {evidence}"),
                false => format!("mined: {evidence}; {}", risk.reason),
            };
            Entry {
                meant: p.meant.clone(),
                target: p.target.clone(),
                heard: p.heard.clone(),
                mode: classify(&p.heard, &p.meant),
                source: Source::Mined,
                count: p.count,
                trials: 0,
                risk: risk.score,
                risk_reason: reason,
                status: Status::Flagged,
            }
        })
        .collect()
}

/// Drop proposals whose (target, heard) any cs file already has, whatever
/// its status or model: built-in files and the user's cs.d. An entry that
/// was reviewed (or rejected) anywhere is never proposed again. `skip` is
/// the review file itself, whose flagged entries [`merge`] refreshes.
pub fn unknown(fresh: Vec<Entry>, files: &[Loaded], skip: Option<&Path>) -> Vec<Entry> {
    let same_file = |file: &str| {
        skip.is_some_and(|out| {
            let file = Path::new(file);
            file == out
                || matches!(
                    (file.canonicalize(), out.canonicalize()),
                    (Ok(a), Ok(b)) if a == b
                )
        })
    };
    let known: std::collections::HashSet<(String, String)> = files
        .iter()
        .filter(|f| !same_file(&f.file))
        .flat_map(|f| &f.cs.entry)
        .map(|e| (e.target.trim().to_string(), normalize(&e.heard)))
        .collect();
    fresh
        .into_iter()
        .filter(|e| !known.contains(&(e.target.clone(), normalize(&e.heard))))
        .collect()
}

/// Merge fresh mined entries into what the review file already holds:
/// reviewed entries (not `flagged`) stay exactly as they are and are not
/// proposed again; flagged ones are replaced by the fresh count.
pub fn merge(existing: Vec<Entry>, fresh: Vec<Entry>) -> Vec<Entry> {
    let mut out: Vec<Entry> = Vec::new();
    let key = |e: &Entry| (e.target.clone(), e.heard.clone());
    let reviewed: std::collections::HashSet<_> = existing
        .iter()
        .filter(|e| e.status != Status::Flagged)
        .map(key)
        .collect();
    let fresh_keys: std::collections::HashSet<_> = fresh.iter().map(key).collect();
    for e in existing {
        if e.status != Status::Flagged || !fresh_keys.contains(&key(&e)) {
            out.push(e);
        }
    }
    out.extend(fresh.into_iter().filter(|e| !reviewed.contains(&key(e))));
    out.sort_by(|a, b| (&a.target, &a.heard).cmp(&(&b.target, &b.heard)));
    out
}

/// The review file's text.
pub fn render(model: &str, entries: Vec<Entry>) -> anyhow::Result<String> {
    let file = CsFile {
        meta: Meta {
            model: model.to_string(),
            domain: "commands".into(),
            generated: today(),
            generator: "ears cs mine".into(),
        },
        entry: entries,
    };
    let body = toml::to_string_pretty(&file)?;
    Ok(format!(
        "# Mined by `ears cs mine` from the local utterance journal.\n\
         # Every entry is flagged: review each, set status = \"active\" to use\n\
         # it or \"rejected\" to keep it from being proposed again.\n\n{body}"
    ))
}

/// Today as YYYY-MM-DD (UTC).
fn today() -> String {
    let days = (now() / 86400.0).floor() as i64;
    // Howard Hinnant's civil_from_days.
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + i64::from(m <= 2);
    format!("{y:04}-{m:02}-{d:02}")
}

/// "90s", "30m", "12h", "7d", "2w" (a bare number is seconds).
pub fn parse_duration(text: &str) -> Result<Duration, String> {
    let text = text.trim();
    let (number, unit) = text
        .find(|c: char| !c.is_ascii_digit() && c != '.')
        .map_or((text, ""), |i| text.split_at(i));
    let n: f64 = number
        .parse()
        .map_err(|_| format!("not a duration: {text:?} (try 7d, 12h, 30m)"))?;
    let scale = match unit {
        "" | "s" => 1.0,
        "m" => 60.0,
        "h" => 3600.0,
        "d" => 86400.0,
        "w" => 7.0 * 86400.0,
        _ => return Err(format!("unknown unit in {text:?}: use s, m, h, d or w")),
    };
    Ok(Duration::from_secs_f64(n * scale))
}

/// Options of `ears cs mine`.
#[derive(Debug, Clone, Default)]
pub struct MineArgs {
    pub since: Option<Duration>,
    pub out: Option<PathBuf>,
    pub journal: Option<PathBuf>,
    pub model: Option<String>,
}

/// `ears cs mine`: read the journal, write the review file, report.
pub fn run(config: &crate::config::Config, args: &MineArgs) -> anyhow::Result<()> {
    use anyhow::Context as _;
    let journal = match &args.journal {
        Some(p) => p.clone(),
        None => journal_path().context("no state dir (HOME unset)")?,
    };
    let out = args
        .out
        .clone()
        .unwrap_or_else(|| config.config_dir.join("cs.d").join("mined.toml"));
    let lines = read_journal(&journal);
    if lines.is_empty() {
        let hint = match config.cs.journal {
            true => "nothing journaled yet",
            false => "the journal is off: set `[cs] journal = true` and restart VAD",
        };
        println!("No journal lines in {} ({hint}).", journal.display());
        return Ok(());
    }
    let mut params = MineParams::from_config(&config.cs);
    params.since = args.since.map(|d| now() - d.as_secs_f64());
    let proposals = mine(&lines, &params);
    let fresh = entries(&proposals, &known_phrases(&config.commands));
    let mut files = crate::cs::builtin_files();
    files.extend(crate::cs::read_dir(&config.config_dir.join("cs.d")));
    let fresh = unknown(fresh, &files, Some(&out));
    let listed: Vec<String> = fresh
        .iter()
        .map(|e| {
            format!(
                "  {:<16} {:?} -> {:?} x{}",
                e.target, e.heard, e.meant, e.count
            )
        })
        .collect();
    let existing = match std::fs::read_to_string(&out) {
        Ok(text) => {
            CsFile::parse(&text)
                .with_context(|| format!("{} is not a cs file", out.display()))?
                .entry
        }
        Err(_) => Vec::new(),
    };
    let found = fresh.len();
    let merged = merge(existing, fresh);
    let model = args.model.as_deref().unwrap_or("any");
    if let Some(dir) = out.parent() {
        std::fs::create_dir_all(dir)?;
    }
    std::fs::write(&out, render(model, merged)?)
        .with_context(|| format!("cannot write {}", out.display()))?;
    println!(
        "{} journal lines, {} proposals (at least {} each; {} already in a cs file) -> {}",
        lines.len(),
        found,
        params.min_count,
        proposals.len() - found,
        out.display()
    );
    for line in &listed {
        println!("{line}");
    }
    println!("All flagged: review them before anything is applied.");
    Ok(())
}

#[cfg(test)]
mod tests;
