//! Client side of the ears stream protocol (`docs/STREAM_PROTOCOL.md`).
//!
//! One persistent WebSocket to the vLLM `ears_stream` plugin replaces the
//! per-tick HTTP requests of [`crate::continuous::ContinuousDecoder`]: ears
//! sends only new audio and the server pushes the growing transcript back.
//!
//! A [`StreamSession`] never makes its owner wait on the network. Commands go
//! into an outbox that a background task drains into the socket; audio queued
//! for the same utterance is coalesced into one frame, so a slow socket costs
//! latency, not frames. Past [`MAX_QUEUED_BYTES`] the session gives up and
//! reports itself closed, which callers treat like a lost connection.
//!
//! Errors reuse [`ContinuousError`]: `Unsupported` means the server has no
//! stream endpoint (404 or refused upgrade, no `ready`, an `unsupported`
//! error) and the caller should use the per-tick HTTP decoder for the rest of
//! the process; `Failed` is transient.

use crate::continuous::{ContinuousError, DecoderState, ASR_TAG};
use futures_util::{SinkExt, StreamExt};
use std::collections::VecDeque;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::sync::{mpsc, Notify};
use tokio_tungstenite::tungstenite::{self, client::IntoClientRequest, Message};
use tracing::debug;

/// Protocol version this client speaks.
pub const PROTOCOL: u64 = 1;

/// How long the server may take to upgrade and then to send `ready`.
pub const READY_TIMEOUT: Duration = Duration::from_secs(2);

/// Audio waiting for the socket beyond this means it cannot keep up (about
/// two minutes of 16 kHz PCM16); the session closes instead of growing.
pub const MAX_QUEUED_BYTES: usize = 4 << 20;

/// `ready` from the server.
#[derive(Debug, Clone, PartialEq)]
pub struct Ready {
    pub model: Option<String>,
    pub sample_rate: u32,
    /// Longest utterance the server decodes.
    pub max_audio_ms: Option<u64>,
}

/// Options of a `start` frame; `None` leaves the server default.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct StartParams {
    /// ISO 639-1 code.
    pub language: Option<String>,
    /// Context-biasing text.
    pub context: Option<String>,
    pub rollback_words: Option<usize>,
    pub min_step_ms: Option<u64>,
}

impl StartParams {
    /// Same language and context as the HTTP decoder of `spec`.
    pub fn from_spec(spec: &crate::continuous::ContinuousSpec) -> Self {
        Self {
            language: spec.language.clone(),
            context: spec.context.clone().filter(|c| !c.trim().is_empty()),
            rollback_words: None,
            min_step_ms: None,
        }
    }
}

/// The full hypothesis for an utterance so far.
#[derive(Debug, Clone, PartialEq)]
pub struct StreamPartial {
    pub utterance: u64,
    pub seq: u64,
    pub text: String,
    /// Byte length of the settled prefix of `text`.
    pub stable_chars: usize,
    /// Qwen3-ASR language name in use, once known.
    pub language: Option<String>,
    pub audio_ms: u64,
}

impl StreamPartial {
    /// Settled prefix; a bad offset from the server settles less, never more.
    pub fn stable(&self) -> &str {
        let end = crate::freeze::boundary(&self.text, self.stable_chars);
        self.text[..end].trim_end()
    }

    /// The same settled state the HTTP decoder keeps, so it can take over.
    pub fn snapshot(&self) -> DecoderState {
        DecoderState {
            // "None" is what silence is detected as; never force it.
            header: self
                .language
                .as_ref()
                .filter(|name| name.as_str() != "None")
                .map(|name| format!("language {name}{ASR_TAG}")),
            stable: self.stable().to_string(),
            ..DecoderState::default()
        }
    }
}

/// Something the server said, or the end of the connection.
#[derive(Debug, Clone, PartialEq)]
pub enum StreamEvent {
    Partial(StreamPartial),
    Final {
        utterance: u64,
        text: String,
    },
    Error {
        utterance: Option<u64>,
        code: String,
        message: String,
    },
    /// The connection is gone; nothing more arrives and nothing is sent.
    Closed(String),
}

/// Server frames as they appear on the wire.
#[derive(serde::Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum Incoming {
    Ready {
        protocol: u64,
        model: Option<String>,
        sample_rate: Option<u32>,
        max_audio_ms: Option<u64>,
    },
    Partial {
        utterance: u64,
        #[serde(default)]
        seq: u64,
        text: String,
        #[serde(default)]
        stable_chars: usize,
        language: Option<String>,
        #[serde(default)]
        audio_ms: u64,
    },
    Final {
        utterance: u64,
        text: String,
    },
    Error {
        utterance: Option<u64>,
        code: String,
        #[serde(default)]
        message: String,
    },
    #[serde(other)]
    Unknown,
}

impl Incoming {
    fn into_event(self) -> Option<StreamEvent> {
        Some(match self {
            Incoming::Partial {
                utterance,
                seq,
                text,
                stable_chars,
                language,
                audio_ms,
            } => StreamEvent::Partial(StreamPartial {
                utterance,
                seq,
                text,
                stable_chars,
                language,
                audio_ms,
            }),
            Incoming::Final { utterance, text } => StreamEvent::Final { utterance, text },
            Incoming::Error {
                utterance,
                code,
                message,
            } => StreamEvent::Error {
                utterance,
                code,
                message,
            },
            Incoming::Ready { .. } | Incoming::Unknown => return None,
        })
    }
}

/// `ws(s)://host/v1/ears/stream` for an `http(s)://host` server URL.
pub fn stream_url(server_url: &str) -> Result<String, ContinuousError> {
    let base = server_url.trim_end_matches('/');
    let rest = |scheme: &str| base.strip_prefix(scheme);
    let url = if let Some(r) = rest("https://") {
        format!("wss://{r}")
    } else if let Some(r) = rest("http://") {
        format!("ws://{r}")
    } else if base.starts_with("ws://") || base.starts_with("wss://") {
        base.to_string()
    } else {
        return Err(ContinuousError::Unsupported(format!(
            "cannot stream to {server_url:?}"
        )));
    };
    Ok(format!("{url}/v1/ears/stream"))
}

enum Outgoing {
    Text(String),
    Audio { utterance: u64, bytes: Vec<u8> },
}

#[derive(Default)]
struct OutboxState {
    items: VecDeque<Outgoing>,
    queued_audio: usize,
    /// Why nothing more may be queued.
    closed: Option<String>,
}

/// Commands for the socket task. Only ever locked briefly, never across an
/// await, so the audio path cannot block on it.
#[derive(Default)]
struct Outbox {
    state: Mutex<OutboxState>,
    notify: Notify,
}

impl Outbox {
    fn lock(&self) -> std::sync::MutexGuard<'_, OutboxState> {
        self.state.lock().unwrap_or_else(|e| e.into_inner())
    }

    fn text(&self, json: serde_json::Value) -> bool {
        let mut s = self.lock();
        if s.closed.is_some() {
            return false;
        }
        s.items.push_back(Outgoing::Text(json.to_string()));
        drop(s);
        self.notify.notify_one();
        true
    }

    fn audio(&self, utterance: u64, bytes: &[u8]) -> bool {
        let mut s = self.lock();
        if s.closed.is_some() {
            return false;
        }
        if s.queued_audio + bytes.len() > MAX_QUEUED_BYTES {
            s.closed = Some("socket too slow for the audio".into());
            s.items.clear();
            drop(s);
            self.notify.notify_one();
            return false;
        }
        s.queued_audio += bytes.len();
        match s.items.back_mut() {
            Some(Outgoing::Audio {
                utterance: u,
                bytes: queued,
            }) if *u == utterance => queued.extend_from_slice(bytes),
            _ => s.items.push_back(Outgoing::Audio {
                utterance,
                bytes: bytes.to_vec(),
            }),
        }
        drop(s);
        self.notify.notify_one();
        true
    }

    /// Drop audio of `utterance` that has not gone out yet.
    fn purge(&self, utterance: u64) {
        let mut s = self.lock();
        let mut freed = 0;
        s.items.retain(|item| match item {
            Outgoing::Audio {
                utterance: u,
                bytes,
            } if *u == utterance => {
                freed += bytes.len();
                false
            }
            _ => true,
        });
        s.queued_audio -= freed;
    }

    fn pop(&self) -> Option<Outgoing> {
        let mut s = self.lock();
        let item = s.items.pop_front();
        if let Some(Outgoing::Audio { bytes, .. }) = &item {
            s.queued_audio -= bytes.len();
        }
        item
    }

    fn close(&self, reason: &str) {
        let mut s = self.lock();
        if s.closed.is_none() {
            s.closed = Some(reason.to_string());
        }
        drop(s);
        self.notify.notify_one();
    }

    fn closed(&self) -> Option<String> {
        self.lock().closed.clone()
    }
}

/// One connection speaking the ears stream protocol.
///
/// Every send returns at once: `false` means the connection is gone (a
/// [`StreamEvent::Closed`] says why). Dropping the session closes the socket,
/// which cancels the active utterance on the server.
pub struct StreamSession {
    ready: Ready,
    outbox: Arc<Outbox>,
    events: mpsc::UnboundedReceiver<StreamEvent>,
    task: tokio::task::JoinHandle<()>,
}

type Socket =
    tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>;

impl StreamSession {
    /// Connect and wait for `ready` (each step bounded by [`READY_TIMEOUT`]).
    pub async fn connect(server_url: &str, api_key: Option<&str>) -> Result<Self, ContinuousError> {
        let url = stream_url(server_url)?;
        let mut request = url
            .as_str()
            .into_client_request()
            .map_err(|e| ContinuousError::Unsupported(format!("{url}: {e}")))?;
        if let Some(key) = api_key {
            let value = format!("Bearer {key}")
                .parse()
                .map_err(|_| ContinuousError::Failed("API key is not a valid header".into()))?;
            request.headers_mut().insert("Authorization", value);
        }
        let mut socket =
            match tokio::time::timeout(READY_TIMEOUT, tokio_tungstenite::connect_async(request))
                .await
            {
                Err(_) => return Err(ContinuousError::Failed(format!("{url}: upgrade timed out"))),
                // 404: the plugin is not installed; any other answer but 101 is
                // a server that will not stream either.
                Ok(Err(tungstenite::Error::Http(response))) => {
                    return Err(ContinuousError::Unsupported(format!(
                        "{url}: upgrade refused ({})",
                        response.status()
                    )))
                }
                Ok(Err(e)) => return Err(ContinuousError::Failed(format!("{url}: {e}"))),
                Ok(Ok((socket, _))) => socket,
            };
        let ready = match tokio::time::timeout(READY_TIMEOUT, wait_ready(&mut socket)).await {
            Ok(result) => result?,
            Err(_) => {
                return Err(ContinuousError::Unsupported(format!(
                    "{url}: no ready within {READY_TIMEOUT:?}"
                )))
            }
        };
        let outbox = Arc::new(Outbox::default());
        let (tx, events) = mpsc::unbounded_channel();
        let task = tokio::spawn(run(socket, outbox.clone(), tx));
        Ok(Self {
            ready,
            outbox,
            events,
            task,
        })
    }

    pub fn ready(&self) -> &Ready {
        &self.ready
    }

    /// Longest utterance the server decodes, in samples.
    pub fn max_samples(&self) -> Option<usize> {
        self.ready
            .max_audio_ms
            .map(|ms| ms as usize * self.ready.sample_rate as usize / 1000)
    }

    /// Begin `utterance` (increasing per connection); cancels any other.
    pub fn start(&self, utterance: u64, params: &StartParams) -> bool {
        let mut frame = serde_json::json!({"type": "start", "utterance": utterance});
        if let Some(language) = &params.language {
            frame["language"] = language.as_str().into();
        }
        if let Some(context) = &params.context {
            frame["context"] = context.as_str().into();
        }
        if let Some(words) = params.rollback_words {
            frame["rollback_words"] = words.into();
        }
        if let Some(ms) = params.min_step_ms {
            frame["min_step_ms"] = ms.into();
        }
        self.outbox.text(frame)
    }

    /// Append samples to `utterance`.
    pub fn push(&self, utterance: u64, samples: &[i16]) -> bool {
        let bytes: Vec<u8> = samples.iter().flat_map(|s| s.to_le_bytes()).collect();
        self.outbox.audio(utterance, &bytes)
    }

    /// Append PCM16 little-endian bytes to `utterance`.
    pub fn push_bytes(&self, utterance: u64, bytes: &[u8]) -> bool {
        self.outbox.audio(utterance, bytes)
    }

    /// No more audio for `utterance`; the server answers with `final`.
    pub fn end(&self, utterance: u64) -> bool {
        self.outbox
            .text(serde_json::json!({"type": "end", "utterance": utterance}))
    }

    /// Drop `utterance`, including audio for it still queued here.
    pub fn cancel(&self, utterance: u64) -> bool {
        self.outbox.purge(utterance);
        self.outbox
            .text(serde_json::json!({"type": "cancel", "utterance": utterance}))
    }

    /// Next event, without waiting.
    pub fn try_recv(&mut self) -> Option<StreamEvent> {
        self.events.try_recv().ok()
    }

    /// Next event; `None` once the connection is gone and drained.
    pub async fn recv(&mut self) -> Option<StreamEvent> {
        self.events.recv().await
    }

    /// Whether commands can still be sent.
    pub fn is_open(&self) -> bool {
        self.outbox.closed().is_none() && !self.task.is_finished()
    }
}

impl Drop for StreamSession {
    fn drop(&mut self) {
        // The task flushes what is queued and closes the socket.
        self.outbox.close("session dropped");
    }
}

async fn wait_ready(socket: &mut Socket) -> Result<Ready, ContinuousError> {
    loop {
        let text = match socket.next().await {
            Some(Ok(Message::Text(text))) => text,
            Some(Ok(Message::Close(_))) | None => {
                return Err(ContinuousError::Unsupported(
                    "stream closed before ready".into(),
                ))
            }
            Some(Ok(_)) => continue,
            Some(Err(e)) => return Err(ContinuousError::Failed(e.to_string())),
        };
        return match serde_json::from_str::<Incoming>(&text) {
            Ok(Incoming::Ready {
                protocol,
                model,
                sample_rate,
                max_audio_ms,
            }) => {
                let sample_rate = sample_rate.unwrap_or(crate::continuous::SAMPLE_RATE as u32);
                if protocol != PROTOCOL || sample_rate != crate::continuous::SAMPLE_RATE as u32 {
                    Err(ContinuousError::Unsupported(format!(
                        "stream protocol {protocol} at {sample_rate} Hz"
                    )))
                } else {
                    Ok(Ready {
                        model,
                        sample_rate,
                        max_audio_ms,
                    })
                }
            }
            Ok(Incoming::Error { code, message, .. }) if code == "unsupported" => {
                Err(ContinuousError::Unsupported(message))
            }
            Ok(Incoming::Error { code, message, .. }) => {
                Err(ContinuousError::Failed(format!("{code}: {message}")))
            }
            _ => Err(ContinuousError::Unsupported(format!(
                "expected ready, got {}",
                crate::continuous::truncate(&text, 200)
            ))),
        };
    }
}

/// Own the socket: forward server frames as events, drain the outbox into
/// it. Ends with exactly one [`StreamEvent::Closed`].
async fn run(mut socket: Socket, outbox: Arc<Outbox>, events: mpsc::UnboundedSender<StreamEvent>) {
    let reason = 'conn: loop {
        tokio::select! {
            message = socket.next() => match message {
                Some(Ok(Message::Text(text))) => {
                    match serde_json::from_str::<Incoming>(&text) {
                        Ok(incoming) => {
                            if let Some(event) = incoming.into_event() {
                                let _ = events.send(event);
                            }
                        }
                        Err(e) => debug!("Stream: unreadable frame ({}): {}", e, text),
                    }
                }
                Some(Ok(Message::Close(_))) | None => break 'conn "closed by server".to_string(),
                Some(Ok(_)) => {}
                Some(Err(e)) => break 'conn e.to_string(),
            },
            _ = outbox.notify.notified() => {
                let mut wrote = false;
                while let Some(item) = outbox.pop() {
                    let message = match item {
                        Outgoing::Text(text) => Message::Text(text),
                        Outgoing::Audio { bytes, .. } => Message::Binary(bytes),
                    };
                    if let Err(e) = socket.feed(message).await {
                        break 'conn e.to_string();
                    }
                    wrote = true;
                }
                if wrote {
                    if let Err(e) = socket.flush().await {
                        break 'conn e.to_string();
                    }
                }
                if let Some(reason) = outbox.closed() {
                    let _ = socket.close(None).await;
                    break 'conn reason;
                }
            }
        }
    };
    outbox.close(&reason);
    let _ = events.send(StreamEvent::Closed(reason));
}

/// A scripted stand-in for the server plugin, for tests.
#[cfg(test)]
pub(crate) mod fake {
    use super::*;
    use tokio::io::AsyncWriteExt;
    use tokio::net::{TcpListener, TcpStream};
    use tokio_tungstenite::tungstenite::handshake::server::{ErrorResponse, Request, Response};

    #[derive(Debug, Clone, Copy, PartialEq)]
    pub enum Mode {
        /// `ready`, then log every frame.
        Normal,
        /// Upgrade, then never say anything.
        Silent,
        /// Answer the upgrade with 404 (plugin not installed).
        NotFound,
        /// `error unsupported`, then close.
        Unsupported,
    }

    /// Frame sent by a test to make the server drop the TCP connection.
    pub const DROP: &str = "__drop__";

    pub struct FakeServer {
        /// `http://127.0.0.1:PORT`, as a configured server URL.
        pub url: String,
        /// What the server saw: `auth ..`, `start {json}`, `audio N`,
        /// `end U`, `cancel U`.
        pub log: Arc<Mutex<Vec<String>>>,
        say: tokio::sync::broadcast::Sender<String>,
    }

    impl FakeServer {
        pub async fn start(mode: Mode) -> Self {
            Self::start_with_http(mode, None).await
        }

        /// Plain HTTP requests (no upgrade) are passed through to `http`,
        /// so one URL can serve both the stream and a wiremock server.
        pub async fn start_with_http(mode: Mode, http: Option<std::net::SocketAddr>) -> Self {
            let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
            let url = format!("http://{}", listener.local_addr().unwrap());
            let log = Arc::new(Mutex::new(Vec::new()));
            let (say, _) = tokio::sync::broadcast::channel(64);
            let (log2, say2) = (log.clone(), say.clone());
            tokio::spawn(async move {
                while let Ok((tcp, _)) = listener.accept().await {
                    let (log, rx) = (log2.clone(), say2.subscribe());
                    tokio::spawn(serve(tcp, mode, http, log, rx));
                }
            });
            Self { url, log, say }
        }

        /// Send a text frame to every connected client.
        pub fn say(&self, frame: serde_json::Value) {
            let _ = self.say.send(frame.to_string());
        }

        pub fn drop_connections(&self) {
            let _ = self.say.send(DROP.into());
        }

        pub fn log(&self) -> Vec<String> {
            self.log.lock().unwrap().clone()
        }

        /// Utterance ids of every `start` seen so far.
        pub fn starts(&self) -> Vec<u64> {
            self.log()
                .iter()
                .filter_map(|l| l.strip_prefix("start "))
                .map(|j| {
                    serde_json::from_str::<serde_json::Value>(j).unwrap()["utterance"]
                        .as_u64()
                        .unwrap()
                })
                .collect()
        }

        pub fn audio_bytes(&self) -> usize {
            self.log()
                .iter()
                .filter_map(|l| l.strip_prefix("audio "))
                .map(|n| n.parse::<usize>().unwrap())
                .sum()
        }

        /// Wait until the log has a line satisfying `pred`.
        pub async fn wait_for(&self, pred: impl Fn(&str) -> bool) -> String {
            for _ in 0..400 {
                if let Some(line) = self.log().into_iter().find(|l| pred(l)) {
                    return line;
                }
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
            panic!("fake stream server never saw it; log: {:?}", self.log());
        }
    }

    // The handshake callback's error type is tungstenite's.
    #[allow(clippy::result_large_err)]
    async fn serve(
        tcp: TcpStream,
        mode: Mode,
        http: Option<std::net::SocketAddr>,
        log: Arc<Mutex<Vec<String>>>,
        mut say: tokio::sync::broadcast::Receiver<String>,
    ) {
        if let Some(http) = http {
            let mut head = [0u8; 4096];
            let n = loop {
                let n = tcp.peek(&mut head).await.unwrap_or(0);
                if n == 0 || head[..n].windows(4).any(|w| w == b"\r\n\r\n") {
                    break n;
                }
                tokio::time::sleep(Duration::from_millis(1)).await;
            };
            let head = String::from_utf8_lossy(&head[..n]).to_ascii_lowercase();
            if !head.contains("upgrade: websocket") {
                let mut tcp = tcp;
                if let Ok(mut upstream) = TcpStream::connect(http).await {
                    let _ = tokio::io::copy_bidirectional(&mut tcp, &mut upstream).await;
                }
                let _ = tcp.shutdown().await;
                return;
            }
        }
        let log2 = log.clone();
        let callback = move |req: &Request, resp: Response| {
            if let Some(auth) = req.headers().get("authorization") {
                log2.lock()
                    .unwrap()
                    .push(format!("auth {}", auth.to_str().unwrap()));
            }
            if mode == Mode::NotFound {
                let mut err = ErrorResponse::new(Some("not found".into()));
                *err.status_mut() = tungstenite::http::StatusCode::NOT_FOUND;
                return Err(err);
            }
            Ok(resp)
        };
        let Ok(mut ws) = tokio_tungstenite::accept_hdr_async(tcp, callback).await else {
            return;
        };
        match mode {
            Mode::Normal => {
                let ready = serde_json::json!({"type": "ready", "protocol": 1,
                    "model": "Qwen/Qwen3-ASR-1.7B", "sample_rate": 16000, "max_audio_ms": 90000});
                let _ = ws.send(Message::Text(ready.to_string())).await;
            }
            Mode::Unsupported => {
                let err = serde_json::json!({"type": "error", "utterance": null,
                    "code": "unsupported", "message": "not Qwen3-ASR"});
                let _ = ws.send(Message::Text(err.to_string())).await;
                let _ = ws.close(None).await;
                return;
            }
            Mode::Silent | Mode::NotFound => {}
        }
        loop {
            tokio::select! {
                message = ws.next() => {
                    let line = match message {
                        Some(Ok(Message::Text(text))) => {
                            let v: serde_json::Value = serde_json::from_str(&text).unwrap();
                            match v["type"].as_str().unwrap() {
                                "start" => format!("start {text}"),
                                other => format!("{other} {}", v["utterance"]),
                            }
                        }
                        Some(Ok(Message::Binary(bytes))) => format!("audio {}", bytes.len()),
                        Some(Ok(_)) => continue,
                        _ => return,
                    };
                    log.lock().unwrap().push(line);
                }
                frame = say.recv() => match frame {
                    Ok(frame) if frame == DROP => return,
                    Ok(frame) => {
                        let _ = ws.send(Message::Text(frame)).await;
                    }
                    Err(_) => return,
                },
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::fake::{FakeServer, Mode};
    use super::*;

    #[test]
    fn stream_url_follows_the_server_url() {
        assert_eq!(
            stream_url("http://gpu:30189/").unwrap(),
            "ws://gpu:30189/v1/ears/stream"
        );
        assert_eq!(
            stream_url("https://asr.example.com").unwrap(),
            "wss://asr.example.com/v1/ears/stream"
        );
        assert!(matches!(
            stream_url("gpu:30189"),
            Err(ContinuousError::Unsupported(_))
        ));
    }

    #[test]
    fn partial_snapshot_matches_the_http_decoder_state() {
        let p = StreamPartial {
            utterance: 7,
            seq: 12,
            text: "Okay, so here is the".into(),
            stable_chars: 13,
            language: Some("English".into()),
            audio_ms: 2700,
        };
        assert_eq!(p.stable(), "Okay, so here");
        assert_eq!(
            p.snapshot(),
            DecoderState {
                header: Some("language English<asr_text>".into()),
                stable: "Okay, so here".into(),
                ..DecoderState::default()
            }
        );
        // Out of range or inside a character: settle less, never panic.
        let p = StreamPartial {
            text: "blåbær".into(),
            stable_chars: 3,
            language: None,
            ..p
        };
        assert_eq!(p.stable(), "");
        assert_eq!(p.snapshot().header, None);
        let p = StreamPartial {
            stable_chars: 99,
            ..p
        };
        assert_eq!(p.stable(), "");
    }

    #[tokio::test]
    async fn handshake_then_utterance_round_trip() {
        let server = FakeServer::start(Mode::Normal).await;
        let mut session = StreamSession::connect(&server.url, Some("sekrit"))
            .await
            .unwrap();
        assert_eq!(session.ready().max_audio_ms, Some(90_000));
        assert_eq!(session.max_samples(), Some(90 * 16_000));
        let params = StartParams {
            language: Some("en".into()),
            context: Some("vLLM".into()),
            rollback_words: Some(2),
            min_step_ms: None,
        };
        assert!(session.start(3, &params));
        assert!(session.push(3, &[1, 2, 3]));
        assert!(session.push_bytes(3, &[4, 0]));
        assert!(session.end(3));
        server.wait_for(|l| l == "end 3").await;
        let log = server.log();
        assert_eq!(log[0], "auth Bearer sekrit");
        let start: serde_json::Value =
            serde_json::from_str(log[1].strip_prefix("start ").unwrap()).unwrap();
        assert_eq!(
            start,
            serde_json::json!({"type": "start", "utterance": 3, "language": "en",
                               "context": "vLLM", "rollback_words": 2})
        );
        assert_eq!(server.audio_bytes(), 8);

        server.say(
            serde_json::json!({"type": "partial", "utterance": 3, "seq": 1,
            "text": "hello there", "stable_chars": 5, "language": "English",
            "audio_ms": 300, "decode_ms": 40}),
        );
        server.say(serde_json::json!({"type": "final", "utterance": 3,
            "text": "hello there.", "audio_ms": 300, "decode_ms": 50}));
        let StreamEvent::Partial(p) = session.recv().await.unwrap() else {
            panic!("expected a partial");
        };
        assert_eq!((p.utterance, p.stable()), (3, "hello"));
        assert_eq!(
            session.recv().await.unwrap(),
            StreamEvent::Final {
                utterance: 3,
                text: "hello there.".into()
            }
        );
    }

    #[tokio::test]
    async fn cancel_drops_audio_still_queued() {
        let outbox = Outbox::default();
        assert!(outbox.audio(1, &[0; 10]));
        assert!(outbox.audio(1, &[0; 6]));
        assert!(outbox.audio(2, &[0; 4]));
        // Same utterance back to back: one frame.
        assert_eq!(outbox.lock().items.len(), 2);
        outbox.purge(1);
        assert_eq!(outbox.lock().queued_audio, 4);
        assert!(matches!(
            outbox.pop(),
            Some(Outgoing::Audio { utterance: 2, .. })
        ));
        assert_eq!(outbox.lock().queued_audio, 0);
    }

    #[test]
    fn a_socket_that_cannot_keep_up_closes_the_session() {
        let outbox = Outbox::default();
        assert!(outbox.audio(1, &vec![0; MAX_QUEUED_BYTES]));
        assert!(!outbox.audio(1, &[0; 2]));
        assert!(outbox.closed().is_some());
        assert!(!outbox.text(serde_json::json!({"type": "end"})));
    }

    #[tokio::test]
    async fn missing_plugin_is_unsupported() {
        let server = FakeServer::start(Mode::NotFound).await;
        let err = StreamSession::connect(&server.url, None)
            .await
            .err()
            .unwrap();
        assert!(matches!(err, ContinuousError::Unsupported(_)), "{err}");
    }

    #[tokio::test]
    async fn unsupported_model_is_unsupported() {
        let server = FakeServer::start(Mode::Unsupported).await;
        let err = StreamSession::connect(&server.url, None)
            .await
            .err()
            .unwrap();
        assert!(matches!(err, ContinuousError::Unsupported(_)), "{err}");
    }

    #[tokio::test]
    async fn no_ready_is_unsupported() {
        let server = FakeServer::start(Mode::Silent).await;
        let err = StreamSession::connect(&server.url, None)
            .await
            .err()
            .unwrap();
        assert!(matches!(err, ContinuousError::Unsupported(_)), "{err}");
    }

    #[tokio::test]
    async fn refused_connection_is_transient() {
        let port = {
            let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
            l.local_addr().unwrap().port()
        };
        let err = StreamSession::connect(&format!("http://127.0.0.1:{port}"), None)
            .await
            .err()
            .unwrap();
        assert!(matches!(err, ContinuousError::Failed(_)), "{err}");
    }

    #[tokio::test]
    async fn lost_connection_is_reported_once() {
        let server = FakeServer::start(Mode::Normal).await;
        let mut session = StreamSession::connect(&server.url, None).await.unwrap();
        session.start(1, &StartParams::default());
        server.wait_for(|l| l.starts_with("start ")).await;
        server.drop_connections();
        assert!(matches!(session.recv().await, Some(StreamEvent::Closed(_))));
        assert!(!session.is_open());
        assert!(!session.push(1, &[0; 4]));
        assert_eq!(session.recv().await, None);
    }
}
