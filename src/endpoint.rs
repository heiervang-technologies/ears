//! Ending an utterance early: VAD waits `max_silence_duration_ms` of
//! silence before it calls an utterance done, which is most of the delay
//! between the last word and the text appearing. The live transcript says
//! when that wait is not needed: it is a whole command on its own ("enter"),
//! it ends in a key command ("... over"), or it ends a sentence.

use crate::commands::{normalize, Command, VoiceCommands};
use std::collections::HashSet;

/// How the live transcript of an utterance ends.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Ending {
    /// Exactly one of the mode's complete command phrases, and not the
    /// start of a longer one ([`Phrases`]): it fires as soon as it ends.
    Exact,
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

/// The phrases that are a whole command on their own in the current mode,
/// for [`Ending::Exact`]. A phrase only counts if no other active phrase
/// starts with it: "over" does not when "over and out" is a phrase too,
/// since a pause after "over" may be someone still saying "... and out".
/// For the same reason nothing counts that is, or starts with, an open
/// phrase: one that takes the words after it ("call ...", "hey friend ...").
#[derive(Debug, Clone, Default)]
pub struct Phrases {
    complete: HashSet<String>,
    normalize: Option<fn(&str) -> String>,
}

impl Phrases {
    /// `complete` phrases fire as said; `open` ones take more words. Both
    /// are compared after `normalize`, as the partial will be.
    pub fn new<C, O>(complete: C, open: O, normalize: fn(&str) -> String) -> Self
    where
        C: IntoIterator,
        C::Item: AsRef<str>,
        O: IntoIterator,
        O::Item: AsRef<str>,
    {
        let all: HashSet<String> = complete
            .into_iter()
            .map(|p| normalize(p.as_ref()))
            .filter(|p| !p.is_empty())
            .collect();
        let open: Vec<String> = open
            .into_iter()
            .map(|p| normalize(p.as_ref()))
            .filter(|p| !p.is_empty())
            .collect();
        let starts = |p: &str, stem: &str| p.strip_prefix(stem).is_some_and(|r| r.starts_with(' '));
        let complete = all
            .iter()
            .filter(|p| !all.iter().any(|q| starts(q, p)))
            .filter(|p| !open.iter().any(|o| *p == o || starts(p, o)))
            .cloned()
            .collect();
        Self {
            complete,
            normalize: Some(normalize),
        }
    }

    /// Whether the partial transcript `text` is exactly one of them.
    pub fn is_complete(&self, text: &str) -> bool {
        self.normalize
            .is_some_and(|normalize| self.complete.contains(&normalize(text)))
    }

    pub fn is_empty(&self) -> bool {
        self.complete.is_empty()
    }
}

/// What dictation hears as a whole command: the key commands, wake and
/// sleep phrases, "command" and the run commands (a `prefix` one is open,
/// and so are the "back" and "literal" words). Not the colour words: "look"
/// and "orange" start too many sentences to end one at a short pause.
/// `command_mode`: whether "command" enters command mode here.
pub fn dictation_phrases(commands: &VoiceCommands, command_mode: bool) -> Phrases {
    let mut complete: Vec<String> = Vec::new();
    let mut open: Vec<String> = Vec::new();
    if commands.enabled {
        for list in [
            &commands.enter,
            &commands.sign_off,
            &commands.new_line,
            &commands.undo,
            &commands.backtrack,
            &commands.standby,
            &commands.wake,
            &commands.sleep,
        ] {
            complete.extend(list.iter().cloned());
        }
        complete.extend(
            (commands.keys.iter())
                .filter(|(_, combo)| crate::commands::key_sequence(combo).is_some())
                .map(|(phrase, _)| phrase.clone()),
        );
        open.extend(commands.back.iter().cloned());
        open.extend(commands.literal.iter().cloned());
    }
    if command_mode {
        complete.extend(
            crate::command_mode::ENTER_PHRASES
                .iter()
                .map(|p| p.to_string()),
        );
    }
    for run in commands.run.resolved() {
        if run.prefix {
            open.extend(run.phrases);
        } else {
            complete.extend(run.phrases);
        }
    }
    Phrases::new(complete, open, normalize)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_phrase_that_starts_a_longer_one_is_not_complete() {
        let p = Phrases::new(
            [
                "over",
                "over and out",
                "enter",
                "peace",
                "peace out",
                "scratch that",
            ],
            ["call", "hey friend"],
            normalize,
        );
        assert!(p.is_complete("Enter."));
        assert!(p.is_complete("Over and out!"));
        assert!(p.is_complete("scratch that"));
        // The first word of a longer phrase may be someone still talking.
        assert!(!p.is_complete("over"));
        assert!(!p.is_complete("peace"));
        // Not a phrase at all, or more than one.
        assert!(!p.is_complete("enter please now"));
        assert!(!p.is_complete("scratch"));
        assert!(!p.is_complete(""));
    }

    #[test]
    fn an_open_phrase_and_what_starts_with_it_never_complete() {
        // "call" takes a name; "call moon" may yet be "call moonbeam".
        let p = Phrases::new(
            ["call", "call moon", "callback", "hey friend"],
            ["call", "hey friend"],
            normalize,
        );
        assert!(!p.is_complete("call"));
        assert!(!p.is_complete("call moon"));
        assert!(!p.is_complete("hey friend"));
        // Only whole words make a prefix.
        assert!(p.is_complete("callback"));
    }

    #[test]
    fn dictation_hears_its_commands_whole() {
        let mut c = VoiceCommands {
            enabled: true,
            enter: vec!["enter".into(), "over".into()],
            ..VoiceCommands::default()
        };
        let p = dictation_phrases(&c, true);
        assert!(p.is_complete("Enter."));
        assert!(p.is_complete("ears off"));
        // "command" may yet be "command mode"; that one is whole.
        assert!(!p.is_complete("command"));
        assert!(p.is_complete("Command mode."));
        assert!(!p.is_complete("look"));
        assert!(p.is_complete("bye friend"));
        // "over and out" is a sign-off: "over" may be its start.
        assert!(!p.is_complete("over"));
        // "back" takes a count; "hey friend" (prefix run) takes words.
        assert!(!p.is_complete("back"));
        assert!(!p.is_complete("hey friend"));
        assert!(!dictation_phrases(&c, false).is_complete("command mode"));
        c.sign_off.clear();
        assert!(dictation_phrases(&c, false).is_complete("over"));
        // Commands off: only the run commands remain.
        c.enabled = false;
        let p = dictation_phrases(&c, false);
        assert!(!p.is_complete("enter") && p.is_complete("bye friend"));
    }

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
