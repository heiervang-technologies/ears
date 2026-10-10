//! Spoken commands in VAD mode.
//!
//! A command is recognized only when it is the *whole* utterance: you say
//! "Over.", pause, and that utterance presses Enter. The same word inside a
//! sentence ("it's over now") is ordinary text. VAD already cuts speech at
//! pauses, so the pause is the boundary. To type a command word as text,
//! prefix it with the literal word: "literal over" types "over".

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// `[commands]` config section.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct VoiceCommands {
    /// Recognize commands at all (default: false).
    #[serde(default)]
    pub enabled: bool,
    /// Phrases that press Enter.
    #[serde(default = "default_enter")]
    pub enter: Vec<String>,
    /// Phrases that press Shift+Enter (a new line without sending, in chat
    /// boxes and agent prompts).
    #[serde(default = "default_new_line")]
    pub new_line: Vec<String>,
    /// Words that, said first, type the rest of the utterance as text.
    #[serde(default = "default_literal")]
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
}

fn default_enter() -> Vec<String> {
    vec!["enter".into()]
}

fn default_new_line() -> Vec<String> {
    vec!["new line".into()]
}

fn default_literal() -> Vec<String> {
    vec!["literal".into()]
}

fn default_accept_ms() -> u64 {
    200
}

impl Default for VoiceCommands {
    fn default() -> Self {
        Self {
            enabled: false,
            enter: default_enter(),
            new_line: default_new_line(),
            literal: default_literal(),
            keys: BTreeMap::new(),
            accept_ms: default_accept_ms(),
        }
    }
}

/// What an utterance asks for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Command {
    /// Press Enter.
    Enter,
    /// Press Shift+Enter.
    NewLine,
    /// Press a key combination from `[commands.keys]`, e.g. "ctrl+b".
    Keys(String),
    /// Type this text instead of interpreting it.
    Literal(String),
}

impl Command {
    /// Short name for logs and events.
    pub fn name(&self) -> &'static str {
        match self {
            Command::Enter => "enter",
            Command::NewLine => "new_line",
            Command::Keys(_) => "keys",
            Command::Literal(_) => "literal",
        }
    }

    /// Name with its key combination, for logs and events: "enter",
    /// "keys:ctrl+b".
    pub fn label(&self) -> String {
        match self {
            Command::Keys(combo) => format!("keys:{combo}"),
            other => other.name().to_string(),
        }
    }

    /// Whether the command presses a key (rather than typing text).
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
    /// The command this whole utterance is, if any.
    pub fn parse(&self, transcript: &str) -> Option<Command> {
        if !self.enabled {
            return None;
        }
        let said = normalize(transcript);
        if said.is_empty() {
            return None;
        }
        let is = |phrases: &[String]| phrases.iter().any(|p| normalize(p) == said);
        if is(&self.enter) {
            return Some(Command::Enter);
        }
        if is(&self.new_line) {
            return Some(Command::NewLine);
        }
        if let Some((_, combo)) = self
            .keys
            .iter()
            .find(|(phrase, combo)| normalize(phrase) == said && keycodes(combo).is_some())
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

#[cfg(test)]
mod tests {
    use super::*;

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
        assert_eq!(c.parse("Over and out."), None);
        assert_eq!(c.parse("Press enter to continue."), None);
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
}
