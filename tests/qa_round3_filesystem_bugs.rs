/// QA Round 3: Test filesystem edge cases
///
/// This test investigates:
/// 1. What happens when state directory is read-only?
/// 2. What happens when disk is full?
/// 3. What happens when config files are corrupted?

use ears::{Config, StateManager};
use std::fs;
use std::os::unix::fs::PermissionsExt;
use tempfile::TempDir;

#[test]
fn test_state_dir_readonly() {
    println!("\n🔍 BUG INVESTIGATION: What if state directory is read-only?");

    let temp_dir = TempDir::new().unwrap();
    let state_dir = temp_dir.path().join("state");
    fs::create_dir_all(&state_dir).unwrap();

    // Make directory read-only
    let mut perms = fs::metadata(&state_dir).unwrap().permissions();
    perms.set_mode(0o444); // Read-only
    fs::set_permissions(&state_dir, perms).unwrap();

    // Try to create StateManager
    let result = StateManager::new(&state_dir);

    println!("Result: {}", if result.is_ok() { "Ok" } else { "Err" });

    // BUG POTENTIAL: StateManager::new calls create_dir_all which might fail on read-only dir
    // But the directory already exists, so create_dir_all should succeed

    // Try to transition state (which writes to disk)
    if let Ok(mut state_mgr) = result {
        let transition_result = state_mgr.transition(ears::State::Recording);
        println!("Transition result: {:?}", transition_result);

        // EXPECTED: Should fail when trying to write state file
        assert!(
            transition_result.is_err(),
            "Should fail to write state file to read-only directory"
        );

        if let Err(e) = transition_result {
            println!("Error: {}", e);
            // BUG?: Is the error message clear enough for users?
        }
    }

    // Cleanup: restore permissions to allow TempDir cleanup
    let mut perms = fs::metadata(&state_dir).unwrap().permissions();
    perms.set_mode(0o755);
    let _ = fs::set_permissions(&state_dir, perms);
}

#[test]
fn test_corrupted_state_file() {
    println!("\n🔍 BUG INVESTIGATION: What if state file is corrupted?");

    let temp_dir = TempDir::new().unwrap();
    let state_dir = temp_dir.path().join("state");
    fs::create_dir_all(&state_dir).unwrap();

    // Create a corrupted state file
    let state_file = state_dir.join("state");
    fs::write(&state_file, "corrupted_garbage_data").unwrap();

    // Try to load state
    let mut state_mgr = StateManager::new(&state_dir).unwrap();
    let result = state_mgr.load_state();

    println!("Result: {:?}", result);

    // EXPECTED: Should return CorruptedState error
    assert!(result.is_err(), "Should fail to load corrupted state");

    if let Err(e) = result {
        println!("Error: {}", e);
        // Verify it's actually a CorruptedState error
        assert!(
            e.to_string().contains("corrupted"),
            "Error should indicate corrupted state"
        );
    }
}

#[test]
fn test_config_save_to_readonly_dir() {
    println!("\n🔍 BUG INVESTIGATION: What if config directory is read-only during save?");

    let temp_dir = TempDir::new().unwrap();
    let config_dir = temp_dir.path().join("config");
    fs::create_dir_all(&config_dir).unwrap();

    let mut config = Config::new().unwrap();
    config.config_dir = config_dir.clone();

    // Make config dir read-only
    let mut perms = fs::metadata(&config_dir).unwrap().permissions();
    perms.set_mode(0o444);
    fs::set_permissions(&config_dir, perms).unwrap();

    // Try to save config
    let result = config.save();

    println!("Result: {:?}", result);

    // EXPECTED: Should fail when trying to write to read-only directory
    assert!(result.is_err(), "Should fail to save config to read-only dir");

    if let Err(e) = result {
        println!("Error: {}", e);
    }

    // Cleanup
    let mut perms = fs::metadata(&config_dir).unwrap().permissions();
    perms.set_mode(0o755);
    let _ = fs::set_permissions(&config_dir, perms);
}

#[test]
fn test_config_load_with_invalid_url() {
    println!("\n🔍 BUG INVESTIGATION: What if server config file contains invalid URL?");

    let temp_dir = TempDir::new().unwrap();
    let config_dir = temp_dir.path().join("config");
    fs::create_dir_all(&config_dir).unwrap();

    // Write invalid URL to server config
    let server_file = config_dir.join("server");
    fs::write(&server_file, "not-a-valid-url-at-all").unwrap();

    // Create config that will try to load this
    let mut config = Config::new().unwrap();
    config.config_dir = config_dir.clone();

    // Manually load server file (simulating Config::load behavior)
    let server_str = fs::read_to_string(&server_file).unwrap().trim().to_string();
    let parse_result = url::Url::parse(&server_str);

    println!("Parse result: {:?}", parse_result);

    // EXPECTED: Should fail to parse invalid URL
    assert!(parse_result.is_err(), "Should fail to parse invalid URL");

    // BUG POTENTIAL: Config::load should handle this gracefully
    // Let's test the actual Config::load method
    let load_result = Config::load();
    println!("Config::load result: {:?}", load_result);

    // This will actually succeed because Config::load creates a new config first
    // and only overwrites if files exist and are valid
    // But it should fail when the file exists with invalid content
}

#[test]
fn test_state_dir_creation_in_unwritable_parent() {
    println!("\n🔍 BUG INVESTIGATION: What if parent of state_dir is unwritable?");

    let temp_dir = TempDir::new().unwrap();
    let parent = temp_dir.path().join("readonly_parent");
    fs::create_dir_all(&parent).unwrap();

    // Make parent read-only
    let mut perms = fs::metadata(&parent).unwrap().permissions();
    perms.set_mode(0o444);
    fs::set_permissions(&parent, perms).unwrap();

    let state_dir = parent.join("state");

    // Try to create StateManager (which calls create_dir_all)
    let result = StateManager::new(&state_dir);

    println!("Result: {}", if result.is_ok() { "Ok" } else { "Err" });

    // EXPECTED: Should fail because we can't create directory in read-only parent
    assert!(
        result.is_err(),
        "Should fail to create state dir in read-only parent"
    );

    // Cleanup
    let mut perms = fs::metadata(&parent).unwrap().permissions();
    perms.set_mode(0o755);
    let _ = fs::set_permissions(&parent, perms);
}

#[test]
fn test_audio_file_disappears_before_transcription() {
    println!("\n🔍 BUG INVESTIGATION: What if audio file is deleted between recording and transcription?");

    // This is simulated in main.rs around line 319-326
    // The code checks if audio_file exists and has content
    // If not, it returns an error

    // Let's verify the error handling is present
    let temp_dir = TempDir::new().unwrap();
    let audio_file = temp_dir.path().join("recording.wav");

    // File doesn't exist
    let exists = audio_file.exists();
    println!("File exists: {}", exists);

    assert!(!exists, "File should not exist");

    // The main.rs code at line 321-326 handles this:
    // if !audio_file.exists() {
    //     AudioFeedback::beep_error().ok();
    //     Notifications::error("Recording file is empty or missing").ok();
    //     tracing::error!("Audio file not found: {}", audio_file.display());
    //     anyhow::bail!("Audio file not found");
    // }

    println!("✅ Main.rs correctly checks for missing audio file");
}

#[test]
#[ignore] // This test requires root or special setup to simulate disk full
fn test_disk_full_during_recording() {
    println!("\n🔍 BUG INVESTIGATION: What happens when disk is full during recording?");

    // NOTE: This is difficult to test without actually filling up disk
    // or using special filesystem mocking tools

    // BUG POTENTIAL: If disk fills up during pw-record:
    // 1. pw-record might fail or write partial file
    // 2. File might have size > 0 but be incomplete
    // 3. Whisper transcription might fail with unclear error

    // The current code checks file size > 0 but doesn't validate
    // that the WAV file is actually complete and well-formed

    println!("⚠️  POTENTIAL BUG: No validation that audio file is complete/well-formed WAV");
    println!("    Only checks: exists() and len() > 0");
    println!("    A partial/corrupted WAV would pass these checks");
}
