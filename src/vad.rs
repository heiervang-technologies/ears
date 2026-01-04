//! Voice Activity Detection (VAD)
//!
//! This module provides Voice Activity Detection capabilities to distinguish
//! speech from silence in audio streams.

use thiserror::Error;

/// Errors that can occur during VAD processing
#[derive(Error, Debug)]
pub enum VadError {
    #[error("Invalid audio data: {0}")]
    InvalidAudio(String),

    #[error("VAD configuration error: {0}")]
    ConfigError(String),
}

/// Voice Activity Detection result
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VadResult {
    /// Speech detected in the audio frame
    Speech,
    /// Silence detected in the audio frame
    Silence,
}

/// Configuration for Voice Activity Detection
#[derive(Debug, Clone)]
pub struct VadConfig {
    /// Sample rate in Hz (default: 16000)
    pub sample_rate: usize,
    /// Frame size in milliseconds (default: 30ms)
    pub frame_size_ms: usize,
    /// Energy threshold for speech detection (default: 0.01)
    /// Higher values = more aggressive silence detection
    pub energy_threshold: f32,
    /// Minimum speech duration in milliseconds (default: 300ms)
    pub min_speech_duration_ms: u64,
    /// Maximum silence duration before ending segment (default: 700ms)
    pub max_silence_duration_ms: u64,
}

impl Default for VadConfig {
    fn default() -> Self {
        Self {
            sample_rate: 16000,
            frame_size_ms: 30,
            energy_threshold: 0.01,
            min_speech_duration_ms: 300,
            max_silence_duration_ms: 700,
        }
    }
}

/// Simple energy-based Voice Activity Detector
///
/// This is a basic VAD implementation that uses energy (RMS) to detect speech.
/// For production use, consider integrating Silero VAD or whisper.cpp's built-in VAD.
pub struct EnergyVad {
    config: VadConfig,
    frame_size_samples: usize,
    speech_frames: usize,
    silence_frames: usize,
    in_speech: bool,
}

impl EnergyVad {
    /// Create a new EnergyVad with the given configuration
    pub fn new(config: VadConfig) -> Self {
        let frame_size_samples = (config.sample_rate * config.frame_size_ms) / 1000;

        Self {
            config,
            frame_size_samples,
            speech_frames: 0,
            silence_frames: 0,
            in_speech: false,
        }
    }

    /// Process an audio frame and detect voice activity
    ///
    /// # Arguments
    /// * `samples` - Audio samples (mono, f32, -1.0 to 1.0)
    ///
    /// # Returns
    /// * `VadResult::Speech` if speech is detected
    /// * `VadResult::Silence` if silence is detected
    pub fn process_frame(&mut self, samples: &[f32]) -> Result<VadResult, VadError> {
        if samples.is_empty() {
            return Err(VadError::InvalidAudio("Empty audio frame".to_string()));
        }

        // Calculate RMS energy of the frame
        let energy = calculate_rms(samples);

        // Determine if this frame contains speech
        let is_speech = energy > self.config.energy_threshold;

        // Update counters
        if is_speech {
            self.speech_frames += 1;
            self.silence_frames = 0;
        } else {
            self.silence_frames += 1;
            if !self.in_speech {
                self.speech_frames = 0;
            }
        }

        // Calculate durations
        let ms_per_frame = (self.frame_size_samples * 1000) / self.config.sample_rate;
        let speech_duration_ms = (self.speech_frames * ms_per_frame) as u64;
        let silence_duration_ms = (self.silence_frames * ms_per_frame) as u64;

        // State machine: determine if we're in a speech segment
        let result = if !self.in_speech {
            // Currently in silence, check if we should start speech segment
            if speech_duration_ms >= self.config.min_speech_duration_ms {
                self.in_speech = true;
                VadResult::Speech
            } else {
                VadResult::Silence
            }
        } else {
            // Currently in speech, check if we should end speech segment
            if silence_duration_ms >= self.config.max_silence_duration_ms {
                self.in_speech = false;
                self.speech_frames = 0;
                self.silence_frames = 0;
                VadResult::Silence
            } else {
                VadResult::Speech
            }
        };

        Ok(result)
    }

    /// Check if currently in a speech segment
    pub fn is_speaking(&self) -> bool {
        self.in_speech
    }

    /// Reset the VAD state
    pub fn reset(&mut self) {
        self.speech_frames = 0;
        self.silence_frames = 0;
        self.in_speech = false;
    }

    /// Get the frame size in samples
    pub fn frame_size(&self) -> usize {
        self.frame_size_samples
    }
}

/// Calculate the RMS (Root Mean Square) energy of an audio frame
fn calculate_rms(samples: &[f32]) -> f32 {
    if samples.is_empty() {
        return 0.0;
    }

    let sum_squares: f32 = samples.iter().map(|&s| s * s).sum();
    let mean_square = sum_squares / samples.len() as f32;
    mean_square.sqrt()
}

/// Speech segment detected by VAD
#[derive(Debug, Clone)]
pub struct SpeechSegment {
    /// Start time in milliseconds
    pub start_ms: u64,
    /// End time in milliseconds
    pub end_ms: u64,
    /// Audio samples for this segment
    pub samples: Vec<f32>,
}

/// VAD segment detector that tracks speech segments
pub struct VadSegmentDetector {
    vad: EnergyVad,
    sample_rate: usize,
    current_segment: Option<Vec<f32>>,
    segment_start_ms: u64,
    total_processed_ms: u64,
}

impl VadSegmentDetector {
    /// Create a new VadSegmentDetector
    pub fn new(config: VadConfig) -> Self {
        let sample_rate = config.sample_rate;
        Self {
            vad: EnergyVad::new(config),
            sample_rate,
            current_segment: None,
            segment_start_ms: 0,
            total_processed_ms: 0,
        }
    }

    /// Process audio and extract speech segments
    ///
    /// # Arguments
    /// * `samples` - Audio samples to process
    ///
    /// # Returns
    /// * `Some(SpeechSegment)` if a complete speech segment was detected
    /// * `None` if still collecting or in silence
    pub fn process(&mut self, samples: &[f32]) -> Result<Option<SpeechSegment>, VadError> {
        let frame_size = self.vad.frame_size();

        // Process in frames
        let mut segment_complete = None;

        for chunk in samples.chunks(frame_size) {
            let result = self.vad.process_frame(chunk)?;

            let frame_duration_ms =
                ((chunk.len() * 1000) / self.sample_rate) as u64;

            match result {
                VadResult::Speech => {
                    if self.current_segment.is_none() {
                        // Start new segment
                        self.current_segment = Some(Vec::new());
                        self.segment_start_ms = self.total_processed_ms;
                    }

                    // Add samples to current segment
                    if let Some(ref mut segment) = self.current_segment {
                        segment.extend_from_slice(chunk);
                    }
                }
                VadResult::Silence => {
                    if let Some(segment_samples) = self.current_segment.take() {
                        // Speech segment ended
                        segment_complete = Some(SpeechSegment {
                            start_ms: self.segment_start_ms,
                            end_ms: self.total_processed_ms,
                            samples: segment_samples,
                        });
                    }
                }
            }

            self.total_processed_ms += frame_duration_ms;
        }

        Ok(segment_complete)
    }

    /// Check if currently in a speech segment
    pub fn is_speaking(&self) -> bool {
        self.vad.is_speaking()
    }

    /// Reset the detector
    pub fn reset(&mut self) {
        self.vad.reset();
        self.current_segment = None;
        self.segment_start_ms = 0;
        self.total_processed_ms = 0;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn generate_sine_wave(frequency: f32, duration_ms: u64, sample_rate: usize) -> Vec<f32> {
        let num_samples = (sample_rate as u64 * duration_ms / 1000) as usize;
        let mut samples = Vec::with_capacity(num_samples);

        for i in 0..num_samples {
            let t = i as f32 / sample_rate as f32;
            let sample = (2.0 * std::f32::consts::PI * frequency * t).sin() * 0.5;
            samples.push(sample);
        }

        samples
    }

    fn generate_silence(duration_ms: u64, sample_rate: usize) -> Vec<f32> {
        let num_samples = (sample_rate as u64 * duration_ms / 1000) as usize;
        vec![0.0; num_samples]
    }

    #[test]
    fn test_calculate_rms() {
        let silence = vec![0.0; 100];
        assert_eq!(calculate_rms(&silence), 0.0);

        let constant = vec![0.5; 100];
        assert!((calculate_rms(&constant) - 0.5).abs() < 0.001);

        let mixed = vec![-1.0, 1.0, -1.0, 1.0];
        assert!((calculate_rms(&mixed) - 1.0).abs() < 0.001);
    }

    #[test]
    fn test_energy_vad_speech_detection() {
        let mut vad = EnergyVad::new(VadConfig {
            sample_rate: 16000,
            frame_size_ms: 30,
            energy_threshold: 0.01,
            min_speech_duration_ms: 100,
            max_silence_duration_ms: 300,
        });

        let frame_size = vad.frame_size();

        // Generate speech (sine wave)
        let speech = generate_sine_wave(440.0, 500, 16000);

        // Process speech frames
        let mut speech_detected = false;
        for chunk in speech.chunks(frame_size) {
            if let Ok(VadResult::Speech) = vad.process_frame(chunk) {
                speech_detected = true;
            }
        }

        assert!(speech_detected, "Should detect speech in sine wave");
    }

    #[test]
    fn test_energy_vad_silence_detection() {
        let mut vad = EnergyVad::new(VadConfig::default());

        let frame_size = vad.frame_size();

        // Generate silence
        let silence = generate_silence(1000, 16000);

        // Process silence frames
        for chunk in silence.chunks(frame_size) {
            let result = vad.process_frame(chunk).unwrap();
            assert_eq!(result, VadResult::Silence);
        }

        assert!(!vad.is_speaking());
    }

    #[test]
    fn test_vad_segment_detector() {
        let mut detector = VadSegmentDetector::new(VadConfig {
            sample_rate: 16000,
            frame_size_ms: 30,
            energy_threshold: 0.01,
            min_speech_duration_ms: 100,
            max_silence_duration_ms: 200,
        });

        // Silence -> Speech -> Silence pattern
        let silence1 = generate_silence(200, 16000);
        let speech = generate_sine_wave(440.0, 500, 16000);
        let silence2 = generate_silence(500, 16000);

        // Process silence (should return None)
        assert!(detector.process(&silence1).unwrap().is_none());

        // Process speech (should still be None, collecting)
        assert!(detector.process(&speech).unwrap().is_none());

        // Process silence (should return completed segment)
        let segment = detector.process(&silence2).unwrap();
        assert!(segment.is_some());

        if let Some(seg) = segment {
            assert!(!seg.samples.is_empty());
            assert!(seg.end_ms > seg.start_ms);
        }
    }

    #[test]
    fn test_vad_reset() {
        let mut vad = EnergyVad::new(VadConfig::default());

        let frame_size = vad.frame_size();
        let speech = generate_sine_wave(440.0, 500, 16000);

        // Process some speech
        for chunk in speech.chunks(frame_size).take(10) {
            vad.process_frame(chunk).unwrap();
        }

        // Reset
        vad.reset();

        assert!(!vad.is_speaking());
        assert_eq!(vad.speech_frames, 0);
        assert_eq!(vad.silence_frames, 0);
    }
}
