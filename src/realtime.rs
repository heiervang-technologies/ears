//! Streaming transcription over vLLM's realtime WebSocket (`WS /v1/realtime`).
//!
//! The batch ghost preview re-transcribes the whole growing recording on
//! every tick. This client instead appends audio to one server-side session
//! as it is recorded and receives text as the server decodes it, so no audio
//! is transcribed twice.
//!
//! Protocol (vLLM 0.28, `Qwen3ASRRealtimeGeneration`):
//!
//! ```text
//! <- session.created
//! -> session.update {model}
//! -> input_audio_buffer.commit            start decoding
//! -> input_audio_buffer.append {audio}    base64 PCM16 @ 16 kHz, repeated
//! -> input_audio_buffer.commit {final}    end of input
//! <- transcription.delta {delta}          repeated
//! <- transcription.done {text}
//! <- error {error, code}
//! ```
//!
//! The server cuts audio into fixed segments (5 s for Qwen3-ASR) and decodes
//! each once, with earlier segments as context. Each segment's text starts
//! with a `language X<asr_text>` header and ends with a newline, and the
//! blind cut can split a word or end a segment with a stray period.
//! [`TranscriptAssembler`] turns that into one clean transcript.

use anyhow::{anyhow, bail, Context, Result};
use base64::Engine;
use futures_util::stream::SplitSink;
use futures_util::{SinkExt, StreamExt};
use std::time::Duration;
use tokio::net::TcpStream;
use tokio::sync::mpsc;
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::{MaybeTlsStream, WebSocketStream};
use url::Url;

/// Sample rate the realtime endpoint expects.
pub const SAMPLE_RATE: u32 = 16_000;

/// Events are buffered up to this many; a stalled consumer backs the reader
/// up instead of growing memory.
const EVENT_QUEUE: usize = 256;

/// One server event, reduced to what ears acts on.
#[derive(Debug, Clone, PartialEq)]
pub enum RealtimeEvent {
    Delta(String),
    Done(String),
    Error(String),
}

/// `http(s)://host/base/` -> `ws(s)://host/base/v1/realtime`.
pub fn realtime_url(server: &Url) -> Result<Url> {
    let mut url = server.clone();
    let scheme = match server.scheme() {
        "http" | "ws" => "ws",
        "https" | "wss" => "wss",
        other => bail!("unsupported server scheme for realtime: {}", other),
    };
    url.set_scheme(scheme)
        .map_err(|_| anyhow!("cannot use {} as a WebSocket URL", server))?;
    let base = url.path().trim_end_matches('/').to_string();
    url.set_path(&format!("{}/v1/realtime", base));
    url.set_query(None);
    Ok(url)
}

/// The first model the server lists, for configs that leave `model` unset
/// (the realtime session requires a name; batch requests do not).
pub async fn default_model(server: &Url, api_key: Option<&str>) -> Result<String> {
    let base = server.as_str().trim_end_matches('/');
    let mut req = reqwest::Client::new()
        .get(format!("{}/v1/models", base))
        .timeout(Duration::from_secs(3));
    if let Some(key) = api_key {
        req = req.bearer_auth(key);
    }
    let body: serde_json::Value = req.send().await?.error_for_status()?.json().await?;
    body["data"][0]["id"]
        .as_str()
        .map(str::to_string)
        .context("server lists no models")
}

type WsStream = WebSocketStream<MaybeTlsStream<TcpStream>>;

/// A live transcription session. Dropping it closes the connection.
pub struct RealtimeSession {
    sink: SplitSink<WsStream, Message>,
    events: mpsc::Receiver<RealtimeEvent>,
    reader: tokio::task::JoinHandle<()>,
}

impl Drop for RealtimeSession {
    fn drop(&mut self) {
        self.reader.abort();
    }
}

impl RealtimeSession {
    /// Connect, select `model` and start decoding. Fails if the server has
    /// no realtime endpoint or does not answer within `timeout`.
    pub async fn connect(
        url: &Url,
        model: &str,
        api_key: Option<&str>,
        timeout: Duration,
    ) -> Result<Self> {
        let mut request = url.as_str().into_client_request()?;
        if let Some(key) = api_key {
            request
                .headers_mut()
                .insert("Authorization", format!("Bearer {}", key).parse()?);
        }
        let (ws, _) = tokio::time::timeout(timeout, tokio_tungstenite::connect_async(request))
            .await
            .map_err(|_| anyhow!("realtime connect timed out"))?
            .context("realtime connect")?;
        let (mut sink, mut stream) = ws.split();

        let created = tokio::time::timeout(timeout, stream.next())
            .await
            .map_err(|_| anyhow!("no session.created from realtime server"))?;
        match created {
            Some(Ok(Message::Text(text))) if event_type(&text) == Some("session.created") => {}
            other => bail!("unexpected realtime greeting: {:?}", other),
        }

        let (tx, events) = mpsc::channel(EVENT_QUEUE);
        let reader = tokio::spawn(async move {
            while let Some(msg) = stream.next().await {
                let event = match msg {
                    Ok(Message::Text(text)) => match parse_event(&text) {
                        Some(event) => event,
                        None => continue,
                    },
                    Ok(Message::Close(_)) | Err(_) => break,
                    Ok(_) => continue,
                };
                if tx.send(event).await.is_err() {
                    break;
                }
            }
        });

        sink.send(json_msg(
            serde_json::json!({"type": "session.update", "model": model}),
        ))
        .await?;
        sink.send(json_msg(
            serde_json::json!({"type": "input_audio_buffer.commit"}),
        ))
        .await?;
        Ok(Self {
            sink,
            events,
            reader,
        })
    }

    /// Append PCM16 mono audio at [`SAMPLE_RATE`]. Empty input is skipped
    /// (the server rejects it).
    pub async fn append(&mut self, pcm: &[u8]) -> Result<()> {
        let pcm = &pcm[..pcm.len() & !1];
        if pcm.is_empty() {
            return Ok(());
        }
        let audio = base64::engine::general_purpose::STANDARD.encode(pcm);
        self.sink
            .send(json_msg(serde_json::json!({
                "type": "input_audio_buffer.append",
                "audio": audio,
            })))
            .await
            .context("realtime append")
    }

    /// Mark the end of input; the server decodes what is left and sends
    /// `transcription.done`.
    pub async fn finish(&mut self) -> Result<()> {
        self.sink
            .send(json_msg(
                serde_json::json!({"type": "input_audio_buffer.commit", "final": true}),
            ))
            .await
            .context("realtime finish")
    }

    /// Next server event; `None` once the connection is gone.
    pub async fn recv(&mut self) -> Option<RealtimeEvent> {
        self.events.recv().await
    }

    /// An event that has already arrived, without waiting.
    pub fn try_recv(&mut self) -> Option<RealtimeEvent> {
        self.events.try_recv().ok()
    }
}

fn json_msg(value: serde_json::Value) -> Message {
    Message::Text(value.to_string())
}

fn event_type(text: &str) -> Option<&'static str> {
    let value: serde_json::Value = serde_json::from_str(text).ok()?;
    match value["type"].as_str()? {
        "session.created" => Some("session.created"),
        _ => None,
    }
}

/// Parse one server message; unknown event types are ignored.
pub fn parse_event(text: &str) -> Option<RealtimeEvent> {
    let value: serde_json::Value = serde_json::from_str(text).ok()?;
    let field = |name: &str| value[name].as_str().unwrap_or_default().to_string();
    match value["type"].as_str()? {
        "transcription.delta" => Some(RealtimeEvent::Delta(field("delta"))),
        "transcription.done" => Some(RealtimeEvent::Done(field("text"))),
        "error" => {
            let code = field("code");
            let error = field("error");
            Some(RealtimeEvent::Error(if code.is_empty() {
                error
            } else {
                format!("{} ({})", error, code)
            }))
        }
        _ => None,
    }
}

/// Builds the transcript from streamed deltas: strips each segment's
/// language header and repairs the seams between segments.
#[derive(Debug, Default, Clone)]
pub struct TranscriptAssembler {
    /// Finished segments, already cleaned.
    done: Vec<String>,
    /// Raw text of the segment being decoded.
    current: String,
}

impl TranscriptAssembler {
    pub fn push(&mut self, delta: &str) {
        self.current.push_str(delta);
        while let Some(i) = self.current.find('\n') {
            let segment = segment_text(&self.current[..i]);
            self.current.drain(..=i);
            if !segment.is_empty() {
                self.done.push(segment);
            }
        }
    }

    /// The transcript so far, including the segment still being decoded.
    pub fn text(&self) -> String {
        let current = segment_text(&self.current);
        let mut out = String::new();
        for segment in self.done.iter().chain(Some(&current)) {
            out = stitch(&out, segment);
        }
        out
    }
}

/// Text of one segment without its `language X<asr_text>` header. A header
/// still arriving yields nothing rather than leaking into the transcript.
fn segment_text(raw: &str) -> String {
    const TAG: &str = "<asr_text>";
    if let Some(i) = raw.find(TAG) {
        return raw[i + TAG.len()..].trim().to_string();
    }
    let trimmed = raw.trim_start();
    if trimmed.is_empty() || trimmed.starts_with("language") || TAG.starts_with(trimmed) {
        return String::new();
    }
    trimmed.trim_end().to_string()
}

/// Words that are almost never capitalised mid-sentence; a segment starting
/// with one of them after a seam period is a continuation, not a new sentence.
const CONTINUATION_WORDS: &[&str] = &[
    "a", "about", "after", "all", "also", "an", "and", "any", "are", "as", "at", "be", "because",
    "before", "but", "by", "can", "could", "did", "do", "does", "for", "from", "had", "has",
    "have", "her", "here", "him", "how", "if", "in", "into", "is", "it", "its", "just", "me",
    "more", "most", "my", "no", "not", "of", "on", "or", "our", "over", "really", "should", "so",
    "some", "than", "that", "the", "their", "them", "then", "there", "these", "they", "this",
    "those", "to", "us", "very", "was", "we", "were", "what", "when", "which", "while", "who",
    "will", "with", "would", "you", "your",
];

fn normalize(word: &str) -> String {
    word.chars()
        .filter(|c| c.is_alphanumeric() || *c == '\'')
        .flat_map(char::to_lowercase)
        .collect()
}

fn lowercase_first(word: &str) -> String {
    let mut chars = word.chars();
    match chars.next() {
        Some(first) => first.to_lowercase().chain(chars).collect(),
        None => String::new(),
    }
}

fn starts_lowercase(word: &str) -> bool {
    word.chars().next().is_some_and(char::is_lowercase)
}

/// Join two segments across a blind cut.
///
/// - The next segment usually repeats the word the cut split (`"the word."`
///   then `"Words arrive"`), so a trailing word that the next segment repeats
///   or completes is dropped, and the repeat takes that word's case.
/// - Otherwise a period at the cut is doubtful: it is removed when the next
///   segment continues with a word that would not start a sentence.
fn stitch(prev: &str, next: &str) -> String {
    if prev.is_empty() {
        return next.to_string();
    }
    if next.is_empty() {
        return prev.to_string();
    }
    let mut left: Vec<&str> = prev.split_whitespace().collect();
    let mut right: Vec<String> = next.split_whitespace().map(str::to_string).collect();

    let overlap = (1..=3.min(left.len()).min(right.len())).rev().find(|&k| {
        let tail = &left[left.len() - k..];
        let head = &right[..k];
        tail.iter()
            .zip(head)
            .all(|(a, b)| !normalize(a).is_empty() && normalize(a) == normalize(b))
    });
    let overlap = overlap.or_else(|| {
        // A split word: the cut kept its start, the next segment all of it.
        let last = normalize(left.last()?);
        let first = normalize(right.first()?);
        (last.chars().count() >= 3 && first.len() > last.len() && first.starts_with(&last))
            .then_some(1)
    });

    if let Some(k) = overlap {
        let dropped_lowercase = starts_lowercase(left[left.len() - k]);
        left.truncate(left.len() - k);
        if dropped_lowercase {
            right[0] = lowercase_first(&right[0]);
        }
    } else if let Some(last) = left.last_mut() {
        let continues = CONTINUATION_WORDS.contains(&normalize(&right[0]).as_str());
        if continues && last.ends_with('.') && !last.ends_with("..") {
            *last = &last[..last.len() - 1];
            right[0] = lowercase_first(&right[0]);
        }
    }

    left.iter()
        .map(|w| w.to_string())
        .chain(right)
        .collect::<Vec<_>>()
        .join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn assemble(deltas: &[&str]) -> String {
        let mut asm = TranscriptAssembler::default();
        for d in deltas {
            asm.push(d);
        }
        asm.text()
    }

    #[test]
    fn realtime_url_from_server() {
        let url = |s: &str| realtime_url(&Url::parse(s).unwrap()).unwrap().to_string();
        assert_eq!(
            url("http://localhost:30189/"),
            "ws://localhost:30189/v1/realtime"
        );
        assert_eq!(
            url("https://asr.example/base"),
            "wss://asr.example/base/v1/realtime"
        );
        assert!(realtime_url(&Url::parse("ftp://x/").unwrap()).is_err());
    }

    #[test]
    fn parses_server_events() {
        assert_eq!(
            parse_event(r#"{"type":"transcription.delta","delta":" hi"}"#),
            Some(RealtimeEvent::Delta(" hi".into()))
        );
        assert_eq!(
            parse_event(r#"{"type":"transcription.done","text":"hi","usage":null}"#),
            Some(RealtimeEvent::Done("hi".into()))
        );
        assert_eq!(
            parse_event(r#"{"type":"error","error":"bad","code":"invalid_audio"}"#),
            Some(RealtimeEvent::Error("bad (invalid_audio)".into()))
        );
        assert_eq!(parse_event(r#"{"type":"session.created","id":"s"}"#), None);
        assert_eq!(parse_event("not json"), None);
    }

    #[test]
    fn strips_language_header_even_mid_stream() {
        assert_eq!(assemble(&["language"]), "");
        assert_eq!(assemble(&["language", " English", "<asr"]), "");
        assert_eq!(
            assemble(&["language", " English", "<asr_text>", "Hello"]),
            "Hello"
        );
        assert_eq!(assemble(&["language None<asr_text>", "", "\n"]), "");
    }

    /// Seams recorded from Qwen3-ASR-1.7B on vLLM 0.28 (espeak, 24 s).
    #[test]
    fn repairs_recorded_segment_seams() {
        let text = assemble(&[
            "language English<asr_text>This is a longer test of real-time streaming \
             transcription. We want to see whether the word.\n",
            "language English<asr_text>Words arrive before the speaker has finished \
             talking. The model processes audio in segments.\n",
            "language English<asr_text>Segments of five seconds, and each segment is \
             appended to the same session. Let's.\n",
            "language English<asr_text>Let's count a few things: one, two, three, four, \
             five. Finally.\n",
            "language English<asr_text>The quick brown fox jumps over the lazy dog near \
             the river bank.",
        ]);
        assert_eq!(
            text,
            "This is a longer test of real-time streaming transcription. We want to see \
             whether the words arrive before the speaker has finished talking. The model \
             processes audio in segments of five seconds, and each segment is appended to \
             the same session. Let's count a few things: one, two, three, four, five. \
             Finally the quick brown fox jumps over the lazy dog near the river bank."
        );
    }

    #[test]
    fn keeps_real_sentence_breaks() {
        assert_eq!(
            stitch("It rained.", "Markus stayed in."),
            "It rained. Markus stayed in."
        );
        assert_eq!(stitch("Is it?", "The end."), "Is it? The end.");
        assert_eq!(stitch("Wait...", "the end"), "Wait... the end");
    }

    /// Fake `/v1/realtime`: checks the client's lifecycle and streams text
    /// back while audio is still arriving.
    #[tokio::test]
    async fn session_streams_audio_and_text() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            let (tcp, _) = listener.accept().await.unwrap();
            let mut ws = tokio_tungstenite::accept_async(tcp).await.unwrap();
            ws.send(Message::Text(
                r#"{"type":"session.created","id":"s"}"#.into(),
            ))
            .await
            .unwrap();
            let mut seen = Vec::new();
            let mut audio_bytes = 0;
            while let Some(Ok(Message::Text(text))) = ws.next().await {
                let event: serde_json::Value = serde_json::from_str(&text).unwrap();
                let kind = event["type"].as_str().unwrap().to_string();
                match kind.as_str() {
                    "input_audio_buffer.append" => {
                        audio_bytes += base64::engine::general_purpose::STANDARD
                            .decode(event["audio"].as_str().unwrap())
                            .unwrap()
                            .len();
                        let delta = r#"{"type":"transcription.delta","delta":"language English<asr_text>Hi\n"}"#;
                        ws.send(Message::Text(delta.into())).await.unwrap();
                    }
                    "input_audio_buffer.commit" if event["final"] == true => {
                        let done = r#"{"type":"transcription.done","text":"Hi"}"#;
                        ws.send(Message::Text(done.into())).await.unwrap();
                        seen.push("final".to_string());
                        break;
                    }
                    _ => seen.push(format!(
                        "{} {}",
                        kind,
                        event["model"].as_str().unwrap_or("")
                    )),
                }
            }
            (seen, audio_bytes)
        });

        let url = Url::parse(&format!("ws://{}/v1/realtime", addr)).unwrap();
        let mut session = RealtimeSession::connect(&url, "m", None, Duration::from_secs(2))
            .await
            .unwrap();
        session.append(&[1, 2, 3]).await.unwrap(); // odd byte dropped
        session.append(&[]).await.unwrap(); // skipped
        assert_eq!(
            session.recv().await,
            Some(RealtimeEvent::Delta(
                "language English<asr_text>Hi\n".into()
            ))
        );
        session.finish().await.unwrap();
        assert_eq!(session.recv().await, Some(RealtimeEvent::Done("Hi".into())));

        let (seen, audio_bytes) = server.await.unwrap();
        assert_eq!(
            seen,
            ["session.update m", "input_audio_buffer.commit ", "final"]
        );
        assert_eq!(audio_bytes, 2);
    }

    #[tokio::test]
    async fn connect_fails_without_realtime_endpoint() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            let (mut tcp, _) = listener.accept().await.unwrap();
            use tokio::io::AsyncWriteExt;
            let _ = tcp
                .write_all(b"HTTP/1.1 404 Not Found\r\ncontent-length: 0\r\n\r\n")
                .await;
        });
        let url = Url::parse(&format!("ws://{}/v1/realtime", addr)).unwrap();
        assert!(
            RealtimeSession::connect(&url, "m", None, Duration::from_secs(2))
                .await
                .is_err()
        );
    }

    #[test]
    fn short_prefixes_are_not_split_words() {
        assert_eq!(
            stitch("put it in", "into the box"),
            "put it in into the box"
        );
    }
}
