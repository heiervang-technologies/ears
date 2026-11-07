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

/// Returns true if running as root (uid 0). Permission tests don't work as root.
fn is_root() -> bool {
    unsafe { libc::getuid() == 0 }
}

#[test]
fn test_state_dir_readonly() {
    if is_root() {
        println!("Skipping test: permission tests don't work as root");
        return;
    }
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
    if is_root() {
        println!("Skipping test: permission tests don't work as root");
        return;
    }
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
    assert!(
        result.is_err(),
        "Should fail to save config to read-only dir"
    );

    if let Err(e) = result {
        println!("Error: {}", e);
    }

    // Cleanup
    let mut perms = fs::metadata(&config_dir).unwrap().permissions();
    perms.set_mode(0o755);
    let _ = fs::set_permissions(&config_dir, perms);
}

#[test]
fn test_state_dir_creation_in_unwritable_parent() {
    if is_root() {
        println!("Skipping test: permission tests don't work as root");
        return;
    }
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
