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

    def transcript_tokens(self, text: str, frozen_bytes: int) -> list[dict] | None:
        """Tokenizer spans of the current transcript, not sampled-token history.

        BPE can merge across the forced-prefix seam on re-tokenization. A
        token touching mutable text is therefore mutable even if part of
        its span is frozen. Offsets on the wire are UTF-8 bytes; HF offsets
        are Unicode characters (and can overlap for byte-fallback tokens).
        Unsupported tokenizers omit this optional detail, never guess it.
        """
        try:
            encoded = self.tokenizer(text, add_special_tokens=False,
                                     return_offsets_mapping=True)
            ids, offsets = encoded["input_ids"], encoded["offset_mapping"]
            if len(ids) != len(offsets):
                return None
            byte_offsets = [0]
            for char in text:
                byte_offsets.append(byte_offsets[-1] + len(char.encode("utf-8")))
            tokens = []
            for token_id, (start, end) in zip(ids, offsets):
                if not 0 <= start < end <= len(text):
                    return None
                a, b = byte_offsets[start], byte_offsets[end]
                tokens.append({"id": int(token_id), "start_byte": a, "end_byte": b,
                               "state": "frozen" if b <= frozen_bytes else "mutable"})
            return tokens
        except (TypeError, AttributeError, KeyError, NotImplementedError, ValueError):
            return None

    # The session may pass the previous decode's open words as `draft`.
    supports_draft = True

    async def __call__(
        self,
        windows: Sequence[np.ndarray],
        prefix_text: str,
        max_tokens: int,
        *,
        context: str = "",
        draft: str = "",
    ) -> str:
        """Greedy continuation of `prefix_text` over `windows`.

        With a `draft` (the open words of the previous decode) the draft is
        appended to the prompt and checked rather than decoded: prompt
        logprobs give the model's own top-1 token at every draft position,
        and the longest run where the draft token *is* the top-1 is exactly
        what greedy decoding would have produced, so it is kept for the cost
        of one prefill. A full match keeps the generation that followed it.
        At the first mismatch the top-1 token is known too; one more request
        continues from there, its prompt served by the prefix cache.
        """
        audio = [np.asarray(w, dtype=np.float32) for w in windows]
        base = self.prompt_token_ids(context, len(windows), prefix_text)
        draft_ids = self._draft_ids(base, context, len(windows), prefix_text, draft)
        if not draft_ids:
            return (await self._generate(base, audio, max_tokens)).text

        out = await self._generate(base + draft_ids, audio,
                                   max(1, max_tokens - len(draft_ids)), verify=True)
        checked = self._check_draft(out, len(draft_ids))
        if checked is None:  # the draft's logprobs were not computed
            return (await self._generate(base, audio, max_tokens)).text
        accepted, top = checked
        if accepted == len(draft_ids):
            return self._text(draft_ids + list(out.outputs[0].token_ids))
        kept = draft_ids[:accepted] + [top]
        if top in self._stop_ids() or max_tokens - len(kept) < 1:
            return self._text(kept)
        rest = await self._generate(base + kept, audio, max_tokens - len(kept))
        return self._text(kept + list(rest.token_ids))

    def _draft_ids(self, base, context, n_windows, prefix_text, draft) -> list[int]:
        """Draft tokens appended to `base`, or [] when there is no draft or
        tokenizing prefix and draft together changes the prefix's tokens."""
        if not prefix_text or not draft.strip():
            return []
        full = self.prompt_token_ids(context, n_windows, prefix_text + draft)
        if len(full) <= len(base) or full[:len(base)] != base:
            return []
        return full[len(base):]

    @staticmethod
    def _check_draft(out, n_draft: int) -> tuple[int, int] | None:
        """(accepted draft tokens, top-1 token at the first mismatch).

        None when a draft position's logprobs are missing: with the prefix
        cache read, positions served from the cache are never computed."""
        ids = out.prompt_token_ids
        plp = out.prompt_logprobs
        start = len(ids) - n_draft
        if plp is None or start < (out.num_cached_tokens or 0) + 1:
            return None
        for i in range(start, len(ids)):
            entry = plp[i]
            if not entry or ids[i] not in entry:
                return None
            if entry[ids[i]].rank == 1:
                continue
            top = next((t for t, lp in entry.items() if lp.rank == 1), None)
            return None if top is None else (i - start, top)
        return n_draft, -1

    def _stop_ids(self) -> set[int]:
        ids = {self.tokenizer.eos_token_id}
        for tok in ("<|im_end|>", "<|endoftext|>"):
            tid = self.tokenizer.convert_tokens_to_ids(tok)
            if isinstance(tid, int):
                ids.add(tid)
        return ids - {None}

    def _text(self, token_ids: list[int]) -> str:
        return self.tokenizer.decode(token_ids, skip_special_tokens=True)

    async def _generate(self, token_ids, audio, max_tokens, *, verify=False):
        """One engine request. Returns the CompletionOutput, or with `verify`
        the RequestOutput (prompt logprobs and cached-token count needed)."""
        from vllm.inputs import TokensPrompt
        from vllm.renderers.inputs.preprocess import parse_model_prompt
        from vllm.sampling_params import SamplingParams

        prompt = TokensPrompt(prompt_token_ids=token_ids, multi_modal_data={"audio": audio})
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
        if verify:
            # Top-1 at every computed prompt position. vLLM skips reading the
            # prefix cache for prompt logprobs unless told otherwise; the draft
            # follows the open audio window, so its positions are recomputed
            # anyway and the cached part is only skipped, never misread
            # (`_check_draft`). No detokenization: rows of cached positions
            # are left unfilled, and the caller decodes the token ids itself.
            params = SamplingParams.from_optional(
                temperature=0.0, max_tokens=min(max_tokens, budget),
                prompt_logprobs=1, detokenize=False,
            )
            params.skip_reading_prefix_cache = False
        else:
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
        return last if verify else last.outputs[0]


__all__ = ["VllmDecoder", "PromptTooLong", "is_qwen3_asr", "default_max_audio_ms", "SAMPLE_RATE"]
