//! Streaming transcription engine
//!
//! This module coordinates all components for real-time streaming transcription:
//! - Audio buffering
//! - VAD (Voice Activity Detection)
//! - Whisper transcription
//! - LocalAgreement policy
//! - Progressive typing

use crate::desktop::{TextInput, TypingMode};
use crate::progressive_typing::{ProgressiveTypingConfig, ProgressiveTypingEngine};
use crate::streaming::{AudioBuffer, LocalAgreementPolicy, StreamingConfig};
use crate::text_filters::TextFilters;
use crate::vad::{SpeechSegment, VadConfig, VadSegmentDetector};
use crate::whisper::WhisperClient;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Instant;
use thiserror::Error;
use tokio::sync::mpsc;
use tracing::{debug, info, warn};

/// Errors that can occur in the streaming engine
#[derive(Error, Debug)]
pub enum StreamingEngineError {
    #[error("VAD error: {0}")]
    VadError(String),

    #[error("Transcription error: {0}")]
    TranscriptionError(String),

    #[error("Audio error: {0}")]
    AudioError(String),

    #[error("Progressive typing error: {0}")]
    ProgressiveTypingError(String),

    #[error("Engine not running")]
    NotRunning,
}

/// Events emitted by the streaming engine
#[derive(Debug, Clone, serde::Serialize)]
pub enum StreamingEvent {
    /// VAD detected probable speech (first frames above threshold, before min duration met)
    SpeechProbable,

    /// VAD confirmed speech (min duration threshold met)
    SpeechStarted,

    /// VAD detected end of speech
    SpeechEnded,

    /// A probable-speech candidate (after `SpeechProbable`) dropped below the
    /// threshold before it was confirmed. No segment is produced. Consumers
    /// that reacted to `SpeechProbable` (e.g. volume ducking) should undo
    /// that reaction here. Carries no audio cue.
    SpeechRejected,

    /// Audio capture ended without being asked to (device unplugged,
    /// PipeWire restart, pw-record exit). No further audio will arrive; the
    /// owner must stop claiming to listen.
    CaptureStopped { reason: String },

    /// New transcript chunk received
    TranscriptUpdate {
        committed: String,
        uncommitted: String,
    },

    /// Transcription segment completed
    SegmentCompleted { text: String, duration_ms: u64 },

    /// Error occurred
    Error(String),

    /// Stats update
    StatsUpdate {
        segments_processed: usize,
        avg_latency_ms: u64,
    },
}

/// Statistics for the streaming engine
#[derive(Debug, Clone, Default)]
pub struct StreamingStats {
    /// Total number of segments processed
    pub segments_processed: usize,
    /// Total latency in milliseconds (sum of all segments)
    pub total_latency_ms: u64,
    /// Average latency in milliseconds
    pub avg_latency_ms: u64,
    /// Number of characters typed
    pub chars_typed: usize,
    /// Number of corrections made
    pub corrections_made: usize,
}

/// Run a blocking, subprocess-driving closure without stalling the async
/// runtime's worker thread.
///
/// Typing helpers (`wtype`, `ydotool`) block on child processes. On the
/// multi-threaded runtime this hands the current worker to the closure via
/// `block_in_place`, so other tasks (capture reader, event loop, IPC) keep
/// running. On a current-thread runtime (tests) it just runs inline.
/// The children themselves are bounded by `desktop::run_bounded`, so the
/// closure is guaranteed to return.
fn run_blocking<T>(f: impl FnOnce() -> T) -> T {
    match tokio::runtime::Handle::try_current() {
        Ok(handle) if handle.runtime_flavor() == tokio::runtime::RuntimeFlavor::MultiThread => {
            tokio::task::block_in_place(f)
        }
        _ => f(),
    }
}

/// Main streaming transcription engine
pub struct StreamingEngine {
    /// Audio buffer for continuous capture
    audio_buffer: AudioBuffer,

    /// VAD segment detector
    vad_detector: VadSegmentDetector,

    /// LocalAgreement policy for stable text
    local_agreement: LocalAgreementPolicy,

    /// Progressive typing engine
    progressive_typing: ProgressiveTypingEngine,

    /// Whisper client for transcription
    whisper_client: Arc<WhisperClient>,

    /// Configuration
    config: StreamingConfig,

    /// Statistics
    stats: StreamingStats,

    /// Event sender
    event_tx: Option<mpsc::UnboundedSender<StreamingEvent>>,

    /// Temporary directory for audio segments
    temp_dir: PathBuf,

    /// Accumulated committed text across all segments (for progressive typing)
    accumulated_text: String,

    /// Track previous speaking state to emit SpeechStarted only on transition
    was_speaking: bool,

    /// Track previous probable-speaking state to emit SpeechProbable only on transition
    was_probably_speaking: bool,

    /// Whether to send Enter key after each segment
    auto_enter: bool,

    /// Current typing mode (needed for send_enter)
    typing_mode: TypingMode,

    /// Text filters (lowercase, remove punctuation, etc.)
    text_filters: TextFilters,

    /// Language for text filter alphabet checking
    language: Option<String>,

    /// Active guided grammar (bash mode). When set, transcription is routed to
    /// the constrained chat-completions path and text filters are bypassed.
    guided_grammar: Option<String>,
}

impl StreamingEngine {
    /// Create a new StreamingEngine
    pub fn new(
        whisper_client: Arc<WhisperClient>,
        config: StreamingConfig,
        vad_config: VadConfig,
        typing_config: ProgressiveTypingConfig,
        temp_dir: PathBuf,
    ) -> Result<Self, StreamingEngineError> {
        let audio_buffer = AudioBuffer::new(config.buffer_size_seconds, vad_config.sample_rate);

        let vad_detector = VadSegmentDetector::new(vad_config)
            .map_err(|e| StreamingEngineError::VadError(e.to_string()))?;
        let local_agreement = LocalAgreementPolicy::new(config.agreement_threshold);
        let progressive_typing = ProgressiveTypingEngine::new(typing_config);

        Ok(Self {
            audio_buffer,
            vad_detector,
            local_agreement,
            progressive_typing,
            whisper_client,
            config,
            stats: StreamingStats::default(),
            event_tx: None,
            temp_dir,
            accumulated_text: String::new(),
            was_speaking: false,
            was_probably_speaking: false,
            auto_enter: false,
            typing_mode: TypingMode::Auto,
            text_filters: TextFilters::default(),
            language: None,
            guided_grammar: None,
        })
    }

    /// Set event sender for receiving streaming events
    pub fn set_event_sender(&mut self, tx: mpsc::UnboundedSender<StreamingEvent>) {
        self.event_tx = Some(tx);
    }

    /// Process audio samples
    ///
    /// # Arguments
    /// * `samples` - Audio samples (mono, f32, -1.0 to 1.0, 16kHz)
    pub async fn process_audio(&mut self, samples: &[f32]) -> Result<(), StreamingEngineError> {
        // Add to audio buffer
        self.audio_buffer.write(samples);

        // Process with VAD to detect speech segments
        let outcome = match self.vad_detector.process(samples) {
            Ok(outcome) => outcome,
            Err(e) => {
                warn!("VAD error: {}", e);
                self.send_event(StreamingEvent::Error(format!("VAD error: {}", e)));
                return Ok(());
            }
        };

        if let Some(segment) = self.handle_vad_outcome(outcome) {
            self.process_segment(segment).await?;
        }

        Ok(())
    }

    /// Translate the detector's state after a chunk into transition events.
    ///
    /// Returns the completed segment (if any) for downstream processing.
    /// Emits exactly one of the speech transition events per edge:
    /// `SpeechProbable` (candidate started), `SpeechStarted` (confirmed),
    /// `SpeechEnded` (segment complete) or `SpeechRejected` (candidate
    /// dropped before confirmation).
    fn handle_vad_outcome(&mut self, outcome: Option<SpeechSegment>) -> Option<SpeechSegment> {
        if let Some(segment) = outcome {
            // Complete speech segment detected
            self.was_speaking = false;
            self.was_probably_speaking = false;
            self.send_event(StreamingEvent::SpeechEnded);
            return Some(segment);
        }

        let is_probable = self.vad_detector.is_probably_speaking();
        let is_speaking = self.vad_detector.is_speaking();

        // Fire SpeechProbable on first speech frames (before min duration met)
        if is_probable && !self.was_probably_speaking {
            self.send_event(StreamingEvent::SpeechProbable);
        }

        // Fire SpeechStarted only on the false→true transition (confirmed)
        if is_speaking && !self.was_speaking {
            self.send_event(StreamingEvent::SpeechStarted);
        }

        // A candidate we announced fell back to silence without ever being
        // confirmed: tell listeners so they can undo the probable reaction.
        if self.was_probably_speaking && !is_probable && !is_speaking && !self.was_speaking {
            debug!("Speech candidate rejected before confirmation");
            self.send_event(StreamingEvent::SpeechRejected);
        }

        self.was_probably_speaking = is_probable;
        self.was_speaking = is_speaking;
        None
    }

    /// Process a complete speech segment
    async fn process_segment(
        &mut self,
        segment: SpeechSegment,
    ) -> Result<(), StreamingEngineError> {
        // Skip segments with no audio data — sending an empty WAV crashes
        // some ASR backends (e.g., Qwen3-ASR ValueError on empty array).
        if segment.samples.is_empty() {
            debug!("Skipping empty speech segment");
            return Ok(());
        }

        let start_time = Instant::now();

        debug!(
            "Processing speech segment: {} - {} ms ({} samples)",
            segment.start_ms,
            segment.end_ms,
            segment.samples.len()
        );

        // Save segment to temporary WAV file
        let wav_start = Instant::now();
        let segment_file = self
            .temp_dir
            .join(format!("segment_{}.wav", self.stats.segments_processed));
        self.save_wav(&segment_file, &segment.samples)
            .map_err(|e| StreamingEngineError::AudioError(e.to_string()))?;
        debug!("WAV save took {:?}", wav_start.elapsed());

        // Transcribe with Whisper. In bash mode a guided grammar routes the
        // request to the constrained chat-completions path.
        let transcribe_start = Instant::now();
        let transcript = match self
            .whisper_client
            .transcribe_with_grammar(&segment_file, self.guided_grammar.as_deref())
            .await
        {
            Ok(text) => text,
            Err(e) => {
                warn!("Transcription error: {}", e);
                self.send_event(StreamingEvent::Error(format!("Transcription error: {}", e)));
                return Err(StreamingEngineError::TranscriptionError(e.to_string()));
            }
        };

        info!("Transcription took {:?}", transcribe_start.elapsed());

        // Clean up temp file
        let _ = std::fs::remove_file(&segment_file);

        if transcript.is_empty() {
            debug!("Empty transcript, skipping");
            return Ok(());
        }

        // Apply text filters (lowercase, remove punctuation, strict alphabet).
        // Bash mode bypasses them: the grammar already guarantees valid shell
        // syntax, and the filters would mangle it (lowercase/strip punctuation).
        let transcript = if self.guided_grammar.is_some() {
            transcript
        } else {
            self.text_filters
                .apply(&transcript, self.language.as_deref())
        };
        if transcript.is_empty() {
            debug!("Transcript filtered out (empty after filters)");
            return Ok(());
        }

        info!("Transcribed: {}", transcript);

        // Each VAD segment is a discrete utterance. Reset agreement state so
        // the previous segment's text doesn't interfere, then feed the
        // transcript twice to force LocalAgreement to commit it immediately.
        self.local_agreement.reset();
        self.local_agreement.process(transcript.clone());
        let (newly_committed, _uncommitted) = self.local_agreement.process(transcript.clone());

        // Accumulate committed text across segments (space-separated)
        if !newly_committed.is_empty() {
            if !self.accumulated_text.is_empty() {
                self.accumulated_text.push(' ');
            }
            self.accumulated_text.push_str(&newly_committed);
        }

        // Update progressive typing with the full accumulated text
        if self.config.progressive_typing && !newly_committed.is_empty() {
            let typing_start = Instant::now();
            let progressive_typing = &mut self.progressive_typing;
            let accumulated = &self.accumulated_text;
            let outcome = run_blocking(|| progressive_typing.update(accumulated));
            let delivered = self.handle_typing_outcome(outcome, typing_start);

            // Send Enter only after typing that we know completed. After a
            // failure (including a timeout) the screen may hold a partial
            // command; submitting it would execute something nobody said.
            if self.auto_enter && delivered {
                if let Err(e) = run_blocking(TextInput::send_enter) {
                    warn!("Failed to send Enter key: {}", e);
                }
            }
        }

        // Update stats
        let latency_ms = start_time.elapsed().as_millis() as u64;
        self.stats.segments_processed += 1;
        self.stats.total_latency_ms += latency_ms;
        if self.stats.segments_processed > 0 {
            self.stats.avg_latency_ms =
                self.stats.total_latency_ms / self.stats.segments_processed as u64;
        }

        // Send events
        self.send_event(StreamingEvent::TranscriptUpdate {
            committed: self.accumulated_text.clone(),
            uncommitted: String::new(),
        });

        self.send_event(StreamingEvent::SegmentCompleted {
            text: transcript,
            duration_ms: segment.end_ms - segment.start_ms,
        });

        self.send_event(StreamingEvent::StatsUpdate {
            segments_processed: self.stats.segments_processed,
            avg_latency_ms: self.stats.avg_latency_ms,
        });

        info!(
            "Segment #{} total: {:?} (avg latency: {}ms)",
            self.stats.segments_processed,
            start_time.elapsed(),
            self.stats.avg_latency_ms
        );

        Ok(())
    }

    /// Account for a progressive-typing attempt. Returns whether the text is
    /// known to have been delivered.
    ///
    /// On failure the typed state is *discarded*, not retried: a timed-out
    /// child may have delivered part of the text, so neither the engine nor
    /// the progressive typer can know what is on screen. Carrying the old
    /// accumulated text forward would replay it (duplicate output) or, with
    /// auto-correction, backspace over characters that were never typed. The
    /// next segment therefore starts from a clean slate and types only its
    /// own words.
    fn handle_typing_outcome(
        &mut self,
        outcome: Result<usize, crate::progressive_typing::ProgressiveTypingError>,
        typing_start: Instant,
    ) -> bool {
        match outcome {
            Ok(chars) => {
                info!("Typed {} characters in {:?}", chars, typing_start.elapsed());
                self.stats.chars_typed += chars;
                true
            }
            Err(e) => {
                warn!(
                    "Progressive typing error after {:?}: {}; discarding uncertain typed state",
                    typing_start.elapsed(),
                    e
                );
                self.send_event(StreamingEvent::Error(format!(
                    "Typing error: {} (output may be partial; not retried)",
                    e
                )));
                self.discard_uncertain_typing_state();
                false
            }
        }
    }

    /// Forget everything about what has been typed so far. Used after a
    /// typing failure, when the on-screen state is unknown.
    fn discard_uncertain_typing_state(&mut self) {
        self.local_agreement.reset();
        self.progressive_typing.reset();
        self.accumulated_text.clear();
    }

    /// Save audio samples to WAV file
    fn save_wav(&self, path: &PathBuf, samples: &[f32]) -> Result<(), std::io::Error> {
        use std::fs::File;
        use std::io::Write;

        let mut file = File::create(path)?;

        // WAV header
        let sample_rate = 16000u32;
        let num_channels = 1u16;
        let bits_per_sample = 16u16;
        let byte_rate = sample_rate * num_channels as u32 * bits_per_sample as u32 / 8;
        let block_align = num_channels * bits_per_sample / 8;
        let data_size = (samples.len() * 2) as u32; // 16-bit samples
        let file_size = 36 + data_size;

        // RIFF header
        file.write_all(b"RIFF")?;
        file.write_all(&file_size.to_le_bytes())?;
        file.write_all(b"WAVE")?;

        // fmt chunk
        file.write_all(b"fmt ")?;
        file.write_all(&16u32.to_le_bytes())?; // chunk size
        file.write_all(&1u16.to_le_bytes())?; // audio format (PCM)
        file.write_all(&num_channels.to_le_bytes())?;
        file.write_all(&sample_rate.to_le_bytes())?;
        file.write_all(&byte_rate.to_le_bytes())?;
        file.write_all(&block_align.to_le_bytes())?;
        file.write_all(&bits_per_sample.to_le_bytes())?;

        // data chunk
        file.write_all(b"data")?;
        file.write_all(&data_size.to_le_bytes())?;

        // Convert f32 samples to i16 and write
        for &sample in samples {
            let sample_i16 = (sample.clamp(-1.0, 1.0) * 32767.0) as i16;
            file.write_all(&sample_i16.to_le_bytes())?;
        }

        Ok(())
    }

    /// Send an event to listeners
    fn send_event(&self, event: StreamingEvent) {
        if let Some(tx) = &self.event_tx {
            let _ = tx.send(event);
        }
    }

    /// Get current statistics
    pub fn stats(&self) -> &StreamingStats {
        &self.stats
    }

    /// Get committed text
    pub fn committed_text(&self) -> &str {
        &self.accumulated_text
    }

    /// Reset the engine (start fresh)
    pub fn reset(&mut self) {
        self.audio_buffer.clear();
        self.vad_detector.reset();
        self.local_agreement.reset();
        self.progressive_typing.reset();
        self.accumulated_text.clear();
        self.stats = StreamingStats::default();
        self.was_speaking = false;
        self.was_probably_speaking = false;
        self.auto_enter = false;
    }

    /// Update configuration
    pub fn update_config(&mut self, config: StreamingConfig) {
        self.config = config;
    }

    /// Update typing configuration
    pub fn update_typing_config(&mut self, config: ProgressiveTypingConfig) {
        self.progressive_typing.set_config(config);
    }

    /// Update just the typing-related settings (progressive typing + auto-correction + mode)
    pub fn set_typing_enabled(
        &mut self,
        progressive: bool,
        auto_correction: bool,
        typing_mode: TypingMode,
        auto_enter: bool,
    ) {
        self.config.progressive_typing = progressive;
        self.config.auto_correction = auto_correction;
        self.auto_enter = auto_enter;
        self.typing_mode = typing_mode;
        self.progressive_typing.set_config(ProgressiveTypingConfig {
            enabled: progressive,
            auto_correction,
            typing_mode,
        });
    }

    /// Update text filters and language
    pub fn set_text_filters(&mut self, filters: TextFilters, language: Option<String>) {
        self.text_filters = filters;
        self.language = language;
    }

    /// Update the active guided grammar (bash mode). `None` disables constrained
    /// decoding and returns to the plain transcription endpoint.
    pub fn set_guided_grammar(&mut self, grammar: Option<String>) {
        self.guided_grammar = grammar;
    }

    /// Check if VAD is currently detecting speech
    pub fn is_speaking(&self) -> bool {
        self.vad_detector.is_speaking()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn drain(rx: &mut mpsc::UnboundedReceiver<StreamingEvent>) -> Vec<String> {
        let mut out = Vec::new();
        while let Ok(ev) = rx.try_recv() {
            out.push(format!("{:?}", ev));
        }
        out
    }

    /// Engine wired to a detector with short (3-frame) thresholds and an
    /// event receiver, driven by injected probabilities.
    fn seq_engine() -> (StreamingEngine, mpsc::UnboundedReceiver<StreamingEvent>) {
        let mut engine = StreamingEngine::new(
            Arc::new(WhisperClient::new("http://localhost:8178")),
            StreamingConfig::default(),
            VadConfig {
                min_speech_duration_ms: 96,
                max_silence_duration_ms: 96,
                pre_speech_buffer_ms: 64,
                ..VadConfig::default()
            },
            ProgressiveTypingConfig::default(),
            PathBuf::new(),
        )
        .unwrap();
        let (tx, rx) = mpsc::unbounded_channel();
        engine.set_event_sender(tx);
        (engine, rx)
    }

    fn feed(engine: &mut StreamingEngine, probs: &[f32]) -> Vec<SpeechSegment> {
        let mut segs = Vec::new();
        for &p in probs {
            let outcome = engine.vad_detector.inject_probability(p);
            if let Some(seg) = engine.handle_vad_outcome(outcome) {
                segs.push(seg);
            }
        }
        segs
    }

    #[test]
    fn test_rejected_candidate_emits_speech_rejected() {
        let (mut engine, mut rx) = seq_engine();
        feed(&mut engine, &[0.9]);
        assert_eq!(drain(&mut rx), vec!["SpeechProbable"]);

        // Dip before confirmation: the candidate is rejected.
        feed(&mut engine, &[0.0]);
        assert_eq!(drain(&mut rx), vec!["SpeechRejected"]);

        // Nothing further while silent.
        feed(&mut engine, &[0.0, 0.0]);
        assert!(drain(&mut rx).is_empty());
    }

    #[test]
    fn test_confirmed_speech_emits_started_and_ended_not_rejected() {
        let (mut engine, mut rx) = seq_engine();
        let segs = feed(&mut engine, &[0.9, 0.9, 0.9, 0.0, 0.0, 0.0]);
        assert_eq!(segs.len(), 1);
        assert_eq!(
            drain(&mut rx),
            vec!["SpeechProbable", "SpeechStarted", "SpeechEnded"]
        );
    }

    #[test]
    fn test_rejected_then_confirmed_sequence() {
        let (mut engine, mut rx) = seq_engine();
        feed(&mut engine, &[0.9, 0.9, 0.0, 0.0]);
        assert_eq!(drain(&mut rx), vec!["SpeechProbable", "SpeechRejected"]);

        let segs = feed(&mut engine, &[0.9, 0.9, 0.9, 0.0, 0.0, 0.0]);
        assert_eq!(segs.len(), 1);
        assert_eq!(
            drain(&mut rx),
            vec!["SpeechProbable", "SpeechStarted", "SpeechEnded"]
        );
    }

    #[test]
    fn test_typing_failure_suppresses_enter_and_discards_state() {
        use crate::progressive_typing::ProgressiveTypingError;
        let (mut engine, mut rx) = seq_engine();
        engine.accumulated_text = "hello world".to_string();
        engine.local_agreement.process("hello world".to_string());

        let delivered = engine.handle_typing_outcome(
            Err(ProgressiveTypingError::TextInputError(
                "child process timed out".to_string(),
            )),
            Instant::now(),
        );

        assert!(
            !delivered,
            "Enter must not follow a failed/timed-out typing"
        );
        assert!(
            engine.committed_text().is_empty(),
            "no replay of uncertain text"
        );
        assert!(engine.progressive_typing.typed_text().is_empty());
        assert_eq!(engine.local_agreement.committed(), "");
        let events = drain(&mut rx);
        assert_eq!(events.len(), 1);
        assert!(events[0].starts_with("Error("), "{}", events[0]);
        assert!(events[0].contains("not retried"));
    }

    #[test]
    fn test_typing_success_keeps_state_and_allows_enter() {
        let (mut engine, mut rx) = seq_engine();
        engine.accumulated_text = "hello".to_string();
        let delivered = engine.handle_typing_outcome(Ok(5), Instant::now());
        assert!(delivered);
        assert_eq!(engine.committed_text(), "hello");
        assert_eq!(engine.stats().chars_typed, 5);
        assert!(drain(&mut rx).is_empty());
    }

    #[test]
    fn test_streaming_stats_default() {
        let stats = StreamingStats::default();
        assert_eq!(stats.segments_processed, 0);
        assert_eq!(stats.avg_latency_ms, 0);
        assert_eq!(stats.chars_typed, 0);
    }
}
