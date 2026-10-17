//! Wake detection for VAD standby.
//!
//! In standby VAD keeps listening but types nothing and runs no other
//! command; only the wake phrase is acted on, and it toggles standby off
//! (and, said while typing, on). Deciding whether an utterance is the wake
//! phrase sits behind [`WakeDetector`] so the detector can be swapped:
//!
//! - [`PhraseWake`] (default) matches the transcript against phrases like
//!   "ears vad". Every utterance in standby is still transcribed, locally
//!   by the configured server, but not typed, logged or published.
//! - A small wake word model would decide from the audio alone and answer
//!   [`AudioVerdict::NotWake`] for everything else, so standby would not
//!   transcribe at all.

/// What a detector concludes from a speech segment's audio.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AudioVerdict {
    /// This is the wake phrase.
    Wake,
    /// This is not the wake phrase; no transcript is needed to know.
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
    fn check_transcript(&mut self, _text: &str) -> bool {
        false
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
    phrases: Vec<String>,
}

impl PhraseWake {
    pub fn new(phrases: &[String]) -> Self {
        Self {
            phrases: phrases
                .iter()
                .map(|p| crate::commands::normalize(p))
                .filter(|p| !p.is_empty())
                .collect(),
        }
    }

    pub fn is_empty(&self) -> bool {
        self.phrases.is_empty()
    }
}

impl WakeDetector for PhraseWake {
    fn check_transcript(&mut self, text: &str) -> bool {
        let said = crate::commands::normalize(text);
        self.phrases.contains(&said)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn phrase_wake_matches_the_whole_utterance() {
        let mut wake = PhraseWake::new(&["ears vad".into(), "ears v a d".into()]);
        assert_eq!(wake.check_audio(&[0.0; 16]), AudioVerdict::Undecided);
        assert!(wake.check_transcript("Ears VAD."));
        assert!(wake.check_transcript("ears, vad!"));
        assert!(wake.check_transcript("Ears V.A.D."));
        assert!(!wake.check_transcript("I turned ears vad off."));
        assert!(!wake.check_transcript(""));
    }

    #[test]
    fn empty_phrases_never_wake() {
        let mut wake = PhraseWake::new(&["".into(), " . ".into()]);
        assert!(wake.is_empty());
        assert!(!wake.check_transcript(""));
        assert_eq!(NoWake.check_audio(&[]), AudioVerdict::NotWake);
    }
}
