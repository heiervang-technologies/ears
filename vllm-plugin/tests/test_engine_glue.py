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


def test_token_spans_use_utf8_and_keep_seam_tokens_mutable():
    d = VllmDecoder(engine(1)[0])
    class OffsetTokenizer:
        def __call__(self, text, **kwargs):
            assert kwargs == {"add_special_tokens": False, "return_offsets_mapping": True}
            # Two byte-fallback pieces can share a Unicode character span.
            return {"input_ids": [1, 2, 3, 4],
                    "offset_mapping": [(0, 1), (1, 2), (1, 2), (2, 5)]}
    d.tokenizer = OffsetTokenizer()
    tokens = d.transcript_tokens("a你 bc", len("a你 ".encode()))
    assert [t["state"] for t in tokens] == ["frozen", "frozen", "frozen", "mutable"]
    assert [(t["start_byte"], t["end_byte"]) for t in tokens] == [(0, 1), (1, 4), (1, 4), (4, 7)]
    assert all(t["state"] == "mutable" for t in d.transcript_tokens("a你 bc", 0))
    d.tokenizer = Tokenizer()  # tokenizer without offsets: no invented word tokens
    assert d.transcript_tokens("abc", 2) is None


# --- draft verification -----------------------------------------------------
#
# A character-level fake model: one token per character, and greedy decoding
# writes TRUTH after the `<asr_text>` tag. Its top-1 at any assistant position
# is TRUTH's next character while the forced text still agrees with TRUTH.

EOS = 0
TRUTH = " here is the plan."
PREFIX = "language English<asr_text>Okay, so"


class CharTokenizer(Tokenizer):
    eos_token_id = EOS

    def decode(self, ids, skip_special_tokens=True):
        return "".join(chr(i) for i in ids if i != EOS)

    def convert_tokens_to_ids(self, tok):
        return None


def greedy_after(text):
    """(top-1 next token, full greedy continuation) after prompt `text`."""
    if "<asr_text>Okay, so" not in text:
        return EOS, [EOS]
    answer = text.split("<asr_text>Okay, so", 1)[1]
    if not TRUTH.startswith(answer):
        return EOS, []
    rest = TRUTH[len(answer):]
    return (ord(rest[0]) if rest else EOS), [ord(c) for c in rest] + [EOS]


def draft_engine(num_cached=0):
    calls = []

    class Renderer:
        def get_tokenizer(self):
            return CharTokenizer()

        async def render_cmpl_async(self, prompts):
            return [{"prompt_token_ids": list(prompts[0]["prompt_token_ids"])}]

    async def generate(engine_input, params, request_id):
        ids = engine_input["prompt_token_ids"]
        calls.append((ids, params))
        _, gen = greedy_after("".join(map(chr, ids)))
        plp = None
        if getattr(params, "prompt_logprobs", None):
            start = "".join(map(chr, ids)).index("<asr_text>Okay, so") + len("<asr_text>Okay, so")
            plp = [None]
            for i in range(1, len(ids)):
                if i < start:
                    plp.append({ids[i]: SimpleNamespace(rank=1)})
                    continue
                top, _ = greedy_after("".join(map(chr, ids[:i])))
                entry = {top: SimpleNamespace(rank=1)}
                if ids[i] != top:
                    entry[ids[i]] = SimpleNamespace(rank=2)
                plp.append(entry)
        yield SimpleNamespace(
            prompt_token_ids=ids, prompt_logprobs=plp, num_cached_tokens=num_cached,
            outputs=[SimpleNamespace(text="".join(chr(t) for t in gen if t != EOS),
                                     token_ids=gen, finish_reason="stop")])

    model_config = SimpleNamespace(
        max_model_len=4096, model="Qwen/Qwen3-ASR-1.7B",
        hf_config=SimpleNamespace(architectures=["Qwen3ASRForConditionalGeneration"]))
    return SimpleNamespace(model_config=model_config, renderer=Renderer(),
                           generate=generate), calls


def decode_with_draft(draft, num_cached=0):
    client, calls = draft_engine(num_cached)
    d = VllmDecoder(client)
    out = asyncio.run(d([np.zeros(1600, np.float32)], PREFIX, 100, draft=draft))
    return out, calls


def test_correct_draft_is_kept_and_generation_continues_in_one_request(fake_vllm):
    out, calls = decode_with_draft(" here is")
    assert out == TRUTH
    ((ids, params),) = calls
    assert "".join(map(chr, ids)).endswith(PREFIX + " here is")
    assert params.prompt_logprobs == 1 and params.detokenize is False
    assert params.skip_reading_prefix_cache is False
    assert params.max_tokens == 100 - len(" here is")


def test_wrong_draft_continues_from_the_models_own_token(fake_vllm):
    out, calls = decode_with_draft(" here as the")
    assert out == TRUTH  # identical to decoding without a draft
    (_, verify), (ids, plain) = calls
    # Kept " here " (accepted) + "i" (top-1 at the mismatch), then generated.
    assert "".join(map(chr, ids)).endswith(PREFIX + " here i")
    assert getattr(plain, "prompt_logprobs", None) is None
    assert plain.max_tokens == 100 - len(" here i")


def test_draft_rejected_by_end_of_sequence_needs_no_second_request(fake_vllm):
    out, calls = decode_with_draft(" here is the plan. And")
    assert out == TRUTH
    assert len(calls) == 1


def test_draft_positions_served_from_cache_fall_back_to_plain_decode(fake_vllm):
    out, calls = decode_with_draft(" here is", num_cached=10_000)
    assert out == TRUTH
    (_, verify), (ids, plain) = calls
    assert "".join(map(chr, ids)).endswith(PREFIX)  # no draft in the retry


def test_no_draft_without_forced_prefix(fake_vllm):
    client, calls = draft_engine()
    asyncio.run(VllmDecoder(client)([np.zeros(1600, np.float32)], "", 100, draft=" hi"))
    ((ids, params),) = calls
    assert getattr(params, "prompt_logprobs", None) is None
