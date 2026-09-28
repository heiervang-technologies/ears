"""vLLM glue: one decode = one engine request.

Follows the in-tree speech-to-text path (`speech_to_text/base/serving.py`,
`realtime/serving.py`): build a `TokensPrompt` with `multi_modal_data`,
`parse_model_prompt` it, render it with `renderer.render_cmpl_async`
(multimodal processing, placeholder expansion, encoder cache hashing) and
hand the engine input to `engine_client.generate`. No other engine access.

vLLM is imported lazily so the rest of the package imports without it.
"""

from __future__ import annotations

import uuid
from collections.abc import Sequence

import numpy as np

from .protocol import SAMPLE_RATE, build_prompt


class PromptTooLong(Exception):
    pass


def is_qwen3_asr(model_config) -> bool:
    archs = getattr(getattr(model_config, "hf_config", None), "architectures", None) or []
    if any("Qwen3ASR" in a for a in archs):
        return True
    return "qwen3-asr" in str(getattr(model_config, "model", "")).lower()


def default_max_audio_ms(max_model_len: int) -> int:
    """Longest utterance whose decode fits the model context.

    Per second of audio a decode spends 13 audio tokens (104 per 8 s window),
    about 3-4 transcript tokens of forced prefix, and up to 5 more of
    `max_tokens`; 20/s with 256 tokens kept for the template, context and
    the 64-token base keeps the worst case inside `max_model_len`. Capped at
    90 s.
    """
    return max(1000, min(90_000, (max_model_len - 256) * 1000 // 20))


class VllmDecoder:
    """`decode(windows, prefix_text, max_tokens, *, context) -> continuation`."""

    def __init__(self, engine_client):
        self.engine_client = engine_client
        self.model_config = engine_client.model_config
        self.renderer = engine_client.renderer
        self.tokenizer = self.renderer.get_tokenizer()
        self.max_model_len = self.model_config.max_model_len

    def prompt_token_ids(self, context: str, n_windows: int, prefix: str) -> list[int]:
        # Same text the per-request chat template renders, tokenized the way
        # the chat path tokenizes a rendered template (no extra specials).
        text = build_prompt(context, n_windows, prefix)
        return list(self.tokenizer.encode(text, add_special_tokens=False))

    async def __call__(
        self,
        windows: Sequence[np.ndarray],
        prefix_text: str,
        max_tokens: int,
        *,
        context: str = "",
    ) -> str:
        from vllm.inputs import TokensPrompt
        from vllm.renderers.inputs.preprocess import parse_model_prompt
        from vllm.sampling_params import SamplingParams

        prompt = TokensPrompt(
            prompt_token_ids=self.prompt_token_ids(context, len(windows), prefix_text),
            multi_modal_data={"audio": [np.asarray(w, dtype=np.float32) for w in windows]},
        )
        parsed = parse_model_prompt(self.model_config, prompt)
        (engine_input,) = await self.renderer.render_cmpl_async([parsed])

        # Placeholders are expanded now (13 tokens per second of audio).
        prompt_len = len(engine_input["prompt_token_ids"])
        budget = self.max_model_len - prompt_len
        if budget < 1:
            raise PromptTooLong(
                f"prompt of {prompt_len} tokens leaves no room in "
                f"max_model_len={self.max_model_len}"
            )
        params = SamplingParams.from_optional(
            temperature=0.0, max_tokens=min(max_tokens, budget)
        )

        request_id = f"ears-stream-{uuid.uuid4().hex}"
        last = None
        # Cancelling this coroutine aborts the engine request (AsyncLLM.generate
        # handles CancelledError/GeneratorExit with an abort).
        async for out in self.engine_client.generate(engine_input, params, request_id):
            last = out
        if last is None or not last.outputs:
            raise RuntimeError("engine returned no output")
        if last.outputs[0].finish_reason == "error":
            raise RuntimeError("engine reported an internal error")
        return last.outputs[0].text


__all__ = ["VllmDecoder", "PromptTooLong", "is_qwen3_asr", "default_max_audio_ms", "SAMPLE_RATE"]
