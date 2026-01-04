use ears::tui::{App, Panel};
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

#[test]
fn investigate_scroll_on_different_panels() {
    let mut app = App::new();
    app.logs = vec!["Log 1".to_string(), "Log 2".to_string(), "Log 3".to_string()];

    println!("\n=== Testing scroll behavior ===");
    println!("Initial - Panel: {:?}, selected_log: {}", app.current_panel, app.selected_log);

    // Try scrolling 'j' on Status panel
    println!("\n--- Scrolling on Status panel ---");
    for i in 0..3 {
        app.handle_key(KeyEvent::new(KeyCode::Char('j'), KeyModifiers::NONE)).ok();
        println!("After 'j' #{} on Status - selected_log: {}", i+1, app.selected_log);
    }

    // Switch to Logs panel
    app.current_panel = Panel::Logs;
    println!("\n--- Switched to Logs panel ---");
    println!("Before scrolling - selected_log: {}", app.selected_log);

    // Now try scrolling
    for i in 0..5 {
        let before = app.selected_log;
        app.handle_key(KeyEvent::new(KeyCode::Char('j'), KeyModifiers::NONE)).ok();
        let after = app.selected_log;
        println!("After 'j' #{}: {} -> {} (logs.len={})", i+1, before, after, app.logs.len());
    }

    println!("\nFinal selected_log: {} (max index should be {})", app.selected_log, app.logs.len()-1);

    // Verify it doesn't go beyond bounds
    assert!(app.selected_log < app.logs.len(), "BUG: selected_log went out of bounds!");

    // Try scrolling up
    println!("\n--- Scrolling up with 'k' ---");
    for i in 0..5 {
        let before = app.selected_log;
        app.handle_key(KeyEvent::new(KeyCode::Char('k'), KeyModifiers::NONE)).ok();
        let after = app.selected_log;
        println!("After 'k' #{}: {} -> {}", i+1, before, after);
    }

    println!("\nFinal selected_log: {} (min should be 0)", app.selected_log);
}

#[test]
fn investigate_scroll_on_empty_logs() {
    let mut app = App::new();
    app.logs.clear(); // No logs
    app.current_panel = Panel::Logs;

    println!("\n=== Scrolling with empty logs ===");
    println!("Initial selected_log: {} (logs.len={})", app.selected_log, app.logs.len());

    // Try scrolling down
    for _ in 0..3 {
        app.handle_key(KeyEvent::new(KeyCode::Char('j'), KeyModifiers::NONE)).ok();
    }

    println!("After scrolling down - selected_log: {}", app.selected_log);

    // Try scrolling up
    for _ in 0..3 {
        app.handle_key(KeyEvent::new(KeyCode::Char('k'), KeyModifiers::NONE)).ok();
    }

    println!("After scrolling up - selected_log: {}", app.selected_log);

    // Check for potential panic or out-of-bounds
    if app.logs.is_empty() && app.selected_log > 0 {
        panic!("BUG: selected_log is {} but logs are empty!", app.selected_log);
    }
}
