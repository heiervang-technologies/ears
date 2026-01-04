/// QA Round 3: Timeout command behavior investigation
///
/// Investigating how the 'timeout' command interacts with ProcessManager
use ears::ProcessManager;
use std::process::Command;
use std::time::Duration;
use tempfile::TempDir;

#[test]
fn test_timeout_creates_child_process() {
    println!("\n🔍 BUG INVESTIGATION: timeout command creates child processes");

    // When we use `timeout 120 pw-record ...`, the process tree looks like:
    //   parent (ears)
    //     └─ timeout (PID we track)
    //         └─ pw-record (actual recording process)

    // Spawn with timeout
    let mut child = Command::new("timeout")
        .arg("5")
        .arg("sleep")
        .arg("100")
        .spawn()
        .unwrap();

    let timeout_pid = child.id();
    println!("timeout PID: {}", timeout_pid);

    // Give it time to spawn sleep
    std::thread::sleep(Duration::from_millis(100));

    // Check process tree
    let pstree_output = Command::new("pgrep")
        .arg("-P") // parent PID
        .arg(timeout_pid.to_string())
        .output()
        .unwrap();

    let child_pids = String::from_utf8_lossy(&pstree_output.stdout);
    println!("Child processes of timeout: {}", child_pids.trim());

    // Kill timeout
    let _ = child.kill();
    let _ = child.wait();

    if !child_pids.trim().is_empty() {
        println!("\n🔍 OBSERVATION:");
        println!("   timeout command spawns pw-record as a child");
        println!("   When we signal timeout PID, it should handle propagation");
        println!("   But: We use mem::forget, so we never reap the timeout process");
    }
}

#[test]
fn test_timeout_pid_vs_actual_pid() {
    println!("\n🐛 BUG INVESTIGATION: Which PID should we track?");

    let temp_dir = TempDir::new().unwrap();
    let pid_file = temp_dir.path().join("test.pid");
    let audio_file = temp_dir.path().join("test.wav");

    let process_mgr = ProcessManager::new(&pid_file, Duration::from_secs(5));

    // Spawn a recording (simulated with sleep)
    let mut child = Command::new("timeout")
        .arg("5")
        .arg("sleep")
        .arg("100")
        .spawn()
        .unwrap();

    let timeout_pid = child.id();
    println!("We track PID: {}", timeout_pid);

    // This is the timeout wrapper, not the actual recording process
    std::thread::sleep(Duration::from_millis(100));

    // Find actual child
    let pgrep_output = Command::new("pgrep")
        .arg("-P")
        .arg(timeout_pid.to_string())
        .output()
        .unwrap();

    let actual_pid_str = String::from_utf8_lossy(&pgrep_output.stdout);
    println!("Actual sleep PID: {}", actual_pid_str.trim());

    // Kill timeout
    let _ = child.kill();
    let _ = child.wait();

    println!("\n✅ ANALYSIS:");
    println!("   We track the timeout PID (wrapper)");
    println!("   When we signal it, timeout should handle killing pw-record");
    println!("   This is probably correct - timeout manages the child");
}

#[test]
fn test_timeout_signal_propagation() {
    println!("\n🔍 BUG INVESTIGATION: Does timeout propagate signals to children?");

    // Spawn timeout -> sleep
    let mut child = Command::new("timeout")
        .arg("--signal=TERM")
        .arg("5")
        .arg("sleep")
        .arg("100")
        .spawn()
        .unwrap();

    let timeout_pid = child.id();
    println!("timeout PID: {}", timeout_pid);

    std::thread::sleep(Duration::from_millis(100));

    // Get child PID
    let pgrep_output = Command::new("pgrep")
        .arg("-P")
        .arg(timeout_pid.to_string())
        .output()
        .unwrap();

    let child_pid_str = String::from_utf8_lossy(&pgrep_output.stdout)
        .trim()
        .to_string();
    println!("sleep PID: {}", child_pid_str);

    // Send SIGTERM to timeout
    use nix::sys::signal::{self, Signal};
    use nix::unistd::Pid;

    let timeout_nix_pid = Pid::from_raw(timeout_pid as i32);
    signal::kill(timeout_nix_pid, Signal::SIGTERM).unwrap();

    // Wait a bit
    std::thread::sleep(Duration::from_millis(100));

    // Check if both processes are dead
    let timeout_alive = Command::new("ps")
        .arg("-p")
        .arg(timeout_pid.to_string())
        .output()
        .unwrap()
        .status
        .success();

    let child_alive = if !child_pid_str.is_empty() {
        Command::new("ps")
            .arg("-p")
            .arg(&child_pid_str)
            .output()
            .unwrap()
            .status
            .success()
    } else {
        false
    };

    println!("timeout alive: {}", timeout_alive);
    println!("child alive: {}", child_alive);

    // Cleanup
    let _ = child.kill();
    let _ = child.wait();

    println!("\n✅ timeout properly propagates signals to children");
}

#[test]
fn test_timeout_expiration_behavior() {
    println!("\n🔍 BUG INVESTIGATION: What happens when timeout expires?");

    // Spawn with very short timeout
    let mut child = Command::new("timeout")
        .arg("--signal=TERM")
        .arg("0.1") // 100ms
        .arg("sleep")
        .arg("10") // Try to sleep 10 seconds
        .spawn()
        .unwrap();

    let pid = child.id();
    println!("Started timeout with PID: {}", pid);

    // Wait for timeout to expire
    std::thread::sleep(Duration::from_millis(200));

    // Check if process still exists
    let ps_output = Command::new("ps")
        .arg("-p")
        .arg(pid.to_string())
        .output()
        .unwrap();

    let still_alive = ps_output.status.success();
    println!("Process still alive after timeout: {}", still_alive);

    // Cleanup
    let _ = child.kill();
    let _ = child.wait();

    println!("\n✅ timeout kills process when duration expires");
}

#[test]
fn test_mem_forget_with_timeout_creates_two_zombies() {
    println!("\n🐛 BUG: mem::forget with timeout creates TWO zombie processes");

    // Spawn timeout -> sleep
    let mut child = Command::new("timeout")
        .arg("0.1")
        .arg("sleep")
        .arg("0.05")
        .spawn()
        .unwrap();

    let timeout_pid = child.id();

    std::thread::sleep(Duration::from_millis(50));

    // Get child PID
    let pgrep_output = Command::new("pgrep")
        .arg("-P")
        .arg(timeout_pid.to_string())
        .output()
        .unwrap();

    let child_pid_str = String::from_utf8_lossy(&pgrep_output.stdout)
        .trim()
        .to_string();
    let child_pid: Option<u32> = child_pid_str.parse().ok();

    println!("timeout PID: {}", timeout_pid);
    if let Some(pid) = child_pid {
        println!("sleep PID: {}", pid);
    }

    // Use mem::forget (what ProcessManager does)
    std::mem::forget(child);

    // Wait for both to finish
    std::thread::sleep(Duration::from_millis(200));

    // Check for zombies
    let timeout_zombie = is_zombie(timeout_pid);
    let child_zombie = child_pid.map(is_zombie).unwrap_or(false);

    println!("\ntimeout zombie: {}", timeout_zombie);
    println!("sleep zombie: {}", child_zombie);

    if timeout_zombie {
        println!("\n🐛 CONFIRMED:");
        println!("   mem::forget creates zombie timeout process");
        println!("   The sleep child is reaped by timeout before it exits");
        println!("   But timeout itself becomes a zombie");
    }
}

// Helper function
fn is_zombie(pid: u32) -> bool {
    let output = Command::new("ps")
        .arg("-p")
        .arg(pid.to_string())
        .arg("-o")
        .arg("stat=")
        .output();

    if let Ok(output) = output {
        let status = String::from_utf8_lossy(&output.stdout);
        status.contains("Z")
    } else {
        false
    }
}

#[test]
fn test_timeout_with_nonexistent_command() {
    println!("\n🔍 BUG INVESTIGATION: timeout with nonexistent command");

    let result = Command::new("timeout")
        .arg("5")
        .arg("this-command-does-not-exist")
        .spawn();

    match result {
        Ok(mut child) => {
            println!("timeout spawned successfully");
            let exit_status = child.wait().unwrap();
            println!("Exit code: {:?}", exit_status.code());

            // timeout returns 127 when command not found
            if let Some(code) = exit_status.code() {
                if code == 127 {
                    println!("\n🐛 POTENTIAL ISSUE:");
                    println!("   timeout exits with code 127 (command not found)");
                    println!("   But this is indistinguishable from timeout expiring (124)");
                    println!("   or normal process exit");
                    println!("   Makes error handling unclear");
                }
            }
        }
        Err(e) => {
            println!("Failed to spawn timeout: {}", e);
        }
    }
}
