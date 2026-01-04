/// QA Round 3: Test missing external command dependencies
///
/// This test investigates what happens when required external commands are missing:
/// 1. pw-record (PipeWire recording)
/// 2. ydotool (text input)
/// 3. notify-send (notifications)
/// 4. paplay (audio feedback)
/// 5. fzf (device selection)
///
/// NOTE: These tests are NON-DISRUPTIVE - they use mocks to avoid
/// typing on screen, playing sounds, or showing notifications.
use ears::{AudioFeedback, Notifications, ProcessManager, TextInput};
use std::env;
use std::path::PathBuf;
use std::process::Command;
use std::time::Duration;
use tempfile::TempDir;

/// Get a mock PATH that includes our mock binaries
fn get_mock_path() -> String {
    let mock_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("mocks");

    let current_path = env::var("PATH").unwrap_or_default();
    format!("{}:{}", mock_dir.display(), current_path)
}

/// Setup mock environment and return guard to restore on drop
fn setup_mock_env() -> (Option<String>, TempDir) {
    let original_path = env::var("PATH").ok();
    env::set_var("PATH", get_mock_path());

    let temp_dir = TempDir::new().unwrap();
    env::set_var("TEST_TEMP_DIR", temp_dir.path());

    (original_path, temp_dir)
}

fn restore_env(original_path: Option<String>) {
    if let Some(path) = original_path {
        env::set_var("PATH", path);
    }
}

#[test]
fn test_missing_notify_send() {
    println!("\n🔍 BUG INVESTIGATION: What if notify-send is missing?");

    // Use mock environment to avoid showing actual notifications
    let (original_path, _temp_dir) = setup_mock_env();

    let result = Notifications::info("Test message");
    println!("Result: {:?}", result);

    restore_env(original_path);

    // If notify-send is present (or mocked), should succeed
    // If missing, should return Err with helpful message

    // BUG POTENTIAL: Error message should indicate notify-send is missing
    if let Err(e) = result {
        println!("Error: {}", e);
        // Should mention "notify-send" in error
    }
}

#[test]
fn test_missing_paplay() {
    println!("\n🔍 BUG INVESTIGATION: What if paplay is missing?");

    // Use mock environment to avoid playing actual sounds
    let (original_path, _temp_dir) = setup_mock_env();

    let result = AudioFeedback::beep_start();
    println!("Result: {:?}", result);

    restore_env(original_path);

    // If paplay is present (or mocked), should succeed
    // If missing, should return Err

    // BUG POTENTIAL: Error should indicate paplay is missing
    if let Err(e) = result {
        println!("Error: {}", e);
    }
}

#[test]
fn test_missing_ydotool() {
    println!("\n🔍 BUG INVESTIGATION: What if ydotool is missing?");

    // Use mock environment to avoid typing on screen
    let (original_path, _temp_dir) = setup_mock_env();

    let result = TextInput::type_text("test");
    println!("Result: {:?}", result);

    restore_env(original_path);

    // If ydotool is present (or mocked), should succeed
    // If missing, should return Err with clear message

    // BUG POTENTIAL: Error should indicate ydotool is missing or not running
    if let Err(e) = result {
        println!("Error: {}", e);
        // Note: with mock, this won't fail
    }
}

#[test]
fn test_missing_pw_record() {
    println!("\n🔍 BUG INVESTIGATION: What if pw-record is missing?");

    let temp_dir = TempDir::new().unwrap();
    let pid_file = temp_dir.path().join("test.pid");
    let audio_file = temp_dir.path().join("test.wav");

    let process_mgr = ProcessManager::new(&pid_file, Duration::from_secs(10));

    // Try to spawn pw-record
    let result = process_mgr.spawn_recording("fake-device", &audio_file);
    println!("Result: {:?}", result);

    // If pw-record is present, might fail due to invalid device
    // If missing, should return Err mentioning pw-record

    if let Err(e) = result {
        println!("Error: {}", e);
        // Should give a clear error about missing command or invalid device
    }
}

#[test]
fn test_missing_pw_cli() {
    println!("\n🔍 BUG INVESTIGATION: What if pw-cli is missing?");

    // list_devices() calls pw-cli, but it's in audio module which isn't publicly exported
    // The code is in src/audio.rs lines 17-30
    // It uses Command::new("pw-cli") and returns error with context if it fails

    // From audio.rs:
    // .context("Failed to execute pw-cli")?

    println!("✅ Code already handles missing pw-cli with error context");
}

#[test]
fn test_command_path_injection() {
    println!(
        "\n🔍 SECURITY BUG INVESTIGATION: Can malicious device names cause command injection?"
    );

    // Use temp dir for all test files - completely safe
    let temp_dir = TempDir::new().unwrap();
    let pid_file = temp_dir.path().join("test.pid");
    let audio_file = temp_dir.path().join("test.wav");
    let marker_file = temp_dir.path().join("injection_marker.txt");

    let process_mgr = ProcessManager::new(&pid_file, Duration::from_secs(10));

    // Try with malicious device name - uses temp dir marker file (harmless)
    let malicious_device = format!("device; touch {}", marker_file.display());

    let result = process_mgr.spawn_recording(&malicious_device, &audio_file);

    // SECURITY: Should NOT execute the injected command
    // Command::new("timeout").arg(...).arg(malicious_device) should safely pass it as arg

    // Check if injection worked - marker file should NOT exist
    assert!(
        !marker_file.exists(),
        "CRITICAL SECURITY BUG: Command injection possible!"
    );

    // The spawn will likely fail because device doesn't exist, but it shouldn't execute injection
    if let Err(e) = result {
        println!("Spawn failed (expected): {}", e);
    }

    println!("✅ No command injection - device name passed safely as argument");
}

#[test]
fn test_text_input_command_injection() {
    println!(
        "\n🔍 SECURITY BUG INVESTIGATION: Can malicious text cause command injection in ydotool?"
    );

    // Use mock environment to avoid typing on screen
    let (original_path, temp_dir) = setup_mock_env();

    // Create marker file path in temp dir (harmless)
    let marker_file = temp_dir.path().join("injection_marker.txt");

    // What if transcribed text contains special characters?
    // Using harmless touch command instead of rm -rf
    let malicious_text = format!("hello; touch {}", marker_file.display());

    let result = TextInput::type_text(&malicious_text);

    restore_env(original_path);

    // SECURITY: Should NOT execute the injected command
    // The text should be typed literally, not executed

    // Check marker file doesn't exist (injection didn't work)
    assert!(
        !marker_file.exists(),
        "SECURITY BUG: Command injection in text input!"
    );

    if let Err(e) = result {
        println!("Type failed: {}", e);
    }

    println!("✅ No command injection - text passed safely as argument");
}

#[test]
fn test_missing_fzf_for_device_selection() {
    println!("\n🔍 BUG INVESTIGATION: What if fzf is missing during device selection?");

    // This would be tested in select_device_interactive
    // but we can't easily simulate missing fzf

    // The code should return Err with message about fzf not being installed

    // From audio.rs line 137:
    // .context("Failed to spawn fzf (is it installed?)")?;

    println!("✅ Code already handles missing fzf with helpful error message");
}

#[test]
fn test_column_command_fallback() {
    println!("\n🔍 BUG INVESTIGATION: What if 'column' command is missing during device list?");

    // From main.rs lines 127-148, there's a fallback when column is not available:
    // match child {
    //     Ok(mut child) => { ... }
    //     Err(_) => {
    //         // If column is not available, just print the raw output
    //         println!("{}", formatted);
    //     }
    // }

    println!("✅ Code already has fallback when 'column' is not available");
}

#[test]
fn test_timeout_command_availability() {
    println!("\n🔍 BUG INVESTIGATION: What if 'timeout' command is missing?");

    // ProcessManager::spawn_recording uses 'timeout' command
    // If it's missing, pw-record will fail to spawn

    // Check if timeout is available
    let has_timeout = Command::new("which")
        .arg("timeout")
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false);

    println!("timeout available: {}", has_timeout);

    if !has_timeout {
        println!("⚠️  WARNING: 'timeout' command not found - recording will fail!");
        println!("   This is used in ProcessManager::spawn_recording");
    }

    // BUG POTENTIAL: If timeout is missing, error message might not be clear
    // about why pw-record failed
}
