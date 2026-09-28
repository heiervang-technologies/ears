import numpy as np
from fastapi import FastAPI
from fastapi.testclient import TestClient

from ears_vllm_stream import EarsStreamPlugin
from ears_vllm_stream.plugin import PATH, STATE_KEY, StreamService


async def fake_decode(windows, prefix_text, max_tokens, *, context):
    return "language English<asr_text>hello world" if not prefix_text else " world"


def app_with(service):
    app = FastAPI()
    EarsStreamPlugin().attach_router(app)
    setattr(app.state, STATE_KEY, service)
    return app


def test_ready_partial_final_over_websocket():
    app = app_with(StreamService("Qwen/Qwen3-ASR-1.7B", fake_decode, 90_000))
    with TestClient(app).websocket_connect(PATH) as ws:
        ready = ws.receive_json()
        assert ready == {"type": "ready", "protocol": 1, "model": "Qwen/Qwen3-ASR-1.7B",
                         "sample_rate": 16000, "max_audio_ms": 90_000}
        ws.send_json({"type": "start", "utterance": 1})
        ws.send_bytes(np.zeros(4800, dtype="<i2").tobytes())
        p = ws.receive_json()
        assert p["type"] == "partial" and p["text"] == "hello world"
        assert p["language"] is None  # pinned only after 2 s of agreeing decodes
        ws.send_json({"type": "end", "utterance": 1})
        f = ws.receive_json()
        assert f["type"] == "final" and f["utterance"] == 1


def test_unsupported_model_is_reported_then_closed():
    app = app_with(StreamService("whisper-1", None, 90_000, "model 'whisper-1' is not Qwen3-ASR"))
    with TestClient(app).websocket_connect(PATH) as ws:
        err = ws.receive_json()
        assert err["code"] == "unsupported" and err["utterance"] is None
        msg = ws.receive()
        assert msg["type"] == "websocket.close"


def test_plugin_shape():
    p = EarsStreamPlugin()
    assert p.name == "ears_stream"
    assert p.required_tasks == ("generate",)
