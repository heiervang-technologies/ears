//! Spoken commands in VAD mode.
//!
//! A command is recognized only when it is the *whole* utterance: you say
//! "Over.", pause, and that utterance presses Enter. The same word inside a
//! sentence ("it's over now") is ordinary text. VAD already cuts speech at
//! pauses, so the pause is the boundary. To type a command word as text,
//! prefix it with the literal word: "literal over" types "over".

use serde::{Deserialize, Serialize};

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
    /// Type this text instead of interpreting it.
    Literal(String),
}

impl Command {
    /// Short name for logs and events.
    pub fn name(&self) -> &'static str {
        match self {
            Command::Enter => "enter",
            Command::NewLine => "new_line",
            Command::Literal(_) => "literal",
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
