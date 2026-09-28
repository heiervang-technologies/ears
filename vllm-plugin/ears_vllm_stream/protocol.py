"""Pure helpers for the ears stream protocol (docs/STREAM_PROTOCOL.md).

Everything here mirrors `src/continuous.rs` so a stream decode and a per-tick
HTTP decode build the same prompt and settle the same text. No vLLM imports.
"""

from __future__ import annotations

import re

PROTOCOL_VERSION = 1

SAMPLE_RATE = 16_000
# Qwen3-ASR encoder attention window: `n_window_infer` 800 mel frames at 10 ms.
ENCODER_WINDOW = 8 * SAMPLE_RATE
# A tail shorter than this is left out: too short to carry a word, and the
# encoder rejects items that produce no features.
MIN_ITEM = SAMPLE_RATE // 10

DEFAULT_ROLLBACK_WORDS = 3
DEFAULT_MIN_STEP_MS = 150

ASR_TAG = "<asr_text>"

# Same table as `language_name` in src/continuous.rs.
LANGUAGE_NAMES = {
    "en": "English",
    "zh": "Chinese",
    "de": "German",
    "fr": "French",
    "es": "Spanish",
    "it": "Italian",
    "pt": "Portuguese",
    "ru": "Russian",
    "ja": "Japanese",
    "ko": "Korean",
    "nl": "Dutch",
    "sv": "Swedish",
    "da": "Danish",
    "fi": "Finnish",
    "pl": "Polish",
    "tr": "Turkish",
    "ar": "Arabic",
    "hi": "Hindi",
}

_CHATML_LIKE_TOKEN = re.compile(r"<\|[^|]+\|>")


def language_name(code: str | None) -> str | None:
    """Qwen3-ASR language name for an ISO 639-1 code; None when unknown."""
    if not code:
        return None
    return LANGUAGE_NAMES.get(code.strip().lower())


def header_for(name: str) -> str:
    return f"language {name}{ASR_TAG}"


def header_language(header: str | None) -> str | None:
    """`language English<asr_text>` -> `English`."""
    if not header:
        return None
    lang = header[: -len(ASR_TAG)] if header.endswith(ASR_TAG) else header
    lang = lang.strip()
    if lang.startswith("language "):
        lang = lang[len("language ") :].strip()
    return lang or None


def sanitize_context(text: str | None) -> str:
    """Context-biasing text for the system turn.

    Blank context means an empty system turn (as the Rust client sends no
    system message, which its template renders as an empty one). ChatML-like
    control tokens and `<asr_text>` are stripped to a fixpoint, like vLLM's
    own `_sanitize_transcription_user_text`, so a client cannot inject turns.
    """
    if not text or not text.strip():
        return ""
    prev = None
    while prev != text:
        prev = text
        text = _CHATML_LIKE_TOKEN.sub("", text).replace(ASR_TAG, "")
    return text


def build_prompt(context: str, n_windows: int, prefix: str) -> str:
    """The text `CHAT_TEMPLATE` in src/continuous.rs renders for one decode."""
    return (
        f"<|im_start|>system\n{context}<|im_end|>\n<|im_start|>user\n<|audio_start|>"
        + "<|audio_pad|>" * n_windows
        + f"<|audio_end|><|im_end|>\n<|im_start|>assistant\n{prefix}"
    )


def window_bounds(n_samples: int) -> list[tuple[int, int]]:
    """Sample ranges of the encoder windows sent for `n_samples` of audio.

    Full 8 s windows, then the open tail unless it is shorter than 100 ms.
    """
    bounds = []
    for start in range(0, n_samples, ENCODER_WINDOW):
        end = min(start + ENCODER_WINDOW, n_samples)
        if end - start == ENCODER_WINDOW or end - start >= MIN_ITEM:
            bounds.append((start, end))
    return bounds


def max_tokens_for(n_samples: int) -> int:
    """A cap against runaway loops: 64 + 5 per second of audio."""
    return 64 + 5 * n_samples // SAMPLE_RATE


def _is_ws(c: str) -> bool:
    return c.isspace()


def settled_prefix(text: str, words: int) -> str:
    """`text` minus its last `words` words, cut at a word start so the kept
    part is byte-identical to what the model produced (then right-trimmed).
    Same as `settled_prefix` in src/continuous.rs."""
    if words == 0:
        return text.rstrip()
    starts = [
        i
        for i, c in enumerate(text)
        if not _is_ws(c) and (i == 0 or _is_ws(text[i - 1]))
    ]
    if len(starts) <= words:
        return ""
    return text[: starts[len(starts) - words]].rstrip()
