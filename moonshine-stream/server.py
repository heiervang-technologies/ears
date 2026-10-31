"""Moonshine Streaming ASR server for ears, CPU only.

    GET  /health                    {"status": "ok", "model": ...}
    GET  /v1/models                 the loaded model
    POST /v1/audio/transcriptions   multipart file=<wav> -> {"text": ...}
    POST /v1/chat/completions       input_audio -> "language English<asr_text>..."
                                    ("language None<asr_text>" for no speech):
                                    ears' speech check. A grammar is ignored.
    GET  /v1/ears/stream            WebSocket, ears stream protocol v1
                                    (docs/STREAM_PROTOCOL.md)

    python server.py MODEL_DIR [--host 127.0.0.1] [--port 30190] [--threads N]

Decoding is incremental (see engine.py): each update encodes only new audio,
then re-decodes with the previous hypothesis as a speculative draft. One
model call runs at a time; on a two-core laptop more would only contend.
"""

import argparse
import asyncio
import base64
import io
import json
import logging
import time
import wave
from concurrent.futures import ThreadPoolExecutor

import numpy as np
from aiohttp import WSMsgType, web

import engine

PROTOCOL = 1
SAMPLE_RATE = engine.SAMPLE_RATE
DEFAULT_ROLLBACK_WORDS = 3
DEFAULT_MIN_STEP_MS = 150
MAX_AUDIO_MS = 80_000  # the adapter's 4096 positions are 81.9 s

log = logging.getLogger("moonshine-stream")


def wav_to_16k(data: bytes) -> np.ndarray:
    with wave.open(io.BytesIO(data)) as w:
        sr, ch, width = w.getframerate(), w.getnchannels(), w.getsampwidth()
        raw = w.readframes(w.getnframes())
    if width != 2:
        raise ValueError(f"need 16-bit PCM, got {8 * width}-bit")
    a = np.frombuffer(raw, "<i2").astype(np.float32) / 32768.0
    if ch > 1:
        a = a.reshape(-1, ch).mean(axis=1)
    if sr != SAMPLE_RATE:
        n = int(len(a) * SAMPLE_RATE / sr)
        a = np.interp(np.linspace(0, len(a) - 1, n), np.arange(len(a)), a).astype(np.float32)
    return a


class Service:
    def __init__(self, model: engine.Model, name: str, settle_s: float):
        self.model = model
        self.name = name
        self.settle_s = settle_s
        self.pool = ThreadPoolExecutor(max_workers=1)

    async def call(self, fn, *args):
        return await asyncio.get_running_loop().run_in_executor(self.pool, fn, *args)

    def transcribe(self, audio: np.ndarray) -> str:
        if len(audio) < SAMPLE_RATE // 10:
            return ""
        return self.model.transcribe(audio[: MAX_AUDIO_MS * SAMPLE_RATE // 1000]).strip()


class BadRequest(Exception):
    def __init__(self, message, utterance=None):
        super().__init__(message)
        self.utterance = utterance


def _is_int(v):
    return isinstance(v, int) and not isinstance(v, bool)


class Utterance:
    def __init__(self, uid, live, rollback_words, min_step):
        self.id = uid
        self.live = live
        self.rollback_words = rollback_words
        self.min_step = min_step
        self.pcm = bytearray()
        self.odd = b""
        self.fed = 0  # samples handed to the engine
        self.last_decode_at = 0
        self.last = None  # (text, stable_chars) of the last partial
        self.seq = 0
        self.ended = False
        self.too_long = False
        self.done = False

    @property
    def n_samples(self):
        return len(self.pcm) // 2


class Session:
    """One WebSocket: at most one utterance, one decode in flight."""

    def __init__(self, service: Service, send):
        self.svc = service
        self._send = send
        self.max_samples = MAX_AUDIO_MS * SAMPLE_RATE // 1000
        self.active: Utterance | None = None
        self._last_id = None
        self._task = None
        self._stray_audio_reported = False
        self._closed = False

    async def on_text(self, raw):
        try:
            try:
                msg = json.loads(raw)
            except ValueError:
                raise BadRequest("frame is not valid JSON") from None
            if not isinstance(msg, dict):
                raise BadRequest("frame is not a JSON object")
            uid = msg.get("utterance")
            uid = uid if _is_int(uid) else None
            kind = msg.get("type")
            if kind == "start":
                self._start(msg)
            elif kind == "end":
                utt = self._addressed(msg, "end")
                if utt is not None:
                    utt.ended = True
            elif kind == "cancel":
                if self._addressed(msg, "cancel") is not None:
                    self.active = None
            else:
                raise BadRequest(f"unknown message type {kind!r}", uid)
        except BadRequest as e:
            await self._error("bad_request", str(e), e.utterance)
        self._schedule()

    async def on_binary(self, data):
        utt = self.active
        if utt is None or utt.ended:
            if not self._stray_audio_reported:
                self._stray_audio_reported = True
                what = "after end" if utt is not None else "without an active utterance"
                await self._error("bad_request", f"audio {what}", utt.id if utt else None)
            return
        buf = utt.odd + data
        utt.odd, buf = (buf[-1:], buf[:-1]) if len(buf) % 2 else (b"", buf)
        room = self.max_samples * 2 - len(utt.pcm)
        if len(buf) > room:
            utt.pcm += buf[: max(room, 0)]
            if not utt.too_long:
                utt.too_long = True
                await self._error("too_long", f"utterance passed max_audio_ms={MAX_AUDIO_MS}; "
                                  "further audio is dropped", utt.id)
        else:
            utt.pcm += buf
        self._schedule()

    async def close(self):
        self._closed = True
        self.active = None
        if self._task is not None and not self._task.done():
            self._task.cancel()

    def _start(self, msg):
        uid = msg.get("utterance")
        if not _is_int(uid):
            raise BadRequest("start needs an integer utterance id")
        if self._last_id is not None and uid <= self._last_id:
            raise BadRequest(f"utterance ids must increase (last was {self._last_id})", uid)
        for key in ("language", "context", "grammar"):
            if msg.get(key) is not None and not isinstance(msg[key], str):
                raise BadRequest(f"{key} must be a string", uid)
        rollback = msg.get("rollback_words")
        rollback = DEFAULT_ROLLBACK_WORDS if rollback is None else rollback
        if not _is_int(rollback) or rollback < 0:
            raise BadRequest("rollback_words must be a non-negative integer", uid)
        step = msg.get("min_step_ms")
        step = DEFAULT_MIN_STEP_MS if step is None else step
        if isinstance(step, bool) or not isinstance(step, (int, float)) or not 0 <= step <= MAX_AUDIO_MS:
            raise BadRequest("min_step_ms must be a number in [0, max_audio_ms]", uid)
        # A start while another utterance is active supersedes it silently.
        self.active = None
        self._last_id = uid
        self._stray_audio_reported = False
        live = engine.Live(self.svc.model, rollback, self.svc.settle_s)
        self.active = Utterance(uid, live, rollback, max(1, int(step * SAMPLE_RATE / 1000)))

    def _addressed(self, msg, kind):
        uid = msg.get("utterance")
        if not _is_int(uid):
            raise BadRequest(f"{kind} needs an integer utterance id")
        utt = self.active
        if utt is not None and utt.id == uid:
            return None if (kind == "end" and utt.ended) else utt
        if self._last_id is not None and uid <= self._last_id:
            return None  # finished or superseded: a benign race
        raise BadRequest(f"{kind} for unknown utterance {uid}", uid)

    def _schedule(self):
        if self._closed or (self._task is not None and not self._task.done()):
            return
        utt = self.active
        if utt is None or utt.done:
            return
        n = utt.n_samples
        if utt.ended:
            final = True
        elif utt.too_long or n - utt.last_decode_at < utt.min_step:
            return
        else:
            final = False
        utt.last_decode_at = n
        self._task = asyncio.ensure_future(self._run(utt, n, final))

    async def _run(self, utt, n, final):
        try:
            await self._step(utt, n, final)
        except asyncio.CancelledError:
            pass
        except Exception as e:  # noqa: BLE001
            log.exception("decode failed")
            if self.active is utt:
                if final:
                    utt.done = True
                await self._error("internal", str(e), utt.id)
        finally:
            self._task = None
            if not self._closed:
                self._schedule()

    @staticmethod
    def _decode(utt, chunk, final):
        """In the model thread: feed new samples, decode, settle."""
        live = utt.live
        live.update(np.frombuffer(chunk, "<i2").astype(np.float32) / 32768.0, final=final)
        text = live.text()
        stable = live.stable_text() if not final else text
        if not text.startswith(stable):  # never claim more than holds
            stable = ""
        return text, len(stable.encode())

    async def _step(self, utt, n, final):
        t0 = time.monotonic()
        # Copied here: the socket keeps appending to pcm while the model runs.
        chunk = bytes(utt.pcm[utt.fed * 2:n * 2])
        utt.fed = n
        text, stable_chars = await self.svc.call(self._decode, utt, chunk, final)
        decode_ms = int((time.monotonic() - t0) * 1000)
        if self.active is not utt:
            return  # cancelled or superseded meanwhile
        audio_ms = n * 1000 // SAMPLE_RATE
        if final:
            utt.done = True
            self.active = None
            log.info("final %d: %.1f s audio, %d ms: %r", utt.id, n / SAMPLE_RATE, decode_ms, text)
            await self._send({"type": "final", "utterance": utt.id, "text": text,
                              "audio_ms": audio_ms, "decode_ms": decode_ms})
            return
        if (text, stable_chars) == utt.last:
            return
        utt.last = (text, stable_chars)
        utt.seq += 1
        await self._send({"type": "partial", "utterance": utt.id, "seq": utt.seq, "text": text,
                          "stable_chars": stable_chars, "language": "English",
                          "audio_ms": audio_ms, "decode_ms": decode_ms})

    async def _error(self, code, message, utterance):
        await self._send({"type": "error", "utterance": utterance, "code": code, "message": message})


async def stream(request):
    svc: Service = request.app["svc"]
    ws = web.WebSocketResponse(heartbeat=None, max_msg_size=8 << 20)
    await ws.prepare(request)
    lock = asyncio.Lock()

    async def send(msg):
        async with lock:
            if not ws.closed:
                await ws.send_str(json.dumps(msg, ensure_ascii=False))

    session = Session(svc, send)
    await send({"type": "ready", "protocol": PROTOCOL, "model": svc.name,
                "sample_rate": SAMPLE_RATE, "max_audio_ms": MAX_AUDIO_MS, "grammar": False})
    try:
        async for msg in ws:
            if msg.type == WSMsgType.BINARY:
                await session.on_binary(msg.data)
            elif msg.type == WSMsgType.TEXT:
                await session.on_text(msg.data)
    finally:
        await session.close()
    return ws


async def health(request):
    return web.json_response({"status": "ok", "model": request.app["svc"].name})


async def models(request):
    return web.json_response({"object": "list", "data": [
        {"id": request.app["svc"].name, "object": "model", "owned_by": "moonshine"}]})


async def transcriptions(request):
    svc: Service = request.app["svc"]
    form = await request.post()
    f = form.get("file")
    if f is None or not hasattr(f, "file"):
        return web.json_response({"detail": "multipart field 'file' missing"}, status=400)
    try:
        audio = wav_to_16k(f.file.read())
    except Exception as e:  # noqa: BLE001 -- bad audio is the caller's error
        return web.json_response({"detail": str(e)}, status=400)
    t = time.monotonic()
    text = await svc.call(svc.transcribe, audio)
    log.info("transcription: %.1f s audio, %.0f ms: %r", len(audio) / SAMPLE_RATE,
             (time.monotonic() - t) * 1000, text)
    if form.get("response_format") == "text":
        return web.Response(text=text)
    return web.json_response({"text": text})


async def chat(request):
    """ears' no-speech check (and bash mode, whose grammar is ignored), in
    Qwen3-ASR's `language X<asr_text>` shape."""
    svc: Service = request.app["svc"]
    try:
        req = await request.json()
        audio = None
        for m in req.get("messages", []):
            content = m.get("content")
            for part in content if isinstance(content, list) else []:
                if isinstance(part, dict) and part.get("type") == "input_audio":
                    audio = base64.b64decode(part["input_audio"]["data"])
        if audio is None:
            return web.json_response({"detail": "no input_audio part"}, status=400)
        pcm = wav_to_16k(audio)
    except Exception as e:  # noqa: BLE001
        return web.json_response({"detail": str(e)}, status=400)
    text = await svc.call(svc.transcribe, pcm)
    header = "language English" if text else "language None"
    log.info("chat: %s %r", header, text)
    return web.json_response({"object": "chat.completion", "model": svc.name, "choices": [{
        "index": 0, "finish_reason": "stop",
        "message": {"role": "assistant", "content": f"{header}<asr_text>{text}"}}]})


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("model_dir")
    ap.add_argument("--name", default="moonshine-ai/moonshine-streaming-tiny")
    ap.add_argument("--host", default="127.0.0.1")
    ap.add_argument("--port", type=int, default=30190)
    ap.add_argument("--threads", type=int)
    ap.add_argument("--settle-s", type=float, default=1.0)
    args = ap.parse_args()
    logging.basicConfig(level=logging.INFO, format="%(asctime)s %(message)s")
    model = engine.Model(args.model_dir, args.threads)
    svc = Service(model, args.name, args.settle_s)
    svc.transcribe(np.zeros(SAMPLE_RATE, np.float32))  # warm up
    app = web.Application(client_max_size=64 << 20)
    app["svc"] = svc
    app.router.add_get("/health", health)
    app.router.add_get("/v1/models", models)
    app.router.add_post("/v1/audio/transcriptions", transcriptions)
    app.router.add_post("/v1/chat/completions", chat)
    app.router.add_get("/v1/ears/stream", stream)
    log.info("moonshine-stream: %s on http://%s:%d", args.name, args.host, args.port)
    web.run_app(app, host=args.host, port=args.port, print=None)


if __name__ == "__main__":
    main()
