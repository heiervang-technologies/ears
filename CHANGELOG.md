# Changelog

All notable changes to the ears project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

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
