# ASR tail latency (2026-11-15)

Question: `Transcription took` over the last 313 calls was p50 96 ms, p90
232 ms, max 965 ms, while a single short request takes about 47 ms. Where
does the tail come from?

Short answer: it is not the network, the connection or the GPU neighbours.
Two things make it:

1. **Output length.** Qwen3-ASR decodes token by token, so a request costs
   roughly 35 ms plus 4 to 10 ms per word of output. Every call over 700 ms in
   the log is a transcript of 70 to 236 words (30 to 100 s of audio), mostly
   background conversation in Norwegian or Swedish heard in dictation mode.
   This is model compute, which is the cost the work is meant to leave.
2. **A serial second request for short results.** When a transcript has at
   most 3 words, or the segment was unsure (`verify`), ears asks the model a
   second time whether the audio is speech at all (`hears_speech`, which drops
   "Okay." heard in a click). That request went out only *after* the
   transcript came back, so every short utterance (most commands) paid two
   round trips. **Fixed in this PR**: the check now goes out together with the
   transcription.

## What actually serves the ASR

`server = "http://localhost:30189/"` reaches a Kubernetes NodePort, not a
local process. That is why `ss` shows nothing listening on 127.0.0.1:30189:
kube-proxy DNATs it in nftables (`KUBE-EXT-5IFZA36PRHM5MA2Y`), and
`tailscale serve` separately exposes the tailnet address and forwards it to
localhost.

| | |
|---|---|
| Service | `default/qwen3-asr`, NodePort 30189 -> 8000 |
| Pod | `qwen3-asr-68b8cbc5c5-f2mb2` on node **rogue** (10.42.3.200) |
| Engine | `vllm/vllm-openai:v0.28.0`, `Qwen/Qwen3-ASR-1.7B`, bf16, `--gpu-memory-utilization 0.30`, `--max-model-len 2048`, CUDA graphs on (no `--enforce-eager`), CPU limit 4 |
| Path from centurion | loopback -> kube-proxy -> pod network to rogue, RTT 0.23 ms |

The RTX 3090 on centurion does **not** serve ASR. It carries
`vllm-omni-tts` (what `say` uses, via `speaker-service`) and `sam-segment`.
So TTS playback cannot slow ASR down through the GPU. On rogue the GPU is
shared with `vllm-omni-tts-rogue` (created 2026-11-13), but that pod received
no POST in the last 24 h, so it holds memory, not compute.

Logs agree. The say log (`/tmp/live-subs-say.jsonl`, from 2026-11-13 15:22
UTC) gives each line's start time; taking about 14 characters per second plus
1.5 s as its playback window, ASR calls made while `say` was talking were no
slower:

| result | during `say` | otherwise |
|---|---|---|
| 0-3 words | n=13, p50 35 ms, p90 51 ms | n=142, p50 40 ms, p90 68 ms |
| 4-20 words | n=5, p50 119 ms, p90 122 ms | n=69, p50 107 ms, p90 183 ms |

## Log analysis

Source: `/run/user/1000/ears/debug.log`, 2026-11-11 to 2026-11-15, 1230
`Transcription took` lines (no rotated `.1` yet).

By path:

| path | n | total p50 | total p90 | max |
|---|---|---|---|---|
| HTTP transcription | 1046 | 119 ms | 318 ms | 3704 ms |
| live stream final (`final_correction = false`) | 183 | 43 ms | 85 ms | 250 ms |

The last 313 calls (the numbers in the brief) are 227 HTTP and 86 live-final.

HTTP API time by transcript length (747 calls with full log context):

| words | n | p50 | p90 | p99 | max |
|---|---|---|---|---|---|
| 0-1 | 196 | 39 | 69 | 218 | 277 |
| 2-3 | 89 | 52 | 77 | 254 | 257 |
| 4-8 | 165 | 79 | 110 | 150 | 171 |
| 9-20 | 157 | 153 | 199 | 265 | 354 |
| 21+ | 140 | 338 | 764 | 1109 | 3704 |

Time between the API reply and `Transcription took` (the serial no-speech
check, plus command-mode work for commands):

| segment | n | extra p50 | extra p90 | API p50 |
|---|---|---|---|---|
| unsure (`verify`) | 270 | 45 ms | 113 ms | 47 ms |
| result <= 3 words | 299 | 67 ms | 125 ms | 43 ms |
| result > 3 words | 482 | 0 | 0 | 149 ms |

So for short utterances the check was more than half of the wait.

Correlation with idle time before a call looks strong (calls after 10 to 60 s
of quiet have p50 161 ms) but is confounded: long utterances are followed by
long gaps. Calls after more than 10 minutes idle have p50 77 ms, so there is
no cold-path penalty.

## Benchmark (measured, 2026-11-15, modest volume: about 60 requests)

espeak-ng clips resampled to 16 kHz mono PCM16, curl against
`http://localhost:30189`, idle server:

| clip | audio | words out | total (3 runs) |
|---|---|---|---|
| "Over." | 0.7 s | 1 | 83 (first), 39, 38 ms |
| "Next track please." | 1.4 s | 3 | 64, 57, 56 ms |
| 16-word sentence | 4.7 s | 16 | 146, 137, 137 ms |
| 75 words | 21.6 s | 75 | 568, 545, 545 ms |
| 225 words | 64.7 s | 225 | 953, 893, 886 ms |

- TCP connect: 0.35 to 0.5 ms; `localhost` lookup 17 us. Keep-alive reuse
  (`num_connects=0`) gave 137 vs 137 ms. reqwest already pools connections in
  the one long-lived `WhisperClient`. Nothing to gain on the connection.
- Concurrency: two 16-word requests at once took 156 and 161 ms (vs 137 alone);
  a 1-word request while a 225-word one was decoding took 48 ms. vLLM batches
  them, so ghost partials in flight do not queue the final behind them in any
  way that matters.
- Serial vs overlapped no-speech check, real request bodies (5 runs each,
  includes curl process start):

  | clip | transcription then check | both at once |
  |---|---|---|
  | "Over." | 89 to 99 ms | 53 to 63 ms |
  | "Next track please." | 132 to 150 ms | 74 to 85 ms |

## The fix in ears

`WhisperClient::checked` now sends the no-speech check concurrently with the
transcription when the result could need it: always for unsure segments, and
for any segment of at most `speech_check_overlap_ms` (new config key,
default 3000; 0 restores the old serial order). When the transcript then
comes back long, the check's answer is ignored. The verdict logic is
unchanged.

The 3000 ms threshold comes from the log: of the 334 segments whose
duration is logged, 97.7 % of those with <= 3 words are 3 s or shorter, while
18 % of the longer results are, and those send a check request that is
thrown away. vLLM batches it with the transcription: two requests at once
took 156 and 161 ms against 137 ms alone, so the waste costs a little GPU
time, not waiting time.

Measurable: every check now logs `No-speech check took X (overlapped|serial):
speech|no speech|unavailable`.

Expected effect: about 40 to 60 ms off every short command and every unsure
segment, which makes up most of the p50 to p90 spread for commands. Long
dictation is unchanged.

## Recommendations (not applied)

Live config:

- Nothing needs setting: `speech_check_overlap_ms` defaults to 3000. Set it to
  0 to compare.
- Keep `final_correction = false`: the live stream final (p50 43 ms) avoids
  re-decoding the whole utterance, which is where the 300 ms to 1 s calls come
  from.

Server side (recommend only):

- **Do not** add `--enforce-eager`. The pod comments name it as the fallback if
  VRAM gets tight. It would drop CUDA graphs and raise the per-token cost, which
  is exactly the tail.
- The long tail is the 21+ word HTTP calls in dictation mode for ambient
  conversation. If it matters, cap them on the client (drop or split segments
  above N seconds when not in dictation focus) rather than tune vLLM.
- `vllm-omni-tts-rogue` holds 0.26 of rogue's GPU and was idle for 24 h. It is
  harmless now, but if it ever takes real traffic, ASR and TTS will share one
  3090 and that would show up here. Measure again if it gets traffic.
- The ASR pod's 4-CPU limit was not seen to throttle at this volume. PyAV
  decoding of the upload is CPU work, so check
  `container_cpu_cfs_throttled_periods_total` if the short-call floor ever
  rises.
