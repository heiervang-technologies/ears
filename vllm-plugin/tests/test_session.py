import asyncio
import json
import random

import numpy as np
import pytest

from ears_vllm_stream.protocol import ENCODER_WINDOW, SAMPLE_RATE
from ears_vllm_stream.session import StreamSession

EN = "language English<asr_text>"


class FakeDecoder:
    """Scripted decoder. `replies` items: str, Exception, or callable(call)."""

    def __init__(self, *replies, block=False):
        self.replies = list(replies)
        self.calls = []
        self.block = block
        self.gates: list[asyncio.Event] = []
        self.cancelled = 0

    async def __call__(self, windows, prefix_text, max_tokens, *, context):
        call = {"windows": list(windows), "prefix": prefix_text,
                "max_tokens": max_tokens, "context": context}
        self.calls.append(call)
        try:
            if self.block:
                gate = asyncio.Event()
                self.gates.append(gate)
                await gate.wait()
        except asyncio.CancelledError:
            self.cancelled += 1
            raise
        reply = self.replies.pop(0) if self.replies else ""
        if callable(reply):
            reply = reply(call)
        if isinstance(reply, Exception):
            raise reply
        return reply

    def release(self):
        for g in self.gates:
            g.set()
        self.gates.clear()


class Harness:
    def __init__(self, decoder, max_audio_ms=90_000):
        self.sent = []
        self.decoder = decoder

        async def send(msg):
            json.dumps(msg)  # must be serialisable
            self.sent.append(msg)

        self.session = StreamSession(send, decoder, max_audio_ms=max_audio_ms)

    async def text(self, **msg):
        await self.session.on_text(json.dumps(msg))
        await settle()

    async def raw(self, s):
        await self.session.on_text(s)
        await settle()

    async def audio(self, seconds=None, samples=None, value=0):
        n = samples if samples is not None else int(seconds * SAMPLE_RATE)
        await self.session.on_binary(np.full(n, value, dtype="<i2").tobytes())
        await settle()

    def of(self, kind):
        return [m for m in self.sent if m["type"] == kind]


async def settle():
    for _ in range(20):
        await asyncio.sleep(0)


def run(coro):
    return asyncio.run(coro)


def test_partial_final_flow_with_forced_language():
    async def go():
        d = FakeDecoder(" Hello there friend", " my friend.", " friend. Bye.")
        h = Harness(d)
        await h.text(type="start", utterance=7, language="en", rollback_words=1,
                     context="vLLM, Hyprland")
        await h.audio(0.3)
        await h.audio(0.3)
        await h.text(type="end", utterance=7)
        assert [c["prefix"] for c in d.calls] == [
            EN, EN + "Hello there", EN + "Hello there my"]
        assert d.calls[0]["context"] == "vLLM, Hyprland"
        p1, p2 = h.of("partial")
        assert p1 == {"type": "partial", "utterance": 7, "seq": 1,
                      "text": "Hello there friend", "stable_chars": 11,
                      "language": "English", "audio_ms": 300,
                      "decode_ms": p1["decode_ms"]}
        assert p2["seq"] == 2 and p2["text"] == "Hello there my friend."
        assert p2["stable_chars"] == len("Hello there my")
        (final,) = h.of("final")
        assert final["text"] == "Hello there my friend. Bye."
        assert final["audio_ms"] == 600 and final["utterance"] == 7
        assert h.sent[-1] is final
        assert not h.of("error")
    run(go())


def test_language_detected_from_first_decode():
    async def go():
        d = FakeDecoder("language German<asr_text>Grüße du da", " da drüben")
        h = Harness(d)
        await h.text(type="start", utterance=1, rollback_words=1)
        await h.audio(0.2)
        await h.audio(0.2)
        assert d.calls[0]["prefix"] == ""
        assert d.calls[1]["prefix"] == "language German<asr_text>Grüße du"
        p1, p2 = h.of("partial")
        assert p1["text"] == "Grüße du da" and p1["language"] == "German"
        assert p2["text"] == "Grüße du da drüben"
        assert p2["stable_chars"] == len("Grüße du da".encode()) == 13  # bytes
    run(go())


def test_no_speech_header_is_not_locked_in():
    async def go():
        d = FakeDecoder("language None<asr_text>", "language English<asr_text>Hi")
        h = Harness(d)
        await h.text(type="start", utterance=1)
        await h.audio(0.2)
        await h.audio(0.2)
        assert [c["prefix"] for c in d.calls] == ["", ""]
        (p,) = h.of("partial")  # the empty first hypothesis is not sent
        assert p["text"] == "Hi" and p["language"] == "English"
    run(go())


def test_unknown_language_is_detected():
    async def go():
        d = FakeDecoder("language English<asr_text>x")
        h = Harness(d)
        await h.text(type="start", utterance=1, language="xx")
        await h.audio(0.2)
        assert d.calls[0]["prefix"] == ""
    run(go())


def test_settled_prefix_never_changes_within_an_utterance():
    words = "alpha beta gamma delta epsilon zeta eta theta iota kappa".split()
    rng = random.Random(4)

    def reply(call):
        # Arbitrary continuation: may repeat, shrink or rewrite open words.
        k = rng.randint(0, 4)
        return "".join(" " + rng.choice(words) for _ in range(k))

    async def go():
        d = FakeDecoder(*[reply] * 200)
        h = Harness(d)
        await h.text(type="start", utterance=3, language="en", rollback_words=2,
                     min_step_ms=0)
        for _ in range(60):
            await h.audio(0.05)
        await h.text(type="end", utterance=3)
        partials = h.of("partial")
        assert len(partials) > 5
        stables = []
        for p in partials:
            b = p["text"].encode()
            stable = b[: p["stable_chars"]]
            for s in stables:
                assert b.startswith(s)
            if stables:
                assert stable.startswith(stables[-1])
            stables.append(stable)
        (final,) = h.of("final")
        assert final["text"].encode().startswith(stables[-1])
        # Every decode forced the settled text as its prefix.
        for c in d.calls:
            assert c["prefix"].startswith(EN)
    run(go())


def test_cancel_sends_nothing_more_and_aborts_decode():
    async def go():
        d = FakeDecoder("Hello", block=True)
        h = Harness(d)
        await h.text(type="start", utterance=1, language="en")
        await h.audio(0.3)
        assert len(d.calls) == 1
        await h.text(type="cancel", utterance=1)
        assert d.cancelled == 1
        d.release()
        await settle()
        await h.audio(0.3)  # stray audio after cancel
        assert [m["type"] for m in h.sent] == ["error"]
        assert h.sent[0]["code"] == "bad_request"
        # A stale cancel/end for it is ignored.
        await h.text(type="cancel", utterance=1)
        await h.text(type="end", utterance=1)
        assert len(h.sent) == 1
    run(go())


def test_superseding_start_drops_old_utterance():
    async def go():
        d = FakeDecoder("old", "new words", block=True)
        h = Harness(d)
        await h.text(type="start", utterance=1, language="en")
        await h.audio(0.3)
        await h.text(type="start", utterance=2, language="en")
        assert d.cancelled == 1
        d.release()
        await settle()
        await h.audio(0.3)
        assert len(d.calls) == 2
        d.release()
        await settle()
        await h.text(type="end", utterance=2)
        d.release()
        await settle()
        assert all(m["utterance"] == 2 for m in h.sent)
        assert [m["type"] for m in h.sent] == ["partial", "final"]
        # The new utterance starts from scratch.
        assert d.calls[1]["prefix"] == EN
    run(go())


def test_end_while_decode_in_flight():
    async def go():
        d = FakeDecoder(" one two three four", " four five", block=True)
        h = Harness(d)
        await h.text(type="start", utterance=5, language="en", rollback_words=1)
        await h.audio(0.3)
        await h.audio(0.3)  # arrives mid-decode
        await h.text(type="end", utterance=5)
        assert len(d.calls) == 1  # still one decode in flight
        d.release()
        await settle()
        assert len(d.calls) == 2  # final decode right after, over all audio
        assert d.calls[1]["prefix"] == EN + "one two three"
        assert sum(len(w) for w in d.calls[1]["windows"]) == int(0.6 * SAMPLE_RATE)
        d.release()
        await settle()
        assert [m["type"] for m in h.sent] == ["partial", "final"]
        assert h.sent[1]["text"] == "one two three four five"
        # Nothing further after the final, even with a duplicate end.
        await h.text(type="end", utterance=5)
        assert len(h.sent) == 2
    run(go())


def test_too_long_cap():
    async def go():
        d = FakeDecoder("a", "a b", "a b c")
        h = Harness(d, max_audio_ms=1000)
        await h.text(type="start", utterance=1, language="en")
        await h.audio(0.6)
        await h.audio(0.6)
        await h.audio(0.6)
        errors = h.of("error")
        assert len(errors) == 1 and errors[0]["code"] == "too_long"
        assert errors[0]["utterance"] == 1
        n_partials = len(h.of("partial"))
        await h.text(type="end", utterance=1)
        assert len(h.of("partial")) == n_partials
        (final,) = h.of("final")
        assert final["audio_ms"] == 1000
        assert sum(len(w) for w in d.calls[-1]["windows"]) == SAMPLE_RATE
        assert d.calls[-1]["max_tokens"] == 64 + 5
    run(go())


def test_too_long_suppresses_partial_in_flight():
    async def go():
        d = FakeDecoder("a", "a b", block=True)
        h = Harness(d, max_audio_ms=1000)
        await h.text(type="start", utterance=1, language="en")
        await h.audio(0.5)
        await h.audio(0.8)  # passes the cap while decoding
        d.release()
        await settle()
        assert [m["code"] for m in h.of("error")] == ["too_long"]
        assert not h.of("partial")
    run(go())


def test_odd_byte_frames():
    async def go():
        d = FakeDecoder("x")
        h = Harness(d)
        await h.text(type="start", utterance=1, language="en")
        samples = (np.arange(4000) - 2000).astype("<i2")
        raw = samples.tobytes()
        pos = 0
        for size in [1, 3, 2, 5, 999, 1001, 4000, 7]:
            await h.session.on_binary(raw[pos:pos + size])
            pos += size
        await h.session.on_binary(raw[pos:])
        await settle()
        await h.text(type="end", utterance=1)
        w = np.concatenate(d.calls[-1]["windows"])
        np.testing.assert_array_equal(w, samples.astype(np.float32) / 32768.0)
    run(go())


def test_windows_and_max_tokens():
    async def go():
        d = FakeDecoder("a", "b")
        h = Harness(d)
        await h.text(type="start", utterance=1, language="en", min_step_ms=0)
        await h.audio(samples=ENCODER_WINDOW + 1599)
        c = d.calls[0]
        # The 1599-sample tail (< 100 ms) is left out.
        assert [len(w) for w in c["windows"]] == [ENCODER_WINDOW]
        assert c["max_tokens"] == 64 + 5 * 8
        await h.audio(samples=SAMPLE_RATE)
        c2 = d.calls[1]
        assert [len(w) for w in c2["windows"]] == [ENCODER_WINDOW, 1599 + SAMPLE_RATE]
        # The closed window is the same array, so it hashes identically.
        assert c2["windows"][0] is c["windows"][0]
        assert c2["max_tokens"] == 64 + 5 * 9
        assert c2["windows"][0].dtype == np.float32
    run(go())


def test_short_final_without_audio_needs_no_decode():
    async def go():
        d = FakeDecoder()
        h = Harness(d)
        await h.text(type="start", utterance=1, language="en")
        await h.audio(samples=100)
        await h.text(type="end", utterance=1)
        assert not d.calls
        assert h.sent == [{"type": "final", "utterance": 1, "text": "",
                           "audio_ms": 0, "decode_ms": 0}]
    run(go())


@pytest.mark.parametrize(
    "frame,utterance",
    [
        ("not json", None),
        ("[1, 2]", None),
        (json.dumps({"type": "bogus", "utterance": 4}), 4),
        (json.dumps({"type": "start"}), None),
        (json.dumps({"type": "start", "utterance": "7"}), None),
        (json.dumps({"type": "start", "utterance": 7, "rollback_words": -1}), 7),
        (json.dumps({"type": "start", "utterance": 7, "min_step_ms": "fast"}), 7),
        (json.dumps({"type": "start", "utterance": 7, "language": 5}), 7),
        (json.dumps({"type": "end", "utterance": 99}), 99),
        (json.dumps({"type": "cancel"}), None),
    ],
)
def test_bad_request_keeps_connection(frame, utterance):
    async def go():
        d = FakeDecoder("fine")
        h = Harness(d)
        await h.raw(frame)
        (err,) = h.sent
        assert err["type"] == "error" and err["code"] == "bad_request"
        assert err["utterance"] == utterance
        # Still usable.
        await h.text(type="start", utterance=100, language="en")
        await h.audio(0.3)
        assert h.of("partial")[0]["text"] == "fine"
    run(go())


def test_ids_must_increase_and_audio_needs_an_utterance():
    async def go():
        d = FakeDecoder()
        h = Harness(d)
        await h.audio(0.1)
        await h.audio(0.1)  # reported once, not per frame
        await h.text(type="start", utterance=5)
        await h.text(type="start", utterance=5)
        errs = h.of("error")
        assert [e["code"] for e in errs] == ["bad_request", "bad_request"]
        assert errs[0]["utterance"] is None and errs[1]["utterance"] == 5
        # The rejected duplicate did not disturb utterance 5.
        assert h.session.active.id == 5
    run(go())


def test_socket_close_cancels_decode():
    async def go():
        d = FakeDecoder("late", block=True)
        h = Harness(d)
        await h.text(type="start", utterance=1, language="en")
        await h.audio(0.3)
        await h.session.close()
        assert d.cancelled == 1
        assert h.session.active is None
        d.release()
        await settle()
        assert h.sent == []
    run(go())


def test_min_step_scheduling():
    async def go():
        d = FakeDecoder(*[f"w{i}" for i in range(20)], block=True)
        h = Harness(d)
        await h.text(type="start", utterance=1, language="en", min_step_ms=500)
        await h.audio(0.2)
        await h.audio(0.2)
        assert not d.calls  # 400 ms < 500 ms
        await h.audio(0.2)
        assert len(d.calls) == 1  # 600 ms
        await h.audio(0.3)
        await h.audio(0.3)  # 600 ms new audio, but one decode in flight
        assert len(d.calls) == 1
        d.release()
        await settle()
        # Enough new audio was buffered: the next decode starts at once.
        assert len(d.calls) == 2
        assert sum(len(w) for w in d.calls[1]["windows"]) == int(1.2 * SAMPLE_RATE)
        d.release()
        await settle()
        await h.audio(0.4)
        assert len(d.calls) == 2  # only 400 ms since the last decode started
        await h.audio(0.1)
        assert len(d.calls) == 3
        d.release()
        await settle()
    run(go())


def test_identical_partial_is_not_resent():
    async def go():
        d = FakeDecoder("same", "same", "same more")
        h = Harness(d)
        await h.text(type="start", utterance=1, language="en", rollback_words=5)
        for _ in range(3):
            await h.audio(0.2)
        assert [p["text"] for p in h.of("partial")] == ["same", "same more"]
        assert [p["seq"] for p in h.of("partial")] == [1, 2]
    run(go())


def test_decode_failure_then_recovery():
    async def go():
        d = FakeDecoder(RuntimeError("engine hiccup"), " back again", RuntimeError("x"))
        h = Harness(d)
        await h.text(type="start", utterance=2, language="en")
        await h.audio(0.2)
        (err,) = h.of("error")
        assert err["code"] == "internal" and err["utterance"] == 2
        await h.audio(0.2)
        (p,) = h.of("partial")
        assert p["text"] == "back again"
        # A failed final decode is reported instead of a final.
        await h.text(type="end", utterance=2)
        assert [e["code"] for e in h.of("error")] == ["internal", "internal"]
        assert not h.of("final")
        # The utterance is over; the connection carries on.
        d.replies = ["next"]
        await h.text(type="start", utterance=3, language="en")
        await h.audio(0.2)
        assert h.sent[-1]["type"] == "partial" and h.sent[-1]["utterance"] == 3
    run(go())
