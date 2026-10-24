# Audio-embedding command routing with EmbeddingGemma 2

Question: can EmbeddingGemma 2 (k8s `default/embeddinggemma2`, NodePort 30201, llama.cpp
b11457, Q8_0) route short spoken ears commands straight from audio, without a transcript?

Script: `route.py` (`python3 route.py all`; clips and caches live in `/tmp/acmd`).

## How to call it with audio

The server is llama.cpp (`owned_by: llamacpp`, `GET /props` exposes `media_marker`). OpenAI-style
`input_audio` content parts are **rejected** on `/v1/embeddings`. What works is the llama.cpp
multimodal prompt object, with the marker from `/props` standing in for the clip:

```json
POST /v1/embeddings
{"input": {"prompt_string": "<__media_JWjkcQPZggi6fKEcWUQ9MaL6JdeGXdVg__>",
           "multimodal_data": ["<base64 of a 16 kHz mono wav>"]}}
```

(`/embeddings` with `"content": {...}` also works but returns un-pooled per-token vectors.)
A 1 s clip costs ~30 tokens (25 tokens/s, per the model card). Output is 768-d. The model card
says: audio without a prefix, text with a task prefix (`task: classification | query: ...`,
`task: sentence similarity | query: ...`, `title: none | text: ...`).

## Setup

- 12 command classes, 2-4 text phrasings each (config phrasings + variants): enter, new line,
  scratch that, back three, ears off, ears on, clear/erase, go back, scroll down, menu, search bar,
  workspace three.
- Queries: 18 command utterances (16 config phrasings + 2 unseen variants) and 14 non-commands,
  several deliberately close to commands ("enter the room", "over there", "the menu was great",
  "clear skies today", "three more", "back then").
- Voices: espeak-ng `en-gb-x-rp` (voice A, used **only** as the few-shot audio exemplar bank),
  `en-us` 160 wpm (B) and `en-us+f3` 185 wpm (C) as queries, plus 15 Kokoro `am_adam` clips
  (9 commands, 6 non-commands) as the most natural, held-out voice. No real voice clips were
  available.
- Score per class = max cosine to that class's exemplars. "Recall @0 FP" = commands routed to the
  right class with a score above the best-scoring non-command (the threshold that keeps false
  triggers at zero). AUC = command vs non-command on the top score.
- Transcript route: Qwen3-ASR-1.7B (port 30189) -> text embedding -> nearest text exemplar, and
  plain exact match.

Small sample (64 espeak queries, 15 Kokoro); treat differences of a few points as noise.

## Results


### espeak B+C

| route | n cmd/non | top-1 | median margin | cmd score med | non score med / max | recall @0 FP | recall @1 FP | AUC |
|---|---|---|---|---|---|---|---|---|
| audio[plain] -> text[none] | 36/28 | 78% | 0.105 | 0.861 | 0.793 / 0.877 | 36% | 36% | 0.78 |
| audio[plain] -> text[classification] | 36/28 | 86% | 0.048 | 0.693 | 0.647 / 0.737 | 28% | 33% | 0.82 |
| audio[plain] -> text[similarity] | 36/28 | 89% | 0.059 | 0.746 | 0.683 / 0.763 | 39% | 42% | 0.80 |
| audio[plain] -> text[document] | 36/28 | 83% | 0.067 | 0.801 | 0.731 / 0.826 | 31% | 56% | 0.83 |
| audio[plain] -> audio few-shot (voice A) | 36/28 | 97% | 0.080 | 0.925 | 0.834 / 0.919 | 56% | 61% | 0.89 |
| audio[classification] -> text[none] | 36/28 | 81% | 0.087 | 0.787 | 0.716 / 0.798 | 44% | 47% | 0.86 |
| audio[classification] -> text[classification] | 36/28 | 81% | 0.064 | 0.823 | 0.768 / 0.851 | 19% | 19% | 0.79 |
| audio[classification] -> text[similarity] | 36/28 | 83% | 0.085 | 0.850 | 0.773 / 0.864 | 39% | 39% | 0.87 |
| audio[classification] -> text[document] | 36/28 | 86% | 0.078 | 0.840 | 0.762 / 0.839 | 50% | 50% | 0.87 |
| audio[classification] -> audio few-shot (voice A) | 36/28 | 94% | 0.083 | 0.926 | 0.846 / 0.919 | 53% | 56% | 0.88 |
| ASR -> text[none] | 36/28 | 86% | 0.162 | 0.877 | 0.793 / 0.886 | 47% | 56% | 0.77 |
| ASR -> text[classification] | 36/28 | 83% | 0.146 | 0.916 | 0.769 / 0.861 | 64% | 67% | 0.82 |
| ASR -> text[similarity] | 36/28 | 89% | 0.110 | 0.976 | 0.846 / 0.938 | 64% | 64% | 0.82 |
| ASR -> text[document] | 36/28 | 86% | 0.152 | 0.936 | 0.793 / 0.922 | 61% | 61% | 0.80 |

ASR exact match (espeak B+C): commands caught 53% (19/36), false triggers 0/28

### Kokoro am_adam

| route | n cmd/non | top-1 | median margin | cmd score med | non score med / max | recall @0 FP | recall @1 FP | AUC |
|---|---|---|---|---|---|---|---|---|
| audio[plain] -> text[none] | 9/6 | 89% | 0.166 | 0.951 | 0.834 / 0.907 | 89% | 89% | 0.91 |
| audio[plain] -> text[classification] | 9/6 | 89% | 0.109 | 0.764 | 0.678 / 0.710 | 89% | 89% | 0.93 |
| audio[plain] -> text[similarity] | 9/6 | 89% | 0.132 | 0.815 | 0.705 / 0.769 | 89% | 89% | 0.93 |
| audio[plain] -> text[document] | 9/6 | 89% | 0.139 | 0.879 | 0.749 / 0.845 | 89% | 89% | 0.91 |
| audio[plain] -> audio few-shot (voice A) | 9/6 | 89% | 0.084 | 0.883 | 0.842 / 0.888 | 44% | 56% | 0.70 |
| audio[classification] -> text[none] | 9/6 | 89% | 0.150 | 0.872 | 0.737 / 0.815 | 89% | 89% | 0.94 |
| audio[classification] -> text[classification] | 9/6 | 89% | 0.117 | 0.886 | 0.784 / 0.834 | 89% | 89% | 0.91 |
| audio[classification] -> text[similarity] | 9/6 | 89% | 0.117 | 0.899 | 0.790 / 0.847 | 89% | 89% | 0.93 |
| audio[classification] -> text[document] | 9/6 | 89% | 0.131 | 0.924 | 0.782 / 0.864 | 89% | 89% | 0.94 |
| audio[classification] -> audio few-shot (voice A) | 9/6 | 89% | 0.070 | 0.889 | 0.846 / 0.878 | 56% | 56% | 0.72 |
| ASR -> text[none] | 9/6 | 89% | 0.155 | 0.931 | 0.795 / 0.868 | 89% | 89% | 0.89 |
| ASR -> text[classification] | 9/6 | 89% | 0.149 | 0.923 | 0.784 / 0.839 | 89% | 89% | 0.89 |
| ASR -> text[similarity] | 9/6 | 89% | 0.122 | 0.983 | 0.856 / 0.938 | 89% | 89% | 0.94 |
| ASR -> text[document] | 9/6 | 89% | 0.133 | 0.950 | 0.804 / 0.908 | 89% | 89% | 0.89 |

ASR exact match (Kokoro am_adam): commands caught 78% (7/9), false triggers 0/6

### Combined: transcript first, audio embedding as second opinion

Exact match on the transcript against the ears config phrase lists (plus the exemplars above); on a miss, accept the class only if audio[plain]->text[document] and ASR->text[similarity] agree and the audio score clears a threshold set at 0 false triggers on the espeak non-commands (in-sample for B+C; Kokoro is held out).

Agreement among non-commands (B+C): 15; audio threshold 0.826

- espeak B+C: exact(config) caught 20/36, FP 0/28; + audio/ASR-embedding agreement rescues 4 more, FP 0/28
- Kokoro: exact(config) caught 8/9, FP 0/6; + audio/ASR-embedding agreement rescues 0 more, FP 1/6

### Transcripts that missed (exact match on exemplars only)

- B `scratch that` -> `Scratch mat.` (None)
- B `ears off` -> `Airsoft.` (None)
- B `fuck off clanker` -> `Fuck off, Lanker.` (None)
- B `hey clanker` -> `Hey, Klanker.` (None)
- B `ears on` -> `Airman.` (None)
- B `workspace three` -> `Workplace free.` (None)
- B `scroll down a bit` -> `Scroll down a bit.` (None)
- C `enter` -> `` (None)
- C `over` -> `Oh man.` (None)
- C `back three` -> `Step three.` (None)
- C `ears off` -> `Years off.` (None)
- C `fuck off clanker` -> `Fuck off, Blenko.` (None)
- C `hey clanker` -> `Fake anchor.` (None)
- C `ears on` -> `Your mind.` (None)
- C `erase` -> `I mean.` (None)
- C `workspace three` -> `Workspace free.` (None)
- C `scroll down a bit` -> `Scroll down a bit.` (None)
- K `hey clanker` -> `Hey, Klanker!` (None)
- K `erase` -> `A race.` (None)

### Per-clip detail: audio[plain] -> text[classification], espeak B+C + Kokoro

- B ok    `enter` -> enter 0.752 (margin 0.085)
- B ok    `over` -> enter 0.712 (margin 0.050)
- B WRONG `new line` -> go_back 0.612 (margin 0.001)
- B ok    `scratch that` -> undo 0.728 (margin 0.097)
- B ok    `back three` -> back_three 0.682 (margin 0.018)
- B ok    `ears off` -> sleep 0.689 (margin 0.011)
- B ok    `fuck off clanker` -> sleep 0.788 (margin 0.072)
- B ok    `hey clanker` -> wake 0.746 (margin 0.048)
- B ok    `ears on` -> wake 0.674 (margin 0.021)
- B ok    `clear` -> clear 0.692 (margin 0.032)
- B WRONG `erase` -> wake 0.651 (margin 0.001)
- B ok    `go back` -> go_back 0.751 (margin 0.057)
- B ok    `scroll down` -> scroll_down 0.762 (margin 0.113)
- B ok    `menu` -> menu 0.720 (margin 0.088)
- B ok    `search bar` -> search_bar 0.745 (margin 0.115)
- B ok    `workspace three` -> workspace_three 0.754 (margin 0.103)
- B ok    `scroll down a bit` -> scroll_down 0.731 (margin 0.068)
- B ok    `switch to workspace 3` -> workspace_three 0.752 (margin 0.115)
- B NON   `okay` -> enter 0.663 (margin 0.012)
- B NON   `yeah` -> wake 0.658 (margin 0.003)
- B NON   `over there` -> go_back 0.630 (margin 0.002)
- B NON   `enter the room` -> enter 0.726 (margin 0.065)
- B NON   `hello world` -> wake 0.647 (margin 0.046)
- B NON   `I think so` -> wake 0.642 (margin 0.011)
- B NON   `go ahead` -> sleep 0.664 (margin 0.005)
- B NON   `clear skies today` -> clear 0.664 (margin 0.042)
- B NON   `back then` -> enter 0.651 (margin 0.002)
- B NON   `the menu was great` -> menu 0.737 (margin 0.078)
- B NON   `three more` -> back_three 0.646 (margin 0.006)
- B NON   `thanks a lot` -> wake 0.646 (margin 0.005)
- B NON   `let me check` -> wake 0.645 (margin 0.004)
- B NON   `no worries` -> enter 0.638 (margin 0.008)
- C ok    `enter` -> enter 0.693 (margin 0.036)
- C ok    `over` -> enter 0.674 (margin 0.014)
- C ok    `new line` -> new_line 0.722 (margin 0.095)
- C ok    `scratch that` -> undo 0.698 (margin 0.074)
- C ok    `back three` -> back_three 0.688 (margin 0.029)
- C ok    `ears off` -> sleep 0.693 (margin 0.019)
- C ok    `fuck off clanker` -> sleep 0.748 (margin 0.044)
- C ok    `hey clanker` -> wake 0.687 (margin 0.017)
- C ok    `ears on` -> wake 0.651 (margin 0.009)
- C ok    `clear` -> clear 0.686 (margin 0.009)
- C WRONG `erase` -> sleep 0.654 (margin 0.002)
- C WRONG `go back` -> sleep 0.641 (margin 0.006)
- C ok    `scroll down` -> scroll_down 0.654 (margin 0.015)
- C ok    `menu` -> menu 0.721 (margin 0.084)
- C ok    `search bar` -> search_bar 0.676 (margin 0.030)
- C ok    `workspace three` -> workspace_three 0.675 (margin 0.011)
- C WRONG `scroll down a bit` -> undo 0.641 (margin 0.006)
- C ok    `switch to workspace 3` -> workspace_three 0.773 (margin 0.111)
- C NON   `okay` -> enter 0.665 (margin 0.013)
- C NON   `yeah` -> wake 0.643 (margin 0.002)
- C NON   `over there` -> enter 0.641 (margin 0.003)
- C NON   `enter the room` -> enter 0.709 (margin 0.053)
- C NON   `hello world` -> back_three 0.626 (margin 0.021)
- C NON   `I think so` -> wake 0.644 (margin 0.000)
- C NON   `go ahead` -> enter 0.615 (margin 0.008)
- C NON   `clear skies today` -> clear 0.691 (margin 0.031)
- C NON   `back then` -> sleep 0.629 (margin 0.003)
- C NON   `the menu was great` -> menu 0.689 (margin 0.039)
- C NON   `three more` -> back_three 0.666 (margin 0.016)
- C NON   `thanks a lot` -> wake 0.647 (margin 0.007)
- C NON   `let me check` -> enter 0.644 (margin 0.004)
- C NON   `no worries` -> sleep 0.647 (margin 0.004)
- K ok    `enter` -> enter 0.782 (margin 0.128)
- K ok    `over` -> enter 0.755 (margin 0.114)
- K ok    `ears off` -> sleep 0.796 (margin 0.049)
- K ok    `hey clanker` -> wake 0.785 (margin 0.080)
- K ok    `scratch that` -> undo 0.762 (margin 0.126)
- K WRONG `erase` -> wake 0.656 (margin 0.003)
- K ok    `scroll down` -> scroll_down 0.776 (margin 0.104)
- K ok    `workspace three` -> workspace_three 0.764 (margin 0.138)
- K ok    `back three` -> back_three 0.749 (margin 0.083)
- K NON   `okay` -> enter 0.664 (margin 0.023)
- K NON   `over there` -> enter 0.710 (margin 0.078)
- K NON   `enter the room` -> enter 0.709 (margin 0.055)
- K NON   `I think so` -> wake 0.628 (margin 0.002)
- K NON   `go ahead` -> go_back 0.691 (margin 0.001)
- K NON   `yeah` -> wake 0.646 (margin 0.003)

### Latency (warm, sequential, seconds)

- text: n=148 median 5 ms, p90 7 ms
- audio: n=111 median 13 ms, p90 16 ms
- asr: n=79 median 128 ms, p90 246 ms

## Reading the numbers

1. **Audio -> text exemplars works as a classifier but not as a detector.** Top-1 among commands
   is 78-89% on espeak and 8/9 on Kokoro, with no ASR at all. But scores are compressed (commands
   ~0.69 vs non-commands ~0.65 with the classification prefix; median margin to the runner-up
   0.05) and the embedding is *semantic*: "enter the room" -> enter (0.71-0.73), "the menu was
   great" -> menu, "clear skies today" -> clear sit above many true commands. At zero false
   triggers only 19-56% of commands survive on espeak. Kokoro looks good (89% @0 FP) but has
   just 6 non-commands, and the dangerous ones there ("over there", "enter the room") still
   land on `enter` at 0.71.
2. **Few-shot audio exemplars** reach 94-97% top-1 on espeak, but that is espeak matching espeak.
   Against the Kokoro voice the same bank drops to AUC 0.70 and 44-56% @0 FP: audio-to-audio
   similarity is dominated by the speaker/synth, so exemplars must come from the user's own voice.
3. **Transcript route is better on the thing that matters.** ASR -> text embedding has wider
   margins (0.11-0.16) and better recall at zero false triggers (61-64% espeak). Exact match on
   the transcript never false-triggered (0/28, 0/6); its misses are ASR errors on robotic espeak
   ("Airsoft", "Workplace free", "Fake anchor"), which the user's config already patches with
   alias phrases. On Kokoro, exact match against the config lists caught 8/9.
4. **As a second opinion** (exact match first; on a miss, accept only if the audio route and the
   ASR-embedding route agree above a threshold): +4/36 rescued on espeak with no false triggers,
   but that threshold was fitted on the same set. On the held-out Kokoro voice it rescued nothing
   and added one false trigger out of 6.
5. **Latency is excellent**: warm audio embedding ~13 ms per clip (text ~5 ms) vs ~130 ms median
   for Qwen3-ASR. That is the main argument for it.

## Verdict

Not worth building as the command router, alone or as a second opinion, with these embeddings
as they come. It can tell *which* command a known-to-be-command clip is, quickly, but it cannot
tell a command from ordinary dictation that shares its words, and that false-trigger rejection
is the whole job for ears (an `enter` or `ears off` fired in the middle of a sentence is
expensive). The cheap, zero-false-trigger path is still transcript exact match plus alias
phrases. If it is revisited, the promising shape is a different one: a tiny trained head
(logistic regression over audio embeddings) on the **user's own** recorded command and
non-command clips, used as a ~13 ms pre-ASR gate for single-word commands, and judged on real
voice rather than TTS.
