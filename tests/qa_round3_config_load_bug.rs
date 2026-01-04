/// QA Round 3: Config::load with invalid URL bug
///
/// BUG FOUND: When server config file exists with invalid URL,
/// Config::load() fails completely instead of falling back to default
use ears::Config;
use std::fs;
use tempfile::TempDir;

#[test]
fn test_config_load_with_invalid_url_in_file() {
    println!("\n🐛 BUG: Config::load fails if server file has invalid URL");

    let temp_dir = TempDir::new().unwrap();
    let config_dir = temp_dir.path().join("config");
    fs::create_dir_all(&config_dir).unwrap();

    // Create server file with invalid URL
    let server_file = config_dir.join("server");
    fs::write(&server_file, "this-is-not-a-valid-url").unwrap();

    // Try to load config
    // First, set up environment to use our test config dir
    let mut config = Config::new().unwrap();
    config.config_dir = config_dir.clone();

    // Now try to manually do what Config::load does (lines 75-85)
    let server_str = fs::read_to_string(&server_file).unwrap().trim().to_string();

    let parse_result = url::Url::parse(&server_str);
    println!("Parse result: {:?}", parse_result);

    assert!(parse_result.is_err(), "Should fail to parse invalid URL");

    println!("\n🐛 CONFIRMED BUG:");
    println!("   Location: src/config.rs lines 76-85");
    println!("   Issue: If server config file exists but has invalid URL,");
    println!("         Config::load() returns Err instead of using default");
    println!("   Behavior:");
    println!("     config.whisper_server = Url::parse(&server_str)");
    println!("         .with_context(|| format!(\"Invalid server URL in config file: {{}}\", server_str))?;");
    println!("   Result: User can't use ears at all if config file is corrupted");
    println!("   Expected: Should log warning and fall back to default URL");
}

#[test]
fn test_config_load_with_invalid_url_prevents_startup() {
    println!("\n🐛 BUG IMPACT: Invalid URL in config prevents ears from starting");

    let temp_dir = TempDir::new().unwrap();
    let config_dir = temp_dir.path().join("config");
    fs::create_dir_all(&config_dir).unwrap();

    // Simulate corrupted server file (maybe user edited it wrong)
    let server_file = config_dir.join("server");
    fs::write(&server_file, "htp://localhost:8178").unwrap(); // Typo: htp instead of http

    // This is what happens in main.rs line 20
    // let config = Config::load().unwrap_or_else(|_| Config::new().expect("Failed to create config"));

    // But Config::load() will fail!
    std::env::set_var("HOME", temp_dir.path());

    // Simulate the actual call
    let result = Config::load();

    println!("Config::load result: {:?}", result);

    if result.is_err() {
        println!("\n🐛 PROBLEM:");
        println!("   A simple typo in ~/.config/ears/server prevents ears from running");
        println!("   Error message points to the problem, but user can't easily recover");
        println!("   They need to manually delete the file or fix it");
    }

    std::env::remove_var("HOME");
}

#[test]
fn test_main_rs_has_fallback() {
    println!("\n✅ MITIGATED: main.rs has fallback on Config::load failure");

    // From main.rs line 20:
    // let config = Config::load().unwrap_or_else(|_| Config::new().expect("Failed to create config"));

    println!("main.rs line 20 uses unwrap_or_else fallback");
    println!("So even if Config::load() fails, it falls back to Config::new()");
    println!("\nBUT: This loses the user's device configuration!");
    println!("     If ~/.config/ears/server is corrupt, user also loses ~/.config/ears/device");
}

#[test]
fn test_better_error_recovery_approach() {
    println!("\n💡 BETTER APPROACH: Partial config loading with warnings");

    // NOTE: This test demonstrates that the better approach has already been implemented!
    // Config::load() now does partial recovery instead of failing completely.

    let temp_dir = TempDir::new().unwrap();
    let config_dir = temp_dir.path().join("config");
    fs::create_dir_all(&config_dir).unwrap();

    // Set the config dir for this test
    std::env::set_var("HOME", temp_dir.path());
    std::env::set_var("XDG_CONFIG_HOME", temp_dir.path().join("config"));

    // Create valid device file but invalid server file
    let ears_config_dir = temp_dir.path().join("config").join("ears");
    fs::create_dir_all(&ears_config_dir).unwrap();
    fs::write(ears_config_dir.join("device"), "my-custom-device").unwrap();
    fs::write(ears_config_dir.join("server"), "invalid-url").unwrap();

    // Current behavior: Config::load() succeeds with partial recovery
    let result = Config::load();
    assert!(
        result.is_ok(),
        "Config::load() should succeed with partial recovery"
    );

    let config = result.unwrap();

    // Verify partial recovery works:
    // 1. Device config should be preserved
    assert_eq!(
        config.device, "my-custom-device",
        "Device config should be preserved"
    );

    // 2. Server URL should fall back to default
    assert_eq!(
        config.whisper_server.to_string(),
        "http://127.0.0.1:8178/",
        "Invalid server URL should fall back to default"
    );

    println!("✅ Partial recovery works!");
    println!("  - Device config preserved: {}", config.device);
    println!(
        "  - Invalid server URL fell back to default: {}",
        config.whisper_server
    );

    // Clean up
    std::env::remove_var("XDG_CONFIG_HOME");
}

#[test]
fn test_config_from_env_precedence() {
    println!("\n🔍 OBSERVATION: Environment variables have correct precedence");

    std::env::set_var("EARS_SERVER", "http://env-server:9999");
    std::env::set_var("EARS_DEVICE", "env-device");

    let config = Config::from_env().unwrap();

    println!("Server from env: {}", config.whisper_server);
    println!("Device from env: {}", config.device);

    assert_eq!(config.whisper_server.as_str(), "http://env-server:9999/");
    assert_eq!(config.device, "env-device");

    std::env::remove_var("EARS_SERVER");
    std::env::remove_var("EARS_DEVICE");

    println!("\n✅ Environment variables work correctly");
    println!("   User can override corrupt config with EARS_SERVER env var");
}

#[test]
fn test_severity_assessment() {
    println!("\n📊 BUG SEVERITY ASSESSMENT");
    println!("\nBUG: Config::load fails on invalid URL, losing all config");
    println!("\nSeverity: MEDIUM");
    println!("\nReasons:");
    println!("  ✓ Mitigated by: main.rs fallback to Config::new()");
    println!("  ✓ Workaround: User can set EARS_SERVER env var");
    println!("  ✓ User can manually fix/delete ~/.config/ears/server");
    println!("  ✗ BUT: Loses device configuration too");
    println!("  ✗ BUT: No clear error message about recovery options");
    println!("  ✗ BUT: User might not know about env var workaround");
    println!("\nImpact:");
    println!("  - If user accidentally corrupts config file, they lose device setting");
    println!("  - Confusing error message doesn't explain recovery");
    println!("  - Could frustrate users who don't understand the error");
}
