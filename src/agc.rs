//! Speech-gated automatic gain.
//!
//! Quiet speech scores lower with Silero and transcribes worse, but a plain
//! AGC also boosts silence, and boosted room noise is what turns into
//! "Thank you." hallucinations. So the level is measured only on frames the
//! detector already scores as speech, it moves slowly, and the gain is
//! capped. Silence gets the same gain as the speech around it, never more.

/// Speech RMS the gain aims for (about -20 dBFS).
pub const TARGET_RMS: f32 = 0.1;
/// Most boost applied (+12 dB).
pub const MAX_GAIN: f32 = 4.0;
/// Most cut applied (-6 dB).
pub const MIN_GAIN: f32 = 0.5;
/// Weight of one 32 ms speech frame in the level estimate (~0.6 s of speech).
const ALPHA: f32 = 0.05;
/// Frames quieter than this are not taken as a speech level (-60 dBFS).
const FLOOR_RMS: f32 = 0.001;

/// Gain derived from the level of recent speech.
#[derive(Debug, Clone)]
pub struct SpeechGain {
    enabled: bool,
    level: Option<f32>,
    gain: f32,
}

impl SpeechGain {
    pub fn new(enabled: bool) -> Self {
        Self {
            enabled,
            level: None,
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
        self.gain = (TARGET_RMS / level).clamp(MIN_GAIN, MAX_GAIN);
    }
}

/// RMS of a block of samples.
pub fn rms(samples: &[f32]) -> f32 {
    if samples.is_empty() {
        return 0.0;
    }
    (samples.iter().map(|s| s * s).sum::<f32>() / samples.len() as f32).sqrt()
}

/// Scale a finished speech segment so its speech, the louder half of its
/// 32 ms frames, sits at [`TARGET_RMS`], within the same gain limits and
/// with peaks kept below full scale. Returns the gain applied.
pub fn normalize_segment(samples: &mut [f32]) -> f32 {
    const FRAME: usize = 512;
    let mut levels: Vec<f32> = samples.chunks(FRAME).map(rms).collect();
    if levels.is_empty() {
        return 1.0;
    }
    levels.sort_by(|a, b| b.total_cmp(a));
    let loud = &levels[..levels.len().div_ceil(2)];
    let speech = (loud.iter().map(|l| l * l).sum::<f32>() / loud.len() as f32).sqrt();
    if speech < FLOOR_RMS {
        return 1.0;
    }
    let peak = samples.iter().fold(0.0f32, |p, s| p.max(s.abs()));
    let mut gain = (TARGET_RMS / speech).clamp(MIN_GAIN, MAX_GAIN);
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
        // Loud speech is cut, but not below the floor.
        for _ in 0..500 {
            g.observe_speech(0.9);
        }
        assert_eq!(g.gain(), MIN_GAIN);
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
