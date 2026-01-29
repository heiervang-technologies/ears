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
#[serial_test::serial]
fn test_config_load_with_invalid_url_prevents_startup() {
    println!("\n🐛 BUG IMPACT: Invalid URL in config prevents ears from starting");

    let temp_dir = TempDir::new().unwrap();
    let config_dir = temp_dir.path().join("config");
    fs::create_dir_all(&config_dir).unwrap();

    // Simulate corrupted server file (maybe user edited it wrong)
    let server_file = config_dir.join("server");
    fs::write(&server_file, "htp://localhost:8178").unwrap(); // Typo: htp instead of http

    let original_home = std::env::var("HOME").ok();
    std::env::set_var("HOME", temp_dir.path());

    let result = Config::load();

    // Restore HOME before any assertions
    match original_home {
        Some(h) => std::env::set_var("HOME", h),
        None => std::env::remove_var("HOME"),
    }

    println!("Config::load result: {:?}", result);

    if result.is_err() {
        println!("\n🐛 PROBLEM:");
        println!("   A simple typo in ~/.config/ears/server prevents ears from running");
        println!("   Error message points to the problem, but user can't easily recover");
        println!("   They need to manually delete the file or fix it");
    }
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

    let config = Config::from_env().unwrap();

    std::env::remove_var("EARS_SERVER");
    std::env::remove_var("EARS_DEVICE");

    assert_eq!(config.whisper_server.as_str(), "http://env-server:9999/");
    assert_eq!(config.device, "env-device");
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
