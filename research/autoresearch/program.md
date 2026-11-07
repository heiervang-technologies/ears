# ears ASR autoresearch: program

You are an autonomous research agent. Goal: make the **same** speech stack
faster and more accurate. The stack is Qwen3-ASR-1.7B served by vLLM v0.28.0
with the ears stream plugin (`vllm-plugin/`), exactly as the production pod
on rogue runs it (RTX 3090). The rig has 4 RTX 3090s, so whatever wins here
runs on rogue unchanged.

Scope (Markus, 2026-11-15): serving and decoding only. Same weights, public
data only. No fine-tuning, and no recordings of real people beyond
LibriSpeech. Budget: $25 for the rig, enforced by `rig/vast.py watch --cap 25`
on centurion; work as if the instance can vanish at any moment, so push
results often.

## Setup (once)

1. `rig/vast.py offers`, `rig/vast.py rent <id>`, `rig/vast.py watch --cap 25`
   (on centurion, in the background).
2. On the rig: clone this branch to `/workspace/ears`, copy the synthetic
   data (`/run/media/me/storage/builds/ears/autoresearch/data/`, made by
   `data/make_synth.py`) to `/workspace/data/`, then
   `bash research/autoresearch/rig/setup.sh 4` (venv per slot, model,
   LibriSpeech subsets).
3. Baseline: `rig/run_exp.sh <slot> baseline-<slot> "prod config"` on all 4
   slots at once. The spread across slots is your noise floor. Write it down.

## The fixed evaluation (do not edit `eval/eval.py` or the manifest)

`eval/eval.py` against one slot's server:
- **latency**: 12 clips replayed in real time over the stream protocol, 2
  runs. `final_rt_p50/p90` = ms from `end` to `final` (what the user waits
  for after the silence wait). `decode_p50` = server `decode_ms` of partials
  (how fresh the live text is).
- **wer**: stream path as ears uses it, at 3x real time, 4 streams at once:
  LibriSpeech test-clean and test-other (300 each, seeded), ears command
  phrases (`cmd`), code and agent dictation (`code`). Pooled is
  word-weighted.

One run takes a few minutes. Each slot runs its own experiment. Rows land in
`/workspace/results/results.tsv`.

## Keep or discard

- **Speed track**: keep if `final_rt_p50` or `decode_p50` improves by more
  than the noise floor, with pooled WER no worse than +0.2 (absolute) and no
  set worse than +0.5.
- **Accuracy track**: keep if pooled WER improves by at least 0.2 absolute, or
  `cmd`/`code` by at least 0.5, with `final_rt_p50` no worse than +5 ms.
- A kept change is committed on the slot's checkout (`git -C
  /workspace/slots/<n>/ears commit`) with the results row in the message.
  Merge winners into the other slots before stacking further experiments.
- Re-run any winner on a second slot before calling it real.

## What you may change

- `vllm serve` flags and env, through `/workspace/slots/<n>/serve.env`
  (`VLLM_EXTRA`, `ENV_EXTRA`). Keep `--max-model-len 2048` and
  `--gpu-memory-utilization 0.30`: production shares the GPU.
- The plugin: `/workspace/slots/<n>/ears/vllm-plugin/` (editable install).
- Request-side defaults the plugin applies (rollback words, step, max tokens,
  context, language handling). If a change needs the ears client to send
  something new, note it as a client change in the report.
- Quantized copies of the same weights (made on the rig), served from local
  disk.

## Ideas, roughly in order of expected value

Speed:
1. Check what vLLM already reuses between steps: prefix caching with
   multimodal hashes should reuse the KV of completed 8 s windows. Measure
   (server log, `decode_ms` vs utterance length). If complete windows are
   re-encoded every step, cache encoder outputs per window in the plugin.
2. Scheduler and compile flags for batch-1 latency: `--max-num-seqs` small
   (4 to 8), `--cuda-graph-sizes` covering 1 to 8, `--async-scheduling`,
   compilation level, `--no-enable-chunked-prefill` (or a large
   `--max-num-batched-tokens`), attention backend
   (`VLLM_ATTENTION_BACKEND`).
3. Plugin work per step: skip a decode when the new audio is silence, fewer
   rollback words (3 → 2 → 1; watch WER), a tighter `max_tokens`, avoid
   Python-side copies of the whole audio per step.
4. Quantization that Ampere runs fast: int8 weight-only, W8A8 int8, AWQ/GPTQ
   int4 with Marlin kernels, fp8 KV cache. Watch WER closely.
5. Speculative decoding only if a cheap drafter exists; the output text is
   rarely in the prompt, so n-gram lookup is unlikely to help.

Accuracy (same weights):
1. Context biasing in the system turn: a vocabulary of command and code
   terms (`cmd`, `code`) without hurting LibriSpeech.
2. Rollback words and step size (more rollback lets the model fix more).
3. The 8 s encoder window boundary: words cut at a window edge. Try
   overlapping or boundary-aware windows for the final decode.
4. Final pass: re-decode the whole utterance for the final only when it is
   short (cost is small there), compared with the stream final.
5. Audio preprocessing: gain normalization, high-pass, leading and trailing
   silence trimming.
6. Decoding parameters: repetition penalty, length limits.

## Reporting

Keep `/workspace/results/results.tsv` complete, including crashes and
discards. Every hour, and when the budget is nearly spent, copy results and
kept diffs back to centurion
(`/run/media/me/storage/builds/ears/autoresearch/results/`). End with
`report.md`: the baseline and noise floor, the winners with numbers, and the
exact changes to bring to the production pod and to the ears client.
