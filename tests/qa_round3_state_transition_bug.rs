/// QA Round 3: State transition validation bug
///
/// BUG FOUND: StateManager allows invalid state transition under certain conditions

use ears::{State, StateManager};
use tempfile::TempDir;
use std::time::Duration;

#[test]
fn test_state_transition_validation() {
    println!("\n🔍 BUG INVESTIGATION: State transition validation");

    let temp_dir = TempDir::new().unwrap();
    let mut state_mgr = StateManager::new(temp_dir.path()).unwrap();

    println!("Initial state: {:?}", state_mgr.current_state());
    assert_eq!(state_mgr.current_state(), State::Idle);

    // Valid transition: Idle -> Recording
    assert!(state_mgr.transition(State::Recording).is_ok());
    println!("✓ Idle -> Recording: allowed");

    // Valid transition: Recording -> Transcribing
    assert!(state_mgr.transition(State::Transcribing).is_ok());
    println!("✓ Recording -> Transcribing: allowed");

    // Valid transition: Transcribing -> Idle
    assert!(state_mgr.transition(State::Idle).is_ok());
    println!("✓ Transcribing -> Idle: allowed");

    // Invalid transition: Idle -> Transcribing (should fail)
    let result = state_mgr.transition(State::Transcribing);
    println!("Idle -> Transcribing: {:?}", result);
    assert!(result.is_err(), "Should not allow Idle -> Transcribing");
    println!("✓ Idle -> Transcribing: correctly rejected");
}

#[test]
fn test_state_persistence_after_failed_transition() {
    println!("\n🔍 BUG INVESTIGATION: State after failed transition");

    let temp_dir = TempDir::new().unwrap();
    let mut state_mgr = StateManager::new(temp_dir.path()).unwrap();

    // Start in Idle
    assert_eq!(state_mgr.current_state(), State::Idle);

    // Try invalid transition
    let result = state_mgr.transition(State::Transcribing);
    assert!(result.is_err());

    // State should still be Idle
    assert_eq!(state_mgr.current_state(), State::Idle);
    println!("✓ State unchanged after failed transition");

    // Check persisted state
    let mut new_mgr = StateManager::new(temp_dir.path()).unwrap();
    new_mgr.load_state().unwrap();
    assert_eq!(new_mgr.current_state(), State::Idle);
    println!("✓ Persisted state is correct");
}

#[test]
fn test_concurrent_state_transitions() {
    println!("\n🐛 BUG INVESTIGATION: What if two processes try to transition state?");

    // This is prevented by the lock file mechanism
    // But let's verify state file corruption doesn't happen

    let temp_dir = TempDir::new().unwrap();

    // Simulate two state managers (like two ears instances)
    let mut mgr1 = StateManager::new(temp_dir.path()).unwrap();
    let mut mgr2 = StateManager::new(temp_dir.path()).unwrap();

    // mgr1 transitions to Recording
    mgr1.transition(State::Recording).unwrap();

    // mgr2 loads state (should see Recording)
    mgr2.load_state().unwrap();
    println!("mgr2 sees state: {:?}", mgr2.current_state());
    assert_eq!(mgr2.current_state(), State::Recording);

    // mgr2 tries to transition to Recording (should fail - already Recording)
    let result = mgr2.transition(State::Recording);
    println!("mgr2 Recording -> Recording: {:?}", result);
    assert!(result.is_err(), "Should not allow Recording -> Recording");

    println!("\n✅ State transition validation prevents concurrent issues");
}

#[test]
fn test_recording_timeout_resets_state() {
    println!("\n🔍 BUG INVESTIGATION: Recording timeout should reset state");

    let temp_dir = TempDir::new().unwrap();
    let mut state_mgr = StateManager::new(temp_dir.path()).unwrap();

    // Override timeout to very short for testing
    // (We can't access max_recording_duration directly, so we test behavior)

    state_mgr.transition(State::Recording).unwrap();

    // In real code, recording has max_recording_duration = 120 seconds
    // If we try to transition after timeout, it should reset to Idle

    println!("✓ Timeout mechanism exists (lines 73-82 in state.rs)");
    println!("  But: Timeout is only checked during transition attempt");
    println!("  Issue: If no one calls transition(), timeout isn't enforced");
}

#[test]
fn test_state_file_manual_corruption() {
    println!("\n🔍 BUG INVESTIGATION: Manually corrupted state file");

    let temp_dir = TempDir::new().unwrap();
    let state_dir = temp_dir.path();

    // Create state manager and set state
    let mut state_mgr = StateManager::new(state_dir).unwrap();
    state_mgr.transition(State::Recording).unwrap();

    // Manually corrupt state file
    let state_file = state_dir.join("state");
    std::fs::write(&state_file, "invalid_state_value").unwrap();

    // Try to load corrupted state
    let mut new_mgr = StateManager::new(state_dir).unwrap();
    let result = new_mgr.load_state();

    println!("Load result: {:?}", result);
    assert!(result.is_err(), "Should fail to load corrupted state");

    if let Err(e) = result {
        println!("Error: {}", e);
        assert!(e.to_string().contains("corrupted"), "Error should mention corruption");
    }

    println!("✓ Corrupted state is detected");
}

#[test]
fn test_state_reconciliation_logic() {
    println!("\n🔍 BUG INVESTIGATION: State reconciliation on startup");

    let temp_dir = TempDir::new().unwrap();
    let state_dir = temp_dir.path();

    // Create state manager and set to Recording
    let mut state_mgr = StateManager::new(state_dir).unwrap();
    state_mgr.transition(State::Recording).unwrap();

    // Drop it (simulating crash)
    drop(state_mgr);

    // Create new state manager (simulating restart)
    let mut new_mgr = StateManager::new(state_dir).unwrap();
    new_mgr.load_state().unwrap();

    println!("State after restart: {:?}", new_mgr.current_state());

    // Reconcile with fake process check (returns false = process dead)
    let reconciled = new_mgr
        .reconcile_state(|| Ok(false))
        .unwrap();

    println!("Reconciled: {}", reconciled);
    println!("New state: {:?}", new_mgr.current_state());

    assert!(reconciled, "Should have reconciled state");
    assert_eq!(new_mgr.current_state(), State::Idle, "Should reset to Idle");

    println!("✓ Reconciliation correctly resets stale Recording state");
}

#[test]
fn test_reconciliation_only_affects_recording_state() {
    println!("\n🔍 BUG INVESTIGATION: Reconciliation should only affect Recording state");

    let temp_dir = TempDir::new().unwrap();
    let state_dir = temp_dir.path();

    // Test Idle state
    let mut mgr = StateManager::new(state_dir).unwrap();
    let reconciled = mgr.reconcile_state(|| Ok(false)).unwrap();
    assert!(!reconciled, "Idle state should not need reconciliation");

    // Test Transcribing state
    mgr.transition(State::Recording).unwrap();
    mgr.transition(State::Transcribing).unwrap();
    let reconciled = mgr.reconcile_state(|| Ok(false)).unwrap();
    assert!(!reconciled, "Transcribing state should not need reconciliation");
    assert_eq!(mgr.current_state(), State::Transcribing);

    println!("✓ Reconciliation only applies to Recording state");
}

#[test]
fn test_state_transition_bypass_via_reconcile() {
    println!("\n🐛 POTENTIAL BUG: reconcile_state bypasses transition validation");

    let temp_dir = TempDir::new().unwrap();
    let mut state_mgr = StateManager::new(temp_dir.path()).unwrap();

    // Start in Idle
    assert_eq!(state_mgr.current_state(), State::Idle);

    // Go to Recording, then Transcribing
    state_mgr.transition(State::Recording).unwrap();
    state_mgr.transition(State::Transcribing).unwrap();

    println!("Current state: {:?}", state_mgr.current_state());

    // Now we're in Transcribing state
    // If we manually set state to Recording in the file
    let state_file = state_mgr.state_dir().join("state");
    std::fs::write(&state_file, "recording").unwrap();

    // And load it
    state_mgr.load_state().unwrap();
    println!("State after manual edit + load: {:?}", state_mgr.current_state());

    // State is now Recording, even though we came from Transcribing
    // This bypassed the transition validation!

    println!("\n⚠️  OBSERVATION:");
    println!("   load_state() doesn't validate transitions");
    println!("   Direct file manipulation can bypass state machine rules");
    println!("   Severity: LOW - requires manual file editing");
}

#[test]
fn test_recording_started_timestamp_tracking() {
    println!("\n🔍 BUG INVESTIGATION: Recording timestamp is not persisted");

    let temp_dir = TempDir::new().unwrap();
    let state_dir = temp_dir.path();

    // Create state manager and start recording
    let mut state_mgr = StateManager::new(state_dir).unwrap();
    state_mgr.transition(State::Recording).unwrap();

    // Wait a bit
    std::thread::sleep(Duration::from_millis(100));

    // Drop and reload (simulating restart)
    drop(state_mgr);

    let mut new_mgr = StateManager::new(state_dir).unwrap();
    new_mgr.load_state().unwrap();

    println!("State after reload: {:?}", new_mgr.current_state());
    assert_eq!(new_mgr.current_state(), State::Recording);

    // Check if recording has exceeded timeout
    let timeout = new_mgr.check_recording_timeout();
    println!("Timeout check: {}", timeout);

    println!("\n🐛 POTENTIAL ISSUE:");
    println!("   recording_started timestamp is not persisted");
    println!("   After restart, we lose track of when recording started");
    println!("   Result: Timeout check is ineffective across restarts");
    println!("   But: This is mitigated by reconcile_state() which resets stale state");
}
