# Ears stream protocol (v1)

A persistent WebSocket between ears and a vLLM server running Qwen3-ASR, for
live dictation (issue #144). ears sends only new audio; the server keeps each
utterance's state and decodes whenever enough new audio has arrived, pushing
the growing transcript back.

It replaces the per-tick HTTP requests of `live_decoding = "continuous"`
(`src/continuous.rs`), which re-send all audio so far every 300 ms. The
decoding itself is the same:

- **Audio as encoder windows.** Qwen3-ASR's audio encoder attends within
  fixed 8 s windows (`n_window_infer` 800 mel frames). The utterance is
  decoded as one `<|audio_start|>…<|audio_end|>` block whose `<|audio_pad|>`
  placeholders are filled by 8 s items in order, so a closed window is
  identical from decode to decode and vLLM's encoder and prefix caches serve
  it. Measured on a 33 s clip against one audio item: mean KL 0.0005 over the
  transcript tokens, identical argmax.
- **Settled text as a forced prefix.** Everything but the last
  `rollback_words` words of the previous hypothesis is forced as the start of
  the assistant turn; the model decodes only the continuation.

The server side is a vLLM endpoint plugin (`vllm-plugin/`, entry point group
`vllm.endpoint_plugins`, name `ears_stream`, enabled with
`VLLM_PLUGINS=ears_stream`).

## Endpoint

`GET /v1/ears/stream` upgraded to WebSocket, on the vLLM server's port. If the
server has an API key, the client sends `Authorization: Bearer <key>` on the
upgrade request. A 404 on the upgrade means the plugin is not installed.

## Frames

Text frames carry one JSON object each. Binary frames carry audio.

### Server → client

```json
{"type": "ready", "protocol": 1, "model": "Qwen/Qwen3-ASR-1.7B",
 "sample_rate": 16000, "max_audio_ms": 90000}
```
Sent once, first, after the upgrade. `max_audio_ms` is the longest utterance
the server will decode (bounded by the model's context).

```json
{"type": "partial", "utterance": 7, "seq": 12, "text": "Okay, so here is the",
 "stable_chars": 13, "audio_ms": 2700, "decode_ms": 64}
```
The full hypothesis for the utterance so far (not a delta). `text[..stable_chars]`
(byte offset, UTF-8) is settled and will prefix every later partial and the
final of this utterance; the rest may still change. `seq` increases per
utterance. `audio_ms` is the audio the hypothesis covers.

```json
{"type": "final", "utterance": 7, "text": "Okay, so here is the plan.",
 "audio_ms": 3100, "decode_ms": 80}
```
Sent once per utterance after `end`. The last message for that utterance.

```json
{"type": "error", "utterance": 7, "code": "too_long", "message": "..."}
```
`utterance` is null for connection-level errors. Codes:
- `bad_request`: malformed frame, audio without an active utterance, wrong
  `utterance` id. The connection stays open.
- `too_long`: the utterance passed `max_audio_ms`. No more partials for it;
  audio beyond the cap is dropped; `end` still yields a `final` over the
  first `max_audio_ms`.
- `unsupported`: the loaded model cannot do this (not Qwen3-ASR). Sent before
  closing.
- `internal`: a decode failed. The utterance continues; the next decode may
  succeed. A failed final decode is sent as `error` instead of `final`.

### Client → server

```json
{"type": "start", "utterance": 7, "language": "en", "context": "vLLM, Hyprland",
 "rollback_words": 3, "min_step_ms": 150}
```
Begins utterance `utterance` (client-chosen, increasing). All fields but
`type` and `utterance` are optional:
- `language`: ISO 639-1 code; forces `language <Name><asr_text>`. Absent or
  unknown: the model detects it.
- `context`: context-biasing text, sent as the system turn.
- `rollback_words`: default 3.
- `min_step_ms`: decode only once at least this much new audio arrived since
  the last decode (default 150).

A `start` while another utterance is active cancels the active one (no
`final` for it).

Binary frame: mono PCM16 little-endian at 16 kHz, appended to the active
utterance. Any length; an odd trailing byte is kept until the next frame.

```json
{"type": "end", "utterance": 7}
```
No more audio for this utterance. The server finishes any decode in flight,
then decodes once more with every word open and sends `final`.

```json
{"type": "cancel", "utterance": 7}
```
Drop the utterance. Nothing more is sent for it.

## Server behaviour

- One utterance per connection at a time, one decode in flight per
  connection. After a decode finishes, the next starts as soon as
  `min_step_ms` of new audio is buffered (or immediately on `end`). There is
  no timer: decoding keeps pace with the audio as the GPU allows.
- A partial is sent only if its text differs from the previous one.
- Results for a cancelled or superseded utterance are never sent.
- Bounded memory: audio per utterance is capped at `max_audio_ms`; incoming
  frames are read continuously so a slow decode never stalls the socket.
- Closing the socket cancels the active utterance and any decode in flight.
- Decode = one engine request: token prompt
  `<|im_start|>system\n{context}<|im_end|>\n<|im_start|>user\n<|audio_start|>`
  + `<|audio_pad|>` × windows + `<|audio_end|><|im_end|>\n<|im_start|>assistant\n`
  + header + settled text, `multi_modal_data={"audio": [8 s windows…]}`,
  greedy, `max_tokens = 64 + 5 × audio seconds`. A tail window shorter than
  100 ms is left out. The header (`language X<asr_text>`) is forced when the
  language is known, else learned from the first decode's output.
- Settling after each partial: `stable = text minus its last rollback_words
  words`, cut at a word start so it stays byte-identical to model output.
  The final decode settles nothing further.

## Client behaviour (ears)

`live_decoding = "continuous"` tries the stream first, then the per-tick HTTP
decoder, then repeated previews:
1. Connect; expect `ready` within 2 s. 404, a refused upgrade, or
   `unsupported` → per-tick HTTP continuous for the rest of the process.
2. Push-to-talk (`ears toggle --ghost`): the preview process tails the
   growing `recording.wav` every 50 ms and sends new samples, and after each
   partial writes the settled state (`header`, `text[..stable_chars]`) to
   `ghost-continuous.json` as the HTTP decoder does. The stop path is
   unchanged: it kills the preview and, with `final_correction = false`,
   finishes with one HTTP continuous decode forcing that settled text.
3. VAD ghost (`ears ghost`): one utterance per VAD segment; `start` at speech
   onset (with the pre-speech buffer), samples as they arrive, `end` when the
   segment completes, `cancel` when the candidate is rejected.
4. Connection lost mid-utterance: fall back to per-tick HTTP for that
   utterance; never replay audio into a second final (no duplicate commits).
