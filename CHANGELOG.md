# Changelog

All notable changes to the ears project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Added
- `ears test [FILE]` — validate the active profile: prints a summary (server, endpoint, model, device, language, masked API key), runs a server health check, and optionally transcribes a sample audio file. Exits non-zero on failure.
- Warning when the configured `server` URL ends in `/v1` — ears appends `/v1/audio/transcriptions` itself, so a trailing `/v1` produces a doubled `/v1/v1/...` path that 404s.

### Changed
- Config files are now written with `0600` permissions, since they may contain a plaintext `api_key`.
- README: documented the `{server}/v1/audio/transcriptions` endpoint behavior, the `ears test` command, the plaintext-key/0600 note, and that `EARS_*` env overrides do not reach keybind-launched `ears toggle`.

### Fixed
- Streaming: fixed UTF-8 panic in LocalAgreementPolicy when history window slides and committed text is not a prefix of new stable prefix (byte-based slicing replaced with char-based)
- Progressive typing: backspace now sends proper BackSpace key events (batched wtype or ydotool) instead of \x08 control characters, respecting configured typing mode
- Progressive typing: fixed index-out-of-bounds panic when committed text is shorter than typed text with auto-correction disabled
- Progressive typing: backspace count now uses char count instead of byte length (fixes multi-byte character handling)
- TUI event handler: terminal read/poll errors are now logged via tracing instead of silently masked as FocusGained events

### Added
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
