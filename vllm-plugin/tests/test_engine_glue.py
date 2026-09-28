"""VllmDecoder against stubbed vLLM modules: checks the glue's call shape."""

import asyncio
import sys
import types
from types import SimpleNamespace

import numpy as np
import pytest

from ears_vllm_stream.engine import PromptTooLong, VllmDecoder, default_max_audio_ms, is_qwen3_asr
from ears_vllm_stream.plugin import build_service


@pytest.fixture
def fake_vllm(monkeypatch):
    calls = {}

    def mod(name, **attrs):
        m = types.ModuleType(name)
        m.__dict__.update(attrs)
        monkeypatch.setitem(sys.modules, name, m)

    class SamplingParams:
        @staticmethod
        def from_optional(**kw):
            calls["sampling"] = kw
            return SimpleNamespace(**kw)

    def parse_model_prompt(model_config, prompt):
        calls["parsed"] = prompt
        return prompt

    mod("vllm")
    mod("vllm.inputs", TokensPrompt=dict)
    mod("vllm.renderers")
    mod("vllm.renderers.inputs")
    mod("vllm.renderers.inputs.preprocess", parse_model_prompt=parse_model_prompt)
    mod("vllm.sampling_params", SamplingParams=SamplingParams)
    return calls


class Tokenizer:
    def encode(self, text, add_special_tokens=True):
        assert add_special_tokens is False
        return [ord(c) for c in text]


def engine(expanded_len, text="language English<asr_text>hi", max_model_len=2048):
    calls = {}

    class Renderer:
        def get_tokenizer(self):
            return Tokenizer()

        async def render_cmpl_async(self, prompts):
            calls["render"] = prompts
            return [{"prompt_token_ids": [0] * expanded_len}]

    async def generate(engine_input, params, request_id):
        calls["generate"] = (engine_input, params, request_id)
        yield SimpleNamespace(outputs=[SimpleNamespace(text="partial", finish_reason=None)])
        yield SimpleNamespace(outputs=[SimpleNamespace(text=text, finish_reason="stop")])

    model_config = SimpleNamespace(
        max_model_len=max_model_len, model="Qwen/Qwen3-ASR-1.7B",
        served_model_name="Qwen/Qwen3-ASR-1.7B",
        hf_config=SimpleNamespace(architectures=["Qwen3ASRForConditionalGeneration"]))
    return SimpleNamespace(model_config=model_config, renderer=Renderer(),
                           generate=generate), calls


def test_decode_renders_tokens_prompt_and_generates(fake_vllm):
    client, calls = engine(expanded_len=1000)
    d = VllmDecoder(client)
    w = [np.zeros(128000, np.float32), np.zeros(1600, np.float32)]
    out = asyncio.run(d(w, "language English<asr_text>", 110, context="ctx"))
    assert out == "language English<asr_text>hi"  # last cumulative output
    (prompt,) = calls["render"]
    text = "".join(map(chr, prompt["prompt_token_ids"]))
    assert text.startswith("<|im_start|>system\nctx<|im_end|>")
    assert text.count("<|audio_pad|>") == 2
    assert text.endswith("assistant\nlanguage English<asr_text>")
    assert len(prompt["multi_modal_data"]["audio"]) == 2
    assert fake_vllm["sampling"] == {"temperature": 0.0, "max_tokens": 110}


def test_max_tokens_clamped_to_context(fake_vllm):
    client, _ = engine(expanded_len=2000)
    asyncio.run(VllmDecoder(client)([np.zeros(1600, np.float32)], "", 500, context=""))
    assert fake_vllm["sampling"]["max_tokens"] == 48
    client, _ = engine(expanded_len=2048)
    with pytest.raises(PromptTooLong):
        asyncio.run(VllmDecoder(client)([np.zeros(1600, np.float32)], "", 500, context=""))


def test_service_setup(monkeypatch):
    client, _ = engine(expanded_len=1)
    s = build_service(client, SimpleNamespace(served_model_name=None))
    assert s.decode is not None and s.model == "Qwen/Qwen3-ASR-1.7B"
    assert s.max_audio_ms == default_max_audio_ms(2048) == 89_600
    monkeypatch.setenv("EARS_STREAM_MAX_AUDIO_MS", "30000")
    assert build_service(client, SimpleNamespace()).max_audio_ms == 30_000
    client.model_config.hf_config.architectures = ["WhisperForConditionalGeneration"]
    client.model_config.model = "openai/whisper-large-v3"
    assert not is_qwen3_asr(client.model_config)
    assert build_service(client, SimpleNamespace()).decode is None
    assert build_service(None, SimpleNamespace()).decode is None
