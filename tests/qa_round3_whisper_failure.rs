/// QA Round 3: Test whisper server failure scenarios
///
/// This test investigates what happens when the whisper server:
/// 1. Becomes unreachable during transcription
/// 2. Returns malformed responses
/// 3. Times out
use ears::WhisperClient;
use std::io::Write;
use tempfile::NamedTempFile;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

#[tokio::test]
async fn test_whisper_server_dies_during_transcription() {
    println!("\n🔍 BUG INVESTIGATION: What if whisper server dies mid-transcription?");

    // Create a temporary audio file
    let mut temp_file = NamedTempFile::new().unwrap();
    temp_file.write_all(b"fake audio data").unwrap();
    temp_file.flush().unwrap();
    let audio_path = temp_file.path();

    // Start a mock server
    let mock_server = MockServer::start().await;

    // Set up a mock that will close the connection
    Mock::given(method("POST"))
        .and(path("/inference"))
        .respond_with(ResponseTemplate::new(500).set_body_string("Internal Server Error"))
        .mount(&mock_server)
        .await;

    let client = WhisperClient::new(mock_server.uri());

    // Try to transcribe - should handle error gracefully
    let result = client.transcribe(audio_path).await;

    println!("Result: {:?}", result);

    // EXPECTED: Should return a clear error, not panic
    assert!(result.is_err(), "Should fail when server returns error");

    // BUG?: Does the error message help the user understand what went wrong?
    if let Err(e) = result {
        let error_msg = e.to_string();
        println!("Error message: {}", error_msg);

        // The error should mention transcription failure
        assert!(
            error_msg.contains("Transcription") || error_msg.contains("Server"),
            "Error message should be descriptive: {}",
            error_msg
        );
    }
}

#[tokio::test]
async fn test_whisper_server_timeout() {
    println!("\n🔍 BUG INVESTIGATION: What if whisper server times out?");

    // Create a temporary audio file
    let mut temp_file = NamedTempFile::new().unwrap();
    temp_file.write_all(b"fake audio data").unwrap();
    temp_file.flush().unwrap();
    let audio_path = temp_file.path();

    // Start a mock server that delays forever (will trigger timeout)
    let mock_server = MockServer::start().await;

    Mock::given(method("POST"))
        .and(path("/inference"))
        .respond_with(ResponseTemplate::new(200).set_delay(std::time::Duration::from_secs(60)))
        .mount(&mock_server)
        .await;

    let client = WhisperClient::new(mock_server.uri());

    // Try to transcribe - should timeout
    let result = client.transcribe(audio_path).await;

    println!("Result: {:?}", result);

    // EXPECTED: Should timeout and return error
    assert!(result.is_err(), "Should fail on timeout");
}

#[tokio::test]
async fn test_whisper_server_malformed_response() {
    println!("\n🔍 BUG INVESTIGATION: What if whisper server returns malformed JSON?");

    // Create a temporary audio file
    let mut temp_file = NamedTempFile::new().unwrap();
    temp_file.write_all(b"fake audio data").unwrap();
    temp_file.flush().unwrap();
    let audio_path = temp_file.path();

    // Start a mock server
    let mock_server = MockServer::start().await;

    // Return invalid JSON
    Mock::given(method("POST"))
        .and(path("/inference"))
        .respond_with(ResponseTemplate::new(200).set_body_string("This is not JSON"))
        .mount(&mock_server)
        .await;

    let client = WhisperClient::new(mock_server.uri());

    // Try to transcribe - should handle parsing error
    let result = client.transcribe(audio_path).await;

    println!("Result: {:?}", result);

    // EXPECTED: Should return parsing error
    assert!(result.is_err(), "Should fail on malformed response");

    if let Err(e) = result {
        println!("Error type: {:?}", e);
    }
}

#[tokio::test]
async fn test_whisper_server_returns_invalid_text_field() {
    println!("\n🔍 BUG INVESTIGATION: What if whisper server returns JSON without 'text' field?");

    // Create a temporary audio file
    let mut temp_file = NamedTempFile::new().unwrap();
    temp_file.write_all(b"fake audio data").unwrap();
    temp_file.flush().unwrap();
    let audio_path = temp_file.path();

    // Start a mock server
    let mock_server = MockServer::start().await;

    // Return JSON but missing 'text' field
    Mock::given(method("POST"))
        .and(path("/inference"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "error": "no speech detected"
        })))
        .mount(&mock_server)
        .await;

    let client = WhisperClient::new(mock_server.uri());

    // Try to transcribe - should handle missing field
    let result = client.transcribe(audio_path).await;

    println!("Result: {:?}", result);

    // EXPECTED: Should return error about missing field
    assert!(result.is_err(), "Should fail when 'text' field is missing");
}
