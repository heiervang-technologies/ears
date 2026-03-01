# Ears Makeover Audit Scratchpad

## Phase 1: Deep Contextualization & Auditing

### Initial Observations
- **Architecture**: Rust daemon interacting with `whisper.cpp` (or OpenAI-compatible ASR) for Linux.
- **Key Components**: TUI (ratatui), Process management (`pw-record`), Desktop integration (`wtype`, `ydotool`), State management, Whisper API.
- **Security Check**: `desktop.rs` handles user inputs correctly by passing them via `.arg(text)`, preventing shell injection vulnerabilities. Post-transcribe hook in `main.rs` also correctly passes arguments.

---

## Phase 2: Categorized Findings

### P0: Critical Security & Integrity
1. **Insecure Predictable Temporary File (`ears-sound.wav`)**
   - **File**: `src/desktop.rs` (`play_embedded` at L328)
   - **Description**: Writes embedded audio to `$XDG_RUNTIME_DIR/ears-sound.wav`. If `XDG_RUNTIME_DIR` is not set, it falls back to `/tmp/ears-sound.wav`. A predictable path in a shared directory (`/tmp`) allows symlink attacks, meaning an attacker can trick the app into overwriting arbitrary files owned by the user (e.g., `~/.bashrc`).
   - **Effort**: XS. **Fix**: Pipe the embedded sound bytes directly to `paplay`'s stdin instead of writing to disk.

2. **Insecure Predictable State Directory Fallback (`/tmp/ears-<pid>`)**
   - **File**: `src/config.rs` (`computed_dirs` at L114)
   - **Description**: If `XDG_RUNTIME_DIR` is missing, `state_dir` falls back to `/tmp/ears-<pid>`. PIDs are predictable. An attacker can pre-create this directory. Since it contains `vad.pid`, `recording.pid`, and `state`, the attacker can supply fake PIDs, causing `ears` to send `SIGTERM` to arbitrary processes, or overwrite the state to disrupt the application.
   - **Effort**: S. **Fix**: Use `project_dirs.runtime_dir()` or fallback to a secure, user-specific directory like `project_dirs.cache_dir().join("run")`.

### P1: High-Impact Bugs & Failures
1. **Blocking Network Call inside Tokio Runtime (UI Freeze/Panic)**
   - **File**: `src/tui/app.rs` (`fetch_model_name` at L324)
   - **Description**: Uses `reqwest::blocking::Client` synchronously inside the TUI's `handle_tick`. Since the TUI is run inside `#[tokio::main]`, using `reqwest::blocking` will panic ("Cannot start a runtime from within a runtime") or block the executor and UI thread for up to 3 seconds during slow network requests.
   - **Effort**: S. **Fix**: Refactor `app.rs` to not use `reqwest::blocking` and instead spawn an async task in `tui::run` with the async `WhisperClient`.

### P2: Bad Practices & Documentation Debt
1. **Redundant HTTP Client (Feature Bloat)**
   - **File**: `Cargo.toml`, `src/tui/app.rs`
   - **Description**: The project includes `reqwest` with the `blocking` feature enabled just for one function in `app.rs`, while it already uses async `reqwest` for everything else (`whisper.rs`).
   - **Effort**: XS. **Fix**: Remove `blocking` feature from `Cargo.toml`.
2. **Zombie Post-Transcribe Hooks**
   - **File**: `src/main.rs`
   - **Description**: `std::thread::spawn` runs a `Command::spawn()` without `.wait()`. The child process will become a zombie until the main `ears` process exits. Since `ears toggle` exits quickly, this is minor, but `ears vad` is long-running and could accumulate zombies.
   - **Effort**: XS. **Fix**: Add `.wait()` inside the thread.

### P4: Technical Debt & Refactoring
1. **Lack of abstraction in `tui::run` for async fetching**
   - **File**: `src/tui/mod.rs`
   - **Description**: Model name fetch is handled lazily inside `App::handle_tick()` sync method, which forces bad patterns.

### P4.5: Performance Bottlenecks
1. **Process Polling in TUI**
   - **File**: `src/tui/mod.rs`
   - **Description**: The event loop could be more efficient by separating UI rendering and state updates.

### P5 & P6: Future Directions & Novel Expansions
1. **Wayland native protocols vs `wtype`**: Explore switching to `zwp_virtual_keyboard_v1` via Wayland RS directly rather than relying on `wtype` or `ydotool` which require external binaries.
2. **Local LLM integration**: Pre/Post processing with a local LLM for grammar correction or action execution (e.g., "delete last sentence").

---

### Action Plan
1. **Phase 4 - Execution (P0)**: Fix `ears-sound.wav` and `state_dir` vulnerabilities. Prepare as atomic commits or single PR equivalent.
2. **Phase 4 - Execution (P1-P4)**: Refactor `app.rs` to fix `reqwest::blocking` panic, remove `blocking` feature, and fix zombie processes.
3. **Phase 4 - Execution (P4.5)**: Review performance if needed.
4. Synthesize final report.
