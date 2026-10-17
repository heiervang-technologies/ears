//! Speech-gated automatic gain.
//!
//! Quiet speech scores lower with Silero and transcribes worse, but a plain
//! AGC also boosts silence, and boosted room noise is what turns into
//! "Thank you." hallucinations. So the level is measured only on frames the
//! detector already scores as speech, it moves slowly, and the gain is
//! capped. Silence gets the same gain as the speech around it, except that
//! after a few seconds without speech the gain drifts back up toward the
//! cap (never past what keeps the room noise below [`NOISE_CEILING`]), so
//! one loud utterance cannot leave the detector deaf to quieter ones.

/// Speech RMS the gain aims for (about -20 dBFS).
pub const TARGET_RMS: f32 = 0.1;
/// Most boost applied (+12 dB).
pub const MAX_GAIN: f32 = 4.0;
/// The detector is never cut. Loud speech is detected anyway, but a cut
/// learned from it also turned down the next, quieter speech; missed, it
/// never raised the gain again, so detection ratcheted deaf after close-up
/// speech.
pub const MIN_GAIN: f32 = 1.0;
/// Most cut applied to a finished segment before transcription (-6 dB).
pub const SEGMENT_MIN_GAIN: f32 = 0.5;
/// Room noise is never raised above this (-30 dBFS) by the gain.
pub const NOISE_CEILING: f32 = 0.03;
/// Frames without speech (~2 s) before the gain starts to recover.
const RECOVER_AFTER_FRAMES: u32 = 62;
/// Per-frame drop of the level estimate while recovering: from no boost to
/// the cap in about 1.5 s.
const RECOVER_DECAY: f32 = 0.97;
/// Weights of one 32 ms quiet frame in the noise floor: it falls within
/// ~0.3 s but rises over about a minute, so a knock or a cough is not taken as the
/// room's noise.
const NOISE_FALL: f32 = 0.1;
const NOISE_RISE: f32 = 0.0005;
/// Weight of one 32 ms speech frame in the level estimate (~0.6 s of speech).
const ALPHA: f32 = 0.05;
/// Frames quieter than this are not taken as a speech level (-60 dBFS).
const FLOOR_RMS: f32 = 0.001;

/// Gain derived from the level of recent speech.
#[derive(Debug, Clone)]
pub struct SpeechGain {
    enabled: bool,
    level: Option<f32>,
    noise: Option<f32>,
    quiet_frames: u32,
    gain: f32,
}

impl SpeechGain {
    pub fn new(enabled: bool) -> Self {
        Self {
            enabled,
            level: None,
            noise: None,
            quiet_frames: 0,
            gain: 1.0,
        }
    }

    /// The gain applied now.
    pub fn gain(&self) -> f32 {
        self.gain
    }

    /// Scale `frame` in place.
    pub fn apply(&self, frame: &mut [f32]) {
        if self.gain != 1.0 {
            for s in frame {
                *s = (*s * self.gain).clamp(-1.0, 1.0);
            }
        }
    }

    /// Feed the RMS of an unscaled frame the detector scored as speech.
    pub fn observe_speech(&mut self, raw_rms: f32) {
        if !self.enabled || raw_rms < FLOOR_RMS {
            return;
        }
        let level = match self.level {
            Some(level) => level + ALPHA * (raw_rms - level),
            None => raw_rms,
        };
        self.level = Some(level);
        self.quiet_frames = 0;
        self.update_gain();
    }

    /// Feed the RMS of an unscaled frame the detector scored as silence:
    /// it tracks the room noise, and after a while without speech lets the
    /// gain recover.
    pub fn observe_silence(&mut self, raw_rms: f32) {
        if !self.enabled {
            return;
        }
        self.noise = Some(match self.noise {
            Some(noise) if raw_rms < noise => noise + NOISE_FALL * (raw_rms - noise),
            Some(noise) => noise + NOISE_RISE * (raw_rms - noise),
            None => raw_rms,
        });
        self.quiet_frames = self.quiet_frames.saturating_add(1);
        if self.quiet_frames > RECOVER_AFTER_FRAMES {
            if let Some(level) = self.level.as_mut() {
                *level = (*level * RECOVER_DECAY).max(TARGET_RMS / MAX_GAIN);
            }
            self.update_gain();
        }
    }

    fn update_gain(&mut self) {
        let Some(level) = self.level else {
            return;
        };
        let noise_cap = match self.noise {
            Some(noise) if noise > 0.0 => (NOISE_CEILING / noise).max(MIN_GAIN),
            _ => MAX_GAIN,
        };
        self.gain = (TARGET_RMS / level).clamp(MIN_GAIN, MAX_GAIN.min(noise_cap));
    }
}

pub use crate::pcm::rms;

/// Scale a finished speech segment so its speech, the louder half of its
/// 32 ms frames, sits at [`TARGET_RMS`], within the same gain limits and
/// with peaks kept below full scale. Returns the gain applied.
pub fn normalize_segment(samples: &mut [f32]) -> f32 {
    let speech = crate::pcm::speech_rms(samples);
    if speech < FLOOR_RMS {
        return 1.0;
    }
    let peak = crate::pcm::peak(samples);
    let mut gain = (TARGET_RMS / speech).clamp(SEGMENT_MIN_GAIN, MAX_GAIN);
    if peak > 0.0 {
        gain = gain.min(0.98 / peak);
    }
    if (gain - 1.0).abs() > 0.01 {
        for s in samples.iter_mut() {
            *s *= gain;
        }
        gain
    } else {
        1.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn disabled_gain_never_moves() {
        let mut g = SpeechGain::new(false);
        g.observe_speech(0.01);
        assert_eq!(g.gain(), 1.0);
    }

    #[test]
    fn quiet_speech_is_raised_within_the_cap() {
        let mut g = SpeechGain::new(true);
        g.observe_speech(0.05);
        assert!((g.gain() - 2.0).abs() < 1e-6);
        for _ in 0..500 {
            g.observe_speech(0.002);
        }
        assert_eq!(g.gain(), MAX_GAIN, "very quiet speech hits the cap");
        // Loud speech is never cut for the detector.
        for _ in 0..500 {
            g.observe_speech(0.9);
        }
        assert_eq!(g.gain(), MIN_GAIN);
    }

    #[test]
    fn gain_recovers_after_silence_within_the_noise_ceiling() {
        let mut g = SpeechGain::new(true);
        for _ in 0..100 {
            g.observe_speech(0.3); // close-up, loud
        }
        assert_eq!(g.gain(), MIN_GAIN);
        // A moment of silence changes nothing yet.
        for _ in 0..RECOVER_AFTER_FRAMES {
            g.observe_silence(0.002);
        }
        assert_eq!(g.gain(), MIN_GAIN);
        // Then the gain climbs back to the cap.
        for _ in 0..200 {
            g.observe_silence(0.002);
        }
        assert_eq!(g.gain(), MAX_GAIN);
        // A noisy room caps it: 0.015 noise may only be doubled.
        let mut noisy = SpeechGain::new(true);
        noisy.observe_speech(0.3);
        for _ in 0..20_000 {
            noisy.observe_silence(0.015);
        }
        assert!((noisy.gain() - 2.0).abs() < 0.01, "{}", noisy.gain());
        // A two-second knock in a quiet room is not taken as its noise.
        for _ in 0..60 {
            g.observe_silence(0.2);
        }
        assert!(g.gain() > 3.0, "{}", g.gain());
        // Speech resets the wait.
        g.observe_speech(0.3);
        let after_speech = g.gain();
        g.observe_silence(0.002);
        assert_eq!(g.gain(), after_speech);
    }

    #[test]
    fn silence_is_never_taken_as_the_speech_level() {
        let mut g = SpeechGain::new(true);
        g.observe_speech(0.05);
        let before = g.gain();
        g.observe_speech(0.0001);
        assert_eq!(g.gain(), before);
    }

    #[test]
    fn apply_scales_and_clips() {
        let mut g = SpeechGain::new(true);
        g.observe_speech(0.025); // gain 4
        let mut frame = [0.1, -0.2, 0.5];
        g.apply(&mut frame);
        assert_eq!(frame, [0.4, -0.8, 1.0]);
    }

    #[test]
    fn segment_normalization_targets_the_speech_not_the_pauses() {
        // Half quiet speech (0.05 RMS square-ish wave), half silence.
        let mut seg: Vec<f32> = (0..512 * 10)
            .map(|i| if (i / 8) % 2 == 0 { 0.05 } else { -0.05 })
            .chain(std::iter::repeat_n(0.0, 512 * 10))
            .collect();
        let gain = normalize_segment(&mut seg);
        assert!((gain - 2.0).abs() < 1e-3, "gain {gain}");
        assert!((seg[0] - 0.1).abs() < 1e-4);
    }

    #[test]
    fn segment_normalization_keeps_peaks_below_full_scale() {
        let mut seg = vec![0.01f32; 512 * 4];
        seg[100] = 0.5;
        let gain = normalize_segment(&mut seg);
        assert!(gain <= 0.98 / 0.5 + 1e-6);
        assert!(seg.iter().all(|s| s.abs() <= 0.98 + 1e-6));
    }
}
