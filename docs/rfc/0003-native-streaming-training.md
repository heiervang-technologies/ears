# RFC 0003: Training Qwen3-ASR for native streaming

Status: proposal, 2026-11-19. Nothing here is implemented or measured yet.
Author seat: mei (Claude instance). Companion to the speaker-adaptation SFT
on `mei/qwen3-asr-training` (heiervang-technologies `ml-experiments`), which
follows Qwen's official SFT recipe and is not replaced by this.

## Goal

Keep what makes ears good today, partials that track speech within a
fraction of a second and a frozen/revisable split the user can read, while
moving the streaming behaviour out of decoding tricks and into the model.
Concretely: an append-only decode where each step costs roughly the same no
matter how long the utterance is, and where the committed text does not have
to be guessed back from a model that was trained on whole clips.

The bet, in one sentence: **fine-tune Qwen3-ASR-1.7B on time-aligned,
interleaved audio-chunk / text sequences with a fixed text delay and a
left-context-only audio encoder, and keep today's speculative tail decode for
the open chunk so responsiveness does not drop.**

## How streaming works today

Everything is in `vllm-plugin/` and `src/continuous.rs`; see
[STREAM_PROTOCOL.md](../STREAM_PROTOCOL.md) and
[TOKEN_FREEZE.md](../TOKEN_FREEZE.md). The checkpoint itself is the stock
offline model.

- Audio is split into 8 s items matching the encoder's attention window
  (`n_window_infer = 800` mel frames). Closed items are identical between
  decodes, so vLLM's encoder and prefix caches serve them. The open item is
  re-encoded every step.
- Every decode re-sends the whole utterance: system turn, all audio
  placeholders, then the language header and the settled text forced as the
  start of the assistant turn. The model generates only the continuation.
- Settling is a policy: all but the last `rollback_words` (default 3) words of
  the previous hypothesis are forced next time. This is LocalAgreement-style
  streaming on top of an offline model.
- A decode starts whenever `min_step_ms` (default 150) of new audio has
  arrived and the previous decode has finished.

What this costs, and why training can help:

1. **The model has never seen a truncated clip.** At a cut mid-word it was
   trained to produce a complete, fluent transcript, so it guesses the tail.
   The 3-word rollback exists to absorb those guesses. It is churn the user
   sees, and it is why a word is not frozen until three more words follow it.
2. **Forced prefixes are off-distribution.** The model was not trained to
   continue a prefix it did not choose (only its own full-clip output). It
   mostly works; when it does not, the failure is a repeated or dropped phrase
   at the seam.
3. **Per-step cost grows within each 8 s window.** Everything after the last
   closed window (open-window audio tokens, settled text) is prefilled again
   each step. Bounded, but it is wasted work, and it is what limits how small
   `min_step_ms` can be on a shared GPU.
4. **Freezing is decided by a word count, not by evidence.** A word that was
   clearly finished 2 s ago and a word that ended 20 ms ago are treated the
   same way relative to the words after them.

## Alternatives considered

| Option | Verdict |
|---|---|
| Official SFT on more data (current branch) | Improves accuracy on Markus's voice; does not change the train/inference mismatch. Keep it, orthogonal. |
| Truncation-robust SFT only (stage 1 below) | Cheap, fits the current plugin unchanged, fixes cost 1 and part of 2. Not append-only. Worth doing as a stepping stone. |
| CTC or transducer head on the encoder | Natively streaming and fast, but throws away the LLM decoder's language modelling, context biasing and grammar constraints that ears relies on (bash mode, `context`). Rejected. |
| Separate small streaming model (Moonshine-style) for partials, Qwen for finals | Already explored in `moonshine-stream/`. Two models disagree at the seam; the frozen prefix would not be Qwen's. Not the bet. |
| **Interleaved delayed-text SFT (stage 2)** | Same model, same tokenizer, same prompt features, append-only decode. The bet. |
| RL on latency/churn rewards | Plausible later refinement once stage 2 exists. Not a first step. |

Prior art this draws on, cited from memory and not re-verified for this
draft: Kyutai's delayed-streams modelling for streaming STT (text stream
delayed by a fixed lag behind audio), wait-k / simultaneous translation
policies, interleaved speech-LLM streaming work (for example Speech ReaLLM),
and the Qwen3-Omni report's description of its AuT encoder being trained
with dynamic attention windows. Whoever picks this up should read those
before committing to the format below.

## The plan, cheapest cancelling test first

### Stage 0: does the encoder already tolerate short windows? (no training)

Decode a held-out set with `n_window_infer` set to 100 and 200 (1 s and 2 s
windows) instead of 800, full clip, offline. Compare WER to the 800 baseline.

- Small loss (say under 10 % relative): the encoder side of streaming is
  close to free. Closed chunks become immutable after 1 to 2 s instead of 8 s.
- Large loss: the encoder needs fine-tuning with a chunked mask (stage 2
  includes it). That raises the compute cost, but the plan is still on.

Also check, while at it, whether the conv front end really is independent per
`2 * n_window = 100` mel frames (1 s) as the config suggests. The answer
fixes the natural chunk size for stage 2.

Cost: a few GPU-minutes when the workstation GPU is free, or any rented card.
Do not evict live services for it.

### Stage 1: truncation-robust SFT (fits the current plugin)

Training example = one clip cut at a random time `t`:

- Audio: `audio[0:t]`, laid out exactly as the plugin does (8 s items, tail
  shorter than 100 ms dropped).
- Forced prefix (masked from the loss): a random prefix of the reference words,
  ending at a word boundary, mimicking what the plugin forces.
- Target: the reference words whose aligned end time is at most `t - delta`,
  after the forced prefix, then end of turn. `delta` around 200 to 300 ms.
  Words that are still in progress are deliberately absent.

Labels come from the model itself: the offline full-clip transcript of the
stock checkpoint (or the human-corrected text where it exists), plus word
timestamps from `Qwen3-ForcedAligner-0.6B` (already in `~/Models`). Unlabelled
audio is therefore usable. Mix in about 30 % untruncated examples so full-clip
behaviour does not regress.

Expected effect: the model stops guessing half-heard words, so
`rollback_words` can be lowered (target 1, maybe 0), and the seam failures
from forced prefixes should drop. No plugin or protocol change.

This reuses the training image on `mei/qwen3-asr-training`: its prefix-masking
preprocessor already implements the forced prefix excluded from the loss. Only
the data builder is new.

### Stage 2: interleaved, delayed-text streaming SFT (the bet)

#### Sequence format

```
<|im_start|>system\n{context}<|im_end|>\n<|im_start|>user\n
<|audio_start|> [chunk 1 audio pads] <|audio_end|> language English<asr_text> Okay so <WAIT>
<|audio_start|> [chunk 2 audio pads] <|audio_end|> here is <WAIT>
<|audio_start|> [chunk 3 audio pads] <|audio_end|> <WAIT>
...
<|audio_start|> [last chunk] <|audio_end|> the plan. <|im_end|>
```

The exact placement of the turn markers is a detail to settle against the chat
template. What matters:

- Chunk size `C` comes from stage 0: 1 s if the conv front end is 1 s local,
  otherwise the smallest size the front end allows.
- After chunk `k`, the target text is every word whose aligned end time is at
  most `k*C - delta` and that has not been emitted yet, followed by `<WAIT>`.
  `delta` is a fixed lag. Train a sweep (0, 160, 320 ms) and choose on the
  latency/WER curve, rather than guessing.
- `<WAIT>` is one existing unused special token from the Qwen tokenizer, so
  the vocabulary and embedding sizes do not change. Pick it by checking that
  the tokenizer reserves it and that the stock model never emits it.
- The language header is emitted once, after the first chunk where the model
  is confident, or forced when `start` carries a language, as today.
- Loss on text and `<WAIT>` tokens only; audio and prompt are masked.
  `<WAIT>` tokens far outnumber words at small `C`, so weight them down or the
  model learns to wait.

#### Encoder

Attention mask: each chunk attends to itself and up to `L` chunks to its left
(start with left context equal to today's 8 s), never to the right. A closed
chunk's encoder output then depends only on audio that will never change: it
is immutable and cacheable as soon as it closes, not when an 8 s window does.

If stage 0 showed little loss at `C`, fine-tune the encoder lightly (LoRA or
top layers) with this mask. If it showed a large loss, unfreeze more of it.
Freezing the encoder entirely is the first ablation to run.

#### Decoder

LoRA on the thinker LLM (28 layers, hidden 2048), rank 32 to 64 to start.
Full SFT is the fallback if LoRA plateaus. The decoder is already causal, so
the only new thing it learns is the interleaved timing.

Positions: Qwen3-ASR uses interleaved MRoPE. For audio-only input the three
sections should collapse to sequential positions. Verify this in the model
code before training. If audio positions are not sequential, interleaving
text between audio items shifts them and the format above needs adjusting.

#### Responsiveness: keep the speculative tail

Committed text only advances when a chunk closes, once per `C`. At `C = 1 s`
that is slower than today's 150 ms cadence, and ears' feel would regress.
So keep two paths:

- **Committed path:** append-only. When a chunk closes, append its audio and
  decode until `<WAIT>`. Everything emitted is frozen. With vLLM prefix
  caching this is one request whose prompt is the previous prompt plus
  generated tokens plus one chunk: the prefix hits the cache, prefill is one
  chunk, decode is a few tokens. Cost per step is constant.
- **Speculative tail:** every `min_step_ms`, as today, decode once more with
  the open partial chunk appended, without caching it. Its output is the
  revisable ghost text. Train for this: in some examples the final chunk is
  cut short at a random point, with the same `t - delta` target rule.

This maps directly onto ears' existing UI. `frozen_bytes` becomes "text the
committed path emitted", which is evidence-based (the words ended `delta`
before a closed chunk) instead of "all but 3 words". Report it as a new
stability profile, `qwen3_asr_interleaved_v1`; the wire protocol, partial and
final frames, and `stable_chars` stay as they are.

The final: on `end`, close the last chunk and decode until `<|im_end|>`.
Optionally keep `final_correction` (a full offline decode) as a quality net
while the streaming model is new; drop it once evaluation says it is not
needed.

#### Plugin work

`session.py` keeps its state machine; `protocol.py` gains the interleaved
prompt builder; `engine.py` needs the committed and speculative requests. The
open question is whether vLLM's Qwen3-ASR multimodal processor accepts many
separate `<|audio_start|>…<|audio_end|>` blocks with text between them in one
prompt, each content-hashed for the cache. The plugin already passes several
audio items inside one block, so this is probably supported, but it must be
verified on v0.28 before training starts, because if it is not, the inference
side costs a vLLM patch.

## Data

- **Sources:** ears' utterance journal and the 6,300-clip export (personal
  data: same handling rules as the speaker SFT branch), plus public read and
  conversational English and Norwegian corpora with licences that allow
  training. Code-switching and command phrases matter for ears and must be
  represented, not just LibriSpeech-style reading.
- **Labels:** stock Qwen3-ASR full-clip transcript (self-distillation), human
  corrections where they exist, word timestamps from Qwen3-ForcedAligner.
- **Timestamp audit:** hand-check aligner output on 50 clips of Markus's
  speech before trusting it. Bad end times move words to the wrong chunk, and
  the model then learns to emit early (hallucinate) or late (lag).
- **Splits:** session-separated, with deduplication by audio content, reserved
  before any training set is built, as the speaker SFT README already requires.

## Evaluation

Every number is computed by a script from the outputs, against the stock
checkpoint running today's plugin as the control arm on the same clips.

| Metric | Meaning | Bar to ship |
|---|---|---|
| Final WER | Accuracy after `end` | No worse than control (within noise) |
| Committed WER | WER of frozen text alone | Close to final WER; frozen text must rarely be wrong |
| Word emission latency | Time from the aligned word end to that word being frozen | Clearly better than control's "3 words later" |
| Ghost latency | Time from word end to it first appearing as ghost | No worse than control |
| Churn | Characters of ghost text later revised, per word | Lower than control |
| GPU time per audio second | Cost on the shared card | Lower than control at the same cadence |
| Command phrases | `cs` probe set and bash-grammar utterances | No regression |

Kill criteria: if stage 2's committed WER cannot get within about 10 %
relative of final WER at any lag in the sweep, the committed path is not
trustworthy; fall back to stage 1 and stop.

## Compute

Unmeasured. The plan is shaped to fit the 2 x 3090 Vast setup already being
prepared: LoRA on a 1.7B decoder with a partly frozen 24-layer encoder should
fit a 24 GB card per process at short sequence lengths, but measure it with the
two-step smoke test the training image already has before renting long. Stage 0
needs inference only.

## Risks and open questions

1. Does the encoder lose too much at 1 to 2 s left-only context? Stage 0
   answers it.
2. vLLM support for interleaved audio blocks, and their cache hashing (see
   Plugin work).
3. MRoPE positions with interleaved text (see Decoder).
4. Aligner timestamp quality on Markus's speech, Norwegian, and code-switching.
5. `<WAIT>` imbalance making the model lazy at small chunk sizes.
6. Language detection now has to happen inside the stream, not from the first
   whole-context decode. The current rule (pin after 2 s and two agreeing
   decodes) may need to become a trained behaviour.
7. Context biasing and grammar constraints: both should carry over, since the
   system turn and the decoder are unchanged, but the grammar entry-rule logic
   in `grammar.py` must be re-checked against chunked output.

## Order of work

1. Stage 0 window test. Also check the conv front end's chunking, the MRoPE
   positions and vLLM interleaved-block support. These are all inference-only
   reading or tests.
2. Data builder: ForcedAligner timestamps plus a self-distillation labelling
   pass, and the timestamp audit.
3. Stage 1 on the existing training image. Evaluate against the control.
   Ship if it wins, even if stage 2 never lands.
4. Stage 2: lag sweep and encoder ablation (frozen, LoRA). Evaluate, then a
   plugin branch with the committed and speculative paths behind a config flag.
