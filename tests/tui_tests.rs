//! Tests for TUI functionality

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ears::tui::{App, Panel};

#[test]
fn test_app_initialization() {
    let app = App::new();
    assert_eq!(app.current_panel, Panel::LiveTranscription);
    assert!(!app.command_mode);
}

#[test]
fn test_panel_navigation_next() {
    let mut app = App::new();

    // Start at LiveTranscription
    assert_eq!(app.current_panel, Panel::LiveTranscription);

    // Press 'l' to go to next panel (Configuration)
    let key = KeyEvent::new(KeyCode::Char('l'), KeyModifiers::NONE);
    let result = app.handle_key(key);
    assert!(result.is_ok());
    assert!(result.unwrap());
    assert_eq!(app.current_panel, Panel::Configuration);

    // Press 'l' again to go to Logs
    let result = app.handle_key(key);
    assert!(result.is_ok());
    assert!(result.unwrap());
    assert_eq!(app.current_panel, Panel::Logs);

    // Press 'l' again to wrap back to LiveTranscription
    let result = app.handle_key(key);
    assert!(result.is_ok());
    assert!(result.unwrap());
    assert_eq!(app.current_panel, Panel::LiveTranscription);
}

#[test]
fn test_panel_navigation_prev() {
    let mut app = App::new();

    // Start at LiveTranscription
    assert_eq!(app.current_panel, Panel::LiveTranscription);

    // Press 'h' to go to previous panel (Logs - wraps around)
    let key = KeyEvent::new(KeyCode::Char('h'), KeyModifiers::NONE);
    let result = app.handle_key(key);
    assert!(result.is_ok());
    assert!(result.unwrap());
    assert_eq!(app.current_panel, Panel::Logs);

    // Press 'h' again to go to Configuration
    let result = app.handle_key(key);
    assert!(result.is_ok());
    assert!(result.unwrap());
    assert_eq!(app.current_panel, Panel::Configuration);

    // Press 'h' again to wrap back to LiveTranscription
    let result = app.handle_key(key);
    assert!(result.is_ok());
    assert!(result.unwrap());
    assert_eq!(app.current_panel, Panel::LiveTranscription);
}

#[test]
fn test_tab_navigation() {
    let mut app = App::new();

    // Press Tab to go to next panel
    let key = KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE);
    let result = app.handle_key(key);
    assert!(result.is_ok());
    assert!(result.unwrap());
    assert_eq!(app.current_panel, Panel::Configuration);

    // Press Shift+Tab to go back
    let key = KeyEvent::new(KeyCode::BackTab, KeyModifiers::SHIFT);
    let result = app.handle_key(key);
    assert!(result.is_ok());
    assert!(result.unwrap());
    assert_eq!(app.current_panel, Panel::LiveTranscription);
}

#[test]
fn test_quit_with_q() {
    let mut app = App::new();

    let key = KeyEvent::new(KeyCode::Char('q'), KeyModifiers::NONE);
    let result = app.handle_key(key);
    assert!(result.is_ok());
    assert!(!result.unwrap()); // Should return false to quit
}

#[test]
fn test_quit_with_ctrl_c() {
    let mut app = App::new();

    let key = KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL);
    let result = app.handle_key(key);
    assert!(result.is_ok());
    assert!(!result.unwrap()); // Should return false to quit
}

#[test]
fn test_command_mode_entry() {
    let mut app = App::new();
    assert!(!app.command_mode);

    // Press ':' to enter command mode
    let key = KeyEvent::new(KeyCode::Char(':'), KeyModifiers::NONE);
    let result = app.handle_key(key);
    assert!(result.is_ok());
    assert!(result.unwrap());
    assert!(app.command_mode);
    assert_eq!(app.command_buffer, "");
}

#[test]
fn test_command_mode_typing() {
    let mut app = App::new();

    // Enter command mode
    let key = KeyEvent::new(KeyCode::Char(':'), KeyModifiers::NONE);
    app.handle_key(key).unwrap();

    // Type 'q'
    let key = KeyEvent::new(KeyCode::Char('q'), KeyModifiers::NONE);
    app.handle_key(key).unwrap();
    assert_eq!(app.command_buffer, "q");

    // Type 'u'
    let key = KeyEvent::new(KeyCode::Char('u'), KeyModifiers::NONE);
    app.handle_key(key).unwrap();
    assert_eq!(app.command_buffer, "qu");

    // Press backspace
    let key = KeyEvent::new(KeyCode::Backspace, KeyModifiers::NONE);
    app.handle_key(key).unwrap();
    assert_eq!(app.command_buffer, "q");
}

#[test]
fn test_command_mode_quit() {
    let mut app = App::new();

    // Enter command mode and type ':q'
    app.handle_key(KeyEvent::new(KeyCode::Char(':'), KeyModifiers::NONE))
        .unwrap();
    app.handle_key(KeyEvent::new(KeyCode::Char('q'), KeyModifiers::NONE))
        .unwrap();

    // Press Enter to execute
    let key = KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE);
    let result = app.handle_key(key);
    assert!(result.is_ok());
    assert!(!result.unwrap()); // Should quit
    assert!(!app.command_mode); // Should exit command mode
}

#[test]
fn test_command_mode_escape() {
    let mut app = App::new();

    // Enter command mode
    app.handle_key(KeyEvent::new(KeyCode::Char(':'), KeyModifiers::NONE))
        .unwrap();
    app.handle_key(KeyEvent::new(KeyCode::Char('q'), KeyModifiers::NONE))
        .unwrap();
    assert!(app.command_mode);
    assert_eq!(app.command_buffer, "q");

    // Press Escape to cancel
    let key = KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE);
    let result = app.handle_key(key);
    assert!(result.is_ok());
    assert!(result.unwrap()); // Should continue
    assert!(!app.command_mode);
    assert_eq!(app.command_buffer, "");
}

#[test]
fn test_vad_toggle() {
    let mut app = App::new();
    assert!(!app.vad_active);

    // Press Space to enable VAD
    let key = KeyEvent::new(KeyCode::Char(' '), KeyModifiers::NONE);
    app.handle_key(key).unwrap();
    assert!(app.vad_active);

    // Press Space again to disable VAD
    app.handle_key(key).unwrap();
    assert!(!app.vad_active);
}

#[test]
fn test_panel_titles() {
    assert_eq!(Panel::Configuration.title(), "Configuration");
    assert_eq!(Panel::Logs.title(), "Logs");
    assert_eq!(Panel::LiveTranscription.title(), "Live");
}

#[test]
fn test_go_to_config_with_c() {
    let mut app = App::new();
    app.current_panel = Panel::Logs;

    // Press 'c' to jump to Configuration panel
    let key = KeyEvent::new(KeyCode::Char('c'), KeyModifiers::NONE);
    app.handle_key(key).unwrap();
    assert_eq!(app.current_panel, Panel::Configuration);
}

#[test]
fn test_log_scrolling() {
    let mut app = App::new();
    app.current_panel = Panel::Logs;

    // Add some logs
    app.logs = vec![
        "Log 1".to_string(),
        "Log 2".to_string(),
        "Log 3".to_string(),
    ];
    app.selected_log = 0;

    // Press 'j' to scroll down
    let key = KeyEvent::new(KeyCode::Char('j'), KeyModifiers::NONE);
    app.handle_key(key).unwrap();
    assert_eq!(app.selected_log, 1);

    app.handle_key(key).unwrap();
    assert_eq!(app.selected_log, 2);

    // Try to scroll past the end
    app.handle_key(key).unwrap();
    assert_eq!(app.selected_log, 2); // Should stay at last item

    // Press 'k' to scroll up
    let key = KeyEvent::new(KeyCode::Char('k'), KeyModifiers::NONE);
    app.handle_key(key).unwrap();
    assert_eq!(app.selected_log, 1);

    app.handle_key(key).unwrap();
    assert_eq!(app.selected_log, 0);

    // Try to scroll past the beginning
    app.handle_key(key).unwrap();
    assert_eq!(app.selected_log, 0); // Should stay at first item
}

#[test]
fn test_command_mode_write() {
    let mut app = App::new();
    let initial_log_count = app.logs.len();

    // Enter command mode and type ':w'
    app.handle_key(KeyEvent::new(KeyCode::Char(':'), KeyModifiers::NONE))
        .unwrap();
    app.handle_key(KeyEvent::new(KeyCode::Char('w'), KeyModifiers::NONE))
        .unwrap();

    // Press Enter to execute
    let key = KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE);
    let result = app.handle_key(key);
    assert!(result.is_ok());
    assert!(result.unwrap()); // Should continue (not quit)
    assert!(!app.command_mode);

    // Should have added a log message
    assert_eq!(app.logs.len(), initial_log_count + 1);
    assert!(app.logs.last().unwrap().contains("saved"));
}

#[test]
fn test_command_mode_write_quit() {
    let mut app = App::new();

    // Enter command mode and type ':wq'
    app.handle_key(KeyEvent::new(KeyCode::Char(':'), KeyModifiers::NONE))
        .unwrap();
    app.handle_key(KeyEvent::new(KeyCode::Char('w'), KeyModifiers::NONE))
        .unwrap();
    app.handle_key(KeyEvent::new(KeyCode::Char('q'), KeyModifiers::NONE))
        .unwrap();

    // Press Enter to execute
    let key = KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE);
    let result = app.handle_key(key);
    assert!(result.is_ok());
    assert!(!result.unwrap()); // Should quit
}
