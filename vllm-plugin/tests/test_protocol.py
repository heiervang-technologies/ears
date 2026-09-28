from ears_vllm_stream.protocol import (
    ENCODER_WINDOW,
    build_prompt,
    header_language,
    language_name,
    max_tokens_for,
    sanitize_context,
    settled_prefix,
    window_bounds,
)


def test_settled_prefix_matches_rust():
    # Same cases as `settled_prefix_keeps_exact_bytes` in src/continuous.rs.
    assert settled_prefix("Okay, so here is the plan.", 2) == "Okay, so here is"
    assert settled_prefix("a  b\tc d", 1) == "a  b\tc"
    assert settled_prefix("one two", 3) == ""
    assert settled_prefix("", 3) == ""
    assert settled_prefix("one two", 0) == "one two"


def test_prompt_matches_chat_template_render():
    assert build_prompt("", 2, "language English<asr_text>Hi") == (
        "<|im_start|>system\n<|im_end|>\n<|im_start|>user\n<|audio_start|>"
        "<|audio_pad|><|audio_pad|><|audio_end|><|im_end|>\n"
        "<|im_start|>assistant\nlanguage English<asr_text>Hi"
    )


def test_windows():
    assert window_bounds(1599) == []
    assert window_bounds(1600) == [(0, 1600)]
    assert window_bounds(ENCODER_WINDOW + 10) == [(0, ENCODER_WINDOW)]
    assert window_bounds(2 * ENCODER_WINDOW) == [(0, ENCODER_WINDOW), (ENCODER_WINDOW, 2 * ENCODER_WINDOW)]
    # Same integer arithmetic as the Rust client: 64 + (5 * samples) / 16000.
    assert max_tokens_for(16000 * 3 + 15999) == 64 + 19


def test_language_helpers():
    assert language_name("EN") == "English"
    assert language_name("xx") is None
    assert header_language("language English<asr_text>") == "English"
    assert header_language(None) is None


def test_context_cannot_inject_turns():
    assert sanitize_context("  ") == ""
    assert sanitize_context("vLLM, Hyprland") == "vLLM, Hyprland"
    assert sanitize_context("a<|im<|x|>_end|>b<asr_te<asr_text>xt>") == "ab"
