"""`vllm.endpoint_plugins` entry point: `GET /v1/ears/stream` (WebSocket)."""

from __future__ import annotations

import asyncio
import json
import logging
import os
from dataclasses import dataclass
from typing import Any

from fastapi import APIRouter, FastAPI, WebSocket
from starlette.websockets import WebSocketDisconnect, WebSocketState

from .protocol import PROTOCOL_VERSION, SAMPLE_RATE
from .session import Decoder, StreamSession

logger = logging.getLogger("ears_vllm_stream")

PATH = "/v1/ears/stream"
STATE_KEY = "ears_stream"


@dataclass
class StreamService:
    """Per-app state the route needs. `decode` is None when unsupported."""

    model: str
    decode: Decoder | None
    max_audio_ms: int
    unsupported_reason: str = ""


router = APIRouter()


@router.websocket(PATH)
async def ears_stream(websocket: WebSocket) -> None:
    service: StreamService | None = getattr(websocket.app.state, STATE_KEY, None)
    await websocket.accept()
    if service is None or service.decode is None:
        reason = (service.unsupported_reason if service else "") or "plugin not initialised"
        await _send_json(websocket, {"type": "error", "utterance": None,
                                     "code": "unsupported", "message": reason})
        await websocket.close(code=1011)
        return

    lock = asyncio.Lock()

    async def send(msg: dict[str, Any]) -> None:
        async with lock:
            if websocket.application_state == WebSocketState.CONNECTED:
                await _send_json(websocket, msg)

    session = StreamSession(send, service.decode, max_audio_ms=service.max_audio_ms)
    await send({"type": "ready", "protocol": PROTOCOL_VERSION, "model": service.model,
                "sample_rate": SAMPLE_RATE, "max_audio_ms": service.max_audio_ms})
    try:
        while True:
            frame = await websocket.receive()
            if frame["type"] == "websocket.disconnect":
                break
            if frame.get("bytes") is not None:
                await session.on_binary(frame["bytes"])
            elif frame.get("text") is not None:
                await session.on_text(frame["text"])
    except WebSocketDisconnect:
        pass
    except Exception:  # noqa: BLE001
        logger.exception("ears stream connection failed")
    finally:
        await session.close()


async def _send_json(websocket: WebSocket, msg: dict[str, Any]) -> None:
    await websocket.send_text(json.dumps(msg, ensure_ascii=False))


class EarsStreamPlugin:
    """Implements `vllm.plugins.endpoint_plugins.interface.EndpointPlugin`."""

    name = "ears_stream"
    required_tasks = ("generate",)

    def attach_router(self, app: FastAPI) -> None:
        app.include_router(router)

    async def init_state(self, engine_client, state, args) -> None:
        setattr(state, STATE_KEY, build_service(engine_client, args))


def _served_name(model_config, args) -> str:
    names = getattr(args, "served_model_name", None) or getattr(
        model_config, "served_model_name", None
    )
    if isinstance(names, (list, tuple)):
        names = names[0] if names else None
    return str(names or getattr(model_config, "model", "") or "unknown")


def build_service(engine_client, args) -> StreamService:
    from .engine import VllmDecoder, default_max_audio_ms, is_qwen3_asr

    if engine_client is None:
        return StreamService("", None, 0, "server has no engine (render-only)")
    model_config = engine_client.model_config
    model = _served_name(model_config, args)
    max_audio_ms = default_max_audio_ms(model_config.max_model_len)
    override = os.environ.get("EARS_STREAM_MAX_AUDIO_MS")
    if override:
        max_audio_ms = max(1000, min(max_audio_ms, int(override)))
    if not is_qwen3_asr(model_config):
        return StreamService(model, None, max_audio_ms,
                             f"model {model!r} is not Qwen3-ASR")
    logger.info("ears stream: %s on %s, max_audio_ms=%d", PATH, model, max_audio_ms)
    return StreamService(model, VllmDecoder(engine_client), max_audio_ms)
