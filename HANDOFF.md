# PR 106 Handoff: Save to Clipboard

## Goal
Add a "save to clipboard" toggle that copies transcribed text to the clipboard via `wl-copy` after each transcription.

## Branch
`feat/issue-92-save-to-clipboard` — PR #106

## What was done

### Original PR (by marksverdhei)
- Added `save_to_clipboard` config field (default: false)
- Added TUI toggle with 'b' key in Configuration panel
- Added clipboard copy via `wl-copy` in `SegmentCompleted` handler (TUI VAD path only)
- Added clickable regions and UI checkbox in both config panel layouts

### Fixes applied (3 commits)

1. **`36fb927` — Config init + clickable region fix**
   - `save_to_clipboard` was hardcoded to `false` in `App::with_profile()` instead of reading from `config.save_to_clipboard`
   - `auto_correction_line` index was wrong in `render_live_transcription_panel` because the clipboard toggle was inserted before it

2. **`9581aa4` — Added clipboard to `ears toggle` path**
   - The `ears toggle` (push-to-talk keybind) path in `stop_and_transcribe()` (main.rs ~line 798) had no clipboard support
   - Added inline `wl-copy` call

3. **`5047a30` — Centralized clipboard + covered all paths**
   - Created `TextInput::copy_to_clipboard()` in `desktop.rs` as shared helper
   - Removed duplicate clipboard code from `tui/app.rs`
   - Added clipboard support to all 4 transcription paths:
     1. `ears toggle` — push-to-talk keybind (main.rs `stop_and_transcribe`)
     2. TUI VAD — Space key (tui/app.rs `handle_streaming_event` → `SegmentCompleted`)
     3. CLI VAD — `ears --vad` (main.rs `handle_vad`)
     4. WebSocket VAD (main.rs ws-listen handler)
   - Manually set `save_to_clipboard = true` in `~/.config/ears/config.toml`

## Current problem: clipboard copy is NOT executing

### Evidence
The debug log at `/run/user/1000/ears/debug.log` shows successful transcription and typing but **no "Copied text to clipboard" log line**, which means the `TextInput::copy_to_clipboard()` call is never reached.

Latest log (10:40:06):
```
INFO ears::whisper: Whisper API call completed: "Testing. Testing."
INFO ears: Text typing completed in 132ms (15 chars)
INFO ears: Post-transcribe hook started
INFO ears: Total stop_and_transcribe: 624ms
```

Expected but missing between typing and post-transcribe:
```
INFO ears::desktop: Copied text to clipboard
```

### Root cause hypothesis: OLD BINARY STILL RUNNING

The `cargo install` completed at **11:16**, but the test logs are from **10:40**. This means the user tested before the latest install. However, `ears toggle` spawns a fresh process each invocation, so a new `ears toggle` after 11:16 should pick up the new binary.

**But wait** — the user may have an `ears` TUI running in another pane that holds the lock, and the toggle is being handled by that older TUI process. Or the binary in `$PATH` isn't `~/.cargo/bin/ears`.

### Things to investigate

1. **Which binary is actually running?**
   ```bash
   which ears
   # Should be ~/.cargo/bin/ears
   ```

2. **Is there a stale TUI process?**
   ```bash
   pgrep -a ears
   ```

3. **Is the text filter stripping the text before clipboard?**
   - Config has `remove_punctuation = true` and `strict_alphabet = true`
   - The `filtered_text` is what gets copied — check if filters produce empty string
   - Whisper returned "Testing. Testing." → after `remove_punctuation` + `strict_alphabet` this becomes "testing testing" which is fine

4. **Does `config.save_to_clipboard` actually resolve to `true`?**
   - Add a `tracing::info!("save_to_clipboard: {}", config.save_to_clipboard);` at the top of `stop_and_transcribe` to confirm

5. **Is the code even compiled in?**
   ```bash
   strings ~/.cargo/bin/ears | grep "Copied text to clipboard"
   # Should return a match
   ```

## Files modified
- `src/config.rs` — added `save_to_clipboard` field + default fn
- `src/desktop.rs` — added `TextInput::copy_to_clipboard()`
- `src/main.rs` — clipboard in toggle, CLI VAD, and WS VAD paths
- `src/tui/app.rs` — init from config, removed local clipboard fn, uses shared helper
- `src/tui/ui.rs` — fixed `auto_correction_line` index ordering
- `~/.config/ears/config.toml` — manually set `save_to_clipboard = true`
