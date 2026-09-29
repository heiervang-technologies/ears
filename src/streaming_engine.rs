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
use std::time::{Duration, Instant};
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
}

/// Minimum spacing between partial transcriptions of the utterance in progress.
const GHOST_PARTIAL_INTERVAL: Duration = Duration::from_millis(300);

/// A partial slower than this is dropped; the next one will be fresher.
const GHOST_PARTIAL_DEADLINE: Duration = Duration::from_secs(4);

/// Minimum audio (replay buffer included) before the first partial is worth it.
const GHOST_MIN_PARTIAL_SAMPLES: usize = 16_000 * 6 / 10;

/// Longest utterance we keep re-transcribing for partials; beyond this the
/// ghost simply waits for the final transcript.
const GHOST_MAX_PARTIAL_SAMPLES: usize = 16_000 * 30;

/// Continuous partials cost about the same at any length; the bound is the
/// server's context.
const GHOST_MAX_CONTINUOUS_SAMPLES: usize = 16_000 * 90;

/// A lost stream connection is tried again after this, at the earliest.
const STREAM_RETRY: Duration = Duration::from_secs(5);

/// A finished partial transcription.
struct Partial {
    utterance: u64,
    result: Result<String, String>,
    /// The continuous decoder, handed back for the next partial.
    decoder: Option<crate::continuous::ContinuousDecoder>,
    /// The server cannot decode continuously; stop trying.
    unsupported: bool,
    /// Pushed by the ears stream, not a request of ours in flight.
    streamed: bool,
    frozen_bytes: usize,
}

impl Partial {
    /// A repeated-preview result (no decoder to hand back).
    fn plain(utterance: u64, result: Result<String, String>) -> Self {
        Self {
            utterance,
            result,
            decoder: None,
            unsupported: false,
            streamed: false,
            frozen_bytes: 0,
        }
    }

    /// A partial the ears stream pushed for `utterance`.
    fn streamed(utterance: u64, text: String, frozen_bytes: usize) -> Self {
        Self {
            streamed: true,
            frozen_bytes,
            ..Self::plain(utterance, Ok(text))
        }
    }
}

/// Connection to the ears stream (`docs/STREAM_PROTOCOL.md`).
enum StreamLink {
    /// Not connected; connect when `retry_at` has passed (None: right away).
    Down {
        retry_at: Option<Instant>,
    },
    Connecting(
        tokio::sync::oneshot::Receiver<
            Result<crate::stream_client::StreamSession, crate::continuous::ContinuousError>,
        >,
    ),
    Up(crate::stream_client::StreamSession),
    /// The server has no stream: per-tick HTTP from now on.
    Unsupported,
}

/// The stream utterance carrying a ghost utterance.
#[derive(Debug, Clone, Copy)]
struct Streamed {
    /// Stream utterance id (increasing; never reused after a cancel).
    id: u64,
    /// Ghost utterance it belongs to.
    utterance: u64,
    /// Samples of the segment sent so far.
    sent: usize,
}

/// Ghost completion bookkeeping.
struct GhostState {
    client: crate::ghost::GhostClient,
    /// Identifies the utterance being spoken. Bumped whenever an utterance
    /// ends (segment complete or candidate rejected) so that partial results
    /// for an older utterance are discarded.
    utterance: u64,
    partial_in_flight: bool,
    last_partial_at: Option<Instant>,
    partial_seq: u64,
    partial_tx: mpsc::UnboundedSender<Partial>,
    partial_rx: mpsc::UnboundedReceiver<Partial>,
    /// Continuous decoder for the utterance in progress (None: start fresh).
    decoder: Option<crate::continuous::ContinuousDecoder>,
    stream: StreamLink,
    /// Stream utterance of the ghost utterance in progress, if streaming.
    streamed: Option<Streamed>,
    /// Last stream utterance id used.
    stream_id: u64,
    /// Ghost utterance decoded per tick to its end (the stream was not up
    /// at its start, or was lost during it). Its audio is never replayed.
    per_tick: Option<u64>,
    /// Settled state of the latest stream partial, for the per-tick decoder
    /// to carry on from if the stream is lost.
    stream_state: Option<crate::continuous::DecoderState>,
    /// Something is currently drawn as a ghost.
    showing: bool,
    /// At least one utterance was committed this session (for spacing).
    committed_any: bool,
    /// The addon was unreachable last time; avoid log spam.
    warned_unavailable: bool,
}

impl GhostState {
    fn new(client: crate::ghost::GhostClient) -> Self {
        let (partial_tx, partial_rx) = mpsc::unbounded_channel();
        Self {
            client,
            utterance: 0,
            partial_in_flight: false,
            last_partial_at: None,
            partial_seq: 0,
            partial_tx,
            partial_rx,
            decoder: None,
            stream: StreamLink::Down { retry_at: None },
            streamed: None,
            stream_id: 0,
            per_tick: None,
            stream_state: None,
            showing: false,
            committed_any: false,
            warned_unavailable: false,
        }
    }

    /// Text as it should appear in the target: utterances after the first
    /// are separated from the previous one by a space.
    fn spaced(&self, text: &str) -> String {
        if self.committed_any {
            format!(" {}", text)
        } else {
            text.to_string()
        }
    }

    fn note_result<T>(&mut self, r: &Result<T, crate::ghost::GhostError>) {
        match r {
            Ok(_) => self.warned_unavailable = false,
            Err(e) if !self.warned_unavailable => {
                warn!("Ghost addon: {}", e);
                self.warned_unavailable = true;
            }
            Err(_) => {}
        }
    }

    /// End the current utterance: stale partials are ignored from now on.
    /// A stream utterance still open for it is cancelled.
    fn next_utterance(&mut self) {
        self.stream_cancel();
        self.utterance += 1;
        self.last_partial_at = None;
        self.decoder = None;
        self.stream_state = None;
    }

    fn session(&self) -> Option<&crate::stream_client::StreamSession> {
        match &self.stream {
            StreamLink::Up(session) => Some(session),
            _ => None,
        }
    }

    /// Drop the stream utterance in progress, if any.
    fn stream_cancel(&mut self) {
        if let Some(streamed) = self.streamed.take() {
            if let Some(session) = self.session() {
                session.cancel(streamed.id);
            }
        }
    }

    /// The current utterance's segment is complete: send the rest of it and
    /// `end`. Its `final` is not used; the committed text comes from the
    /// final transcription as always.
    fn stream_end(&mut self, samples: &[f32]) {
        let Some(streamed) = self.streamed.take() else {
            return;
        };
        let Some(session) = self.session() else {
            return;
        };
        if streamed.utterance == self.utterance {
            let end = samples
                .len()
                .min(session.max_samples().unwrap_or(usize::MAX));
            if end > streamed.sent {
                let pcm: Vec<i16> = samples[streamed.sent..end]
                    .iter()
                    .map(|&s| f32_to_i16(s))
                    .collect();
                session.push(streamed.id, &pcm);
            }
            session.end(streamed.id);
        } else {
            session.cancel(streamed.id);
        }
    }

    fn clear(&mut self) {
        if self.showing {
            let r = self.client.clear();
            self.note_result(&r);
            self.showing = false;
        }
    }
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

    /// Enable or disable ghost completion. When enabled, partial transcripts
    /// are shown as inline preedit via the fcitx5 `earsghost` addon while
    /// speaking and the final transcript is committed through it instead of
    /// being typed.
    pub fn set_ghost(&mut self, enabled: bool) {
        self.set_ghost_client(
            enabled.then(|| crate::ghost::GhostClient::new(crate::ghost::default_socket_path())),
        );
    }

    /// Enable ghost completion with an explicit client (tests, custom paths).
    pub fn set_ghost_client(&mut self, client: Option<crate::ghost::GhostClient>) {
        match (client, self.ghost.is_some()) {
            (Some(mut client), false) => {
                match client.probe() {
                    Ok(shown) => info!("Ghost completion enabled ({:?})", shown),
                    Err(e) => warn!(
                        "Ghost completion enabled but the fcitx5 addon is not reachable ({}); \
                         final text will be typed instead",
                        e
                    ),
                }
                self.ghost = Some(GhostState::new(client));
                self.ghost_stream_link();
            }
            (None, true) => {
                if let Some(mut ghost) = self.ghost.take() {
                    ghost.clear();
                }
            }
            _ => {}
        }
    }

    /// Decode ghost partials continuously with this server, or `None` to
    /// re-transcribe the growing utterance.
    pub fn set_continuous(&mut self, spec: Option<crate::continuous::ContinuousSpec>) {
        self.continuous_http = spec.is_some();
        self.continuous = spec;
        if let Some(ghost) = self.ghost.as_mut() {
            ghost.decoder = None;
            ghost.stream_cancel();
            ghost.stream = StreamLink::Down { retry_at: None };
        }
        // Connect now so the first utterance need not wait for it.
        self.ghost_stream_link();
    }

    /// Advance the stream connection. Never waits on the network: connecting
    /// runs as its own task and is picked up here once it is done.
    fn ghost_stream_link(&mut self) {
        use crate::continuous::ContinuousError;
        let Some(spec) = self.continuous.as_ref() else {
            return;
        };
        let Some(ghost) = self.ghost.as_mut() else {
            return;
        };
        let next = match &mut ghost.stream {
            StreamLink::Connecting(rx) => match rx.try_recv() {
                Ok(Ok(session)) => {
                    info!(
                        "Ghost partials over the ears stream ({})",
                        session.ready().model.as_deref().unwrap_or("unknown model")
                    );
                    StreamLink::Up(session)
                }
                Ok(Err(ContinuousError::Unsupported(e))) => {
                    info!("No ears stream on the server, decoding per tick: {}", e);
                    StreamLink::Unsupported
                }
                Ok(Err(e)) => {
                    debug!("Ears stream not connected: {}", e);
                    StreamLink::Down {
                        retry_at: Some(Instant::now() + STREAM_RETRY),
                    }
                }
                Err(tokio::sync::oneshot::error::TryRecvError::Empty) => return,
                Err(tokio::sync::oneshot::error::TryRecvError::Closed) => StreamLink::Down {
                    retry_at: Some(Instant::now() + STREAM_RETRY),
                },
            },
            StreamLink::Down { retry_at } if retry_at.is_none_or(|t| Instant::now() >= t) => {
                // No runtime (sync tests): stay down.
                let Ok(runtime) = tokio::runtime::Handle::try_current() else {
                    return;
                };
                let (tx, rx) = tokio::sync::oneshot::channel();
                let url = spec.server_url.clone();
                let key = spec.api_key.clone();
                runtime.spawn(async move {
                    let session =
                        crate::stream_client::StreamSession::connect(&url, key.as_deref()).await;
                    let _ = tx.send(session);
                });
                StreamLink::Connecting(rx)
            }
            _ => return,
        };
        ghost.stream = next;
    }

    /// Turn what the stream pushed into partials for the current utterance.
    /// A lost connection hands the utterance to the per-tick decoder.
    fn ghost_stream_events(&mut self) {
        use crate::stream_client::StreamEvent;
        let Some(ghost) = self.ghost.as_mut() else {
            return;
        };
        let StreamLink::Up(session) = &mut ghost.stream else {
            return;
        };
        let mut lost = None;
        while let Some(event) = session.try_recv() {
            match event {
                StreamEvent::Partial(partial) => match ghost.streamed {
                    Some(s) if s.id == partial.utterance => {
                        ghost.stream_state = Some(partial.snapshot());
                        let text = partial.text.trim().to_string();
                        let frozen_bytes = partial.stable().trim_start().len();
                        let _ = ghost.partial_tx.send(Partial::streamed(
                            s.utterance,
                            text,
                            frozen_bytes,
                        ));
                    }
                    _ => debug!("Stale stream partial (utterance {})", partial.utterance),
                },
                // The committed text comes from the final transcription.
                StreamEvent::Final { .. } => {}
                StreamEvent::Error { code, message, .. } if code == "unsupported" => {
                    info!("Ears stream unsupported, decoding per tick: {}", message);
                    lost = Some(StreamLink::Unsupported);
                    break;
                }
                StreamEvent::Error {
                    utterance,
                    code,
                    message,
                } => debug!("Ears stream error {} ({:?}): {}", code, utterance, message),
                StreamEvent::Closed(reason) => {
                    warn!("Ears stream lost ({}); decoding per tick", reason);
                    lost = Some(StreamLink::Down {
                        retry_at: Some(Instant::now() + STREAM_RETRY),
                    });
                }
            }
        }
        if let Some(link) = lost {
            // Finish the utterance per tick from what the stream settled;
            // its audio is never sent again.
            if let Some(streamed) = ghost.streamed.take() {
                ghost.per_tick = Some(streamed.utterance);
            }
            ghost.stream = link;
        }
    }

    /// Feed the utterance in progress to the ears stream: `start` at speech
    /// onset with the replay buffer, then only new samples. Returns whether
    /// the stream owns this utterance's partials; false means decode per
    /// tick instead.
    fn ghost_stream_feed(&mut self) -> bool {
        if self.continuous.is_none() || self.guided_grammar.is_some() {
            return false;
        }
        self.ghost_stream_link();
        let muted = self.typing_mode == TypingMode::None;
        let samples = if self.vad_detector.is_speaking() {
            self.vad_detector.current_segment_samples()
        } else {
            None
        };
        let spec = self.continuous.as_ref().expect("checked above");
        let Some(ghost) = self.ghost.as_mut() else {
            return false;
        };
        if muted {
            // Nothing may be shown; do not spend the server on it either.
            ghost.stream_cancel();
            return true;
        }
        let Some(samples) = samples else {
            return false;
        };
        if ghost.per_tick == Some(ghost.utterance) {
            return false;
        }
        let session = match &ghost.stream {
            StreamLink::Up(session) => session,
            StreamLink::Connecting(_) => return true, // settled within 2 s
            StreamLink::Down { .. } | StreamLink::Unsupported => {
                ghost.per_tick = Some(ghost.utterance);
                return false;
            }
        };
        let mut streamed = match ghost.streamed {
            Some(s) if s.utterance == ghost.utterance => s,
            _ => {
                ghost.stream_id += 1;
                session.start(
                    ghost.stream_id,
                    &crate::stream_client::StartParams::from_spec(spec),
                );
                Streamed {
                    id: ghost.stream_id,
                    utterance: ghost.utterance,
                    sent: 0,
                }
            }
        };
        let end = samples
            .len()
            .min(GHOST_MAX_CONTINUOUS_SAMPLES)
            .min(session.max_samples().unwrap_or(usize::MAX));
        if end > streamed.sent {
            let pcm: Vec<i16> = samples[streamed.sent..end]
                .iter()
                .map(|&s| f32_to_i16(s))
                .collect();
            // Refused only when the connection is gone; its Closed event
            // hands the utterance over.
            session.push(streamed.id, &pcm);
            streamed.sent = end;
        }
        ghost.streamed = Some(streamed);
        true
    }

    /// Whether ghost completion is active.
    pub fn ghost_enabled(&self) -> bool {
        self.ghost.is_some()
    }

    /// Apply finished partial transcriptions for the utterance in progress.
    fn ghost_poll_partials(&mut self) {
        let muted = self.typing_mode == TypingMode::None;
        let filters = self.text_filters.clone();
        let language = self.language.clone();
        let bash = self.guided_grammar.is_some();
        let speaking = self.vad_detector.is_speaking();
        self.ghost_stream_events();
        let Some(ghost) = self.ghost.as_mut() else {
            return;
        };
        while let Ok(partial) = ghost.partial_rx.try_recv() {
            let Partial {
                utterance,
                result,
                decoder,
                unsupported,
                streamed,
                frozen_bytes,
            } = partial;
            if !streamed {
                ghost.partial_in_flight = false;
            }
            if unsupported {
                self.continuous_http = false;
            }
            // Resolve the server's model once, not per utterance.
            if let (Some(spec), Some(model)) = (
                self.continuous.as_mut(),
                decoder.as_ref().and_then(|d| d.model()),
            ) {
                if spec.model.is_none() {
                    spec.model = Some(model.to_string());
                }
            }
            if utterance != ghost.utterance || !speaking {
                continue; // stale: that utterance already ended
            }
            if !streamed {
                ghost.decoder = decoder;
            }
            if muted {
                continue; // typing switched off: show nothing in the target
            }
            let text = match result {
                Ok(text) => text,
                Err(e) => {
                    debug!("Partial transcription failed: {}", e);
                    continue;
                }
            };
            let (text, frozen_bytes) = if bash {
                (text, 0)
            } else {
                crate::freeze::filtered(&text, frozen_bytes, &filters, language.as_deref())
            };
            let shown = ghost.spaced(&text);
            let frozen_bytes = if frozen_bytes > 0 {
                frozen_bytes + shown.len() - text.len()
            } else {
                0
            };
            let r = ghost.client.preedit_frozen(&shown, frozen_bytes);
            ghost.note_result(&r);
            if r.is_ok() {
                ghost.showing = true;
            }
        }
    }

    /// Start a partial transcription of the utterance so far, if due.
    fn ghost_maybe_start_partial(&mut self) {
        if self.ghost.is_none() || self.ghost_stream_feed() {
            return;
        }
        let Some(ghost) = self.ghost.as_ref() else {
            return;
        };
        if ghost.partial_in_flight
            || !self.vad_detector.is_speaking()
            || ghost
                .last_partial_at
                .is_some_and(|t| t.elapsed() < GHOST_PARTIAL_INTERVAL)
        {
            return;
        }
        let Some(samples) = self.vad_detector.current_segment_samples() else {
            return;
        };
        let continuous =
            self.continuous.is_some() && self.continuous_http && self.guided_grammar.is_none();
        let max = if continuous {
            GHOST_MAX_CONTINUOUS_SAMPLES
        } else {
            GHOST_MAX_PARTIAL_SAMPLES
        };
        if samples.len() < GHOST_MIN_PARTIAL_SAMPLES || samples.len() > max {
            return;
        }
        let samples = samples.to_vec();

        let ghost = self.ghost.as_mut().expect("checked above");
        if continuous {
            let spec = self.continuous.as_ref().expect("checked above");
            // After a lost stream, carry on from what it settled.
            let mut decoder = ghost.decoder.take().unwrap_or_else(|| {
                let fresh = spec.decoder();
                match ghost.stream_state.take() {
                    Some(state) => fresh.resume(state),
                    None => fresh,
                }
            });
            ghost.partial_in_flight = true;
            ghost.last_partial_at = Some(Instant::now());
            let utterance = ghost.utterance;
            let tx = ghost.partial_tx.clone();
            tokio::spawn(async move {
                let pcm: Vec<i16> = samples.iter().map(|&s| f32_to_i16(s)).collect();
                let outcome = decoder.step(&pcm, false, GHOST_PARTIAL_DEADLINE).await;
                let unsupported = matches!(
                    outcome,
                    Err(crate::continuous::ContinuousError::Unsupported(_))
                );
                let _ = tx.send(Partial {
                    utterance,
                    result: outcome.map_err(|e| e.to_string()),
                    frozen_bytes: decoder.snapshot().stable.len(),
                    decoder: (!unsupported).then_some(decoder),
                    unsupported,
                    streamed: false,
                });
            });
            return;
        }
        ghost.partial_seq += 1;
        let path = self
            .temp_dir
            .join(format!("ghost_partial_{}.wav", ghost.partial_seq));
        if let Err(e) = write_wav(&path, &samples) {
            debug!("Cannot write partial WAV: {}", e);
            return;
        }
        ghost.partial_in_flight = true;
        ghost.last_partial_at = Some(Instant::now());
        let utterance = ghost.utterance;
        let tx = ghost.partial_tx.clone();
        let client = self.whisper_client.clone();
        let grammar = self.guided_grammar.clone();
        tokio::spawn(async move {
            let result = client
                .transcribe_preview(&path, grammar.as_deref(), GHOST_PARTIAL_DEADLINE)
                .await
                .map_err(|e| e.to_string());
            let _ = std::fs::remove_file(&path);
            let _ = tx.send(Partial::plain(utterance, result));
        });
    }

    /// Deliver a final transcript through the ghost addon. Falls back to
    /// ordinary typing when the addon cannot deliver it.
    fn ghost_commit(&mut self, text: &str) {
        if self.typing_mode == TypingMode::None {
            // Typing switched off (e.g. `ears typing off`): the transcript is
            // still published on IPC, but nothing reaches the focused app.
            self.ghost_clear();
            return;
        }
        let Some(ghost) = self.ghost.as_mut() else {
            return;
        };
        let spaced = ghost.spaced(text);
        let delivery = ghost.client.commit(&spaced);
        ghost.showing = false;
        match delivery {
            crate::ghost::Delivery::Delivered => {
                ghost.warned_unavailable = false;
                self.stats.chars_typed += spaced.chars().count();
                ghost.committed_any = true;
            }
            crate::ghost::Delivery::NotDelivered => {
                info!("Ghost addon did not deliver the text; typing it instead");
                let mode = self.typing_mode;
                let typing_start = Instant::now();
                let outcome = run_blocking(|| TextInput::type_text(&spaced, mode))
                    .map(|_| 0)
                    .map_err(|e| {
                        crate::progressive_typing::ProgressiveTypingError::TextInputError(
                            e.to_string(),
                        )
                    });
                if self.handle_typing_outcome(outcome, typing_start) {
                    self.stats.chars_typed += spaced.chars().count();
                    if let Some(ghost) = self.ghost.as_mut() {
                        ghost.committed_any = true;
                    }
                }
            }
            crate::ghost::Delivery::Unknown => {
                // The addon may have committed before the reply was lost.
                // Typing it again could duplicate it, so stop instead.
                self.handle_typing_outcome(
                    Err(
                        crate::progressive_typing::ProgressiveTypingError::TextInputError(
                            "ghost commit outcome unknown (addon did not answer)".to_string(),
                        ),
                    ),
                    Instant::now(),
                );
            }
        }
    }

    /// Remove the ghost (utterance produced nothing to commit).
    fn ghost_clear(&mut self) {
        if let Some(ghost) = self.ghost.as_mut() {
            ghost.clear();
        }
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
        let segment_file = self
            .temp_dir
            .join(format!("segment_{}.wav", self.stats.segments_processed));
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
        let _ = std::fs::remove_file(&segment_file);

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
        seq_engine_at("http://localhost:8178", PathBuf::new())
    }

    fn seq_engine_at(
        whisper_url: &str,
        temp_dir: PathBuf,
    ) -> (StreamingEngine, mpsc::UnboundedReceiver<StreamingEvent>) {
        let mut engine = StreamingEngine::new(
            Arc::new(WhisperClient::new(whisper_url)),
            StreamingConfig::default(),
            VadConfig {
                min_speech_duration_ms: 96,
                max_silence_duration_ms: 96,
                pre_speech_buffer_ms: 64,
                ..VadConfig::default()
            },
            ProgressiveTypingConfig::default(),
            temp_dir,
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
    fn test_typing_failure_suppresses_enter_and_suspends_typing() {
        use crate::progressive_typing::ProgressiveTypingError;
        let (mut engine, mut rx) = seq_engine();
        let dir = tempfile::tempdir().unwrap();
        let monitor = crate::health::HealthMonitor::start(dir.path()).unwrap();
        let health = monitor.health();
        engine.set_health(health.clone());
        engine.accumulated_text = "hello world".to_string();

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
        assert!(engine.typing_suspended());
        assert!(health.snapshot().typing_paused);
        assert_eq!(
            engine.committed_text(),
            "hello world",
            "transcript history is preserved"
        );
        assert!(engine.progressive_typing.typed_text().is_empty());
        let events = drain(&mut rx);
        assert_eq!(events.len(), 1);
        assert!(events[0].starts_with("Error("), "{}", events[0]);
        assert!(events[0].contains("not retried"));
        assert!(events[0].contains("restart listening"));

        // A later successful-looking outcome does not lift the suspension.
        engine.handle_typing_outcome(Ok(3), Instant::now());
        assert!(engine.typing_suspended());

        // Explicit resume starts clean.
        engine.resume_typing();
        assert!(!engine.typing_suspended());
        assert!(!health.snapshot().typing_paused);
        assert!(engine.committed_text().is_empty());
    }

    #[test]
    fn test_reset_clears_typing_suspension() {
        let (mut engine, _rx) = seq_engine();
        engine.suspend_typing();
        assert!(engine.typing_suspended());
        engine.reset();
        assert!(!engine.typing_suspended());
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

    /// Fake `earsghost` addon: records every line, answers "OK preedit".
    fn fake_ghost_addon(dir: &std::path::Path) -> (PathBuf, std::sync::mpsc::Receiver<String>) {
        use std::io::{BufRead, Write};
        let path = dir.join("ghost.sock");
        let listener = std::os::unix::net::UnixListener::bind(&path).unwrap();
        let (tx, rx) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            for stream in listener.incoming() {
                let Ok(stream) = stream else { return };
                let mut w = stream.try_clone().unwrap();
                for line in std::io::BufReader::new(stream).lines() {
                    let Ok(line) = line else { break };
                    let reply: &[u8] = if line == "S" {
                        b"OK preedit TestApp\n"
                    } else {
                        b"OK preedit\n"
                    };
                    let _ = tx.send(line);
                    let _ = w.write_all(reply);
                }
            }
        });
        (path, rx)
    }

    fn ghost_engine(dir: &std::path::Path) -> (StreamingEngine, std::sync::mpsc::Receiver<String>) {
        ghost_engine_at(dir, "http://localhost:8178")
    }

    /// Ghost engine whose final transcriptions go to `whisper_url`.
    fn ghost_engine_at(
        dir: &std::path::Path,
        whisper_url: &str,
    ) -> (StreamingEngine, std::sync::mpsc::Receiver<String>) {
        let (mut engine, _rx) = seq_engine_at(whisper_url, dir.to_path_buf());
        engine.typing_mode = TypingMode::Wtype;
        let (path, lines) = fake_ghost_addon(dir);
        engine.set_ghost_client(Some(crate::ghost::GhostClient::with_focus_probe(
            path,
            || Some("TestApp".to_string()),
        )));
        assert_eq!(lines.recv().unwrap(), "S", "probe on enable");
        (engine, lines)
    }

    /// Next command the engine sent, skipping focus status queries.
    fn next_cmd(lines: &std::sync::mpsc::Receiver<String>) -> String {
        loop {
            let line = lines.recv().unwrap();
            if line != "S" {
                return line;
            }
        }
    }

    #[test]
    fn test_ghost_commits_are_spaced_and_clear_nothing_extra() {
        let dir = tempfile::tempdir().unwrap();
        let (mut engine, lines) = ghost_engine(dir.path());
        engine.ghost_commit("hello world");
        engine.ghost_commit("second one");
        assert_eq!(next_cmd(&lines), "C hello world");
        assert_eq!(next_cmd(&lines), "C  second one");
        assert_eq!(engine.stats().chars_typed, 11 + 11);
    }

    #[test]
    fn test_ghost_rejected_candidate_clears_visible_ghost() {
        let dir = tempfile::tempdir().unwrap();
        let (mut engine, lines) = ghost_engine(dir.path());
        // Simulate a visible partial, then a rejected candidate.
        engine.ghost.as_mut().unwrap().showing = true;
        feed(&mut engine, &[0.9]);
        feed(&mut engine, &[0.0]);
        assert_eq!(next_cmd(&lines), "X");
        assert!(!engine.ghost.as_ref().unwrap().showing);
    }

    #[test]
    fn test_ghost_stale_partials_are_ignored() {
        let dir = tempfile::tempdir().unwrap();
        let (mut engine, lines) = ghost_engine(dir.path());
        feed(&mut engine, &[0.9, 0.9, 0.9]); // confirmed speech
        let ghost = engine.ghost.as_mut().unwrap();
        let current = ghost.utterance;
        ghost
            .partial_tx
            .send(Partial::plain(current + 7, Ok("old".into())))
            .unwrap();
        ghost
            .partial_tx
            .send(Partial::plain(current, Ok("fresh".into())))
            .unwrap();
        engine.ghost_poll_partials();
        assert_eq!(next_cmd(&lines), "F 0 fresh");
        assert!(
            lines.try_iter().all(|l| l == "S"),
            "stale partial must not be shown"
        );
    }

    fn spec() -> crate::continuous::ContinuousSpec {
        crate::continuous::ContinuousSpec {
            server_url: "http://127.0.0.1:9".into(),
            api_key: None,
            model: Some("Qwen/Qwen3-ASR-1.7B".into()),
            language: Some("en".into()),
            context: None,
        }
    }

    #[test]
    fn test_ghost_continuous_decoder_follows_its_utterance() {
        let dir = tempfile::tempdir().unwrap();
        let (mut engine, lines) = ghost_engine(dir.path());
        engine.set_continuous(Some(spec()));
        feed(&mut engine, &[0.9, 0.9, 0.9]); // confirmed speech
        let ghost = engine.ghost.as_mut().unwrap();
        let current = ghost.utterance;
        let mut stale = Partial::plain(current + 1, Ok("old".into()));
        stale.decoder = Some(spec().decoder());
        ghost.partial_tx.send(stale).unwrap();
        engine.ghost_poll_partials();
        assert!(
            engine.ghost.as_ref().unwrap().decoder.is_none(),
            "a stale utterance's decoder is dropped"
        );
        let ghost = engine.ghost.as_mut().unwrap();
        let mut fresh = Partial::plain(current, Ok("fresh".into()));
        fresh.decoder = Some(spec().decoder());
        ghost.partial_tx.send(fresh).unwrap();
        engine.ghost_poll_partials();
        assert_eq!(next_cmd(&lines), "F 0 fresh");
        assert!(engine.ghost.as_ref().unwrap().decoder.is_some());
        // The next utterance starts from a fresh decoder.
        engine.ghost.as_mut().unwrap().next_utterance();
        assert!(engine.ghost.as_ref().unwrap().decoder.is_none());
    }

    #[test]
    fn test_ghost_unsupported_server_falls_back_to_repeat() {
        let dir = tempfile::tempdir().unwrap();
        let (mut engine, _lines) = ghost_engine(dir.path());
        engine.set_continuous(Some(spec()));
        let ghost = engine.ghost.as_mut().unwrap();
        let mut refused = Partial::plain(ghost.utterance, Err("unsupported".into()));
        refused.unsupported = true;
        ghost.partial_tx.send(refused).unwrap();
        engine.ghost_poll_partials();
        assert!(!engine.continuous_http);
    }

    #[test]
    fn test_ghost_muting_clears_visible_ghost() {
        let dir = tempfile::tempdir().unwrap();
        let (mut engine, lines) = ghost_engine(dir.path());
        engine.ghost.as_mut().unwrap().showing = true;
        engine.set_typing_enabled(false, false, TypingMode::None, false);
        assert_eq!(next_cmd(&lines), "X");
    }

    #[test]
    fn test_ghost_reset_drops_old_partials() {
        let dir = tempfile::tempdir().unwrap();
        let (mut engine, lines) = ghost_engine(dir.path());
        engine.ghost.as_mut().unwrap().showing = true;
        let old = engine.ghost.as_ref().unwrap().utterance;
        engine.reset();
        assert_eq!(next_cmd(&lines), "X");
        feed(&mut engine, &[0.9, 0.9, 0.9]);
        engine
            .ghost
            .as_ref()
            .unwrap()
            .partial_tx
            .send(Partial::plain(old, Ok("stale".into())))
            .unwrap();
        engine.ghost_poll_partials();
        std::thread::sleep(std::time::Duration::from_millis(50));
        assert!(
            lines.try_iter().all(|l| l == "S"),
            "pre-reset partial shown"
        );
    }

    #[test]
    fn test_ghost_respects_typing_switch_off() {
        let dir = tempfile::tempdir().unwrap();
        let (mut engine, lines) = ghost_engine(dir.path());
        engine.typing_mode = TypingMode::None;
        feed(&mut engine, &[0.9, 0.9, 0.9]);
        let current = engine.ghost.as_ref().unwrap().utterance;
        engine
            .ghost
            .as_ref()
            .unwrap()
            .partial_tx
            .send(Partial::plain(current, Ok("secret".into())))
            .unwrap();
        engine.ghost_poll_partials();
        engine.ghost_commit("secret");
        std::thread::sleep(std::time::Duration::from_millis(50));
        assert!(lines.try_recv().is_err(), "nothing may reach the app");
    }

    fn spec_at(url: &str) -> crate::continuous::ContinuousSpec {
        crate::continuous::ContinuousSpec {
            server_url: url.into(),
            ..spec()
        }
    }

    /// What `process_audio` does for the ghost between segments, then a
    /// moment for the socket tasks to run.
    async fn ghost_tick(engine: &mut StreamingEngine) {
        engine.ghost_poll_partials();
        engine.ghost_maybe_start_partial();
        tokio::time::sleep(Duration::from_millis(5)).await;
    }

    async fn tick_until(engine: &mut StreamingEngine, done: impl Fn(&StreamingEngine) -> bool) {
        for _ in 0..400 {
            if done(engine) {
                return;
            }
            ghost_tick(engine).await;
        }
        panic!("condition never held");
    }

    fn stream_up(engine: &StreamingEngine) -> bool {
        matches!(engine.ghost.as_ref().unwrap().stream, StreamLink::Up(_))
    }

    /// Tick until the addon gets a command other than a status query.
    async fn next_cmd_ticking(
        engine: &mut StreamingEngine,
        lines: &std::sync::mpsc::Receiver<String>,
    ) -> String {
        for _ in 0..400 {
            ghost_tick(engine).await;
            if let Some(line) = lines.try_iter().find(|l| l != "S") {
                return line;
            }
        }
        panic!("the ghost addon never got a command");
    }

    fn stream_partial(id: u64, text: &str, stable_chars: usize) -> serde_json::Value {
        serde_json::json!({"type": "partial", "utterance": id, "seq": 1, "text": text,
            "stable_chars": stable_chars, "language": "English", "audio_ms": 1000,
            "decode_ms": 50})
    }

    fn transcription(text: &str) -> wiremock::ResponseTemplate {
        wiremock::ResponseTemplate::new(200).set_body_json(serde_json::json!({"text": text}))
    }

    fn chat_reply(content: &str) -> wiremock::ResponseTemplate {
        wiremock::ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "choices": [{"message": {"content": content}, "finish_reason": "stop"}]
        }))
    }

    #[tokio::test]
    async fn test_ghost_stream_utterance_follows_the_segment() {
        use crate::stream_client::fake::{FakeServer, Mode};
        use wiremock::{matchers::path, Mock, MockServer};
        let whisper = MockServer::start().await;
        Mock::given(path("/v1/audio/transcriptions"))
            .respond_with(transcription("hello stream."))
            .mount(&whisper)
            .await;
        let server = FakeServer::start(Mode::Normal).await;
        let dir = tempfile::tempdir().unwrap();
        let (mut engine, lines) = ghost_engine_at(dir.path(), &whisper.uri());
        engine.set_continuous(Some(spec_at(&server.url)));
        tick_until(&mut engine, stream_up).await;

        feed(&mut engine, &[0.9, 0.9, 0.9]);
        ghost_tick(&mut engine).await;
        let start = server.wait_for(|l| l.starts_with("start ")).await;
        assert!(start.contains(r#""language":"en""#), "{start}");
        let id = server.starts()[0];
        server.say(stream_partial(id, "hello stream", 5));
        assert_eq!(
            next_cmd_ticking(&mut engine, &lines).await,
            "F 5 hello stream"
        );
        assert_eq!(
            engine
                .ghost
                .as_ref()
                .unwrap()
                .stream_state
                .as_ref()
                .unwrap()
                .stable,
            "hello"
        );

        let segs = feed(&mut engine, &[0.9, 0.0, 0.0, 0.0]);
        assert_eq!(segs.len(), 1);
        let total = segs[0].samples.len();
        engine
            .process_segment(segs.into_iter().next().unwrap())
            .await
            .unwrap();
        server.wait_for(|l| l == format!("end {id}")).await;
        // Every sample went out exactly once, before `end`.
        assert_eq!(server.audio_bytes(), 2 * total);
        assert_eq!(
            next_cmd_ticking(&mut engine, &lines).await,
            "C hello stream."
        );
        assert_eq!(server.starts(), vec![id]);
    }

    #[tokio::test]
    async fn test_ghost_stream_drops_stale_partials_and_cancels_on_reset() {
        use crate::stream_client::fake::{FakeServer, Mode};
        let server = FakeServer::start(Mode::Normal).await;
        let dir = tempfile::tempdir().unwrap();
        let (mut engine, lines) = ghost_engine(dir.path());
        engine.set_continuous(Some(spec_at(&server.url)));
        tick_until(&mut engine, stream_up).await;

        feed(&mut engine, &[0.9, 0.9, 0.9]);
        ghost_tick(&mut engine).await;
        server.wait_for(|l| l.starts_with("start ")).await;
        let old = server.starts()[0];
        engine.reset();
        server.wait_for(|l| l == format!("cancel {old}")).await;

        feed(&mut engine, &[0.9, 0.9, 0.9]);
        ghost_tick(&mut engine).await;
        server
            .wait_for(|l| l.starts_with("start ") && !l.contains(&format!(":{old},")))
            .await;
        let fresh = server.starts()[1];
        assert!(fresh > old, "stream utterance ids only increase");
        server.say(stream_partial(old, "old words", 0));
        server.say(stream_partial(fresh, "fresh words", 0));
        assert_eq!(
            next_cmd_ticking(&mut engine, &lines).await,
            "F 0 fresh words"
        );

        // Muting cancels the stream utterance too.
        engine.set_typing_enabled(false, false, TypingMode::None, false);
        server.wait_for(|l| l == format!("cancel {fresh}")).await;
        ghost_tick(&mut engine).await;
        assert_eq!(server.starts().len(), 2, "no stream while muted");
    }

    #[tokio::test]
    async fn test_ghost_without_stream_endpoint_decodes_per_tick() {
        use wiremock::{matchers::path, Mock, MockServer};
        // wiremock answers the upgrade with 404: the plugin is not installed.
        let server = MockServer::start().await;
        Mock::given(path("/v1/chat/completions"))
            .respond_with(chat_reply("over http"))
            .mount(&server)
            .await;
        let dir = tempfile::tempdir().unwrap();
        let (mut engine, lines) = ghost_engine(dir.path());
        engine.set_continuous(Some(spec_at(&server.uri())));
        tick_until(&mut engine, |e| {
            matches!(e.ghost.as_ref().unwrap().stream, StreamLink::Unsupported)
        })
        .await;
        feed(&mut engine, &[0.9; 24]);
        assert_eq!(next_cmd_ticking(&mut engine, &lines).await, "F 0 over http");
    }

    #[tokio::test]
    async fn test_ghost_lost_stream_falls_back_without_duplicate_commit() {
        use crate::stream_client::fake::{FakeServer, Mode};
        use wiremock::{matchers::path, Mock, MockServer};
        let http = MockServer::start().await;
        Mock::given(path("/v1/chat/completions"))
            .respond_with(chat_reply(" then http"))
            .mount(&http)
            .await;
        Mock::given(path("/v1/audio/transcriptions"))
            .respond_with(transcription("streamed words then http."))
            .mount(&http)
            .await;
        // One URL for both: the stream here, plain HTTP passed to wiremock.
        let server = FakeServer::start_with_http(Mode::Normal, Some(*http.address())).await;
        let dir = tempfile::tempdir().unwrap();
        let (mut engine, lines) = ghost_engine_at(dir.path(), &server.url);
        engine.set_continuous(Some(spec_at(&server.url)));
        tick_until(&mut engine, stream_up).await;

        feed(&mut engine, &[0.9; 24]);
        ghost_tick(&mut engine).await;
        server.wait_for(|l| l.starts_with("start ")).await;
        let id = server.starts()[0];
        server.say(stream_partial(id, "streamed words so far", 14));
        assert_eq!(
            next_cmd_ticking(&mut engine, &lines).await,
            "F 14 streamed words so far"
        );

        server.drop_connections();
        // The per-tick decoder carries on from the settled text.
        assert_eq!(
            next_cmd_ticking(&mut engine, &lines).await,
            "F 14 streamed words then http"
        );
        let requests = http.received_requests().await.unwrap();
        let chat: serde_json::Value = serde_json::from_slice(
            &requests
                .iter()
                .find(|r| r.url.path() == "/v1/chat/completions")
                .unwrap()
                .body,
        )
        .unwrap();
        let messages = chat["messages"].as_array().unwrap();
        assert_eq!(
            messages.last().unwrap()["content"],
            "language English<asr_text>streamed words"
        );

        let segs = feed(&mut engine, &[0.0, 0.0, 0.0]);
        engine
            .process_segment(segs.into_iter().next().unwrap())
            .await
            .unwrap();
        let commits: Vec<String> = lines.try_iter().filter(|l| l.starts_with("C ")).collect();
        assert_eq!(commits, vec!["C streamed words then http."]);
        tokio::time::sleep(Duration::from_millis(50)).await;
        let finals = http
            .received_requests()
            .await
            .unwrap()
            .iter()
            .filter(|r| r.url.path() == "/v1/audio/transcriptions")
            .count();
        assert_eq!(finals, 1);
        // The utterance was never replayed into a new stream.
        assert_eq!(server.starts(), vec![id]);
    }

    #[test]
    fn test_guided_segments_are_discrete_without_progressive_typing() {
        let (mut engine, _rx) = seq_engine();
        assert!(!engine.config.progressive_typing);
        engine.set_guided_grammar(Some("root ::= command".to_string()));

        assert_eq!(engine.commit_transcript("git status"), "git status");
        assert_eq!(engine.committed_text(), "git status");

        assert_eq!(engine.commit_transcript("cargo test"), "cargo test");
        assert_eq!(engine.committed_text(), "cargo test");
    }

    #[test]
    fn test_dictation_segments_still_accumulate() {
        let (mut engine, _rx) = seq_engine();
        engine.commit_transcript("hello");
        engine.commit_transcript("world");
        assert_eq!(engine.committed_text(), "hello world");
    }

    #[test]
    fn test_switching_guided_mode_clears_typing_state() {
        let (mut engine, _rx) = seq_engine();
        engine.accumulated_text = "previous dictation".to_string();

        engine.set_guided_grammar(Some("root ::= command".to_string()));
        assert!(engine.committed_text().is_empty());

        // Replacing one grammar with another keeps command state.
        engine.commit_transcript("pwd");
        engine.set_guided_grammar(Some("root ::= other".to_string()));
        assert_eq!(engine.committed_text(), "pwd");

        engine.set_guided_grammar(None);
        assert!(engine.committed_text().is_empty());
    }

    #[test]
    fn test_streaming_stats_default() {
        let stats = StreamingStats::default();
        assert_eq!(stats.segments_processed, 0);
        assert_eq!(stats.avg_latency_ms, 0);
        assert_eq!(stats.chars_typed, 0);
    }
}
