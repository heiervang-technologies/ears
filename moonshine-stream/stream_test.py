"""Stream a WAV to an ears stream server at real-time pace, as ears does.

    python stream_test.py ws://127.0.0.1:30190/v1/ears/stream clip.wav [--speed 1] [--quiet]

Checks the protocol promises (settled text prefixes every later partial and
the final) and reports latency: how far behind the audio partials arrive,
and how long the final takes after `end`.
"""

import argparse
import asyncio
import json
import time

import websockets

import bench


async def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("url")
    ap.add_argument("wav")
    ap.add_argument("--speed", type=float, default=1.0)
    ap.add_argument("--frame-ms", type=int, default=50)
    ap.add_argument("--quiet", action="store_true")
    args = ap.parse_args()
    audio = bench.load(args.wav)
    pcm = (audio * 32767).clip(-32768, 32767).astype("<i2").tobytes()
    step = 16000 * args.frame_ms // 1000 * 2

    async with websockets.connect(args.url, max_size=None) as ws:
        print(json.loads(await ws.recv()))
        await ws.send(json.dumps({"type": "start", "utterance": 1}))
        t0 = time.monotonic()
        lags, stable, partials = [], "", 0
        violations = 0
        final = None

        async def reader():
            nonlocal stable, partials, violations, final
            async for raw in ws:
                m = json.loads(raw)
                now = time.monotonic() - t0
                if m["type"] == "partial":
                    partials += 1
                    text = m["text"]
                    if not text.startswith(stable):
                        violations += 1
                    stable = text.encode()[: m["stable_chars"]].decode()
                    lags.append(now * args.speed * 1000 - m["audio_ms"])
                    if not args.quiet:
                        print(f"{now:6.2f}s audio {m['audio_ms'] / 1000:5.2f}s dec {m['decode_ms']:4d}ms "
                              f"| {stable}\033[2m{text[len(stable):]}\033[0m")
                elif m["type"] == "final":
                    if not m["text"].startswith(stable):
                        violations += 1
                    final = (m, time.monotonic())
                    return
                else:
                    print(m)

        task = asyncio.create_task(reader())
        for i in range(0, len(pcm), step):
            await ws.send(pcm[i:i + step])
            target = t0 + (i + step) / 2 / 16000 / args.speed
            await asyncio.sleep(max(0, target - time.monotonic()))
        t_end = time.monotonic()
        await ws.send(json.dumps({"type": "end", "utterance": 1}))
        await task
        m, t_final = final
        lags.sort()
        print(f"\npartials {partials}, lag behind audio: median {lags[len(lags) // 2]:.0f} ms, "
              f"p90 {lags[int(len(lags) * 0.9)]:.0f} ms; final {(t_final - t_end) * 1000:.0f} ms "
              f"after end (decode {m['decode_ms']} ms); prefix violations {violations}")
        print("final:", m["text"])


asyncio.run(main())
