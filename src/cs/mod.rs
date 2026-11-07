//! Compensating Scaffolding (cs): what a given ASR model hears, mapped to
//! what was meant, for command-like matching only (docs/rfc/0002).
//!
//! A cs file lists *additional phrasings* of phrases that are already
//! configured: "yourself" heard for the sleep phrase "ears off". Nothing here
//! adds an action or rewrites dictated text. The phrase lists that matching
//! uses are extended with the `heard` of every active entry for their target
//! when they are built ([`apply_commands`], [`apply_actions`],
//! [`decide_variants`]); matching itself is unchanged, and with no entries
//! every list is exactly what it was.
//!
//! Where entries come from, in rising precedence (a later entry with the same
//! target and heard replaces an earlier one, `status = "rejected"` included):
//!
//! 1. built-in files embedded at build time from the repo's `cs/any/*.toml`,
//!    then `cs/<model>/*.toml` (see `build.rs`; adding a file needs no code);
//! 2. user files, `~/.config/ears/cs.d/*.toml` in name order, `any` files
//!    before model files.
//!
//! A file applies when its `meta.model` is `any`, or names the ASR model the
//! server runs (the first id of its `/v1/models`, resolved once per VAD
//! start). Model ids compare by their last path segment, case-insensitively:
//! `Qwen/Qwen3-ASR-1.7B` is `qwen3-asr-1.7b`. A malformed file is logged and
//! skipped, never fatal.

pub mod check;
pub mod context;
pub mod migrate;
pub mod mine;
pub mod risk;

use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::sync::{Arc, RwLock};

use crate::commands::{normalize, VoiceCommands};

include!(concat!(env!("OUT_DIR"), "/cs_builtin.rs"));

/// How the ASR got it wrong.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FailureMode {
    /// Sounds the same, different words: "years off" for "ears off".
    Homophone,
    /// One word heard as two, or two as one: "back track".
    Split,
    /// Spelled out letter by letter: "v a d".
    Spelled,
    /// A name heard as a nearby one: "klinker" for "clanker".
    NameDrift,
    /// Part of the phrase lost: "clanker" for "hey clanker".
    Truncation,
    /// Unrelated words: "adsense" for "ears on".
    Hallucination,
    #[default]
    Other,
}

impl FailureMode {
    pub fn as_str(self) -> &'static str {
        match self {
            FailureMode::Homophone => "homophone",
            FailureMode::Split => "split",
            FailureMode::Spelled => "spelled",
            FailureMode::NameDrift => "name_drift",
            FailureMode::Truncation => "truncation",
            FailureMode::Hallucination => "hallucination",
            FailureMode::Other => "other",
        }
    }
}

/// Where an entry came from.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Source {
    /// Written by a person (or moved over from config.toml by `ears cs migrate`).
    #[default]
    Hand,
    /// Heard back from the ASR by `ears cs build`.
    Probe,
    /// Proposed from the utterance journal by `ears cs mine`.
    Mined,
}

impl Source {
    pub fn as_str(self) -> &'static str {
        match self {
            Source::Hand => "hand",
            Source::Probe => "probe",
            Source::Mined => "mined",
        }
    }
}

/// Whether an entry is applied.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Status {
    /// Applied.
    #[default]
    Active,
    /// Not applied; shown by `ears cs check` for review.
    Flagged,
    /// Not applied, and kept so tools do not propose it again.
    Rejected,
}

impl Status {
    pub fn as_str(self) -> &'static str {
        match self {
            Status::Active => "active",
            Status::Flagged => "flagged",
            Status::Rejected => "rejected",
        }
    }
}

/// One mishearing: `heard` stands for `meant` in the list `target`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Entry {
    /// The canonical phrase, exactly as configured.
    pub meant: String,
    /// The list it extends: "wake", "sleep", "enter"..., "run:<name>",
    /// "action:<name>", "decide:<option>".
    pub target: String,
    /// What the ASR writes (compared as `commands::normalize` does).
    pub heard: String,
    #[serde(default)]
    pub mode: FailureMode,
    #[serde(default)]
    pub source: Source,
    /// Times observed (probe hits or mined events).
    #[serde(default)]
    pub count: u32,
    /// Probe trials for `meant` (0 for hand and mined entries).
    #[serde(default)]
    pub trials: u32,
    /// 0..1, from `cs::risk::assess`.
    #[serde(default)]
    pub risk: f32,
    #[serde(default)]
    pub risk_reason: String,
    #[serde(default)]
    pub status: Status,
}

/// A cs file's `[meta]`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Meta {
    /// ASR model id, or "any".
    #[serde(default = "any_model")]
    pub model: String,
    #[serde(default = "commands_domain")]
    pub domain: String,
    #[serde(default)]
    pub generated: String,
    /// "hand", "ears cs build", "ears cs migrate", "ears cs mine".
    #[serde(default)]
    pub generator: String,
}

fn any_model() -> String {
    ANY.into()
}

fn commands_domain() -> String {
    "commands".into()
}

impl Default for Meta {
    fn default() -> Self {
        Self {
            model: any_model(),
            domain: commands_domain(),
            generated: String::new(),
            generator: String::new(),
        }
    }
}

/// One cs file.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct CsFile {
    #[serde(default)]
    pub meta: Meta,
    #[serde(default)]
    pub entry: Vec<Entry>,
}

impl CsFile {
    pub fn parse(text: &str) -> Result<Self, toml::de::Error> {
        toml::from_str(text)
    }

    pub fn to_toml(&self) -> Result<String, toml::ser::Error> {
        toml::to_string(self)
    }

    /// Whether it applies to `model` (None: unknown, only `any` files do).
    pub fn applies_to(&self, model: Option<&str>) -> bool {
        let file = model_key(&self.meta.model);
        file == ANY || model.is_some_and(|m| model_key(m) == file)
    }
}

/// `meta.model` of files for every model.
pub const ANY: &str = "any";

/// A model id as files name it: last path segment, lowercase.
pub fn model_key(id: &str) -> String {
    id.trim()
        .rsplit('/')
        .next()
        .unwrap_or_default()
        .to_lowercase()
}

/// List keys under `[commands]` that entries can extend. (`steer` is a
/// program, not phrases, and `keys` maps phrases to keys; neither is one.)
pub const COMMAND_TARGETS: &[&str] = &[
    "enter",
    "sign_off",
    "new_line",
    "undo",
    "back",
    "backtrack",
    "standby",
    "wake",
    "sleep",
    "literal",
];

/// An entry and the file it came from (`any/commands.toml` for a built-in,
/// the path for a user file).
#[derive(Debug, Clone, PartialEq)]
pub struct Located {
    pub file: String,
    pub entry: Entry,
}

/// The merged entries for one model.
#[derive(Debug, Clone, Default)]
pub struct Scaffold {
    model: Option<String>,
    entries: Vec<Located>,
}

/// A file read from somewhere, before merging.
#[derive(Debug, Clone)]
pub struct Loaded {
    /// Where it came from, for `ears cs check`.
    pub file: String,
    pub cs: CsFile,
}

impl Scaffold {
    /// No entries: every list as configured.
    pub fn empty() -> Self {
        Self::default()
    }

    /// Built-in and user files for `model` (None: unknown model, only `any`
    /// files apply).
    pub fn load(model: Option<&str>) -> Scaffold {
        Self::load_from(user_dir().as_deref(), model)
    }

    /// As [`Scaffold::load`], with user files from `dir`.
    pub fn load_from(dir: Option<&Path>, model: Option<&str>) -> Scaffold {
        let user = dir.map(read_dir).unwrap_or_default();
        Self::merge(builtin_files(), user, model)
    }

    /// Merge built-in files, then user files over them; within each, `any`
    /// files before model files, otherwise in the order given. Files that do
    /// not apply to `model` are left out.
    pub fn merge(builtin: Vec<Loaded>, user: Vec<Loaded>, model: Option<&str>) -> Scaffold {
        let mut scaffold = Scaffold {
            model: model.map(str::to_string),
            entries: Vec::new(),
        };
        for layer in [builtin, user] {
            let (any, specific): (Vec<_>, Vec<_>) = layer
                .into_iter()
                .filter(|f| f.cs.applies_to(model))
                .partition(|f| model_key(&f.cs.meta.model) == ANY);
            for file in any.into_iter().chain(specific) {
                for entry in file.cs.entry {
                    scaffold.insert(&file.file, entry);
                }
            }
        }
        scaffold
    }

    fn insert(&mut self, file: &str, mut entry: Entry) {
        entry.target = canonical_target(&entry.target);
        if normalize(&entry.heard).is_empty() || entry.target.is_empty() {
            tracing::warn!(file, ?entry, "cs entry without heard or target; skipped");
            return;
        }
        let located = Located {
            file: file.to_string(),
            entry,
        };
        let same = |l: &Located| {
            l.entry.target == located.entry.target
                && normalize(&l.entry.heard) == normalize(&located.entry.heard)
        };
        match self.entries.iter_mut().find(|l| same(l)) {
            Some(existing) => *existing = located,
            None => self.entries.push(located),
        }
    }

    /// The model it was loaded for (None: unknown).
    pub fn model(&self) -> Option<&str> {
        self.model.as_deref()
    }

    /// The `heard` of every active entry for `target`.
    pub fn variants(&self, target: &str) -> Vec<&str> {
        let target = canonical_target(target);
        self.entries
            .iter()
            .map(|l| &l.entry)
            .filter(|e| e.status == Status::Active && e.target == target)
            .map(|e| e.heard.as_str())
            .collect()
    }

    /// Every merged entry, whatever its status.
    pub fn entries(&self) -> impl Iterator<Item = &Entry> {
        self.entries.iter().map(|l| &l.entry)
    }

    /// Every merged entry with the file it came from.
    pub fn located(&self) -> impl Iterator<Item = &Located> {
        self.entries.iter()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
}

/// Targets compare trimmed; a decide option normalized ("decide:Deploy
/// now!" is "decide:deploy now").
fn canonical_target(target: &str) -> String {
    let target = target.trim();
    match target.strip_prefix("decide:") {
        Some(option) => format!("decide:{}", normalize(option)),
        None => target.to_string(),
    }
}

/// `~/.config/ears/cs.d`.
pub fn user_dir() -> Option<PathBuf> {
    directories::ProjectDirs::from("com", "heiervang", "ears").map(|d| d.config_dir().join("cs.d"))
}

/// The embedded defaults. A malformed one is logged and skipped (a test
/// keeps that from shipping).
pub fn builtin_files() -> Vec<Loaded> {
    BUILTIN
        .iter()
        .filter_map(|(name, text)| match CsFile::parse(text) {
            Ok(cs) => Some(Loaded {
                file: format!("builtin:{name}"),
                cs,
            }),
            Err(error) => {
                tracing::warn!(file = name, %error, "built-in cs file does not parse; skipped");
                None
            }
        })
        .collect()
}

/// Every `*.toml` in `dir`, in name order. A missing directory is none; a
/// file that cannot be read or parsed is logged and skipped.
pub fn read_dir(dir: &Path) -> Vec<Loaded> {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut paths: Vec<PathBuf> = entries
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|e| e == "toml") && p.is_file())
        .collect();
    paths.sort();
    paths
        .into_iter()
        .filter_map(|path| {
            let parsed = std::fs::read_to_string(&path)
                .map_err(|e| e.to_string())
                .and_then(|t| CsFile::parse(&t).map_err(|e| e.to_string()));
            match parsed {
                Ok(cs) => Some(Loaded {
                    file: path.display().to_string(),
                    cs,
                }),
                Err(error) => {
                    tracing::warn!(file = %path.display(), %error, "cs file skipped");
                    None
                }
            }
        })
        .collect()
}

static ACTIVE: RwLock<Option<Arc<Scaffold>>> = RwLock::new(None);

/// Make `scaffold` the one list builders use from now on (VAD start).
pub fn set_active(scaffold: Scaffold) {
    if let Ok(mut active) = ACTIVE.write() {
        *active = Some(Arc::new(scaffold));
    }
}

/// The scaffold set at VAD start; empty before that.
pub fn active() -> Arc<Scaffold> {
    ACTIVE
        .read()
        .ok()
        .and_then(|a| a.clone())
        .unwrap_or_default()
}

/// Append each variant not already in `list` (compared normalized).
fn extend(list: &mut Vec<String>, variants: Vec<&str>) {
    for variant in variants {
        let said = normalize(variant);
        if !list.iter().any(|p| normalize(p) == said) {
            list.push(variant.to_string());
        }
    }
}

/// The `[commands]` lists extended with their active variants: the phrase
/// lists by key, run commands by `run:<name>`. The wake detector is built
/// from the standby, wake and sleep lists, so it takes them from here.
pub fn apply_commands(mut commands: VoiceCommands, scaffold: &Scaffold) -> VoiceCommands {
    let c = &mut commands;
    for (target, list) in [
        ("enter", &mut c.enter),
        ("sign_off", &mut c.sign_off),
        ("new_line", &mut c.new_line),
        ("undo", &mut c.undo),
        ("back", &mut c.back),
        ("backtrack", &mut c.backtrack),
        ("standby", &mut c.standby),
        ("wake", &mut c.wake),
        ("sleep", &mut c.sleep),
        ("literal", &mut c.literal),
    ] {
        extend(list, scaffold.variants(target));
    }
    for entry in scaffold.entries() {
        if entry.status != Status::Active {
            continue;
        }
        if let Some(name) = entry.target.strip_prefix("run:") {
            let heard = commands.run.heard_as.entry(name.to_string()).or_default();
            extend(heard, vec![entry.heard.as_str()]);
        }
    }
    commands
}

/// Command-mode actions with their active `action:<name>` variants added to
/// their phrasings.
pub fn apply_actions(actions: &mut [crate::command_mode::Action], scaffold: &Scaffold) {
    for action in actions {
        extend(
            &mut action.text,
            scaffold.variants(&format!("action:{}", action.name)),
        );
    }
}

/// For `ears decide`: each active `decide:<option>` variant (normalized)
/// with the index of its option.
pub fn decide_variants(options: &[String], scaffold: &Scaffold) -> Vec<(String, usize)> {
    let mut heard: Vec<(String, usize)> = Vec::new();
    for (i, option) in options.iter().enumerate() {
        for variant in scaffold.variants(&format!("decide:{option}")) {
            let said = normalize(variant);
            if !heard.iter().any(|(h, _)| *h == said) {
                heard.push((said, i));
            }
        }
    }
    heard
}

#[cfg(test)]
mod tests;
