//! Integration tests for WhisperClient
//!
//! These tests use wiremock to simulate a whisper.cpp server without
//! requiring an actual server to be running.

use ears::{WhisperClient, WhisperError};
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

/// Owned, valid PCM fixture: even silence must include audio frames.
fn create_test_audio_file() -> tempfile::TempPath {
    let path = tempfile::NamedTempFile::new().unwrap().into_temp_path();
    std::fs::write(&path, ears::continuous::wav_bytes(&[0; 512])).unwrap();
    path
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

    // Also mock the fallback endpoint so it returns 500 too
    Mock::given(method("GET"))
        .and(path("/v1/models"))
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
        .and(path("/v1/audio/transcriptions"))
        .respond_with(ResponseTemplate::new(200).set_body_string(response_body))
        .mount(&mock_server)
        .await;

    // Create test audio file
    let audio_path = create_test_audio_file();

    // Create client and test
    let client = WhisperClient::new(mock_server.uri());
    let result = client.transcribe(&audio_path).await;

    // Cleanup
    drop(audio_path);

    assert!(result.is_ok());
    assert_eq!(result.unwrap(), "Hello world");
    assert_eq!(mock_server.received_requests().await.unwrap().len(), 1);
}

#[tokio::test]
async fn test_transcribe_filters_thank_you() {
    // Start mock server
    let mock_server = MockServer::start().await;

    // Set up mock response with silence artifact
    let response_body = r#"{"text": "Thank you."}"#;
    Mock::given(method("POST"))
        .and(path("/v1/audio/transcriptions"))
        .respond_with(ResponseTemplate::new(200).set_body_string(response_body))
        .mount(&mock_server)
        .await;

    // Create test audio file
    let audio_path = create_test_audio_file();

    // Create client and test
    let client = WhisperClient::new(mock_server.uri());
    let result = client.transcribe(&audio_path).await;

    // Cleanup
    drop(audio_path);

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
    let result = client.transcribe("/nonexistent/path/audio.wav").await;

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
    let audio_path = tempfile::NamedTempFile::new().unwrap().into_temp_path();

    let client = WhisperClient::new("http://localhost:8178");
    let result = client.transcribe(&audio_path).await;

    // Cleanup
    drop(audio_path);

    assert!(result.is_err());
    match result {
        Err(WhisperError::InvalidAudioFile(msg)) => {
            assert!(
                msg.contains("no audio data"),
                "Expected 'no audio data' in error, got: {}",
                msg
            );
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
        .and(path("/v1/audio/transcriptions"))
        .respond_with(ResponseTemplate::new(500))
        .mount(&mock_server)
        .await;

    // Create test audio file
    let audio_path = create_test_audio_file();

    // Create client with very short timeout to fail fast
    let client = WhisperClient::with_retry_config(mock_server.uri(), 1, 10, 50);
    let result = client.transcribe(&audio_path).await;

    // Cleanup
    drop(audio_path);

    assert!(matches!(result, Err(WhisperError::TranscriptionError(_))));
    assert_eq!(mock_server.received_requests().await.unwrap().len(), 2);
}

#[tokio::test]
async fn test_transcribe_with_retry_eventually_succeeds() {
    // Start mock server
    let mock_server = MockServer::start().await;

    let attempts = std::sync::atomic::AtomicUsize::new(0);
    Mock::given(method("POST"))
        .and(path("/v1/audio/transcriptions"))
        .respond_with(move |_: &wiremock::Request| {
            if attempts.fetch_add(1, std::sync::atomic::Ordering::SeqCst) < 2 {
                ResponseTemplate::new(500)
            } else {
                ResponseTemplate::new(200)
                    .set_body_json(serde_json::json!({"text":"Success after retry"}))
            }
        })
        .expect(3)
        .mount(&mock_server)
        .await;

    // Create test audio file
    let audio_path = create_test_audio_file();

    // Two allowed retries must reach the third response, exactly.
    let client = WhisperClient::with_retry_config(mock_server.uri(), 2, 1, 2);
    let result = client.transcribe(&audio_path).await;

    // Cleanup
    drop(audio_path);

    assert!(
        result.is_ok(),
        "Expected retry to eventually succeed, got: {:?}",
        result
    );
    assert_eq!(result.unwrap(), "Success after retry");
}

#[tokio::test]
async fn test_transcribe_trims_whitespace() {
    // Start mock server
    let mock_server = MockServer::start().await;

    // Response with extra whitespace
    let response_body = r#"{"text": "  Hello world  "}"#;
    Mock::given(method("POST"))
        .and(path("/v1/audio/transcriptions"))
        .respond_with(ResponseTemplate::new(200).set_body_string(response_body))
        .mount(&mock_server)
        .await;

    // Create test audio file
    let audio_path = create_test_audio_file();

    // Create client and test
    let client = WhisperClient::new(mock_server.uri());
    let result = client.transcribe(&audio_path).await;

    // Cleanup
    drop(audio_path);

    assert!(result.is_ok());
    assert_eq!(result.unwrap(), "Hello world");
    assert_eq!(mock_server.received_requests().await.unwrap().len(), 1);
}

#[tokio::test]
async fn zero_retries_sends_exactly_once() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/audio/transcriptions"))
        .respond_with(ResponseTemplate::new(500))
        .expect(1)
        .mount(&server)
        .await;
    let audio = create_test_audio_file();
    let client = WhisperClient::with_retry_config(server.uri(), 0, 1, 2);
    assert!(matches!(
        client.transcribe(&audio).await,
        Err(WhisperError::TranscriptionError(_))
    ));
}

#[tokio::test]
async fn bearer_auth_covers_health_fallback_and_transcription() {
    use wiremock::matchers::header;
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/health"))
        .and(header("authorization", "Bearer test-secret"))
        .respond_with(ResponseTemplate::new(404))
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/v1/models"))
        .and(header("authorization", "Bearer test-secret"))
        .respond_with(ResponseTemplate::new(200))
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/v1/audio/transcriptions"))
        .and(header("authorization", "Bearer test-secret"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({"text":"hello"})))
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/v1/chat/completions"))
        .and(header("authorization", "Bearer test-secret"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_json(serde_json::json!({"choices":[{"message":{"content":"hello"}}]})),
        )
        .expect(1)
        .mount(&server)
        .await;
    let client = WhisperClient::with_retry_config(server.uri(), 0, 1, 2)
        .with_model(Some("test-asr".into()))
        .with_api_key(Some("test-secret".into()));
    client.health_check().await.unwrap();
    assert_eq!(
        client.transcribe(create_test_audio_file()).await.unwrap(),
        "hello"
    );
    assert_eq!(
        client
            .transcribe_with_grammar(create_test_audio_file(), Some("root ::= \"hello\""))
            .await
            .unwrap(),
        "hello"
    );
}
