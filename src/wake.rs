//! Wake detection for VAD standby.
//!
//! In standby VAD keeps listening but types nothing and runs no other
//! command; only wake phrases are acted on: one that toggles standby
//! ("ears vad"), one that resumes typing ("ears on") and one that enters
//! standby ("ears off"). Deciding whether an utterance is a wake phrase
//! sits behind [`WakeDetector`] so the detector can be swapped:
//!
//! - [`PhraseWake`] (default) matches the transcript against phrases like
//!   "ears vad". Every utterance in standby is still transcribed, locally
//!   by the configured server, but not typed, logged or published.
//! - A small wake word model would decide from the audio alone and answer
//!   [`AudioVerdict::NotWake`] for everything else, so standby would not
//!   transcribe at all.

/// What a wake phrase asks for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Wake {
    /// Switch between typing and standby.
    Toggle,
    /// Type (leave standby).
    On,
    /// Stand by.
    Off,
}

impl Wake {
    /// Whether to be in standby afterwards, from whether it is now.
    pub fn standby_after(self, standby: bool) -> bool {
        match self {
            Wake::Toggle => !standby,
            Wake::On => false,
            Wake::Off => true,
        }
    }
}

/// What a detector concludes from a speech segment's audio.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AudioVerdict {
    /// This is a wake phrase.
    Wake(Wake),
    /// This is no wake phrase; no transcript is needed to know.
    NotWake,
    /// The audio alone does not tell: check the transcript.
    Undecided,
}

/// Decides whether an utterance is the wake phrase.
pub trait WakeDetector: Send {
    /// A finished speech segment (16 kHz mono), checked before it is
    /// transcribed.
    fn check_audio(&mut self, _samples: &[f32]) -> AudioVerdict {
        AudioVerdict::Undecided
    }

    /// The segment's transcript, checked when the audio was undecided.
    fn check_transcript(&mut self, _text: &str) -> Option<Wake> {
        None
    }
}

/// No wake phrase: standby is never entered by voice.
pub struct NoWake;

impl WakeDetector for NoWake {
    fn check_audio(&mut self, _samples: &[f32]) -> AudioVerdict {
        AudioVerdict::NotWake
    }
}

/// Wake on a transcript that is exactly one of the phrases, compared as
/// spoken commands are (case and punctuation ignored).
pub struct PhraseWake {
    phrases: Vec<(String, Wake)>,
}

impl PhraseWake {
    pub fn new(toggle: &[String], on: &[String], off: &[String]) -> Self {
        let tagged = |list: &[String], wake| {
            list.iter()
                .map(|p| (crate::commands::normalize(p), wake))
                .collect::<Vec<_>>()
        };
        let mut phrases = tagged(toggle, Wake::Toggle);
        phrases.extend(tagged(on, Wake::On));
        phrases.extend(tagged(off, Wake::Off));
        phrases.retain(|(p, _)| !p.is_empty());
        Self { phrases }
    }

    pub fn is_empty(&self) -> bool {
        self.phrases.is_empty()
    }
}

impl WakeDetector for PhraseWake {
    fn check_transcript(&mut self, text: &str) -> Option<Wake> {
        let said = crate::commands::normalize(text);
        self.phrases
            .iter()
            .find(|(p, _)| *p == said)
            .map(|&(_, wake)| wake)
    }
}

/// Whether `text` reads as `language` (a code or English name: "sv",
/// "swedish"). Used to stand by when VAD picks up speech in a language
/// you don't dictate in, typically a TV or a call in the background.
/// Only Swedish is known; any other language never matches.
///
/// Even with English pinned, Qwen3-ASR writes Swedish speech out as
/// Swedish, so the transcript tells. It counts words only Swedish uses
/// (not Norwegian, Danish or English: "jag", "inte", "och"...) and a
/// Swedish "ä"/"ö" without a Norwegian "æ"/"ø": two of those make it
/// Swedish, so one stray word in English dictation does not.
pub fn sounds_like(language: &str, text: &str) -> bool {
    if !matches!(
        language.to_lowercase().as_str(),
        "sv" | "swedish" | "svenska"
    ) {
        return false;
    }
    // Not "vad": in English dictation that is the VAD.
    const SWEDISH_ONLY: &[&str] = &[
        "jag", "inte", "och", "är", "att", "hur", "varför", "mycket", "också", "någon", "något",
        "några", "hon", "honom", "här", "där", "säger", "själv", "måste", "sedan", "mig", "dig",
        "vill", "kanske", "nej", "tack", "blev", "från", "när", "inget",
    ];
    let lower = text.to_lowercase();
    let mut score = lower
        .split(|c: char| !c.is_alphabetic())
        .filter(|word| SWEDISH_ONLY.contains(word))
        .count();
    let swedish_letters = lower.contains(['ä', 'ö']);
    let norwegian_letters = lower.contains(['æ', 'ø']);
    if swedish_letters && !norwegian_letters {
        score += 1;
    }
    score >= 2
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn phrase_wake_matches_the_whole_utterance() {
        let mut wake = PhraseWake::new(
            &["ears vad".into(), "ears v a d".into()],
            &["ears on".into()],
            &["ears off".into()],
        );
        assert_eq!(wake.check_audio(&[0.0; 16]), AudioVerdict::Undecided);
        assert_eq!(wake.check_transcript("Ears VAD."), Some(Wake::Toggle));
        assert_eq!(wake.check_transcript("ears, vad!"), Some(Wake::Toggle));
        assert_eq!(wake.check_transcript("Ears V.A.D."), Some(Wake::Toggle));
        assert_eq!(wake.check_transcript("Ears on."), Some(Wake::On));
        assert_eq!(wake.check_transcript("Ears off!"), Some(Wake::Off));
        assert_eq!(wake.check_transcript("I turned ears vad off."), None);
        assert_eq!(wake.check_transcript("Ears on the table."), None);
        assert_eq!(wake.check_transcript(""), None);
    }

    #[test]
    fn swedish_is_told_from_english_and_norwegian() {
        let heard = "Hej, vad gjorde du idag? Jag tänkte att vi kunde gå och handla.";
        assert!(sounds_like("sv", heard));
        assert!(sounds_like("Swedish", "Det är inte så farligt."));
        assert!(sounds_like("sv", "jag vet inte"));
        assert!(!sounds_like(
            "sv",
            "I was thinking we could go shopping later."
        ));
        assert!(!sounds_like(
            "sv",
            "Jeg tror ikke det er så farlig, vi får se."
        ));
        assert!(!sounds_like(
            "sv",
            "Hun sa at hun skulle komme etter middag."
        ));
        assert!(!sounds_like("sv", "Ask Jag to turn the vad off."));
        assert!(!sounds_like("sv", "and"));
        assert!(!sounds_like("de", heard));
    }

    #[test]
    fn on_and_off_are_one_way() {
        assert!(Wake::Toggle.standby_after(false));
        assert!(!Wake::Toggle.standby_after(true));
        assert!(!Wake::On.standby_after(true));
        assert!(!Wake::On.standby_after(false));
        assert!(Wake::Off.standby_after(false));
        assert!(Wake::Off.standby_after(true));
    }

    #[test]
    fn empty_phrases_never_wake() {
        let mut wake = PhraseWake::new(&["".into(), " . ".into()], &[], &[]);
        assert!(wake.is_empty());
        assert_eq!(wake.check_transcript(""), None);
        assert_eq!(NoWake.check_audio(&[]), AudioVerdict::NotWake);
    }
}
