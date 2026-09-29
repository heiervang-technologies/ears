# ears-vllm-stream

Server side of the ears stream protocol ([`docs/STREAM_PROTOCOL.md`](../docs/STREAM_PROTOCOL.md)):
a vLLM endpoint plugin that serves `GET /v1/ears/stream` (WebSocket) on the
vLLM port for live dictation with Qwen3-ASR. Built against vLLM 0.28.0
(`vllm/vllm-openai:v0.28.0`) and `Qwen/Qwen3-ASR-1.7B`.

## Layout

| Path | What |
|------|------|
| `ears_vllm_stream/protocol.py` | Pure helpers mirroring `src/continuous.rs`: prompt text, 8 s windows, 100 ms tail rule, `max_tokens`, `settled_prefix`, language names |
| `ears_vllm_stream/session.py` | Connection state machine: utterance lifecycle, settling, `min_step_ms` scheduling, cancel/supersede, caps, error codes. Talks to the engine only through `decode(windows, prefix_text, max_tokens, *, context) -> str` |
| `ears_vllm_stream/engine.py` | vLLM glue (`VllmDecoder`): one decode = one engine request |
| `ears_vllm_stream/plugin.py` | `EarsStreamPlugin` (entry point `vllm.endpoint_plugins` / `ears_stream`) and the WebSocket route |
| `tools/stream_wav.py` | Streams a 16 kHz mono WAV in real time and prints partials with timestamps |
| `tools/check_prompt.py` | Verifies the per-decode prompt is token-identical to the Rust client's chat template on a live server |

## How a decode reaches the engine

Same path as vLLM's in-tree speech-to-text and realtime handlers, nothing new:

1. The prompt text `<|im_start|>system\n{context}<|im_end|>\n<|im_start|>user\n<|audio_start|>`
   + `<|audio_pad|>` x windows + `<|audio_end|><|im_end|>\n<|im_start|>assistant\n`
   + header + settled text is tokenized with the renderer's tokenizer
   (`add_special_tokens=False`, as the chat path tokenizes a rendered template).
2. `TokensPrompt(prompt_token_ids=..., multi_modal_data={"audio": [float32 8 s windows...]})`
   goes through `parse_model_prompt` and `engine_client.renderer.render_cmpl_async`,
   which runs the Qwen3-ASR multimodal processor (placeholder expansion, 13
   tokens per second of audio, content-hashed so closed windows hit the
   processor and encoder caches).
3. `engine_client.generate(engine_input, SamplingParams(temperature=0,
   max_tokens=min(64 + 5*s, room left in max_model_len)), request_id)`.
   Cancelling the decode task aborts the engine request.

## Install into the qwen3-asr pod

The deployment runs `pip install ... && vllm serve ...` at startup. The
`vllm/vllm-openai` image has **no `git`**, so `pip install git+https://...`
fails there; install from the GitHub archive instead (same repo, branch and
subdirectory; verified to install and register the entry point):

```bash
pip install --no-cache-dir -q librosa soundfile av \
  "ears-vllm-stream @ https://github.com/heiervang-technologies/ears/archive/refs/heads/feat/stream-plugin.tar.gz#subdirectory=vllm-plugin"
```

Where `git` is available the equivalent is
`"ears-vllm-stream @ git+https://github.com/heiervang-technologies/ears@feat/stream-plugin#subdirectory=vllm-plugin"`.
Pin a commit (`archive/<sha>.tar.gz`) once merged, so a restart cannot pick
up an unreviewed change.

and add to the container env:

```yaml
- name: VLLM_PLUGINS
  value: ears_stream
```

Endpoint plugins load only when named in `VLLM_PLUGINS`. Note that
`VLLM_PLUGINS` is also an allowlist for `vllm.general_plugins`: the image's
`lora_filesystem_resolver` / `lora_hf_hub_resolver` stop loading. They are
unused here (no runtime LoRA); if that changes, list them too
(`ears_stream,lora_filesystem_resolver,lora_hf_hub_resolver`).

The plugin needs nothing beyond the image (fastapi, starlette, numpy ship with
vLLM) and declares no dependencies, so the install cannot disturb vLLM's
pinned stack. `--trust-request-chat-template` is not needed by the plugin (it
builds token prompts); keep it while the per-tick HTTP fallback is in use.

Startup log lines to look for: `Loaded endpoint plugin ears_stream` and
`ears stream: /v1/ears/stream on Qwen/Qwen3-ASR-1.7B, max_audio_ms=...`.

### Settings

- `max_audio_ms` defaults to `min(90 s, (max_model_len - 256) / 20 tokens per s)`:
  89.6 s at `--max-model-len 2048`. Override (downwards) with
  `EARS_STREAM_MAX_AUDIO_MS`.
- A non-Qwen3-ASR model, or the CPU-only render server, answers every
  connection with `error` code `unsupported` and closes.
- API keys: vLLM's `AuthenticationMiddleware` guards `/v1/*` for WebSocket
  upgrades too; send `Authorization: Bearer <key>`.

## Try it

```bash
python tools/stream_wav.py clip.wav --url ws://localhost:30189/v1/ears/stream --language en
#   0.62s audio   0.65s  #3   [   600ms   71ms] Okay, so|here is the
#   ...
#   9.10s audio   9.00s  FINAL [  9000ms   95ms] Okay, so here is the plan.
python tools/check_prompt.py http://localhost:30189   # 36/36 token-identical
```

`|` marks `stable_chars`: text left of it is settled for the utterance.
Add `--tokens` to display the model tokenizer's frozen/mutable spans and
8-second encoder-window state. See [token freeze tracking](../docs/TOKEN_FREEZE.md)
for the additive `stability` fields and the distinction between live freezing
and final correction.

## Develop

```bash
python -m venv .venv && .venv/bin/pip install -e '.[test]'
.venv/bin/python -m pytest
```

Tests use a fake decoder; no GPU or vLLM needed.
