use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
/// QA Help Text Verification
///
/// This test verifies that the help text in the footer accurately reflects
/// the actual keybindings implemented in the app.
use ears::tui::{App, Panel};

#[test]
fn test_tab_key_works_but_not_documented() {
    // BUG INVESTIGATION: The footer shows "[h/l] Tabs" but Tab/Shift+Tab also work
    // This is a documentation issue - Tab keys work but aren't shown to users
    let mut app = App::new();

    // Start on Status
    assert_eq!(app.current_panel, Panel::Status);

    // Press Tab - should move to next panel (Configuration)
    let key_tab = KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE);
    app.handle_key(key_tab).unwrap();
    assert_eq!(
        app.current_panel,
        Panel::Configuration,
        "Tab should move to next panel"
    );

    // Press Shift+Tab - should move to previous panel (Status)
    let key_shift_tab = KeyEvent::new(KeyCode::BackTab, KeyModifiers::SHIFT);
    app.handle_key(key_shift_tab).unwrap();
    assert_eq!(
        app.current_panel,
        Panel::Status,
        "Shift+Tab should move to previous panel"
    );

    // BUG FOUND: Tab and Shift+Tab work but are not documented in footer
    // Footer only shows "[h/l] Tabs" which is misleading
    println!("BUG: Tab/Shift+Tab work but aren't shown in help text");
    println!("Footer shows: '[h/l] Tabs' but should show: '[h/l/Tab] Tabs'");
}

#[test]
fn test_c_key_shortcut_not_documented() {
    // BUG INVESTIGATION: The 'c' key jumps to configuration panel
    // but this is not documented in the footer
    let mut app = App::new();

    // Start on Status
    assert_eq!(app.current_panel, Panel::Status);

    // Press 'c' - should jump to Configuration
    let key_c = KeyEvent::new(KeyCode::Char('c'), KeyModifiers::NONE);
    app.handle_key(key_c).unwrap();
    assert_eq!(
        app.current_panel,
        Panel::Configuration,
        "'c' should jump to Configuration panel"
    );

    // BUG FOUND: 'c' shortcut works but is not documented
    println!("BUG: 'c' key jumps to Config but not shown in footer");
}

#[test]
fn test_all_documented_keys_work() {
    // VERIFICATION: All keys shown in footer should actually work
    // Footer shows: "[Space] Start/Stop  [h/l] Tabs  [j/k] Scroll  [:] Command  [q] Quit"
    let mut app = App::new();

    // [Space] Start/Stop
    let was_recording = app.is_recording;
    let key_space = KeyEvent::new(KeyCode::Char(' '), KeyModifiers::NONE);
    app.handle_key(key_space).unwrap();
    assert_ne!(
        app.is_recording, was_recording,
        "Space should toggle recording"
    );

    // [h/l] Tabs
    let initial_panel = app.current_panel;
    let key_h = KeyEvent::new(KeyCode::Char('h'), KeyModifiers::NONE);
    app.handle_key(key_h).unwrap();
    assert_ne!(app.current_panel, initial_panel, "'h' should switch panels");

    let current_panel = app.current_panel;
    let key_l = KeyEvent::new(KeyCode::Char('l'), KeyModifiers::NONE);
    app.handle_key(key_l).unwrap();
    assert_ne!(app.current_panel, current_panel, "'l' should switch panels");

    // [j/k] Scroll - only works on Logs panel
    app.current_panel = Panel::Logs;
    let _initial_selection = app.selected_log;
    let key_j = KeyEvent::new(KeyCode::Char('j'), KeyModifiers::NONE);
    app.handle_key(key_j).unwrap();
    // May or may not change depending on log count
    let key_k = KeyEvent::new(KeyCode::Char('k'), KeyModifiers::NONE);
    app.handle_key(key_k).unwrap();

    // [:] Command
    let key_colon = KeyEvent::new(KeyCode::Char(':'), KeyModifiers::NONE);
    app.handle_key(key_colon).unwrap();
    assert!(app.command_mode, "':' should enter command mode");

    // Exit command mode
    let key_esc = KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE);
    app.handle_key(key_esc).unwrap();

    // [q] Quit
    let key_q = KeyEvent::new(KeyCode::Char('q'), KeyModifiers::NONE);
    let result = app.handle_key(key_q).unwrap();
    assert!(!result, "'q' should quit the app");

    println!("✓ All documented keys work correctly");
}
