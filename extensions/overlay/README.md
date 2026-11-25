# ears overlay: the light and subtitles

A thin line along the bottom edge of every screen shows what ears is doing,
and moves like liquid with your voice while it listens. Above it, while
`ears subtitles on`, ears' live transcript is drawn: settled words bright, the
ones still forming fainter, a recognised command radiant.

| colour | means |
|---|---|
| dim grey | VAD is off, nothing is recorded |
| cyan | listening: what you say is typed where the cursor is |
| pale blue | listening, typing off, nothing takes the words |
| orange | standby: only the wake phrase is acted on |
| violet | command mode: speech goes to actions, nothing is typed |
| dim violet, pulsing | command mode while its embeddings load: only exact action names fire yet |
| red, steady | `ears decide` has a question open (it floats above the line) |
| pink | vision mode (`ears look`) |
| red, breathing | a fault: the mic delivers nothing, or the ASR server is down |
| normal colour, reddish rim | a warning: the configured mic is missing and ears captures from a fallback one, the ASR is slow or degraded, or the mic is silent |

Yellow (the talking stick), green (an answered agent call) and the
push-to-talk colour come from HAIos tools. Without them those states
never show.

## Requirements

- [Quickshell](https://quickshell.org) (`qs`). On Arch it's `pacman -S quickshell`.
- A Wayland compositor with the layer-shell protocol: Hyprland, sway, niri,
  river, KDE Plasma. GNOME doesn't have it.
- Python 3.9 or newer (stdlib only) and `pw-record` for the level meter.

## Install

```bash
curl -fsSL https://ears.hei.tools/install | sh -s -- --overlay
```

Or by hand from a checkout:

```bash
install -m755 extensions/overlay/ears-overlay ~/.local/bin/
install -Dm644 extensions/overlay/ears-overlay.service ~/.config/systemd/user/ears-overlay.service
systemctl --user daemon-reload && systemctl --user enable --now ears-overlay.service
```

Without systemd: `ears-overlay start` (detached) and `ears-overlay stop`.
`ears-overlay run` (the unit's command) takes over from a detached scene,
so the two never draw two lights.
`ears-overlay check` says whether it would start and why.
`touch ~/.config/ears/no-overlay` opts out.

## How it works

`ears-overlay` writes one Quickshell scene (embedded in the script) to
`~/.cache/ears/overlay/shell.qml` and runs it. The scene starts
`ears-overlay feed`, which emits one JSON line per change. The scene never
reads ears' files itself.

The feed reads only what ears publishes in `$XDG_RUNTIME_DIR/ears/`:

| file | for |
|---|---|
| `vad.pid` | VAD counts as running only while this pid lives |
| `vad.standby`, `vad.command`, `vad.vision`, `vad.chant` | the mode |
| `vad.command.loading` | command mode's embeddings are still loading |
| `vad-health.json` | faults and warnings, used only while it belongs to the live VAD in this boot |
| `subtitle.json`, `subtitles.on` | the subtitle line, and the modes it shows in (`subtitle_modes`) |
| `decision.json`, `decision-last.json` | an open `ears decide` question, and what each answer matched |
| `vision.json`, `chant.json` | vision mode and chant meters |

The level comes from its own `pw-record` on the device ears actually opened
(`device_opened` in the health report), and only while VAD runs, so the
overlay never opens a microphone that ears has closed. Mode changes also go
to `$XDG_RUNTIME_DIR/ears/led.json` for other consumers, such as a
desktop shader that follows the ears colour.

Tests and debugging can redirect inputs with environment variables:
`EARS_OVERLAY_RUN`, `EARS_OVERLAY_PROC`, `EARS_OVERLAY_CONFIG`,
`EARS_OVERLAY_BOOT_ID` and `EARS_OVERLAY_RECORDER` (a stand-in recorder
command).
