//! Snapshot tests for TUI rendering
//!
//! These tests use `insta` to capture snapshots of the TUI output.
//! Run `cargo insta review` to review and accept snapshot changes.
//!
//! NOTE: These tests use a temp HOME directory to ensure consistent
//! config values across different environments (local vs CI).

use ears::tui::{App, Panel};
use ratatui::{backend::TestBackend, Terminal};
use std::env;
use tempfile::TempDir;

/// Setup a clean environment for tests to get consistent config.
/// Returns (TempDir, original HOME, original XDG_CONFIG_HOME) for cleanup.
fn setup_test_env() -> (TempDir, Option<String>, Option<String>) {
    let original_home = env::var("HOME").ok();
    let original_xdg = env::var("XDG_CONFIG_HOME").ok();
    let temp_dir = TempDir::new().unwrap();
    // Set both HOME and XDG_CONFIG_HOME to ensure directories crate uses temp dir
    env::set_var("HOME", temp_dir.path());
    env::set_var("XDG_CONFIG_HOME", temp_dir.path().join(".config"));
    (temp_dir, original_home, original_xdg)
}

/// Restore environment variables after test
fn restore_test_env(original_home: Option<String>, original_xdg: Option<String>) {
    match original_home {
        Some(h) => env::set_var("HOME", h),
        None => env::remove_var("HOME"),
    }
    match original_xdg {
        Some(x) => env::set_var("XDG_CONFIG_HOME", x),
        None => env::remove_var("XDG_CONFIG_HOME"),
    }
}

/// Helper function to render the app and return the terminal buffer as a string
fn render_to_string(app: &mut App, width: u16, height: u16) -> String {
    let backend = TestBackend::new(width, height);
    let mut terminal = Terminal::new(backend).unwrap();

    app.clear_clickable_regions();
    terminal.draw(|f| ears::tui::ui::render(app, f)).unwrap();

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

    // Normalize variable content for consistent snapshots across versions
    // Replace the current version with a placeholder
    let version = env!("CARGO_PKG_VERSION");
    output
        .replace(&format!("v{}", version), "vX.X.X")
        // Replace "(connecting...)" or "unknown" model display with placeholder
        .replace("(connecting...)", "[MODEL]")
        .replace("unknown", "[MODEL]")
}

#[test]
#[serial_test::serial]
fn snapshot_initial_state() {
    let (_env, orig_home, orig_xdg) = setup_test_env();
    let mut app = App::new();
    let output = render_to_string(&mut app, 80, 24);
    restore_test_env(orig_home, orig_xdg);
    insta::assert_snapshot!(output);
}

#[test]
#[serial_test::serial]
fn snapshot_recording_state() {
    let (_env, orig_home, orig_xdg) = setup_test_env();
    let mut app = App::new();
    app.is_recording = true;
    app.recording_duration = 42;
    let output = render_to_string(&mut app, 80, 24);
    restore_test_env(orig_home, orig_xdg);
    insta::assert_snapshot!(output);
}

#[test]
#[serial_test::serial]
fn snapshot_status_panel() {
    let (_env, orig_home, orig_xdg) = setup_test_env();
    let mut app = App::new();
    let output = render_to_string(&mut app, 100, 30);
    restore_test_env(orig_home, orig_xdg);
    insta::assert_snapshot!(output);
}

#[test]
#[serial_test::serial]
fn snapshot_config_panel() {
    let (_env, orig_home, orig_xdg) = setup_test_env();
    let mut app = App::new();
    app.current_panel = Panel::Configuration;
    let output = render_to_string(&mut app, 100, 30);
    restore_test_env(orig_home, orig_xdg);
    insta::assert_snapshot!(output);
}

#[test]
#[serial_test::serial]
fn snapshot_logs_panel_with_content() {
    let (_env, orig_home, orig_xdg) = setup_test_env();
    let mut app = App::new();
    app.current_panel = Panel::Logs;
    app.logs = vec![
        "2024-01-04 12:00:00 - Recording started".to_string(),
        "2024-01-04 12:00:05 - Transcription: Hello world".to_string(),
        "2024-01-04 12:00:10 - Recording stopped".to_string(),
    ];
    let output = render_to_string(&mut app, 100, 30);
    restore_test_env(orig_home, orig_xdg);
    insta::assert_snapshot!(output);
}

#[test]
#[serial_test::serial]
fn snapshot_command_mode() {
    let (_env, orig_home, orig_xdg) = setup_test_env();
    let mut app = App::new();
    app.command_mode = true;
    app.command_buffer = "quit".to_string();
    let output = render_to_string(&mut app, 80, 24);
    restore_test_env(orig_home, orig_xdg);
    insta::assert_snapshot!(output);
}

#[test]
#[serial_test::serial]
fn snapshot_small_terminal() {
    let (_env, orig_home, orig_xdg) = setup_test_env();
    let mut app = App::new();
    let output = render_to_string(&mut app, 60, 15);
    restore_test_env(orig_home, orig_xdg);
    insta::assert_snapshot!(output);
}

#[test]
#[serial_test::serial]
fn snapshot_wide_terminal() {
    let (_env, orig_home, orig_xdg) = setup_test_env();
    let mut app = App::new();
    let output = render_to_string(&mut app, 120, 40);
    restore_test_env(orig_home, orig_xdg);
    insta::assert_snapshot!(output);
}
