/// Test for URL validation edge cases in Config
///
/// Issue: The Config module validates URLs but might miss some edge cases
/// that could cause issues in the WhisperClient

use ears::Config;
use std::env;
use tempfile::TempDir;
use std::sync::Mutex;
use std::sync::LazyLock;

// Serialize tests that modify env vars to prevent interference
static ENV_LOCK: LazyLock<Mutex<()>> = LazyLock::new(|| Mutex::new(()));

#[test]
fn test_url_with_trailing_slash_normalization() {
    let _lock = ENV_LOCK.lock().unwrap();

    let temp_dir = TempDir::new().unwrap();
    let original_home = env::var("HOME").ok();
    let original_xdg = env::var("XDG_CONFIG_HOME").ok();
    let original_server = env::var("EARS_SERVER").ok();
    let original_device = env::var("EARS_DEVICE").ok();

    env::set_var("HOME", temp_dir.path());
    env::set_var("XDG_CONFIG_HOME", temp_dir.path().join("config"));
    env::remove_var("EARS_SERVER");
    env::remove_var("EARS_DEVICE");

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
    assert!(endpoint.contains("//health"), "URL construction creates double slash");

    // Cleanup
    if let Some(home) = original_home {
        env::set_var("HOME", home);
    } else {
        env::remove_var("HOME");
    }
    if let Some(xdg) = original_xdg {
        env::set_var("XDG_CONFIG_HOME", xdg);
    } else {
        env::remove_var("XDG_CONFIG_HOME");
    }
    if let Some(server) = original_server {
        env::set_var("EARS_SERVER", server);
    }
    if let Some(device) = original_device {
        env::set_var("EARS_DEVICE", device);
    }
}

#[test]
fn test_url_without_port() {
    let _lock = ENV_LOCK.lock().unwrap();

    let temp_dir = TempDir::new().unwrap();
    let original_home = env::var("HOME").ok();
    let original_xdg = env::var("XDG_CONFIG_HOME").ok();
    let original_server = env::var("EARS_SERVER").ok();
    let original_device = env::var("EARS_DEVICE").ok();

    env::set_var("HOME", temp_dir.path());
    env::set_var("XDG_CONFIG_HOME", temp_dir.path().join("config"));
    env::remove_var("EARS_SERVER");
    env::remove_var("EARS_DEVICE");

    let mut config = Config::new().unwrap();

    // URL without explicit port (should use default 80/443)
    config.whisper_server = url::Url::parse("http://whisper.example.com").unwrap();
    config.save().unwrap();

    let loaded = Config::load().unwrap();
    assert_eq!(loaded.whisper_server.as_str(), "http://whisper.example.com/");

    // This is valid, just checking it works
    println!("URL without port: {}", loaded.whisper_server);

    // Cleanup
    if let Some(home) = original_home {
        env::set_var("HOME", home);
    } else {
        env::remove_var("HOME");
    }
    if let Some(xdg) = original_xdg {
        env::set_var("XDG_CONFIG_HOME", xdg);
    } else {
        env::remove_var("XDG_CONFIG_HOME");
    }
    if let Some(server) = original_server {
        env::set_var("EARS_SERVER", server);
    }
    if let Some(device) = original_device {
        env::set_var("EARS_DEVICE", device);
    }
}

#[test]
fn test_url_with_path() {
    let _lock = ENV_LOCK.lock().unwrap();

    let temp_dir = TempDir::new().unwrap();
    let original_home = env::var("HOME").ok();
    let original_xdg = env::var("XDG_CONFIG_HOME").ok();
    let original_server = env::var("EARS_SERVER").ok();
    let original_device = env::var("EARS_DEVICE").ok();

    env::set_var("HOME", temp_dir.path());
    env::set_var("XDG_CONFIG_HOME", temp_dir.path().join("config"));
    env::remove_var("EARS_SERVER");
    env::remove_var("EARS_DEVICE");

    let mut config = Config::new().unwrap();

    // What if the server is behind a reverse proxy with a path?
    config.whisper_server = url::Url::parse("http://example.com/whisper-api").unwrap();
    config.save().unwrap();

    let loaded = Config::load().unwrap();
    // When URL has a path, trailing slash is not automatically added by url crate
    assert_eq!(loaded.whisper_server.as_str(), "http://example.com/whisper-api");

    // Now when we construct endpoints:
    let endpoint = format!("{}/health", loaded.whisper_server);
    // Since there's no trailing slash, this creates "http://example.com/whisper-api/health"
    // This is actually CORRECT - no double slash!

    println!("Endpoint with path: {}", endpoint);
    assert_eq!(endpoint, "http://example.com/whisper-api/health", "URL with path should work correctly");

    // Cleanup
    if let Some(home) = original_home {
        env::set_var("HOME", home);
    } else {
        env::remove_var("HOME");
    }
    if let Some(xdg) = original_xdg {
        env::set_var("XDG_CONFIG_HOME", xdg);
    } else {
        env::remove_var("XDG_CONFIG_HOME");
    }
    if let Some(server) = original_server {
        env::set_var("EARS_SERVER", server);
    }
    if let Some(device) = original_device {
        env::set_var("EARS_DEVICE", device);
    }
}

#[test]
fn test_url_validation_allows_trailing_slash() {
    let _lock = ENV_LOCK.lock().unwrap();

    let temp_dir = TempDir::new().unwrap();
    let original_home = env::var("HOME").ok();
    let original_xdg = env::var("XDG_CONFIG_HOME").ok();
    let original_server = env::var("EARS_SERVER").ok();
    let original_device = env::var("EARS_DEVICE").ok();

    env::set_var("HOME", temp_dir.path());
    env::set_var("XDG_CONFIG_HOME", temp_dir.path().join("config"));
    env::remove_var("EARS_SERVER");
    env::remove_var("EARS_DEVICE");

    let mut config = Config::new().unwrap();

    // User explicitly adds trailing slash
    config.whisper_server = url::Url::parse("http://localhost:8178/").unwrap();
    config.save().unwrap();

    let loaded = Config::load().unwrap();
    assert_eq!(loaded.whisper_server.as_str(), "http://localhost:8178/");

    // Still creates double slash
    let endpoint = format!("{}/health", loaded.whisper_server);
    println!("Explicit trailing slash endpoint: {}", endpoint);

    // Cleanup
    if let Some(home) = original_home {
        env::set_var("HOME", home);
    } else {
        env::remove_var("HOME");
    }
    if let Some(xdg) = original_xdg {
        env::set_var("XDG_CONFIG_HOME", xdg);
    } else {
        env::remove_var("XDG_CONFIG_HOME");
    }
    if let Some(server) = original_server {
        env::set_var("EARS_SERVER", server);
    }
    if let Some(device) = original_device {
        env::set_var("EARS_DEVICE", device);
    }
}
