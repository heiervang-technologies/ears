# A full setup: Hei's

This is the configuration ears is developed against. It drives a Hyprland
desktop by voice all day: always-on dictation, spoken key commands, a
command mode, steering to other tools, agent hand-offs and the overlay. It
is copied from a live machine with the private parts replaced: microphone
IDs, LAN addresses and home paths (`~/` here was an absolute path).

Treat it as a map, not a drop-in. Several commands call HAIos tools that
aren't part of ears (listed below). Without them those phrases do nothing
useful, and the rest still works.

```
examples/hei/
├── config/
│   ├── ears/                      → ~/.config/ears/
│   │   ├── config.toml            the main config: server, mic, VAD tuning, spoken commands
│   │   ├── actions.toml           command mode: ~350 actions matched by embedding
│   │   ├── config.bash.toml       profile `ears -p bash`: dictate shell commands (grammar-constrained)
│   │   ├── config.stick.toml      profile `ears -p stick`: type nothing, the hook routes the text
│   │   ├── cs.d/markus.toml       own Compensating Scaffolding: "Claire" means "clear"
│   │   └── hooks/post-transcribe  hands a transcript to the agent holding the talking stick
│   └── voice-commands/            → ~/.config/voice-commands/ (plugins for bin/voice-cmd)
│       ├── desktop.toml           "detach", "go to <tmux session>", "focus <window>"
│       ├── controls.toml          "my controls" / "your controls" (agent computer-use handoff)
│       └── centurion.toml         wake the desktop agent's face
└── bin/                           → somewhere in PATH
    ├── voice-cmd                  the steer router: matches an utterance against the plugins
    └── voice-nav                  "go to" / "focus": tmux sessions and Hyprland windows, fuzzy
```

## How the pieces connect

- **Dictation.** `ears vad` runs all day (contrib/systemd/ears-vad.service).
  `server` points at Qwen3-ASR on vLLM with the stream plugin
  (vllm-plugin/), and `live_decoding = "continuous"` streams partials into the
  ghost text. `[text_filters]` lowercases and strips punctuation, which suits
  typing into terminals.
- **Spoken commands.** `[commands]`:
  - `enter`/`over` press Enter, and `sign_off` phrases ("over and out")
    press Enter and then stand by.
  - `wake`/`sleep`/`standby` switch the mic mode. The long lists are what the
    ASR actually hears for "hey clanker".
  - `[commands.keys]` maps phrases to key combinations.
  - `[commands.run.*]` runs programs, for example `close window` runs
    `hyprctl dispatch killactive`.
- **Steering.** `steer = ["voice-cmd", "handle"]`: every utterance is offered
  to `bin/voice-cmd` first. Exit 0 means handled and nothing is typed.
  `voice-cmd` matches whole utterances only, so normal dictation passes
  through. Its plugins live in `config/voice-commands/`.
- **Command mode.** Say "command" and the light turns violet. Each utterance
  is then embedded as raw audio by embeddinggemma2 (`server` in
  actions.toml, a llama-server) and fires the nearest action in
  `actions.toml`. `ears teach NAME` adds your own voice as anchors.
- **Profiles.** `ears -p bash` and `ears -p stick` are the same mic with
  other behaviour (README, "Profiles").
- **Overlay.** extensions/overlay draws the light and the subtitles. The
  subtitles show in the modes listed in `subtitle_modes`.
- **Vision and say echo.** `[vision]` (`ears look`) and `[say_echo]` (don't
  type what the machine's own text-to-speech said) are the newer pieces. See
  their docs/ pages.

## HAIos-only pieces

These commands call tools from HAIos (heiervang-technologies/hai-os), not
from ears:

| tool | used by |
|---|---|
| `haios-voice-route`, `talking-stick`, `director` | "centurion", "cursor mode", "talking stick", the post-transcribe hook |
| `my-controls`, `your-controls` | voice-commands/controls.toml |
| `centurion-eye`, `friend` | waking the desktop agent's face, `friend-*` runs |
| `ears-help`, `ears-commands` | "ears help", "show commands" |
| `hypr-group-smart`, `quickpick`, `mirror`, `monitor-mode`, … | various actions in actions.toml |

## Using it

Start from your own config.toml, then take what you want:

```bash
cp examples/hei/config/ears/cs.d/markus.toml ~/.config/ears/cs.d/
install -m755 examples/hei/bin/voice-cmd examples/hei/bin/voice-nav ~/.local/bin/
mkdir -p ~/.config/voice-commands && cp examples/hei/config/voice-commands/desktop.toml ~/.config/voice-commands/
```

Then add `steer = ["voice-cmd", "handle"]` under `[commands]`. Copy
config.toml whole only after replacing `device` (see `ears list`) and
`server`.
