# RFC 0004: ears-desktop, one installable app for Linux and Windows

Status: draft for review, 2026-11-24. Design only; no code lands with this
RFC. Numbering assumes 0001 (complexity audit, #266) and 0003 (desktop
primitives, #286) land as numbered.

## Summary

Today ears is a Rust daemon plus pieces that each install on their own: the
overlay (`extensions/overlay/`, Quickshell), the fcitx5 ghost addon, the vLLM
plugin, `moonshine-stream`, hand-edited TOML, and Hyprland keybinds. All of it
is Linux-only and most of it is Wayland-only.

**ears-desktop** bundles these into one product that a person installs once,
on Linux or Windows:

1. **ears**, the daemon, unchanged in role, made portable.
2. **The light**, the overlay line and subtitles, drawn by Quickshell on
   Linux and by the desktop app on Windows, from one feed.
3. **The ears desktop app**: a control centre that shows the mode, lets you
   switch it, and lets you see and edit what ears knows: the config, the
   compensating scaffolding (RFC 0002), the command-mode actions and the
   enabled features.
4. **A model manager**: detects the hardware, recommends an ASR, and installs
   it with one click: Qwen3-ASR 1.7B or 0.6B behind our vLLM stream plugin,
   or Moonshine Streaming for machines without a capable GPU.

Four modes are first-class in the app and the light: **idle**, **write**,
**command** and **character**. The first three exist today. Character mode
is new and is specified here.

## Naming

RFC 0003 proposes `ears-desk`, a companion that executes compositor
operations for command mode. That is a different thing from this RFC's app.
To avoid two names one letter apart, this RFC proposes renaming 0003's
companion to **`ears-wm`** (it talks to the window manager) and keeping
**ears-desktop** for the bundle. The overlay keeps its user-facing name,
**the light**; `ears-overlay` stays the binary on Linux.

## Goals and non-goals

Goals:

- One download per platform, one install, one tray icon. A working
  dictation setup on a fresh Windows 11 machine without a terminal.
- The four modes are visible at all times (the light) and switchable by
  voice, hotkey and the app.
- Scaffolding, config and features are visible and editable without opening
  TOML, and every edit is validated and reversible.
- ASR install is one click, local, verified, and removable.

Non-goals:

- macOS. Nothing here should block it, but it is not planned.
- Moving desktop control into ears core. The guardrails of #209 hold: the
  app and `ears-wm` are consumers of ears; removing them changes nothing in
  ears.
- A cloud ASR. Remote servers stay configurable as today, but the installer
  installs local models only.

## The four modes

| mode | today | light | what speech does |
|---|---|---|---|
| idle | standby (`vad.standby`) | orange | only the wake phrase is acted on |
| write | dictation | cyan | typed where the cursor is, ghost text while forming |
| command | command (`vad.command`) | violet | routed to actions, nothing typed |
| character | new | new colour, proposed amber-white | resolved one character or key at a time, typed exactly |

VAD off (dim grey) stays a state, not a mode. Vision, chant and decide stay
sub-states over a mode as today (`src/vad_mode.rs`), and the app shows them
but does not list them as modes.

The app labels modes with these four words. Internally `Mode` keeps its
names; the app maps them. Renaming `Dictation` to `Write` in code is not
worth the churn.

## Character mode

### Purpose

Input where every character must be right: passwords, email addresses,
identifiers, URLs, shell flags, code. You say "at sign" and get `@`; "capital
k" and get `K`; "dash dash help" and get `--help`. Dictation is the wrong tool
here: the ASR normalizes, punctuates and guesses words.

**Correctness is the requirement. Smoothness is the optimization target.**
A character mode that is fast and sometimes wrong is worse than none.

### Entering and leaving

- Voice: "character mode" / "spell" from write or command mode; "done" or
  "write mode" to leave. Like command mode, entering is explicit so normal
  speech never lands here.
- Hotkey and app button, like the other modes.
- Leaving restores the previous mode.

### Vocabulary

A closed, versioned table of spoken names to output, shipped as data
(TOML, like scaffolding) and extendable by the user:

- Letters: "a" … "z", plus the NATO alphabet ("alpha" … "zulu"), which is far
  more robust for the ASR than bare letters. "capital"/"upper" prefix for
  uppercase; "caps on"/"caps off" for runs.
- Digits: "zero" … "nine"; "number" prefix optional.
- Symbols: "at sign" `@`, "dot" `.`, "dash"/"minus" `-`, "underscore" `_`,
  "slash" `/`, "backslash", "colon", "semicolon", "open/close paren",
  "bracket", "brace", "quote", "single quote", "backtick", "tilde", "pipe",
  "ampersand", "star", "hash", "percent", "caret", "dollar", "equals", "plus",
  "less than", "greater than", "question mark", "exclamation mark", "comma",
  "space".
- Keys: "enter", "tab", "backspace", "delete", arrows, "home", "end",
  "escape".
- Edits, handled by the mode, not typed: "scratch" (remove the last
  character), "scratch three", "clear" (remove everything typed in this
  character session).
- Repetition: "three dashes", "dash times three".

Each entry has one canonical spoken form and optional variants. Mishearings
of these names go into the scaffolding (`domain = "characters"`, new target
`char:<name>`), so the existing `ears cs build` probing and `ears cs mine`
mining work unchanged in shape.

### Decoding

Character mode runs on a vocabulary of a few hundred phrases, so it can use
methods that are wrong for open dictation. In order of preference:

1. **Constrained decoding on Qwen3-ASR.** The vLLM plugin already takes a
   GBNF grammar per utterance (`vllm-plugin/ears_vllm_stream/grammar.py`,
   used for the bash grammar). A `characters.gbnf` generated from the
   vocabulary table makes the model only able to emit valid sequences of
   names. The cost to watch is grammar compile time, as with bash mode;
   generating one grammar per vocabulary version and warming it at startup
   avoids per-utterance compiles.
2. **Constrained decoding on Moonshine.** `moonshine-stream/engine.py` runs
   the decoder itself, so a token-level mask from a prefix trie of the
   vocabulary is straightforward. This keeps character mode available on
   weak machines.
3. **Audio kNN, as command mode.** embeddinggemma2 audio embeddings with
   per-kind z-scores already route command-mode actions. Per-name voice
   samples, collected during use, can vote alongside the words. This helps
   most with single short names ("b"/"d"/"e") where words alone are weak.
4. **A dedicated phoneme or keyword model** only if 1–3 measure badly. Out of
   scope until the measurement exists.

Words first, as in command mode: when the constrained transcript resolves
to valid names, they fire; the audio vote confirms and teaches.

### Feedback and correction

- Every resolved character shows in the light as a large glyph (`@`, not
  "at sign") before and as it is typed, so a mistake is visible at once.
- Characters are typed as they resolve; "scratch" corrects. No confirmation
  step per character, which would destroy the flow.
- Optional **secure input**: when the focused field is a password field (if
  the platform reports it) or when asked, glyphs show as dots and nothing goes
  to subtitles, the journal or logs.

### Typing

Characters are sent as key events, not as text through the dictation path:
symbols need the right key and modifier for the active layout, and some
fields reject synthetic paste. Linux: wtype with explicit keysyms. Windows:
`SendInput` with `KEYEVENTF_UNICODE` for characters and virtual keys for
named keys.

### Smoothness targets

To be measured on centurion and a weak laptop before the targets are fixed:

- Character error rate on a fixed spoken test set (passwords, emails, URLs),
  target zero on the test set with a clean mic.
- Latency from the end of a name to its glyph: every name is a complete
  phrase, so `exact_silence_ms` (150 ms) endpointing from #284 applies to
  every token.
- Names per second sustained in a run without errors.

## The desktop app

### Technology

**Tauri 2** (Rust core, web UI, WebView2 on Windows, WebKitGTK on Linux).
Reasons: the app can link the ears crate directly for config parsing,
validation and scaffolding logic instead of re-implementing them; it gives
tray icon, global hotkeys, autostart, an installer bundler (MSI/NSIS, deb,
AppImage) and a signed updater on both platforms; and the same window
machinery can draw the light on Windows.

Alternatives considered: egui (no native installer/updater story, weaker
for dense editing views), Qt/QML (would share the Quickshell scene on Linux,
but heavy to ship and license-sensitive on Windows).

### How it talks to ears

- **Reads** state from the ears IPC socket (events: mode, segment, voice
  command, health) and from a new `ears feed` stream (below).
- **Writes** through ears, never by editing files behind its back: `ears
  config set/validate`, `ears cs …`, and mode switches over IPC. Where a CLI
  verb is missing, add the verb to ears, justified on its own, rather than a
  private file format in the app.

### Views

1. **Home**: current mode with the four mode buttons, ASR server and model,
   mic, latency, health faults. Same colours as the light.
2. **Scaffolding**: every `cs` file by model and domain. Per entry: meant,
   heard, target, failure mode, source, count, risk and status. Filters for
   `flagged` entries with approve/reject, which is today's `ears cs check`
   loop with one click per entry. Shows when the active ASR model has no
   scaffolding yet and offers `ears cs build`.
3. **Config**: the TOML as a form generated from the config schema, with the
   raw text one tab away. Every save validates, writes atomically and keeps a
   numbered history in `~/.config/ears/history/` with a diff view and
   restore. This replaces the ~40 hand-made `config.toml.bak-*` files on
   centurion.
4. **Commands and actions**: command-mode actions (`actions.toml`) grouped by
   family, with phrasings, effect, and the voice samples that taught them;
   test a phrase by typing or speaking it and see what would fire.
5. **Characters**: the character vocabulary, user additions, and a practice
   pane that measures your character error rate.
6. **Features**: switches for subtitles, ghost text, AEC, chant, decide,
   vision, journal (`ears cs mine` input), with what each needs on this
   platform and why it is unavailable if it is.
7. **Models**: the model manager below.
8. **Mic check**: `ears mic-check` with its report rendered.

## The light on both platforms

The overlay today is a Python script that writes a Quickshell scene and a
feed process that reads ears' runtime files and runs its own `pw-record` for
the level.

Proposed:

- **Move the feed into ears** as `ears feed`: one JSON line per change,
  same content as `ears-overlay feed` now, plus the input level computed by
  ears from the audio it already captures. Then no consumer opens the mic,
  and Windows needs no Python.
- **Linux**: Quickshell scene unchanged, reading `ears feed`.
- **Windows**: the app opens one transparent, click-through, topmost,
  non-activating window per monitor along the bottom edge (layered window
  with `WS_EX_TRANSPARENT | WS_EX_NOACTIVATE | WS_EX_TOOLWINDOW`) and draws
  the line and subtitles from the same feed. The QML scene's look is the
  spec; the web version reimplements it.
- GNOME on Linux has no layer-shell; the app can offer the same window-based
  light there as a fallback.

## Porting ears to Windows

The daemon is written against Linux. Each row is work; the "seam" column is
the trait or module boundary that makes both platforms possible.

| area | Linux today | Windows | seam |
|---|---|---|---|
| audio capture | `pw-record` child process (`src/pcm.rs`, `src/recording.rs`) | WASAPI in-process via `cpal` | `AudioSource` |
| device list | `pw-cli` (`src/audio.rs`) | `cpal` / MMDevice | `AudioSource::devices` |
| echo cancellation | PipeWire `ears_aec_mic` | WASAPI communications stream AEC, or off | per-platform feature flag |
| typing | wtype, ydotool (`src/desktop.rs`) | `SendInput` (e.g. `enigo`) | `TextSink` |
| ghost text | fcitx5 addon (`fcitx5-addon/`, `src/ghost.rs`) | TSF text input processor (phase 5); until then subtitles only | `GhostSink` |
| runtime files | `$XDG_RUNTIME_DIR/ears/` | `%LOCALAPPDATA%\ears\run\` | `paths::runtime_dir` |
| single instance | `flock` (`src/lock.rs`) | `LockFileEx` (e.g. `fs4`) or a named mutex | `lock` |
| IPC | Unix socket (`src/ipc/socket.rs`) | named pipe (e.g. `interprocess`) | `ipc::Listener` |
| process control, signals | `nix`, `libc`, SIGTERM, `timeout` | Job objects, `TerminateProcess`, in-process timers | `process` |
| waybar refresh | `pkill -SIGRTMIN+9` | none; the app and feed replace it | Linux-only |
| notifications | `notify-send` | toast via `notify-rust` | `Notifier` |
| sounds | `paplay` | `rodio` on both | `Sounds` |
| keybinds | Hyprland binds calling `ears toggle` | global hotkeys registered by the app | app |
| keyboard layout | `hyprctl`, `dconf` | `GetKeyboardLayout` | `TextSink` |
| command-mode actions | `keys` and `run` (often `hyprctl dispatch`) | `keys` and `run` work; ship a Windows default action pack | data |
| autostart | systemd user service | app autostart (Run key or Task Scheduler) | app |
| TUI | ratatui/crossterm | works in Windows Terminal; low priority | none |

Order of the port: introduce each seam on Linux first with the current
behaviour behind it (no user-visible change, each a small PR), then add the
Windows implementation behind `cfg(windows)`. CI gains a `windows-latest`
build and test job from the first seam PR so nothing regresses quietly.

The Silero VAD (`voice_activity_detector`, ONNX Runtime) and the HTTP and
WebSocket clients (`reqwest`, `tokio-tungstenite` with rustls) are already
portable.

## Model manager and one-click ASR

### Recommendation

The app inspects GPU vendor and VRAM (NVML on NVIDIA, DXGI on Windows,
`/sys` and `nvidia-smi` on Linux), system RAM, CPU and free disk, and
recommends one of:

| hardware | recommended | notes |
|---|---|---|
| NVIDIA GPU, enough VRAM for 1.7B with headroom | Qwen3-ASR-1.7B, vLLM + `ears_stream` plugin | today's centurion and rogue setup |
| NVIDIA GPU, less VRAM | Qwen3-ASR-0.6B, same stack | |
| no NVIDIA GPU, or a laptop on battery | Moonshine Streaming, `moonshine-stream` on CPU | today's hackbook setup |

VRAM thresholds are deliberately not written here: they must be measured
with the pinned vLLM version, our `gpu_memory_utilization`, and the plugin's
caches, on at least one 8 GB and one 12 GB card. Character mode's grammar
must also be measured on each.

The user can override the recommendation, keep a remote server instead, or
install more than one model and switch.

### What one click does

1. Check free disk against the download plus install size, and show both.
2. Download a pinned runtime and the model weights with a progress bar (bytes,
   resumable), from a **signed manifest** at `ears.hei.tools` that lists
   exact versions, URLs and SHA-256 of every artefact. Nothing is resolved
   from an index at install time.
3. Verify every hash. On mismatch, delete and stop.
4. Show the model licence (Qwen3-ASR and Moonshine Streaming licence terms
   are to be checked and recorded in the manifest before shipping) and record
   acceptance.
5. Install as a user service bound to `127.0.0.1` on a free port.
6. Start it, wait for health, run a short known clip through it, and only
   then write the ears profile and switch to it.
7. Offer `ears cs build` for the new model, since scaffolding is per model.

Uninstall removes the runtime, the weights and the service, and switches
ears back to the previous profile.

### Runtimes per platform

**Linux, Qwen3-ASR**: a `uv`-managed Python with the pinned vLLM (0.28.0
today) and `ears-vllm-stream`, or the pinned `vllm/vllm-openai` image under
podman when present. Weights go to the user's Hugging Face cache, or to a
user-chosen data disk.

**Linux, Moonshine**: `uv`-managed Python with `moonshine-stream` and ONNX
Runtime, CPU only.

**Windows, Moonshine**: easiest. ONNX Runtime has native Windows builds; ship
`moonshine-stream` with an embedded Python, or (better, later) port its
engine to Rust on `ort` so it runs inside a single signed binary.

**Windows, Qwen3-ASR**: the hard part. vLLM does not run on native Windows.
Options:

| option | one click? | cost |
|---|---|---|
| WSL2 + CUDA + vLLM inside a managed distro | not on a machine without WSL: enabling it needs admin and usually a reboot | best fidelity, same plugin |
| Docker Desktop with the vLLM image | no: Docker Desktop is a separate install with its own licence terms | heavy |
| a native Windows server implementing the ears stream protocol on PyTorch/transformers | yes | new server; no vLLM prefix caching; slower, must be measured |

Recommendation: the ears stream protocol (`docs/STREAM_PROTOCOL.md`) is the
contract, not vLLM. Ship **WSL2-backed vLLM as the Windows Qwen path**, with
the installer doing as much as it can (detect WSL, run `wsl --install` with an
elevation prompt, resume after reboot, import a pinned distro image) and
calling the reboot step what it is instead of claiming one click. Moonshine
stays the true one-click default on Windows until a native Qwen server is
measured good enough.

## Packaging, signing, updates

- Linux: AppImage and a deb, plus the existing `curl | sh` installer gaining
  `--desktop`. An AUR package only if published from our own account.
- Windows: MSI via the Tauri bundler, Authenticode-signed so SmartScreen
  does not block it. Needs an EV or OV code-signing certificate: a
  procurement step with a lead time.
- Updates: the Tauri updater with signed release manifests covers the app
  and the bundled ears binary. This closes #84 for desktop installs. Model
  runtimes update through the model manager's manifest, separately and only
  on request.
- One ears binary per install. Centurion has three installed copies, and
  an old copy silently drops newer config (ghost, commands). The app checks `ears --version` against the
  version it bundles and warns on any other `ears` on `PATH`.

## Phases

Each phase ends with something usable.

0. **Decisions** (this RFC): names, Tauri, `ears feed`, the Windows Qwen path,
   character mode's colour and entry phrases.
1. **`ears feed` and the app on Linux**: home, modes, config with history,
   scaffolding view. The light keeps Quickshell. No Windows yet.
2. **Character mode on Linux**: vocabulary table, `characters.gbnf`,
   Moonshine trie mask, glyph feedback, the practice pane and its
   measurements.
3. **Seams in ears core**: `AudioSource`, `TextSink`, `GhostSink`, runtime
   paths, lock, IPC, process, notifier, sounds. Each a no-change PR on Linux;
   `windows-latest` CI from the first one.
4. **Windows build**: Windows implementations, the window-based light, global
   hotkeys, tray, MSI, signing. Write, command and character mode working with
   a remote ASR.
5. **Model manager**: manifest, downloads, Moonshine one-click on both,
   Qwen on Linux, then WSL2 Qwen on Windows.
6. **Windows ghost text**: TSF text input processor. Large and separately
   reviewed; dictation works without it.

## Debt guardrails

- ears core gains only generic capabilities: `ears feed`, CLI verbs for
  config and scaffolding, platform seams. No UI, installer or download code
  in the ears crate.
- The app is optional on Linux: ears, the light and the CLI keep working
  without it.
- Every platform-specific piece sits behind a seam with a Linux
  implementation that is today's behaviour, so the port cannot change Linux
  behaviour by accident.
- No package is installed from an unpinned source. The manifest is the only
  source of artefacts.

## Open questions

1. Character mode's colour and its entry and exit phrases; whether "spell"
   collides with anything in existing scaffolding.
2. Whether character mode should also accept bare letters ("b", "d", "e")
   or require NATO for the confusable set.
3. Windows Qwen: is WSL2 acceptable as the GPU path, or is a native server
   worth building first?
4. Where weights live by default on machines with a data disk (on centurion,
   KC3; elsewhere, discover).
5. Whether the app ships the TUI too, or retires it for desktop installs.
6. Code-signing certificate: who procures it, and under which entity.
7. Rename of RFC 0003's `ears-desk` to `ears-wm`: agree before 0003 merges.

## Task list

The complete list of work, for issues once the RFC is accepted.

**Core (ears crate)**
- [ ] `ears feed` (JSON lines: mode, sub-states, health, subtitle, decision, level)
- [ ] Input level published from the capture path
- [ ] `ears config get/set/validate` and config history with restore
- [ ] Config schema export for the app's form
- [ ] Character mode: `Mode::Character`, entry/exit, edits, repetition
- [ ] Character vocabulary format, default table, user overrides
- [ ] Scaffolding domain `characters` and target `char:<name>`; probe phrases for `ears cs build`
- [ ] Key-event typing path for characters (layout-aware)
- [ ] Secure input: no subtitles, journal or logs while active
- [ ] Seams: `AudioSource`, `TextSink`, `GhostSink`, runtime paths, lock, IPC listener, process control, notifier, sounds
- [ ] Windows implementations of every seam
- [ ] `windows-latest` CI build and tests

**ASR servers**
- [ ] `characters.gbnf` generation from the vocabulary and warm-up in the vLLM plugin
- [ ] Token-trie mask for character mode in `moonshine-stream`
- [ ] VRAM measurements for 0.6B and 1.7B (8 GB and 12 GB cards) with the pinned stack
- [ ] Moonshine packaging for Windows (embedded Python first, Rust `ort` port later)
- [ ] WSL2 distro image with pinned vLLM and plugin
- [ ] Health and known-clip self-test endpoint use for install verification

**Desktop app**
- [ ] Tauri skeleton: tray, autostart, single instance, global hotkeys
- [ ] Home view and mode switching
- [ ] Scaffolding view with approve/reject
- [ ] Config form, raw editor, history and diff
- [ ] Commands/actions view with phrase tester
- [ ] Characters view and practice pane
- [ ] Features view with per-platform availability
- [ ] Mic check view
- [ ] Window-based light for Windows and GNOME
- [ ] Stray-binary check (`ears` on `PATH` vs bundled)

**Model manager**
- [ ] Hardware detection (GPU vendor, VRAM, RAM, disk)
- [ ] Signed manifest format and hosting at `ears.hei.tools`
- [ ] Resumable, hash-verified downloads with progress
- [ ] Licence display and acceptance record
- [ ] Service install per platform (systemd user unit; Windows user task)
- [ ] Post-install self-test, profile write, switch
- [ ] Uninstall and switch back

**Release**
- [ ] AppImage, deb, `install --desktop`
- [ ] MSI, Authenticode signing, certificate
- [ ] Updater and signed release manifests (closes #84 for desktop)
- [ ] Docs: install, modes, character mode, model manager, Windows notes

**Character mode measurement**
- [ ] Spoken test set (passwords, emails, URLs, flags), recorded on two mics
- [ ] Character error rate and latency per decoder (Qwen grammar, Moonshine mask, kNN vote)
- [ ] Targets fixed from the measurements
