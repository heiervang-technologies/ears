/// Test for potential race condition in state management
///
/// Issue: When the recording process is stopped, the state file might not be updated
/// to reflect the actual process state. If the process dies unexpectedly, the state
/// file could still say "Recording" even though the process is dead.
///
/// This test verifies whether StateManager properly handles the case where:
/// 1. Process is killed externally
/// 2. State file still says "Recording"
/// 3. Application restarts and loads state
use ears::{ProcessManager, State, StateManager};
use std::time::Duration;
use tempfile::TempDir;

#[test]
fn test_stale_state_after_process_death() {
    let temp_dir = TempDir::new().unwrap();
    let state_dir = temp_dir.path();
    let pid_file = state_dir.join("test.pid");

    // Setup: Start with Recording state
    let mut state_mgr = StateManager::new(state_dir).unwrap();
    state_mgr.transition(State::Recording).unwrap();

    // Verify state is Recording
    assert_eq!(state_mgr.current_state(), State::Recording);

    // Simulate process death by writing a dead PID
    let process_mgr = ProcessManager::new(&pid_file, Duration::from_secs(120));
    std::fs::write(&pid_file, "999999").unwrap(); // Non-existent PID

    // Now reload state in a "new" instance
    let mut new_state_mgr = StateManager::new(state_dir).unwrap();
    new_state_mgr.load_state().unwrap();

    // Before reconciliation: State file says "Recording" but process is dead
    assert_eq!(new_state_mgr.current_state(), State::Recording);
    assert_eq!(process_mgr.is_recording_alive().unwrap(), false);

    // FIX: Reconcile state with actual process status
    let was_reconciled = new_state_mgr
        .reconcile_state(|| {
            process_mgr
                .is_recording_alive()
                .map_err(|e| Box::new(e) as Box<dyn std::error::Error>)
        })
        .unwrap();

    // State should have been reconciled (reset to Idle)
    assert!(was_reconciled, "State should have been reconciled");
    assert_eq!(
        new_state_mgr.current_state(),
        State::Idle,
        "State should be Idle after reconciliation"
    );

    println!("BUG FIXED: State reconciled from Recording to Idle when process was dead");
}

#[test]
fn test_state_transition_without_process_check() {
    let temp_dir = TempDir::new().unwrap();
    let state_dir = temp_dir.path();

    let mut state_mgr = StateManager::new(state_dir).unwrap();

    // We can transition to Recording state...
    state_mgr.transition(State::Recording).unwrap();

    // But there's no guarantee that a recording process actually started!
    // The StateManager is decoupled from ProcessManager
    // This can lead to inconsistent state.

    assert_eq!(state_mgr.current_state(), State::Recording);
    println!("State transition succeeded without process validation");
}
