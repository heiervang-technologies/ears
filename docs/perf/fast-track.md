# Fast track: what money buys in latency (2026-11-15)

Question (Markus): if we spend money on faster GPUs, running the **same**
custom stack (Qwen3-ASR-1.7B on vLLM with the ears stream plugin), how much
faster does ears feel? Is a rented vast.ai GPU worth it?

Short answer: **not for latency.** With the stream plugin the model work per
utterance is already 30 to 50 ms on rogue's RTX 3090. A datacentre GPU can save
roughly half of that, and the nearest vast hosts cost 14 to 27 ms of ping, paid
on every partial and on the final. The remaining delay is in endpointing
(silence wait) and the command accept hold, which no GPU changes.

## Measured: the current stack on rogue (RTX 3090)

`tools/bench/bench_stream.py` replays 16 kHz PCM clips in real time (20 ms
chunks) over the stream protocol and records each partial's arrival, the
server's `decode_ms`, and the final's round trip after `end` (what the user
waits for once the silence wait is over). espeak-ng clips, 3 runs each, from
centurion to `ws://localhost:30189/v1/ears/stream` (k8s NodePort to rogue,
RTT 0.23 ms).

| clip | audio | step | final round trip p50 | partial decode p50 / max | partials |
|---|---|---|---|---|---|
| "Next track please." | 1.6 s | 150 ms | 52 ms | 50 / 58 ms | 2 |
| 12-word question | 3.8 s | 150 ms | 31 ms | 35 / 58 ms | 15 |
| 56-word paragraph | 19.4 s | 150 ms | 32 ms | 36 / 64 ms | 80 |
| "Next track please." | 1.6 s | 50 ms | 52 ms | 35 / 58 ms | 5 |
| 12-word question | 3.8 s | 50 ms | 44 ms | 34 / 58 ms | 27 |
| 56-word paragraph | 19.4 s | 50 ms | 39 ms | 43 / 68 ms | 119 |

(First run of each clip is 5 to 30 ms slower; p50 is over the 3 runs.)

What this means:
- **The final costs 30 to 50 ms**, and it does not grow with utterance length:
  the stream has already decoded everything but the last few words.
- **A live update lands 35 to 45 ms after its audio**, plus on average half a
  step of waiting for the step to fill.
- **`live_min_step_ms = 50` already works on the 3090**: decodes take less
  than the step, so live text refreshes about every 50 ms instead of every
  150 ms, at the cost of more GPU time (119 vs 80 decodes for the paragraph,
  decode p50 43 vs 36 ms). Free to try.

## Expected on faster GPUs (estimate, not measured)

Decode at batch 1 is bound by memory bandwidth plus fixed per-step overhead
(vLLM scheduling, Python, the encoder pass). Assume about a third of the
current 35 ms is fixed overhead that does not shrink with hardware.

| GPU | memory bandwidth | est. final / partial decode | vast price seen (1 GPU, on-demand) | nearest seen host |
|---|---|---|---|---|
| RTX 3090 (rogue, now) | 936 GB/s | 30-50 ms (measured) | owned | 0.2 ms |
| RTX 4090 | 1008 GB/s | about the same | $0.34-0.54/h | DE, FI |
| RTX 5090 | 1792 GB/s | ~20-35 ms | $0.21-0.80/h | NL, CZ |
| RTX PRO 6000 | 1792 GB/s | ~20-35 ms | $1.0-2.7/h | DE, CZ, FR |
| H100 SXM | 3350 GB/s | ~15-25 ms | $1.87-3.82/h | DE ($2.27), CZ |

Prices: public vast.ai offer search, 2026-11-15, a 64-offer sample; they move
daily.

## Measured: network distance from centurion

| endpoint | RTT |
|---|---|
| rogue (current ASR) | 0.23 ms |
| Helsinki (Hetzner) | 14 ms |
| Amsterdam / Frankfurt (Leaseweb) | 22 ms |
| Falkenstein / Nuremberg (Hetzner) | 27 ms |

A remote server adds one full RTT to every partial (audio travels there,
text travels back) and to the final.

## The arithmetic

`remote is faster  ⇔  local_compute − remote_compute > RTT`

- Final: 30-50 ms local vs ~15-25 ms on an H100, saving ~15-25 ms; RTT 14-27 ms.
  **A wash at best.**
- Partials: same saving, same RTT. **A wash.**
- Long HTTP transcriptions (`final_correction = true`, or the HTTP fallback)
  would gain more (decode is ~5 ms/word on the 3090), but the stream path
  makes those rare.

## Where the remaining latency is (after #287, #288, #289)

Live log after installing 4c33b81, dictation:
`speech end → VAD notices → 350 ms early finish (settled) → final 30-73 ms → ghost commit`.
Command mode: `400 ms command silence (150 ms on exact phrases) → match → 200 ms accept hold → action`.

| cost | size | can money fix it? |
|---|---|---|
| silence wait, dictation | 350 ms (700/1500 fallback) | no; smarter endpointing does |
| silence wait, commands | 150-400 ms | no |
| accept hold before a command fires | 200 ms | no; part B moves it after the action |
| final after the end | 30-50 ms | partly (~15-25 ms), but ping takes it back; `final_from_partial = "settled"` removes it for free |
| live text refresh | 150 ms step + 35-45 ms | `live_min_step_ms = 50` on the current GPU, free |

## What money could buy instead

- **A local RTX 5090 in rogue** (no ping): roughly 10-20 ms off every final and
  partial. Small; not the bottleneck today.
- **Headroom for an end-of-turn model** (e.g. an audio smart-turn model, run at
  each short pause) or for a larger ASR model. That is where compute would
  turn into perceived speed, by letting the silence wait drop safely below
  350 ms. CPU may already be enough for a small turn model.

## How to verify on a rented GPU (if wanted; about $2-3)

1. Rent one H100 SXM (or RTX 5090) in DE/NL/FI for one hour.
2. Run the same image as the `qwen3-asr` pod (`vllm/vllm-openai:v0.28.0`,
   `Qwen/Qwen3-ASR-1.7B`, bf16, `--max-model-len 2048`, CUDA graphs on) with
   the `ears_vllm_stream` plugin installed and an API key set, with port 8000
   exposed.
3. Run `tools/bench/bench_stream.py ws://HOST:PORT/v1/ears/stream clips/*.pcm`
   at steps 150 and 50, and compare the final round trip and partial lag with
   the table above. Ping the host first and subtract.
