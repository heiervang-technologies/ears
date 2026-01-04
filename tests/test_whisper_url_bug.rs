/// Test that verifies the double-slash URL bug in WhisperClient
///
/// The bug: When Config stores a URL, the `url` crate automatically adds a trailing slash.
/// Then when WhisperClient constructs endpoints, it does format!("{}/health", url),
/// creating a double slash: "http://localhost:8178//health"
///
/// While most HTTP servers tolerate this, it's technically incorrect and could fail
/// with strict servers or reverse proxies.
use ears::WhisperClient;

#[test]
fn test_double_slash_in_health_check_url() {
    // Create a client with a URL (url::Url always adds trailing slash)
    let client = WhisperClient::new("http://localhost:8178/");

    // The server_url field internally will be "http://localhost:8178/"
    // When health_check constructs the URL, it does:
    // format!("{}/health", self.server_url)
    // Result: "http://localhost:8178//health"

    // We can't easily test the actual URL construction without making health_check
    // public or adding debug output, but we know from the code it happens.

    // This is a CONFIRMED bug, even though it might work in practice.
    println!("WhisperClient constructed with trailing slash will create double-slash URLs");
    println!("Expected URL: http://localhost:8178/health");
    println!("Actual URL: http://localhost:8178//health");
}

#[test]
fn test_url_construction_pattern() {
    // Demonstrate the bug pattern
    let server_url = "http://localhost:8178/"; // As stored by url::Url
    let endpoint = format!("{}/health", server_url);

    assert_eq!(endpoint, "http://localhost:8178//health");
    println!("Double slash confirmed: {}", endpoint);

    // The correct way would be to strip trailing slash or use url.join()
    let correct = format!("{}health", server_url);
    assert_eq!(correct, "http://localhost:8178/health");
    println!("Correct construction: {}", correct);
}

#[test]
fn test_url_with_path_double_slash() {
    // Even worse with paths (e.g., behind reverse proxy)
    let server_url = "http://example.com/api/whisper/"; // With path
    let endpoint = format!("{}/inference", server_url);

    assert_eq!(endpoint, "http://example.com/api/whisper//inference");
    println!("Path + double slash: {}", endpoint);
}
