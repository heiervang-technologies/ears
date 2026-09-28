# Changelog

All notable changes to the ears project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Fixed
- VAD pipeline could freeze forever after a transcription: typing helpers (`wtype`, `ydotool`) and desktop probes (`hyprctl`, `which`) were run synchronously on the async worker with no deadline. Every such child is now spawned with a bounded wait that kills and reaps it on expiry, and typing runs via `block_in_place` so the capture reader and event loop keep running. A typing timeout is reported as an error (text may be partially delivered) and is never retried.
- Volume stayed ducked when a probable-speech candidate was rejected before confirmation. The engine now emits `SpeechRejected` on that edge (no audio cue) and the ducker restores on it. Duck/restore operations are serialized and epoch-guarded so a late duck can never run after a restore.
- Microphone loss was invisible: `ContinuousCapture::is_running()` reported false immediately after `start()`, `stop()` was a no-op, and a dead `pw-record` left the pipeline idling with a healthy-looking status. The child is now owned in a shared slot (kill + reap from `stop()`/`Drop`), reader liveness is published through `status_rx()`, and the pipeline emits `CaptureStopped { reason }`. The TUI turns listening off and logs the reason; headless `ears vad` exits and resets the state file to idle.
- Desktop capability detection (`hyprctl` + `which wtype`) is probed once per session with bounded probes instead of on every segment; `TextInput::refresh_capabilities()` forces a re-probe after a typing backend failure.

### Added
- Independent desktop VAD health supervision with atomic `vad-health.json` snapshots: exact owner identity, microphone and detector progress, probability/rejection measurements, processing stage, backlog, and typing suspension. Opt-in periodic debug summaries help distinguish capture loss, rejected speech, and stalled downstream work without recording audio or transcripts. Only one desktop health owner per state directory may start.
- Typing or Enter failure pauses further keyboard input while transcription continues; check the target and restart listening to resume. Clipboard preservation failures abort paste before mutation.
- `StreamingEvent::SpeechRejected` and `StreamingEvent::CaptureStopped { reason }` (additive; existing variants and payloads unchanged).
- Deterministic probability-sequence tests for the VAD state machine and engine events (candidate rejection, confirmation, silence termination, second/third utterances) and fake-backend ducking tests including the late-duck race.

## [1.0.0] - 2026-05-09

First public release. Ears is now considered stable and ready for general use.

### Added
- `ears test [FILE]` — validate the active profile: prints a summary (server, endpoint, model, device, language, masked API key), runs a server health check, and optionally transcribes a sample audio file. Exits non-zero on failure.
- Warning when the configured `server` URL ends in `/v1` — ears appends `/v1/audio/transcriptions` itself, so a trailing `/v1` produces a doubled `/v1/v1/...` path that 404s.
- Volume ducking — optionally lowers system volume while VAD detects speech, restoring on speech end. Toggle and percentage adjustable in TUI config panel.
- TUI header refresh — bolder branding, right-aligned status indicator, version display
- README — sharper opening hook, updated feature list, accurate project structure

### Changed
- Release workflow now reads version from `Cargo.toml` instead of auto-bumping from commit count. Releases happen on intentional version bumps.
- Versioning reset to `1.0.0` to mark the first stable public release.
- Config files are now written with `0600` permissions, since they may contain a plaintext `api_key`.
- README: documented the `{server}/v1/audio/transcriptions` endpoint behavior, the `ears test` command, the plaintext-key/0600 note, and that `EARS_*` env overrides do not reach keybind-launched `ears toggle`.

### Fixed
- TUI: `save_config()` no longer clobbers the user's real config. Unit tests construct a real `App` (reading `~/.config/ears`) and exercise toggle keys; a concurrent env-override test could make config loading fail, and the previous `.unwrap_or_default()` then wrote a default config over the user's real settings. Now a no-op under `cfg!(test)` and skips the save on load failure instead of writing defaults.
- Streaming: fixed UTF-8 panic in LocalAgreementPolicy when history window slides and committed text is not a prefix of new stable prefix (byte-based slicing replaced with char-based)
- Progressive typing: backspace now sends proper BackSpace key events (batched wtype or ydotool) instead of \x08 control characters, respecting configured typing mode
- Progressive typing: fixed index-out-of-bounds panic when committed text is shorter than typed text with auto-correction disabled
- Progressive typing: backspace count now uses char count instead of byte length (fixes multi-byte character handling)
- TUI event handler: terminal read/poll errors are now logged via tracing instead of silently masked as FocusGained events

### Added
- Bash mode: constrain dictation to valid shell syntax via grammar-guided decoding. Enable with `bash_mode = true` (optional `guided_grammar` override); routes requests to `/v1/chat/completions` with `structured_outputs.grammar` since the transcription endpoint does not support guided decoding. Built-in grammar in `grammars/bash.gbnf`. Toggle live in the TUI config panel with `g`. Best used with push-to-talk.
- `docs/ARCHITECTURE.md` — system design documentation covering state machine, audio pipeline, VAD, IPC protocol, and TUI architecture
- `LICENSE` — MIT license file (was declared in Cargo.toml but missing)
- `CHANGELOG.md` — this file
- Comprehensive test coverage for state machine transitions, config edge cases, audio device parsing, streaming UTF-8 handling, and progressive typing

## [1.1.121] - 2026-03-13

### Changed
- Technical debt cleanup: removed dead code, improved logging, deduplicated IPC socket helpers

## [1.1.119] - 2026-03-12

### Added
- `ears auto-enter` command with live IPC toggle for auto-enter setting

## [1.1.117] - 2026-03-11

### Fixed
- Text filters (lowercase, remove punctuation) now applied correctly in VAD mode

## [1.1.115] - 2026-03-10

### Fixed
- VAD audio cues volume and reliability improvements

## [1.1.113] - 2026-03-09

### Fixed
- TUI typing settings now persist to active profile config file

## [1.1.111] - 2026-03-08

### Added
- VAD audio feedback sounds (start/stop/error beeps)
- Fixed toggle/VAD mode conflicts

## [1.1.109] - 2026-03-07

### Added
- WebSocket audio input mode (`ears ws-listen`)
