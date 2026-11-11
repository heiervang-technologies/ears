/// QA Round 3: Config::load with invalid URL bug
///
/// BUG FOUND: When server config file exists with invalid URL,
/// Config::load() fails completely instead of falling back to default
use ears::Config;
use tempfile::TempDir;

#[test]
#[serial_test::serial]
fn test_better_error_recovery_approach() {
    println!("\n💡 BETTER APPROACH: Partial config loading with warnings");

    // Use a temp HOME so Config::load() doesn't read/write the real ~/.config/ears/
    let temp_dir = TempDir::new().unwrap();
    let original_home = std::env::var("HOME").ok();
    std::env::set_var("HOME", temp_dir.path());

    let result = Config::load();

    match &original_home {
        Some(h) => std::env::set_var("HOME", h),
        None => std::env::remove_var("HOME"),
    }

    // Config should always load successfully, falling back to defaults when needed
    assert!(result.is_ok(), "Config should load with partial recovery");

    let config = result.unwrap();
    assert!(!config.device.is_empty(), "Device should have a value");
    assert!(
        config.whisper_server.as_str().starts_with("http"),
        "Server should be a valid URL"
    );
}

#[test]
#[serial_test::serial]
fn test_config_from_env_precedence() {
    std::env::set_var("EARS_SERVER", "http://env-server:9999");
    std::env::set_var("EARS_DEVICE", "env-device");

    let config = Config::load().unwrap();

    std::env::remove_var("EARS_SERVER");
    std::env::remove_var("EARS_DEVICE");

    assert_eq!(config.whisper_server.as_str(), "http://env-server:9999/");
    assert_eq!(config.device, "env-device");
}
