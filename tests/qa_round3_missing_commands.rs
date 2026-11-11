/// QA Round 3: a malicious device name must not inject a shell command
/// into the `pw-record` invocation.
use ears::ProcessManager;
use std::time::Duration;
use tempfile::TempDir;

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
