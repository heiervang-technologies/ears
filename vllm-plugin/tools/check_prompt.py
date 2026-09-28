#!/usr/bin/env python3
"""Check that the plugin's per-decode prompt is token-identical to what the
per-request chat template of src/continuous.rs renders on a live server.

    python tools/check_prompt.py [http://localhost:30189]

For each case it asks `/v1/chat/completions/render` to render the Rust
client's request (template + 8 s windows + forced assistant prefix), and
`/tokenize` to tokenize `build_prompt(...)` with the server's own tokenizer
(which is what the plugin does in-process), then compares the ids with each
expanded audio placeholder collapsed back to one `<|audio_pad|>`.
Needs the server to run with --trust-request-chat-template. Stdlib only.
"""

from __future__ import annotations

import base64
import io
import json
import os
import sys
import urllib.request
import wave

sys.path.insert(0, os.path.join(os.path.dirname(__file__), ".."))

from ears_vllm_stream.protocol import ENCODER_WINDOW, build_prompt, sanitize_context  # noqa: E402

# Verbatim copy of CHAT_TEMPLATE in src/continuous.rs.
CHAT_TEMPLATE = "".join([
    "{%- set ns = namespace(n=0, asst='', sys='') -%}",
    "{%- for m in messages -%}",
    "{%- if m.role == 'system' -%}",
    "{%- set ns.sys = m.content if m.content is string else (m.content | map(attribute='text') | join('')) -%}",
    "{%- endif -%}",
    "{%- if m.role == 'assistant' -%}",
    "{%- set ns.asst = m.content if m.content is string else (m.content | map(attribute='text') | join('')) -%}",
    "{%- endif -%}",
    "{%- if m.role == 'user' and m.content is not string -%}",
    "{%- for c in m.content -%}",
    "{%- if c.type == 'audio' or c.type == 'input_audio' or ('audio' in c) or ('audio_url' in c) -%}",
    "{%- set ns.n = ns.n + 1 -%}",
    "{%- endif -%}",
    "{%- endfor -%}",
    "{%- endif -%}",
    "{%- endfor -%}",
    "{{- '<|im_start|>system\\n' + ns.sys + '<|im_end|>\\n<|im_start|>user\\n<|audio_start|>' -}}",
    "{{- '<|audio_pad|>' * ns.n -}}",
    "{{- '<|audio_end|><|im_end|>\\n<|im_start|>assistant\\n' + ns.asst -}}",
])

AUDIO_PAD = 151676


def wav_b64(n: int) -> str:
    buf = io.BytesIO()
    with wave.open(buf, "wb") as w:
        w.setnchannels(1)
        w.setsampwidth(2)
        w.setframerate(16000)
        w.writeframes(bytes(range(256)) * (2 * n // 256) + b"\0" * (2 * n % 256))
    return base64.b64encode(buf.getvalue()).decode()


def post(base: str, path: str, body: dict) -> dict:
    req = urllib.request.Request(base + path, data=json.dumps(body).encode(),
                                 headers={"Content-Type": "application/json"})
    key = os.environ.get("VLLM_API_KEY")
    if key:
        req.add_header("Authorization", f"Bearer {key}")
    with urllib.request.urlopen(req) as r:
        return json.loads(r.read())


def rendered_ids(base: str, model: str, context: str, n_windows: int, prefix: str) -> list[int]:
    items = [wav_b64(ENCODER_WINDOW)] * (n_windows - 1) + [wav_b64(16000)]
    messages = []
    if context.strip():
        messages.append({"role": "system", "content": context})
    messages.append({"role": "user", "content": [
        {"type": "input_audio", "input_audio": {"data": d, "format": "wav"}} for d in items]})
    messages.append({"role": "assistant", "content": prefix})
    out = post(base, "/v1/chat/completions/render", {
        "model": model, "messages": messages, "add_generation_prompt": False,
        "continue_final_message": True, "chat_template": CHAT_TEMPLATE,
        "temperature": 0.0, "max_tokens": 64})
    ids = out["token_ids"]
    spans = sorted((p["offset"], p["length"]) for p in out["features"]["mm_placeholders"]["audio"])
    collapsed, pos = [], 0
    for off, length in spans:
        collapsed += ids[pos:off] + [AUDIO_PAD]
        pos = off + length
    return collapsed + ids[pos:]


def plugin_ids(base: str, model: str, context: str, n_windows: int, prefix: str) -> list[int]:
    text = build_prompt(sanitize_context(context), n_windows, prefix)
    return post(base, "/tokenize", {"model": model, "prompt": text,
                                    "add_special_tokens": False})["tokens"]


def main() -> int:
    base = (sys.argv[1] if len(sys.argv) > 1 else "http://localhost:30189").rstrip("/")
    model = json.loads(urllib.request.urlopen(base + "/v1/models").read())["data"][0]["id"]
    cases = [
        (ctx, n, prefix)
        for ctx in ["", "vLLM, Hyprland", "Ærlig talt – ünïcödé 日本語\nline two"]
        for n in [1, 2, 3]
        for prefix in ["", "language English<asr_text>",
                       "language English<asr_text>Okay, so here is",
                       "language Norwegian<asr_text>Hei på deg,  og \"sånn\""]
    ]
    bad = 0
    for ctx, n, prefix in cases:
        a = rendered_ids(base, model, ctx, n, prefix)
        b = plugin_ids(base, model, ctx, n, prefix)
        ok = a == b
        bad += not ok
        print(f"{'OK  ' if ok else 'DIFF'} windows={n} ctx={ctx[:12]!r} prefix={prefix[-24:]!r} ({len(a)} ids)")
        if not ok:
            print("  template:", a)
            print("  plugin:  ", b)
    print(f"{len(cases) - bad}/{len(cases)} token-identical")
    return 1 if bad else 0


if __name__ == "__main__":
    sys.exit(main())
