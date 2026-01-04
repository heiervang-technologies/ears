/// Test for URL validation edge cases in Config
///
/// Issue: The Config module validates URLs but might miss some edge cases
/// that could cause issues in the WhisperClient
use ears::Config;
use std::env;
use tempfile::TempDir;

#[test]
fn test_url_with_trailing_slash_normalization() {
    let temp_dir = TempDir::new().unwrap();
    env::set_var("HOME", temp_dir.path());

    let mut config = Config::new().unwrap();

    // Set URL without trailing slash
    config.whisper_server = url::Url::parse("http://localhost:8178").unwrap();
    config.save().unwrap();

    // Load it back
    let loaded = Config::load().unwrap();

    // url crate automatically adds trailing slash
    assert_eq!(loaded.whisper_server.as_str(), "http://localhost:8178/");

    // But what if we use this URL for API calls?
    let endpoint = format!("{}/health", loaded.whisper_server);
    // This creates "http://localhost:8178//health" - DOUBLE SLASH!

    println!("Endpoint with potential issue: {}", endpoint);
    assert!(
        endpoint.contains("//health"),
        "URL construction creates double slash"
    );
}

#[test]
fn test_url_without_port() {
    let temp_dir = TempDir::new().unwrap();
    env::set_var("HOME", temp_dir.path());

    let mut config = Config::new().unwrap();

    // URL without explicit port (should use default 80/443)
    config.whisper_server = url::Url::parse("http://whisper.example.com").unwrap();
    config.save().unwrap();

    let loaded = Config::load().unwrap();
    assert_eq!(
        loaded.whisper_server.as_str(),
        "http://whisper.example.com/"
    );

    // This is valid, just checking it works
    println!("URL without port: {}", loaded.whisper_server);
}

#[test]
fn test_url_with_path() {
    let temp_dir = TempDir::new().unwrap();
    env::set_var("HOME", temp_dir.path());

    let mut config = Config::new().unwrap();

    // What if the server is behind a reverse proxy with a path?
    config.whisper_server = url::Url::parse("http://example.com/whisper-api").unwrap();
    config.save().unwrap();

    let loaded = Config::load().unwrap();
    // URL with path may or may not have trailing slash depending on how it was saved
    assert!(
        loaded
            .whisper_server
            .as_str()
            .starts_with("http://example.com/whisper-api"),
        "URL with path should be preserved"
    );

    // Now when we construct endpoints:
    let endpoint = format!("{}/health", loaded.whisper_server);
    // This creates "http://example.com/whisper-api//health"
    // DOUBLE SLASH AGAIN!

    println!("Endpoint with path: {}", endpoint);
    assert!(
        endpoint.contains("//health"),
        "URL with path creates double slash"
    );
}

#[test]
fn test_url_validation_allows_trailing_slash() {
    let temp_dir = TempDir::new().unwrap();
    env::set_var("HOME", temp_dir.path());

    let mut config = Config::new().unwrap();

    // User explicitly adds trailing slash
    config.whisper_server = url::Url::parse("http://localhost:8178/").unwrap();
    config.save().unwrap();

    let loaded = Config::load().unwrap();
    assert_eq!(loaded.whisper_server.as_str(), "http://localhost:8178/");

    // Still creates double slash
    let endpoint = format!("{}/health", loaded.whisper_server);
    println!("Explicit trailing slash endpoint: {}", endpoint);
}
