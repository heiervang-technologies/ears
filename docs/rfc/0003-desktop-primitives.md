# RFC 0003: Desktop primitives for command mode

Status: draft for review, 2026-11-13. Design only; no code lands with this
RFC. Scope: command mode (violet). Dictation, standby and vision mode keep
their behaviour except where a section says otherwise.

## Summary

Command mode today picks one of a flat list of actions. Nothing in an
utterance can say *which* window, workspace or monitor, or *how many*. This
RFC adds, in order of how much each one buys:

1. **Typed slots in action phrasings** (`workspace {n}`, `focus {dir}`,
   `close {window}`), parsed from the words only. The embedding keeps
   choosing the *family* when the words are unclear; it never picks a slot
   value. Closed slots (numbers, directions, monitors) work with `run` and
   argument substitution, with no new process: 124 of the 348 actions on
   centurion become about 20 templates.
2. **`again`** ("again", "again three times") and counts ("scroll down
   five"), in core.
3. **`ears-desk`**, an optional companion that publishes the live desktop
   (windows, workspaces, monitors, their spoken names and positions) and
   executes resolved operations over the compositor socket. ears keeps the
   language (hearing, parsing, resolving a spoken name against a list,
   asking back); the companion keeps the compositor and the side effects.
   Hyprland first, niri and sway behind the same interface.
4. **Ambiguity and confirmation**: reversible verbs take the most recently
   used match; destructive verbs on a named or ambiguous target ask, with
   numbered labels on screen and the red decide mode for yes/no.
5. **`undo`** for operations the companion can invert.
6. **Hints**: window hints from compositor geometry first; in-window hints
   later, from the browser's remote protocol or SAM 3 (vision mode).
   AT-SPI is weak on this desktop (measured below) and comes last.

## What exists today

Every claim here is about `origin/main` at `ea3d492`.

**Routing.** An action is a name, phrasings (`text`), and one effect, `keys`
or `run` (`src/command_mode.rs:113-133`). `parse_actions` adds built-ins
(`leave`, colours, `none`, `wrong`, `chant`, `agent`;
`src/command_mode.rs:166-239`) and rejects anything that is not exactly one
of `keys`/`run` (`:258-272`). Routing is words first: an utterance that is
an action's name or phrasing after `plain()` fires it (`:1040-1067`);
otherwise the audio and text embeddings vote and fire only when they agree
or one fires alone (`combine`, `:1099-1114`).

**The only parameter today is `rest`** (#251): an action whose phrasing
starts the utterance gets everything after it as one more `run` argument,
longest phrasing first (`by_rest`, `:1071-1093`; argument appended in
`effect`, `:1396-1410`). It is untyped, words-only (`exact` is forced on,
`:277`), and must be last. Of 348 actions on centurion one uses it
(`call-agent`).

**Execution.** `fire_effect` (`src/streaming_engine.rs:2631`) presses keys or
starts the program. `run_program` first holds the ghost in the accept
colour for `accept_ms` (200 ms on centurion) when a ghost is showing, then
spawns (`src/streaming_engine.rs:1204-1209`; hold at
`src/streaming_engine/ghost.rs:918-931`). Every `hyprctl dispatch` is a
process.

**Mode and sub-states.** `Mode` holds exactly one of dictation, standby,
command, vision (`src/vad_mode.rs:64`). Command mode's state is
`CommandState { mode, chant }`; sub-states of command mode go there so they
end with it (`src/vad_mode.rs:47-60`). An open decision (red, `ears decide`)
lies *over* the mode and takes every utterance until it closes
(`src/vad_mode.rs:13-16`).

**Words-first firing and early end (#284).** When the words alone fire an
action, the engine does not wait for the audio embedding
(`src/streaming_engine.rs:2452-2457`); the embedding arrives later for the
record and to teach the voice (`:2539-2556`). Separately, the live
transcript can end the utterance early: a phrase that is complete
(`Ending::Exact`) and not the start of a longer one ends after
`exact_silence_ms` (default 150) instead of `max_silence_duration_ms`
(1500 on centurion) (`src/endpoint.rs:101-152`, `src/config.rs:181-195`,
`src/streaming_engine.rs:480-495`). An *open* phrasing (a `rest` action's)
never counts as complete (`src/command_mode.rs:1269-1285`).

**Stage timing.** `Timing` records silence, asr, embedding, decision and
action in ms after the utterance ended (`src/command_mode.rs:937-958`),
written into `command-last.json` (`:41`). The action stage includes the
accept hold (`:954-955`).

**Compensating scaffolding (RFC 0002).** cs entries add mishearings of
existing phrases; for command mode the target is `action:<name>`
(`src/cs/mod.rs:476-486`, applied when actions load,
`src/command_mode.rs:1203`). The ASR context in command mode is every
action's first phrasing, then every second, and so on
(`src/cs/context.rs:93-104`), cut at 512 characters
(`src/cs/context.rs:33`, `build` at `:39`).

**Chant.** Chanting folds repeats into units (`src/chant.rs:57`) and asks
the same `Router::by_rest`/`by_words` (`src/chant.rs:94-105`). Whatever
parses a command must live in the `Router` so chant gets it for free.

**Decide.** A decision has options, an asker pid (it lapses when the asker
exits), an optional QTE timeout, `other`, and `exact`: "only an option said
exactly ... For options that act (a click), where a near miss must not
count" (`src/decide.rs:35-60`). Ordinals are understood up to six
(`src/decide.rs:107-156`). cs variants apply as `decide:<option>`
(`src/cs/mod.rs:509`).

**Numbers are parsed three times.** `vision::number` (0 to 20, 30, 40, 50,
"to"/"for", `src/vision.rs:137-172`), `commands::count` (1 to 10,
"won"/"to"/"for", `src/commands.rs:791-806`) and decide's `ORDINALS`
(1 to 6, `src/decide.rs:107-156`).

**Compositor code is already in core, three ways.** `pointer.rs` does
Hyprland and sway for vision (outputs, screenshot, cursor, click;
`src/pointer.rs:1-35`); `ghost.rs` reads the focused window from Hyprland
or niri (`src/ghost.rs:122-170`); `desktop.rs` asks Hyprland for the
keyboard layout. Each spawns `hyprctl` per question.

**IPC.** Two sockets. `ears.sock` broadcasts `StreamingEvent`s as JSON
lines and never reads (`src/ipc.rs:81-112`); it already carries
`VoiceCommand { name, text }` (`src/streaming_engine.rs:100`).
`ears-cmd.sock` takes line verbs (`claim`, `typing-*`, `standby-*`,
`command-*`, `colour-*`) and answers each (`src/ipc.rs:114-200`).

**Side effects already live outside ears.** On centurion 26 actions call
`ears-motion` (scroll, autoscroll, pan, zoom, focus by position, monitor;
`~/dotfiles/private/bin/ears-motion`) and 88 call `ears-tmux`. RFC #209
asked that desktop control not grow inside ears; these scripts are the
companion pattern in practice, one process per action.

## What this machine shows

Read-only checks on centurion, 2026-11-13. Nothing was dispatched, no
desktop state was changed, `ears vad` was not run.

**actions.toml** (`~/.config/ears/actions.toml`): 348 actions, 923
phrasings, 339 `run`, 9 `keys`, 44 `exact`, 1 `rest`.

| kind | actions | examples |
|---|---|---|
| a number in the name | 45 | `switch-to-workspace-1..10`, `move-window-to-workspace-N`, `move-window-silently-to-workspace-N`, `tmux-window-0..9`, `switch-to-group-window-1..5` |
| a direction or monitor | 63 | `movewindow l/r/u/d/mon:*`, `swapwindow`, `resizeactive`, `moveintogroup`, `movecurrentworkspacetomonitor`, `ears-motion focus/pan/scroll/monitor/zoom`, `hypr-group-smart` |
| a verb on the focused window | 12 | `close`, `fullscreen` (3 variants), `togglefloating`, `pseudo`, `bringactivetotop`, `togglegroup`, `pop-window-out` |
| other workspace moves | 4 | `next-workspace`, `former-workspace`, scratchpad |
| launch an app | 39 | `terminal`, `browser`, `file-manager`, `omarchy-launch-*`, `gtk-launch ...` |
| tmux keys | 78 | `ears-tmux c`, `ears-tmux %`, ... |
| keys | 8 | enter, escape, arrows |
| everything else | 99 | media, brightness, toggles, menus, notifications |

The first three rows plus the fourth, 124 actions, are instances of about
20 primitives. 200 of the `run` actions are `hyprctl` and match the 25
dispatchers bound in `hyprctl binds` (270 binds): the file was generated
from the keybindings, so its names follow the keys, not the speech.

**ASR context coverage.** Round-robin first phrasings of all 355 actions
(348 plus built-ins) total 7315 characters; 50 of them fit in the 512
character cap. Today about one action in seven gets context biasing.

**Windows.** Hyprland 0.56.2 (it ships a Lua `eval` and `--batch`). 10
clients over workspaces 2 to 5 on three monitors (HDMI-A-1 3840x2160 at
1.6, DP-1 1920x1200, DP-2 2560x1440 at 1.25). 9 of the 10 are `Alacritty`.
Titles do not tell them apart either: four are `me@centurion:~`, three are
`me@centurion:~/Pictures/ht/mei`. The tenth is `hover`, a browser built
from a Mozilla-style tree and started with `--remote-debugging-port 9333`.
So "focus the terminal" is ambiguous nine ways and titles cannot resolve
it; position and recency can (`focusHistoryID`, `workspace`, `monitor`,
`at`/`size` are all in `clients`).

**Dispatchers.** `focuswindow`, `closewindow`, `movetoworkspacesilent`,
`focuscurrentorlast`, `focusurgentorlast`, `swapwindow`,
`resizewindowpixel`, `movewindowpixel`, `settiled`, `setfloating`,
`alterzorder`, `centerwindow`, `tagwindow` and `movecursor` are present as
strings in `/usr/bin/Hyprland` 0.56.2; the 25 bound ones are in use. The
`address:` window selector for the `*pixel` and `focuswindow` dispatchers
is to be confirmed against the 0.56 docs in phase D1, not assumed.

**Latency of asking Hyprland.** `j/clients` on
`$XDG_RUNTIME_DIR/hypr/<sig>/.socket.sock`: median 0.06 ms (30 runs,
6.2 kB answer). `hyprctl -j clients` as a process: median 3.9 ms, p90
5.2 ms (15 runs). Not large either way; a process per action is not the
latency problem. Live state is the reason for a resident companion: the
event socket (`.socket2.sock`) lets it keep the window list current
without polling, so resolving a name costs no query at all.

**Command-mode decisions in the running daemon** (`debug.log` since
2026-11-11, 20 utterances, the "in X" of `Command mode: ... by ... in`,
measured from the start of `command_segment`, after the silence): median
109 ms by words, 131 ms by both votes, 103 ms by meaning (p90 355 ms).
The daemon predates #284, so `command-last.json` has no `timing` yet; the
phase measurements below use it once the daemon is restarted.

**AT-SPI.** `org.a11y.Bus` is on the session bus, `at-spi2-registryd`
runs, the `Atspi-2.0` typelib is installed. `org.a11y.Status.IsEnabled` is
`false`. The registry lists 7 applications: polkit agent, two portals,
walker, two heierchat (WebKit) with one node each, and waybar with 345
nodes, 22 of them showing and actionable (walk: 61 ms). Alacritty has no
accessibility support, and the browser does not register while
`IsEnabled` is false. I did not switch it on: that changes desktop state
for every toolkit app. So AT-SPI hints today cover the top bar and nothing
the user looks at most. AT-SPI on Wayland also reports extents relative to
the window, so a hint needs the window's position from the compositor
anyway (to verify in phase H2).

## Proposal

### 1. Slots and templates (core, words only)

A phrasing may contain typed slots. The `rest` flag becomes the special
case "one untyped slot at the end".

```toml
[[action]]
name = "workspace"
text = ["workspace {n}", "go to workspace {n}", "switch to workspace {n}", "super {n}"]
run = ["hyprctl", "dispatch", "workspace", "{n}"]

[[action]]
name = "move-to-workspace"
text = ["move {window} to workspace {n}", "move to workspace {n}", "super shift {n}"]
desk = "move-to-workspace"        # executed by ears-desk (section 4)

[[action]]
name = "scroll"
text = ["scroll {dir}", "scroll {dir} {n}"]
run = ["ears-motion", "scroll", "{dir}", "{n?5}"]   # default when not said
```

Slot kinds:

| kind | values | parsed by | source |
|---|---|---|---|
| `n` | 0 to 20, 30, 40, 50, digits; "to"/"for" via cs | `spoken::number` | closed |
| `ord` | first .. tenth, "last", "next", "previous", "other" | `spoken::ordinal` | closed |
| `dir` | left, right, up, down (aliases above/below/top/bottom) | core table | closed |
| `monitor` | left/right/top/main/next/previous, or a spoken name | core table, names from `desk.json` | closed + live |
| `workspace` | a number, "previous", "next", "empty", named/special ones | core + `desk.json` | closed + live |
| `window` | "this"/"it"/"that" (focused), "last"/"other" (MRU 1), an app's spoken name, a hint label; optional qualifiers "left", "on the big screen", "second" | core matcher over `desk.json` | live |
| `count` | "twice", "three times", "N more times" | `spoken` | closed |
| `rest` | the remaining words, untyped, last | as today | open |

Rules:

- **Words decide values; the embedding decides families.** Text anchors
  for a template are its phrasings rendered with two sample values
  ("workspace three", "workspace seven"), so the kNN can still pick
  `workspace` from a fuzzy utterance. If the words do not fill the
  template's required slots, nothing fires: the subtitle shows the family
  with a gap ("workspace ?") as `nearest` does today
  (`src/command_mode.rs:1116-1123`). An embedding never invents a number.
  Today 10 sibling actions (`switch-to-workspace-1..10`) compete with each other
  in the kNN; with one template they cannot misroute among themselves.
- **One parser, in `Router`.** `by_words`/`by_rest` become
  `Router::parse(words) -> Option<Frame>` where
  `Frame { action, slots, count }`. `fires_by_words`, chant's `by_words`
  and the early-end phrases all go through it, so words-first firing
  (#284) and chanting work for templates unchanged.
- **Closed slots are expanded for the endpoint.** `Phrases` is a set of
  complete strings (`src/endpoint.rs:107-152`). For closed slots ears
  expands each template into its instances (5 templates x 4 phrasings x 21
  numbers is about 400 strings) and passes them as complete phrases, so
  "workspace three" ends after 150 ms instead of 700 to 1500. The existing
  prefix rule keeps "scroll down" from ending early while "scroll down
  five" is a phrase. Live slots are expanded from the current vocabulary
  the same way and rebuilt when it changes.
- **Teaching.** A firing by words teaches the voice its phrasing today,
  except for `rest` actions (`src/command_mode.rs:1380-1383`). Keep
  teaching for templates whose slots are all closed (the audio of
  "workspace three" is a good anchor for the family); skip it when a live
  slot was filled, as for `rest`.
- **`spoken.rs`** replaces the three number parsers (`vision.rs:137`,
  `commands.rs:791`, decide `ORDINALS`) with one module and one set of
  tests; the homophones hard-coded there ("to", "for", "won") become
  `slot:number` cs entries (section "cs").

### 2. Verbs and targets

The primitive set, all targets defaulting to `this` (the focused window, the
current workspace or monitor):

| verb | targets | Hyprland | niri | inverse (undo) |
|---|---|---|---|---|
| focus | window, dir, monitor, workspace | `focuswindow`, `movefocus`, `focusmonitor`, `workspace` | `focus-window --id`, `focus-column-*`, `focus-monitor-*`, `focus-workspace` | focus the previous ref |
| move | window to workspace / monitor / dir | `movetoworkspace[silent]`, `movewindow` | `move-window-to-workspace`, `move-column-*` | move back to the recorded place |
| swap | window with dir / window | `swapwindow` | `swap-window-*` (to verify) | swap back |
| close | window | `closewindow`, `killactive` | `close-window --id` | none: confirm instead |
| fullscreen | window, mode (full, width, tiled) | `fullscreen`, `fullscreenstate` | `fullscreen-window`, `maximize-column` | same toggle |
| float | window (toggle, on, off) | `togglefloating`, `setfloating`, `settiled` | `toggle-window-floating` | same toggle |
| resize | window, dir, n steps | `resizeactive`, `resizewindowpixel` | `set-column-width` | negative step |
| group | window, dir | `togglegroup`, `moveintogroup`, `changegroupactive` | n/a (columns) | toggle / move out |
| workspace to monitor | workspace, monitor/dir | `movecurrentworkspacetomonitor` | `move-workspace-to-monitor-*` | move back |
| open | app (from desktop entries and aliases) | `exec` | `spawn` | none |
| scroll / pan / zoom | dir, n | `ears-motion` (unchanged) | same | none |

"Move this to workspace two" is `move {window} to workspace {n}` with
`window = this`. Unsupported pairs answer "not on this compositor" in the
subtitle with the error beep, never a silent no-op.

### 3. `again`, counts, `undo`

- **`again`** is a built-in, exact (like `chant`): "again", "do it again",
  "repeat", "again {count}", "{count} more times". It re-fires the last
  firing's frame. Relative targets are resolved again at that moment, so
  "focus left", "again" walks one further. Not repeatable: `leave`,
  colours, `chant`, `agent`, `wrong`, `none`, and `taught`. A destructive
  frame repeated goes through confirmation as if said fresh. Counts are
  capped at 10, as `back N` is (`BACK_MAX_WORDS`, `src/commands.rs:400`).
- **Counts** in a frame: a template that takes `{n}` gets it as an argument
  ("scroll down five" -> `ears-motion scroll down 5`, which already takes
  notches); otherwise "twice", "three times" repeat the frame, 30 ms apart.
- **`undo`** asks ears-desk to invert its last operation (table above),
  within 30 s, one step per "undo". Operations without an inverse (close,
  open, anything run by a flat `run`) say so. **`wrong`** keeps its meaning
  (forget the voice anchor the last firing added) and also undoes, when an
  inverse exists: a misroute should leave neither the anchor nor the
  effect. "undo" does not press ctrl+z; "control z" stays a keys action.

### 4. Where it runs: language in ears, effects in ears-desk

RFC #209 said desktop control must not grow inside ears core. Since then
command mode (`src/command_mode.rs`, 1979 lines) and vision mode (which
clicks, `src/vision.rs`, `src/pointer.rs`) were built in core. The line that
still holds, and that this RFC draws:

- **ears core owns language**: hearing, the slot grammar, matching spoken
  names against a list, ranking candidates, asking back (overlay labels,
  decide), confirmation policy, `again`, timing. It already does all of
  these for flat actions.
- **ears-desk owns the compositor**: the live model of windows, workspaces
  and monitors and their spoken names; executing an operation on a
  concrete ref; recording inverses for undo; drawing hint labels (or
  publishing them for the LED scene to draw).
- Without ears-desk, everything up to section 3 works for closed slots via
  `run`; `desk = ...` actions and `{window}` slots do not fire and say
  "ears-desk is not running". Removing the companion leaves today's ears.

Protocol, using what exists:

1. **Vocabulary, desk to ears: a state file.** ears-desk writes
   `$XDG_RUNTIME_DIR/ears/desk.json` with its pid (dead pid = ignored, as
   decide treats a dead asker, `src/decide.rs:39-40`), a generation number,
   and the model:
   ```json
   {"pid": 4242, "compositor": "hyprland", "generation": 812,
    "windows": [{"ref": "0x55ff955d15f0", "names": ["terminal", "alacritty"],
                 "title": "me@centurion:~", "workspace": "5", "monitor": "HDMI-A-1",
                 "rect": [0, 1440, 1200, 675], "mru": 0, "visible": true}],
    "workspaces": [{"id": "5", "names": ["five"]}],
    "monitors": [{"ref": "HDMI-A-1", "names": ["big screen", "samsung"], "rect": [0, 1440, 2400, 1350]}],
    "hints": null, "undo": 3}
   ```
   ears re-reads it when its mtime changes, at most once per utterance.
   Spoken names come from the class, desktop entries (`Name`,
   `StartupWMClass`) and a user alias table (`[desk.aliases]`
   `Alacritty = ["terminal"]`).
2. **Operations, ears to desk: an event on `ears.sock`.** A new
   `StreamingEvent::DeskOp { id, op, args, words }` with resolved refs
   (`{"op": "close", "window": "0x55ff..."}`), next to `VoiceCommand`.
   Other socket consumers (talking-stick, fay) see it too; that is useful,
   not harmful.
3. **Result, desk to ears: a verb on `ears-cmd.sock`.**
   `desk-done <id> ok|error <reason>`. The engine takes it like the late
   embedding: it sets `action_ms` and beeps on error after the fact, never
   blocking the VAD loop (`src/streaming_engine.rs:2539-2556` is the
   pattern).

So no new socket and no process per action. The dispatch itself goes over
Hyprland's request socket from the resident companion.

The accept hold (`accept_ms`, up to 200 ms before the action starts,
`src/streaming_engine.rs:1204-1208`) should run after the op is sent, not
before: the colour is feedback, not a gate.

### 5. Compositor abstraction (in ears-desk)

```rust
trait Compositor {
    fn snapshot(&self) -> Desk;                    // windows, workspaces, monitors
    fn events(&self) -> Receiver<Change>;          // keeps Desk current
    fn apply(&self, op: &Op) -> Result<Option<Op>>; // done, and its inverse
    fn supports(&self, op: OpKind) -> bool;
}
```

- **Hyprland**: request socket `.socket.sock` (`j/clients`, `j/workspaces`,
  `j/monitors`, `dispatch ...`), events from `.socket2.sock`. The instance
  comes from `HYPRLAND_INSTANCE_SIGNATURE`, else the newest live entry of
  `hyprctl instances` (on centurion the variable is not set in every shell,
  and the runtime dir holds three instance directories, one live).
- **niri**: `$NIRI_SOCKET` JSON requests and its event stream. ears already
  reads niri's focused window (`src/ghost.rs:144-170`).
- **sway**: `$SWAYSOCK` IPC; `pointer.rs` already knows it.
- Detection order as `pointer.rs` does it (sway first because of `hyprctl`
  shims, `src/pointer.rs:14-15`).

Later, the per-question `hyprctl` calls in `ghost.rs`, `pointer.rs` and
`desktop.rs` can read `desk.json` when ears-desk runs. Not in this RFC's
phases; noted so nobody adds a fourth copy.

### 6. Ambiguity and confirmation

Resolution of a `window` slot, in order:

1. a hint label on screen, or `this`/`last`;
2. qualifiers narrow: monitor ("on the big screen"), position ("left
   terminal"), ordinal ("second terminal", left to right, top to bottom);
3. names: exact spoken name, then a title word;
4. still several: prefer the current workspace, then visible, then MRU.

What happens with more than one left depends on the verb:

- **Reversible verbs** (focus, move, swap, float, fullscreen, resize) take
  the best by step 4 and fire. "next" or "again" then moves to the next
  candidate. On centurion's 9 terminals, "focus the terminal" focuses the
  most recent one; a second "next" walks the rest. No question asked:
  undoing is cheaper than asking.
- **Destructive verbs** (close, and any action marked `confirm`) never
  pick. With several candidates, each gets a numbered label on screen
  (from `rect`) and command mode waits for a number, "next" or "cancel"
  (a `pending` frame in `CommandState`, beside `chant`, ending with the
  mode; 8 s, as `[vision] confirm_s`). With one candidate named by words
  or reached by embedding, ears opens a decision ("Close hover: Create |
  Suno?", options yes/no, `exact`, QTE timeout), the red mode that exists
  for this (`src/decide.rs:49-53`).
- `close` with no target, said exactly, keeps today's behaviour: it closes
  the focused window at once (it is `exact` on centurion; the backup
  `actions.toml.bak-exact-close-20261103` dates that change).

Labels are spoken numbers 1 to 20, not letters: the ASR spells letters out
(cs failure mode `spelled`, `src/cs/mod.rs:49-50`) and numbers have one
parser. Spoken feedback goes through the subtitle line; `say` is not used
by ears.

### 7. Hints

- **H1, windows.** "show windows" / "hints" labels every visible window at
  its centre from `desk.json` rects, on every monitor. Labels stay while
  command mode is on or until used. A bare number focuses; a verb plus a
  number acts ("close three" confirms as in section 6). Source: compositor
  geometry, available on every backend, no permissions.
- **H2, in the browser.** Most clickable things on this desktop are in the
  browser, and the browser here already runs with a remote debugging port.
  A DOM query for links, buttons and inputs in the viewport gives exact
  rects and names (Vimium-style) in tens of ms. Needs Markus's go-ahead:
  it couples ears-desk to that port.
- **H3, anything on screen: SAM 3.** "show buttons" prompts vision mode's
  segmenter with a class word and labels the candidates. It reuses
  `ears look` (screenshot, `/segment`, ranking, `pointer::click`) and
  costs a SAM round trip (centurion LAN), so it is the slowest source and
  the most general.
- **H4, AT-SPI**, opt-in: only useful once `IsEnabled` is on and toolkit
  apps register; today it would label waybar only.

Drawing: one renderer for vision candidates, ambiguity labels and hints.
The vision overlay (`vision.json`, drawn by the ears LED scene) is the
existing one; hints go in `desk.json` `hints` and use the command-mode
colour (violet), not vision's pink.

## Interaction with what exists

| piece | change |
|---|---|
| `Mode`, `CommandState` (`src/vad_mode.rs:47-73`) | `CommandState` gains `pending: Option<Pending>` (ambiguity) and `hints: bool`; both end with command mode, like `chant`. No new `Mode`. |
| chant (`src/chant.rs:94-105`) | goes through `Router::parse`, so "workspace three workspace three" fires once. `again` is chantable; a pending question is not (numbers are answered plainly). |
| cs (`src/cs/mod.rs`) | new targets `slot:number`, `slot:dir`, `slot:window:<name>` (e.g. heard "ala critty", meant "alacritty"): mishearings of a value apply in every template. `action:<name>` keeps working for literal words. `ears cs build` probes values in a carrier phrase ("focus {value}"). |
| ASR context (`src/cs/context.rs:93-104`) | order: built-ins, one rendered phrasing per template, live window and monitor names, then flat actions round-robin. Templates collapse 124 actions into about 20 entries, so far more of the vocabulary fits under 512 characters than today's 50 of 355. |
| words-first, early end (#284) | template frames count as words (`fires_by_words`); closed and live instances are complete phrases for `Ending::Exact`. |
| decide (`src/decide.rs`) | used as is for yes/no confirmation. Its `exact` mode should accept the digit form of a numeric option; numbered labels otherwise stay in command mode. |
| `Hearing`, `command-last.json` | gains `slots` and, for desk ops, the op id; `Timing` gains nothing: `action_ms` becomes the ack from ears-desk. |
| `rest` actions | unchanged; `rest` is `{rest}` at the end. |
| vision mode | unchanged in phase order; H3 calls into it. |

## Latency budget

From the end of speech; the stages are `Timing`'s.

| stage | today (centurion) | budget for a slotted command |
|---|---|---|
| silence before the end is called | 150 ms when the transcript is an exact phrase; 400 key command; 700 finished sentence; 1500 otherwise | 150 ms: every complete frame is an exact phrase |
| asr + decision | median 103 to 131 ms after the segment (20 utterances, above); `COMMAND_ASR_BUDGET` caps the ASR at 1500 ms (`src/streaming_engine.rs:168`) | unchanged; parsing a frame is in-process (microseconds) |
| embedding | not waited for when words fire | not waited for |
| resolve a window | n/a | under 1 ms: a list in memory |
| dispatch | `hyprctl` process, median 3.9 ms | under 1 ms over the socket from ears-desk; the op event on `ears.sock` is a local write |
| accept hold | up to 200 ms before the action | after the action |

Per primitive, from the last word to the effect: workspace, focus, move,
swap, float, fullscreen by words: about 300 ms (150 + about 130 + a few).
By embedding: add the wait for the audio embedding as today. Ambiguous
reversible: the same, then about 300 ms per "next". Destructive with
confirmation: one more utterance. H1 hints: drawing only, under 50 ms. H2:
a DOM query, tens of ms (to measure). H3: a SAM round trip, the slowest
(vision mode's own budget, `docs/VISION_MODE.md`).

The single largest term is the silence: a slotted phrase that is not
recognized as complete waits 700 to 1500 ms. That is why expansion into
complete phrases is in phase 1, not an optimization for later.

## Migration of the 348 flat actions

Nothing is rewritten by hand and nothing is lost:

- `ears actions migrate --dry-run` groups actions whose `run` differs only
  in one argument that is a number or direction and whose phrasings differ
  only in that word, and prints the template that replaces them, with the
  phrasings each family had. Without `--dry-run` it writes a new file next
  to the old one (`actions.toml.new`); the user moves it in. Voice anchors
  of the old actions (`command-anchors.json`) are relabelled to the template
  name, so the voice's training is kept.
- Expected on centurion: 45 numbered actions -> 5 templates; 63 direction
  and monitor actions -> about 12; 12 focused-window verbs and 4 workspace
  moves -> desk verbs with `this` (phase D2). 39 launchers can become
  `open {app}` once ears-desk knows desktop entries (open question 5). The
  78 tmux keys, 8 keys and 99 others stay flat; they have no slot.
- Old names stay valid as aliases for `ears teach`, cs `action:` targets
  and logs for one release.

## Phased plan

Each phase is one or two small PRs and useful alone.

| phase | PRs | useful because | measure |
|---|---|---|---|
| S1 `spoken.rs` | one refactor: number, ordinal, count parsing with the union of today's tests; homophones become `cs/any` `slot:number` entries | one place to fix "to" vs "two" | no behaviour change: existing tests pass |
| S2 closed-slot templates | `{n}`, `{dir}`, `{monitor}`, `{count}` in phrasings and `run`; `Router::parse`; expansion into `Phrases`; context per template; chant through the parser | "workspace three", "scroll down five" with no new process | share of command utterances ending Exact (`timing.silence_ms`), context coverage, sibling misroutes via the cs journal (#279) and `wrong` |
| S3 migration tool | `ears actions migrate --dry-run` | 348 -> about 245 actions on centurion (124 become about 20 templates) | action count; anchors relabelled; no phrase lost (diff of expanded phrase sets is empty) |
| S4 `again` and counts | built-in `again`, repeat counts | "again three times" | uses per day from `command-last.json` |
| D1 ears-desk, read-only | companion binary with the Hyprland backend writing `desk.json` from the event socket; `ears desk status` | window names enter the ASR context and the parser | name hit rate ("no window called X" misses), freshness (generation lag) |
| D2 desk ops | `DeskOp` event, `desk-done` verb, verbs of section 2 on `{window}`, MRU choice, "next" | "focus hover", "move this to workspace two" | `action_ms` (target under 10 ms after the decision), error rate |
| D3 confirmation and ambiguity labels | `pending` frame, numbered labels, decide for yes/no | "close the terminal" safely | confirmations answered vs timed out; cancels |
| D4 undo | inverse stack in ears-desk, `undo`, `wrong` undoes | cheap recovery | undo uses; inverse failures |
| H1 window hints | labels from rects, bare number focuses | point at a window without a name | labels used per hint session |
| H2 browser hints | DOM query over the remote protocol (if approved) | links and buttons by number | query time, click success |
| H3 SAM hints | "show buttons" through vision | any app | time to labels |
| N1 niri backend | second `Compositor` | same grammar on niri machines | parity checklist of section 2's table |

## Non-goals and risks

- No LLM in the routing path. The `agent` escape hatch stays the place for
  what the grammar cannot say.
- No dictation-mode changes: slots apply in command mode only (and in
  standby run commands only if asked for later).
- Risk: templates make more utterances parse as commands. Mitigation: a
  frame fires only when every required slot is filled; `cs check` scores
  the expanded phrases like any other; `none` still catches chatter.
- Risk: `desk.json` is world-readable window titles in the runtime dir.
  It lives under the user's `$XDG_RUNTIME_DIR`, which is mode 0700 (the
  `ears/` dir inside is 0755, so the parent is what protects it), like
  `vision.json`.

## Open questions for Markus

1. **Where does ears-desk live?** (a) a subcommand of the ears binary run
   as its own user service, (b) a separate crate in this repo, (c) a
   separate repo or hai-os. *Recommendation: (b)*, a second binary in this
   repo sharing `pointer.rs`/`ghost.rs` compositor code, installed and
   started only when enabled. It keeps #209's "nothing depends on it" while
   avoiding a third copy of the Hyprland code.
2. **Transport.** Existing sockets (op as an `ears.sock` event, result as an
   `ears-cmd.sock` verb, vocabulary as `desk.json`) or a dedicated
   request/response socket for ears-desk? *Recommendation: existing
   sockets*, as above; revisit only if the ack round trip measures over
   10 ms.
3. **Ambiguity for reversible verbs: act or ask?** *Recommendation: act on
   the most recent match, "next" to walk.* On centurion 9 of 10 windows are
   terminals; asking every time would make "focus terminal" a two-step
   command.
4. **What does `wrong` do once undo exists?** *Recommendation: forget the
   anchor and undo the effect when it can be undone.* "undo" alone keeps
   the anchor.
5. **Should the 39 launchers become `open {app}`?** *Recommendation: yes,
   in D2*, with "open" focusing an existing window of that app first
   (as `omarchy-launch-or-focus` does) and "new {app}" always launching.
6. **Hint labels: numbers or words?** *Recommendation: numbers 1 to 20*,
   with two-word labels ("blue three") only if a screen has more than 20
   targets.
7. **Browser hints over the remote debugging port (H2)?**
   *Recommendation: yes, read-only DOM queries plus a click by coordinates
   through `pointer.rs`; no script injection.* Needs your go-ahead because
   it ties ears-desk to the browser's debugging port.
8. **AT-SPI.** Switching `org.a11y.Status.IsEnabled` on makes toolkit apps
   build trees for everyone. *Recommendation: leave it off; H4 stays
   opt-in and last.*
9. **Destructive actions.** Which flat actions besides `close` should
   confirm when reached by embedding or by name (e.g. `tmux-kill-*`,
   `lock-system`)? *Recommendation: a `confirm = "by-name"` default for
   `close`, and `confirm = "embedding"` for every action that is `exact`
   today*, which keeps them words-only as now.
