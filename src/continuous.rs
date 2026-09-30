//! Continuous live decoding for Qwen3-ASR on vLLM (issue #144).
//!
//! The repeated preview re-transcribes the whole growing clip on every tick,
//! so each tick costs more than the last. This decoder keeps every tick
//! roughly constant instead:
//!
//! * **Audio as encoder windows.** Qwen3-ASR's audio encoder attends within
//!   fixed 8 s windows that never see each other. The clip is sent as one
//!   audio block made of 8 s items, so a closed window is byte-identical from
//!   tick to tick and vLLM's encoder and prefix caches serve it. Only the open
//!   window is encoded again. Measured against a single audio item on the same
//!   clip: mean KL 0.0005 over the transcript tokens, identical argmax.
//! * **Settled text as a forced prefix.** Everything but the last few words of
//!   the previous hypothesis is fed back as the start of the assistant turn,
//!   so the model only decodes the continuation. The last words stay open to
//!   revision as more audio arrives.
//!
//! This needs the server to accept a per-request chat template (vLLM
//! `--trust-request-chat-template`): the stock Qwen3-ASR template drops
//! assistant turns and wraps each audio item in its own markers. When the
//! server refuses, [`ContinuousError::Unsupported`] tells the caller to fall
//! back to the repeated preview.

use base64::{engine::general_purpose::STANDARD as BASE64, Engine as _};
use std::time::Duration;

/// Samples per second the recorder produces (mono PCM16).
pub const SAMPLE_RATE: usize = 16_000;

/// Qwen3-ASR encoder attention window: `n_window_infer` 800 mel frames at a
/// 10 ms hop.
pub const ENCODER_WINDOW: usize = 8 * SAMPLE_RATE;

/// Words at the end of a hypothesis that stay open to revision.
pub const DEFAULT_ROLLBACK_WORDS: usize = 3;

/// A tail shorter than this is left out of the request: too short to carry a
/// word, and the encoder rejects items that produce no features.
const MIN_ITEM: usize = SAMPLE_RATE / 10;

/// Ends the `language X` header Qwen3-ASR writes before the transcript.
/// Audio needed before a detected language may be pinned.
pub const MIN_PIN_SAMPLES: usize = 2 * SAMPLE_RATE;

pub const ASR_TAG: &str = "<asr_text>";

/// One audio block whose `<|audio_pad|>` placeholders are filled by the
/// window items in order, followed by the assistant prefix. With a single
/// item this renders exactly like the stock template.
const CHAT_TEMPLATE: &str = concat!(
    "{%- set ns = namespace(n=0, asst='', sys='') -%}",
    "{%- for m in messages -%}",
    "{%- if m.role == 'system' -%}",
    "{%- set ns.sys = m.content if m.content is string else (m.content | map(attribute='text') | join('')) -%}",
    "{%- endif -%}",
    // vLLM hands string content to a template that iterates content as a
    // list of text parts.
    "{%- if m.role == 'assistant' -%}",
    "{%- set ns.asst = m.content if m.content is string else (m.content | map(attribute='text') | join('')) -%}",
    "{%- endif -%}",
    "{%- if m.role == 'user' and m.content is not string -%}",
    "{%- for c in m.content -%}",
    "{%- if c.type == 'audio' or c.type == 'input_audio' or ('audio' in c) or ('audio_url' in c) -%}",
    "{%- set ns.n = ns.n + 1 -%}",
    "{%- endif -%}",
    "{%- endfor -%}",
    "{%- endif -%}",
    "{%- endfor -%}",
    "{{- '<|im_start|>system\\n' + ns.sys + '<|im_end|>\\n<|im_start|>user\\n<|audio_start|>' -}}",
    "{{- '<|audio_pad|>' * ns.n -}}",
    "{{- '<|audio_end|><|im_end|>\\n<|im_start|>assistant\\n' + ns.asst -}}",
);

#[derive(Debug, thiserror::Error)]
pub enum ContinuousError {
    /// The server cannot run continuous decoding; use the repeated preview.
    #[error("continuous decoding unsupported by server: {0}")]
    Unsupported(String),
    /// This tick failed; the next one may succeed.
    #[error("continuous decoding failed: {0}")]
    Failed(String),
}

/// Where and how to decode; makes a fresh decoder per utterance.
#[derive(Debug, Clone)]
pub struct ContinuousSpec {
    pub server_url: String,
    pub api_key: Option<String>,
    pub model: Option<String>,
    pub language: Option<String>,
    /// Context-biasing text (the `prompt` config).
    pub context: Option<String>,
}

impl ContinuousSpec {
    pub fn decoder(&self) -> ContinuousDecoder {
        ContinuousDecoder::new(&self.server_url, self.api_key.clone(), self.model.clone())
            .with_language(self.language.as_deref())
            .with_context(self.context.clone())
    }
}

/// Decoder state for one utterance.
pub struct ContinuousDecoder {
    http: reqwest::Client,
    url: String,
    api_key: Option<String>,
    model: Option<String>,
    rollback_words: usize,
    /// Context-biasing text, sent as the system turn like the stock template.
    context: Option<String>,
    /// `language X<asr_text>`, fixed up front or learned from the first reply.
    header: Option<String>,
    /// Language detected by the last decode while none is pinned.
    candidate: Option<String>,
    /// Transcript text the next tick forces as prefix.
    stable: String,
    /// Base64 WAV of each closed window, so it is encoded once.
    closed: Vec<String>,
}

impl ContinuousDecoder {
    pub fn new(server_url: &str, api_key: Option<String>, model: Option<String>) -> Self {
        Self {
            http: reqwest::Client::builder()
                .connect_timeout(Duration::from_secs(2))
                .build()
                .expect("Failed to build HTTP client"),
            url: format!("{}/v1/chat/completions", server_url.trim_end_matches('/')),
            api_key,
            model,
            rollback_words: DEFAULT_ROLLBACK_WORDS,
            context: None,
            header: None,
            candidate: None,
            stable: String::new(),
            closed: Vec::new(),
        }
    }

    /// Pin the transcription language (ISO code such as "en"). Unknown codes
    /// are left to the model's own detection.
    pub fn with_language(mut self, code: Option<&str>) -> Self {
        self.header = code
            .and_then(language_name)
            .map(|name| format!("language {name}{ASR_TAG}"));
        self
    }

    pub fn with_context(mut self, context: Option<String>) -> Self {
        self.context = context.filter(|c| !c.trim().is_empty());
        self
    }

    pub fn with_rollback_words(mut self, words: usize) -> Self {
        self.rollback_words = words;
        self
    }

    /// Settled state, for handing the utterance to another process.
    pub fn snapshot(&self) -> DecoderState {
        DecoderState {
            header: self.header.clone(),
            stable: self.stable.clone(),
            ..DecoderState::default()
        }
    }

    /// Continue an utterance another decoder started.
    pub fn resume(mut self, state: DecoderState) -> Self {
        if state.header.is_some() {
            self.header = state.header;
        }
        self.stable = state.stable;
        self
    }

    /// Model the decoder uses, once known.
    pub fn model(&self) -> Option<&str> {
        self.model.as_deref()
    }

    /// Decode `pcm` (the whole utterance so far) and return the full
    /// hypothesis. With `last` the whole hypothesis is final; otherwise its
    /// last words stay open for the next tick. `deadline` bounds the whole
    /// step, model lookup included.
    pub async fn step(
        &mut self,
        pcm: &[i16],
        last: bool,
        deadline: Duration,
    ) -> Result<String, ContinuousError> {
        tokio::time::timeout(deadline, self.step_inner(pcm, last))
            .await
            .map_err(|_| ContinuousError::Failed("timed out".to_string()))?
    }

    async fn step_inner(&mut self, pcm: &[i16], last: bool) -> Result<String, ContinuousError> {
        let model = match &self.model {
            Some(m) if !is_qwen3_asr(m) => {
                return Err(ContinuousError::Unsupported(format!(
                    "model {m:?} is not Qwen3-ASR"
                )));
            }
            Some(m) => m.clone(),
            None => {
                let m = self.server_model().await?;
                self.model = Some(m.clone());
                m
            }
        };

        let windows: Vec<&[i16]> = pcm.chunks(ENCODER_WINDOW).collect();
        let mut audio = Vec::with_capacity(windows.len());
        for (i, w) in windows.iter().enumerate() {
            let closed = w.len() == ENCODER_WINDOW;
            if closed && i < self.closed.len() {
                audio.push(self.closed[i].clone());
                continue;
            }
            if w.len() < MIN_ITEM {
                continue;
            }
            let b64 = BASE64.encode(wav_bytes(w));
            if closed && i == self.closed.len() {
                self.closed.push(b64.clone());
            }
            audio.push(b64);
        }
        if audio.is_empty() {
            return Ok(self.stable.clone());
        }

        // Without a known header the model writes it first; only then can
        // settled text follow it.
        let prefix = match &self.header {
            Some(h) => format!("{h}{}", self.stable),
            None => String::new(),
        };
        let content: Vec<_> = audio
            .into_iter()
            .map(|data| {
                serde_json::json!({"type": "input_audio",
                                   "input_audio": {"data": data, "format": "wav"}})
            })
            .collect();
        let mut messages = Vec::with_capacity(3);
        if let Some(context) = &self.context {
            messages.push(serde_json::json!({"role": "system", "content": context}));
        }
        messages.push(serde_json::json!({"role": "user", "content": content}));
        messages.push(serde_json::json!({"role": "assistant", "content": prefix}));
        let body = serde_json::json!({
            "model": model,
            "messages": messages,
            "add_generation_prompt": false,
            "continue_final_message": true,
            "chat_template": CHAT_TEMPLATE,
            "temperature": 0.0,
            // A cap against runaway loops, not a budget: speech runs about 3
            // tokens a second, and a cold start decodes the whole clip.
            "max_tokens": 64 + 5 * pcm.len() / SAMPLE_RATE,
        });

        let mut request = self.http.post(&self.url).json(&body);
        if let Some(key) = &self.api_key {
            request = request.bearer_auth(key);
        }
        let response = request
            .send()
            .await
            .map_err(|e| ContinuousError::Failed(e.to_string()))?;
        let status = response.status();
        let text = response
            .text()
            .await
            .map_err(|e| ContinuousError::Failed(e.to_string()))?;
        if !status.is_success() {
            return Err(classify_error(status, &text));
        }
        let reply: serde_json::Value =
            serde_json::from_str(&text).map_err(|e| ContinuousError::Failed(e.to_string()))?;
        let choice = &reply["choices"][0];
        let continuation = choice["message"]["content"].as_str().ok_or_else(|| {
            ContinuousError::Failed("ASR response is missing string message.content".into())
        })?;
        // Cut off by max_tokens: the text is a runaway, not a transcript.
        if choice["finish_reason"].as_str() == Some("length") {
            return Err(ContinuousError::Failed("decode hit max_tokens".to_string()));
        }
        // The final tick must re-decode the words left open; an empty
        // continuation there would silently drop them.
        if last && !self.stable.is_empty() && continuation.trim().is_empty() {
            return Err(ContinuousError::Failed(
                "final decode dropped the open words".to_string(),
            ));
        }

        let hypothesis = if self.header.is_some() {
            format!("{}{}", self.stable, continuation)
        } else {
            match continuation.split_once(ASR_TAG) {
                Some((lang, rest)) => {
                    self.learn_language(lang.trim(), pcm.len());
                    rest.to_string()
                }
                None => continuation.to_string(),
            }
        };
        let hypothesis = hypothesis.trim_start().to_string();
        if !last && self.header.is_some() {
            // A shorter continuation must never retract the forced prefix.
            let settled = settled_prefix(&hypothesis, self.rollback_words);
            if settled.len() > self.stable.len() && settled.starts_with(&self.stable) {
                self.stable = settled.to_string();
            }
        }
        Ok(hypothesis.trim_end().to_string())
    }

    /// Pin the detected language once it is trustworthy. Forcing a wrong
    /// one turns the rest of the utterance into a translation, and a fraction
    /// of a second of audio is easily misdetected (English espeak came out as
    /// Arabic, #153). So pin only after [`MIN_PIN_SAMPLES`] of audio and two
    /// decodes in a row agreeing; "language None" (silence) never counts.
    fn learn_language(&mut self, lang: &str, samples: usize) {
        if lang == "language None" {
            self.candidate = None;
            return;
        }
        let header = format!("{lang}{ASR_TAG}");
        if samples >= MIN_PIN_SAMPLES && self.candidate.as_deref() == Some(header.as_str()) {
            self.header = Some(header);
            self.candidate = None;
        } else {
            self.candidate = Some(header);
        }
    }

    async fn server_model(&self) -> Result<String, ContinuousError> {
        let url = self.url.replace("/v1/chat/completions", "/v1/models");
        let mut request = self.http.get(url);
        if let Some(key) = &self.api_key {
            request = request.bearer_auth(key);
        }
        let response = request
            .send()
            .await
            .map_err(|e| ContinuousError::Failed(e.to_string()))?;
        // A server without an OpenAI model list (whisper.cpp) is not vLLM.
        if !response.status().is_success() {
            return Err(ContinuousError::Unsupported(format!(
                "no model list ({})",
                response.status()
            )));
        }
        let reply: serde_json::Value = response
            .json()
            .await
            .map_err(|e| ContinuousError::Unsupported(format!("no model list: {e}")))?;
        let id = reply["data"][0]["id"].as_str().unwrap_or_default();
        if !is_qwen3_asr(id) {
            return Err(ContinuousError::Unsupported(format!(
                "model {id:?} is not Qwen3-ASR"
            )));
        }
        Ok(id.to_string())
    }
}

/// What one decoder hands to the next for the same utterance.
#[derive(Debug, Clone, Default, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct DecoderState {
    pub header: Option<String>,
    pub stable: String,
    /// Which recording this belongs to (set by the caller).
    #[serde(default)]
    pub owner: Option<String>,
    /// Continuous decoding stopped working for this recording.
    #[serde(default)]
    pub unsupported: bool,
    /// Text of the segments already finished (see [`Rollover`]).
    #[serde(default)]
    pub prefix: String,
    /// Where the current segment starts, in samples.
    #[serde(default)]
    pub offset: usize,
}

/// Long recordings outgrow the server's context (about 90 s of audio). A
/// long recording is therefore decoded in segments: once the current one is
/// long enough, it is finished at a pause and a fresh one starts where it
/// ended. The finished text is kept verbatim in front of the live one.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Rollover {
    /// Finish the segment at the first pause after this many samples.
    pub soft: usize,
    /// Finish it here even without a pause.
    pub hard: usize,
}

impl Rollover {
    pub const DEFAULT: Rollover = Rollover {
        soft: 60 * SAMPLE_RATE,
        hard: 80 * SAMPLE_RATE,
    };

    /// Whether a segment of `len` samples ending in `recent` should end now.
    pub fn due(&self, len: usize, recent: &[i16]) -> bool {
        len >= self.hard || (len >= self.soft && is_pause(recent))
    }
}

/// Samples inspected for a pause at a segment boundary.
pub const PAUSE_WINDOW: usize = SAMPLE_RATE * 3 / 10;

/// Whether `pcm` (the last [`PAUSE_WINDOW`] samples) is quiet enough to cut
/// between words: RMS under about -36 dBFS.
pub fn is_pause(pcm: &[i16]) -> bool {
    if pcm.len() < PAUSE_WINDOW {
        return false;
    }
    let tail = &pcm[pcm.len() - PAUSE_WINDOW..];
    let energy: f64 = tail.iter().map(|&s| f64::from(s) * f64::from(s)).sum();
    (energy / tail.len() as f64).sqrt() < 500.0
}

/// Finished segments' text followed by the live segment's.
pub fn join_segments(prefix: &str, text: &str) -> String {
    match (prefix.trim_end(), text.trim_start()) {
        ("", t) => t.to_string(),
        (p, "") => p.to_string(),
        (p, t) => format!("{p} {t}"),
    }
}

fn is_qwen3_asr(model: &str) -> bool {
    model.to_ascii_lowercase().contains("qwen3-asr")
}

fn classify_error(status: reqwest::StatusCode, body: &str) -> ContinuousError {
    let lower = body.to_ascii_lowercase();
    // A refused template or an audio layout the model class rejects will not
    // fix itself on the next tick.
    if lower.contains("chat template") || lower.contains("only one audio") || status == 404 {
        ContinuousError::Unsupported(format!("{status}: {}", truncate(body, 200)))
    } else {
        ContinuousError::Failed(format!("{status}: {}", truncate(body, 200)))
    }
}

pub(crate) fn truncate(s: &str, max: usize) -> &str {
    match s.char_indices().nth(max) {
        Some((i, _)) => &s[..i],
        None => s,
    }
}

/// `text` minus its last `words` words, cut at a word boundary so the kept
/// part is byte-identical to what the model produced.
pub fn settled_prefix(text: &str, words: usize) -> &str {
    if words == 0 {
        return text.trim_end();
    }
    let starts: Vec<usize> = text
        .char_indices()
        .filter(|&(i, c)| {
            !c.is_whitespace()
                && (i == 0
                    || text[..i]
                        .chars()
                        .next_back()
                        .is_some_and(char::is_whitespace))
        })
        .map(|(i, _)| i)
        .collect();
    if starts.len() <= words {
        return "";
    }
    text[..starts[starts.len() - words]].trim_end()
}

/// Qwen3-ASR language name for an ISO code; the model is prompted with names.
pub fn language_name(code: &str) -> Option<&'static str> {
    Some(match code.to_ascii_lowercase().as_str() {
        "en" => "English",
        "zh" => "Chinese",
        "de" => "German",
        "fr" => "French",
        "es" => "Spanish",
        "it" => "Italian",
        "pt" => "Portuguese",
        "ru" => "Russian",
        "ja" => "Japanese",
        "ko" => "Korean",
        "nl" => "Dutch",
        "sv" => "Swedish",
        "da" => "Danish",
        "fi" => "Finnish",
        "pl" => "Polish",
        "tr" => "Turkish",
        "ar" => "Arabic",
        "hi" => "Hindi",
        "id" => "Indonesian",
        "th" => "Thai",
        "vi" => "Vietnamese",
        "ms" => "Malay",
        "cs" => "Czech",
        "el" => "Greek",
        "hu" => "Hungarian",
        "ro" => "Romanian",
        "fa" => "Persian",
        "mk" => "Macedonian",
        _ => return None,
    })
}

/// Mono 16 kHz PCM16 WAV in memory.
pub fn wav_bytes(pcm: &[i16]) -> Vec<u8> {
    let data_len = (pcm.len() * 2) as u32;
    let mut out = Vec::with_capacity(44 + data_len as usize);
    out.extend_from_slice(b"RIFF");
    out.extend_from_slice(&(36 + data_len).to_le_bytes());
    out.extend_from_slice(b"WAVEfmt ");
    out.extend_from_slice(&16u32.to_le_bytes());
    out.extend_from_slice(&1u16.to_le_bytes());
    out.extend_from_slice(&1u16.to_le_bytes());
    out.extend_from_slice(&(SAMPLE_RATE as u32).to_le_bytes());
    out.extend_from_slice(&(SAMPLE_RATE as u32 * 2).to_le_bytes());
    out.extend_from_slice(&2u16.to_le_bytes());
    out.extend_from_slice(&16u16.to_le_bytes());
    out.extend_from_slice(b"data");
    out.extend_from_slice(&data_len.to_le_bytes());
    for s in pcm {
        out.extend_from_slice(&s.to_le_bytes());
    }
    out
}

/// PCM16 samples from little-endian bytes (a trailing odd byte is dropped).
pub fn samples(bytes: &[u8]) -> Vec<i16> {
    let (pairs, _) = bytes.as_chunks::<2>();
    pairs.iter().map(|&b| i16::from_le_bytes(b)).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn settled_prefix_keeps_exact_bytes() {
        assert_eq!(
            settled_prefix("Okay, so here is the plan.", 2),
            "Okay, so here is"
        );
        assert_eq!(settled_prefix("a  b\tc d", 1), "a  b\tc");
        assert_eq!(settled_prefix("one two", 3), "");
        assert_eq!(settled_prefix("", 3), "");
        assert_eq!(settled_prefix("one two", 0), "one two");
    }

    #[test]
    fn wav_bytes_is_a_valid_header() {
        let wav = wav_bytes(&[1, -1, 3]);
        assert_eq!(wav.len(), 44 + 6);
        assert_eq!(&wav[0..4], b"RIFF");
        assert_eq!(u32::from_le_bytes(wav[40..44].try_into().unwrap()), 6);
        assert_eq!(samples(&wav[44..]), vec![1, -1, 3]);
    }

    #[test]
    fn rollover_waits_for_a_pause_until_the_hard_limit() {
        let r = Rollover {
            soft: 10,
            hard: 20 * SAMPLE_RATE,
        };
        let speech: Vec<i16> = (0..PAUSE_WINDOW)
            .map(|i| if i % 2 == 0 { 3000 } else { -3000 })
            .collect();
        let quiet = vec![20i16; PAUSE_WINDOW];
        assert!(!r.due(5, &quiet), "too short");
        assert!(!r.due(SAMPLE_RATE, &speech), "no pause yet");
        assert!(r.due(SAMPLE_RATE, &quiet));
        assert!(r.due(20 * SAMPLE_RATE, &speech), "hard limit");
        assert!(!is_pause(&quiet[..10]), "not enough audio to judge");
    }

    #[test]
    fn segments_join_with_one_space() {
        assert_eq!(join_segments("", "b"), "b");
        assert_eq!(join_segments("a.", ""), "a.");
        assert_eq!(join_segments("a. ", " b"), "a. b");
    }

    #[test]
    fn language_names() {
        assert_eq!(language_name("EN"), Some("English"));
        assert_eq!(language_name("xx"), None);
    }

    #[test]
    fn refused_template_is_unsupported() {
        let e = classify_error(
            reqwest::StatusCode::BAD_REQUEST,
            "Chat template is passed with request, but --trust-request-chat-template is not set",
        );
        assert!(matches!(e, ContinuousError::Unsupported(_)));
        let e = classify_error(reqwest::StatusCode::INTERNAL_SERVER_ERROR, "boom");
        assert!(matches!(e, ContinuousError::Failed(_)));
    }

    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    fn reply(content: &str) -> ResponseTemplate {
        ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "choices": [{"message": {"content": content}}],
            "usage": {"prompt_tokens": 1}
        }))
    }

    async fn bodies(server: &MockServer) -> Vec<serde_json::Value> {
        server
            .received_requests()
            .await
            .unwrap()
            .iter()
            .filter(|r| r.url.path() == "/v1/chat/completions")
            .map(|r| serde_json::from_slice(&r.body).unwrap())
            .collect()
    }

    const TICK: Duration = Duration::from_secs(2);

    #[tokio::test]
    async fn settled_text_is_forced_as_prefix() {
        let server = MockServer::start().await;
        // Two agreeing detections on 2 s of audio pin the language.
        Mock::given(path("/v1/chat/completions"))
            .respond_with(reply("language English<asr_text>Hello there friend"))
            .up_to_n_times(2)
            .mount(&server)
            .await;
        Mock::given(path("/v1/chat/completions"))
            .respond_with(reply(" my friend."))
            .mount(&server)
            .await;
        let mut d = ContinuousDecoder::new(&server.uri(), None, Some("Qwen/Qwen3-ASR-1.7B".into()))
            .with_rollback_words(1);
        let pcm = vec![0i16; MIN_PIN_SAMPLES];
        for _ in 0..2 {
            assert_eq!(
                d.step(&pcm, false, TICK).await.unwrap(),
                "Hello there friend"
            );
        }
        assert_eq!(
            d.step(&pcm, false, TICK).await.unwrap(),
            "Hello there my friend."
        );
        let sent = bodies(&server).await;
        assert_eq!(sent[0]["messages"][1]["content"], "");
        assert_eq!(sent[1]["messages"][1]["content"], "");
        assert_eq!(
            sent[2]["messages"][1]["content"],
            "language English<asr_text>Hello there"
        );
        assert_eq!(sent[2]["continue_final_message"], true);
        assert!(sent[2]["chat_template"]
            .as_str()
            .unwrap()
            .contains("audio_pad"));
    }

    #[tokio::test]
    async fn audio_goes_as_encoder_windows() {
        let server = MockServer::start().await;
        Mock::given(path("/v1/chat/completions"))
            .respond_with(reply("hi"))
            .mount(&server)
            .await;
        let mut d = ContinuousDecoder::new(&server.uri(), None, Some("Qwen/Qwen3-ASR-1.7B".into()))
            .with_language(Some("en"));
        let mut pcm: Vec<i16> = (0..ENCODER_WINDOW + SAMPLE_RATE)
            .map(|i| i as i16)
            .collect();
        d.step(&pcm, false, TICK).await.unwrap();
        pcm.extend(std::iter::repeat_n(7, SAMPLE_RATE));
        d.step(&pcm, false, TICK).await.unwrap();
        let sent = bodies(&server).await;
        let items = |b: &serde_json::Value| b["messages"][0]["content"].as_array().unwrap().clone();
        assert_eq!(items(&sent[0]).len(), 2);
        assert_eq!(items(&sent[1]).len(), 2);
        // The closed window is sent byte-identical so the server cache hits.
        assert_eq!(items(&sent[0])[0], items(&sent[1])[0]);
        assert_ne!(items(&sent[0])[1], items(&sent[1])[1]);
        let first = items(&sent[0])[0]["input_audio"]["data"]
            .as_str()
            .unwrap()
            .to_string();
        let wav = BASE64.decode(first).unwrap();
        assert_eq!(wav.len(), 44 + 2 * ENCODER_WINDOW);
    }

    #[tokio::test]
    async fn final_step_keeps_every_word_open() {
        let server = MockServer::start().await;
        Mock::given(path("/v1/chat/completions"))
            .respond_with(reply(" a b c"))
            .mount(&server)
            .await;
        let mut d = ContinuousDecoder::new(&server.uri(), None, Some("Qwen/Qwen3-ASR-1.7B".into()))
            .with_language(Some("en"))
            .resume(DecoderState {
                stable: "x y".into(),
                ..DecoderState::default()
            });
        let pcm = vec![0i16; SAMPLE_RATE];
        assert_eq!(d.step(&pcm, true, TICK).await.unwrap(), "x y a b c");
        // A final tick does not settle anything further.
        assert_eq!(d.snapshot().stable, "x y");
    }

    #[tokio::test]
    async fn short_continuation_cannot_unfreeze_the_prefix() {
        let server = MockServer::start().await;
        Mock::given(path("/v1/chat/completions"))
            .respond_with(reply(" new"))
            .mount(&server)
            .await;
        let mut d = ContinuousDecoder::new(&server.uri(), None, Some("Qwen/Qwen3-ASR-1.7B".into()))
            .with_language(Some("en"))
            .resume(DecoderState {
                stable: "one two three four".into(),
                ..Default::default()
            });
        let pcm = vec![0i16; SAMPLE_RATE];
        for _ in 0..2 {
            assert_eq!(
                d.step(&pcm, false, TICK).await.unwrap(),
                "one two three four new"
            );
            assert_eq!(d.snapshot().stable, "one two three four");
        }
        assert_eq!(
            bodies(&server).await[1]["messages"][1]["content"],
            "language English<asr_text>one two three four"
        );
    }

    #[tokio::test]
    async fn refused_template_reports_unsupported() {
        let server = MockServer::start().await;
        Mock::given(path("/v1/chat/completions"))
            .respond_with(ResponseTemplate::new(400).set_body_string(
                "Chat template is passed with request, but --trust-request-chat-template is not set",
            ))
            .mount(&server)
            .await;
        let mut d = ContinuousDecoder::new(&server.uri(), None, Some("Qwen/Qwen3-ASR-1.7B".into()));
        let err = d
            .step(&vec![0i16; SAMPLE_RATE], false, TICK)
            .await
            .unwrap_err();
        assert!(matches!(err, ContinuousError::Unsupported(_)));
    }

    #[tokio::test]
    async fn other_models_are_unsupported() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/v1/models"))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_body_json(serde_json::json!({"data": [{"id": "whisper-large-v3"}]})),
            )
            .mount(&server)
            .await;
        let mut d = ContinuousDecoder::new(&server.uri(), None, None);
        let err = d
            .step(&vec![0i16; SAMPLE_RATE], false, TICK)
            .await
            .unwrap_err();
        assert!(matches!(err, ContinuousError::Unsupported(_)));
    }

    #[tokio::test]
    async fn configured_non_qwen_model_is_unsupported_without_a_request() {
        let server = MockServer::start().await;
        let mut d = ContinuousDecoder::new(&server.uri(), None, Some("whisper-1".into()));
        let err = d
            .step(&vec![0i16; SAMPLE_RATE], false, TICK)
            .await
            .unwrap_err();
        assert!(matches!(err, ContinuousError::Unsupported(_)));
        assert!(server.received_requests().await.unwrap().is_empty());
    }

    #[tokio::test]
    async fn context_goes_in_the_system_turn() {
        let server = MockServer::start().await;
        Mock::given(path("/v1/chat/completions"))
            .respond_with(reply("hi"))
            .mount(&server)
            .await;
        let mut d = ContinuousDecoder::new(&server.uri(), None, Some("Qwen/Qwen3-ASR-1.7B".into()))
            .with_language(Some("en"))
            .with_context(Some("vLLM, Hyprland".into()));
        d.step(&vec![0i16; SAMPLE_RATE], false, TICK).await.unwrap();
        let sent = bodies(&server).await;
        assert_eq!(sent[0]["messages"][0]["role"], "system");
        assert_eq!(sent[0]["messages"][0]["content"], "vLLM, Hyprland");
        // Blank context sends no system turn.
        let d = ContinuousDecoder::new("http://x", None, None).with_context(Some("  ".into()));
        assert!(d.context.is_none());
    }

    #[tokio::test]
    async fn server_without_model_list_is_unsupported() {
        let server = MockServer::start().await;
        Mock::given(path("/v1/models"))
            .respond_with(ResponseTemplate::new(404))
            .mount(&server)
            .await;
        let mut d = ContinuousDecoder::new(&server.uri(), None, None);
        let err = d
            .step(&vec![0i16; SAMPLE_RATE], false, TICK)
            .await
            .unwrap_err();
        assert!(matches!(err, ContinuousError::Unsupported(_)));
    }

    #[tokio::test]
    async fn malformed_success_responses_preserve_decoder_state() {
        for body in [
            serde_json::json!({}),
            serde_json::json!({"choices": []}),
            serde_json::json!({"choices": [{"message": {"content": null}}]}),
            serde_json::json!({"choices": [{"message": {"content": 42}}]}),
        ] {
            let server = MockServer::start().await;
            Mock::given(path("/v1/chat/completions"))
                .respond_with(ResponseTemplate::new(200).set_body_json(body))
                .mount(&server)
                .await;
            for stable in ["", "already frozen words"] {
                let mut decoder =
                    ContinuousDecoder::new(&server.uri(), None, Some("Qwen/Qwen3-ASR-1.7B".into()))
                        .with_language(Some("en"))
                        .resume(DecoderState {
                            stable: stable.into(),
                            ..Default::default()
                        });
                let before = decoder.snapshot();
                for last in [false, true] {
                    let error = decoder
                        .step(&vec![0i16; SAMPLE_RATE], last, TICK)
                        .await
                        .unwrap_err();
                    assert!(matches!(error, ContinuousError::Failed(_)));
                    assert_eq!(decoder.snapshot(), before);
                }
            }
        }
    }

    #[tokio::test]
    async fn runaway_decode_is_rejected() {
        let server = MockServer::start().await;
        Mock::given(path("/v1/chat/completions"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "choices": [{"message": {"content": " la la la"}, "finish_reason": "length"}]
            })))
            .mount(&server)
            .await;
        let mut d = ContinuousDecoder::new(&server.uri(), None, Some("Qwen/Qwen3-ASR-1.7B".into()))
            .with_language(Some("en"));
        let err = d
            .step(&vec![0i16; SAMPLE_RATE], false, TICK)
            .await
            .unwrap_err();
        assert!(matches!(err, ContinuousError::Failed(_)));
    }

    #[tokio::test]
    async fn final_that_drops_the_open_words_fails() {
        let server = MockServer::start().await;
        Mock::given(path("/v1/chat/completions"))
            .respond_with(reply(""))
            .mount(&server)
            .await;
        let mut d = ContinuousDecoder::new(&server.uri(), None, Some("Qwen/Qwen3-ASR-1.7B".into()))
            .with_language(Some("en"))
            .resume(DecoderState {
                stable: "a b c d e".into(),
                ..DecoderState::default()
            });
        let pcm = vec![0i16; SAMPLE_RATE];
        // A preview tick may legitimately add nothing...
        assert_eq!(d.step(&pcm, false, TICK).await.unwrap(), "a b c d e");
        // ...but the final must re-decode the open words.
        let err = d.step(&pcm, true, TICK).await.unwrap_err();
        assert!(matches!(err, ContinuousError::Failed(_)));
    }

    #[tokio::test]
    async fn silence_does_not_pin_the_language() {
        let server = MockServer::start().await;
        Mock::given(path("/v1/chat/completions"))
            .respond_with(reply("language None<asr_text>"))
            .up_to_n_times(1)
            .mount(&server)
            .await;
        Mock::given(path("/v1/chat/completions"))
            .respond_with(reply("language Norwegian<asr_text>Hei der"))
            .mount(&server)
            .await;
        let mut d = ContinuousDecoder::new(&server.uri(), None, Some("Qwen/Qwen3-ASR-1.7B".into()));
        let pcm = vec![0i16; MIN_PIN_SAMPLES];
        assert_eq!(d.step(&pcm, false, TICK).await.unwrap(), "");
        assert_eq!(d.snapshot().header, None);
        assert_eq!(d.step(&pcm, false, TICK).await.unwrap(), "Hei der");
        assert_eq!(d.snapshot().header, None, "one detection is not enough");
        d.step(&pcm, false, TICK).await.unwrap();
        assert_eq!(
            d.snapshot().header.as_deref(),
            Some("language Norwegian<asr_text>")
        );
    }

    #[tokio::test]
    async fn early_misdetection_is_not_pinned() {
        // #153: the first fraction of a second came back as Arabic.
        let server = MockServer::start().await;
        Mock::given(path("/v1/chat/completions"))
            .respond_with(reply("language Arabic<asr_text>هذا"))
            .up_to_n_times(1)
            .mount(&server)
            .await;
        Mock::given(path("/v1/chat/completions"))
            .respond_with(reply("language English<asr_text>This is a longer test"))
            .mount(&server)
            .await;
        let mut d = ContinuousDecoder::new(&server.uri(), None, Some("Qwen/Qwen3-ASR-1.7B".into()));
        d.step(&vec![0i16; SAMPLE_RATE / 2], false, TICK)
            .await
            .unwrap();
        assert_eq!(d.snapshot().header, None, "too little audio to pin");
        let long = vec![0i16; MIN_PIN_SAMPLES];
        d.step(&long, false, TICK).await.unwrap();
        assert_eq!(d.snapshot().header, None, "disagrees with the last decode");
        assert_eq!(
            d.step(&long, false, TICK).await.unwrap(),
            "This is a longer test"
        );
        assert_eq!(
            d.snapshot().header.as_deref(),
            Some("language English<asr_text>")
        );
        // Settling starts only once the language is pinned.
        assert_eq!(d.snapshot().stable, "This is");
    }

    #[tokio::test]
    async fn deadline_covers_the_whole_step() {
        let server = MockServer::start().await;
        Mock::given(path("/v1/models"))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_body_json(serde_json::json!({"data": [{"id": "Qwen/Qwen3-ASR-1.7B"}]}))
                    .set_delay(Duration::from_millis(300)),
            )
            .mount(&server)
            .await;
        Mock::given(path("/v1/chat/completions"))
            .respond_with(reply("hi").set_delay(Duration::from_millis(300)))
            .mount(&server)
            .await;
        let mut d = ContinuousDecoder::new(&server.uri(), None, None);
        let started = std::time::Instant::now();
        let err = d
            .step(&vec![0i16; SAMPLE_RATE], false, Duration::from_millis(450))
            .await
            .unwrap_err();
        assert!(matches!(err, ContinuousError::Failed(_)));
        assert!(started.elapsed() < Duration::from_millis(550));
    }

    #[tokio::test]
    async fn short_tail_is_left_out() {
        let server = MockServer::start().await;
        let mut d = ContinuousDecoder::new(&server.uri(), None, Some("Qwen/Qwen3-ASR-1.7B".into()));
        // Nothing worth sending: no request at all.
        assert_eq!(d.step(&[0i16; 100], false, TICK).await.unwrap(), "");
        assert!(bodies(&server).await.is_empty());
    }

    /// Live check against a real server:
    /// `EARS_CONTINUOUS_SERVER=http://localhost:30189 EARS_CONTINUOUS_WAV=clip.wav
    ///  cargo test --lib continuous::tests::live -- --ignored --nocapture`
    #[tokio::test]
    #[ignore]
    async fn live_ticks_match_a_final_pass() {
        let server = std::env::var("EARS_CONTINUOUS_SERVER").unwrap();
        let bytes = std::fs::read(std::env::var("EARS_CONTINUOUS_WAV").unwrap()).unwrap();
        let pcm = samples(crate::ghost::growing_wav_payload(&bytes).unwrap());
        let mut decoder = ContinuousDecoder::new(&server, None, None).with_language(Some("en"));
        let tick = SAMPLE_RATE * 3 / 10;
        let mut end = tick;
        let started = std::time::Instant::now();
        while end < pcm.len() {
            let t = std::time::Instant::now();
            let text = decoder
                .step(&pcm[..end], false, Duration::from_secs(4))
                .await
                .unwrap();
            println!(
                "{:5.1}s {:4}ms | {}",
                end as f64 / 16000.0,
                t.elapsed().as_millis(),
                text
            );
            end += tick;
        }
        let t = std::time::Instant::now();
        let last = decoder
            .step(&pcm, true, Duration::from_secs(4))
            .await
            .unwrap();
        println!(
            "final {}ms (total {:?}): {}",
            t.elapsed().as_millis(),
            started.elapsed(),
            last
        );
        let mut fresh = ContinuousDecoder::new(&server, None, None).with_language(Some("en"));
        let reference = fresh
            .step(&pcm, true, Duration::from_secs(10))
            .await
            .unwrap();
        assert_eq!(last, reference);
    }
}
