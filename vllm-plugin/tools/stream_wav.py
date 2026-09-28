#!/usr/bin/env python3
"""Stream a 16 kHz mono PCM16 WAV over the ears stream protocol in real time
and print every server message with a timestamp.

    pip install websockets
    python tools/stream_wav.py clip.wav --url ws://localhost:30189/v1/ears/stream --language en

Timestamps are seconds since the first audio frame; `audio` is how much audio
had been sent when the message arrived, so `t - audio` is the lag behind
real time. With an API key set on the server, export VLLM_API_KEY.
"""

from __future__ import annotations

import argparse
import asyncio
import json
import os
import sys
import time
import wave


def load_pcm(path: str) -> bytes:
    with wave.open(path, "rb") as w:
        if (w.getframerate(), w.getnchannels(), w.getsampwidth()) != (16000, 1, 2):
            sys.exit(
                f"{path}: need 16 kHz mono PCM16, got {w.getframerate()} Hz, "
                f"{w.getnchannels()} ch, {8 * w.getsampwidth()} bit "
                "(convert: ffmpeg -i in.wav -ar 16000 -ac 1 -c:a pcm_s16le out.wav)"
            )
        return w.readframes(w.getnframes())


async def connect(url: str, key: str | None):
    headers = {"Authorization": f"Bearer {key}"} if key else {}
    try:
        from websockets.asyncio.client import connect as ws_connect

        return await ws_connect(url, additional_headers=headers, max_size=None)
    except ImportError:  # websockets < 13
        import websockets

        return await websockets.connect(url, extra_headers=headers, max_size=None)


async def run(args) -> int:
    pcm = load_pcm(args.wav)
    try:
        ws = await connect(args.url, os.environ.get("VLLM_API_KEY"))
    except Exception as e:  # noqa: BLE001
        # A 404 on the upgrade means the plugin is not installed.
        print(f"upgrade to {args.url} failed: {e!r}", file=sys.stderr)
        return 1
    ready = json.loads(await asyncio.wait_for(ws.recv(), 5))
    print(f"ready: {ready}")
    if ready.get("type") != "ready":
        return 1

    start = {"type": "start", "utterance": args.utterance,
             "rollback_words": args.rollback_words, "min_step_ms": args.min_step_ms}
    if args.language:
        start["language"] = args.language
    if args.context:
        start["context"] = args.context
    await ws.send(json.dumps(start))

    t0 = time.monotonic()
    sent = 0  # bytes
    chunk = int(16000 * args.chunk_ms / 1000) * 2

    async def sender():
        nonlocal sent
        while sent < len(pcm):
            part = pcm[sent : sent + chunk]
            await ws.send(part)
            sent += len(part)
            if args.speed > 0:
                due = t0 + sent / 2 / 16000 / args.speed
                await asyncio.sleep(max(0.0, due - time.monotonic()))
        await ws.send(json.dumps({"type": "end", "utterance": args.utterance}))

    send_task = asyncio.create_task(sender())
    code = 0
    try:
        while True:
            msg = json.loads(await asyncio.wait_for(ws.recv(), args.timeout))
            t = time.monotonic() - t0
            audio = sent / 2 / 16000
            kind = msg.get("type")
            if kind == "partial":
                s = msg["text"].encode()[: msg["stable_chars"]].decode(errors="replace")
                rest = msg["text"][len(s):]
                print(f"{t:7.2f}s audio {audio:6.2f}s  #{msg['seq']:<3} "
                      f"[{msg['audio_ms']:>6}ms {msg['decode_ms']:>4}ms] {s}|{rest}")
            elif kind == "final":
                print(f"{t:7.2f}s audio {audio:6.2f}s  FINAL "
                      f"[{msg['audio_ms']:>6}ms {msg['decode_ms']:>4}ms] {msg['text']}")
                break
            else:
                print(f"{t:7.2f}s audio {audio:6.2f}s  {msg}")
                if kind == "error" and msg.get("code") in ("internal", "unsupported") \
                        and send_task.done():
                    code = 1
                    break
    except asyncio.TimeoutError:
        print(f"no message for {args.timeout}s", file=sys.stderr)
        code = 1
    finally:
        send_task.cancel()
        await ws.close()
    return code


def main() -> int:
    p = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    p.add_argument("wav")
    p.add_argument("--url", default="ws://localhost:30189/v1/ears/stream")
    p.add_argument("--language", help="ISO 639-1 code, e.g. en (default: detect)")
    p.add_argument("--context", help="context-biasing text")
    p.add_argument("--rollback-words", type=int, default=3)
    p.add_argument("--min-step-ms", type=float, default=150)
    p.add_argument("--chunk-ms", type=float, default=50, help="audio per binary frame")
    p.add_argument("--speed", type=float, default=1.0, help="1 = real time, 0 = as fast as possible")
    p.add_argument("--utterance", type=int, default=1)
    p.add_argument("--timeout", type=float, default=30.0)
    return asyncio.run(run(p.parse_args()))


if __name__ == "__main__":
    sys.exit(main())
