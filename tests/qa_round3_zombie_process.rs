/// QA Round 3: Zombie process bug
///
/// BUG FOUND: ProcessManager::spawn_recording() uses std::mem::forget(child)
/// which prevents the Child destructor from running. This creates zombie processes
/// that persist until the parent process exits.

use ears::ProcessManager;
use std::process::Command;
use std::time::Duration;
use tempfile::TempDir;

#[test]
fn test_zombie_process_bug() {
    println!("\n🐛 BUG: std::mem::forget(child) creates zombie processes");

    let temp_dir = TempDir::new().unwrap();
    let pid_file = temp_dir.path().join("test.pid");
    let audio_file = temp_dir.path().join("test.wav");

    let process_mgr = ProcessManager::new(&pid_file, Duration::from_secs(10));

    // Spawn a short-lived process
    let mut child = Command::new("sleep").arg("0.1").spawn().unwrap();
    let pid = child.id();

    println!("Spawned process with PID: {}", pid);

    // Simulate what ProcessManager does (line 74)
    std::mem::forget(child);

    // Wait for process to finish
    std::thread::sleep(Duration::from_millis(200));

    // Check process status
    let status_output = Command::new("ps")
        .arg("-p")
        .arg(pid.to_string())
        .arg("-o")
        .arg("stat=")
        .output()
        .unwrap();

    let status = String::from_utf8_lossy(&status_output.stdout);
    println!("Process status: '{}'", status.trim());

    // BUG: Process becomes zombie (Z) because we never wait() on it
    if status.contains("Z") {
        println!("🐛 CONFIRMED: Process became zombie!");
        println!("   Location: src/process.rs line 74");
        println!("   Issue: std::mem::forget(child) prevents Child::drop() from running");
        println!("   Result: Child::drop() normally calls wait(), cleaning up zombie");
    }
}

#[test]
fn test_correct_process_cleanup() {
    println!("\n✅ CORRECT: Without mem::forget, zombie is cleaned up");

    // Spawn a short-lived process
    let mut child = Command::new("sleep").arg("0.1").spawn().unwrap();
    let pid = child.id();

    println!("Spawned process with PID: {}", pid);

    // DON'T use mem::forget - let Child drop naturally
    drop(child);

    // Wait for process to finish
    std::thread::sleep(Duration::from_millis(200));

    // Check process status
    let status_output = Command::new("ps")
        .arg("-p")
        .arg(pid.to_string())
        .output()
        .unwrap();

    // Process should be gone (not even a zombie)
    let exit_code = status_output.status.code().unwrap_or(0);
    println!("ps exit code: {}", exit_code);

    // ps returns non-zero when process doesn't exist
    assert_ne!(exit_code, 0, "Process should be cleaned up completely");
}

#[test]
fn test_why_mem_forget_was_used() {
    println!("\n🔍 ANALYSIS: Why was std::mem::forget(child) used?");

    println!("\nOriginal problem:");
    println!("  - Need to keep pw-record running in background");
    println!("  - But don't want to block waiting for it");
    println!("  - If Child drops, it calls wait() which would block (or reap immediately)");

    println!("\nBetter solution:");
    println!("  - Spawn process and immediately detach it");
    println!("  - Or spawn in separate thread that handles cleanup");
    println!("  - Or use a background thread to monitor and reap");

    println!("\nCurrent consequence:");
    println!("  - Every recording creates a zombie when pw-record exits");
    println!("  - Zombies accumulate until ears process exits");
    println!("  - OS limit on processes can be reached with many recordings");
}

#[test]
fn demonstrate_solution() {
    println!("\n💡 SOLUTION: Spawn with proper cleanup");

    use std::thread;

    // Solution 1: Spawn in thread that waits
    let child = Command::new("sleep").arg("0.1").spawn().unwrap();
    let pid = child.id();

    println!("Spawned PID: {}", pid);

    // Spawn a thread to wait for the child
    thread::spawn(move || {
        let _ = child.wait_with_output();
        println!("Child {} cleaned up", pid);
    });

    // Main thread can continue without blocking
    std::thread::sleep(Duration::from_millis(200));

    println!("✅ This approach prevents zombies while allowing background execution");
}

#[test]
fn test_zombie_accumulation_scenario() {
    println!("\n🐛 SCENARIO: Multiple recordings create multiple zombies");

    let temp_dir = TempDir::new().unwrap();

    // Simulate multiple recording sessions
    let mut pids = Vec::new();

    for i in 0..5 {
        let mut child = Command::new("sleep").arg("0.1").spawn().unwrap();
        let pid = child.id();
        pids.push(pid);

        println!("Recording {}: spawned PID {}", i + 1, pid);

        // This is what ProcessManager does
        std::mem::forget(child);

        // Small delay between recordings
        std::thread::sleep(Duration::from_millis(50));
    }

    // Wait for all to finish
    std::thread::sleep(Duration::from_millis(300));

    // Count zombies
    let zombie_count = pids.iter().filter(|&&pid| {
        let output = Command::new("ps")
            .arg("-p")
            .arg(pid.to_string())
            .arg("-o")
            .arg("stat=")
            .output()
            .unwrap();

        let status = String::from_utf8_lossy(&output.stdout);
        status.contains("Z")
    }).count();

    println!("\n🐛 RESULT: {} out of {} processes became zombies", zombie_count, pids.len());

    if zombie_count > 0 {
        println!("   Impact: With heavy use, zombie processes accumulate");
        println!("   Severity: MEDIUM - zombies are cleaned up when ears exits");
        println!("   But: Long-running ears daemon could accumulate many zombies");
    }
}
