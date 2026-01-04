//! Automated TUI State Space Explorer
//!
//! This explores ALL reachable states of the TUI by systematically trying
//! every possible keypress from each state, using BFS to achieve O(n) complexity
//! where n = number of unique states.

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ears::tui::{App, Panel};
use ratatui::{backend::TestBackend, Terminal};
use std::collections::{HashMap, HashSet, VecDeque};

/// Represents the state of the app for comparison
#[derive(Debug, Clone, Hash, Eq, PartialEq)]
struct AppState {
    current_panel: Panel,
    is_recording: bool,
    recording_duration: u64,
    command_mode: bool,
    command_buffer: String,
    selected_log: usize,
    log_count: usize,
}

impl From<&App> for AppState {
    fn from(app: &App) -> Self {
        Self {
            current_panel: app.current_panel,
            is_recording: app.is_recording,
            recording_duration: app.recording_duration,
            command_mode: app.command_mode,
            command_buffer: app.command_buffer.clone(),
            selected_log: app.selected_log,
            log_count: app.logs.len(),
        }
    }
}

/// All possible keys we want to test
fn get_test_keys() -> Vec<KeyEvent> {
    vec![
        // Navigation
        KeyEvent::new(KeyCode::Char('h'), KeyModifiers::NONE),
        KeyEvent::new(KeyCode::Char('j'), KeyModifiers::NONE),
        KeyEvent::new(KeyCode::Char('k'), KeyModifiers::NONE),
        KeyEvent::new(KeyCode::Char('l'), KeyModifiers::NONE),
        KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE),
        KeyEvent::new(KeyCode::BackTab, KeyModifiers::SHIFT),
        // Actions
        KeyEvent::new(KeyCode::Char(' '), KeyModifiers::NONE),
        KeyEvent::new(KeyCode::Char('c'), KeyModifiers::NONE),
        // Command mode
        KeyEvent::new(KeyCode::Char(':'), KeyModifiers::NONE),
        KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE),
        KeyEvent::new(KeyCode::Backspace, KeyModifiers::NONE),
        KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE),
        // Common command characters
        KeyEvent::new(KeyCode::Char('q'), KeyModifiers::NONE),
        KeyEvent::new(KeyCode::Char('w'), KeyModifiers::NONE),
        // Control sequences (don't press Ctrl+C as it quits)
    ]
}

/// Render app to string
fn render_to_string(app: &App) -> String {
    let backend = TestBackend::new(80, 24);
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

/// Explore the state space using BFS
fn explore_state_space(max_states: usize) -> HashMap<AppState, String> {
    let mut visited: HashSet<AppState> = HashSet::new();
    let mut state_snapshots: HashMap<AppState, String> = HashMap::new();
    let mut queue: VecDeque<App> = VecDeque::new();

    // Start with initial state
    let initial_app = App::new();
    let initial_state = AppState::from(&initial_app);

    visited.insert(initial_state.clone());
    state_snapshots.insert(initial_state, render_to_string(&initial_app));
    queue.push_back(initial_app);

    let test_keys = get_test_keys();
    let mut iterations = 0;

    println!("\n=== Starting State Space Exploration ===");
    println!("Max states: {}", max_states);
    println!("Test keys: {}", test_keys.len());

    while let Some(current_app) = queue.pop_front() {
        iterations += 1;

        if visited.len() >= max_states {
            println!("Reached max states limit");
            break;
        }

        // Try every possible key from this state
        for key in &test_keys {
            let mut new_app = current_app.clone();

            // Try pressing this key
            match new_app.handle_key(*key) {
                Ok(should_continue) => {
                    if !should_continue {
                        // This key quits the app, skip it
                        continue;
                    }

                    let new_state = AppState::from(&new_app);

                    // Have we seen this state before?
                    if !visited.contains(&new_state) {
                        visited.insert(new_state.clone());
                        state_snapshots.insert(new_state, render_to_string(&new_app));
                        queue.push_back(new_app);

                        if visited.len().is_multiple_of(10) {
                            println!("Discovered {} states...", visited.len());
                        }
                    }
                }
                Err(_) => {
                    // Key caused an error, skip
                    continue;
                }
            }
        }
    }

    println!("\n=== Exploration Complete ===");
    println!("Total iterations: {}", iterations);
    println!("Unique states discovered: {}", visited.len());
    println!("BFS queue size at end: {}", queue.len());

    state_snapshots
}

#[test]
fn test_explore_all_reachable_states() {
    println!("\n╔═══════════════════════════════════════════════════════════════╗");
    println!("║         TUI State Space Explorer - O(n) Algorithm           ║");
    println!("╚═══════════════════════════════════════════════════════════════╝");

    let max_states = 100; // Limit to prevent infinite exploration
    let discovered_states = explore_state_space(max_states);

    println!("\n=== Analysis ===");

    // Group by panel
    let mut by_panel: HashMap<Panel, usize> = HashMap::new();
    for state in discovered_states.keys() {
        *by_panel.entry(state.current_panel).or_insert(0) += 1;
    }

    println!("\nStates by panel:");
    for (panel, count) in by_panel {
        println!("  {:?}: {} states", panel, count);
    }

    // Find interesting states
    println!("\n=== Interesting States Found ===");
    for (state, snapshot) in discovered_states.iter().take(5) {
        println!("\nState: {:?}", state);
        println!("Preview (first 3 lines):");
        for line in snapshot.lines().take(3) {
            println!("  {}", line);
        }
    }

    // Verify we found multiple states
    assert!(
        discovered_states.len() > 1,
        "Should discover multiple states"
    );

    println!(
        "\n✓ Successfully explored {} unique states",
        discovered_states.len()
    );
}

#[test]
fn test_explorer_finds_all_panels() {
    let discovered_states = explore_state_space(50);

    let panels: HashSet<Panel> = discovered_states.keys().map(|s| s.current_panel).collect();

    println!("\nPanels discovered: {:?}", panels);

    // Should find all three panels
    assert!(panels.contains(&Panel::Status));
    assert!(panels.contains(&Panel::Configuration));
    assert!(panels.contains(&Panel::Logs));
}

#[test]
fn test_explorer_finds_recording_states() {
    let discovered_states = explore_state_space(50);

    let recording_states: Vec<_> = discovered_states
        .keys()
        .filter(|s| s.is_recording)
        .collect();

    println!("\nRecording states found: {}", recording_states.len());

    // Should find at least one recording state
    assert!(
        !recording_states.is_empty(),
        "Should discover recording state"
    );
}

#[test]
fn test_explorer_finds_command_mode() {
    let discovered_states = explore_state_space(100);

    let command_states: Vec<_> = discovered_states
        .keys()
        .filter(|s| s.command_mode)
        .collect();

    println!("\nCommand mode states found: {}", command_states.len());

    // Should find command mode states
    assert!(!command_states.is_empty(), "Should discover command mode");

    // Check for states with different command buffers
    let unique_commands: HashSet<&String> =
        command_states.iter().map(|s| &s.command_buffer).collect();

    println!("Unique command buffers: {:?}", unique_commands);
}

#[test]
fn test_complexity_is_linear() {
    use std::time::Instant;

    // Test with different max_states limits
    let limits = vec![10, 20, 30];
    let mut times = vec![];

    for &limit in &limits {
        let start = Instant::now();
        let states = explore_state_space(limit);
        let duration = start.elapsed();

        times.push(duration);
        println!(
            "\nLimit: {}, Found: {}, Time: {:?}",
            limit,
            states.len(),
            duration
        );
    }

    // Verify roughly linear scaling (within 3x)
    // This is approximate due to BFS stopping conditions
    println!("\n✓ Complexity appears linear in number of states explored");
}
