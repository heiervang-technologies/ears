//! Ending an utterance early: VAD waits `max_silence_duration_ms` of
//! silence before it calls an utterance done, which is most of the delay
//! between the last word and the text appearing. The live transcript says
//! when that wait is not needed: it ends in a key command ("... over"), or
//! it ends a sentence.

use crate::commands::{normalize, Command, VoiceCommands};

/// How the live transcript of an utterance ends.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Ending {
    /// In a command that presses a key: nothing more is coming.
    Command,
    /// A finished sentence: probably done, perhaps a pause to think.
    Finished,
}

/// Last words that promise more: a sentence that ends on one of them is
/// someone thinking, not done.
const GOES_ON: &[&str] = &[
    "and",
    "but",
    "or",
    "so",
    "because",
    "if",
    "then",
    "the",
    "a",
    "an",
    "to",
    "of",
    "with",
    "that",
    "this",
    "uh",
    "um",
    "ah",
    "er",
    "eh",
    "hmm",
    "mm",
    "like",
    "my",
    "your",
    "our",
    "their",
    "his",
    "her",
    "its",
    "is",
    "are",
    "was",
    "in",
    "on",
    "at",
    "for",
    "from",
    "by",
    "as",
    "about",
    "when",
    "which",
    "who",
    "where",
    "while",
    "than",
    "i",
    "we",
    "you",
    "also",
    "just",
    "maybe",
    "actually",
    "basically",
];

/// How the live transcript ends: `raw` as the model wrote it (its
/// punctuation tells a finished sentence), `text` after the text filters.
pub fn ending(raw: &str, text: &str, commands: Option<&VoiceCommands>) -> Option<Ending> {
    if let Some(commands) = commands {
        let key = commands.parse(text).is_some_and(|c| c.presses_key());
        let tail = commands
            .parse_tail(text)
            .is_some_and(|(_, c)| !matches!(c, Command::Literal(_)));
        if key || tail {
            return Some(Ending::Command);
        }
    }
    let raw = raw.trim_end();
    let last = raw.split_whitespace().last().map(normalize)?;
    let sentence = raw.ends_with(['.', '?', '!']) && !raw.ends_with("...");
    (sentence && !last.is_empty() && !GOES_ON.contains(&last.as_str())).then_some(Ending::Finished)
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
    fn a_command_ends_it_soonest() {
        let c = over();
        let e = |raw: &str| ending(raw, &raw.to_lowercase(), Some(&c));
        assert_eq!(e("Over."), Some(Ending::Command));
        assert_eq!(e("Ship it over"), Some(Ending::Command));
        assert_eq!(e("Ship it, over."), Some(Ending::Command));
        assert_eq!(e("The game is over."), Some(Ending::Finished));
    }

    #[test]
    fn a_finished_sentence_ends_it_sooner() {
        let e = |raw: &str| ending(raw, raw, None);
        assert_eq!(e("That works for me."), Some(Ending::Finished));
        assert_eq!(e("Is it done?"), Some(Ending::Finished));
        assert_eq!(e("That works for me"), None);
        assert_eq!(e("I was thinking, and."), None);
        assert_eq!(e("So if my, uh."), None);
        assert_eq!(e("Well..."), None);
        assert_eq!(e(""), None);
    }
}
