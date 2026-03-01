# Project Makeover Report: Ears Speech Recognition Daemon

## Executive Summary
Overall, the `ears` repository represents a well-designed, robust Linux daemon that successfully bridges modern desktop audio (PipeWire), virtual keyboard automation (`wtype`/`ydotool`), and transcription (Whisper.cpp). The structural boundaries between the UI (Ratatui), state management, and desktop integration are clearly defined.

However, during this deep-dive audit, we identified several critical security and operational vulnerabilities that posed significant risks to user security and application stability. Predictable temporary file and directory paths opened the door to symlink and DoS attacks. Furthermore, hidden blocking network calls inside the asynchronous `tokio` event loop undermined the reactivity of the application and frequently triggered panics.

I have proactively addressed the P0 through P4 issues to secure and stabilize the codebase while optimizing feature dependencies.

---

## Findings & Remediation

### P0: Critical Security & Integrity (Fixed)
1. **Insecure Predictable Temporary File (`ears-sound.wav`)**
   - **Status**: ✅ **Remediated**
   - **File**: `src/desktop.rs`
   - **Description**: The embedded notification sound was being written to a predictable `/tmp/ears-sound.wav` path if `XDG_RUNTIME_DIR` was unset. This allowed attackers to use symlink attacks to overwrite arbitrary files owned by the executing user.
   - **Effort**: XS
   - **Fix Applied**: Completely eliminated the temporary file dependency by safely piping the embedded WAV bytes directly to `paplay` via its `stdin`.

2. **Insecure Predictable State Directory Fallback (`/tmp/ears-<pid>`)**
   - **Status**: ✅ **Remediated**
   - **File**: `src/config.rs`
   - **Description**: The fallback `state_dir` defaulted to a predictable `/tmp/ears-<pid>`. A local attacker could pre-create this directory and populate it with malicious `vad.pid` files (forcing `ears` to kill arbitrary user processes via SIGTERM) or inject false state data.
   - **Effort**: S
   - **Fix Applied**: Updated the fallback path to leverage standard, user-owned XDG structures `~/.cache/ears/run` via the `directories` crate.

### P1: High-Impact Bugs & Failures (Fixed)
1. **Blocking Network Call inside Tokio Runtime (UI Freeze/Panic)**
   - **Status**: ✅ **Remediated**
   - **File**: `src/tui/app.rs` & `src/tui/event.rs`
   - **Description**: `reqwest::blocking::Client` was being used synchronously inside the TUI's `handle_tick` function. Because the TUI event loop is executed via a `#[tokio::main]` runtime, initializing a blocking client caused immediate runtime panics ("Cannot drop a runtime..."). Additionally, this design blocked the UI thread for up to 3 seconds during server health checks.
   - **Effort**: S
   - **Fix Applied**: Upgraded the `EventHandler` to run on a background thread utilizing Tokio `mpsc` channels. Converted the model name fetch process into a fully asynchronous Tokio task that yields `Event::ModelFetched(model)` back to the UI thread, eliminating all blocking bottlenecks.

### P2: Bad Practices & Documentation Debt (Fixed)
1. **Redundant HTTP Client (Feature Bloat)**
   - **Status**: ✅ **Remediated**
   - **File**: `Cargo.toml`
   - **Description**: The project included `reqwest` with the `blocking` feature enabled just for the legacy synchronous TUI call, creating bloat and increasing compile times.
   - **Effort**: XS
   - **Fix Applied**: Dropped the `blocking` feature constraint in `Cargo.toml`.

2. **Zombie Post-Transcribe Hooks**
   - **Status**: ✅ **Remediated**
   - **File**: `src/main.rs`
   - **Description**: In `run_post_transcribe_hook()`, a detached thread spawned a `Command` without reaping its exit status (no `wait()`), leading to zombie processes on the system during long-running VAD daemon usage.
   - **Effort**: XS
   - **Fix Applied**: Bound the spawned command to a local mutable variable and executed `child.wait()` to ensure proper process reaping.

### P4: Technical Debt & Refactoring (Fixed)
1. **Lack of abstraction in `tui::run` for async fetching**
   - **Status**: ✅ **Remediated**
   - **File**: `src/tui/mod.rs` & `src/tui/app.rs`
   - **Description**: The TUI application was not wired for asynchronous inter-task communication.
   - **Effort**: S
   - **Fix Applied**: Passed an `UnboundedSender<Event>` into the App context to unlock seamless async background processing for IO operations.

---

## Architectural Critique & Future Directions

### Current Architecture Assessment
The current architecture splits concerns neatly into standard desktop abstractions and process management modules. Persisting application state via `$XDG_RUNTIME_DIR/ears/state` makes integration with window managers (like Waybar) effortless. However, maintaining heavy integration layers over shell commands (e.g., `paplay`, `wtype`, `ydotool`) is fragile in complex environment contexts. Relying on parsed outputs (like `hyprctl devices -j` and `dconf`) is acceptable for a rapid MVP, but represents a longer-term stability risk.

### P5 & P6: Strategic Vision & Expansions
These items require consensus and represent future directions beyond the scope of immediate remediation:

1. **Wayland Native Protocols (`zwp_virtual_keyboard_v1`)**
   Instead of using `wtype` (which relies on Hyprland explicitly) or `ydotool` (which requires sudo/daemon setup), `ears` could leverage `wayland-client` crate bindings to directly interface with `zwp_virtual_keyboard_v1`. This would dramatically reduce external dependencies and bring typing latency down to strictly zero.

2. **Native Audio Processing (CPAL & Rodio)**
   Currently, the project leans heavily on `pw-record` combined with brittle polling to ensure the subprocess is alive. Incorporating a crate like `cpal` to natively ingest PipeWire audio would remove subprocess reliance, enabling pure internal VAD buffer streams.

3. **In-Flight Grammar Correction (Local LLM Integration)**
   The local ASR (Whisper) sometimes produces contextless hallucinations or stutters. By piping the raw transcription through a locally hosted small LLM (e.g., Llama 3 8B via Ollama), `ears` could format, punctuate, and correct capitalization intelligently based on user intent.
