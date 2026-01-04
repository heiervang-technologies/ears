//! Edge Case Testing - Try to break the TUI with extreme values
//!
//! Tests unusual scenarios that might not occur during normal exploration

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ears::tui::App;
use ratatui::{backend::TestBackend, Terminal};

fn render_to_string(app: &App, width: u16, height: u16) -> String {
    let backend = TestBackend::new(width, height);
    let mut terminal = Terminal::new(backend).unwrap();

    terminal
        .draw(|f| ears::tui::ui::render(app, f))
        .unwrap();

    let buffer = terminal.backend().buffer();
    let mut output = String::new();

    for y in 0..buffer.area().height {
        for x in 0..buffer.area().width {
            let cell = &buffer[(x, y)];
            output.push_str(cell.symbol());
        }
        if y < buffer.area().height - 1 {
            output.push('\n');
        }
    }

    output
}

#[test]
fn edge_case_tiny_terminal() {
    println!("\n🔍 EDGE CASE: Tiny terminal (10x5)");

    let app = App::new();
    let output = render_to_string(&app, 10, 5);

    println!("{}", output);

    assert!(!output.is_empty(), "Should render something even in tiny terminal");
}

#[test]
fn edge_case_very_wide_terminal() {
    println!("\n🔍 EDGE CASE: Very wide terminal (300x50)");

    let app = App::new();
    let output = render_to_string(&app, 300, 50);

    assert!(!output.is_empty());
    assert!(output.contains("ears"));
}

#[test]
fn edge_case_very_tall_terminal() {
    println!("\n🔍 EDGE CASE: Very tall terminal (80x100)");

    let app = App::new();
    let output = render_to_string(&app, 80, 100);

    assert!(!output.is_empty());
    assert!(output.contains("ears"));
}

#[test]
fn edge_case_many_logs() {
    println!("\n🔍 EDGE CASE: 1000 log entries");

    let mut app = App::new();

    // Add 1000 log entries
    for i in 0..1000 {
        app.logs.push(format!("Log entry {}", i));
    }

    let output = render_to_string(&app, 80, 24);

    assert!(!output.is_empty());
    assert!(output.contains("Logs") || output.contains("Status"));
}

#[test]
fn edge_case_very_long_log_line() {
    println!("\n🔍 EDGE CASE: Log line with 1000 characters");

    let mut app = App::new();
    app.logs.push("A".repeat(1000));

    let output = render_to_string(&app, 80, 24);

    assert!(!output.is_empty());
    // Should not crash, even if truncated
}

#[test]
fn edge_case_special_characters_in_logs() {
    println!("\n🔍 EDGE CASE: Special characters in logs");

    let mut app = App::new();
    app.logs.push("Special: \t\n\r\0".to_string());
    app.logs.push("Unicode: 你好世界 🚀 ñ ü".to_string());
    app.logs.push("Symbols: <>&\"'".to_string());

    let output = render_to_string(&app, 80, 24);

    assert!(!output.is_empty());
    println!("Rendered successfully with special characters");
}

#[test]
fn edge_case_very_long_command_buffer() {
    println!("\n🔍 EDGE CASE: Very long command buffer");

    let mut app = App::new();
    app.command_mode = true;
    app.command_buffer = "x".repeat(200);

    let output = render_to_string(&app, 80, 24);

    assert!(!output.is_empty());
    // Should not crash even if truncated
}

#[test]
fn edge_case_max_recording_duration() {
    println!("\n🔍 EDGE CASE: Very long recording duration");

    let mut app = App::new();
    app.is_recording = true;
    app.recording_duration = u64::MAX;

    let output = render_to_string(&app, 80, 24);

    assert!(!output.is_empty());
    assert!(output.contains("●")); // Should show recording indicator
    println!("Max duration value: {}", u64::MAX);
}

#[test]
fn edge_case_scroll_beyond_bounds() {
    println!("\n🔍 EDGE CASE: Scroll beyond log bounds");

    let mut app = App::new();
    app.logs = vec!["Log 1".to_string(), "Log 2".to_string()];

    // Try scrolling down many times
    for _ in 0..100 {
        app.handle_key(KeyEvent::new(KeyCode::Char('j'), KeyModifiers::NONE))
            .ok();
    }

    println!("Selected log: {} (total logs: {})", app.selected_log, app.logs.len());

    // Should not panic and should stay within bounds
    assert!(app.selected_log < app.logs.len(), "selected_log should stay within bounds");

    let output = render_to_string(&app, 80, 24);
    assert!(!output.is_empty());
}

#[test]
fn edge_case_empty_logs() {
    println!("\n🔍 EDGE CASE: No logs at all");

    let mut app = App::new();
    app.logs.clear();
    app.current_panel = ears::tui::Panel::Logs;

    let output = render_to_string(&app, 80, 24);

    assert!(!output.is_empty());
    assert!(output.contains("Logs"));
}

#[test]
fn edge_case_rapid_panel_switching() {
    println!("\n🔍 EDGE CASE: Rapid panel switching");

    let mut app = App::new();

    // Switch panels 1000 times
    for _ in 0..1000 {
        app.handle_key(KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE))
            .ok();
    }

    let output = render_to_string(&app, 80, 24);
    assert!(!output.is_empty());

    println!("Final panel: {:?}", app.current_panel);
}

#[test]
fn edge_case_command_mode_spam() {
    println!("\n🔍 EDGE CASE: Spam enter command mode");

    let mut app = App::new();

    // Enter command mode many times
    for _ in 0..100 {
        app.handle_key(KeyEvent::new(KeyCode::Char(':'), KeyModifiers::NONE))
            .ok();
    }

    let output = render_to_string(&app, 80, 24);
    assert!(!output.is_empty());

    println!("Command mode: {}", app.command_mode);
}

#[test]
fn edge_case_mixed_operations() {
    println!("\n🔍 EDGE CASE: Random mixed operations");

    let mut app = App::new();

    let operations = vec![
        KeyEvent::new(KeyCode::Char('l'), KeyModifiers::NONE),
        KeyEvent::new(KeyCode::Char(' '), KeyModifiers::NONE),
        KeyEvent::new(KeyCode::Char(':'), KeyModifiers::NONE),
        KeyEvent::new(KeyCode::Char('w'), KeyModifiers::NONE),
        KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE),
        KeyEvent::new(KeyCode::Char('j'), KeyModifiers::NONE),
        KeyEvent::new(KeyCode::Char('k'), KeyModifiers::NONE),
        KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE),
    ];

    // Repeat 50 times
    for _ in 0..50 {
        for op in &operations {
            app.handle_key(*op).ok();
        }
    }

    let output = render_to_string(&app, 80, 24);
    assert!(!output.is_empty());

    println!("Final state - Panel: {:?}, Recording: {}, Command: {}",
             app.current_panel, app.is_recording, app.command_mode);
}

#[test]
fn edge_case_backspace_on_empty_buffer() {
    println!("\n🔍 EDGE CASE: Backspace on empty command buffer");

    let mut app = App::new();
    app.command_mode = true;
    app.command_buffer = String::new();

    // Backspace many times on empty buffer
    for _ in 0..50 {
        app.handle_key(KeyEvent::new(KeyCode::Backspace, KeyModifiers::NONE))
            .ok();
    }

    assert_eq!(app.command_buffer, "", "Buffer should remain empty");

    let output = render_to_string(&app, 80, 24);
    assert!(!output.is_empty());
}
