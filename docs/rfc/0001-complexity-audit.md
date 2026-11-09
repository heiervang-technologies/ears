# RFC 0001: Complexity audit

- Status: draft, for discussion
- Date: 2026-11-09
- Scope: `main` at `0e50d32`, plus the live setup on centurion (config, waybar, Hyprland binds, running processes, `debug.log` from 2026-11-07 to 2026-11-09)
- Question (Markus): how complex is the implementation compared with what it could be, without giving up any quality of life?

## Summary

ears has 34.9k lines of Rust in `src/`: 23.6k of code and 11.2k of inline tests. There are another 6.1k lines of Rust in `tests/` and 1.3k lines of bats, mocks and runner scripts. On top of that sit three side components:

| Component | Lines |
|---|---|
| fcitx5 addon (C++) | 0.5k |
| vLLM stream plugin (Python, 0.9k of it tests) | 2.2k |
| moonshine-stream (Python) | 1.0k |

Most of that size pays for real features that are used every day: ghost text, the live stream, voice commands, standby, command mode, decisions, say-echo, subtitles and ws-listen for Fay. The audit found **no single feature that is both big and unused**, apart from the TUI.

The excess is in four places:

1. **Code that no longer does anything.** In the engine, a LocalAgreement policy is fed every transcript twice so that it always commits, and an audio ring buffer is written to but never read. Progressive typing has backspace and correction branches that cannot run. The bash-era bats suite tests a script that was deleted in January. Finally, about 1,000 lines of "QA round 3" tests only print and assert nothing.
2. **The same thing written two or three times.**
   - The VAD pipeline is built three times: `ears vad`, `ears ws-listen` and the TUI.
   - The IPC command loop is also written three times.
   - The live-partial fallback chain (stream, then per-tick HTTP, then repeat) exists twice: once in the engine and once in the push-to-talk preview.
   - There are four WAV readers, two with WAV header constants that disagree (44 and 78 bytes).
   - There are two ASR health probes and three `vad.pid` liveness checks.
3. **Modes stored as separate flags.** `StreamingEngine` has 48 fields. Standby, command, vision, the open decision, the input claim, typing off and typing suspended are each their own bool or Option. Each mode has its own marker file, and `haios-ears-led` rebuilds the current mode from those files with its own precedence rules. `process_segment` is 473 lines long, with 21 early returns.
4. **A front end nobody launches.** The TUI is the default command, but nothing on the desk starts it. It is 4.3k lines of `src/` plus about 2.3k lines of tests and three dependencies. It has had no feature work since 2026-10-01; since then it has only received small changes to keep up with the engine.

Two cheap operational fixes also turned up:

- **Log noise.** Waybar runs `ears typing status` every 2 seconds, and every CLI run logs two lines. Those are 83k of the 111k lines in `debug.log`. ONNX Runtime INFO adds another 24k.
- **A post-transcribe hook that VAD skips.** `~/.config/ears/hooks/post-transcribe` exists, but only push-to-talk runs it.

**Safe simplifications** (behaviour unchanged) remove about 4,500 lines, most of it dead tests and dead code. **Decisions for Markus** could remove about 8,000 more; the TUI alone accounts for most of that.

## Method

- Line counts come from `wc -l`. Inline tests are counted from the first `#[cfg(test)]` line in each file. Function lengths come from brace matching.
- Code paths were traced by reading the code. Callers were checked with grep, and last-touch dates come from `git log`.
- Usage comes from:
  - `~/.config/ears/config.toml`
  - `$XDG_RUNTIME_DIR/ears/debug.log`, about 2 days. It mixes the VAD daemon and the two Fay `ws-listen` processes.
  - waybar, Hyprland and keyd config
  - `pgrep`

  Everything was read-only. The VAD daemon was not running when this was written, but both ws-listen services were.
- Line deltas are estimates and include the tests that go with the code.

## 1. Size map

### 1.1 Rust by area (`src/`, code lines excluding inline tests)

| Area | Files | Code | Inline tests |
|---|---|---|---|
| Engine | `streaming_engine.rs` 2,307, `streaming_engine/ghost.rs` 915, `streaming_engine/tests.rs` (tests only) | 3,222 | 1,712 |
| Front ends | `main.rs` 1,236, `tui/` 3,580, `ws_input.rs` 159, `ipc*` 376 | 5,351 | 1,103 |
| ASR clients | `whisper.rs` 773, `continuous.rs` 545, `stream_client.rs` 660, `streaming.rs` 257, `endpoint.rs` 95 | 2,330 | 1,415 |
| Capture and VAD | `vad.rs` 812, `continuous_capture.rs` 350, `agc.rs` 146, `wake.rs` 143, `pcm.rs` 195, `audio.rs` 126, `process.rs` 203, `pipeline.rs` 88 | 2,063 | 1,135 |
| Text out | `desktop.rs` 968, `progressive_typing.rs` 239, `ghost.rs` 512, `ghost_preview.rs` 674, `ghost_style.rs` 444, `ghost_terminal.rs` 272, `typing_switch.rs` 131, `freeze.rs` 31, `shell_focus.rs` 120, `pointer.rs` 211 | 3,602 | 1,503 |
| Voice control | `commands.rs` 872, `command_mode.rs` 887, `decide.rs` 390, `colour_mode.rs` 64, `subtitles.rs` 201, `say_echo.rs` 407, `bash.rs` 105, `vision.rs` 686 | 3,612 | 1,312 |
| Plumbing | `config.rs` 832, `cli.rs` 310, `health.rs` 677, `state.rs` 307, `lock.rs` 146, `ducker.rs` 290, `mic_check.rs` 426, `text_filters.rs` 166, `post_transcribe.rs` 75, `prompt.rs` 81, `owned_file.rs` 67, `lib.rs` 92 | 3,469 | 1,946 |
| **Total** | 56 files | **23,649** | **11,214** |

### 1.2 Tests and side components

| What | Lines | Notes |
|---|---|---|
| Inline `#[test]` | 11,214 | 544 tests |
| `tests/*.rs` | 6,086 | 34 files, 193 tests, 8 insta snapshots |
| bats, mocks, `cross-language/` | 1,326 | targets `bin/ears`, the bash script deleted in #69; not in CI |
| `fcitx5-addon/` | 544 | |
| `vllm-plugin/` | 2,206 | 893 of them tests; runs in `components-ci.yml` |
| `moonshine-stream/` | 974 | |
| `contrib/`, `examples/`, `scripts/` | 1,113 | |
| Docs | 3,062 | README alone is 988 |

The Rust test-to-code ratio is about 0.73 (17.3k test lines to 23.6k code lines). That is healthy. The problem is that about 3,300 of those test lines test nothing that still exists, or assert nothing (section 2.7).

### 1.3 Largest functions

| Lines | Function |
|---|---|
| 480 | `tui/ui.rs:149 render_config_panel` |
| 473 | `streaming_engine.rs:531 StreamingEngine::process_segment`: 21 early returns, about 50 if/match arms |
| 294 | `main.rs:892 handle_vad` |
| 271 | `tui/app.rs:436 handle_key` |
| 243 | `tui/mod.rs:256 run` |
| 233 | `tui/ui.rs:837 render_live_transcription_panel` |
| 206 | `main.rs:24 main` |
| 202 | `ghost_preview.rs:396 run_ghost_preview` |
| 197 | `main.rs:1269 handle_ws_listen` |
| 183 | `main.rs:295 handle_look` |

### 1.4 Surface

- **Config:** 79 keys, plus `commands.run.<name>` entries with 7 keys each, plus 7 `EARS_*` environment overrides. Every field has a reader. The live config sets all but about 15 of them. The ones it never sets are `language`, `api_key`, `model`, `prompt`, `guided_grammar`, `live_rollback_words`, `live_min_step_ms`, `vad.end_threshold`, `vad.command_silence_ms`, `vad.finished_silence_ms`, `ghost.alacritty`, `commands.tail`, and `run.prefix`/`rest_flag`/`while_running`.
- **CLI:** 25 subcommands. These include the hidden `ghost-preview`, plus `select`, `list` and `current`, which are hidden duplicates of `device …`.
- **Modes and features in the VAD path:**

| Feature | Live evidence |
|---|---|
| Dictation | 345 transcriptions (VAD and ws-listen) |
| Ghost preedit | used throughout |
| Stream final committed | 69 |
| Standby | 11 times; wake and sleep phrases |
| Voice commands | 45 |
| Run commands | 6, e.g. call-agent and pause-music |
| Command mode (purple) | entered 5 times, 8 actions fired |
| Colour words | 6 |
| Decide (red) | 4 |
| Vision (pink) | enabled in config; 0 uses in this window |
| Subtitles | on |
| Bash grammar | off |
| say-echo | 11 drops or cuts |
| Text-field gate | 10 drops |
| Steer | 1 |
| Early end (`endpoint`) | 28 |
| Input claim | IPC only |
| ws-listen | 2 services for Fay |
| Push-to-talk `toggle --ghost` | bound to an Esc tap; 1 recording in the window |
| TUI | no launcher anywhere |

- **Chant mode** is not on `main`. It is PR #264 (+954 lines) and is not in the installed binary.

## 2. Findings by area

### 2.1 Engine and modes

**What it buys.** One VAD utterance can be dropped, verified, matched against a wake phrase, routed to command mode, a decision or vision, turned into a voice command, have its echo filtered out, be gated to text fields, be typed or ghost-committed, and finish with a tail command ("... over"). This is the heart of the daily workflow, and `streaming_engine/tests.rs` (1,712 lines) covers it well.

**What it costs.**
- **Field count.** `StreamingEngine` has 48 fields.
- **Mode state lives in seven places:**
  - `standby: bool`
  - `command_mode: Option<CommandMode>`
  - `vision_mode: bool`
  - `decisions` (an open decision)
  - `claimed: bool`
  - `typing_mode == None`, which is typing off and the yellow route
  - `typing_suspended: bool`
- **Separate state for one choice.** The grammar choice is held in three fields (`guided_grammar`, `configured_grammar`, `shell_grammar`).
- **Mode markers.** Each mode writes its own file in the runtime directory: `vad.standby`, `vad.command`, `vad.vision`, `decision.json` and `subtitles.on`. `haios-ears-led` reads them one by one and applies its own precedence (`haios-ears-led:114-188`).
- **Precedence rules in two places.** `subtitle_mode()` (`streaming_engine.rs:1777`) already derives a single mode from the flags, so the precedence rules exist twice: once in Rust and once in Python.
- **Transitions are spread out.** They live in `apply_wake`, `switch_to_colour`, `enter_command_mode`/`leave_command_mode`, `enter_vision_mode`/`leave_vision_mode` and `request_*`. Each must remember to leave the other modes. #236 and #237 were bugs of exactly this kind.

**Vestigial machinery.**
- `commit_transcript` resets a `LocalAgreementPolicy` and then feeds it the same transcript twice "to force LocalAgreement to commit it immediately" (`streaming_engine.rs:1368-1392`). The result is always the transcript.
- `audio_buffer` is written on every chunk (`:410`) and only ever cleared.
- `StreamingConfig.chunk_size_ms`, `buffer_size_seconds` and `auto_correction` are never read.
- `AudioBuffer`, `TranscriptChunk` and `StreamingError` in `streaming.rs` are only re-exported and tested.
- Uncalled functions: `update_config`, `update_typing_config` and `ghost_enabled`.

**Simpler design with the same behaviour.**
- **One mode enum.** Replace the mode flags with `enum Mode { Dictation, Standby, Command(CommandMode), Vision }`. Keep the decision as an overlay, because it takes every utterance in any mode. Keep `Output { Type, Off, Claimed, Suspended }` as a second axis.
- **One transition function.** `fn set_mode(&mut self, to: Mode, heard: &str)` would write the markers, play the cue, publish the event and leave the old mode.
- **One mode file.** Write a single `mode.json` (`{mode, decision, output, subtitles}`) next to the old markers, so the LED and `ears prompt` read one file. Remove the markers once `haios-ears-led` has moved over.
- **Split `process_segment` into stages:** gate (drop/verify), transcribe (stream final or HTTP), screen (say-echo, sleep-on-language, filters), route (mode/command/wake), deliver (sink plus tail command). Each stage returns `Continue(T)` or `Done(reason)`, and the 21 `self.discard(..); return Ok(())` exits become one match.

### 2.2 Live partials and transcription paths

**What it buys.** Ghost text that tracks speech within about 150 ms, a committed final with no second decode (`final_correction=false`), early end of an utterance on "over" or a finished sentence, and survival when the ASR server goes away.

**Paths.**

| Path | Code | Live use |
|---|---|---|
| A. Push-to-talk: WAV file → `/v1/audio/transcriptions` | `main.rs:1468-1865` | Esc tap |
| A′. Push-to-talk live preview: hidden `ghost-preview` that tails the WAV | `ghost_preview.rs`, 674 | with A |
| B. VAD segment → HTTP final | `process_segment` | yes |
| B′. Stream final committed instead of B | `ghost.rs:553` | 69 times |
| C. Partials over the ears stream (`/v1/ears/stream`) | `stream_client.rs`, 660 | 43 sessions |
| D. Partials as one chat-completions call per tick | `continuous.rs`, 545 | fallback; ran twice on 11-08 after "Ears stream lost (closed by server)" |
| E. Partials by re-transcribing the whole clip (`live_decoding=repeat`, the default) | `ghost.rs:808-830` | servers without Qwen3 |
| F. ws-listen: remote audio into the same engine | `main.rs:1269-1467`, `ws_input.rs` | Fay, 2 services |
| G. Grammar-guided decoding through chat-completions | `whisper.rs:321…` | off |

**Duplication.**
- The C → D → E fallback is implemented twice: in `streaming_engine/ghost.rs` for VAD, and in `ghost_preview.rs` (`stream_ghost_preview` `:256`, `run_ghost_preview` `:396`, `finish_continuous` `:603`) for push-to-talk.
- D and C send the same Qwen3-ASR windows with a forced prefix. C just keeps that state on the server.
- `WhisperClient::health_check` and `health::probe_asr` do the same `/health`-then-`/v1/models` probe.
- WAV is read in four places: `main.rs:1723`, `ghost.rs:215`, `vad.rs:770`, and the validation in `whisper.rs`. `whisper::WAV_HEADER_SIZE = 78` (re-exported from `lib.rs`) disagrees with `pcm::WAV_HEADER_LEN = 44`.

**whisper.cpp.** Nothing in the code is specific to whisper.cpp any more: ears only posts to `/v1/audio/transcriptions`. What remains is the default URL `127.0.0.1:8178`, the `WhisperClient` name and doc comments. There is nothing to remove; a rename is optional.

**Simpler design.**
- **One `LivePartials` driver** that both the engine and push-to-talk use: about −250 lines.
- **Dropping D** is possible, but it is a real fallback today: it ran when the stream dropped mid-session. Reconnecting the stream on the next utterance gives the same result with less code. Then D goes: about −600 lines including tests. See decision D3.

### 2.3 Text output

**What it buys.**
- **fcitx5 ghost commit** is the primary path. It works in any app with an input method, with no wtype races.
- **wtype** is the fallback when the addon does not deliver (logged once in this window).
- **ydotool** presses Enter, Shift+Enter, Backspace and key sequences on every compositor.
- **wl-copy plus ydotool Ctrl+V** is the paste path, chosen automatically on niri and other non-Hyprland compositors; niri support landed in #261.
- **The tmux wrap and popup** handle ghost text that overflows the pane (`ghost_terminal.rs`).

**Costs and dead parts.**
- `progressive_typing` and `auto_correction` are hardcoded in headless VAD (`main.rs:980-981`: `progressive_typing: typing`, `auto_correction: true`). The config keys only affect the TUI.
- Progressive typing's correction and backspace branches (`progressive_typing.rs:75-195`) cannot run. `accumulated_text` is only ever appended to, and the undo commands re-sync it with `set_typed_text`. Only `tests/progressive_backspace.rs` exercises them.
- Backspace is implemented twice, and the two behave differently: `progressive_typing.rs` uses wtype or ydotool, `desktop.rs:765` uses ydotool only. The ydotool spawn-and-status boilerplate is copied four times in `desktop.rs:720-819`.
- Ghost commit, then type fallback, then refuse when focus is unknown: this logic exists in `streaming_engine/ghost.rs:836-891` and again in `main.rs:1798-1813` plus `ghost_preview.rs:185-203`.
- `process_segment` has three delivery branches: ghost, guided, progressive (`:898-937`).
- Dead functions: `TextInput::send_keys`, `type_text_with_delay` (called only from a test), and the `pub` `shell_focus::foreground_command`/`terminal_foreground`, which are only used inside their own module.
- Possible bug: the Hyprland/Omarchy detection only checks `hyprctl`. On niri, where wtype also works, it falls to the paste path.

**Simpler design.**
- **One sink type:** `trait TextSink { preview, commit, delete, keys }`, implemented by `GhostSink { fallback }`, `KeystrokeSink { Wtype | Paste }` and `NullSink`, plus a clipboard tee. The engine and push-to-talk would both call `sink.commit(text)`. Progressive typing becomes "commit the new suffix".
- **Keep the paste path.** It is how ears works on niri. Keep ydotool too: it is not optional, because it presses the keys.

### 2.4 Front ends: VAD daemon, ws-listen, TUI, push-to-talk

- **`ears vad`.** This is the daily driver: started from the waybar click or an Esc hold. Its pipeline builder, `start_vad_pipeline`, and the `TypingSettings` type live in `src/tui/mod.rs:56-253`, so the daemon depends on the TUI module.
- **`ears ws-listen`.** Fay's input; two systemd services are running now. `handle_ws_listen` is a hand copy of the pipeline builder ("like start_vad_pipeline but without pw-record"). It has its own IPC command loop and gets standby and command mode only where the copy remembered to add them.
- **TUI.**
  - It offers live toggles for config fields, a device picker, profile and language cycling, a `/v1/models` list, a log view with search and export, themes, and the volume ducker UI.
  - It ignores the Standby, CommandMode and Claim IPC commands (`tui/mod.rs:347-356`) and has no wake phrases, so it already lags the daemon.
  - Nothing on the desk launches it.
  - The ducker (`ducker.rs`, 290 code lines) is wired only into the TUI, and `duck_enabled=false`.
- **Push-to-talk.** Bound through keyd: an Esc tap sends F14, which runs `ears toggle --ghost`. It was used once in the log window. It is the only path that runs the post-transcribe hook, and the only user of `ghost_preview.rs`.

**Simpler design.**
- Move the pipeline builder and one shared IPC command loop into `src/vad_daemon.rs`, and build `vad`, `ws-listen` and (while it lives) the TUI from it. ws-listen then gets every IPC command for free.
- `main.rs` (1,236 code lines) shrinks to dispatch once these move into their own modules:

| From `main.rs` | To |
|---|---|
| `handle_look` | `vision.rs` |
| `handle_decide` | `decide.rs` |
| `handle_toggle` / `start_recording` / `stop_and_transcribe` | `push_to_talk.rs` |
| device / profile / server | `config.rs`, `audio.rs` |

### 2.5 Voice control features

| Feature | Size | Verdict |
|---|---|---|
| Voice commands, run commands, keys, tail commands | `commands.rs`, 872 | used constantly; keep |
| Standby and wake phrases | `wake.rs` 143, plus the engine | used; keep, fold into `Mode` |
| Command mode (embeddinggemma2 kNN) | `command_mode.rs`, 887 | used; keep |
| Decide (red) | `decide.rs` 390, plus about 150 in the engine | used by agents; keep |
| Colour words | `colour_mode.rs` 64, plus about 75 in the engine | used; keep, becomes `set_mode` calls |
| Subtitles | `subtitles.rs`, 201 | on; keep |
| say-echo | `say_echo.rs`, 407 | 11 hits in 2 days; keep |
| Vision / pink | `vision.rs` 686, `pointer.rs` 211, `handle_look` 183 | new (11-04), enabled, no uses in the window; keep for now |
| Bash grammar | `bash.rs` 105, `shell_focus.rs` 120, `grammars/` 115, about 150 lines across the engine, whisper, config and stream client | off live; see D4 |

**Duplication.** Decide and command mode both hear by "words, else embeddinggemma2 on the audio":
- Command mode runs ASR and the embedding in parallel, with a 1.5 s ASR budget.
- Decide runs them one after the other, and re-reads `actions.toml` on every utterance to find the embedding server (`streaming_engine.rs:2008-2054`).

A shared `hear(words_future, audio, options)` helper would remove about 60 lines and give decisions the same latency budget.

### 2.6 CLI and config

- **Hidden duplicates.** `select`, `list` and `current` duplicate `device select|list|current` and have no references on the desk. `ears ghost` is `ears vad` with ghost forced on, which the live config already sets.
- **Switch subcommands.** `typing`, `standby`, `command`, `subtitles` and `auto-enter` are five copies of an on/off/toggle/status switch. `subtitles` takes a free-form string, and `auto-enter` only toggles. One generic switch handler saves about 70 lines and makes them behave alike.
- **Legacy migration.** `Config::migrate_old_files` (`config.rs:523-620`, plus tests) converts the bash-era flat files. That script was deleted on 2026-01-09. A stale `~/.config/ears/server` is still on the desk and ignored. The migration also repeats the full `Config` struct literal.
- **Dead method.** `Config::save_text_filters` has no callers.
- **Stale references.** `ears claim` is mentioned in two doc comments, but claims are IPC-only. `CLAUDE.md` lists `recording.rs`, which does not exist.

### 2.7 Tests

| Class | Files | Lines | Proposal |
|---|---|---|---|
| bats suite for the deleted bash `bin/ears`, mocks, cross-language runner | `tests/unit`, `tests/integration`, `tests/mocks`, `tests/cross-language`, `run_tests.sh`, `test_helper.bash`, bats-only fixtures | 1,326 | delete |
| Print-only "investigations" with 0-1 asserts | `qa_round3_timeout_bug` (0 asserts, 42 printlns), `qa_round3_zombie_process` (tests `sleep`/`ps`, not ears), `qa_round3_config_edge_cases`, `test_desktop_integration`, `investigate_scroll`, `bug_log_scroll_after_toggle` | 1,028 | delete; move any single real assert into a topic test |
| Exploratory TUI tests, light on asserts | `tui_bug_hunter`, `tui_explorer`, `tui_interaction_demo`, `qa_agent_investigation` | 1,203 | delete with the TUI, or now |
| Mixed: real asserts with many printlns | the other `qa_round3_*`, `qa_ui_rendering_issues`, `qa_help_text_verification`, `test_whisper_url_bug` | about 1,900 | keep the asserts, rename by topic; about −40% |
| Real regression tests | `test_vad_audio`, `whisper_integration`, `helper_deadlines`, `profile_switch`, `config_io_errors`, `clipboard_safety`, `recording_cleanup`, `test_state_race_condition`, `test_strict_alphabet`, `test_url_validation`, plus the TUI ones while the TUI lives | about 2,000 | keep |

### 2.8 Runtime state and logging

- **Lock and liveness checks.**
  - `vad.pid` is checked three ways: `main.rs:882 vad_running`, `state.rs:37 is_external_vad_alive` and `prompt.rs:39 pid_alive`.
  - `vad.lock` and `vad-health.lock` both guard the same process.
  - `ears.sock.lock` sits outside `$XDG_RUNTIME_DIR/ears/`.
- **Log spam.** `main.rs:91-92` logs "ears started" and "Log file: …" for every command except `prompt` and `vad-file`. Waybar's `custom/ears` (`interval: 2`) runs `ears typing status` through that path, which writes 41.5k pairs in 2 days. ONNX Runtime adds 24k INFO lines. About 97% of `debug.log` is noise, and it grows to about 9 MB in two days on tmpfs.
- **Hook skipped by VAD.** The post-transcribe hook runs only on push-to-talk (`main.rs:1840`), yet a hook is installed. VAD dictation silently skips it.

## 3. Proposals

Ranked by complexity removed divided by risk to quality of life. "Must keep working" lists what to check by hand on top of `cargo test`.

### 3.1 Safe simplifications (no behaviour change)

| # | Proposal | Files | Est. Δ lines | Risk | Must keep working |
|---|---|---|---|---|---|
| S1 | Quiet the log: log "started" at debug, or after the cheap subcommands (`typing status`, `prompt`); default filter `info,ort=warn` | `main.rs:80-92` | +6 (−97% of log volume) | none | `debug.log` still records VAD sessions and errors |
| S2 | Delete the bats, mocks and cross-language suite | `tests/unit`, `tests/integration`, `tests/mocks`, `tests/cross-language`, `tests/run_tests.sh`, `tests/test_helper.bash`, bats-only fixtures | −1,330 | none (not in CI; tests a deleted script) | CI green |
| S3 | Delete the print-only investigation tests | the six files in 2.7 | −1,030 | none | CI green |
| S4 | Remove vestigial engine machinery: LocalAgreement double-feed, write-only `AudioBuffer`, unused `StreamingConfig` fields, `TranscriptChunk`, `StreamingError`; and the uncalled `update_config`, `update_typing_config`, `ghost_enabled`, `send_keys`, `type_text_with_delay`, `save_text_filters`, `pid_file_path` | `streaming.rs` (to about 30 lines or gone), `streaming_engine.rs`, `desktop.rs`, `config.rs`, `process.rs`, `lib.rs` | −500 | very low | dictation text and spacing across utterances; undo, "back N", backtrack |
| S5 | Progressive typing becomes "type the new suffix". Drop the unreachable correction and backspace branches; accept the `progressive_typing` and `auto_correction` keys as no-ops with a warning | `progressive_typing.rs`, `tests/progressive_backspace.rs`, `config.rs`, `tui/app.rs` | −300 | low | typing with ghost off (wtype and paste); auto-Enter; typing-failure suspension |
| S6 | Remove the bash-era config migration and its duplicate struct literal | `config.rs:523-620` plus tests | −160 | very low (the desk migrated long ago) | loading config.toml and profiles |
| S7 | Small dedups: one WAV reader and one header constant; one ASR health probe; one `vad_alive()`; `ws_input` using `pcm::samples_f32`; one ydotool helper | `pcm.rs`, `whisper.rs`, `ghost.rs`, `vad.rs`, `main.rs`, `health.rs`, `state.rs`, `prompt.rs`, `ws_input.rs`, `desktop.rs` | −250 | low | push-to-talk WAV validation; `ears prompt`; waybar health state; ws-listen audio |
| S8 | CLI: drop the hidden `select`/`list`/`current`; make `ghost` a clap alias of `vad`; one generic switch handler for typing, standby, command, subtitles and auto-enter | `cli.rs`, `main.rs:547-638` | −115 | low (the hidden aliases have no users; `ghost` keeps working) | waybar middle-click `ears typing toggle`; `ears standby`/`command` from voice calls; `ears subtitles on` |
| S9 | One VAD pipeline builder and one IPC command loop in `src/vad_daemon.rs`, used by `vad`, `ws-listen` and the TUI | `tui/mod.rs:56-253`, `main.rs:892-1466` | −220 | low to medium | Fay's two ws-listen services (`--live` partials, `--keep-audio`, `--say-echo`); `ears vad start/stop/status`; capture restart after a lost device |
| S10 | One `Mode` enum, one `set_mode` transition, and `mode.json` written next to the old markers | `streaming_engine.rs`, `subtitles.rs`, `prompt.rs`, then `haios-ears-led` | −100 net (removes a class of mode bugs, see #236 and #237) | medium (covered by 1,712 lines of engine tests) | every colour word; "command"/"out"; wake and sleep phrases; a decision opened during standby or command mode; LED colours; `ears prompt --standby` |
| S11 | Split `process_segment` into gate → transcribe → screen → route → deliver | `streaming_engine.rs` | about 0 (473 lines become about 5 functions of 60-120) | medium; do it after S10 | as S10, plus tail commands, the text-field gate, steer and say-echo |
| S12 | A `TextSink` shared by the engine and push-to-talk | `desktop.rs`, `streaming_engine/ghost.rs`, `main.rs:1798-1813`, `ghost_preview.rs:185-203`, `progressive_typing.rs` | −200 | medium | ghost commit and its wtype fallback; refusing to type with unknown focus; paste on niri; clipboard copy; Enter and keys |
| S13 | One `LivePartials` driver (stream → per-tick → repeat) used by VAD and push-to-talk | `streaming_engine/ghost.rs`, `ghost_preview.rs` | −250 | medium | push-to-talk ghost on the Esc tap; ghost partials in VAD; recovery after a stream loss |
| S14 | Shared "hear by words and embedding" for decide and command mode; decide gets the parallel 1.5 s ASR budget | `streaming_engine.rs:1911-2054`, `command_mode.rs` | −60 | low (decisions get a little faster) | `ears decide` answers by words, by embedding, `--other`, and cancel |
| S15 | Run the post-transcribe hook on VAD commits too, or document that it is push-to-talk only | `streaming_engine.rs`, `post_transcribe.rs` | +15 | low | an installed hook sees VAD text |
| S16 | Fix the stale docs: `CLAUDE.md` (`recording.rs`), the `ears claim` comments, the whisper.cpp wording in the README | docs | ±0 | none | |

**Safe total: about −4,500 lines.** Of that, about 2,400 is tests that test nothing (S2, S3) and about 1,000 is dead code (S4 to S6). The rest is consolidation.

Suggested order:
1. S1 to S8, in any order. They are small and independent.
2. S9.
3. S10, then S11.
4. S12 and S13 together.

### 3.2 Decisions for Markus

| # | Question | If yes | Quality-of-life cost | Recommendation |
|---|---|---|---|---|
| D1 | Drop the TUI, the default `ears` command? | −4,300 `src/`, about −2,300 tests (`tui_*`, `qa_ui_rendering_issues`, `investigate_scroll`, snapshots), the ratatui, crossterm and insta dependencies, demo.gif/tape, and README sections. Bare `ears` would print help or status. | Loses the interactive settings panel, the log viewer with search and export, and the model list. Device selection stays (`ears device select`, fzf); config stays editable as TOML; `debug.log` remains. | Yes, after S9. Nothing launches it, and it already lags the daemon (no standby, command mode or claim). |
| D2 | The volume ducker: drop it, or wire it into `ears vad`? | Drop: −470 (`ducker.rs`, the TUI wiring, the `vad.duck_*` keys) | None today (`duck_enabled=false`; only the TUI uses it) | Drop together with D1, unless you want ducking in VAD |
| D3 | Replace the per-tick HTTP decoder (D) with a stream reconnect? | −600 (`continuous.rs`, its branches in `ghost.rs` and `ghost_preview.rs`, tests). A dropped stream reconnects on the next utterance; servers without the plugin use `repeat`. | Behaviour is the same on the vLLM plugin and moonshine-stream. A plain vLLM Qwen3-ASR without the plugin, such as a failover host, would show ghost text by `repeat`: correct, but more compute per tick. | Yes, if every ASR host you fail over to runs the stream plugin. Check rogue and big-dog first. |
| D4 | The bash grammar modes (`bash_mode`, `bash_terminal`, `guided_grammar`, shell detection) | Remove: about −600 (`bash.rs`, `shell_focus.rs`, `grammars/`, the guided path in `whisper.rs` and `stream_client.rs`, 42 references in the engine). Keep: collapse them into `enum Grammar { Off, Always(g), AtShellPrompt(g) }` (about −40). | Off on your desk, and the `config.bash.toml` profile has not been touched since August. But `bash_terminal` defaults to **true** for everyone else. | Your call. If kept, fold into `Grammar` and consider defaulting `bash_terminal` to false. |
| D5 | Drop `[language_servers]`? | about −60 | None on this desk (empty; `sleep_on_language` now covers Swedish) | Yes, unless another machine uses it |
| D6 | Chant mode (PR #264, +954) | Land it as a `Mode` variant on top of S10, not as more engine flags | None; it only changes how it lands | Merge S10 first, then rebase #264 onto it |
| D7 | Push-to-talk ghost preview (`ghost_preview.rs`, 674) | Keep (it is bound to the Esc tap) but build it from S12 and S13 rather than its own copies | None | Keep and consolidate. Do not drop. |

**Total if D1 to D5 all go:** about −8,000 more, which brings `src/` plus `tests/` (bats included) from about 42k to about 29.5k lines with the same daily behaviour.

### 3.3 Considered and kept

- **ydotool and the paste path.** ydotool presses every key ears sends, on every compositor. Paste is how text gets typed on niri (#261). Separately, the niri wtype detection looks like a bug worth fixing.
- **say-echo, vision, subtitles, decide, command mode, colour words.** All are recent, used or enabled, and reasonably contained. What they cost is their wiring into the engine, which S10 and S11 address, not their modules.
- **Profiles (`-p`).** About 70 lines, still used by one desk script.
- **The `whisper` naming.** Renaming `WhisperClient` to `AsrClient` is churn with no gain in behaviour. Do it only alongside S7.
- **Config phrase lists that merge with the built-ins.** Users cannot remove a default phrase. This is a design choice, not complexity; it is noted here only because it surprised the audit.
