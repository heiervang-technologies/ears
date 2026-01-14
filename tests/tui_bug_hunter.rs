//! QA Bug Hunter - Systematically find bugs in TUI states
//!
//! This test explores all reachable states and checks for:
//! - Rendering crashes
//! - Invalid state combinations
//! - Missing visual elements
//! - Inconsistencies between state and display

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ears::tui::{App, Panel};
use ratatui::{backend::TestBackend, Terminal};
use std::collections::{HashMap, HashSet, VecDeque};

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

fn get_test_keys() -> Vec<KeyEvent> {
    vec![
        KeyEvent::new(KeyCode::Char('h'), KeyModifiers::NONE),
        KeyEvent::new(KeyCode::Char('j'), KeyModifiers::NONE),
        KeyEvent::new(KeyCode::Char('k'), KeyModifiers::NONE),
        KeyEvent::new(KeyCode::Char('l'), KeyModifiers::NONE),
        KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE),
        KeyEvent::new(KeyCode::BackTab, KeyModifiers::SHIFT),
        KeyEvent::new(KeyCode::Char(' '), KeyModifiers::NONE),
        KeyEvent::new(KeyCode::Char('c'), KeyModifiers::NONE),
        KeyEvent::new(KeyCode::Char(':'), KeyModifiers::NONE),
        KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE),
        KeyEvent::new(KeyCode::Backspace, KeyModifiers::NONE),
        KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE),
        KeyEvent::new(KeyCode::Char('q'), KeyModifiers::NONE),
        KeyEvent::new(KeyCode::Char('w'), KeyModifiers::NONE),
    ]
}

fn render_to_string(app: &mut App) -> String {
    let backend = TestBackend::new(80, 24);
    let mut terminal = Terminal::new(backend).unwrap();

    app.clear_clickable_regions();
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

fn explore_and_collect_states(max_states: usize) -> Vec<(AppState, App, String)> {
    let mut visited: HashSet<AppState> = HashSet::new();
    let mut results: Vec<(AppState, App, String)> = Vec::new();
    let mut queue: VecDeque<App> = VecDeque::new();

    let mut initial_app = App::new();
    let initial_state = AppState::from(&initial_app);
    let initial_output = render_to_string(&mut initial_app);

    visited.insert(initial_state.clone());
    results.push((initial_state, initial_app.clone(), initial_output));
    queue.push_back(initial_app);

    let test_keys = get_test_keys();

    while let Some(current_app) = queue.pop_front() {
        if visited.len() >= max_states {
            break;
        }

        for key in &test_keys {
            let mut new_app = current_app.clone();

            match new_app.handle_key(*key) {
                Ok(should_continue) => {
                    if !should_continue {
                        continue;
                    }

                    let new_state = AppState::from(&new_app);

                    if !visited.contains(&new_state) {
                        let output = render_to_string(&mut new_app);
                        visited.insert(new_state.clone());
                        results.push((new_state, new_app.clone(), output));
                        queue.push_back(new_app);
                    }
                }
                Err(_) => continue,
            }
        }
    }

    results
}

#[derive(Debug)]
struct Bug {
    description: String,
    state: AppState,
    visual_sample: String,
}

#[test]
fn bug_hunt_all_states_have_app_name() {
    println!("\n🐛 BUG HUNT: Checking all states show app name...");

    let states = explore_and_collect_states(200);
    let mut bugs = Vec::new();

    for (state, _app, output) in &states {
        if !output.contains("ears") {
            bugs.push(Bug {
                description: "App name 'ears' missing from output".to_string(),
                state: state.clone(),
                visual_sample: output.lines().take(5).collect::<Vec<_>>().join("\n"),
            });
        }
    }

    if !bugs.is_empty() {
        println!("\n❌ BUGS FOUND: {} states missing app name", bugs.len());
        for bug in &bugs {
            println!("\nState: {:?}", bug.state);
            println!("Sample:\n{}", bug.visual_sample);
        }
    } else {
        println!("✓ All {} states show app name", states.len());
    }

    assert!(bugs.is_empty(), "Found {} bugs", bugs.len());
}

#[test]
fn bug_hunt_recording_indicator_consistency() {
    println!("\n🐛 BUG HUNT: Checking recording indicator consistency...");

    let states = explore_and_collect_states(200);
    let mut bugs = Vec::new();

    for (state, _app, output) in &states {
        let has_recording_symbol = output.contains("●");
        let has_idle_symbol = output.contains("○");

        if state.is_recording {
            // Should show recording indicator
            if !has_recording_symbol {
                bugs.push(Bug {
                    description: "State says recording=true but no ● symbol shown".to_string(),
                    state: state.clone(),
                    visual_sample: output.lines().take(3).collect::<Vec<_>>().join("\n"),
                });
            }
        } else {
            // Should show idle indicator
            if !has_idle_symbol {
                bugs.push(Bug {
                    description: "State says recording=false but no ○ symbol shown".to_string(),
                    state: state.clone(),
                    visual_sample: output.lines().take(3).collect::<Vec<_>>().join("\n"),
                });
            }
        }
    }

    if !bugs.is_empty() {
        println!(
            "\n❌ BUGS FOUND: {} inconsistent recording indicators",
            bugs.len()
        );
        for bug in &bugs {
            println!("\n{}", bug.description);
            println!("State: {:?}", bug.state);
            println!("Sample:\n{}", bug.visual_sample);
        }
    } else {
        println!(
            "✓ All {} states have consistent recording indicators",
            states.len()
        );
    }

    assert!(bugs.is_empty(), "Found {} bugs", bugs.len());
}

#[test]
fn bug_hunt_command_mode_display() {
    println!("\n🐛 BUG HUNT: Checking command mode display...");

    let states = explore_and_collect_states(200);
    let mut bugs = Vec::new();

    for (state, _app, output) in &states {
        if state.command_mode {
            // Command buffer should be visible
            if !state.command_buffer.is_empty() {
                let expected_display = format!(":{}", state.command_buffer);
                if !output.contains(&expected_display) {
                    bugs.push(Bug {
                        description: format!(
                            "Command mode active with buffer '{}' but '{}' not shown in output",
                            state.command_buffer, expected_display
                        ),
                        state: state.clone(),
                        visual_sample: output
                            .lines()
                            .skip(output.lines().count().saturating_sub(5))
                            .collect::<Vec<_>>()
                            .join("\n"),
                    });
                }
            }
        }
    }

    if !bugs.is_empty() {
        println!(
            "\n❌ BUGS FOUND: {} command mode display issues",
            bugs.len()
        );
        for bug in bugs.iter().take(5) {
            println!("\n{}", bug.description);
            println!("State: {:?}", bug.state);
            println!("Footer:\n{}", bug.visual_sample);
        }
    } else {
        println!("✓ All command mode states display correctly");
    }

    assert!(bugs.is_empty(), "Found {} bugs", bugs.len());
}

#[test]
fn bug_hunt_panel_name_visibility() {
    println!("\n🐛 BUG HUNT: Checking panel names are visible...");

    let states = explore_and_collect_states(200);
    let mut bugs = Vec::new();

    for (state, _app, output) in &states {
        let expected_panel = match state.current_panel {
            Panel::Status => "Status",
            Panel::Configuration => "Configuration",
            Panel::Logs => "Logs",
            Panel::LiveTranscription => "Live",
        };

        if !output.contains(expected_panel) {
            bugs.push(Bug {
                description: format!(
                    "Current panel is {:?} but '{}' not found in output",
                    state.current_panel, expected_panel
                ),
                state: state.clone(),
                visual_sample: output.lines().take(10).collect::<Vec<_>>().join("\n"),
            });
        }
    }

    if !bugs.is_empty() {
        println!("\n❌ BUGS FOUND: {} missing panel names", bugs.len());
        for bug in bugs.iter().take(3) {
            println!("\n{}", bug.description);
            println!("State: {:?}", bug.state);
            println!("Sample:\n{}", bug.visual_sample);
        }
    } else {
        println!("✓ All {} states show correct panel name", states.len());
    }

    assert!(bugs.is_empty(), "Found {} bugs", bugs.len());
}

#[test]
fn bug_hunt_empty_output() {
    println!("\n🐛 BUG HUNT: Checking for empty/truncated output...");

    let states = explore_and_collect_states(200);
    let mut bugs = Vec::new();

    for (state, _app, output) in &states {
        if output.trim().is_empty() {
            bugs.push(Bug {
                description: "Output is completely empty".to_string(),
                state: state.clone(),
                visual_sample: format!("(empty - length: {})", output.len()),
            });
        } else if output.lines().count() < 20 {
            bugs.push(Bug {
                description: format!(
                    "Output seems truncated - only {} lines (expected ~24)",
                    output.lines().count()
                ),
                state: state.clone(),
                visual_sample: format!("Line count: {}", output.lines().count()),
            });
        }
    }

    if !bugs.is_empty() {
        println!("\n❌ BUGS FOUND: {} empty/truncated outputs", bugs.len());
        for bug in &bugs {
            println!("\n{}", bug.description);
            println!("State: {:?}", bug.state);
        }
    } else {
        println!("✓ All {} states produce full output", states.len());
    }

    assert!(bugs.is_empty(), "Found {} bugs", bugs.len());
}

#[test]
fn bug_hunt_selected_log_bounds() {
    println!("\n🐛 BUG HUNT: Checking selected_log stays within bounds...");

    let states = explore_and_collect_states(200);
    let mut bugs = Vec::new();

    for (state, _app, _output) in &states {
        if state.log_count > 0 && state.selected_log >= state.log_count {
            bugs.push(Bug {
                description: format!(
                    "selected_log ({}) >= log_count ({}) - out of bounds!",
                    state.selected_log, state.log_count
                ),
                state: state.clone(),
                visual_sample: String::new(),
            });
        }
    }

    if !bugs.is_empty() {
        println!(
            "\n❌ BUGS FOUND: {} out-of-bounds log selections",
            bugs.len()
        );
        for bug in &bugs {
            println!("\n{}", bug.description);
            println!("State: {:?}", bug.state);
        }
    } else {
        println!("✓ All log selections are within bounds");
    }

    assert!(bugs.is_empty(), "Found {} bugs", bugs.len());
}

#[test]
fn bug_hunt_summary_report() {
    println!("\n╔═══════════════════════════════════════════════════════════════╗");
    println!("║              🐛 COMPREHENSIVE BUG HUNT REPORT                ║");
    println!("╚═══════════════════════════════════════════════════════════════╝\n");

    let states = explore_and_collect_states(200);

    println!("Explored {} unique states\n", states.len());

    // Collect all bugs
    let mut all_bugs: Vec<Bug> = Vec::new();

    // Check 1: App name
    for (state, _, output) in &states {
        if !output.contains("ears") {
            all_bugs.push(Bug {
                description: "[CRITICAL] App name missing".to_string(),
                state: state.clone(),
                visual_sample: output.lines().take(3).collect::<Vec<_>>().join("\n"),
            });
        }
    }

    // Check 2: Recording indicator
    for (state, _, output) in &states {
        if state.is_recording && !output.contains("●") {
            all_bugs.push(Bug {
                description: "[HIGH] Recording indicator missing when recording=true".to_string(),
                state: state.clone(),
                visual_sample: output.lines().take(3).collect::<Vec<_>>().join("\n"),
            });
        }
    }

    // Check 3: Panel visibility
    for (state, _, output) in &states {
        let panel_name = match state.current_panel {
            Panel::Status => "Status",
            Panel::Configuration => "Configuration",
            Panel::Logs => "Logs",
            Panel::LiveTranscription => "Live",
        };
        if !output.contains(panel_name) {
            all_bugs.push(Bug {
                description: format!("[MEDIUM] Panel name '{}' not visible", panel_name),
                state: state.clone(),
                visual_sample: output.lines().take(5).collect::<Vec<_>>().join("\n"),
            });
        }
    }

    // Check 4: Command mode
    for (state, _, output) in &states {
        if state.command_mode && !state.command_buffer.is_empty() {
            let expected = format!(":{}", state.command_buffer);
            if !output.contains(&expected) {
                all_bugs.push(Bug {
                    description: format!(
                        "[MEDIUM] Command buffer '{}' not shown",
                        state.command_buffer
                    ),
                    state: state.clone(),
                    visual_sample: output
                        .lines()
                        .skip(output.lines().count().saturating_sub(3))
                        .collect::<Vec<_>>()
                        .join("\n"),
                });
            }
        }
    }

    // Check 5: Bounds checking
    for (state, _, _) in &states {
        if state.log_count > 0 && state.selected_log >= state.log_count {
            all_bugs.push(Bug {
                description: format!(
                    "[CRITICAL] Log selection out of bounds ({} >= {})",
                    state.selected_log, state.log_count
                ),
                state: state.clone(),
                visual_sample: String::new(),
            });
        }
    }

    println!("═══════════════════════════════════════════════════════════════");
    println!("TOTAL BUGS FOUND: {}", all_bugs.len());
    println!("═══════════════════════════════════════════════════════════════\n");

    if all_bugs.is_empty() {
        println!("✅ NO BUGS FOUND! All states passed validation.");
    } else {
        println!("Unique bug types:");
        let mut bug_types: HashMap<String, usize> = HashMap::new();
        for bug in &all_bugs {
            let bug_type = bug
                .description
                .split(']')
                .next()
                .unwrap_or("Unknown")
                .to_string()
                + "]";
            *bug_types.entry(bug_type).or_insert(0) += 1;
        }

        for (bug_type, count) in bug_types {
            println!("  {} - {} occurrences", bug_type, count);
        }

        println!("\nFirst 5 bugs:");
        for (i, bug) in all_bugs.iter().take(5).enumerate() {
            println!("\n{}. {}", i + 1, bug.description);
            println!("   State: {:?}", bug.state);
            if !bug.visual_sample.is_empty() {
                println!(
                    "   Visual: {}",
                    bug.visual_sample.lines().next().unwrap_or("")
                );
            }
        }
    }

    // This test is informational - we don't fail it
    // Individual bug hunt tests will fail if they find bugs
}
