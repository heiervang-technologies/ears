# Qwen3-ASR live token freeze tracking

The running setup inspected on 2026-09-29 served `Qwen/Qwen3-ASR-1.7B`
on `http://localhost:30189` with a 2048-token context and
`--trust-request-chat-template`. Ears' continuous path is implemented by
`src/continuous.rs` and the `ears_stream` plugin at `/v1/ears/stream`.

There are two distinct windows:

- The audio encoder processes fixed 8-second items. Closed items can be
  reused by vLLM's caches. The model's published configuration has
  `thinker_config.audio_config.n_window_infer = 800`:
  [Qwen model configuration](https://huggingface.co/Qwen/Qwen3-ASR-1.7B/blob/main/config.json).
- Ears forces an increasing text prefix into each new assistant prompt.
  The last `rollback_words` words (default 3) remain revisable. This is a
  client/plugin decoding policy, not a sliding attention window that makes
  words immutable simply because their audio is old. It differs from the
  [upstream Qwen streaming wrapper](https://github.com/QwenLM/Qwen3-ASR/blob/main/qwen_asr/inference/qwen3_asr.py),
  which uses token rollback.

## What “frozen” guarantees

The frozen UTF-8 prefix is passed verbatim into later decoding requests for
this utterance. The model only generates its continuation. No confidence
threshold or repeated-text heuristic is used. Until a language header is
pinned, no live prefix is frozen. Completed rollover segments are frozen
in front of the next segment. HTTP fallback now preserves this prefix
monotonically, including when a continuation is shorter than the rollback
allowance.

This is a **live decoding guarantee**. Ghost text is still preedit, not text
committed to the application. `final_correction = true` runs a separate full
transcription that can replace even a live-frozen prefix. Switching to
repeated transcription also removes that live guarantee. Cancellation,
new recordings, and rejected speech candidates reset the state.

## Visualization

With `live_decoding = "continuous"`, push-to-talk and VAD ghost previews
send the boundary automatically. The updated fcitx5 addon draws the frozen
prefix with `HighLight` and underlines both parts. The Wayland frontend
exports the highlighted prefix as the preedit cursor range `[0, n)`, which
patched Alacritty uses for the configured frozen color. Both remain
uncommitted until the existing commit path runs. Filters map the boundary
into the displayed text, including Unicode case changes and punctuation
removal; invalid byte offsets freeze nothing.

Applications and input-method themes can override preedit formatting.
An older addon falls back to the existing uniform ghost display. Rebuild
and install the ears binary and `fcitx5-addon` to enable the inline split;
update the vLLM plugin for the additional tokenizer metadata. No system
configuration or running services are changed by building this branch.

For a live view alongside any application (including those that ignore
preedit styling), run `ears ghost-watch` in another terminal. It observes
the addon without claiming input focus or ghost ownership, prints the
`FROZEN | revisable` split on every change, and clears when the ghost clears.
`ears ghost-watch --json` emits changed snapshots with `scope: "live_decode"`,
`text` and `frozen_bytes` for other visualizers. Ctrl-C stops the observer.

For a model-token visualization while streaming a WAV:

```bash
python vllm-plugin/tools/stream_wav.py clip.wav --tokens \
  --url ws://localhost:30189/v1/ears/stream --language en
```

This prints each tokenizer ID, its text span and `FROZEN`/`mutable`, plus
closed audio windows and the current open window. Audio-window counts are
not word timestamps. An older server reports metadata unavailable.

## Additive protocol extension

Partial and final frames now contain `stability`:

```json
{
  "profile": "qwen3_asr_forced_prefix_v1",
  "scope": "live_decode",
  "frozen_bytes": 5,
  "rollback_words": 3,
  "encoder_window_ms": 8000,
  "closed_audio_windows": 1,
  "open_audio_ms": 500,
  "token_basis": "retokenized_transcript",
  "tokens": [
    {"id": 14990, "start_byte": 0, "end_byte": 5, "state": "frozen"}
  ]
}
```

The token entry above illustrates the shape; consumers must use the actual
server's IDs and spans. `frozen_bytes` equals legacy `stable_chars` on
partials. Final frames have `scope = "final"` and freeze the entire returned
text for this stream. A later client-side final correction is outside that
scope. Boundaries refer to the raw `text`, before display filtering.

The tokenizer is the loaded model's own tokenizer. IDs describe a fresh
tokenization of the transcript, **not** the history of sampled tokens or
persistent token identities across updates. BPE can merge across the
freeze boundary: a token crossing it is conservatively marked mutable,
although the portion before `frozen_bytes` is frozen. Byte-fallback tokens
can share overlapping Unicode spans. If tokenizer offsets are unavailable,
`tokens` and `token_basis` are omitted; the text boundary remains exact.

No protocol version bump is needed: these fields are additive. A partial
is also emitted when only the frozen boundary changes, even if its text is
identical. Existing clients continue using `stable_chars`.

The addon accepts `F <frozen_bytes> <escaped text>` alongside the existing
`P`, `C`, `X`, and `S` commands. The read-only `T` command returns
`OK state <frozen_bytes> <escaped text>` without taking ghost ownership. Its byte boundary is measured after
unescaping (and after CR removal), validated before slicing, and applies
only to formatting. No marker characters are inserted into the transcript.

## Continuous integration

Pull requests run the Rust suite, the plugin's protocol/session/engine tests,
and a build of the fcitx5 addon. Plugin tests use a deterministic decoder and
need no GPU, model download, or installed vLLM. A live rollout still needs the
synthetic WAV check above to verify the deployed model and tokenizer, and real
application testing to verify preedit rendering.
## Alacritty and tmux overflow

For a tmux client descended from the focused Alacritty window, Ears compares
preview display width with `pane_width - cursor_x`. Text that would overflow
(or contains a line break or tab) no longer fits the cursor's row. When
`alacritty.toml` sets `[colors.preedit] wrap = true` (patched Alacritty), Ears
word-wraps it into the pane: the first row starts at the cursor, later rows are
newline-separated and indented by `pane_left` spaces, which Alacritty leaves
undrawn, and the grid under the text is hidden. When the rows below the cursor
run out, the oldest words give way to `…`. The frozen byte count is mapped into
the wrapped text, and the commit still delivers the original transcript.
Without `wrap`, the preview moves to the fcitx popup; short previews stay
inline. Only the matched client's pane is used. Probes have deadlines; unknown
geometry and older addons retain the existing inline behavior. Other terminal
servers and non-default tmux sockets are not inferred from unrelated clients.

The addon command `B <frozen_bytes> <escaped text>` shares `F`'s byte validation
and ownership rules, but selects a wrapped popup. Read-only popup rows wrap around 48 columns (fcitx preedit itself is single-line); the observer retains the raw text and original
boundary, and commit still delivers the original transcript. Switching between
popup and inline clears the previous surface. Popup colors follow the fcitx
classic UI theme; popup text has no underline, so a theme with the terminal's
font and colors (`NormalColor` for the ghost, `HighlightColor` for frozen
text) matches it. Clicking a preview row does not commit it.

Validated in an isolated headless Wayland session with the patched Alacritty,
tmux and fcitx addon: inline frozen color, multiline popup, exact observer text
and byte boundary, and clearing without sending text to the shell. The Sway
test exercises rendering; Hyprland focus probing is covered separately.
