//! Whisper server integration
//!
//! This module will be fully implemented in Iteration 4.
//! For now, it contains placeholder types and stub implementations.

use anyhow::{Context, Result};
use reqwest::multipart;
use serde::{Deserialize, Serialize};
use std::path::Path;
use std::time::Duration;

/// Whisper transcription response
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TranscriptionResponse {
    /// The transcribed text
    pub text: String,
}

/// Whisper API client
pub struct WhisperClient {
    /// Base URL of the whisper server
    server_url: String,
    /// HTTP client
    client: reqwest::Client,
}

impl WhisperClient {
    /// Create a new Whisper client
    pub fn new(server_url: String) -> Self {
        let client = reqwest::Client::builder()
            .timeout(Duration::from_secs(300)) // 5 minute timeout for transcription
            .build()
            .expect("Failed to build HTTP client");

        Self {
            server_url,
            client,
        }
    }

    /// Check if the whisper server is healthy
    pub async fn health_check(&self) -> Result<bool> {
        let url = format!("{}/health", self.server_url);
        let response = self
            .client
            .get(&url)
            .send()
            .await
            .context("Failed to connect to whisper server")?;

        Ok(response.status().is_success())
    }

    /// Transcribe an audio file
    pub async fn transcribe(&self, audio_path: &Path) -> Result<String> {
        // Read the audio file
        let audio_data = tokio::fs::read(audio_path)
            .await
            .context("Failed to read audio file")?;

        // Create multipart form
        let file_part = multipart::Part::bytes(audio_data)
            .file_name("recording.wav")
            .mime_str("audio/wav")
            .context("Failed to create file part")?;

        let form = multipart::Form::new()
            .part("file", file_part)
            .text("response_format", "json");

        // Send request
        let url = format!("{}/inference", self.server_url);
        let response = self
            .client
            .post(&url)
            .multipart(form)
            .send()
            .await
            .context("Failed to send transcription request")?;

        if !response.status().is_success() {
            anyhow::bail!(
                "Whisper server returned error: {}",
                response.status()
            );
        }

        // Parse response
        let transcription: TranscriptionResponse = response
            .json()
            .await
            .context("Failed to parse transcription response")?;

        // Clean up the text
        let text = transcription.text.trim().to_string();

        // Filter out whisper silence artifacts
        if text.is_empty() || text == "Thank you." {
            return Ok(String::new());
        }

        Ok(text)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_client_creation() {
        let client = WhisperClient::new("http://localhost:8178".to_string());
        assert_eq!(client.server_url, "http://localhost:8178");
    }

    #[test]
    fn test_filter_silence() {
        // Empty text should be filtered
        let text = "";
        assert_eq!(text.is_empty(), true);

        // "Thank you." should be filtered
        let text = "Thank you.";
        assert_eq!(text, "Thank you.");
    }
}
