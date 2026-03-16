use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
/// QA Agent Investigation - Finding real bugs in the TUI
///
/// This test file investigates potential bugs in the ears TUI
use ears::tui::{App, Panel};

#[test]
fn test_keybinding_conflict_c_key() {
    // BUG INVESTIGATION: The 'c' key has a potential conflict
    // - Ctrl+C should quit the app
    // - 'c' alone jumps to configuration panel
    //
    // This test verifies both behaviors work correctly
    let mut app = App::new();

    // Start on Configuration panel
    assert_eq!(app.current_panel, Panel::Configuration);

    // Press 'c' alone - should jump to Configuration panel
    let key_c = KeyEvent::new(KeyCode::Char('c'), KeyModifiers::NONE);
    let result = app.handle_key(key_c).unwrap();
    assert!(result, "Pressing 'c' should continue the app");
    assert_eq!(
        app.current_panel,
        Panel::Configuration,
        "'c' should jump to Configuration panel"
    );

    // Press Ctrl+C - should quit
    let key_ctrl_c = KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL);
    let result = app.handle_key(key_ctrl_c).unwrap();
    assert!(!result, "Ctrl+C should quit the app");

    println!("✓ 'c' key behavior is correct - no conflict");
}

#[test]
fn test_empty_command_handling() {
    // BUG INVESTIGATION: What happens when user enters command mode
    // and presses Enter with an empty command?
    let mut app = App::new();

    // Enter command mode
    let key_colon = KeyEvent::new(KeyCode::Char(':'), KeyModifiers::NONE);
    app.handle_key(key_colon).unwrap();
    assert!(app.command_mode, "Should be in command mode");
    assert_eq!(app.command_buffer, "", "Command buffer should be empty");

    // Press Enter immediately (empty command)
    let key_enter = KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE);
    let result = app.handle_key(key_enter).unwrap();

    // What happens? Let's check
    assert!(result, "Empty command should not quit the app");
    assert!(!app.command_mode, "Should exit command mode");

    // Check that no log was added (empty commands are silently ignored)
    let last_log = app.logs.last().unwrap();
    println!("Last log after empty command: {}", last_log);

    // FIXED: Empty commands are now silently ignored
    assert_eq!(
        last_log, "TUI initialized",
        "Empty command should not add a log entry"
    );
}

#[test]
fn test_whitespace_only_command() {
    // BUG INVESTIGATION: What happens with whitespace-only commands?
    let mut app = App::new();

    // Enter command mode and type spaces
    let key_colon = KeyEvent::new(KeyCode::Char(':'), KeyModifiers::NONE);
    app.handle_key(key_colon).unwrap();

    // Type several spaces
    for _ in 0..3 {
        let key_space = KeyEvent::new(KeyCode::Char(' '), KeyModifiers::NONE);
        app.handle_key(key_space).unwrap();
    }

    assert_eq!(app.command_buffer, "   ", "Buffer should have 3 spaces");

    // Press Enter
    let key_enter = KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE);
    let result = app.handle_key(key_enter).unwrap();

    assert!(result, "Whitespace command should not quit");

    let last_log = app.logs.last().unwrap();
    println!("Whitespace command logged: {}", last_log);

    // The command is trimmed, so this should behave like empty command (silently ignored)
    assert_eq!(
        last_log, "TUI initialized",
        "Whitespace-only command should be silently ignored like empty commands"
    );
}

#[test]
fn test_very_long_command() {
    // BUG INVESTIGATION: What happens with extremely long commands?
    let mut app = App::new();

    // Enter command mode
    let key_colon = KeyEvent::new(KeyCode::Char(':'), KeyModifiers::NONE);
    app.handle_key(key_colon).unwrap();

    // Type 1000 characters
    for _ in 0..1000 {
        let key = KeyEvent::new(KeyCode::Char('a'), KeyModifiers::NONE);
        app.handle_key(key).unwrap();
    }

    assert_eq!(
        app.command_buffer.len(),
        1000,
        "Buffer should have 1000 chars"
    );

    // Press Enter
    let key_enter = KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE);
    let result = app.handle_key(key_enter).unwrap();

    assert!(result, "Long command should not crash");

    let last_log = app.logs.last().unwrap();
    println!("Long command logged (length: {})", last_log.len());

    // The log contains "Unknown command: " + 1000 a's
    // This could cause UI issues if the log panel doesn't handle long lines well
    assert!(last_log.len() > 1000, "Log should contain the full command");
}

#[test]
fn test_escape_key_in_command_mode() {
    // BUG INVESTIGATION: Verify escape key properly cancels command mode
    let mut app = App::new();

    // Enter command mode and type something
    let key_colon = KeyEvent::new(KeyCode::Char(':'), KeyModifiers::NONE);
    app.handle_key(key_colon).unwrap();

    let key_q = KeyEvent::new(KeyCode::Char('q'), KeyModifiers::NONE);
    app.handle_key(key_q).unwrap();

    assert_eq!(app.command_buffer, "q");
    assert!(app.command_mode);

    // Press Escape
    let key_esc = KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE);
    let result = app.handle_key(key_esc).unwrap();

    assert!(result, "Escape should not quit");
    assert!(!app.command_mode, "Should exit command mode");
    assert_eq!(app.command_buffer, "", "Buffer should be cleared");

    println!("✓ Escape key works correctly in command mode");
}

#[test]
fn test_backspace_on_empty_command() {
    // BUG INVESTIGATION: What happens when backspacing on empty command buffer?
    let mut app = App::new();

    // Enter command mode
    let key_colon = KeyEvent::new(KeyCode::Char(':'), KeyModifiers::NONE);
    app.handle_key(key_colon).unwrap();

    assert_eq!(app.command_buffer, "");

    // Press backspace on empty buffer
    let key_backspace = KeyEvent::new(KeyCode::Backspace, KeyModifiers::NONE);
    let result = app.handle_key(key_backspace).unwrap();

    assert!(result, "Backspace should not crash");
    assert_eq!(app.command_buffer, "", "Buffer should still be empty");

    println!("✓ Backspace on empty buffer handled correctly");
}

#[test]
fn test_scroll_on_non_logs_panel() {
    // BUG INVESTIGATION: j/k keys should only scroll on Logs panel
    // What happens when pressing j/k on other panels?
    let mut app = App::new();

    // Start on Configuration panel
    assert_eq!(app.current_panel, Panel::Configuration);

    // Press j and k - should do nothing
    let key_j = KeyEvent::new(KeyCode::Char('j'), KeyModifiers::NONE);
    let key_k = KeyEvent::new(KeyCode::Char('k'), KeyModifiers::NONE);

    app.handle_key(key_j).unwrap();
    app.handle_key(key_k).unwrap();

    // Still on Configuration panel
    assert_eq!(app.current_panel, Panel::Configuration);

    // Switch to LiveTranscription panel
    app.current_panel = Panel::LiveTranscription;

    app.handle_key(key_j).unwrap();
    app.handle_key(key_k).unwrap();

    // Still on LiveTranscription panel
    assert_eq!(app.current_panel, Panel::LiveTranscription);

    println!("✓ Scroll keys correctly do nothing on non-Logs panels");
}

#[test]
fn test_rapid_panel_switching() {
    // BUG INVESTIGATION: Does rapid panel switching cause issues?
    let mut app = App::new();

    // Rapidly switch panels 100 times
    for _ in 0..100 {
        let key_h = KeyEvent::new(KeyCode::Char('h'), KeyModifiers::NONE);
        app.handle_key(key_h).unwrap();
    }

    // Let's just verify no crash and state is consistent
    assert!(!app.command_mode);

    println!("✓ Rapid panel switching handled correctly");
}

#[test]
fn test_command_mode_with_special_chars() {
    // BUG INVESTIGATION: Can user type special characters in command mode?
    let mut app = App::new();

    // Enter command mode
    let key_colon = KeyEvent::new(KeyCode::Char(':'), KeyModifiers::NONE);
    app.handle_key(key_colon).unwrap();

    // Try typing various special characters
    let special_chars = vec![
        '!', '@', '#', '$', '%', '^', '&', '*', '(', ')', '-', '=', '[', ']',
    ];

    for ch in special_chars {
        let key = KeyEvent::new(KeyCode::Char(ch), KeyModifiers::NONE);
        app.handle_key(key).unwrap();
    }

    assert!(
        !app.command_buffer.is_empty(),
        "Should accept special characters"
    );
    println!("Command buffer with special chars: {}", app.command_buffer);

    // Press Enter
    let key_enter = KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE);
    app.handle_key(key_enter).unwrap();

    // Should log unknown command
    let last_log = app.logs.last().unwrap();
    assert!(
        last_log.starts_with("Unknown command:"),
        "Should log unknown command"
    );

    println!("✓ Special characters in command mode handled correctly");
}
