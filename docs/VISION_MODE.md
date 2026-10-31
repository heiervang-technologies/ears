# Vision mode (pink): point at the screen by voice

Status: P0 and pink mode implemented (`ears look`, colour word "pink"), 2026-11-03.

Say what you want to click. ears finds it on screen with SAM 3, outlines its
best guess in pink, and clicks it once you confirm:

    "pink"                          LED turns pink; speech is no longer typed
    "the send button"               the send button is outlined, numbered 1;
                                    other matches are outlined faintly as 2, 3, ...
    "click" / "left click"          left click inside it
    "right click" / "middle click"  that button
    "up twenty" / "down five"       scroll there by that many notches, then look again (forms)
    "next" / "two"                  move to the next match, or pick #2
    "the search field"              look for something else on the same screenshot
    "right click the blue folder"   a verb up front: outline, then "yes" does it
    "cancel"                        drop the guess; "blue" leaves vision mode

The SAM model runs on centurion. Hackbook, or any other machine, sends its
own screenshot over the LAN.

## Colour

Cyan and teal are taken: `#22d3ee` is dictation, and it is also the default
accent of `see`. Vision mode is **pink**, `#ff4fd8`. The trigger words are
"pink", "magenta", "vision" and "look", in `Colour::parse`
(`src/colour_mode.rs`). The LED gets a `vision` route in `route()` and
`tone()` of `haios-ears-led`.

## Pipeline

```
speech onset (VAD) ──► screenshot (grim, focused output) ──► POST /image  ─┐
                                                              (centurion)  │ vision features
stream partials ──► stable noun phrase? ──► POST /segment {image_id,text} ◄┘ computed while
                                            (speculative; reused if the       you still talk
                                             final text says the same)
final text ──► parse verb + target + ordinal/spatial words
           ──► candidates (masks, scores) ──► rank ──► overlay (pink outline + numbers)
           ──► confirm by voice ──► pointer backend: move + click inside the mask
```

### 1. Take the screenshot at speech onset, not at the end

What you are describing is what was on screen when you started speaking.
Taking the screenshot at `SpeechStarted` also lets the slow part (the image
encoder) run while you are still talking.

### 2. Split SAM into an image call and a prompt call

The image encoder is the expensive half of SAM 3. In transformers,
`Sam3Model.get_vision_features()` and `forward(vision_embeds=...)` already
separate it from the text prompt (`modeling_sam3.py:2238, 2272`). Today
`sam-segment` (`~/ht/cloud/docker/sam-segment/app.py`) runs both on every
request. Add two endpoints:

- `POST /image` (JPEG/PNG) returns `{image_id, size}`. It computes the vision
  features once and keeps them in a small LRU cache on the GPU (about 4
  images).
- `POST /segment {image_id, prompt:{text}}` runs only the text and detector
  half. That should be well under the documented 0.7 to 3 s per segment;
  this needs measuring.

`next`, `two` and re-wording ("no, the *other* send button") reuse the same
`image_id`, so they cost only the prompt half.

### 3. Query early from the stream

The live stream, Moonshine on hackbook or Qwen on centurion, settles text
before you finish. Once a settled partial holds a whole noun phrase, fire
`/segment` speculatively. If the final target matches, its candidates are
already there, and the outline can appear the moment you stop talking.

### 4. Parse the utterance (no LLM needed)

```
[verb] [ordinal/spatial] target [spatial]
verb    : click | right click | double click | move to | hover | (none: outline only)
ordinal : first … fifth, last, "number N"
spatial : top/bottom/left/right, "in the sidebar" (later), "next to X" (later)
target  : the rest, articles stripped → SAM text prompt
```

SAM 3 works best on short noun phrases ("send button", "blue folder icon"),
and it returns *every* instance above 0.4. So ordinals and spatial words are
applied by ears, after segmentation, to those instances. They are not passed
to SAM.

### 5. Rank candidates

Start from SAM's score, then add priors:

- inside the focused window, and near the pointer;
- a plausible size for a click target, not half the screen;
- above a minimum mask area.

Show at most 5 candidates. Number them in rank order, so "two" means what it
shows.

### 6. Where to click

Click the point inside the mask farthest from its edge (the pole of
inaccessibility, from a distance transform), not the centroid. The centroid
of a ring, an L-shaped region or a text label with a hole can fall outside
the mask. "move to" only moves the pointer there.

### 7. Overlay: extend the LED scene, not `see`'s GTK window

`see highlight` uses GTK4 with gtk4-layer-shell and Hyprland-only geometry.
Hackbook has neither. The ears LED scene already runs on both machines, per
screen. So:

- ears writes `$XDG_RUNTIME_DIR/ears/vision.json`:
  `{id, prompt, output, candidates:[{n, score, polygon:[[x,y],...], point:[x,y]}], picked, state}`.
- Polygons are simplified mask contours in output-local logical pixels, a
  few hundred points each. Sending them instead of RLE keeps the QML side
  trivial.
- `haios-ears-led feed` passes it on. QML draws a `Shape` path per candidate:
  a soft pink fill plus a 2 px outline for #1, faint outlines and number
  badges for the rest. The prompt line goes where the subtitles go
  ("click 'send button'? yes / next / cancel"), reusing the `decide` layout
  and its timeout bar.

### 8. Pointer and screen backends (`src/pointer.rs`, `src/screen.rs`)

| | Hyprland (centurion) | sway (hackbook) |
|---|---|---|
| outputs, scale, focus | `hyprctl -j monitors / activewindow` | `swaymsg -t get_outputs / get_tree` |
| screenshot | `grim -o OUT` | `grim -o OUT` |
| move | `hyprctl dispatch movecursor x y` | `swaymsg seat - cursor set x y` |
| click | `ydotool click 0xC0/0xC1` | `swaymsg seat - cursor press/release button1/3` |

The backend is chosen at runtime from `$HYPRLAND_INSTANCE_SIGNATURE` or
`$SWAYSOCK`. Hackbook's `ydotool` shim cannot click, and `/dev/uinput` is
root-only there, so the sway seat commands are the path that works.

### 9. Confirmation and safety

- Never click unconfirmed by default. An optional `[vision] auto_click_score`
  (default off) clicks at once when exactly one candidate scores above it.
- Reuse `Decision::hear` matching (words, ordinals, embeddinggemma2
  fallback) with the fixed options `yes`, `next`, `cancel` plus the
  candidate numbers. "click it" and "that one" count as yes. A new
  description re-queries the same screenshot.
- The guess times out after 8 s, with the bar draining as in decide mode.
- This is the user's own input, so it does not go through the agent
  `controls` gate. It does stop if `controls` says an agent is driving.

### 10. Running it on centurion, using it from hackbook

- `sam-segment` is a k8s Deployment pinned to centurion's 3090, NodePort
  30190, and **currently scaled to 0**.
- Entering pink mode should warm it: scale to 1, then poll `/health`, which
  returns 503 while loading. The LED shows "warming" until it is ready.
  After N idle minutes it scales back down, so the GPU is not held. Who may
  scale it, ears or a small broker on centurion, needs deciding (see
  Questions).
- Hackbook sends a 1280×800 JPEG, about 150 KB, over the LAN.
- `[vision] server = "http://centurion.lan:30190"` in the ears config.

## Plan

1. **P0, CLI first:** `ears look "send button" [--click|--right|--move]`,
   with no VAD.
   - Screenshot, `/segment`, ranking, `vision.json` overlay, confirm via the
     existing `ears decide`, then the pointer backend.
   - Works on both machines.
   - Measures end-to-end latency per stage.
2. **P1, server:** `/image` + `/segment {image_id}` with the feature cache,
   and warm-up/scale-down.
3. **P2, pink mode:** colour word, screenshot at speech onset, utterance
   grammar, the confirm loop in the VAD engine, speculative queries from
   stream partials.
4. **P3:** spatial relations ("next to", "in the sidebar"), "scroll down",
   "drag X to Y", follow-ups on the last target ("right click it").

## Decided (Markus, 2026-11-03)

- Pink, triggered by "pink / magenta / vision / look".
- A bare description only outlines. Then the command: click, left click,
  right click, middle click, or "up N" / "down N" to scroll forms.
- ears (this desk) owns scaling `sam-segment` for now. SAM may later also
  run on other GPU machines, but it runs on centurion first.

## Measured

`/segment` on a 1280×800 hackbook screenshot (JPEG, 180 KB), sent to
centurion's 3090, round trip including upload: 160 to 210 ms. That is far
under the documented 0.7 to 3 s. Splitting off `/image` (step 2) is
therefore an optimisation for later, not a prerequisite.
