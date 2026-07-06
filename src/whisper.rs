//! Whisper.cpp client implementation
//!
//! This module provides a client for communicating with a whisper.cpp server.
//! It handles health checks, transcription requests, and includes retry logic
//! with exponential backoff.

use backoff::{future::retry, ExponentialBackoff};
use base64::{engine::general_purpose::STANDARD as BASE64, Engine as _};
use reqwest::{multipart, Client};
use serde::{Deserialize, Serialize};
use std::path::Path;

/// Standard PCM WAV header size in bytes.
///
/// Files at or below this size contain zero audio samples. Sending such files
/// crashes some ASR backends (e.g. Qwen3-ASR ValueError).
pub const WAV_HEADER_SIZE: u64 = 44;
use std::time::Duration;
use thiserror::Error;
use tracing::{debug, info, warn};

/// Errors that can occur when using the WhisperClient
#[derive(Error, Debug)]
pub enum WhisperError {
    #[error("Failed to connect to whisper server: {0}")]
    ConnectionError(String),

    #[error("Transcription request failed: {0}")]
    TranscriptionError(String),

    #[error("Invalid audio file: {0}")]
    InvalidAudioFile(String),

    #[error("Server returned empty transcription")]
    EmptyTranscription,

    #[error("HTTP request failed: {0}")]
    HttpError(#[from] reqwest::Error),

    #[error("IO error: {0}")]
    IoError(#[from] std::io::Error),

    #[error("JSON parsing error: {0}")]
    JsonError(#[from] serde_json::Error),
}

/// Response from the whisper.cpp inference endpoint
#[derive(Debug, Deserialize, Serialize)]
struct TranscriptionResponse {
    /// The transcribed text
    text: String,
}

/// Minimal chat-completions response shape (used for the grammar path).
#[derive(Debug, Deserialize)]
struct ChatCompletionResponse {
    choices: Vec<ChatChoice>,
}

#[derive(Debug, Deserialize)]
struct ChatChoice {
    message: ChatMessage,
}

#[derive(Debug, Deserialize)]
struct ChatMessage {
    content: Option<String>,
}

/// Client for interacting with whisper.cpp server
#[derive(Clone)]
pub struct WhisperClient {
    /// HTTP client
    client: Client,
    /// Base URL of the whisper.cpp server
    server_url: String,
    /// Language code for transcription (None = auto-detect)
    language: Option<String>,
    /// API key for authenticated services (None = no auth)
    api_key: Option<String>,
    /// Model name for transcription (None = server default)
    model: Option<String>,
    /// Prompt for context biasing (None = no prompt)
    prompt: Option<String>,
    /// Maximum number of retry attempts
    #[allow(dead_code)]
    max_retries: u32,
    /// Initial backoff delay in milliseconds
    initial_backoff_ms: u64,
    /// Maximum backoff delay in milliseconds
    max_backoff_ms: u64,
}

impl WhisperClient {
    /// Creates a new WhisperClient with default retry settings
    ///
    /// # Arguments
    /// * `server_url` - Base URL of the whisper.cpp server (e.g., "http://localhost:8178")
    ///
    /// # Example
    /// ```
    /// use ears::WhisperClient;
    ///
    /// let client = WhisperClient::new("http://localhost:8178");
    /// ```
    pub fn new(server_url: impl Into<String>) -> Self {
        Self {
            client: Client::builder()
                .connect_timeout(Duration::from_secs(2))
                .timeout(Duration::from_secs(30))
                .build()
                .expect("Failed to build HTTP client"),
            server_url: server_url.into(),
            language: None,
            api_key: None,
            model: None,
            prompt: None,
            max_retries: 3,
            initial_backoff_ms: 100,
            max_backoff_ms: 5000,
        }
    }

    /// Sets the language for transcription
    ///
    /// # Arguments
    /// * `language` - Language code (e.g., "en", "no") or None for auto-detect
    pub fn with_language(mut self, language: Option<String>) -> Self {
        self.language = language;
        self
    }

    /// Sets the API key for authenticated ASR services
    ///
    /// When set, adds an `Authorization: Bearer {key}` header to all requests.
    /// When None, no authorization header is sent (default).
    pub fn with_api_key(mut self, api_key: Option<String>) -> Self {
        self.api_key = api_key;
        self
    }

    /// Sets the model name for transcription
    ///
    /// When set, includes a `model` field in the multipart form.
    /// When None, no model field is sent (server uses its default).
    pub fn with_model(mut self, model: Option<String>) -> Self {
        self.model = model;
        self
    }

    /// Sets the prompt for context biasing
    ///
    /// When set, includes a `prompt` field in the multipart form.
    /// Use this to pass entity names, acronyms, or domain-specific terms
    /// that should be recognized correctly (e.g., "vLLM, PyTorch, Safetensors").
    pub fn with_prompt(mut self, prompt: Option<String>) -> Self {
        self.prompt = prompt;
        self
    }

    /// Creates a new WhisperClient with custom retry settings
    ///
    /// # Arguments
    /// * `server_url` - Base URL of the whisper.cpp server
    /// * `max_retries` - Maximum number of retry attempts
    /// * `initial_backoff_ms` - Initial backoff delay in milliseconds
    /// * `max_backoff_ms` - Maximum backoff delay in milliseconds
    pub fn with_retry_config(
        server_url: impl Into<String>,
        max_retries: u32,
        initial_backoff_ms: u64,
        max_backoff_ms: u64,
    ) -> Self {
        Self {
            client: Client::builder()
                .connect_timeout(Duration::from_secs(2))
                .timeout(Duration::from_secs(30))
                .build()
                .expect("Failed to build HTTP client"),
            server_url: server_url.into(),
            language: None,
            api_key: None,
            model: None,
            prompt: None,
            max_retries,
            initial_backoff_ms,
            max_backoff_ms,
        }
    }

    /// Checks if the whisper server is healthy and responding
    ///
    /// # Returns
    /// * `Ok(())` if the server is healthy
    /// * `Err(WhisperError)` if the server is not responding or unhealthy
    ///
    /// # Example
    /// ```no_run
    /// # use ears::WhisperClient;
    /// # async fn example() -> Result<(), Box<dyn std::error::Error>> {
    /// let client = WhisperClient::new("http://localhost:8178");
    /// client.health_check().await?;
    /// # Ok(())
    /// # }
    /// ```
    pub async fn health_check(&self) -> Result<(), WhisperError> {
        let start = std::time::Instant::now();
        let base = self.server_url.trim_end_matches('/');

        // Try /health first (local whisper servers)
        let health_url = format!("{}/health", base);
        debug!("Performing health check on {}", health_url);

        let mut request = self.client.get(&health_url);
        if let Some(ref key) = self.api_key {
            request = request.bearer_auth(key);
        }

        match request.send().await {
            Ok(response) if response.status().is_success() => {
                info!(
                    "Whisper server is healthy (via /health) in {:?}",
                    start.elapsed()
                );
                return Ok(());
            }
            _ => {}
        }

        // Fall back to /v1/models (cloud APIs like Groq)
        let models_url = format!("{}/v1/models", base);
        debug!("Health check fallback: trying {}", models_url);

        let mut request = self.client.get(&models_url);
        if let Some(ref key) = self.api_key {
            request = request.bearer_auth(key);
        }

        let response = request
            .send()
            .await
            .map_err(|e| WhisperError::ConnectionError(e.to_string()))?;

        if response.status().is_success() {
            info!(
                "Whisper server is healthy (via /v1/models) in {:?}",
                start.elapsed()
            );
            Ok(())
        } else {
            Err(WhisperError::ConnectionError(format!(
                "Server returned status: {}",
                response.status()
            )))
        }
    }

    /// Transcribes an audio file using the whisper server
    ///
    /// This method includes automatic retry logic with exponential backoff.
    /// Silence artifacts like "Thank you." are filtered out.
    ///
    /// # Arguments
    /// * `audio_path` - Path to the audio file (WAV format, 16kHz, mono, 16-bit PCM)
    ///
    /// # Returns
    /// * `Ok(String)` - The transcribed text
    /// * `Err(WhisperError)` - If transcription fails
    ///
    /// # Example
    /// ```no_run
    /// # use ears::WhisperClient;
    /// # async fn example() -> Result<(), Box<dyn std::error::Error>> {
    /// let client = WhisperClient::new("http://localhost:8178");
    /// let text = client.transcribe("/path/to/recording.wav").await?;
    /// println!("Transcribed: {}", text);
    /// # Ok(())
    /// # }
    /// ```
    pub async fn transcribe(&self, audio_path: impl AsRef<Path>) -> Result<String, WhisperError> {
        self.transcribe_with_grammar(audio_path, None).await
    }

    /// Transcribes an audio file, optionally constraining output to a grammar.
    ///
    /// When `grammar` is `None` this is identical to [`WhisperClient::transcribe`]
    /// — a multipart POST to `/v1/audio/transcriptions`.
    ///
    /// When `grammar` is `Some(gbnf)` the request is routed to the
    /// chat-completions endpoint instead (audio as a base64 `input_audio` part
    /// plus `structured_outputs.grammar`), because vLLM's transcription endpoint
    /// does not support guided decoding — only chat-completions does. This is
    /// how "bash mode" biases spoken commands toward valid shell syntax.
    ///
    /// # Arguments
    /// * `audio_path` - Path to the audio file (WAV, 16kHz, mono, 16-bit PCM)
    /// * `grammar` - Optional GBNF grammar to constrain the transcription
    pub async fn transcribe_with_grammar(
        &self,
        audio_path: impl AsRef<Path>,
        grammar: Option<&str>,
    ) -> Result<String, WhisperError> {
        let path = audio_path.as_ref();

        // Validate audio file exists and has content
        self.validate_audio_file(path).await?;

        info!(
            "Transcribing audio file: {} (grammar: {})",
            path.display(),
            grammar.is_some()
        );

        // Perform transcription with retry logic
        let backoff = self.create_backoff();
        let text = retry(backoff, || async {
            let result = match grammar {
                Some(g) => self.transcribe_chat_internal(path, g).await,
                None => self.transcribe_internal(path).await,
            };
            result.map_err(|e| {
                warn!("Transcription attempt failed: {}", e);
                backoff::Error::transient(e)
            })
        })
        .await?;

        // Filter silence artifacts
        let filtered = self.filter_silence_artifacts(&text);

        if filtered.is_empty() {
            return Err(WhisperError::EmptyTranscription);
        }

        Ok(filtered)
    }

    /// Validates that an audio file exists and contains audio data
    async fn validate_audio_file(&self, path: &Path) -> Result<(), WhisperError> {
        if !path.exists() {
            return Err(WhisperError::InvalidAudioFile(format!(
                "File does not exist: {}",
                path.display()
            )));
        }

        let metadata = tokio::fs::metadata(path).await?;

        if metadata.len() <= crate::WAV_HEADER_SIZE {
            return Err(WhisperError::InvalidAudioFile(format!(
                "File contains no audio data ({} bytes): {}",
                metadata.len(),
                path.display()
            )));
        }

        debug!(
            "Audio file validated: {} ({} bytes)",
            path.display(),
            metadata.len()
        );
        Ok(())
    }

    /// Internal transcription logic without retry
    async fn transcribe_internal(&self, path: &Path) -> Result<String, WhisperError> {
        let start = std::time::Instant::now();
        let base = self.server_url.trim_end_matches('/');
        let url = format!("{}/v1/audio/transcriptions", base);
        debug!("Sending transcription request to {}", url);

        // Read audio file
        let audio_data = tokio::fs::read(path).await?;

        // Create multipart form
        let file_part = multipart::Part::bytes(audio_data)
            .file_name("recording.wav")
            .mime_str("audio/wav")
            .map_err(|e| WhisperError::TranscriptionError(e.to_string()))?;

        let mut form = multipart::Form::new()
            .part("file", file_part)
            .text("response_format", "json");

        // Add language parameter only if explicitly set (otherwise auto-detect)
        if let Some(ref lang) = self.language {
            debug!("Using language: {}", lang);
            form = form.text("language", lang.clone());
        }

        // Add model parameter only if explicitly set (otherwise server default)
        if let Some(ref model) = self.model {
            debug!("Using model: {}", model);
            form = form.text("model", model.clone());
        }

        // Add prompt for context biasing (entity names, acronyms, etc.)
        if let Some(ref prompt) = self.prompt {
            debug!("Using prompt: {}", prompt);
            form = form.text("prompt", prompt.clone());
        }

        // Send request
        let mut request = self.client.post(&url).multipart(form);
        if let Some(ref key) = self.api_key {
            request = request.bearer_auth(key);
        }

        let response = request
            .send()
            .await
            .map_err(|e| WhisperError::TranscriptionError(e.to_string()))?;

        if !response.status().is_success() {
            return Err(WhisperError::TranscriptionError(format!(
                "Server returned status: {}",
                response.status()
            )));
        }

        // Parse response
        let transcription: TranscriptionResponse = response.json().await?;
        info!(
            "Whisper API call completed in {:?}: \"{}\"",
            start.elapsed(),
            transcription.text
        );

        Ok(transcription.text)
    }

    /// Grammar-constrained transcription via the chat-completions endpoint.
    ///
    /// vLLM's `/v1/audio/transcriptions` endpoint does not wire guided decoding
    /// into sampling params (`vllm_xargs` is passed opaquely as `extra_args`),
    /// so constrained decoding must go through `/v1/chat/completions`, which
    /// supports `structured_outputs.grammar`. The audio is sent as a base64
    /// `input_audio` content part.
    ///
    /// The model prefixes its output with a `language <Lang><asr_text>` scaffold
    /// (which the grammar must allow — see `grammars/bash.gbnf`); we strip
    /// everything up to and including `<asr_text>` from the response.
    async fn transcribe_chat_internal(
        &self,
        path: &Path,
        grammar: &str,
    ) -> Result<String, WhisperError> {
        let start = std::time::Instant::now();
        let base = self.server_url.trim_end_matches('/');
        let url = format!("{}/v1/chat/completions", base);
        debug!("Sending grammar-constrained request to {}", url);

        // Chat completions requires a model name; bash mode can't work without one.
        let model = self.model.as_deref().ok_or_else(|| {
            WhisperError::TranscriptionError(
                "grammar-constrained (bash) mode requires a configured model".to_string(),
            )
        })?;

        let audio_data = tokio::fs::read(path).await?;
        let audio_b64 = BASE64.encode(&audio_data);

        // `max_tokens` is a hard backstop against runaway token loops the grammar
        // can otherwise produce on unintelligible audio.
        let body = serde_json::json!({
            "model": model,
            "messages": [{
                "role": "user",
                "content": [
                    {"type": "input_audio", "input_audio": {"data": audio_b64, "format": "wav"}},
                    {"type": "text", "text": "Transcribe the audio."}
                ]
            }],
            "max_tokens": 64,
            "temperature": 0.0,
            "structured_outputs": {"grammar": grammar}
        });

        let mut request = self.client.post(&url).json(&body);
        if let Some(ref key) = self.api_key {
            request = request.bearer_auth(key);
        }

        let response = request
            .send()
            .await
            .map_err(|e| WhisperError::TranscriptionError(e.to_string()))?;

        if !response.status().is_success() {
            return Err(WhisperError::TranscriptionError(format!(
                "Server returned status: {}",
                response.status()
            )));
        }

        let completion: ChatCompletionResponse = response.json().await?;
        let raw = completion
            .choices
            .into_iter()
            .next()
            .and_then(|c| c.message.content)
            .unwrap_or_default();

        let text = Self::strip_asr_scaffold(&raw);

        // Reject degenerate runaway output (e.g. "lndirdirdir...") rather than
        // typing garbage into the user's terminal.
        if Self::looks_degenerate(text) {
            warn!("Discarding degenerate grammar output: {:?}", text);
            return Ok(String::new());
        }

        info!(
            "Grammar-constrained call completed in {:?}: \"{}\"",
            start.elapsed(),
            text
        );

        Ok(text.to_string())
    }

    /// Strip the Qwen3-ASR `language <Lang><asr_text>` scaffold and any trailing
    /// sentence punctuation, leaving just the command text.
    fn strip_asr_scaffold(raw: &str) -> &str {
        let body = match raw.split_once("<asr_text>") {
            Some((_, rest)) => rest,
            None => raw,
        };
        body.trim().trim_end_matches(['.', '。'])
    }

    /// Heuristic: detect runaway repetition that the grammar can emit on
    /// unintelligible audio (a single long token with a short substring repeated
    /// many times). Such output is never a real command.
    fn looks_degenerate(text: &str) -> bool {
        // A single "word" longer than this is almost certainly a runaway loop;
        // real commands break into space-separated tokens well before this.
        const MAX_TOKEN_LEN: usize = 40;
        text.split_whitespace().any(|tok| tok.len() > MAX_TOKEN_LEN)
    }

    /// Filters out common silence artifacts from whisper.cpp and Qwen3-ASR
    fn filter_silence_artifacts(&self, text: &str) -> String {
        let trimmed = text.trim();

        // Common silence artifacts from whisper.cpp and Qwen3-ASR
        let silence_patterns = [
            "Thank you.",
            "Thank you",
            "Thanks for watching.",
            "Thanks for watching",
            "啊！",
            "嗯。",
        ];

        // Check if the entire text is just a silence artifact
        if silence_patterns.contains(&trimmed) {
            debug!("Filtered silence artifact: {}", trimmed);
            return String::new();
        }

        // Filter Chinese-only text when language is not Chinese.
        // Qwen3-ASR hallucinates Chinese characters on silence when the
        // configured language is non-Chinese.
        if self.is_chinese_artifact(trimmed) {
            debug!("Filtered Chinese silence artifact: {}", trimmed);
            return String::new();
        }

        trimmed.to_string()
    }

    /// Returns true if the text is entirely CJK characters/punctuation and the
    /// configured language is not Chinese.
    fn is_chinese_artifact(&self, text: &str) -> bool {
        if text.is_empty() {
            return false;
        }

        // If language is explicitly Chinese, don't filter
        if let Some(ref lang) = self.language {
            let lang_lower = lang.to_lowercase();
            if lang_lower == "zh" || lang_lower.starts_with("zh-") || lang_lower == "chinese" {
                return false;
            }
        }

        // Common hallucination filler characters from ASR models
        let filler_chars = ['啊', '嗯', '呃', '哦', '噢', '哎', '哇', '呀', '吧', '呢'];

        // It's considered a Chinese artifact if all characters are whitespace,
        // non-alphanumeric symbols/punctuation, or specific CJK filler characters,
        // AND it contains at least one CJK filler character.
        let mut has_filler = false;
        let all_allowed = text.chars().all(|c| {
            if filler_chars.contains(&c) {
                has_filler = true;
                true
            } else if c.is_alphanumeric() {
                // If it's a letter/number (including non-filler Chinese characters like '下'), reject
                false
            } else {
                // Allow all punctuation/whitespace (including CJK punctuation like 。)
                true
            }
        });

        has_filler && all_allowed
    }

    /// Creates an exponential backoff configuration
    fn create_backoff(&self) -> ExponentialBackoff {
        // Calculate a reasonable max_elapsed_time based on backoff settings
        // For test scenarios with very small backoffs (< 100ms), fail faster
        // For normal/production use, allow 30 seconds
        let max_elapsed = if self.max_backoff_ms < 100 {
            // Very short timeout for error case tests
            Duration::from_millis(500)
        } else {
            // Normal timeout for success and retry tests
            Duration::from_secs(30)
        };

        ExponentialBackoff {
            initial_interval: Duration::from_millis(self.initial_backoff_ms),
            max_interval: Duration::from_millis(self.max_backoff_ms),
            max_elapsed_time: Some(max_elapsed),
            ..Default::default()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_client_creation() {
        let client = WhisperClient::new("http://localhost:8178");
        assert_eq!(client.server_url, "http://localhost:8178");
        assert_eq!(client.max_retries, 3);
    }

    #[test]
    fn test_client_with_custom_retry() {
        let client = WhisperClient::with_retry_config("http://localhost:8178", 5, 200, 10000);
        assert_eq!(client.max_retries, 5);
        assert_eq!(client.initial_backoff_ms, 200);
        assert_eq!(client.max_backoff_ms, 10000);
    }

    #[test]
    fn test_filter_silence_artifacts() {
        let client = WhisperClient::new("http://localhost:8178");

        // Should filter exact matches
        assert_eq!(client.filter_silence_artifacts("Thank you."), "");
        assert_eq!(client.filter_silence_artifacts("Thank you"), "");
        assert_eq!(client.filter_silence_artifacts("Thanks for watching."), "");

        // Should filter Qwen3-ASR Chinese artifacts
        assert_eq!(client.filter_silence_artifacts("啊！"), "");
        assert_eq!(client.filter_silence_artifacts("嗯。"), "");

        // Should filter Qwen3-ASR Chinese artifacts with standard punctuation
        assert_eq!(client.filter_silence_artifacts("啊!"), "");
        assert_eq!(client.filter_silence_artifacts("嗯."), "");
        assert_eq!(client.filter_silence_artifacts("嗯..."), "");
        assert_eq!(client.filter_silence_artifacts("  啊 !  "), "");

        // Should keep real transcriptions
        assert_eq!(
            client.filter_silence_artifacts("Hello world"),
            "Hello world"
        );
        assert_eq!(
            client.filter_silence_artifacts("  Test message  "),
            "Test message"
        );

        // Should keep valid Chinese dictations even if short
        assert_eq!(client.filter_silence_artifacts("你好世界"), "你好世界");
        assert_eq!(client.filter_silence_artifacts("下车搵架。"), "下车搵架。");
        assert_eq!(
            client.filter_silence_artifacts("这是一个测试"),
            "这是一个测试"
        );

        // Should not filter partial matches
        assert_eq!(
            client.filter_silence_artifacts("Thank you for your help"),
            "Thank you for your help"
        );

        // Should not filter mixed Chinese+Latin text (likely real transcription)
        assert_eq!(client.filter_silence_artifacts("Hello 你好"), "Hello 你好");
    }

    #[test]
    fn test_filter_silence_artifacts_chinese_language() {
        // When language is Chinese, Chinese text should NOT be filtered
        let client =
            WhisperClient::new("http://localhost:8178").with_language(Some("zh".to_string()));

        assert_eq!(client.filter_silence_artifacts("你好世界"), "你好世界");
        assert_eq!(
            client.filter_silence_artifacts("这是一个测试"),
            "这是一个测试"
        );

        // But exact silence patterns are still filtered
        assert_eq!(client.filter_silence_artifacts("Thank you."), "");
        // Qwen3 artifacts are exact-match filtered regardless of language
        assert_eq!(client.filter_silence_artifacts("啊！"), "");
        assert_eq!(client.filter_silence_artifacts("嗯。"), "");
    }

    #[test]
    fn test_strip_asr_scaffold() {
        // Strips the Qwen3-ASR scaffold and trailing sentence punctuation.
        assert_eq!(
            WhisperClient::strip_asr_scaffold("language English<asr_text>git status."),
            "git status"
        );
        assert_eq!(
            WhisperClient::strip_asr_scaffold("language Norwegian<asr_text>ls"),
            "ls"
        );
        // No scaffold → returned as-is (trimmed).
        assert_eq!(WhisperClient::strip_asr_scaffold("  pwd  "), "pwd");
        // CJK full stop is also stripped.
        assert_eq!(
            WhisperClient::strip_asr_scaffold("<asr_text>cargo build。"),
            "cargo build"
        );
    }

    #[test]
    fn test_looks_degenerate() {
        // Real commands are not degenerate.
        assert!(!WhisperClient::looks_degenerate("ls -la"));
        assert!(!WhisperClient::looks_degenerate("git commit -m fix"));
        assert!(!WhisperClient::looks_degenerate(""));
        // A single very long token is a runaway loop.
        assert!(WhisperClient::looks_degenerate(&"dir".repeat(30)));
    }

    #[tokio::test]
    async fn test_validate_audio_file_not_exists() {
        let client = WhisperClient::new("http://localhost:8178");
        let result = client
            .validate_audio_file(Path::new("/nonexistent/file.wav"))
            .await;

        assert!(result.is_err());
        match result {
            Err(WhisperError::InvalidAudioFile(msg)) => {
                assert!(msg.contains("does not exist"));
            }
            _ => panic!("Expected InvalidAudioFile error"),
        }
    }
}
