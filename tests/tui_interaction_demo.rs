//! Demo: Simulating user interaction with the TUI
//!
//! This shows what's possible: we can simulate keypresses and verify
//! the visual output changes, but we can't "freely" interact like a real user.

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ears::tui::{App, Panel};
use ratatui::{backend::TestBackend, Terminal};

/// Helper to render app to string
fn render_to_string(app: &App, width: u16, height: u16) -> String {
    let backend = TestBackend::new(width, height);
    let mut terminal = Terminal::new(backend).unwrap();

    terminal.draw(|f| ears::tui::ui::render(app, f)).unwrap();

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
fn test_interactive_session_scripted() {
    let mut app = App::new();

    println!("\n=== INITIAL STATE ===");
    let output = render_to_string(&app, 80, 24);
    println!("{}", output);
    assert!(output.contains("○ Idle"));
    assert_eq!(app.current_panel, Panel::Status);

    println!("\n=== PRESS 'l' to go to Configuration panel ===");
    app.handle_key(KeyEvent::new(KeyCode::Char('l'), KeyModifiers::NONE))
        .unwrap();
    let output = render_to_string(&app, 80, 24);
    println!("{}", output);
    assert_eq!(app.current_panel, Panel::Configuration);

    println!("\n=== PRESS 'l' again to go to Logs panel ===");
    app.handle_key(KeyEvent::new(KeyCode::Char('l'), KeyModifiers::NONE))
        .unwrap();
    let output = render_to_string(&app, 80, 24);
    println!("{}", output);
    assert_eq!(app.current_panel, Panel::Logs);

    println!("\n=== PRESS Space to start recording ===");
    app.handle_key(KeyEvent::new(KeyCode::Char(' '), KeyModifiers::NONE))
        .unwrap();
    let output = render_to_string(&app, 80, 24);
    println!("{}", output);
    assert!(app.is_recording);
    assert!(output.contains("●")); // Recording indicator

    println!("\n=== PRESS ':' to enter command mode ===");
    app.handle_key(KeyEvent::new(KeyCode::Char(':'), KeyModifiers::NONE))
        .unwrap();
    let output = render_to_string(&app, 80, 24);
    println!("{}", output);
    assert!(app.command_mode);

    println!("\n=== TYPE 'quit' ===");
    for c in "quit".chars() {
        app.handle_key(KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE))
            .unwrap();
    }
    let output = render_to_string(&app, 80, 24);
    println!("{}", output);
    assert!(output.contains(":quit"));
    assert_eq!(app.command_buffer, "quit");
}

#[test]
fn test_cannot_freely_interact() {
    // This test demonstrates the limitation:
    // We can only test PRE-SCRIPTED interactions

    let mut app = App::new();

    // We CANNOT do:
    // - Wait for user input and respond dynamically
    // - See colors or animations
    // - Experience timing/responsiveness
    // - Make decisions based on visual appearance

    // We CAN do:
    // - Simulate a sequence of keypresses
    app.handle_key(KeyEvent::new(KeyCode::Char('l'), KeyModifiers::NONE))
        .unwrap();
    app.handle_key(KeyEvent::new(KeyCode::Char('j'), KeyModifiers::NONE))
        .unwrap();
    app.handle_key(KeyEvent::new(KeyCode::Char('k'), KeyModifiers::NONE))
        .unwrap();

    // - Check the resulting state
    assert_eq!(app.current_panel, Panel::Configuration);

    // - Verify visual output
    let output = render_to_string(&app, 80, 24);
    assert!(output.contains("Configuration"));

    // But this is NOT the same as "freely interacting" - it's automated testing
    // of pre-planned interaction sequences.
}

#[test]
fn test_complex_interaction_flow() {
    println!("\n=== SIMULATING A COMPLETE USER SESSION ===\n");

    let mut app = App::new();

    // User opens app
    println!("1. App opens on Status panel");
    let output = render_to_string(&app, 80, 24);
    println!("{}\n", output);

    // Navigate to Config
    println!("2. User presses Tab to go to Config");
    app.handle_key(KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE))
        .unwrap();
    let output = render_to_string(&app, 80, 24);
    println!("{}\n", output);

    // Navigate to Logs
    println!("3. User presses Tab again to go to Logs");
    app.handle_key(KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE))
        .unwrap();

    // Add some log entries (simulating app activity)
    println!("4. App generates some logs");
    app.logs.push("Started recording".to_string());
    app.logs.push("Transcribed: Hello world".to_string());
    app.logs.push("Stopped recording".to_string());

    let output = render_to_string(&app, 80, 24);
    println!("{}\n", output);
    assert!(output.contains("Hello world"));

    // Scroll through logs
    println!("5. User presses 'j' to scroll down in logs");
    app.handle_key(KeyEvent::new(KeyCode::Char('j'), KeyModifiers::NONE))
        .unwrap();
    assert_eq!(app.selected_log, 1);

    // Navigate back to Status
    println!("6. User presses 'h' twice to go back to Status");
    app.handle_key(KeyEvent::new(KeyCode::Char('h'), KeyModifiers::NONE))
        .unwrap();
    app.handle_key(KeyEvent::new(KeyCode::Char('h'), KeyModifiers::NONE))
        .unwrap();

    let output = render_to_string(&app, 80, 24);
    println!("{}\n", output);
    assert_eq!(app.current_panel, Panel::Status);

    println!("=== SESSION COMPLETE ===");
    println!("\nThis demonstrates SCRIPTED interaction, not FREE interaction.");
    println!("We can test specific user flows, but not explore the UI freely.");
}
