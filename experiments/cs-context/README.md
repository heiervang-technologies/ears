# ASR context for command phrases (cs prevention)

Does passing the phrases a mode listens for to Qwen3-ASR as context make
command phrases come out right, and at what cost? RFC 0002, "Prevention".

## Setup

- ASR: `Qwen/Qwen3-ASR-1.7B` on vLLM, `localhost:30189` (centurion), 2026-11-13.
- Audio: `espeak-ng`, two voices: `us150` (en-us, 150 wpm) and `gb175`
  (en-gb, 175 wpm, pitch 40), resampled to 16 kHz mono. espeak is harder to
  hear than a person; absolute rates are lower than live use.
- Clips: 48 command phrases (`commands.txt`: wake/sleep, `[commands]`, run
  commands, command-mode actions incl. tmux and Hyprland jargon) and 44
  ordinary sentences (`ordinary.txt`, several deliberately close to a
  command: "turn it down a little bit please", "we should close the window
  before it rains").
- Paths: `http` is `/v1/audio/transcriptions` with the context as `prompt`
  (what ears' final transcription sends); `chat` is chat completions with the
  context as the system turn (what the live decoder sends); `forced` is chat
  with the `language English<asr_text>` header forced (a pinned language).
- Contexts: `none`; `list`, the 48 command phrases (757 chars); `capN`, the
  48 phrases shuffled into 274 other command phrasings (5.3k chars) and cut
  to whole phrases within N chars, so only some test phrases are in it
  ("in ctx" / "not in ctx").
- Scoring with ears' `commands::normalize`. **cmd exact**: the phrase comes
  back exactly. **misroute**: a command clip comes back as a *different*
  known command phrase. **ordinary FP**: an ordinary sentence comes back as
  any known command phrase. **empty**: no text (`language None`).
- `+fallback` rows are derived, not decoded again: an empty result with a
  context is replaced by the same clip's `none` result (temperature 0) and
  both latencies are added. This is what ears does.
- Requests: 759 ASR requests in all (692 main run, 55 non-speech/forced run,
  11 probes or repeats and one rejected request) plus one `tokenize` call,
  two in flight at most.

```bash
python3 -I eval.py run      # 692 requests -> results.jsonl, then the table
python3 -I eval.py run2     # 55 requests  -> results-noise.jsonl
python3 -I eval.py table    # tables from the saved results
python3 -I eval.py noise
```

## Measured

| path | condition | voice | ctx chars | cmd exact | in ctx | not in ctx | misroute | ordinary FP | ordinary exact | empty | median ms | p90 ms |
|---|---|---|---|---|---|---|---|---|---|---|---|---|
| http | none | us150 | 0 | 67% (32/48) | - | 67% (32/48) | 0 | 0% (0/44) | 93% (41/44) | 0 | 92 | 108 |
| http | none | gb175 | 0 | 60% (29/48) | - | 60% (29/48) | 0 | 0% (0/44) | 95% (42/44) | 0 | 89 | 109 |
| http | list | us150 | 757 | 85% (41/48) | 85% (41/48) | - | 0 | 0% (0/44) | 98% (43/44) | 7 | 89 | 110 |
| http | list | gb175 | 757 | 81% (39/48) | 81% (39/48) | - | 0 | 0% (0/44) | 95% (42/44) | 7 | 90 | 109 |
| http | cap256 | us150 | 242 | 73% (35/48) | 100% (5/5) | 70% (30/43) | 0 | 0% (0/44) | 95% (42/44) | 7 | 90 | 113 |
| http | cap1024 | us150 | 1021 | 69% (33/48) | 100% (9/9) | 62% (24/39) | 0 | 0% (0/44) | 95% (42/44) | 9 | 90 | 110 |
| http | cap4096 | us150 | 4080 | 77% (37/48) | 81% (30/37) | 64% (7/11) | 0 | 0% (0/44) | 95% (42/44) | 9 | 97 | 114 |
| chat | none | us150 | 0 | 75% (18/24) | - | 75% (18/24) | 0 | - | - | 0 | 70 | 80 |
| chat | list | us150 | 757 | 75% (18/24) | 75% (18/24) | - | 0 | - | - | 5 | 58 | 64 |
| http | list+fallback | us150 | 757 | 94% (45/48) | 94% (45/48) | - | 0 | 0% (0/44) | 98% (43/44) | 0 | 96 | 114 |
| http | list+fallback | gb175 | 757 | 85% (41/48) | 85% (41/48) | - | 0 | 0% (0/44) | 95% (42/44) | 0 | 92 | 111 |
| http | cap256+fallback | us150 | 242 | 83% (40/48) | 100% (5/5) | 81% (35/43) | 0 | 0% (0/44) | 95% (42/44) | 0 | 95 | 115 |
| http | cap1024+fallback | us150 | 1021 | 81% (39/48) | 100% (9/9) | 77% (30/39) | 0 | 0% (0/44) | 95% (42/44) | 0 | 99 | 116 |
| http | cap4096+fallback | us150 | 4080 | 88% (42/48) | 86% (32/37) | 91% (10/11) | 0 | 0% (0/44) | 95% (42/44) | 0 | 102 | 119 |
| chat | list+fallback | us150 | 757 | 88% (21/24) | 88% (21/24) | - | 0 | - | - | 0 | 59 | 110 |

Non-speech clips (1.5 s unless noted), what came back:

| clip | http none | http list | http cap1024 | chat none | chat list | forced list |
|---|---|---|---|---|---|---|
| silence | No. | (empty) | (empty) | (empty) | (empty) | ears on, ears off, hey c... |
| white-quiet | (empty) | (empty) | (empty) | (empty) | (empty) | ears on, ears off, hey c... |
| white-loud | (empty) | (empty) | (empty) | (empty) | (empty) | ears on, ears off, hey c... |
| pink | (empty) | (empty) | (empty) | (empty) | (empty) | ears on, ears off, hey c... |
| brown | (empty) | (empty) | (empty) | (empty) | (empty) | ears on, ears off, hey c... |
| hum | (empty) | (empty) | (empty) | (empty) | (empty) | ears on, ears off, hey c... |
| beep (0.3 s + 0.7 s pad) | (empty) | (empty) | (empty) | (empty) | (empty) | Ears on, ears off, hey c... |
| short-white (0.4 s) | (empty) | (empty) | (empty) | (empty) | (empty) | ears on, ears off, hey c... |

Forced header with the `list` context on the 7 clips the context emptied
(`http list us150`): 7 of 7 came back non-empty, 5 of 7 exact ("Ears on.",
"Ears off.", "Over and out.", "Enter.", "Close window."; "Ears held.",
"Run." wrong as without context).

Other observations from `results.jsonl`:

- Every empty result with a context was a command clip that the model had
  decoded to text without one, mostly short ones: "ears on", "ears off",
  "ears help", "over and out", "enter", "wrong", "close window", "new line",
  "tmux split". Without context none of the 88 speech clips came back empty.
- Fixed by the context (`us150`, `none` -> `list`): "Hey, Klanker!" -> "Hey
  clanker.", "Listen, planker." -> "Listen clanker.", "Fuck off, Glanker." ->
  "Fuck off, clanker.", "Varinoid." -> "Voronoi.", "Timux leeft." -> "tmux
  split", "Teamokscopie-mod." -> "tmux copy mode.", "Demux kill the active
  pain." -> "tmux kill the active pane.", "Toddl scratch pad." -> "Toggle
  scratchpad.", "Brighten us up." -> "Brightness up.".
- No ordinary sentence came out as a command phrase in any condition
  (0 of 220 decodes with a context). The context changed 9 of those 220
  ordinary decodes: 7 for the better ("ears after" -> "ears off", "bill" ->
  "build" in four, "poll request" -> "pull request"), 1 for the worse
  ("descaling" -> "scaling", gb175, `list`) and 1 sideways.
- Command clips never came back as a different command (misroute 0).
- `tokenize`: the 1021-char `cap1024` context is 228 tokens.

## Interpretation

- The context helps where the model has no prior for the word (clanker,
  tmux, voronoi, scratchpad): +27 points (us150) and +25 (gb175) exact match
  with the fallback, with no false positives on ordinary sentences and no
  misroutes in this set. n is small (48 phrases, synthetic voices), so the
  size of the gain is uncertain; the direction is consistent across both
  voices and every cap.
- Without the fallback the context is a trade: it empties short phrases
  that were right ("ears off", "enter"). Retrying the empties without the
  context keeps nearly all of the gain and removes that loss, at about
  +5 ms median (only empties pay for a second request).
- Forcing the language header avoids the empties but writes the context out
  on every non-speech clip, so it must never be combined with a context
  without an echo check. The unforced path heard nothing in every
  non-speech clip, with or without context.
- Phrases outside a capped context were not measurably hurt (62-70% vs 67%
  without context; 77-91% with the fallback); the in-context phrases carry
  the gain. Latency was flat up to 4096 chars (+5 ms median).
- The cap is set by the token budget, not by accuracy: the stream plugin
  reserves 256 tokens for template, context and its 64-token base, which
  leaves roughly 170 tokens (about 750 chars) for a context before the
  longest utterance could overflow. Default 512 chars (about 115 tokens),
  between the measured 256 and 1024; it is not itself measured.

## What ears does with this

`[cs] context = true` (default) and `context_chars = 512`: in command mode
and standby the mode's phrases go to the ASR as context (final HTTP
transcription and the live decoder), never in dictation, bash mode or while
a decision is open. An empty or echoed result is transcribed again without
the context. See `src/cs/context.rs`.
