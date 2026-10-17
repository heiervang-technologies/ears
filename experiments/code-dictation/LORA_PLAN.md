# Code LoRA on the Qwen3-ASR decoder: feasibility and plan

The question: could Qwen3-ASR write code directly from spoken code ("for i in
range ten" → `for i in range(10):`), so the syntax gets filled in during
decoding at no extra latency? And is a cheap ranker over candidate parses a
better first step?

Research only (2026-10-22): no training was run and nothing on the cluster was
changed. The scripts in this directory are `zero_shot_probe.py`,
`rank_probe.py` and `lora_shapes.py`.

## Summary

| Question | Answer |
|---|---|
| Can the deployed vLLM serve LoRA adapters per request? | **Yes.** vLLM 0.28.0 marks `Qwen3ASRForConditionalGeneration` as `SupportsLoRA`. It needs `--enable-lora` and a pod restart. Adapter names from PEFT map cleanly to vLLM's (verified offline). |
| Training recipe | Use the official `QwenLM/Qwen3-ASR/finetuning/qwen3_asr_sft.py` (full SFT, supports a `prompt`/context field), wrapped with PEFT LoRA r16 on the decoder only and gradient checkpointing turned on. That trains 17.4 M parameters and produces a ~33 MiB adapter. |
| VRAM | About 7 to 10 GB for 1.7B at batch 8 with clips of 10 s or less; about 4 to 6 GB for 0.6B. |
| Maintenance window | **No memory window needed on rogue**, whose 3090 has 14.7 GB free next to the live ASR. The catch is compute: training slows live ASR, so run it at night (about 2 h). Centurion has about 5 GB free and is not suitable. Titan is NotReady. |
| Zero-shot prompting | Plain decoding and code-as-context: 0/20 exact. Few-shot multi-turn prompting (3 audio→code turns): 5 to 6/20 exact and mostly code-shaped, at the same latency. Prompting alone does not write code reliably. |
| Ranker (ASR decoder scores the candidates) | **12/16 top-1** with code context, at about 50 ms per candidate and no extra model or VRAM. Do this first. |

## 1. Serving LoRA with the deployed vLLM

Evidence, all gathered read-only:

- `kubectl get deploy qwen3-asr -o yaml`: image `vllm/vllm-openai:v0.28.0`, args
  `vllm serve Qwen/Qwen3-ASR-1.7B --trust-request-chat-template --dtype bfloat16
  --gpu-memory-utilization 0.30 --max-model-len 2048` plus the `ears_stream`
  plugin.
- The pod runs on **rogue**, not centurion; the comments in the manifest still
  describe centurion's GPU. The checked-in manifest
  `~/ht/cloud/k8s/ai/qwen3-asr/qwen3-asr.yaml` says centurion and `latest`, so
  the repo and the cluster have drifted.
- Versions inside the pod: vllm 0.28.0, transformers 5.15.1, torch 2.13.0+cu130.
- `vllm/model_executor/models/qwen3_asr.py` (inside the pod):
  - `class Qwen3ASRForConditionalGeneration(nn.Module, SupportsMultiModal,
    SupportsPP, SupportsMRoPE, SupportsTranscription, SupportsLoRA)`.
  - `packed_modules_mapping`: `qkv_proj` ← q/k/v and `gate_up_proj` ← gate/up.
  - `get_mm_mapping()`: language_model = `language_model`, tower =
    `audio_tower.`. LoRA goes on the decoder by default. The tower is covered
    only with `--enable-tower-connector-lora`, which we don't need.
- `vllm/lora/utils.py::parse_fine_tuned_lora_name` strips
  `base_model.model.` and applies the model's `hf_to_vllm_mapper`.
  `lora_shapes.py` checks that PEFT's saved keys
  (`base_model.model.model.language_model.layers.N.self_attn.q_proj.lora_A.weight`)
  become `language_model.model.layers.N.self_attn.q_proj.lora_A.weight`. The
  `thinker.model.*` naming used by the qwen-asr package maps the same way.
- `vllm serve --help=LoRAConfig`: `--enable-lora`, `--max-lora-rank
  {8,16,32,64,...}` (default 16), `--max-loras` (default 1 per batch),
  `--max-cpu-loras`, `--lora-target-modules`, and `--lora-modules NAME=PATH` for
  static adapters. Adapters can also be loaded at runtime with
  `VLLM_ALLOW_RUNTIME_LORA_UPDATING=1` and `/v1/load_lora_adapter`.

Change needed (option A, one deployment):

```
vllm serve Qwen/Qwen3-ASR-1.7B ... \
  --enable-lora --max-lora-rank 16 --max-loras 2 \
  --lora-modules code-py=/root/.cache/huggingface/lora/code-py
```

The adapter goes under rogue's `~/Models/huggingface/lora/`, which is already
mounted. Requests pick it with `"model": "code-py"`; prose keeps using
`Qwen/Qwen3-ASR-1.7B`.

Costs of option A:

- A pod restart: about 2 to 3 min of no ASR (pip install at startup, then the
  readiness probe).
- Extra memory: roughly 35 MB per adapter slot plus the LoRA CUDA graphs. That
  fits inside the 1.4 GiB of KV headroom at 0.30, and rogue has 14.7 GB unused
  if more is needed.
- Latency: punica LoRA kernels typically add **about 10 to 25 % per decode
  step** at rank 16 for a model this small. Once LoRA is enabled this applies
  to *every* request, base ones included, so prose dictation (about 100 ms
  today) could gain 10 to 25 ms. This is an estimate, not a measurement: it
  needs a before/after test on a scratch deployment.
- The WS `/v1/ears/stream` plugin calls `engine_client.generate(engine_input,
  params, request_id)` (`vllm-plugin/ears_vllm_stream/engine.py:125`) without a
  `lora_request`. Streaming code dictation needs a small change to pass
  `LoRARequest` through. The per-utterance chat-completions path used by
  `dictate.py` needs no change.

Option B, merged weights as a second deployment:

- Merge the adapter into a full checkpoint (about 4.4 GB on rogue's disk,
  which has 1.6 TB free) and serve it as `qwen3-asr-code` on its own NodePort
  at about 0.30 utilisation (9 GB). Rogue can hold it.
- No restart and no tax on the prose path.
- The cost is 9 GB of VRAM per language, versus 35 MB for an adapter.

Option C: qwen-asr or transformers serving outside vLLM. It is slower and gives
nothing over B.

**Recommendation:**

1. Prototype with B, which touches nothing live, for a clean comparison.
2. Move to A if the measured LoRA tax on base requests is small (it saves VRAM
   per language).

## 2. Training recipe

### Code

- **Base:** `QwenLM/Qwen3-ASR` → `finetuning/qwen3_asr_sft.py` (fetched
  2026-10-22).
  - It is a full fine-tune with the HF `Trainer` and nothing frozen.
  - JSONL rows look like `{"audio": wav, "text": "language
    English<asr_text>…", "prompt": <optional system context>}`.
  - Labels are masked to the assistant text (`labels[i, :pl] = -100`).
  - Its README example is batch 32, grad_acc 4, lr 2e-5, 1 epoch.
- **Our changes:**
  - Wrap the model with PEFT LoRA: `r=16, alpha=32, dropout=0.05`, target
    `.*language_model.*\.(q|k|v|o|gate|up|down)_proj`. With the qwen-asr class
    the regex is `.*thinker\.model.*` instead.
  - Freeze the audio tower and projector.
  - Turn on `gradient_checkpointing` and compute logits only at label
    positions.
  - Use lr 1e-4 to 2e-4 (a LoRA rate).
- **In-house prior art:** none for ASR. Nothing in ~/ht, ~/casa or ~/worktrees
  mentions ms-swift, Qwen3-ASR training or `--enable-lora`.
  - `NbAiLab/nb-asr-beta-qwen06b-lunde05` is a full fine-tune of
    Qwen3-ASR-0.6B (no adapter_config). Its run name reads 16 GPUs, 50k steps,
    lr 1e-4, cosine. It was trained on NbAiLab's cluster and its recipe is not
    published.
  - The closest code is `~/ht/area51/ml-experiments/qwen3-tts-norwegian/train_lora.py`:
    PEFT LoRA on a Qwen3 audio model (TTS talker, q/k/v/o/gate/up/down,
    accelerate, adamw_8bit). Reuse its LoRA and accelerate scaffolding.
  - The heiertech gemma adapters (r8, all-linear, from `~/ht/trainfer`) show
    the house's adapter conventions.
- **ms-swift** is an alternative trainer. Whether it supports Qwen3-ASR is
  unverified, and the official script is simpler.

### Data

One row per spoken line:

```json
{"audio": "py/000123.flac",
 "prompt": "<preceding ≤20 lines of the file, or names in scope>",
 "text": "language English<asr_text>args = parse_args(argv)"}
```

- **Corpus.** Python lines from local repos (~/ht has plenty), deduplicated.
  Drop lines over about 80 characters and any that are mostly string literals.
  Keep up to 20 preceding lines as context (this mirrors what the editor
  sends). Use 50 to 80 % of rows with context and the rest without, so the
  adapter works either way.
- **Verbalizer.** Code → spoken words is the inverse of `spoken.py` (track B).
  Produce 2 or 3 phrasings per line (terse, "of/close", explicit "open
  paren"), so that "what goes without saying" is learned as a *distribution*,
  not one convention.
- **Audio.** TTS with varied voices: Kokoro on rogue (fast, several voices),
  Qwen3-TTS (vllm-omni-tts), espeak-ng for a little robustness. Add speed
  jitter and noise or room augmentation.
- **Real voice.** Have Markus read 300 to 500 lines (about 30 min). Half go to
  training (upsampled), half to evaluation.
- **Prose replay.** Mix in 10 to 20 % prose dictation rows with their normal
  targets, so comments and strings inside code still transcribe well.
- **Amount.** 10k to 30k rows to start. LoRA format-learning tasks usually
  saturate early; plot the eval curve at 5k, 10k and 20k.

### VRAM (RTX 3090, bf16)

| | 1.7B (2.06 B params) | 0.6B (~0.9 B) |
|---|---|---|
| Frozen weights | 4.1 GB | 1.8 GB |
| LoRA r16 + fp32 Adam + grads | 17.4 M params ≈ 0.3 GB | ≈ 0.15 GB |
| Activations, grad-ckpt, ~350 tok/sample (125 audio tok for 10 s + ≤200 context + ~30 target), batch 8 | ~1–2 GB | ~0.7–1.5 GB |
| Logits (vocab 151,936) at batch 8: full / label-only | ~3.4 GB / <0.2 GB | same |
| CUDA context, allocator slack | ~1 GB | ~1 GB |
| **Total** | **~7–10 GB** (batch 8) | **~4–6 GB** |

- Parameter counts come from the safetensors headers and `lora_shapes.py`
  (meta device): decoder 1.72 B including the 311 M embedding (tied lm_head),
  audio tower 0.32 B.
- 0.6B weights are not cached locally (only NbAiLab's Norwegian fine-tune is).
  The 1.7B model is already on rogue and centurion.

### Where and how long

- **rogue** (3090: 9.9 of 24.6 GB used by qwen3-asr plus a llama-server; 1.6 TB
  disk; 30 GB RAM) fits 1.7B training next to the live ASR **without stopping
  anything**. No Python ML environment exists there yet; use a container
  (docker is installed) or a uv venv with torch, transformers, peft and
  qwen-asr (about 6 GB on rogue's disk, not this machine's).
- **Compute contention is the real cost.** Training saturates the GPU, so live
  dictation latency would spike while it runs. Run overnight or tell Markus
  first. Nothing has to be paused.
- **centurion** has about 5 GB free next to vllm-omni-tts (Markus's voice
  output), sam-segment, embeddinggemma and mpvpaper. Do not train there.
- **titan** (2× 3090) is NotReady. It would be ideal once it is back.
- **Time:** about 1 s per step at batch 8 × 350 tokens (frozen base, so
  backward costs about 2× forward, plus the checkpoint recompute; roughly
  25 TFLOPS effective). 20k rows × 2 epochs → 5k steps → **about 1.5 to 2 h**.
  The TTS data pipeline adds about 30 min (20k × 3 s through Kokoro). Evaluation
  takes about 10 min.
- **Disk:** about 1 to 2 GB of FLAC for 20k clips, 33 MiB per adapter, and
  4.4 GB for a merged checkpoint if option B is used. All of it goes on rogue
  or on KC3 (`/run/media/me/storage/builds/ears/code-lora/`); none of it was
  created now.

### Step by step

1. Track B's verbalizer runs code → spoken. Build `train.jsonl` and `eval.jsonl`
   with held-out repos for evaluation (KC3 or rogue).
2. Synthesise the audio through Kokoro and Qwen3-TTS (live services, sequential
   and rate-limited), and record Markus's set.
3. On rogue, in a container, run `qwen3_asr_sft.py` with the PEFT wrap: 1.7B,
   r16, batch 8, lr 1e-4, 2 epochs, with evaluation every 500 steps. Run it
   overnight.
4. Evaluate offline with transformers, adapter on and off (section 5).
5. Serve via option B (merged, second deployment) and run `e2e.py` against it.
   Measure the latency.
6. If it wins over ranker plus `spoken.py`, move to option A (`--enable-lora`)
   in a maintenance window of about 5 min. Measure the base-request latency
   before and after.

## 3. Zero-shot probe (live endpoint, 2026-10-22)

`python zero_shot_probe.py <scratch>`:

- 20 espeak-ng utterances, 5 conditions, decoded sequentially at temperature 0.
- The context is a 17-line Python snippet defining the names used.
- Few-shot uses three earlier (audio, code) turns through a per-request
  multi-turn chat template (allowed by `--trust-request-chat-template`).

| Condition | Exact | Median latency |
|---|---|---|
| plain | 0/20 | 112 ms |
| ctx (code as system context) | 0/20 (**8/20 empty**: `language None`) | 83 ms |
| fewshot (3 audio→code turns) | 5/20 | 109 ms |
| fewshot + ctx | 6/20 | 98 ms |
| prefill (ctx + forced `language English<asr_text>` + the first code token) | 2/20 | 66 ms |

Examples (spoken → fewshot+ctx):

- Right:
  - "for i in range ten" → `for i in range(10):`
  - "class request handler" → `class RequestHandler:`
  - "return none" → `return None`
  - "for key comma value in data dot items" → `for key, value in data.items():`
  - "x equals len of names" → `x = len(names)`
- Code-shaped but wrong:
  - "def parse args of argv" → `def pass_args(args):`
  - "if name equals equals string main" → `if name == "main":`
  - "lines equals f dot read of close dot split lines" →
    `lines = f.read().close().splitlines`
- Not converted:
  - "count plus equals one" → `count plus equals one.`
  - "with open of path as f" → `with open on path as f.`

Findings:

- The decoder *can* write code when shown examples (in-context learning
  survives the ASR fine-tune). But it is inconsistent, mishears identifiers
  ("pass args") and drops implied syntax. That is evidence a LoRA would learn
  the mapping quickly, since the capacity is there, but prompting is not
  enough.
- **A long code context makes the model call the audio non-speech.** 8/20
  clips came back `language None<asr_text>`. With a short names-only context
  the same clips decode, and forcing the assistant prefix to `language
  English<asr_text>` removes the failure in a single request. `dictate.py`
  currently retries without context on empty output; prefilling the language
  would save that second request.

## 4. Ranker

Idea: `spoken.py` plus the ASR n-best produce several candidate lines, and one
forward pass scores each one. No generation.

- **Local model choice.** There is no code LM on this machine or the cluster.
  `llm-pool` only routes to titan-llm (down) and lithium-llm; `~/Models` has
  only `unsloth/qwen3-0.6b-unsloth-bnb-4bit` (a 4-bit general model that would
  need bitsandbytes plus about 1 GB of VRAM).
- **Better choice: the ASR decoder itself.** Its decoder is a Qwen3-1.7B
  derivative, and through vLLM `prompt_logprobs` it gives log P(candidate |
  audio, preceding code). That hears the audio as well as reading the code. No
  new model and no VRAM, because it runs on the live endpoint.

`python rank_probe.py <scratch>`: 16 clips with 3 or 4 candidates each (the
correct one plus the plausible wrong parses a rule translator emits).

| Ranker input | Top-1 | Latency |
|---|---|---|
| audio only | 7/16 | ~148 ms for ~3 candidates, sequential |
| audio + preceding code | **12/16** | ~148 ms for ~3 candidates, sequential |

- With context it fixes exactly the ambiguities that need context:
  `parse_args(argv)` over `pass_args(args)`, `range(10)` over `range(ten)`,
  `is None` over `is non`, `len(names)` over `len(name)`.
- The four misses:
  - `count + 1` over `count += 1`: the acoustics favour the dropped "equals".
  - `.close()`: it was literally spoken.
  - `name == "main"`: the context has no `__name__`.
  - `items.index(key)`: genuinely ambiguous wording.
- Latency is about 50 ms per candidate sequentially. Sent concurrently, the
  shared audio and context prefix is cached and the candidates batch, so a
  top-4 rerank should cost about 60 to 80 ms.

**Verdict: worth it, and first.** It reuses `spoken.py`'s n-best and
alternative parses, needs no training or infrastructure change, and also gives
a confidence margin (the gap between top-1 and top-2) for deciding when to show
the picker. It does not remove the need for the rules to generate the right
candidate, and that is where a LoRA would help.

## 5. Risks and evaluation

- **Catastrophic forgetting of prose.** With a per-request adapter (A) or a
  separate deployment (B), the base model is untouched, so prose is safe *as
  long as routing is right*. What remains:
  - comments and strings inside code are prose; mitigated by prose replay
    rows;
  - a misrouted prose utterance (an adapter request from the wrong window)
    comes out code-shaped;
  - under A, a small latency tax on every request.
- **"What goes without saying."** The adapter learns the verbalizer's
  conventions, not Markus's.
  - If the training data always says "colon", it won't add one when he
    doesn't.
  - Mitigations: several phrasings per line, his real recordings upsampled,
    and an eval set of his natural speech recorded *before* he reads the
    conventions.
  - Implicit syntax that is genuinely ambiguous (`items index key` → `[key]`
    or `.index(key)`) stays ambiguous. Keep the beam and picker UI.
- **Hallucinated completion.** The prefill condition already produced extra
  context lines ("class request handler" → the whole class body copied from
  the context). An adapter trained on single lines with EOS should stop this.
  Cap `max_tokens` and keep one line per utterance.
- **Identifiers.** Rare names are acoustic guesses (`pass_args`). Context
  biasing and the ranker help; the adapter alone does not fix it.
- **Evaluation (three axes):**
  1. Held-out repos, synthetic voices: exact line match, AST-normalised match
     and CER, against `spoken.py` plus the ranker on the same clips.
  2. Markus's real recordings: the same metrics. This is the one that
     decides.
  3. Regression: base prose WER with the adapter off (must be unchanged; it is
     the same weights) and on (to measure the damage from a misroute), plus
     p50 and p95 latency for both. Go/no-go: the adapter must beat ranker plus
     rules on (2) by a clear margin (more than 10 points exact match) to
     justify its upkeep.

## 6. Per-language cost

| Item | Shared | Per language |
|---|---|---|
| Base model, serving (adapter slots or the second deployment), TTS pipeline, training script, eval harness, ranker | yes, built once | — |
| Verbalizer (code → spoken, several phrasings) | framework | rules, about 1 to 3 days (the inverse of `spoken.py`'s `python_line` / `bash_line`) |
| Corpus | — | about 1 h (local repos; bash comes from dotfiles and scripts) |
| Synthetic audio | — | about 30 min of TTS, 1 to 2 GB |
| Real-voice set | — | 300 to 500 lines read by Markus, about 30 min |
| Adapter training | — | about 2 h overnight on rogue, 33 MiB adapter (or 4.4 GB checkpoint plus 9 GB VRAM under option B) |
| Evaluation | harness | about 10 min to run; 100+ held-out real utterances |

- **One adapter or several.** One multi-language adapter with the language
  shown in the context (a file name or `# python`) avoids extra slots and
  shares the "code-ness". Per-language adapters isolate regressions. Start with
  one adapter for Python and bash.
- **Ranker cost.** The ranker has no per-language model cost, only the
  candidate generator (the rules).

## Disk and safety

- Scratch: `/run/media/me/storage/builds/ears/code-dictation-lora/` (KC3 mount
  verified), about 2 MB of WAVs.
- No downloads and no package installs. The offline check used the existing
  `~/ht/cont/.venv` (torch 2.13 cpu, transformers 5.15) with peft 0.21 taken
  from `~/worktrees/.trainfer-adapters-venv` via `PYTHONPATH`.
- Kubernetes and rogue were used read-only: get, read-only `exec`
  (grep/help/import), and `nvidia-smi`.

Reproduce:

```bash
python zero_shot_probe.py /run/media/me/storage/builds/ears/code-dictation-lora/zsp
python rank_probe.py      /run/media/me/storage/builds/ears/code-dictation-lora/zsp
PYTHONPATH=$(echo ~/worktrees/.trainfer-adapters-venv/lib/*/site-packages) \
  ~/ht/cont/.venv/bin/python lora_shapes.py \
  ~/Models/huggingface/hub/models--Qwen--Qwen3-ASR-1.7B/snapshots/7278e1e70fe206f11671096ffdd38061171dd6e5/ 16
```
