# TUI Visual Testing Strategy

## Problem

Testing TUI (Terminal User Interface) applications is challenging because:
1. You can't visually inspect the output in a headless CI environment
2. Traditional unit tests only verify logic, not visual appearance
3. Layout bugs, alignment issues, and visual regressions are hard to catch

## Solution: ASCII Snapshot Testing

We use a two-part testing strategy:

### 1. **Logic Tests** (`tests/tui_tests.rs`)
- Test key handling, navigation, state transitions
- Fast, focused unit tests
- Example: Verify pressing 'j' increments `selected_log`

### 2. **Visual/Snapshot Tests** (`tests/tui_snapshot_tests.rs`)
- Capture actual ASCII/ANSI output of the TUI
- Save snapshots for regression detection
- Catch visual bugs that logic tests miss

## How It Works

### Ratatui's TestBackend

Ratatui provides a `TestBackend` that renders to an in-memory buffer instead of a real terminal:

```rust
use ratatui::{backend::TestBackend, Terminal};

fn render_to_string(app: &App, width: u16, height: u16) -> String {
    let backend = TestBackend::new(width, height);
    let mut terminal = Terminal::new(backend).unwrap();

    terminal.draw(|f| ears::tui::ui::render(app, f)).unwrap();

    // Convert buffer to ASCII string
    let buffer = terminal.backend().buffer();
    let mut output = String::new();

    for y in 0..buffer.area().height {
        for x in 0..buffer.area().width {
            let cell = &buffer[(x, y)];
            output.push_str(cell.symbol());
        }
        output.push('\n');
    }

    output
}
```

### Snapshot Testing with `insta`

The `insta` crate automatically saves and compares ASCII output:

```rust
#[test]
fn snapshot_initial_state() {
    let app = App::new();
    let output = render_to_string(&app, 80, 24);
    insta::assert_snapshot!(output);
}
```

**First run**: Creates a snapshot file in `tests/snapshots/`
**Subsequent runs**: Compares current output against saved snapshot
**On mismatch**: Shows a diff and prompts for review

## Example Snapshot

```
┌──────────────────────────────────────────────────────────────────────────────┐
│ears v1.0.0 │ Status: ○ Idle                                                  │
└──────────────────────────────────────────────────────────────────────────────┘
┌──────────────────────────────────────────────────────────────────────────────┐
│ ▸ Status │ ▸ Configuration │ ▸ Logs                                          │
└──────────────────────────────────────────────────────────────────────────────┘
┌ Status ──────────────────────────────────────────────────────────────────────┐
│                                                                              │
│Current State: Idle                                                           │
│Model: ggml-base.en                                                           │
│Server: http://localhost:8178                                                 │
│Device: Default Device                                                        │
│                                                                              │
└──────────────────────────────────────────────────────────────────────────────┘
┌──────────────────────────────────────────────────────────────────────────────┐
│[Space] Start/Stop  [h/l] Tabs  [j/k] Scroll  [:] Command  [q] Quit           │
└──────────────────────────────────────────────────────────────────────────────┘
```

This is the **actual** rendered output - pixel-perfect ASCII!

## Workflow

### Running Tests

```bash
# Run all TUI tests (logic + visual)
cargo test --test tui_tests
cargo test --test tui_snapshot_tests

# Run with output visible
cargo test --test tui_visual_tests -- --nocapture
```

### Reviewing Snapshot Changes

When the UI changes (intentionally or by accident):

```bash
# Run tests - they will fail showing diffs
cargo test --test tui_snapshot_tests

# Review changes interactively
cargo insta review

# Or accept all changes
cargo insta accept --all

# Or reject all changes
cargo insta reject --all
```

### CI Integration

In CI, snapshot tests will fail if visual output changes without updating snapshots:

```bash
# CI should run:
cargo insta test --check
```

This ensures all visual changes are intentional and reviewed.

## What Gets Tested

Our snapshot tests cover:

1. **Initial state** - Default TUI appearance
2. **Recording state** - Visual indicator when recording
3. **Panel switching** - Status, Configuration, Logs panels
4. **Command mode** - Vim-style command input
5. **Different terminal sizes** - Small (60x15) to large (120x40)
6. **Content rendering** - Logs with actual text
7. **Empty states** - Panels with no data

## Benefits

✅ **Catch visual regressions** - Alignment, spacing, border changes
✅ **No manual inspection needed** - Automated visual testing
✅ **Works in CI** - No terminal required
✅ **Fast** - Renders in milliseconds
✅ **Comprehensive** - Tests exact output, not just logic
✅ **Easy to review** - Human-readable diffs

## Limitations

❌ **No color verification** - ASCII output doesn't include ANSI colors
❌ **No interactive testing** - Can't test actual user interaction
❌ **Snapshot maintenance** - Need to update snapshots when changing UI

For color verification and final validation, manual testing is still recommended:

```bash
cargo run -- --tui
```

## Adding New Tests

1. Write a test that renders a specific state:
   ```rust
   #[test]
   fn snapshot_new_feature() {
       let mut app = App::new();
       // Set up specific state
       app.some_new_field = true;

       let output = render_to_string(&app, 80, 24);
       insta::assert_snapshot!(output);
   }
   ```

2. Run the test - it will fail (no snapshot exists)
3. Review the output: `cargo insta review`
4. Accept if it looks correct: press 'a' or run `cargo insta accept`

## Best Practices

- **Test different states**: idle, recording, error states
- **Test different sizes**: ensure responsive layout
- **Test edge cases**: empty data, long text, special characters
- **Keep snapshots small**: Focus on specific features
- **Review carefully**: Snapshots become the "source of truth"

---

**Result**: We can now test TUI visuals without needing a real terminal! 🎉
