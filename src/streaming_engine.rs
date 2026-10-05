//! Streaming transcription engine
//!
//! This module coordinates all components for real-time streaming transcription:
//! - Audio buffering
//! - VAD (Voice Activity Detection)
//! - Whisper transcription
//! - LocalAgreement policy
//! - Progressive typing

mod ghost;
use ghost::GhostState;

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

    /// Transcription segment completed. `audio_path` is the utterance as a
    /// 16 kHz mono WAV, present only when the engine keeps audio
    /// ([`StreamingEngine::set_keep_audio_dir`]); the consumer owns the file.
    SegmentCompleted {
        text: String,
        duration_ms: u64,
        #[serde(skip_serializing_if = "Option::is_none")]
        audio_path: Option<String>,
    },

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

    health: Option<crate::health::PipelineHealth>,

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

    /// Set after a typing failure. While suspended, transcription and
    /// transcript events continue but nothing is injected and Enter is never
    /// sent, because the target may hold a partial command. Cleared only by
    /// an explicit [`StreamingEngine::resume_typing`] or a fresh engine
    /// (i.e. restarting listening).
    typing_suspended: bool,

    /// Ghost completion state; `Some` when output goes to the fcitx5 ghost
    /// addon as inline preedit instead of being typed.
    ghost: Option<GhostState>,
    /// Decode ghost partials continuously (issue #144) instead of
    /// re-transcribing the growing utterance: over the ears stream when the
    /// server has it, else per tick over HTTP.
    continuous: Option<crate::continuous::ContinuousSpec>,
    /// The per-tick HTTP decoder still works (false: repeat instead).
    continuous_http: bool,
    /// Keep each transcribed utterance as a WAV here and report it on
    /// `SegmentCompleted` (ws-listen `--keep-audio`).
    keep_audio_dir: Option<PathBuf>,
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
            health: None,
            was_probably_speaking: false,
            auto_enter: false,
            typing_mode: TypingMode::Auto,
            text_filters: TextFilters::default(),
            language: None,
            guided_grammar: None,
            typing_suspended: false,
            ghost: None,
            continuous: None,
            continuous_http: false,
            keep_audio_dir: None,
        })
    }

    pub fn set_health(&mut self, health: crate::health::PipelineHealth) {
        health.set_typing_paused(self.typing_suspended);
        self.vad_detector.set_health(health.clone());
        self.health = Some(health);
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
        let _stage = self
            .health
            .as_ref()
            .map(|h| h.enter(crate::health::Stage::Detecting));
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

        let segment = self.handle_vad_outcome(outcome);

        if self.ghost.is_some() {
            self.ghost_poll_partials();
            if segment.is_none() {
                self.ghost_maybe_start_partial();
            }
        }

        if let Some(segment) = segment {
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
            if let Some(ghost) = self.ghost.as_mut() {
                ghost.next_utterance();
                ghost.clear();
            }
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
        if let Some(ghost) = self.ghost.as_mut() {
            ghost.stream_end(&segment.samples);
            // Partials still in flight belong to this finished utterance.
            ghost.next_utterance();
        }
        if segment.samples.is_empty() {
            debug!("Skipping empty speech segment");
            self.ghost_clear();
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
        // A unique owned path survives the request and is removed on success,
        // failure, or cancellation. A deterministic name races other engines.
        let segment_file = tempfile::Builder::new()
            .prefix("segment_")
            .suffix(".wav")
            .tempfile_in(&self.temp_dir)
            .map_err(|e| StreamingEngineError::AudioError(e.to_string()))?
            .into_temp_path();
        let saving = self
            .health
            .as_ref()
            .map(|h| h.enter(crate::health::Stage::Saving));
        self.save_wav(&segment_file, &segment.samples)
            .map_err(|e| StreamingEngineError::AudioError(e.to_string()))?;
        debug!("WAV save took {:?}", wav_start.elapsed());
        drop(saving);

        // Transcribe with Whisper. In bash mode a guided grammar routes the
        // request to the constrained chat-completions path.
        let transcribe_start = Instant::now();
        let transcribing = self
            .health
            .as_ref()
            .map(|h| h.enter(crate::health::Stage::Transcribing));
        let transcript = match self
            .whisper_client
            .transcribe_with_grammar(&segment_file, self.guided_grammar.as_deref())
            .await
        {
            Ok(text) => text,
            Err(e) => {
                warn!("Transcription error: {}", e);
                self.send_event(StreamingEvent::Error(format!("Transcription error: {}", e)));
                self.ghost_clear();
                return Err(StreamingEngineError::TranscriptionError(e.to_string()));
            }
        };

        drop(transcribing);
        info!("Transcription took {:?}", transcribe_start.elapsed());

        // Clean up temp file
        drop(segment_file);

        if transcript.is_empty() {
            debug!("Empty transcript, skipping");
            self.ghost_clear();
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
            self.ghost_clear();
            return Ok(());
        }

        info!("Transcribed: {}", transcript);

        let guided_command = self.guided_grammar.is_some();
        let newly_committed = self.commit_transcript(&transcript);

        let typing = self
            .health
            .as_ref()
            .map(|h| h.enter(crate::health::Stage::Typing));

        // Update progressive typing with the full accumulated text
        if self.ghost.is_some() {
            if self.typing_suspended {
                debug!("Typing suspended after earlier failure; ghost cleared, nothing committed");
                self.ghost_clear();
            } else {
                self.ghost_commit(&transcript);
            }
        } else if self.typing_suspended {
            debug!("Typing suspended after earlier failure; transcript kept, nothing injected");
        } else if guided_command {
            // Guided output is one complete command per VAD segment. Type it
            // directly even when progressive typing is disabled, while keeping
            // TypingMode::None as the explicit no-input mode (ws-listen,
            // `ears typing off`).
            if self.typing_mode != TypingMode::None && !newly_committed.is_empty() {
                let typing_start = Instant::now();
                let mode = self.typing_mode;
                let chars = newly_committed.chars().count();
                let outcome = run_blocking(|| TextInput::type_text(&newly_committed, mode))
                    .map(|()| chars)
                    .map_err(|e| {
                        crate::progressive_typing::ProgressiveTypingError::TextInputError(
                            e.to_string(),
                        )
                    });
                let delivered = self.handle_typing_outcome(outcome, typing_start);
                self.send_enter_if_delivered(delivered, typing_start);
            }
        } else if self.config.progressive_typing && !newly_committed.is_empty() {
            let typing_start = Instant::now();
            let progressive_typing = &mut self.progressive_typing;
            let accumulated = &self.accumulated_text;
            let outcome = run_blocking(|| progressive_typing.update(accumulated));
            let delivered = self.handle_typing_outcome(outcome, typing_start);
            self.send_enter_if_delivered(delivered, typing_start);
        }

        drop(typing);

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

        let audio_path = self
            .keep_audio_dir
            .as_deref()
            .and_then(|dir| keep_segment_audio(dir, &segment.samples));

        self.send_event(StreamingEvent::SegmentCompleted {
            text: transcript,
            duration_ms: segment.end_ms - segment.start_ms,
            audio_path,
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

    /// Commit a completed VAD transcript according to the active mode.
    ///
    /// Dictation mode preserves the historical space-separated accumulation
    /// used by progressive typing. Guided (bash) mode treats each segment as a
    /// fresh command and never carries text or typing agreement across
    /// segments.
    fn commit_transcript(&mut self, transcript: &str) -> String {
        self.local_agreement.reset();
        if self.guided_grammar.is_some() {
            self.progressive_typing.reset();
            self.accumulated_text.clear();
            self.accumulated_text.push_str(transcript);
            return transcript.to_string();
        }

        // Each VAD segment is a discrete utterance. Reset agreement state so
        // the previous segment's text doesn't interfere, then feed the
        // transcript twice to force LocalAgreement to commit it immediately.
        self.local_agreement.process(transcript.to_string());
        let (newly_committed, _uncommitted) = self.local_agreement.process(transcript.to_string());

        // Accumulate committed text across segments (space-separated)
        if !newly_committed.is_empty() {
            if !self.accumulated_text.is_empty() {
                self.accumulated_text.push(' ');
            }
            self.accumulated_text.push_str(&newly_committed);
        }

        newly_committed
    }

    /// Send Enter after typing when auto-Enter is on.
    ///
    /// Only after typing that we know completed: after a failure (including
    /// a timeout) the screen may hold a partial command, and submitting it
    /// would execute something nobody said.
    fn send_enter_if_delivered(&mut self, delivered: bool, typing_start: Instant) {
        if !(self.auto_enter && delivered) {
            return;
        }
        if let Err(e) = run_blocking(TextInput::send_enter) {
            // A failed Enter leaves target state uncertain too: the next
            // utterance must not append to and submit this one.
            self.handle_typing_outcome(
                Err(
                    crate::progressive_typing::ProgressiveTypingError::TextInputError(format!(
                        "Enter key failed: {e}"
                    )),
                ),
                typing_start,
            );
        }
    }

    /// Account for a progressive-typing attempt. Returns whether the text is
    /// known to have been delivered.
    ///
    /// On failure typing is *suspended*, not retried: a timed-out child may
    /// have delivered part of the text, so neither the engine nor the
    /// progressive typer can know what is on screen. Typing the next segment
    /// would append to that partial command, and auto-Enter would execute it.
    /// Transcription keeps running so the transcript history and clipboard
    /// stay useful; the user is told to check the target and restart
    /// listening to resume injection.
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
                    "Progressive typing error after {:?}: {}; typing suspended until listening is restarted",
                    typing_start.elapsed(),
                    e
                );
                self.send_event(StreamingEvent::Error(format!(
                    "Typing error: {}. Output may be partial and was not retried. \
                     Typing is paused: check the target window, then restart listening to resume.",
                    e
                )));
                self.suspend_typing();
                false
            }
        }
    }

    /// Stop injecting text and Enter until [`StreamingEngine::resume_typing`]
    /// or a fresh engine. The progressive typer's notion of what is on screen
    /// is discarded because it is no longer trustworthy.
    fn suspend_typing(&mut self) {
        self.typing_suspended = true;
        if let Some(ref health) = self.health {
            health.set_typing_paused(true);
        }
        self.progressive_typing.reset();
    }

    /// Whether injection is currently paused after a typing failure.
    pub fn typing_suspended(&self) -> bool {
        self.typing_suspended
    }

    /// Explicitly resume injection after the user has checked the target.
    /// Starts the progressive typer from a clean slate so nothing already
    /// transcribed is replayed.
    pub fn resume_typing(&mut self) {
        self.typing_suspended = false;
        if let Some(ref health) = self.health {
            health.set_typing_paused(false);
        }
        self.progressive_typing.reset();
        self.local_agreement.reset();
        self.accumulated_text.clear();
    }

    /// Save audio samples to WAV file
    fn save_wav(&self, path: &std::path::Path, samples: &[f32]) -> Result<(), std::io::Error> {
        write_wav(path, samples)
    }
}

/// Most kept utterance clips retained in a keep-audio directory.
pub const KEEP_AUDIO_MAX: usize = 32;

/// Write a kept utterance clip into `dir` and prune the oldest beyond
/// [`KEEP_AUDIO_MAX`]. Failure only costs the audio: the transcript is still
/// delivered, without `audio_path`.
fn keep_segment_audio(dir: &std::path::Path, samples: &[f32]) -> Option<String> {
    let kept = std::fs::create_dir_all(dir).and_then(|()| {
        let file = tempfile::Builder::new()
            .prefix(&format!(
                "utterance_{}_",
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map(|d| d.as_millis())
                    .unwrap_or(0)
            ))
            .suffix(".wav")
            .tempfile_in(dir)?;
        write_wav(file.path(), samples)?;
        file.keep().map(|(_, path)| path).map_err(|e| e.error)
    });
    match kept {
        Ok(path) => {
            prune_kept_audio(dir, KEEP_AUDIO_MAX);
            Some(path.to_string_lossy().into_owned())
        }
        Err(e) => {
            warn!("Failed to keep utterance audio in {}: {}", dir.display(), e);
            None
        }
    }
}

fn prune_kept_audio(dir: &std::path::Path, keep: usize) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    let mut clips: Vec<(std::time::SystemTime, PathBuf)> = entries
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .filter(|p| {
            p.extension().is_some_and(|x| x == "wav")
                && p.file_name()
                    .and_then(|n| n.to_str())
                    .is_some_and(|n| n.starts_with("utterance_"))
        })
        .filter_map(|p| Some((std::fs::metadata(&p).ok()?.modified().ok()?, p)))
        .collect();
    if clips.len() <= keep {
        return;
    }
    clips.sort();
    for (_, old) in &clips[..clips.len() - keep] {
        let _ = std::fs::remove_file(old);
    }
}

fn f32_to_i16(sample: f32) -> i16 {
    (sample.clamp(-1.0, 1.0) * 32767.0) as i16
}

/// Write mono 16 kHz 16-bit PCM WAV.
fn write_wav(path: &std::path::Path, samples: &[f32]) -> Result<(), std::io::Error> {
    {
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

        for &sample in samples {
            file.write_all(&f32_to_i16(sample).to_le_bytes())?;
        }

        Ok(())
    }
}

impl StreamingEngine {
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
        self.typing_suspended = false;
        if let Some(ref health) = self.health {
            health.set_typing_paused(false);
        }
        // Partials still in flight belong to the old session.
        if let Some(ghost) = self.ghost.as_mut() {
            ghost.clear();
            ghost.next_utterance();
            ghost.committed_any = false;
        }
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
        if typing_mode == TypingMode::None {
            // Muted: take down a ghost that is already on screen.
            self.ghost_clear();
            if let Some(ghost) = self.ghost.as_mut() {
                ghost.stream_cancel();
            }
        }
    }

    /// Update text filters and language
    pub fn set_text_filters(&mut self, filters: TextFilters, language: Option<String>) {
        self.text_filters = filters;
        self.language = language;
    }

    /// Keep every transcribed utterance as a WAV in `dir` and report its path
    /// on `SegmentCompleted`, for consumers that want the audio itself (e.g. a
    /// model that hears speech). At most [`KEEP_AUDIO_MAX`] clips are retained,
    /// so an absent consumer cannot fill the disk.
    pub fn set_keep_audio_dir(&mut self, dir: Option<PathBuf>) {
        self.keep_audio_dir = dir;
    }

    /// Update the active guided grammar (bash mode). `None` disables constrained
    /// decoding and returns to the plain transcription endpoint.
    pub fn set_guided_grammar(&mut self, grammar: Option<String>) {
        if self.guided_grammar.is_some() != grammar.is_some() {
            // Dictation and command text must never bleed into each other.
            self.local_agreement.reset();
            self.progressive_typing.reset();
            self.accumulated_text.clear();
        }
        self.guided_grammar = grammar;
    }

    /// Check if VAD is currently detecting speech
    pub fn is_speaking(&self) -> bool {
        self.vad_detector.is_speaking()
    }
}

#[cfg(test)]
mod tests;
