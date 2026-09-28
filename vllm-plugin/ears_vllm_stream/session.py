"""Connection state machine for the ears stream protocol.

Transport-agnostic and engine-agnostic: a `StreamSession` is fed decoded
frames (`on_text`, `on_binary`) and talks back through an async `send(dict)`.
Decoding goes through an async `decode(windows, prefix_text, max_tokens,
*, context)` callable returning the model's continuation text, so the whole
lifecycle is testable without vLLM or a GPU.
"""

from __future__ import annotations

import asyncio
import json
import logging
import time
from collections.abc import Awaitable, Callable, Sequence
from dataclasses import dataclass, field
from typing import Any, Protocol

import numpy as np

from .protocol import (
    ASR_TAG,
    DEFAULT_MIN_STEP_MS,
    DEFAULT_ROLLBACK_WORDS,
    ENCODER_WINDOW,
    MIN_PIN_SAMPLES,
    SAMPLE_RATE,
    header_for,
    header_language,
    language_name,
    max_tokens_for,
    sanitize_context,
    settled_prefix,
    window_bounds,
)

logger = logging.getLogger("ears_vllm_stream")


class Decoder(Protocol):
    def __call__(
        self,
        windows: Sequence[np.ndarray],
        prefix_text: str,
        max_tokens: int,
        *,
        context: str,
    ) -> Awaitable[str]: ...


Send = Callable[[dict[str, Any]], Awaitable[None]]


class BadRequest(Exception):
    def __init__(self, message: str, utterance: int | None = None):
        super().__init__(message)
        self.utterance = utterance


@dataclass
class Utterance:
    id: int
    context: str
    rollback_words: int
    min_step: int  # samples
    max_samples: int
    header: str | None = None
    candidate: str | None = None  # language detected last while unpinned
    stable: str = ""
    pcm: bytearray = field(default_factory=bytearray)  # PCM16 LE, capped
    odd: bytes = b""  # trailing odd byte kept for the next frame
    closed: list[np.ndarray] = field(default_factory=list)  # cached windows
    last_decode_at: int = 0  # samples at the start of the last decode
    last_text: str = ""
    seq: int = 0
    ended: bool = False
    too_long: bool = False
    done: bool = False

    @property
    def n_samples(self) -> int:
        return len(self.pcm) // 2

    def windows(self, n: int) -> list[np.ndarray]:
        """Float32 encoder windows over the first `n` samples; closed windows
        are converted once and reused so they stay identical across decodes."""
        out = []
        for i, (start, end) in enumerate(window_bounds(n)):
            if end - start == ENCODER_WINDOW and i < len(self.closed):
                out.append(self.closed[i])
                continue
            w = (
                np.frombuffer(self.pcm, dtype="<i2", count=end - start, offset=start * 2)
                .astype(np.float32)
                / 32768.0
            )
            if end - start == ENCODER_WINDOW and i == len(self.closed):
                self.closed.append(w)
            out.append(w)
        return out


def _ms(samples: int) -> int:
    return samples * 1000 // SAMPLE_RATE


class StreamSession:
    """One WebSocket connection: at most one utterance, one decode in flight."""

    def __init__(self, send: Send, decode: Decoder, *, max_audio_ms: int):
        self._send = send
        self._decode = decode
        self.max_audio_ms = max_audio_ms
        self._max_samples = max_audio_ms * SAMPLE_RATE // 1000
        self.active: Utterance | None = None
        self._last_id: int | None = None
        self._task: asyncio.Task | None = None
        self._stray_audio_reported = False
        self._closed = False

    # ---------------------------------------------------------------- frames

    async def on_text(self, raw: str) -> None:
        try:
            try:
                msg = json.loads(raw)
            except (json.JSONDecodeError, ValueError):
                raise BadRequest("frame is not valid JSON") from None
            if not isinstance(msg, dict):
                raise BadRequest("frame is not a JSON object")
            uid = msg.get("utterance")
            uid = uid if _is_int(uid) else None
            kind = msg.get("type")
            if kind == "start":
                self._start(msg)
            elif kind == "end":
                self._end(msg)
            elif kind == "cancel":
                self._cancel(msg)
            else:
                raise BadRequest(f"unknown message type {kind!r}", uid)
        except BadRequest as e:
            await self._error("bad_request", str(e), e.utterance)
        self._schedule()

    async def on_binary(self, data: bytes) -> None:
        utt = self.active
        if utt is None or utt.ended:
            if not self._stray_audio_reported:
                self._stray_audio_reported = True
                what = "after end" if utt is not None else "without an active utterance"
                await self._error(
                    "bad_request", f"audio {what}", utt.id if utt is not None else None
                )
            return
        buf = utt.odd + data
        if len(buf) % 2:
            utt.odd = buf[-1:]
            buf = buf[:-1]
        else:
            utt.odd = b""
        room = utt.max_samples * 2 - len(utt.pcm)
        if len(buf) > room:
            utt.pcm += buf[: max(room, 0)]
            if not utt.too_long:
                utt.too_long = True
                await self._error(
                    "too_long",
                    f"utterance passed max_audio_ms={self.max_audio_ms}; "
                    "further audio is dropped",
                    utt.id,
                )
        else:
            utt.pcm += buf
        self._schedule()

    async def close(self) -> None:
        """Socket gone: drop the utterance and any decode in flight."""
        self._closed = True
        self.active = None
        task = self._task
        if task is not None and not task.done():
            task.cancel()
            try:
                await task
            except (asyncio.CancelledError, Exception):
                pass

    # ------------------------------------------------------------- lifecycle

    def _start(self, msg: dict) -> None:
        uid = msg.get("utterance")
        if not _is_int(uid):
            raise BadRequest("start needs an integer utterance id")
        if self._last_id is not None and uid <= self._last_id:
            raise BadRequest(
                f"utterance ids must increase (last was {self._last_id})", uid
            )
        language = msg.get("language")
        if language is not None and not isinstance(language, str):
            raise BadRequest("language must be a string", uid)
        context = msg.get("context")
        if context is not None and not isinstance(context, str):
            raise BadRequest("context must be a string", uid)
        rollback = msg.get("rollback_words", DEFAULT_ROLLBACK_WORDS)
        if rollback is None:
            rollback = DEFAULT_ROLLBACK_WORDS
        if not _is_int(rollback) or rollback < 0:
            raise BadRequest("rollback_words must be a non-negative integer", uid)
        min_step_ms = msg.get("min_step_ms", DEFAULT_MIN_STEP_MS)
        if min_step_ms is None:
            min_step_ms = DEFAULT_MIN_STEP_MS
        if (
            isinstance(min_step_ms, bool)
            or not isinstance(min_step_ms, (int, float))
            or not min_step_ms >= 0  # also rejects NaN
            or min_step_ms > self.max_audio_ms
        ):
            raise BadRequest("min_step_ms must be a number in [0, max_audio_ms]", uid)

        # A start while another utterance is active supersedes it silently.
        self._drop_active()
        self._last_id = uid
        self._stray_audio_reported = False
        name = language_name(language)
        self.active = Utterance(
            id=uid,
            context=sanitize_context(context),
            rollback_words=rollback,
            min_step=max(1, int(min_step_ms * SAMPLE_RATE / 1000)),
            max_samples=self._max_samples,
            header=header_for(name) if name else None,
        )

    def _end(self, msg: dict) -> None:
        utt = self._addressed(msg, "end")
        if utt is None:
            return
        utt.ended = True

    def _cancel(self, msg: dict) -> None:
        if self._addressed(msg, "cancel") is None:
            return
        self._drop_active()

    def _addressed(self, msg: dict, kind: str) -> Utterance | None:
        """The active utterance `msg` names. None for a stale id (an utterance
        already finished or superseded: ignored, those races are benign)."""
        uid = msg.get("utterance")
        if not _is_int(uid):
            raise BadRequest(f"{kind} needs an integer utterance id")
        utt = self.active
        if utt is not None and utt.id == uid:
            return None if (kind == "end" and utt.ended) else utt
        if self._last_id is not None and uid <= self._last_id:
            return None
        raise BadRequest(f"{kind} for unknown utterance {uid}", uid)

    def _drop_active(self) -> None:
        self.active = None
        if self._task is not None and not self._task.done():
            # Aborts the engine request; `_run` re-schedules once it unwinds,
            # so a superseding utterance never overlaps the old decode.
            self._task.cancel()

    # ------------------------------------------------------------ scheduling

    def _schedule(self) -> None:
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
        elif not window_bounds(n):
            return
        else:
            final = False
        utt.last_decode_at = n
        self._task = asyncio.create_task(self._run(utt, n, final))

    async def _run(self, utt: Utterance, n: int, final: bool) -> None:
        try:
            await self._step(utt, n, final)
        except asyncio.CancelledError:
            pass
        finally:
            self._task = None
            if not self._closed:
                self._schedule()

    async def _step(self, utt: Utterance, n: int, final: bool) -> None:
        windows = utt.windows(n)
        audio_ms = _ms(sum(len(w) for w in windows))
        t0 = time.monotonic()
        if not windows:
            text: str | None = utt.stable
        else:
            prefix = f"{utt.header}{utt.stable}" if utt.header else ""
            try:
                continuation = await self._decode(
                    windows, prefix, max_tokens_for(n), context=utt.context
                )
            except asyncio.CancelledError:
                raise
            except Exception as e:  # noqa: BLE001 - any engine failure
                logger.warning("ears stream decode failed: %s", e, exc_info=True)
                if self.active is utt:
                    if final:
                        utt.done = True
                        self.active = None
                    await self._error("internal", f"decode failed: {e}", utt.id)
                return
            text = self._absorb(utt, continuation, final, n)
        decode_ms = int((time.monotonic() - t0) * 1000)
        if self.active is not utt:
            return  # cancelled or superseded meanwhile: send nothing
        if final:
            utt.done = True
            self.active = None
            await self._send(
                {
                    "type": "final",
                    "utterance": utt.id,
                    "text": text,
                    "audio_ms": audio_ms,
                    "decode_ms": decode_ms,
                }
            )
            return
        if utt.too_long or text == utt.last_text:
            return
        utt.last_text = text
        utt.seq += 1
        await self._send(
            {
                "type": "partial",
                "utterance": utt.id,
                "seq": utt.seq,
                "text": text,
                "stable_chars": len(utt.stable.encode("utf-8")),
                "language": header_language(utt.header),
                "audio_ms": audio_ms,
                "decode_ms": decode_ms,
            }
        )

    @staticmethod
    def _absorb(utt: Utterance, continuation: str, final: bool, n: int) -> str:
        """Fold a decode's continuation into the utterance; return the text.

        Mirrors `ContinuousDecoder::step`, plus two guards the protocol's
        promises need: settled text only ever grows, and a detected
        `language None` (no speech yet) is not locked in as the header.

        A detected language is pinned only after MIN_PIN_SAMPLES of audio
        and two decodes in a row agreeing: forcing a misdetection from the
        first fraction of a second turns the utterance into a translation
        (English espeak came out as Arabic, ears#153).
        """
        if utt.header is not None:
            hypothesis = utt.stable + continuation
        else:
            lang, tag, rest = continuation.partition(ASR_TAG)
            if tag:
                header = f"{lang.strip()}{ASR_TAG}"
                if header_language(header) in (None, "None"):
                    utt.candidate = None
                elif n >= MIN_PIN_SAMPLES and utt.candidate == header:
                    utt.header = header
                    utt.candidate = None
                else:
                    utt.candidate = header
                hypothesis = rest
            else:
                hypothesis = continuation
        hypothesis = hypothesis.lstrip()
        if not final and utt.header is not None:
            settled = settled_prefix(hypothesis, utt.rollback_words)
            if len(settled) > len(utt.stable) and settled.startswith(utt.stable):
                utt.stable = settled
        return hypothesis.rstrip()

    async def _error(self, code: str, message: str, utterance: int | None) -> None:
        await self._send(
            {"type": "error", "utterance": utterance, "code": code, "message": message}
        )


def _is_int(v: object) -> bool:
    return isinstance(v, int) and not isinstance(v, bool)
