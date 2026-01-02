//! Integration tests for WhisperClient
//!
//! These tests use wiremock to simulate a whisper.cpp server without
//! requiring an actual server to be running.

use ears::{WhisperClient, WhisperError};
use std::fs;
use std::path::PathBuf;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

/// Creates a temporary test audio file
fn create_test_audio_file() -> PathBuf {
    let temp_dir = std::env::temp_dir();
    let audio_path = temp_dir.join(format!("test_audio_{}.wav", std::process::id()));

    // Create a minimal WAV file (44 bytes header + some data)
    // This is a valid WAV file structure, though the audio data is just zeros
    let wav_data = vec![
        // RIFF header
        0x52, 0x49, 0x46, 0x46, // "RIFF"
        0x24, 0x00, 0x00, 0x00, // File size - 8
        0x57, 0x41, 0x56, 0x45, // "WAVE"
        // fmt chunk
        0x66, 0x6D, 0x74, 0x20, // "fmt "
        0x10, 0x00, 0x00, 0x00, // fmt chunk size
        0x01, 0x00, // Audio format (PCM)
        0x01, 0x00, // Num channels (mono)
        0x80, 0x3E, 0x00, 0x00, // Sample rate (16000)
        0x00, 0x7D, 0x00, 0x00, // Byte rate
        0x02, 0x00, // Block align
        0x10, 0x00, // Bits per sample (16)
        // data chunk
        0x64, 0x61, 0x74, 0x61, // "data"
        0x00, 0x00, 0x00, 0x00, // Data size (0)
    ];

    fs::write(&audio_path, wav_data).expect("Failed to create test audio file");
    audio_path
}

/// Cleanup test audio file
fn cleanup_test_audio_file(path: &PathBuf) {
    let _ = fs::remove_file(path);
}

#[tokio::test]
async fn test_health_check_success() {
    // Start mock server
    let mock_server = MockServer::start().await;

    // Set up mock response for health check
    Mock::given(method("GET"))
        .and(path("/health"))
        .respond_with(ResponseTemplate::new(200))
        .mount(&mock_server)
        .await;

    // Create client and test
    let client = WhisperClient::new(mock_server.uri());
    let result = client.health_check().await;

    assert!(result.is_ok());
}

#[tokio::test]
async fn test_health_check_server_down() {
    // Create client pointing to non-existent server
    let client = WhisperClient::new("http://localhost:99999");
    let result = client.health_check().await;

    assert!(result.is_err());
    match result {
        Err(WhisperError::ConnectionError(_)) => {}
        _ => panic!("Expected ConnectionError"),
    }
}

#[tokio::test]
async fn test_health_check_server_error() {
    // Start mock server
    let mock_server = MockServer::start().await;

    // Set up mock response for health check with error status
    Mock::given(method("GET"))
        .and(path("/health"))
        .respond_with(ResponseTemplate::new(500))
        .mount(&mock_server)
        .await;

    // Create client and test
    let client = WhisperClient::new(mock_server.uri());
    let result = client.health_check().await;

    assert!(result.is_err());
    match result {
        Err(WhisperError::ConnectionError(msg)) => {
            assert!(msg.contains("500"));
        }
        _ => panic!("Expected ConnectionError with status code"),
    }
}

#[tokio::test]
async fn test_transcribe_success() {
    // Start mock server
    let mock_server = MockServer::start().await;

    // Set up mock response for transcription
    let response_body = r#"{"text": "Hello world"}"#;
    Mock::given(method("POST"))
        .and(path("/inference"))
        .respond_with(ResponseTemplate::new(200).set_body_string(response_body))
        .mount(&mock_server)
        .await;

    // Create test audio file
    let audio_path = create_test_audio_file();

    // Create client and test
    let client = WhisperClient::new(mock_server.uri());
    let result = client.transcribe(&audio_path).await;

    // Cleanup
    cleanup_test_audio_file(&audio_path);

    assert!(result.is_ok());
    assert_eq!(result.unwrap(), "Hello world");
}

#[tokio::test]
async fn test_transcribe_filters_thank_you() {
    // Start mock server
    let mock_server = MockServer::start().await;

    // Set up mock response with silence artifact
    let response_body = r#"{"text": "Thank you."}"#;
    Mock::given(method("POST"))
        .and(path("/inference"))
        .respond_with(ResponseTemplate::new(200).set_body_string(response_body))
        .mount(&mock_server)
        .await;

    // Create test audio file
    let audio_path = create_test_audio_file();

    // Create client and test
    let client = WhisperClient::new(mock_server.uri());
    let result = client.transcribe(&audio_path).await;

    // Cleanup
    cleanup_test_audio_file(&audio_path);

    // Should return error because filtered text is empty
    assert!(result.is_err());
    match result {
        Err(WhisperError::EmptyTranscription) => {}
        _ => panic!("Expected EmptyTranscription error"),
    }
}

#[tokio::test]
async fn test_transcribe_file_not_found() {
    let client = WhisperClient::new("http://localhost:8178");
    let result = client
        .transcribe("/nonexistent/path/audio.wav")
        .await;

    assert!(result.is_err());
    match result {
        Err(WhisperError::InvalidAudioFile(msg)) => {
            assert!(msg.contains("does not exist"));
        }
        _ => panic!("Expected InvalidAudioFile error"),
    }
}

#[tokio::test]
async fn test_transcribe_empty_file() {
    // Create empty audio file
    let temp_dir = std::env::temp_dir();
    let audio_path = temp_dir.join(format!("empty_audio_{}.wav", std::process::id()));
    fs::write(&audio_path, b"").expect("Failed to create empty file");

    let client = WhisperClient::new("http://localhost:8178");
    let result = client.transcribe(&audio_path).await;

    // Cleanup
    cleanup_test_audio_file(&audio_path);

    assert!(result.is_err());
    match result {
        Err(WhisperError::InvalidAudioFile(msg)) => {
            assert!(msg.contains("empty"));
        }
        _ => panic!("Expected InvalidAudioFile error for empty file"),
    }
}

#[tokio::test]
async fn test_transcribe_server_error() {
    // Start mock server
    let mock_server = MockServer::start().await;

    // Set up mock response with error
    Mock::given(method("POST"))
        .and(path("/inference"))
        .respond_with(ResponseTemplate::new(500))
        .mount(&mock_server)
        .await;

    // Create test audio file
    let audio_path = create_test_audio_file();

    // Create client with very short timeout to fail fast
    let client = WhisperClient::with_retry_config(mock_server.uri(), 1, 10, 50);
    let result = client.transcribe(&audio_path).await;

    // Cleanup
    cleanup_test_audio_file(&audio_path);

    // backoff library will eventually return the error after retries
    assert!(result.is_err());
}

#[tokio::test]
async fn test_transcribe_with_retry_eventually_succeeds() {
    // Start mock server
    let mock_server = MockServer::start().await;

    // First two requests fail, third succeeds
    Mock::given(method("POST"))
        .and(path("/inference"))
        .respond_with(
            ResponseTemplate::new(500)
                .append_header("X-Retry", "1"),
        )
        .up_to_n_times(2)
        .mount(&mock_server)
        .await;

    Mock::given(method("POST"))
        .and(path("/inference"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_string(r#"{"text": "Success after retry"}"#),
        )
        .mount(&mock_server)
        .await;

    // Create test audio file
    let audio_path = create_test_audio_file();

    // Create client with retry enabled
    let client = WhisperClient::with_retry_config(mock_server.uri(), 3, 10, 100);
    let result = client.transcribe(&audio_path).await;

    // Cleanup
    cleanup_test_audio_file(&audio_path);

    assert!(result.is_ok());
    assert_eq!(result.unwrap(), "Success after retry");
}

#[tokio::test]
async fn test_transcribe_trims_whitespace() {
    // Start mock server
    let mock_server = MockServer::start().await;

    // Response with extra whitespace
    let response_body = r#"{"text": "  Hello world  "}"#;
    Mock::given(method("POST"))
        .and(path("/inference"))
        .respond_with(ResponseTemplate::new(200).set_body_string(response_body))
        .mount(&mock_server)
        .await;

    // Create test audio file
    let audio_path = create_test_audio_file();

    // Create client and test
    let client = WhisperClient::new(mock_server.uri());
    let result = client.transcribe(&audio_path).await;

    // Cleanup
    cleanup_test_audio_file(&audio_path);

    assert!(result.is_ok());
    assert_eq!(result.unwrap(), "Hello world");
}
