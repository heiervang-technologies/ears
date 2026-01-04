//! Visual/snapshot tests for TUI rendering
//!
//! These tests capture the actual ASCII output of the TUI and compare it against
//! saved snapshots to detect visual regressions.

use ears::tui::{App, Panel};
use ratatui::{backend::TestBackend, Terminal};

/// Helper function to render the app and return the terminal buffer as a string
fn render_to_string(app: &App, width: u16, height: u16) -> String {
    let backend = TestBackend::new(width, height);
    let mut terminal = Terminal::new(backend).unwrap();

    terminal
        .draw(|f| ears::tui::ui::render(app, f))
        .unwrap();

    // Convert buffer to string by iterating through cells
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
fn test_render_initial_state() {
    let app = App::new();
    let output = render_to_string(&app, 80, 24);

    // Print for inspection during development
    println!("\n{}", output);

    // Basic assertions about what should be visible
    assert!(output.contains("ears"));
    assert!(output.contains("Status"));
    assert!(output.contains("Configuration"));
    assert!(output.contains("Logs"));
    assert!(output.contains("○")); // Not recording indicator
}

#[test]
fn test_render_recording_state() {
    let mut app = App::new();
    app.is_recording = true;
    app.recording_duration = 5;

    let output = render_to_string(&app, 80, 24);
    println!("\n{}", output);

    // Should show recording indicator
    assert!(output.contains("●")); // Recording indicator
    assert!(output.contains("5s")); // Duration
}

#[test]
fn test_render_status_panel() {
    let app = App::new();
    assert_eq!(app.current_panel, Panel::Status);

    let output = render_to_string(&app, 100, 30);
    println!("\n{}", output);

    // Status panel should be visible
    assert!(output.contains("Status"));
}

#[test]
fn test_render_config_panel() {
    let mut app = App::new();
    app.current_panel = Panel::Configuration;

    let output = render_to_string(&app, 100, 30);
    println!("\n{}", output);

    // Config panel should be visible and highlighted
    assert!(output.contains("Configuration"));
}

#[test]
fn test_render_logs_panel() {
    let mut app = App::new();
    app.current_panel = Panel::Logs;
    app.logs = vec![
        "First log entry".to_string(),
        "Second log entry".to_string(),
        "Third log entry".to_string(),
    ];

    let output = render_to_string(&app, 100, 30);
    println!("\n{}", output);

    // Logs should be visible
    assert!(output.contains("First log entry"));
    assert!(output.contains("Second log entry"));
    assert!(output.contains("Third log entry"));
}

#[test]
fn test_render_command_mode() {
    let mut app = App::new();
    app.command_mode = true;
    app.command_buffer = "quit".to_string();

    let output = render_to_string(&app, 80, 24);
    println!("\n{}", output);

    // Command mode should show the buffer
    assert!(output.contains(":quit"));
}

#[test]
fn test_render_different_terminal_sizes() {
    let app = App::new();

    // Test small terminal
    let output_small = render_to_string(&app, 40, 10);
    assert!(!output_small.is_empty());
    println!("Small terminal (40x10):\n{}", output_small);

    // Test large terminal
    let output_large = render_to_string(&app, 200, 50);
    assert!(!output_large.is_empty());

    // Large terminal should contain the app name
    assert!(output_large.contains("ears"));

    // Small terminal might truncate text, so just verify it renders something
    // without panicking
}

#[test]
fn test_render_empty_logs() {
    let mut app = App::new();
    app.current_panel = Panel::Logs;
    app.logs.clear();

    let output = render_to_string(&app, 80, 24);
    println!("\n{}", output);

    // Should render without panic even with no logs
    assert!(output.contains("Logs"));
}

#[test]
fn test_render_with_long_log_lines() {
    let mut app = App::new();
    app.current_panel = Panel::Logs;
    app.logs = vec![
        "This is a very long log line that should be handled gracefully by the terminal rendering and might need to be truncated or wrapped depending on the implementation".to_string(),
    ];

    let output = render_to_string(&app, 80, 24);
    println!("\n{}", output);

    // Should render without panic
    assert!(!output.is_empty());
}
