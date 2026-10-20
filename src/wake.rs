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
