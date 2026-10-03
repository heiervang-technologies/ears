# ears — Runtime Detection Map

Everything the `ears` binary detects, probes, or senses at runtime, with
source locations. Mapped from `468122e` (PR #140 head, the build installed on
2026-09-20). Line numbers refer to that commit; the ghost-completion branch
adds `src/ghost.rs` and hooks in `src/streaming_engine.rs` (see §12).
"Unbounded" means the probe has no timeout.

1. Environment / desktop
2. Audio
3. Voice activity (Silero VAD)
4. Transcription server
5. State files, processes, locks, health sidecar
6. IPC / control / WebSocket
7. Text processing
8. Config
9. CLI subcommands
10. Detection timing matrix
11. Behaviour the docs don't state
12. Ghost completion hooks

---

## 1. Environment / desktop

### 1.1 Compositor / "Omarchy" detection (typing backend)

| What | How | Where | When | Influences |
|---|---|---|---|---|
| Hyprland is running | `hyprctl version`, exit status only, 2 s timeout | `src/desktop.rs:744-755` | First `TypingMode::Auto` type or backspace, then cached | wtype vs paste |
| wtype on PATH | `which wtype`, 2 s timeout, only if Hyprland found | `src/desktop.rs:760-768` | Same | Same |
| Cache | `CAPABILITY_CACHE: Mutex<Option<bool>>` | `src/desktop.rs:457, 723-734` | Process lifetime | — |
| Invalidation | `refresh_capabilities()` after any Auto-mode typing error | `src/desktop.rs:737-741, 809-812` | After a typing failure | Next Auto call re-probes |

`is_omarchy() == true` means Auto uses `wtype`, otherwise clipboard paste.
Progressive-typing backspace follows the same rule
(`src/progressive_typing.rs:148-152`).

### 1.2 Keyboard layout, used as the transcription language

`KeyboardLayout::detect_language()` (`src/desktop.rs:57-71`) tries Hyprland,
then GNOME, else `None` (auto-detect).

| Step | Command / parse | Where | Notes |
|---|---|---|---|
| 1 | `hyprctl devices -j`, first `"active_keymap"` in `"keyboards"` | `src/desktop.rs:74-92, 119-147` | No `main` filter. Unbounded |
| 1a | Keymap name → code: english+us→`us`, english+uk→`gb`, norwegian/norsk→`no`, german→`de`, french→`fr`, spanish→`es`, swedish→`se`, danish→`dk`, finnish→`fi`; else a 2-char first word | `src/desktop.rs:150-189` | |
| 2 | Fallback `hyprctl getoption input:kb_layout`, first entry | `src/desktop.rs:95-113` | Unbounded |
| 3 | GNOME `dconf read /org/gnome/desktop/input-sources/mru-sources` | `src/desktop.rs:192-228` | Unbounded |
| Map | `us/gb/uk→en`, `no→no`, `de,fr,es`, `se→sv`, `dk→da`, `fi→fi`, else `None` | `src/desktop.rs:231-244` | |

The detected language always wins over `config.language`
(`detect_language().or_else(|| config.language.clone())`). Call sites: toggle
start/stop (`src/main.rs:816, 943`), `ears test` (`:269`), VAD pipeline start
(`src/tui/mod.rs:97`), headless `ears vad` settings (`src/main.rs:468`, re-run
on every auto-enter toggle), `ears ws-listen` (`src/main.rs:608`).

It drives the `language` request field, per-language server selection
(`src/config.rs:551-566`), the text-filter alphabet check and the Chinese
artifact filter.

### 1.3 Typing / key-injection backends

| Action | Command | Timeout | Where |
|---|---|---|---|
| Type (wtype) | `wtype -d 4 -- <text>` | 5 s + 20 ms/char | `src/desktop.rs:826-844, 466-475` |
| Paste | `wl-paste --no-newline` → `wl-copy -- <text>` → `ydotool key ctrl+v` → restore via `wl-copy` stdin | 5 s each; output capped at 1 MiB | `src/desktop.rs:849-911` |
| Enter | `ydotool key 28:1 28:0` after 50 ms | 5 s | `src/desktop.rs:776-794` |
| Backspace | `wtype -k BackSpace ×N` or `ydotool key 14:1 14:0 ×N` | 10 s | `src/progressive_typing.rs:137-194` |

Paste aborts before touching the clipboard if the read times out or overflows.
Timeouts kill and reap the child; callers never retry (`src/desktop.rs:477-511`).

### 1.4 Clipboard

`copy_to_clipboard` runs `wl-copy <text>` fire-and-forget with a reaper thread
(`src/desktop.rs:920-932`) when `save_to_clipboard` is on (toggle `src/main.rs:1000`,
headless vad `:529`, ws-listen `:726`, TUI `src/tui/app.rs:1463`). No availability probe.

### 1.5 Notifications and sound

- `notify-send -u <urgency> -a ears <msg>` (`src/desktop.rs:270-280`), toggle path only.
- `paplay --volume=<vol*65536/100>` (`src/desktop.rs:343-359`); volume 0 skips.
- Custom override `$HOME/.local/share/ears-sounds/<name>.wav`, checked per cue
  (`src/desktop.rs:334-340, 387-398`); embedded fallback written to
  `/tmp/ears-sound-<hash>.wav` keyed on a pointer address (`:365-384`).
- Cues: `start, done, bell, vad_open, vad_close, vad_speech, vad_speech_start,
  vad_speech_confirm, vad_end, toggle_on, toggle_off`.

### 1.6 Waybar

`pkill -RTMIN+9 waybar` on every state persist and force-reset (`src/state.rs:68-70, 200-202`).

### 1.7 Volume ducking

`wpctl get-volume/set-volume @DEFAULT_AUDIO_SINK@`, 3 s timeout, capped at 1.5
(`src/ducker.rs:45-75`). Duck on `SpeechProbable`; restore on `SpeechEnded`,
`SpeechRejected`, `CaptureStopped` and shutdown. Epoch plus op-lock prevent
stale ducks; `Drop` restores synchronously.

### 1.8 Terminal (TUI only)

crossterm raw mode, alternate screen, mouse capture; 250 ms input tick
(`src/tui/mod.rs:35-42`, `src/tui/event.rs:31-61`).

---

## 2. Audio

### 2.1 Device discovery

`pw-cli ls Node`, keeping `media.class = "Audio/Source"` nodes with name and
description (`src/audio.rs:16-96`), for `ears device list/select` and the TUI
`d` picker. Unbounded. `fzf` for interactive pick, `column -t` for printing.

### 2.2 Device selection

`config.device` (default `"default"`), overridden by `EARS_DEVICE`. Passed as
`pw-record --target <device>`; nothing validates it exists. The TUI blocks the
picker when `EARS_DEVICE` is set. Note: a sink *monitor* is not accepted as a
target; remap it into a source first (`pactl load-module module-remap-source`).

### 2.3 Capture

| Mode | Command | Where |
|---|---|---|
| Toggle | `timeout 120 pw-record --target D --rate 16000 --channels 1 --format s16 recording.wav`; PID → `recording.pid` | `src/process.rs:48-81` |
| VAD | `pw-record … -` to stdout; 1600-sample (100 ms) chunks, i16→f32, unbounded mpsc | `src/continuous_capture.rs:145-198, 228-349` |
| WS | WebSocket binary frames | §6.3 |

### 2.4 Capture liveness (VAD)

`CaptureStatus {Idle, Running, Stopped{reason}}` on a watch channel. EOF and
read errors become `Stopped` with the pw-record exit status; a requested stop
becomes `Idle`. A generation counter keeps a stale reader away from a
restarted child. On `Stopped` the pipeline emits `CaptureStopped{reason}` and
exits; headless plays the close cue and returns state to Idle, the TUI turns
listening off.

### 2.5 Levels (health only)

Per chunk: peak and RMS, `capture_last_ms`, `captured_samples`
(`src/health.rs:122-135`). Written to `vad-health.json` only.

### 2.6 Toggle-mode file validation

After SIGTERM (SIGKILL after 1 s) and 300 ms: `recording.wav` exists, is
larger than 44 bytes, and has `RIFF…WAVE` magic (`src/main.rs:887-930`).

---

## 3. Voice activity (Silero VAD)

### 3.1 Model

`voice_activity_detector = "0.2"` (Silero v5 via ONNX Runtime), 16 kHz,
512-sample (32 ms) frames (`src/vad.rs:13-16, 85-100`). Built once per
pipeline start.

### 3.2 Per-frame decision (every 32 ms)

- `is_speech = probability >= speech_threshold` (0.5).
- Confirm: not in speech and `speech_frames*32 >= min_speech_duration_ms`
  (300 → 10 frames).
- End: in speech and `silence_frames*32 >= max_silence_duration_ms`
  (1200 → 38 frames).
- Probable: `!in_speech && speech_frames > 0`. Rejected candidate:
  probable and the frame is below threshold. One dip resets the candidate.
- Every frame reports to health.

### 3.3 Segmentation

Re-frames 1600-sample chunks into 512-sample frames. A 500 ms pre-speech ring
(filled with every `Silence` frame, including unconfirmed candidate frames) is
prepended on confirmation. The segment is returned when speech ends. No
maximum segment length.

### 3.4 Events (edge-detected per 100 ms chunk)

| Event | Condition | Consumers |
|---|---|---|
| `SpeechProbable` | candidate started | cue, duck |
| `SpeechStarted` | confirmed | cue, TUI speaking |
| `SpeechEnded` | segment complete | cue, restore volume, transcription |
| `SpeechRejected` | candidate dropped unconfirmed | restore volume, clear ghost |

All events are also broadcast over IPC and WebSocket.

---

## 4. Transcription server

- **Endpoint selection:** `language_servers[lang]` else `server`/`model`
  (`src/config.rs:551-566`). A path ending in `/v1` warns but is not rewritten.
- **Health check:** `GET /health`, then `GET /v1/models`; bearer if set; 2 s
  connect, 30 s total, no retry (`src/whisper.rs:206-256`). Run at toggle
  start, VAD pipeline start, ws-listen and `ears test`.
- **Model discovery (TUI only):** `GET /v1/models`, `data[0].id`, display only.
- **Transcription:** `POST /v1/audio/transcriptions` multipart (file, json,
  language, model, prompt), or in bash mode `POST /v1/chat/completions` with
  base64 audio and a GBNF grammar. Retries with backoff up to 30 s.
  Post-filters drop "Thank you." style artifacts, CJK filler-only output and
  degenerate grammar tokens; empty becomes `EmptyTranscription`.
- **Preview transcription (ghost branch):** `transcribe_preview` does a single
  attempt with a 4 s deadline and no retry.

---

## 5. State files, processes, locks, health sidecar

State dir: `$XDG_RUNTIME_DIR/ears` (else `~/.cache/ears/run`). Config dir:
`~/.config/ears`.

| File | Meaning |
|---|---|
| `state` | `idle` / `recording` / `transcribing` / `vad_active` |
| `recording.pid`, `recording.wav` | toggle recorder |
| `toggle.lock` | serializes toggles |
| `vad.pid` | headless VAD (also `ears ghost`) PID; running either command again stops it |
| `vad.lock` | advisory only |
| `vad-health.lock`, `vad-health.json` | single health owner, 1 Hz snapshot |
| `segment_<n>.wav`, `ghost_partial_<n>.wav` | per-segment and per-partial audio, deleted after use |
| `ghost.sock` | earsghost fcitx5 addon socket (owned by fcitx5) |
| `debug.log` | tracing output, `RUST_LOG` |

Staleness detection: recorder PID liveness, stale `transcribing` reset,
stale `recording` with dead PID reset, VAD-active blocks toggle, external VAD
via `vad.pid` + `kill(pid,0)`.

`vad-health.json` fields: identity (`pid`, `process_start_ticks`, `boot_id`,
`session_id`, `session_kind`, `device`), progress (`stage`, `stage_since_ms`,
`capture_last_ms`, `captured_samples`, `processed_frames`, `queue_chunks`,
`audio_backlog_ms`), levels (`rms`, `peak`), detector (`probability`,
`threshold`, `candidate_frames`, `speaking`, `rejected_candidates`,
`last_rejected_frames`, `last_rejection_probability`, `last_rejection_peak`,
`gain`, `last_segment_mean_probability`, `dropped_segments`), and failures
(`capture_error`, `typing_paused`). `problem()` ranks: pipeline stopped,
capture stopped, no audio, typing paused, slow stage (5 s, 20 s while
transcribing), detection stalled, backlog.

---

## 6. IPC / control

- **Event socket** `$XDG_RUNTIME_DIR/ears.sock`: newline-delimited JSON of
  every `StreamingEvent`. Started by `ears vad`/`ears ghost` and the TUI.
  ws-listen uses `--socket` or `ears-ws.sock`.
- **Command socket** `$XDG_RUNTIME_DIR/ears-cmd.sock`: `toggle-auto-enter`,
  and on main `typing-on|off|toggle|status` (runtime typing switch).
- **WebSocket** `ears ws-listen`, default `0.0.0.0:8765`, no auth.
  `{"type":"start"}`/`{"type":"end"}` delimit sessions; binary frames are
  s16le 16 kHz mono; events are echoed back. No capture, health or typing.

---

## 7. Text processing

Per VAD segment: skip empty, save WAV, transcribe, filters (bypassed in bash
mode), LocalAgreement fed twice (always commits), accumulate, output
(progressive typing, or ghost commit), events. Filters: `strict_alphabet`
(default on; en/no/es/fr/de only), `remove_punctuation`, `lowercase`.
A typing failure suspends injection until listening restarts.

---

## 8. Config

`~/.config/ears/config.toml` or `config.<profile>.toml`. Profile precedence:
`-p` > `EARS_PROFILE` > `~/.config/ears/profile` > none.

| Key | Default | Env override |
|---|---|---|
| `server` | `http://127.0.0.1:8178` | `EARS_SERVER` |
| `device` | `default` | `EARS_DEVICE` |
| `language` | auto | `EARS_LANGUAGE` |
| `api_key`, `model`, `prompt` | none | `EARS_API_KEY`, `EARS_MODEL`, `EARS_PROMPT` |
| `text_filters.{lowercase,remove_punctuation,strict_alphabet,alphabet_threshold}` | false, false, true, 0.5 | — |
| `typing_mode` | `auto` | — |
| `auto_enter` | true | — |
| `progressive_typing` | false (TUI only; `ears vad` forces true) | — |
| `save_to_clipboard` | false | — |
| `bash_mode`, `guided_grammar` | false, built-in | — |
| `cue_volume` | 100 | — |
| `language_servers.<lang>.{server,model}` | empty | — |
| `vad.{speech_threshold,min_speech_duration_ms,max_silence_duration_ms,pre_speech_buffer_ms}` | 0.5, 300, 1200, 500 | — |
| `vad.{duck_enabled,duck_percent}` | false, 50 | — |

Runtime typing switch state: `$XDG_STATE_HOME/ears/typing` (`EARS_TYPING_STATE`).

---

## 9. CLI subcommands

| Command | Detects |
|---|---|
| *(none)* TUI | config, external VAD, env overrides, profiles, model, keyboard layout, health, `pw-cli` |
| `toggle` | locks, state, recorder PID, VAD guard, layout, health, WAV validity, hook |
| `vad` | `vad.pid` (stop if alive), state, layout, health, health lock, capture liveness, sockets |
| `ghost` | same as `vad`, plus the earsghost addon (probed at start) |
| `ws-listen` | layout, health, WebSocket protocol |
| `device`, `profile`, `server`, `test`, `auto-enter`, `typing` | device list, profile files, URL validity, health + transcription, command socket |

---

## 10. Detection timing matrix

| Cadence | Detections |
|---|---|
| Process start | profile, config, env, state dir |
| Per toggle press | locks, state, PID, layout, health, WAV validity |
| VAD/ghost start | layout (fixed for session), health, health lock, `/proc` identity, pw-record, ghost addon probe |
| Per 100 ms chunk | RMS/peak, queue length, VAD edge events, ghost partial poll/start |
| Per 32 ms frame | Silero probability and state machine |
| Every 600 ms while speaking (ghost) | partial transcription of the utterance so far |
| Per segment end | WAV, transcription, filters, typing or ghost commit, clipboard |
| Every 1 s | health `problem()` and JSON publish |

---

## 11. Behaviour the docs don't state

1. `ears vad` always types (forces `progressive_typing`), unlike the TUI.
2. The TUI pipeline uses the config snapshot from TUI launch for device,
   server, profile and language.
3. ASR language and server are fixed at pipeline start.
4. `hyprctl devices`, `hyprctl getoption`, `dconf` and `pw-cli` probes are
   unbounded.
5. `vad.lock` enforces nothing; `vad-health.lock` and `vad.pid` do.
6. A second instance takes over `ears.sock` and `ears-cmd.sock`.
7. External-VAD detection trusts a bare PID.
8. The in-process 120 s recording timeout never fires across toggles; the
   `timeout 120` wrapper is the real cap.
9. Silence artifacts surface as `Error` events.
10. `segment_<n>.wav` is left behind on transcription error; `hook-*.wav`
    is never removed.
11. `AudioBuffer` is written but never read in production.
12. The hyprctl parser takes the first keyboard, not the `main` one.

---

## 12. Ghost completion hooks

- Partial audio: `VadSegmentDetector::current_segment_samples()` exposes the
  in-progress segment (replay buffer plus confirmed speech).
- Partials: `StreamingEngine::ghost_maybe_start_partial` snapshots it every
  600 ms while speaking and spawns `transcribe_preview`; results are tagged
  with an utterance id so stale ones are dropped (`ghost_poll_partials`).
- Final: `process_segment` commits through `ghost_commit`, falling back to
  ordinary typing if the addon cannot deliver. Respects the runtime typing
  switch and typing suspension.
- Output: `src/ghost.rs` speaks a line protocol to the `earsghost` fcitx5
  addon (`fcitx5-addon/`), which shows preedit in the focused input context
  and commits on request. Styling is app-controlled; alacritty needs the
  `colors.preedit` patch for grey, non-underlined ghost text.
