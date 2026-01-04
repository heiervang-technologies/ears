//! Potential bug: When logs are added via toggle_recording, selected_log doesn't update

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ears::tui::{App, Panel};

#[test]
fn bug_selected_log_doesnt_follow_new_logs() {
    let mut app = App::new();
    app.current_panel = Panel::Logs;

    println!("\n=== Testing log scroll behavior when logs are added ===");

    // Initial state
    let initial_logs = app.logs.len();
    println!(
        "Initial: {} logs, selected_log: {}",
        initial_logs, app.selected_log
    );

    // Scroll to the last log
    while app.selected_log < app.logs.len() - 1 {
        app.handle_key(KeyEvent::new(KeyCode::Char('j'), KeyModifiers::NONE))
            .ok();
    }
    println!(
        "After scrolling to bottom: selected_log: {} (last index: {})",
        app.selected_log,
        app.logs.len() - 1
    );

    assert_eq!(
        app.selected_log,
        app.logs.len() - 1,
        "Should be at last log"
    );

    // Now toggle recording (which adds a log)
    app.handle_key(KeyEvent::new(KeyCode::Char(' '), KeyModifiers::NONE))
        .ok();

    let new_logs = app.logs.len();
    println!(
        "After toggle recording: {} logs, selected_log: {}",
        new_logs, app.selected_log
    );

    // BUG?: selected_log is still pointing to the old last log,
    // not the new last log that was just added
    if app.selected_log != app.logs.len() - 1 {
        println!("\n⚠️  POTENTIAL UX ISSUE:");
        println!("   User was viewing the last log");
        println!("   A new log was added: '{}'", app.logs.last().unwrap());
        println!("   But selected_log didn't move to show the new log");
        println!("   User might not notice the new log entry!");
    }

    // Add more logs
    for _ in 0..5 {
        app.handle_key(KeyEvent::new(KeyCode::Char(' '), KeyModifiers::NONE))
            .ok();
    }

    println!("\nAfter adding more logs:");
    println!("  Total logs: {}", app.logs.len());
    println!("  selected_log: {}", app.selected_log);
    println!("  Last 3 logs:");
    for (i, log) in app.logs.iter().rev().take(3).enumerate() {
        let idx = app.logs.len() - 1 - i;
        let marker = if idx == app.selected_log {
            "👉"
        } else {
            "  "
        };
        println!("    {} [{}] {}", marker, idx, log);
    }
}
