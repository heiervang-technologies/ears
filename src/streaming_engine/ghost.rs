//! Ghost session state, stream fallback, partials, and application delivery.

use super::{f32_to_i16, run_blocking, write_wav, StreamingEngine};
use crate::desktop::{TextInput, TypingMode};
use std::time::{Duration, Instant};
use tokio::sync::mpsc;
use tracing::{debug, info, warn};

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

/// How long to wait for the stream's `final` before transcribing the
/// segment in full instead.
const STREAM_FINAL_TIMEOUT: Duration = Duration::from_millis(1500);

/// A finished partial transcription.
pub(super) struct Partial {
    pub(super) utterance: u64,
    pub(super) result: Result<String, String>,
    /// The continuous decoder, handed back for the next partial.
    pub(super) decoder: Option<crate::continuous::ContinuousDecoder>,
    /// The server cannot decode continuously; stop trying.
    pub(super) unsupported: bool,
    /// Pushed by the ears stream, not a request of ours in flight.
    pub(super) streamed: bool,
    pub(super) frozen_bytes: usize,
}

impl Partial {
    /// A repeated-preview result (no decoder to hand back).
    pub(super) fn plain(utterance: u64, result: Result<String, String>) -> Self {
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
    pub(super) fn streamed(utterance: u64, text: String, frozen_bytes: usize) -> Self {
        Self {
            streamed: true,
            frozen_bytes,
            ..Self::plain(utterance, Ok(text))
        }
    }
}

/// Connection to the ears stream (`docs/STREAM_PROTOCOL.md`).
pub(super) enum StreamLink {
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
pub(super) struct Streamed {
    /// Stream utterance id (increasing; never reused after a cancel).
    pub(super) id: u64,
    /// Ghost utterance it belongs to.
    pub(super) utterance: u64,
    /// Samples of the segment sent so far.
    pub(super) sent: usize,
}

/// Ghost completion bookkeeping.
pub(super) struct GhostState {
    pub(super) client: crate::ghost::GhostClient,
    /// Identifies the utterance being spoken. Bumped whenever an utterance
    /// ends (segment complete or candidate rejected) so that partial results
    /// for an older utterance are discarded.
    pub(super) utterance: u64,
    pub(super) partial_in_flight: bool,
    pub(super) last_partial_at: Option<Instant>,
    pub(super) partial_seq: u64,
    pub(super) partial_tx: mpsc::UnboundedSender<Partial>,
    pub(super) partial_rx: mpsc::UnboundedReceiver<Partial>,
    /// Continuous decoder for the utterance in progress (None: start fresh).
    pub(super) decoder: Option<crate::continuous::ContinuousDecoder>,
    pub(super) stream: StreamLink,
    /// Stream utterance of the ghost utterance in progress, if streaming.
    pub(super) streamed: Option<Streamed>,
    /// Last stream utterance id used.
    pub(super) stream_id: u64,
    /// Ghost utterance decoded per tick to its end (the stream was not up
    /// at its start, or was lost during it). Its audio is never replayed.
    pub(super) per_tick: Option<u64>,
    /// Settled state of the latest stream partial, for the per-tick decoder
    /// to carry on from if the stream is lost.
    pub(super) stream_state: Option<crate::continuous::DecoderState>,
    /// Something is currently drawn as a ghost.
    pub(super) showing: bool,
    /// At least one utterance was committed this session (for spacing).
    pub(super) committed_any: bool,
    /// The addon was unreachable last time; avoid log spam.
    pub(super) warned_unavailable: bool,
}

impl GhostState {
    pub(super) fn new(client: crate::ghost::GhostClient) -> Self {
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
    pub(super) fn spaced(&self, text: &str) -> String {
        if self.committed_any {
            format!(" {}", text)
        } else {
            text.to_string()
        }
    }

    pub(super) fn note_result<T>(&mut self, r: &Result<T, crate::ghost::GhostError>) {
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
    pub(super) fn next_utterance(&mut self) {
        self.stream_cancel();
        self.utterance += 1;
        self.last_partial_at = None;
        self.decoder = None;
        self.stream_state = None;
    }

    pub(super) fn session(&self) -> Option<&crate::stream_client::StreamSession> {
        match &self.stream {
            StreamLink::Up(session) => Some(session),
            _ => None,
        }
    }

    /// Drop the stream utterance in progress, if any.
    pub(super) fn stream_cancel(&mut self) {
        if let Some(streamed) = self.streamed.take() {
            if let Some(session) = self.session() {
                session.cancel(streamed.id);
            }
        }
    }

    /// The current utterance's segment is complete: send the rest of it and
    /// `end`. Returns the stream utterance id when the server got the whole
    /// segment, so its `final` can stand in for a full transcription
    /// (`final_correction = false`); None when part of it was cut off.
    pub(super) fn stream_end(&mut self, samples: &[f32]) -> Option<u64> {
        let streamed = self.streamed.take()?;
        let session = self.session()?;
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
            (end == samples.len()).then_some(streamed.id)
        } else {
            session.cancel(streamed.id);
            None
        }
    }

    pub(super) fn clear(&mut self) {
        if self.showing {
            let r = self.client.clear();
            self.note_result(&r);
            self.showing = false;
        }
    }
}

impl StreamingEngine {
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
    pub(super) fn ghost_stream_link(&mut self) {
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
    pub(super) fn ghost_stream_events(&mut self) {
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
    pub(super) fn ghost_stream_feed(&mut self) -> bool {
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

    /// The stream's `final` for utterance `id`, when it can be committed
    /// as is. None (transcribe in full instead) on timeout, error, a lost
    /// stream, or a short result: short results go through the full
    /// transcription's no-speech check, which catches "Okay." on noise.
    pub(super) async fn ghost_stream_final(&mut self, id: u64) -> Option<String> {
        use crate::stream_client::StreamEvent;
        let ghost = self.ghost.as_mut()?;
        let StreamLink::Up(session) = &mut ghost.stream else {
            return None;
        };
        let deadline = tokio::time::Instant::now() + STREAM_FINAL_TIMEOUT;
        loop {
            match tokio::time::timeout_at(deadline, session.recv()).await {
                Ok(Some(StreamEvent::Final { utterance, text })) if utterance == id => {
                    let text = text.trim().to_string();
                    let words = text.split_whitespace().count();
                    return (words > crate::whisper::NO_SPEECH_CHECK_MAX_WORDS).then_some(text);
                }
                Ok(Some(StreamEvent::Error { utterance, .. })) if utterance == Some(id) => {
                    return None
                }
                Ok(Some(StreamEvent::Closed(reason))) => {
                    warn!("Ears stream lost ({}); decoding per tick", reason);
                    ghost.stream = StreamLink::Down {
                        retry_at: Some(Instant::now() + STREAM_RETRY),
                    };
                    return None;
                }
                Ok(None) => {
                    ghost.stream = StreamLink::Down {
                        retry_at: Some(Instant::now() + STREAM_RETRY),
                    };
                    return None;
                }
                Ok(Some(_)) => continue, // stale partials or other utterances
                Err(_) => {
                    debug!("No stream final for utterance {} in time", id);
                    return None;
                }
            }
        }
    }

    /// Whether ghost completion is active.
    pub fn ghost_enabled(&self) -> bool {
        self.ghost.is_some()
    }

    /// Apply finished partial transcriptions for the utterance in progress.
    pub(super) fn ghost_poll_partials(&mut self) {
        let muted = self.typing_mode == TypingMode::None;
        let filters = self.text_filters.clone();
        let language = self.language.clone();
        let bash = self.guided_grammar.is_some();
        let commands = (!bash && self.commands.enabled).then(|| self.commands.clone());
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
            let command = commands
                .as_ref()
                .and_then(|c| c.parse(&text))
                .is_some_and(|c| c.presses_key());
            // A command word so far is drawn whole in the accept (frozen)
            // colour, so you see it will press its key, not be typed.
            let frozen_bytes = if command {
                shown.len()
            } else if frozen_bytes > 0 {
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
    pub(super) fn ghost_maybe_start_partial(&mut self) {
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
    /// ordinary typing when the addon cannot deliver it. Returns how many
    /// characters reached the focused app, if any did.
    pub(super) fn ghost_commit(&mut self, text: &str) -> Option<usize> {
        if self.typing_mode == TypingMode::None {
            // Typing switched off (e.g. `ears typing off`): the transcript is
            // still published on IPC, but nothing reaches the focused app.
            self.ghost_clear();
            return None;
        }
        let ghost = self.ghost.as_mut()?;
        let spaced = ghost.spaced(text);
        let delivery = ghost.client.commit(&spaced);
        ghost.showing = false;
        match delivery {
            crate::ghost::Delivery::Delivered => {
                ghost.warned_unavailable = false;
                self.stats.chars_typed += spaced.chars().count();
                ghost.committed_any = true;
                Some(spaced.chars().count())
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
                    Some(spaced.chars().count())
                } else {
                    None
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
                None
            }
        }
    }

    /// Show a recognized command in the accept (frozen) colour for
    /// `accept_ms`, then remove it. Nothing is committed.
    pub(super) async fn ghost_accept(&mut self, heard: &str) {
        let hold = Duration::from_millis(self.commands.accept_ms);
        if let Some(ghost) = self.ghost.as_mut() {
            if !hold.is_zero() {
                let shown = ghost.spaced(heard.trim());
                let r = ghost.client.preedit_frozen(&shown, shown.len());
                ghost.note_result(&r);
                if r.is_ok() {
                    ghost.showing = true;
                    tokio::time::sleep(hold).await;
                }
            }
        }
        self.ghost_clear();
    }

    /// Remove the ghost (utterance produced nothing to commit).
    pub(super) fn ghost_clear(&mut self) {
        if let Some(ghost) = self.ghost.as_mut() {
            ghost.clear();
        }
    }
}
