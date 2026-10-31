//! Spoken commands in VAD mode.
//!
//! A command is recognized only when it is the *whole* utterance: you say
//! "Over.", pause, and that utterance presses Enter. The same word inside a
//! sentence ("it's over now") is ordinary text. VAD already cuts speech at
//! pauses, so the pause is the boundary. To type a command word as text,
//! prefix it with the literal word: "literal over" types "over".
//!
//! Run commands (`[commands.run.<name>]`) start a program instead of pressing
//! a key: "hey friend" runs `friend wake`. They have their own switch, are
//! heard in standby too, and one marked `prefix` also matches an utterance
//! that starts with its phrase, passing the rest of it to the program. One
//! with `while_running` is heard only while the process in that pid file
//! lives: "friend" alone wakes friend only while friend is on screen.

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// `[commands]` config section.
///
/// Phrase lists add to the built-in phrases instead of replacing them:
/// `sleep = ["fuck off clanker"]` keeps "ears off" working too.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct VoiceCommands {
    /// Recognize commands at all (default: false).
    #[serde(default)]
    pub enabled: bool,
    /// Phrases that press Enter.
    #[serde(default = "default_enter", deserialize_with = "more_enter")]
    pub enter: Vec<String>,
    /// Phrases that press Enter and then put VAD in standby ("over and
    /// out").
    #[serde(default = "default_sign_off", deserialize_with = "more_sign_off")]
    pub sign_off: Vec<String>,
    /// Phrases that press Shift+Enter (a new line without sending, in chat
    /// boxes and agent prompts).
    #[serde(default = "default_new_line", deserialize_with = "more_new_line")]
    pub new_line: Vec<String>,
    /// Phrases that delete the last dictated utterance (say it again to
    /// delete the one before). Only in the window it was typed into.
    #[serde(default = "default_undo", deserialize_with = "more_undo")]
    pub undo: Vec<String>,
    /// Words that, followed by a number from 1 to 10 ("back 3", "back
    /// three"), delete that many of the last dictated words.
    #[serde(default = "default_back", deserialize_with = "more_back")]
    pub back: Vec<String>,
    /// Phrases that delete the last dictated sentence.
    #[serde(default = "default_backtrack", deserialize_with = "more_backtrack")]
    pub backtrack: Vec<String>,
    /// Phrases that switch VAD between typing and standby, where it only
    /// listens for these phrases (see `crate::wake`).
    #[serde(default = "default_standby", deserialize_with = "more_standby")]
    pub standby: Vec<String>,
    /// Phrases that resume typing from standby ("ears on").
    #[serde(default = "default_wake", deserialize_with = "more_wake")]
    pub wake: Vec<String>,
    /// Phrases that put VAD in standby ("ears off").
    #[serde(default = "default_sleep", deserialize_with = "more_sleep")]
    pub sleep: Vec<String>,
    /// Words that, said first, type the rest of the utterance as text.
    #[serde(default = "default_literal", deserialize_with = "more_literal")]
    pub literal: Vec<String>,
    /// Phrases that press a key combination, e.g. `background = "ctrl+b"`.
    /// See [`keycodes`] for the key names.
    #[serde(default)]
    pub keys: BTreeMap<String, String>,
    /// How long a recognized command is shown in the accept colour (the
    /// ghost's frozen colour) before its key is pressed, in milliseconds.
    /// 0: press at once.
    #[serde(default = "default_accept_ms")]
    pub accept_ms: u64,
    /// Steering: a command (argv) that is offered each utterance that is not
    /// one of the commands above, with the utterance as its last argument,
    /// e.g. `["voice-cmd", "handle"]`. Exit status 0 means it acted on it
    /// (clicked, switched workspace...), and the utterance is not typed.
    /// Empty: off.
    #[serde(default)]
    pub steer: Vec<String>,
    /// How long steering may take before the utterance is typed anyway, in
    /// milliseconds.
    #[serde(default = "default_steer_timeout_ms")]
    pub steer_timeout_ms: u64,
    /// Type only when a text field has focus (asked from the ears fcitx5
    /// addon). Speech elsewhere goes to steering, or nowhere. When the addon
    /// can't be asked, typing goes ahead as before.
    #[serde(default)]
    pub text_fields_only: bool,
    /// Window classes that always get typing, for apps that never tell the
    /// input method about their text fields.
    #[serde(default)]
    pub type_anyway: Vec<String>,
    /// Languages that put VAD in standby when heard, e.g. `["sv"]`: speech
    /// you don't dictate in is most likely from the background. The
    /// utterance is not typed. See `crate::wake::sounds_like`.
    #[serde(default)]
    pub sleep_on_language: Vec<String>,
    /// Phrases that run a program (`[commands.run.<name>]`), merged over
    /// the built-in ones. Independent of `enabled`.
    #[serde(default)]
    pub run: RunCommands,
}

/// What to do with an utterance that is not an ears command.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Route {
    /// Type it.
    Type,
    /// Nowhere to type it: drop it.
    Drop,
}

impl VoiceCommands {
    /// Whether to type an utterance that steering did not take, given
    /// whether a text field has focus (None: unknown) and the focused
    /// window's class.
    pub fn route(&self, text_field: Option<bool>, class: Option<&str>) -> Route {
        let exempt = class.is_some_and(|class| {
            self.type_anyway
                .iter()
                .any(|c| c.eq_ignore_ascii_case(class))
        });
        if self.text_fields_only && text_field == Some(false) && !exempt {
            Route::Drop
        } else {
            Route::Type
        }
    }

    /// Offer `heard` to the steering command. True when it acted.
    pub async fn steer(&self, heard: &str) -> bool {
        let Some((program, args)) = self.steer.split_first() else {
            return false;
        };
        let child = tokio::process::Command::new(program)
            .args(args)
            .arg(heard)
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .kill_on_drop(true)
            .spawn();
        let mut child = match child {
            Ok(child) => child,
            Err(error) => {
                tracing::warn!(%error, program, "Steering command did not start");
                return false;
            }
        };
        let limit = std::time::Duration::from_millis(self.steer_timeout_ms);
        match tokio::time::timeout(limit, child.wait()).await {
            Ok(Ok(status)) => status.success(),
            Ok(Err(error)) => {
                tracing::warn!(%error, "Steering command failed");
                false
            }
            Err(_) => {
                tracing::warn!("Steering command took over {:?}; typing instead", limit);
                false
            }
        }
    }
}

/// `[commands.run]`: programs started by a spoken phrase.
///
/// Holds only what the config file says, so saving the config does not
/// write the built-ins into it; [`RunCommands::resolved`] merges them.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RunCommands {
    /// Recognize run commands at all (default: true), even when
    /// `[commands] enabled` is false.
    #[serde(default = "default_run_enabled")]
    pub enabled: bool,
    /// Entries by name. One named like a built-in overrides only the fields
    /// it sets; `enabled = false` removes it.
    #[serde(flatten)]
    pub entries: BTreeMap<String, RunEntry>,
}

/// One `[commands.run.<name>]` table, as written in the config.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct RunEntry {
    /// False removes the entry (a built-in included).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub enabled: Option<bool>,
    /// Phrases that run it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub phrases: Option<Vec<String>>,
    /// Program and arguments, e.g. `["friend", "wake"]`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub command: Option<Vec<String>>,
    /// Also match an utterance that starts with a phrase, appending the
    /// rest of it to the command.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub prefix: Option<bool>,
    /// Flag put before the rest, e.g. "--say". Empty: the rest alone.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rest_flag: Option<String>,
    /// Pid file: heard only while that process runs. `~/` and
    /// `$XDG_RUNTIME_DIR` are expanded. Empty: always heard.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub while_running: Option<String>,
    /// Also leave standby, like a wake phrase: "centurion" wakes ears and
    /// hands the talking stick to the centurion agent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resume: Option<bool>,
}

/// A resolved run command: built-in and config merged.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RunCommand {
    pub name: String,
    pub phrases: Vec<String>,
    pub command: Vec<String>,
    pub prefix: bool,
    pub rest_flag: Option<String>,
    pub while_running: Option<String>,
    pub resume: bool,
}

fn default_run_enabled() -> bool {
    true
}

impl Default for RunCommands {
    fn default() -> Self {
        Self {
            enabled: default_run_enabled(),
            entries: BTreeMap::new(),
        }
    }
}

/// The built-in phrases followed by the configured ones not already there.
fn with_defaults(mut phrases: Vec<String>, configured: Vec<String>) -> Vec<String> {
    for phrase in configured {
        if !phrases.iter().any(|p| normalize(p) == normalize(&phrase)) {
            phrases.push(phrase);
        }
    }
    phrases
}

/// `more_X`: deserialize a phrase list for field X on top of `default_X`.
macro_rules! accumulate {
    ($($more:ident => $default:ident),* $(,)?) => {$(
        fn $more<'de, D: serde::Deserializer<'de>>(de: D) -> Result<Vec<String>, D::Error> {
            Ok(with_defaults($default(), Vec::<String>::deserialize(de)?))
        }
    )*};
}

accumulate! {
    more_enter => default_enter,
    more_sign_off => default_sign_off,
    more_new_line => default_new_line,
    more_undo => default_undo,
    more_back => default_back,
    more_backtrack => default_backtrack,
    more_standby => default_standby,
    more_wake => default_wake,
    more_sleep => default_sleep,
    more_literal => default_literal,
}

/// Built-in run commands: summon and dismiss the friend desktop companion.
pub fn builtin_run_commands() -> Vec<RunCommand> {
    let strings = |list: &[&str]| list.iter().map(|s| s.to_string()).collect();
    vec![
        RunCommand {
            name: "friend-wake".into(),
            phrases: strings(&["hey friend", "hi friend", "okay friend", "ok friend"]),
            command: strings(&["friend", "wake"]),
            prefix: true,
            rest_flag: Some("--say".into()),
            while_running: None,
            resume: false,
        },
        // Addressed by name while on screen: friend listens even if another
        // agent holds the talking stick (`--take`).
        RunCommand {
            name: "friend-name".into(),
            phrases: strings(&["friend"]),
            command: strings(&["friend", "wake", "--take"]),
            prefix: true,
            rest_flag: Some("--say".into()),
            while_running: Some("$XDG_RUNTIME_DIR/haios-mascot.pid".into()),
            resume: false,
        },
        RunCommand {
            name: "friend-dismiss".into(),
            phrases: strings(&["bye friend", "goodbye friend", "bye bye friend"]),
            command: strings(&["friend", "dismiss"]),
            prefix: false,
            rest_flag: None,
            while_running: None,
            resume: false,
        },
    ]
}

impl RunCommands {
    /// The run commands in effect: the built-ins with config entries merged
    /// field by field over them, minus disabled ones and ones without
    /// phrases or a program. Empty when `enabled` is false.
    pub fn resolved(&self) -> Vec<RunCommand> {
        if !self.enabled {
            return Vec::new();
        }
        let mut all = builtin_run_commands();
        for (name, entry) in &self.entries {
            let i = match all.iter().position(|r| &r.name == name) {
                Some(i) => i,
                None => {
                    all.push(RunCommand {
                        name: name.clone(),
                        phrases: Vec::new(),
                        command: Vec::new(),
                        prefix: false,
                        rest_flag: None,
                        while_running: None,
                        resume: false,
                    });
                    all.len() - 1
                }
            };
            if entry.enabled == Some(false) {
                all.remove(i);
                continue;
            }
            let run = &mut all[i];
            if let Some(phrases) = &entry.phrases {
                run.phrases = phrases.clone();
            }
            if let Some(command) = &entry.command {
                run.command = command.clone();
            }
            if let Some(prefix) = entry.prefix {
                run.prefix = prefix;
            }
            if let Some(flag) = &entry.rest_flag {
                run.rest_flag = Some(flag.clone()).filter(|f| !f.is_empty());
            }
            if let Some(pidfile) = &entry.while_running {
                run.while_running = Some(pidfile.clone()).filter(|f| !f.is_empty());
            }
            if let Some(resume) = entry.resume {
                run.resume = resume;
            }
        }
        all.retain(|r| !r.command.is_empty() && r.phrases.iter().any(|p| !normalize(p).is_empty()));
        all
    }
}

fn default_enter() -> Vec<String> {
    vec!["enter".into()]
}

fn default_new_line() -> Vec<String> {
    vec!["new line".into()]
}

fn default_sign_off() -> Vec<String> {
    vec!["over and out".into()]
}

fn default_undo() -> Vec<String> {
    vec!["scratch that".into()]
}

fn default_back() -> Vec<String> {
    vec!["back".into()]
}

fn default_backtrack() -> Vec<String> {
    vec!["backtrack".into(), "back track".into()]
}

/// Most words "back N" deletes.
pub const BACK_MAX_WORDS: usize = 10;

fn default_standby() -> Vec<String> {
    vec!["ears vad".into(), "ears v a d".into()]
}

fn default_wake() -> Vec<String> {
    vec!["ears on".into()]
}

fn default_sleep() -> Vec<String> {
    vec!["ears off".into()]
}

fn default_literal() -> Vec<String> {
    vec!["literal".into()]
}

fn default_steer_timeout_ms() -> u64 {
    800
}

fn default_accept_ms() -> u64 {
    200
}

impl Default for VoiceCommands {
    fn default() -> Self {
        Self {
            enabled: false,
            enter: default_enter(),
            sign_off: default_sign_off(),
            new_line: default_new_line(),
            undo: default_undo(),
            back: default_back(),
            backtrack: default_backtrack(),
            standby: default_standby(),
            wake: default_wake(),
            sleep: default_sleep(),
            literal: default_literal(),
            keys: BTreeMap::new(),
            accept_ms: default_accept_ms(),
            steer: Vec::new(),
            steer_timeout_ms: default_steer_timeout_ms(),
            text_fields_only: false,
            type_anyway: Vec::new(),
            sleep_on_language: Vec::new(),
            run: RunCommands::default(),
        }
    }
}

/// What an utterance asks for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Command {
    /// Press Enter.
    Enter,
    /// Press Enter, then stand by.
    SignOff,
    /// Press Shift+Enter.
    NewLine,
    /// Delete the last dictated utterance.
    Undo,
    /// Delete this many of the last dictated words (1 to 10).
    Back(usize),
    /// Delete the last dictated sentence.
    Backtrack,
    /// Press a key combination from `[commands.keys]`, e.g. "ctrl+b".
    Keys(String),
    /// Type this text instead of interpreting it.
    Literal(String),
    /// Start a program from `[commands.run]`.
    Run {
        /// The entry's name, e.g. "friend-wake".
        name: String,
        /// Program and arguments, the rest of a prefix match included.
        argv: Vec<String>,
    },
}

impl Command {
    /// Short name for logs and events.
    pub fn name(&self) -> &'static str {
        match self {
            Command::Enter => "enter",
            Command::SignOff => "sign_off",
            Command::NewLine => "new_line",
            Command::Undo => "undo",
            Command::Back(_) => "back",
            Command::Backtrack => "backtrack",
            Command::Keys(_) => "keys",
            Command::Literal(_) => "literal",
            Command::Run { .. } => "run",
        }
    }

    /// Name with its key combination, for logs and events: "enter",
    /// "keys:ctrl+b". A run command is its entry's name: "friend-wake".
    pub fn label(&self) -> String {
        match self {
            Command::Keys(combo) => format!("keys:{combo}"),
            Command::Run { name, .. } => name.clone(),
            Command::Back(n) => format!("back:{n}"),
            other => other.name().to_string(),
        }
    }

    /// Whether the command presses a key or runs a program (rather than
    /// typing text).
    pub fn presses_key(&self) -> bool {
        !matches!(self, Command::Literal(_))
    }
}

/// Lowercase, punctuation to spaces, whitespace collapsed: "Over." and
/// " over! " both become "over".
pub fn normalize(text: &str) -> String {
    text.chars()
        .map(|c| {
            if c.is_alphanumeric() {
                c.to_lowercase().next().unwrap_or(c)
            } else {
                ' '
            }
        })
        .collect::<String>()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

impl VoiceCommands {
    /// The command this whole utterance is, if any. Run commands are
    /// recognized even when `enabled` is false.
    pub fn parse(&self, transcript: &str) -> Option<Command> {
        let said = normalize(transcript);
        if said.is_empty() {
            return None;
        }
        if self.enabled {
            if let Some(command) = self.parse_keys(transcript, &said) {
                return Some(command);
            }
        }
        self.parse_run(transcript, &said)
    }

    /// The run command this utterance is, if any: a whole phrase, or for a
    /// `prefix` entry, a phrase at the start followed by more words.
    fn parse_run(&self, transcript: &str, said: &str) -> Option<Command> {
        for run in self.run.resolved() {
            if run
                .while_running
                .as_deref()
                .is_some_and(|f| !process_running(f))
            {
                continue;
            }
            for phrase in run.phrases.iter().map(|p| normalize(p)) {
                if phrase.is_empty() {
                    continue;
                }
                let mut argv = run.command.clone();
                if said == phrase {
                    return Some(Command::Run {
                        name: run.name,
                        argv,
                    });
                }
                if !run.prefix || !said.starts_with(&format!("{phrase} ")) {
                    continue;
                }
                argv.extend(run.rest_flag.clone());
                argv.push(rest_after(transcript, &phrase, said));
                return Some(Command::Run {
                    name: run.name,
                    argv,
                });
            }
        }
        None
    }

    /// Commands behind `enabled`: keys, deletes and literal.
    fn parse_keys(&self, transcript: &str, said: &str) -> Option<Command> {
        let is = |phrases: &[String]| phrases.iter().any(|p| normalize(p) == said);
        if is(&self.enter) {
            return Some(Command::Enter);
        }
        if is(&self.sign_off) {
            return Some(Command::SignOff);
        }
        if is(&self.new_line) {
            return Some(Command::NewLine);
        }
        if is(&self.undo) {
            return Some(Command::Undo);
        }
        if is(&self.backtrack) {
            return Some(Command::Backtrack);
        }
        for word in &self.back {
            let prefix = normalize(word);
            if let Some(n) = said
                .strip_prefix(&prefix)
                .filter(|_| !prefix.is_empty())
                .and_then(|rest| rest.strip_prefix(' '))
                .and_then(count)
            {
                return Some(Command::Back(n));
            }
        }
        if let Some((_, combo)) = self
            .keys
            .iter()
            .find(|(phrase, combo)| normalize(phrase) == said && key_sequence(combo).is_some())
        {
            return Some(Command::Keys(combo.clone()));
        }
        for word in &self.literal {
            let prefix = normalize(word);
            if prefix.is_empty() {
                continue;
            }
            let prefix_words = prefix.split(' ').count();
            let words: Vec<&str> = transcript.split_whitespace().collect();
            let head = normalize(&words[..prefix_words.min(words.len())].join(" "));
            if head == prefix && words.len() > prefix_words {
                return Some(Command::Literal(words[prefix_words..].join(" ")));
            }
        }
        None
    }
}

/// Start a run command's program detached: in a session of its own, with
/// no stdin, stdout or stderr, reaped by a thread so the engine never waits
/// for it. Returns its pid.
pub fn spawn_detached(argv: &[String]) -> std::io::Result<u32> {
    use std::os::unix::process::CommandExt;
    use std::process::{Command, Stdio};
    let (program, args) = argv
        .split_first()
        .ok_or_else(|| std::io::Error::new(std::io::ErrorKind::InvalidInput, "empty command"))?;
    let mut cmd = Command::new(program);
    cmd.args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    // SAFETY: setsid is async-signal-safe and touches no parent state.
    unsafe {
        cmd.pre_exec(|| {
            if libc::setsid() == -1 {
                return Err(std::io::Error::last_os_error());
            }
            Ok(())
        });
    }
    let mut child = cmd.spawn()?;
    let pid = child.id();
    std::thread::spawn(move || child.wait());
    Ok(pid)
}

/// Whether the process whose pid is in `pidfile` is alive. A missing or
/// stale file (the process crashed) is not running.
fn process_running(pidfile: &str) -> bool {
    let path = if let Some(rest) = pidfile.strip_prefix("~/") {
        std::env::var("HOME").map(|h| format!("{h}/{rest}"))
    } else if let Some(rest) = pidfile.strip_prefix("$XDG_RUNTIME_DIR") {
        std::env::var("XDG_RUNTIME_DIR").map(|d| format!("{d}{rest}"))
    } else {
        Ok(pidfile.to_string())
    };
    let Ok(path) = path else { return false };
    std::fs::read_to_string(path)
        .ok()
        .and_then(|s| s.trim().parse::<u32>().ok())
        .is_some_and(|pid| pid > 0 && std::path::Path::new(&format!("/proc/{pid}")).exists())
}

/// What `transcript` says after `phrase` (normalized, and known to start
/// `said`), as heard: "Hey, friend, how are you?" after "hey friend" is
/// "how are you?". Normalized if the words do not split there.
fn rest_after(transcript: &str, phrase: &str, said: &str) -> String {
    let words: Vec<&str> = transcript.split_whitespace().collect();
    for k in 1..words.len() {
        let head = normalize(&words[..k].join(" "));
        if head == phrase {
            return words[k..].join(" ");
        }
        if head.len() > phrase.len() {
            break;
        }
    }
    said[phrase.len()..].trim().to_string()
}

/// A spoken count from 1 to [`BACK_MAX_WORDS`]: digits, number words, and
/// the homophones ASR writes for them ("to", "for").
fn count(word: &str) -> Option<usize> {
    let n = match word {
        "one" | "won" => 1,
        "two" | "to" | "too" => 2,
        "three" => 3,
        "four" | "for" => 4,
        "five" => 5,
        "six" => 6,
        "seven" => 7,
        "eight" => 8,
        "nine" => 9,
        "ten" => 10,
        digits => digits.parse().ok()?,
    };
    (1..=BACK_MAX_WORDS).contains(&n).then_some(n)
}

/// Linux input keycodes for a combination like "ctrl+b" or "alt+left",
/// modifiers first, as `ydotool key` takes them. None if any part is not a
/// known key name: ctrl, shift, alt, super, a-z, 0-9, f1-f12, enter, tab,
/// esc, space, backspace, delete, up, down, left, right, home, end,
/// pageup, pagedown.
pub fn keycodes(combo: &str) -> Option<Vec<u16>> {
    const LETTERS: &str = "abcdefghijklmnopqrstuvwxyz";
    const LETTER_CODES: [u16; 26] = [
        30, 48, 46, 32, 18, 33, 34, 35, 23, 36, 37, 38, 50, 49, 24, 25, 16, 19, 31, 20, 22, 47, 17,
        45, 21, 44,
    ];
    let code = |name: &str| -> Option<u16> {
        let name = name.trim().to_ascii_lowercase();
        if name.len() == 1 {
            let c = name.chars().next()?;
            if let Some(i) = LETTERS.find(c) {
                return Some(LETTER_CODES[i]);
            }
            if let Some(d) = c.to_digit(10) {
                return Some(if d == 0 { 11 } else { d as u16 + 1 });
            }
            return None;
        }
        if let Some(n) = name.strip_prefix('f').and_then(|n| n.parse::<u16>().ok()) {
            return match n {
                1..=10 => Some(58 + n),
                11 => Some(87),
                12 => Some(88),
                _ => None,
            };
        }
        Some(match name.as_str() {
            "ctrl" | "control" => 29,
            "shift" => 42,
            "alt" => 56,
            "super" | "meta" => 125,
            "enter" | "return" => 28,
            "tab" => 15,
            "esc" | "escape" => 1,
            "space" => 57,
            "backspace" => 14,
            "delete" | "del" => 111,
            "up" => 103,
            "down" => 108,
            "left" => 105,
            "right" => 106,
            "home" => 102,
            "end" => 107,
            "pageup" => 104,
            "pagedown" => 109,
            _ => return None,
        })
    };
    let codes = combo.split('+').map(code).collect::<Option<Vec<_>>>()?;
    (!codes.is_empty()).then_some(codes)
}

/// Combinations pressed one after another: "ctrl+a backspace", or with a
/// count, "ctrl+u*40" (e.g. clear every line of an input: in readline and
/// Claude Code another ctrl+u at a line start eats the line break above).
/// None if any part is not a key combination ([`keycodes`]).
pub fn key_sequence(spec: &str) -> Option<Vec<Vec<u16>>> {
    const MAX_REPEAT: usize = 200;
    let mut sequence = Vec::new();
    for part in spec.split([' ', ',']).filter(|p| !p.is_empty()) {
        let (combo, times) = match part.split_once('*') {
            Some((combo, n)) => (combo, n.parse::<usize>().ok()?),
            None => (part, 1),
        };
        if !(1..=MAX_REPEAT).contains(&times) {
            return None;
        }
        let codes = keycodes(combo)?;
        sequence.extend(std::iter::repeat_n(codes, times));
    }
    (!sequence.is_empty()).then_some(sequence)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn key_sequences() {
        assert_eq!(key_sequence("ctrl+u"), Some(vec![vec![29, 22]]));
        assert_eq!(key_sequence("ctrl+u*3").unwrap().len(), 3);
        assert_eq!(
            key_sequence("ctrl+a, backspace"),
            Some(vec![vec![29, 30], vec![14]])
        );
        assert_eq!(key_sequence("ctrl+u*0"), None);
        assert_eq!(key_sequence("ctrl+u*x"), None);
        assert_eq!(key_sequence("ctrl+nope"), None);
        assert_eq!(key_sequence(""), None);
    }

    #[test]
    fn speech_outside_text_fields_is_dropped_only_when_asked() {
        let mut commands = VoiceCommands {
            type_anyway: vec!["Steam".into()],
            ..Default::default()
        };
        assert_eq!(commands.route(Some(false), Some("hover")), Route::Type);
        commands.text_fields_only = true;
        assert_eq!(commands.route(Some(false), Some("hover")), Route::Drop);
        assert_eq!(commands.route(Some(true), Some("hover")), Route::Type);
        assert_eq!(
            commands.route(None, Some("hover")),
            Route::Type,
            "unknown: type"
        );
        assert_eq!(commands.route(Some(false), Some("steam")), Route::Type);
    }

    #[tokio::test]
    async fn steering_takes_an_utterance_when_its_command_exits_zero() {
        let dir = tempfile::tempdir().unwrap();
        let log = dir.path().join("heard");
        let script = dir.path().join("steer");
        std::fs::write(
            &script,
            format!(
                "#!/bin/sh\nprintf '%s' \"$1\" > {}\n[ \"$1\" = 'click search' ]\n",
                log.display()
            ),
        )
        .unwrap();
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();
        let mut commands = VoiceCommands {
            steer: vec![script.display().to_string()],
            ..Default::default()
        };
        assert!(commands.steer("click search").await);
        assert_eq!(std::fs::read_to_string(&log).unwrap(), "click search");
        assert!(!commands.steer("hello there").await);
        commands.steer = vec!["sleep".into()];
        commands.steer_timeout_ms = 50;
        assert!(!commands.steer("5").await, "a slow command loses to typing");
        commands.steer = vec!["/nonexistent/steer".into()];
        assert!(!commands.steer("x").await);
    }

    #[test]
    fn configured_phrases_add_to_the_built_in_ones() {
        let commands: VoiceCommands = toml::from_str(
            r#"
            enabled = true
            sleep = ["fuck off clanker", "Ears off."]
            enter = ["over"]
            "#,
        )
        .unwrap();
        assert_eq!(commands.sleep, ["ears off", "fuck off clanker"]);
        assert_eq!(commands.enter, ["enter", "over"]);
        assert_eq!(commands.wake, default_wake());
        assert_eq!(commands.parse("Enter."), Some(Command::Enter));
        assert_eq!(commands.parse("Over."), Some(Command::Enter));
    }

    fn over() -> VoiceCommands {
        VoiceCommands {
            enabled: true,
            enter: vec!["over".into(), "enter".into()],
            ..VoiceCommands::default()
        }
    }

    #[test]
    fn only_key_commands_press_keys() {
        let commands = over();
        assert!(commands.parse("Over.").unwrap().presses_key());
        assert!(!commands.parse("literal over").unwrap().presses_key());
        assert_eq!(VoiceCommands::default().accept_ms, 200);
    }

    #[test]
    fn key_phrases_press_their_combination() {
        let mut commands = over();
        commands.keys.insert("background".into(), "ctrl+b".into());
        commands.keys.insert("broken".into(), "ctrl+nope".into());
        assert_eq!(
            commands.parse("Background."),
            Some(Command::Keys("ctrl+b".into()))
        );
        assert!(commands.parse("Background.").unwrap().presses_key());
        assert_eq!(commands.parse("Run it in the background."), None);
        assert_eq!(commands.parse("Broken."), None, "bad combos are dictation");
    }

    #[test]
    fn key_combinations_map_to_keycodes() {
        assert_eq!(keycodes("ctrl+b"), Some(vec![29, 48]));
        assert_eq!(keycodes("Ctrl + Shift + T"), Some(vec![29, 42, 20]));
        assert_eq!(keycodes("alt+left"), Some(vec![56, 105]));
        assert_eq!(keycodes("f5"), Some(vec![63]));
        assert_eq!(keycodes("1"), Some(vec![2]));
        assert_eq!(keycodes("0"), Some(vec![11]));
        assert_eq!(keycodes("esc"), Some(vec![1]));
        assert_eq!(keycodes("ctrl+"), None);
        assert_eq!(keycodes("hyper+b"), None);
    }

    #[test]
    fn scratch_that_undoes() {
        let commands = over();
        assert_eq!(commands.parse("Scratch that."), Some(Command::Undo));
        assert_eq!(commands.parse("Scratch that line, please."), None);
        assert!(Command::Undo.presses_key());
    }

    #[test]
    fn disabled_recognizes_nothing() {
        assert_eq!(VoiceCommands::default().parse("Enter."), None);
    }

    #[test]
    fn whole_utterance_only() {
        let c = over();
        assert_eq!(c.parse("Over."), Some(Command::Enter));
        assert_eq!(c.parse("  over! "), Some(Command::Enter));
        assert_eq!(c.parse("Enter"), Some(Command::Enter));
        assert_eq!(c.parse("It's over now."), None);
        assert_eq!(c.parse("Over and out."), Some(Command::SignOff));
        assert_eq!(c.parse("Over and out of time."), None);
        assert_eq!(c.parse("Press enter to continue."), None);
    }

    #[test]
    fn back_takes_a_count_from_one_to_ten() {
        let c = over();
        assert_eq!(c.parse("Back 3."), Some(Command::Back(3)));
        assert_eq!(c.parse("Back, three."), Some(Command::Back(3)));
        assert_eq!(c.parse("back two"), Some(Command::Back(2)));
        assert_eq!(c.parse("Back to."), Some(Command::Back(2)));
        assert_eq!(c.parse("Back 10."), Some(Command::Back(10)));
        assert_eq!(c.parse("Back 11."), None);
        assert_eq!(c.parse("Back 0."), None);
        assert_eq!(c.parse("Back."), None, "a bare back is dictation");
        assert_eq!(c.parse("Back to the drawing board."), None);
        assert_eq!(c.parse("Backup 3."), None);
        assert!(Command::Back(3).presses_key());
        assert_eq!(Command::Back(3).label(), "back:3");
    }

    #[test]
    fn backtrack_phrase() {
        let c = over();
        assert_eq!(c.parse("Backtrack."), Some(Command::Backtrack));
        assert_eq!(c.parse("Back track!"), Some(Command::Backtrack));
        assert_eq!(c.parse("Let's backtrack a bit."), None);
    }

    #[test]
    fn new_line_phrase() {
        let c = over();
        assert_eq!(c.parse("New line."), Some(Command::NewLine));
        assert_eq!(c.parse("New-line"), Some(Command::NewLine));
        assert_eq!(c.parse("A new line of code."), None);
    }

    #[test]
    fn literal_types_the_rest_as_text() {
        let c = over();
        assert_eq!(
            c.parse("Literal over."),
            Some(Command::Literal("over.".into()))
        );
        assert_eq!(
            c.parse("literal, new line"),
            Some(Command::Literal("new line".into()))
        );
        assert_eq!(c.parse("Literal."), None, "nothing to type");
        assert_eq!(c.parse("Literally over."), None);
    }

    fn run(name: &str, argv: &[&str]) -> Option<Command> {
        Some(Command::Run {
            name: name.into(),
            argv: argv.iter().map(|s| s.to_string()).collect(),
        })
    }

    #[test]
    fn friend_phrases_run_by_default_even_with_commands_off() {
        let c = VoiceCommands::default();
        assert!(!c.enabled);
        assert_eq!(
            c.parse("Hey friend."),
            run("friend-wake", &["friend", "wake"])
        );
        assert_eq!(
            c.parse("Hey, friend!"),
            run("friend-wake", &["friend", "wake"])
        );
        assert_eq!(
            c.parse("OK friend"),
            run("friend-wake", &["friend", "wake"])
        );
        assert_eq!(
            c.parse("Okay, friend."),
            run("friend-wake", &["friend", "wake"])
        );
        assert_eq!(
            c.parse("Bye-bye, friend."),
            run("friend-dismiss", &["friend", "dismiss"])
        );
        assert_eq!(
            c.parse("Goodbye friend"),
            run("friend-dismiss", &["friend", "dismiss"])
        );
        assert_eq!(c.parse("Enter."), None, "keys stay behind enabled");
        assert!(c.parse("Hey friend.").unwrap().presses_key());
        assert_eq!(c.parse("Hey friend.").unwrap().label(), "friend-wake");
    }

    #[test]
    fn prefix_passes_the_rest_as_heard() {
        let c = VoiceCommands::default();
        assert_eq!(
            c.parse("Hey, friend, how are you?"),
            run("friend-wake", &["friend", "wake", "--say", "how are you?"])
        );
        assert_eq!(
            c.parse("hi friend what's the weather"),
            run(
                "friend-wake",
                &["friend", "wake", "--say", "what's the weather"]
            )
        );
        assert_eq!(
            c.parse("Hey friend..."),
            run("friend-wake", &["friend", "wake"])
        );
        assert_eq!(
            c.parse("My friend said hey friend."),
            None,
            "phrase must lead"
        );
        assert_eq!(c.parse("Hey friendly neighbour."), None, "whole words only");
        assert_eq!(
            c.parse("Bye friend, see you."),
            None,
            "dismiss is no prefix"
        );
    }

    #[test]
    fn run_entries_merge_over_the_builtins() {
        let toml = r#"
            [run.friend-wake]
            phrases = ["yo friend"]
            rest_flag = ""

            [run.friend-dismiss]
            enabled = false

            [run.music]
            phrases = ["play music"]
            command = ["playerctl", "play"]
        "#;
        let c: VoiceCommands = toml::from_str(toml).unwrap();
        assert_eq!(c.parse("Hey friend."), None, "phrases replaced");
        assert_eq!(
            c.parse("Yo friend, hello"),
            run("friend-wake", &["friend", "wake", "hello"]),
            "command and prefix kept, flag dropped"
        );
        assert_eq!(c.parse("Bye friend."), None, "built-in removed");
        assert_eq!(c.parse("Play music!"), run("music", &["playerctl", "play"]));
        assert_eq!(c.parse("Play music now"), None, "no prefix by default");

        let back = toml::to_string(&c).unwrap();
        assert!(
            !back.contains("\"bye friend\""),
            "built-ins are not saved: {back}"
        );
        assert_eq!(toml::from_str::<VoiceCommands>(&back).unwrap(), c);
    }

    #[test]
    fn friend_by_name_only_while_its_process_runs() {
        let dir = std::env::temp_dir().join(format!("ears-while-running-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let alive = dir.join("alive.pid");
        let stale = dir.join("stale.pid");
        std::fs::write(&alive, std::process::id().to_string()).unwrap();
        std::fs::write(&stale, "999999999").unwrap();
        let with = |pidfile: &std::path::Path| -> VoiceCommands {
            toml::from_str(&format!(
                "[run.friend-name]\nwhile_running = {:?}\n",
                pidfile.to_str().unwrap()
            ))
            .unwrap()
        };
        let on = with(&alive);
        assert_eq!(
            on.parse("Friend."),
            run("friend-name", &["friend", "wake", "--take"])
        );
        assert_eq!(
            on.parse("Friend, what time is it?"),
            run(
                "friend-name",
                &["friend", "wake", "--take", "--say", "what time is it?"]
            )
        );
        assert_eq!(
            on.parse("Hey friend."),
            run("friend-wake", &["friend", "wake"]),
            "hey friend stays the summon"
        );
        assert_eq!(on.parse("My friend called."), None, "the name must lead");
        assert_eq!(on.parse("Friendly fire."), None, "whole words only");
        assert_eq!(
            with(&stale).parse("Friend."),
            None,
            "crashed: not on screen"
        );
        assert_eq!(
            with(&dir.join("gone.pid")).parse("Friend."),
            None,
            "not on screen"
        );
        let always: VoiceCommands =
            toml::from_str("[run.friend-name]\nwhile_running = \"\"\n").unwrap();
        assert_eq!(
            always.parse("Friend."),
            run("friend-name", &["friend", "wake", "--take"]),
            "empty: always heard"
        );
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn run_switch_turns_them_all_off() {
        let c: VoiceCommands = toml::from_str("enabled = true\n[run]\nenabled = false\n").unwrap();
        assert!(c.run.resolved().is_empty());
        assert_eq!(c.parse("Hey friend."), None);
        assert_eq!(c.parse("Enter."), Some(Command::Enter));
        let c: VoiceCommands = toml::from_str("[run.broken]\nphrases = [\"x y\"]\n").unwrap();
        assert_eq!(c.parse("x y"), None, "no program, no command");
    }

    #[test]
    fn key_commands_win_and_literal_escapes_a_run_phrase() {
        let mut c = over();
        c.enter.push("hey friend".into());
        assert_eq!(c.parse("Hey friend."), Some(Command::Enter));
        let c = over();
        assert_eq!(
            c.parse("Literal hey friend."),
            Some(Command::Literal("hey friend.".into()))
        );
    }

    #[test]
    fn detached_spawn_reports_a_missing_program() {
        let err = spawn_detached(&["ears-test-no-such-program".into()]).unwrap_err();
        assert_eq!(err.kind(), std::io::ErrorKind::NotFound);
        assert!(spawn_detached(&[]).is_err());
    }
}
