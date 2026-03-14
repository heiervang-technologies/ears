//! Streaming transcription with LocalAgreement policy
//!
//! This module implements real-time streaming transcription with Voice Activity Detection (VAD).
//! It uses the LocalAgreement policy to ensure only stable text prefixes are committed and typed.

use std::collections::VecDeque;
use thiserror::Error;

/// Errors that can occur during streaming transcription
#[derive(Error, Debug)]
pub enum StreamingError {
    #[error("Audio buffer overflow")]
    BufferOverflow,

    #[error("Transcription backend error: {0}")]
    BackendError(String),

    #[error("VAD error: {0}")]
    VadError(String),
}

/// Represents a chunk of transcribed text
#[derive(Debug, Clone)]
pub struct TranscriptChunk {
    /// The transcribed text for this chunk
    pub text: String,
    /// Timestamp in milliseconds from start
    pub timestamp_ms: u64,
    /// Confidence score (0.0 - 1.0)
    pub confidence: f32,
}

/// LocalAgreement policy for determining stable text prefixes
///
/// This policy ensures that text is only committed when n consecutive
/// transcription iterations agree on the same prefix. This prevents
/// partial words from being typed.
pub struct LocalAgreementPolicy {
    /// Number of consecutive agreements required
    n: usize,
    /// History of recent transcripts
    history: VecDeque<String>,
    /// The committed (stable) prefix
    committed: String,
}

impl LocalAgreementPolicy {
    /// Create a new LocalAgreementPolicy
    ///
    /// # Arguments
    /// * `n` - Number of consecutive agreements required (default: 2)
    pub fn new(n: usize) -> Self {
        Self {
            n: n.max(1), // Ensure at least 1
            history: VecDeque::with_capacity(n),
            committed: String::new(),
        }
    }

    /// Process a new transcript and return stable and unstable portions
    ///
    /// # Returns
    /// * `newly_committed` - Text that just became stable (should be typed)
    /// * `uncommitted` - Text that is not yet stable (show in TUI as gray)
    pub fn process(&mut self, new_transcript: String) -> (String, String) {
        // Add to history
        self.history.push_back(new_transcript.clone());
        if self.history.len() > self.n {
            self.history.pop_front();
        }

        // Find longest common prefix across all history
        let stable_prefix = self.find_common_prefix();

        // Calculate what's newly committed
        let newly_committed = if stable_prefix.len() > self.committed.len() {
            stable_prefix[self.committed.len()..].to_string()
        } else {
            String::new()
        };

        // Calculate uncommitted portion
        let uncommitted = if new_transcript.len() > stable_prefix.len() {
            new_transcript[stable_prefix.len()..].to_string()
        } else {
            String::new()
        };

        // Update committed prefix
        self.committed = stable_prefix;

        (newly_committed, uncommitted)
    }

    /// Find the longest common prefix across all items in history
    fn find_common_prefix(&self) -> String {
        if self.history.len() < self.n {
            // Not enough history for agreement yet
            return String::new();
        }

        // Start with the first transcript
        let mut prefix = self.history[0].clone();

        // Find common prefix with all subsequent transcripts
        for transcript in self.history.iter().skip(1) {
            prefix = common_prefix(&prefix, transcript);
            if prefix.is_empty() {
                break;
            }
        }

        prefix
    }

    /// Get the currently committed text
    pub fn committed(&self) -> &str {
        &self.committed
    }

    /// Reset the policy (clear history and committed text)
    pub fn reset(&mut self) {
        self.history.clear();
        self.committed.clear();
    }
}

/// Find the longest common prefix of two strings
fn common_prefix(a: &str, b: &str) -> String {
    let mut prefix = String::new();
    let chars_a: Vec<char> = a.chars().collect();
    let chars_b: Vec<char> = b.chars().collect();

    for (ca, cb) in chars_a.iter().zip(chars_b.iter()) {
        if ca == cb {
            prefix.push(*ca);
        } else {
            break;
        }
    }

    prefix
}

/// Audio buffer for streaming transcription
///
/// Manages a circular buffer of audio samples for continuous recording
pub struct AudioBuffer {
    /// Circular buffer of f32 audio samples (mono, 16kHz)
    buffer: Vec<f32>,
    /// Current write position
    write_pos: usize,
    /// Total samples written (for overflow detection)
    total_written: usize,
    /// Buffer capacity in samples
    capacity: usize,
}

impl AudioBuffer {
    /// Create a new AudioBuffer
    ///
    /// # Arguments
    /// * `duration_seconds` - Duration to buffer in seconds
    /// * `sample_rate` - Sample rate in Hz (default: 16000)
    pub fn new(duration_seconds: usize, sample_rate: usize) -> Self {
        let capacity = duration_seconds * sample_rate;
        Self {
            buffer: vec![0.0; capacity],
            write_pos: 0,
            total_written: 0,
            capacity,
        }
    }

    /// Write audio samples to the buffer
    ///
    /// # Arguments
    /// * `samples` - Audio samples to write (mono, f32, -1.0 to 1.0)
    pub fn write(&mut self, samples: &[f32]) {
        for &sample in samples {
            self.buffer[self.write_pos] = sample;
            self.write_pos = (self.write_pos + 1) % self.capacity;
            self.total_written += 1;
        }
    }

    /// Read the most recent samples from the buffer
    ///
    /// # Arguments
    /// * `num_samples` - Number of samples to read
    ///
    /// # Returns
    /// A Vec of the most recent samples, in chronological order
    pub fn read_recent(&self, num_samples: usize) -> Vec<f32> {
        let num_samples = num_samples.min(self.capacity).min(self.total_written);
        let mut result = Vec::with_capacity(num_samples);

        // Calculate start position
        let start_pos = if self.total_written < self.capacity {
            0
        } else {
            self.write_pos
        };

        // Read samples in order
        for i in 0..num_samples {
            let pos = (start_pos + i) % self.capacity;
            result.push(self.buffer[pos]);
        }

        result
    }

    /// Get the total number of samples written
    pub fn total_written(&self) -> usize {
        self.total_written
    }

    /// Clear the buffer
    pub fn clear(&mut self) {
        self.buffer.fill(0.0);
        self.write_pos = 0;
        self.total_written = 0;
    }
}

/// Configuration for streaming transcription
#[derive(Debug, Clone)]
pub struct StreamingConfig {
    /// Chunk size in milliseconds for transcription
    pub chunk_size_ms: u64,
    /// Audio buffer size in seconds
    pub buffer_size_seconds: usize,
    /// LocalAgreement threshold (n)
    pub agreement_threshold: usize,
    /// Enable progressive typing
    pub progressive_typing: bool,
    /// Enable auto-correction (backspace and retype)
    pub auto_correction: bool,
}

impl Default for StreamingConfig {
    fn default() -> Self {
        Self {
            chunk_size_ms: 500,
            buffer_size_seconds: 10,
            agreement_threshold: 2,
            progressive_typing: false,
            auto_correction: false,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_common_prefix() {
        assert_eq!(common_prefix("hello", "hello"), "hello");
        assert_eq!(common_prefix("hello", "help"), "hel");
        assert_eq!(common_prefix("hello", "world"), "");
        assert_eq!(common_prefix("", "hello"), "");
        assert_eq!(common_prefix("hello", ""), "");
    }

    #[test]
    fn test_local_agreement_basic() {
        let mut policy = LocalAgreementPolicy::new(2);

        // First iteration: "Hello"
        let (committed, uncommitted) = policy.process("Hello".to_string());
        assert_eq!(committed, ""); // Not enough history
        assert_eq!(uncommitted, "Hello");

        // Second iteration: "Hello wo"
        let (committed, uncommitted) = policy.process("Hello wo".to_string());
        assert_eq!(committed, "Hello"); // "Hello" is now stable
        assert_eq!(uncommitted, " wo");

        // Third iteration: "Hello world"
        let (committed, uncommitted) = policy.process("Hello world".to_string());
        assert_eq!(committed, " wo"); // " wo" is now stable
        assert_eq!(uncommitted, "rld");

        // Fourth iteration: "Hello world"
        let (committed, uncommitted) = policy.process("Hello world".to_string());
        assert_eq!(committed, "rld"); // "rld" is now stable
        assert_eq!(uncommitted, "");

        assert_eq!(policy.committed(), "Hello world");
    }

    #[test]
    fn test_local_agreement_correction() {
        let mut policy = LocalAgreementPolicy::new(2);

        // First: "Hello word"
        policy.process("Hello word".to_string());

        // Second: "Hello word" (agrees)
        let (committed, _) = policy.process("Hello word".to_string());
        assert_eq!(committed, "Hello word");

        // Third: "Hello world" (corrects previous)
        let (committed, uncommitted) = policy.process("Hello world".to_string());
        // Only "Hello wor" is stable now (common prefix)
        assert_eq!(committed, "");
        assert_eq!(uncommitted, "ld");
    }

    #[test]
    fn test_audio_buffer_write_read() {
        let mut buffer = AudioBuffer::new(1, 16000); // 1 second at 16kHz

        // Write some samples
        let samples = vec![0.1, 0.2, 0.3, 0.4, 0.5];
        buffer.write(&samples);

        // Read them back
        let read_samples = buffer.read_recent(5);
        assert_eq!(read_samples, samples);
    }

    #[test]
    fn test_audio_buffer_circular() {
        let mut buffer = AudioBuffer::new(1, 10); // Small buffer: 10 samples

        // Write 15 samples (more than capacity)
        let samples: Vec<f32> = (0..15).map(|i| i as f32).collect();
        buffer.write(&samples);

        // Should only have the last 10 samples
        let read_samples = buffer.read_recent(10);
        let expected: Vec<f32> = (5..15).map(|i| i as f32).collect();
        assert_eq!(read_samples, expected);
    }

    #[test]
    fn test_local_agreement_reset() {
        let mut policy = LocalAgreementPolicy::new(2);

        policy.process("Hello".to_string());
        policy.process("Hello world".to_string());

        assert!(!policy.committed().is_empty());

        policy.reset();

        assert_eq!(policy.committed(), "");
        assert_eq!(policy.history.len(), 0);
    }

    #[test]
    fn test_streaming_config_default() {
        let config = StreamingConfig::default();

        assert_eq!(config.chunk_size_ms, 500);
        assert_eq!(config.buffer_size_seconds, 10);
        assert_eq!(config.agreement_threshold, 2);
        assert!(!config.progressive_typing);
        assert!(!config.auto_correction);
    }

    #[test]
    fn test_common_prefix_multibyte() {
        // Multi-byte UTF-8 characters
        assert_eq!(common_prefix("café", "cafétéria"), "café");
        assert_eq!(common_prefix("cafétéria", "café"), "café");
        assert_eq!(common_prefix("日本語", "日本人"), "日本");
        assert_eq!(common_prefix("日本語", "中国語"), "");
    }

    #[test]
    fn test_local_agreement_empty_transcripts() {
        let mut policy = LocalAgreementPolicy::new(2);

        let (committed, uncommitted) = policy.process(String::new());
        assert_eq!(committed, "");
        assert_eq!(uncommitted, "");

        let (committed, uncommitted) = policy.process(String::new());
        assert_eq!(committed, "");
        assert_eq!(uncommitted, "");
    }

    #[test]
    fn test_local_agreement_n_equals_1() {
        let mut policy = LocalAgreementPolicy::new(1);

        // With n=1, everything commits immediately
        let (committed, uncommitted) = policy.process("Hello".to_string());
        assert_eq!(committed, "Hello");
        assert_eq!(uncommitted, "");

        let (committed, uncommitted) = policy.process("Hello world".to_string());
        assert_eq!(committed, " world");
        assert_eq!(uncommitted, "");

        assert_eq!(policy.committed(), "Hello world");
    }

    #[test]
    fn test_local_agreement_n_equals_3() {
        let mut policy = LocalAgreementPolicy::new(3);

        // First two iterations: not enough history
        let (committed, _) = policy.process("Hello".to_string());
        assert_eq!(committed, "");

        let (committed, _) = policy.process("Hello world".to_string());
        assert_eq!(committed, "");

        // Third iteration: now we have 3 items in history
        let (committed, uncommitted) = policy.process("Hello world!".to_string());
        assert_eq!(committed, "Hello");
        assert_eq!(uncommitted, " world!");
    }

    #[test]
    fn test_audio_buffer_empty_read() {
        let buffer = AudioBuffer::new(1, 16000);

        // No writes, reading should return empty
        let samples = buffer.read_recent(100);
        assert_eq!(samples.len(), 0);
    }

    #[test]
    fn test_audio_buffer_read_more_than_written() {
        let mut buffer = AudioBuffer::new(1, 16000);

        // Write only 3 samples
        buffer.write(&[0.1, 0.2, 0.3]);

        // Request more than written
        let samples = buffer.read_recent(100);
        assert_eq!(samples.len(), 3);
        assert_eq!(samples, vec![0.1, 0.2, 0.3]);
    }
}
