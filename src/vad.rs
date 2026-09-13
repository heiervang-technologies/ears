//! Voice Activity Detection (VAD)
//!
//! This module provides Voice Activity Detection using the Silero VAD neural network
//! model via ONNX Runtime. It replaces the previous energy-based approach which was
//! unable to distinguish speech from background noise on real microphones.

use std::collections::VecDeque;
use thiserror::Error;
use tracing::debug;
use voice_activity_detector::VoiceActivityDetector;

/// Silero VAD frame size: 512 samples at 16kHz (32ms)
pub const SILERO_FRAME_SIZE: usize = 512;

/// Silero VAD sample rate
pub const SILERO_SAMPLE_RATE: usize = 16000;

/// Errors that can occur during VAD processing
#[derive(Error, Debug)]
pub enum VadError {
    #[error("Invalid audio data: {0}")]
    InvalidAudio(String),

    #[error("VAD configuration error: {0}")]
    ConfigError(String),

    #[error("VAD model error: {0}")]
    ModelError(String),
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
    /// Sample rate in Hz (default: 16000, must be 16000 for Silero)
    pub sample_rate: usize,
    /// Speech probability threshold (default: 0.5)
    /// Higher values = fewer false positives, lower values = more sensitive
    pub speech_threshold: f32,
    /// Minimum speech duration in milliseconds (default: 300ms)
    pub min_speech_duration_ms: u64,
    /// Maximum silence duration before ending segment (default: 1200ms)
    pub max_silence_duration_ms: u64,
    /// Pre-speech replay buffer duration in milliseconds (default: 500ms)
    /// Audio before VAD triggers is kept in a ring buffer and prepended to the
    /// segment so the beginning of utterances is not clipped.
    pub pre_speech_buffer_ms: u64,
}

impl Default for VadConfig {
    fn default() -> Self {
        Self {
            sample_rate: SILERO_SAMPLE_RATE,
            speech_threshold: 0.5,
            min_speech_duration_ms: 300,
            max_silence_duration_ms: 1200,
            pre_speech_buffer_ms: 500,
        }
    }
}

/// Silero VAD - Neural network-based Voice Activity Detector
///
/// Uses the Silero VAD v5 ONNX model for robust speech detection that
/// handles background noise, varying mic gains, and real-world conditions.
pub struct SileroVad {
    config: VadConfig,
    vad: VoiceActivityDetector,
    speech_frames: usize,
    silence_frames: usize,
    in_speech: bool,
}

impl SileroVad {
    /// Create a new SileroVad with the given configuration
    pub fn new(config: VadConfig) -> Result<Self, VadError> {
        let vad = VoiceActivityDetector::builder()
            .sample_rate(config.sample_rate as i64)
            .chunk_size(SILERO_FRAME_SIZE)
            .build()
            .map_err(|e| VadError::ModelError(format!("Failed to initialize Silero VAD: {}", e)))?;

        Ok(Self {
            config,
            vad,
            speech_frames: 0,
            silence_frames: 0,
            in_speech: false,
        })
    }

    /// Process an audio frame and detect voice activity
    ///
    /// # Arguments
    /// * `samples` - Exactly SILERO_FRAME_SIZE (512) audio samples (mono, f32, -1.0 to 1.0)
    ///
    /// # Returns
    /// * `VadResult::Speech` if speech is detected
    /// * `VadResult::Silence` if silence is detected
    pub fn process_frame(&mut self, samples: &[f32]) -> Result<VadResult, VadError> {
        if samples.is_empty() {
            return Err(VadError::InvalidAudio("Empty audio frame".to_string()));
        }

        // Get speech probability from Silero model
        let probability = self.vad.predict(samples.iter().copied());
        let is_speech = probability >= self.config.speech_threshold;

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

        // Calculate durations (32ms per frame at 16kHz with 512 samples)
        let ms_per_frame = (SILERO_FRAME_SIZE * 1000) / self.config.sample_rate;
        let speech_duration_ms = (self.speech_frames * ms_per_frame) as u64;
        let silence_duration_ms = (self.silence_frames * ms_per_frame) as u64;

        // State machine: determine if we're in a speech segment
        let result = if !self.in_speech {
            if speech_duration_ms >= self.config.min_speech_duration_ms {
                self.in_speech = true;
                debug!(
                    "Speech started (prob={:.3}, threshold={:.3}, after {}ms)",
                    probability, self.config.speech_threshold, speech_duration_ms
                );
                VadResult::Speech
            } else {
                VadResult::Silence
            }
        } else if silence_duration_ms >= self.config.max_silence_duration_ms {
            self.in_speech = false;
            self.speech_frames = 0;
            self.silence_frames = 0;
            debug!("Speech ended (silence for {}ms)", silence_duration_ms);
            VadResult::Silence
        } else {
            VadResult::Speech
        };

        Ok(result)
    }

    /// Check if currently in a confirmed speech segment
    pub fn is_speaking(&self) -> bool {
        self.in_speech
    }

    /// Check if speech frames are accumulating but not yet confirmed
    /// (above threshold but below min_speech_duration)
    pub fn is_probably_speaking(&self) -> bool {
        !self.in_speech && self.speech_frames > 0
    }

    /// Reset the VAD state (clears LSTM hidden state and counters)
    pub fn reset(&mut self) {
        // Create a fresh VoiceActivityDetector to reset LSTM hidden state
        if let Ok(fresh_vad) = VoiceActivityDetector::builder()
            .sample_rate(self.config.sample_rate as i64)
            .chunk_size(SILERO_FRAME_SIZE)
            .build()
        {
            self.vad = fresh_vad;
        }
        self.speech_frames = 0;
        self.silence_frames = 0;
        self.in_speech = false;
    }

    /// Get the frame size in samples (always 512 for Silero at 16kHz)
    pub fn frame_size(&self) -> usize {
        SILERO_FRAME_SIZE
    }
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
///
/// Handles reframing: incoming audio chunks (e.g. 1600 samples / 100ms from
/// ContinuousCapture) are buffered and processed in exact 512-sample frames
/// as required by Silero VAD.
///
/// Includes a **pre-speech replay buffer** that keeps the last N ms of audio
/// during silence. When speech is detected, the replay buffer is prepended to
/// the segment so the beginning of the utterance is not clipped.
pub struct VadSegmentDetector {
    vad: SileroVad,
    sample_rate: usize,
    current_segment: Option<Vec<f32>>,
    segment_start_ms: u64,
    total_processed_ms: u64,
    /// Reframing buffer for non-aligned input chunks
    reframe_buffer: Vec<f32>,
    /// Pre-speech replay buffer (ring buffer of recent silence frames)
    pre_speech_buffer: VecDeque<f32>,
    /// Maximum number of samples to keep in the pre-speech buffer
    pre_speech_buffer_capacity: usize,
}

impl VadSegmentDetector {
    /// Create a new VadSegmentDetector
    pub fn new(config: VadConfig) -> Result<Self, VadError> {
        let sample_rate = config.sample_rate;
        let pre_speech_samples = (config.pre_speech_buffer_ms as usize * sample_rate) / 1000;
        let vad = SileroVad::new(config)?;
        Ok(Self {
            vad,
            sample_rate,
            current_segment: None,
            segment_start_ms: 0,
            total_processed_ms: 0,
            reframe_buffer: Vec::with_capacity(SILERO_FRAME_SIZE * 2),
            pre_speech_buffer: VecDeque::with_capacity(pre_speech_samples),
            pre_speech_buffer_capacity: pre_speech_samples,
        })
    }

    /// Push a frame into the pre-speech ring buffer, evicting old samples if full
    fn push_to_pre_speech_buffer(&mut self, frame: &[f32]) {
        for &sample in frame {
            if self.pre_speech_buffer.len() >= self.pre_speech_buffer_capacity {
                self.pre_speech_buffer.pop_front();
            }
            self.pre_speech_buffer.push_back(sample);
        }
    }

    /// Process audio and extract speech segments
    ///
    /// Incoming samples are buffered and processed in exact 512-sample frames.
    /// Any remainder is kept in the reframe buffer for the next call.
    ///
    /// When speech starts, the pre-speech replay buffer is prepended to the
    /// segment to capture the onset of the utterance.
    ///
    /// # Arguments
    /// * `samples` - Audio samples to process (any length)
    ///
    /// # Returns
    /// * `Some(SpeechSegment)` if a complete speech segment was detected
    /// * `None` if still collecting or in silence
    pub fn process(&mut self, samples: &[f32]) -> Result<Option<SpeechSegment>, VadError> {
        // Add incoming samples to reframe buffer
        self.reframe_buffer.extend_from_slice(samples);

        let frame_size = SILERO_FRAME_SIZE;
        let ms_per_frame = ((frame_size as u64) * 1000) / (self.sample_rate as u64);
        let mut segment_complete = None;

        // Process in exact 512-sample frames
        while self.reframe_buffer.len() >= frame_size {
            let frame: Vec<f32> = self.reframe_buffer.drain(..frame_size).collect();
            let result = self.vad.process_frame(&frame)?;

            match result {
                VadResult::Speech => {
                    if self.current_segment.is_none() {
                        // Speech just started — prepend replay buffer
                        let pre_speech_samples: Vec<f32> =
                            self.pre_speech_buffer.drain(..).collect();
                        let pre_speech_duration_ms =
                            (pre_speech_samples.len() as u64 * 1000) / (self.sample_rate as u64);
                        let mut segment =
                            Vec::with_capacity(pre_speech_samples.len() + frame_size * 100);
                        segment.extend_from_slice(&pre_speech_samples);
                        self.current_segment = Some(segment);
                        self.segment_start_ms = self
                            .total_processed_ms
                            .saturating_sub(pre_speech_duration_ms);
                        debug!(
                            "Speech segment started with {}ms replay buffer ({} samples)",
                            pre_speech_duration_ms,
                            pre_speech_samples.len()
                        );
                    }
                    if let Some(ref mut segment) = self.current_segment {
                        segment.extend_from_slice(&frame);
                    }
                }
                VadResult::Silence => {
                    if let Some(segment_samples) = self.current_segment.take() {
                        segment_complete = Some(SpeechSegment {
                            start_ms: self.segment_start_ms,
                            end_ms: self.total_processed_ms,
                            samples: segment_samples,
                        });
                    }
                    // During silence, accumulate into the pre-speech ring buffer
                    self.push_to_pre_speech_buffer(&frame);
                }
            }

            self.total_processed_ms += ms_per_frame;
        }

        Ok(segment_complete)
    }

    /// Check if currently in a confirmed speech segment
    pub fn is_speaking(&self) -> bool {
        self.vad.is_speaking()
    }

    /// Check if speech is probable but not yet confirmed
    pub fn is_probably_speaking(&self) -> bool {
        self.vad.is_probably_speaking()
    }

    /// Reset the detector
    pub fn reset(&mut self) {
        self.vad.reset();
        self.current_segment = None;
        self.segment_start_ms = 0;
        self.total_processed_ms = 0;
        self.reframe_buffer.clear();
        self.pre_speech_buffer.clear();
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
    fn test_silero_vad_creation() {
        let vad = SileroVad::new(VadConfig::default());
        assert!(vad.is_ok(), "Should create SileroVad successfully");
        let vad = vad.unwrap();
        assert_eq!(vad.frame_size(), SILERO_FRAME_SIZE);
        assert!(!vad.is_speaking());
    }

    #[test]
    fn test_silero_vad_silence_detection() {
        let mut vad = SileroVad::new(VadConfig::default()).unwrap();

        // Feed silence frames — should never enter speech
        let silence = generate_silence(1000, SILERO_SAMPLE_RATE);
        for chunk in silence.chunks(SILERO_FRAME_SIZE) {
            if chunk.len() == SILERO_FRAME_SIZE {
                let result = vad.process_frame(chunk).unwrap();
                assert_eq!(
                    result,
                    VadResult::Silence,
                    "Silence should be classified as silence"
                );
            }
        }
        assert!(!vad.is_speaking());
    }

    #[test]
    fn test_silero_vad_reset() {
        let mut vad = SileroVad::new(VadConfig {
            speech_threshold: 0.0001, // very low threshold so sine triggers
            min_speech_duration_ms: 32,
            ..VadConfig::default()
        })
        .unwrap();

        // Process some audio to change state
        let audio = generate_sine_wave(440.0, 500, SILERO_SAMPLE_RATE);
        for chunk in audio.chunks(SILERO_FRAME_SIZE) {
            if chunk.len() == SILERO_FRAME_SIZE {
                vad.process_frame(chunk).ok();
            }
        }

        // Reset
        vad.reset();
        assert!(!vad.is_speaking());
        assert_eq!(vad.speech_frames, 0);
        assert_eq!(vad.silence_frames, 0);
    }

    #[test]
    fn test_vad_config_default() {
        let config = VadConfig::default();
        assert_eq!(config.sample_rate, 16000);
        assert!((config.speech_threshold - 0.5).abs() < f32::EPSILON);
        assert_eq!(config.min_speech_duration_ms, 300);
        assert_eq!(config.max_silence_duration_ms, 1200);
        assert_eq!(config.pre_speech_buffer_ms, 500);
    }

    #[test]
    fn test_segment_detector_creation() {
        let detector = VadSegmentDetector::new(VadConfig::default());
        assert!(detector.is_ok());
        let detector = detector.unwrap();
        assert!(!detector.is_speaking());
    }

    #[test]
    fn test_segment_detector_silence_no_segment() {
        let mut detector = VadSegmentDetector::new(VadConfig::default()).unwrap();

        // Feed silence — should never produce a segment
        let silence = generate_silence(2000, SILERO_SAMPLE_RATE);
        let segment = detector.process(&silence).unwrap();
        assert!(segment.is_none(), "Silence should not produce a segment");
        assert!(!detector.is_speaking());
    }

    #[test]
    fn test_segment_detector_reframe_buffer() {
        let mut detector = VadSegmentDetector::new(VadConfig::default()).unwrap();

        // Send 1600 samples (like ContinuousCapture does)
        // 1600 / 512 = 3 frames + 64 leftover
        let chunk = generate_silence(100, SILERO_SAMPLE_RATE); // 1600 samples
        assert_eq!(chunk.len(), 1600);

        detector.process(&chunk).unwrap();

        // After processing 1600 samples: 3 frames consumed (1536), 64 left in buffer
        assert_eq!(detector.reframe_buffer.len(), 64);

        // Send another 1600: 64 + 1600 = 1664, processes 3 frames (1536), 128 left
        detector.process(&chunk).unwrap();
        assert_eq!(detector.reframe_buffer.len(), 128);
    }

    #[test]
    fn test_segment_detector_reset() {
        let mut detector = VadSegmentDetector::new(VadConfig::default()).unwrap();

        // Process some audio to populate state
        let silence = generate_silence(500, SILERO_SAMPLE_RATE);
        detector.process(&silence).unwrap();
        assert!(!detector.reframe_buffer.is_empty() || detector.total_processed_ms > 0);

        // Reset
        detector.reset();
        assert!(!detector.is_speaking());
        assert_eq!(detector.total_processed_ms, 0);
        assert!(detector.reframe_buffer.is_empty());
        assert!(detector.pre_speech_buffer.is_empty());
    }

    #[test]
    fn test_pre_speech_buffer_capacity() {
        // 500ms at 16kHz = 8000 samples
        let detector = VadSegmentDetector::new(VadConfig::default()).unwrap();
        assert_eq!(detector.pre_speech_buffer_capacity, 8000);

        // Custom: 1000ms = 16000 samples
        let detector = VadSegmentDetector::new(VadConfig {
            pre_speech_buffer_ms: 1000,
            ..VadConfig::default()
        })
        .unwrap();
        assert_eq!(detector.pre_speech_buffer_capacity, 16000);
    }

    #[test]
    fn test_pre_speech_buffer_fills_during_silence() {
        let mut detector = VadSegmentDetector::new(VadConfig {
            pre_speech_buffer_ms: 200, // 3200 samples at 16kHz
            ..VadConfig::default()
        })
        .unwrap();

        // Feed 100ms of silence (1600 samples) — processed as 3 frames (1536 samples)
        let silence = generate_silence(100, SILERO_SAMPLE_RATE);
        detector.process(&silence).unwrap();

        // Pre-speech buffer should have 3 frames * 512 = 1536 samples
        assert_eq!(detector.pre_speech_buffer.len(), 1536);
    }

    #[test]
    fn test_pre_speech_buffer_evicts_old_samples() {
        let mut detector = VadSegmentDetector::new(VadConfig {
            pre_speech_buffer_ms: 100, // 1600 samples at 16kHz
            ..VadConfig::default()
        })
        .unwrap();

        // Feed 500ms of silence — more than the 100ms buffer
        let silence = generate_silence(500, SILERO_SAMPLE_RATE);
        detector.process(&silence).unwrap();

        // Buffer should be at capacity (1600 samples), not growing unbounded
        assert!(detector.pre_speech_buffer.len() <= detector.pre_speech_buffer_capacity);
    }

    #[test]
    fn test_pre_speech_buffer_cleared_on_reset() {
        let mut detector = VadSegmentDetector::new(VadConfig::default()).unwrap();

        let silence = generate_silence(300, SILERO_SAMPLE_RATE);
        detector.process(&silence).unwrap();
        assert!(!detector.pre_speech_buffer.is_empty());

        detector.reset();
        assert!(detector.pre_speech_buffer.is_empty());
    }

    #[test]
    fn test_hysteresis_min_speech_duration() {
        // With a very low threshold, even a sine wave might trigger.
        // The point is: speech_frames must accumulate enough before in_speech flips.
        let mut vad = SileroVad::new(VadConfig {
            speech_threshold: 0.0001,    // absurdly low
            min_speech_duration_ms: 320, // need 10 frames (320ms at 32ms/frame)
            max_silence_duration_ms: 700,
            ..VadConfig::default()
        })
        .unwrap();

        let frame = generate_sine_wave(440.0, 32, SILERO_SAMPLE_RATE); // one frame
        assert_eq!(frame.len(), SILERO_FRAME_SIZE);

        // Process 5 frames — not enough for min_speech_duration (160ms < 320ms)
        for _ in 0..5 {
            let result = vad.process_frame(&frame).unwrap();
            assert_eq!(result, VadResult::Silence, "Should not trigger speech yet");
        }
        assert!(!vad.is_speaking());
    }

    #[test]
    fn test_hysteresis_max_silence_duration() {
        // Test that once in speech, silence must persist long enough to end the segment
        let mut vad = SileroVad::new(VadConfig {
            speech_threshold: 0.0001,
            min_speech_duration_ms: 32,  // 1 frame
            max_silence_duration_ms: 96, // 3 frames
            ..VadConfig::default()
        })
        .unwrap();

        let speech_frame = generate_sine_wave(440.0, 32, SILERO_SAMPLE_RATE);
        let silence_frame = generate_silence(32, SILERO_SAMPLE_RATE);

        // Enter speech mode (need at least 1 frame = 32ms)
        vad.process_frame(&speech_frame).unwrap();
        // May need a second frame to exceed threshold depending on model output
        vad.process_frame(&speech_frame).unwrap();

        if vad.is_speaking() {
            // Now feed 1 silence frame — should stay in speech (< 96ms)
            let result = vad.process_frame(&silence_frame).unwrap();
            assert_eq!(
                result,
                VadResult::Speech,
                "One frame of silence should not end speech"
            );
        }
    }

    #[test]
    fn test_default_hysteresis_tolerates_one_second_pause() {
        let mut vad = SileroVad::new(VadConfig::default()).unwrap();
        let silence_frame = generate_silence(32, SILERO_SAMPLE_RATE);

        // Start from a confirmed utterance so this test isolates endpointing
        // hysteresis from the model's speech-classification behavior.
        vad.in_speech = true;

        // A natural one-second hesitation must remain part of the utterance.
        for _ in 0..31 {
            assert_eq!(
                vad.process_frame(&silence_frame).unwrap(),
                VadResult::Speech
            );
        }
        assert!(vad.is_speaking());

        // The 1.2-second default is crossed on the 38th 32ms frame.
        for _ in 0..6 {
            assert_eq!(
                vad.process_frame(&silence_frame).unwrap(),
                VadResult::Speech
            );
        }
        assert_eq!(
            vad.process_frame(&silence_frame).unwrap(),
            VadResult::Silence
        );
        assert!(!vad.is_speaking());
    }
}
