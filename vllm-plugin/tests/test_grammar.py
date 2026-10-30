import asyncio
import json

import pytest

from ears_vllm_stream import grammar as g
from ears_vllm_stream.session import StreamSession

from test_session import EN, FakeDecoder, Harness, run

BASH = g.builtin("bash")


def test_open_quote_scans_bash_quoting():
    assert g.open_quote("git status") is None
    assert g.open_quote('git commit -m "fix the') == '"'
    assert g.open_quote('git commit -m "fix" && echo \'hi') == "'"
    assert g.open_quote("echo 'it\\'") is None  # no escapes in single quotes
    assert g.open_quote('echo "a \\" b') == '"'
    assert g.open_quote('echo \\"') is None


def test_entry_rule_follows_the_settled_text():
    sg = g.parse(BASH)
    assert sg.for_prefix("") == BASH  # nothing settled: the full grammar
    cont = sg.for_prefix("git status")
    assert cont.rstrip().endswith("root ::= cont")
    # The original root survives under another name, defined exactly once.
    assert cont.count("root ::=") == 1
    assert "ears_unforced_root" in cont
    assert sg.for_prefix('git commit -m "fix the').rstrip().endswith("root ::= cont_dq")
    assert sg.for_prefix("echo 'hi").rstrip().endswith("root ::= cont_sq")
    assert len(sg.variants()) == 4


def test_grammar_without_continuation_rules_leaves_forced_decodes_free():
    sg = g.parse('root ::= "ls"')
    assert sg.for_prefix("") == 'root ::= "ls"'
    assert sg.for_prefix("ls") is None


def test_grammar_needs_a_root_and_a_size_cap():
    with pytest.raises(ValueError):
        g.StreamGrammar('body ::= "ls"')
    with pytest.raises(ValueError):
        g.StreamGrammar("root ::= " + '"a" ' * 40_000)


class WarmingDecoder(FakeDecoder):
    def __init__(self, *replies):
        super().__init__(*replies)
        self.warmed = []

    def warm(self, texts):
        self.warmed.extend(texts)


def test_session_constrains_each_decode_and_warms_variants():
    async def go():
        d = WarmingDecoder(EN + "git status -s", " -s && ls", " ls -la.")
        h = Harness(d)
        await h.text(type="start", utterance=1, language="en", rollback_words=1,
                     min_step_ms=0, grammar=BASH)
        assert len(d.warmed) == 4
        await h.audio(0.5)
        await h.audio(0.5)
        await h.text(type="end", utterance=1)
        first, second, final = d.calls
        assert first["prefix"] == "" and first["grammar"] == BASH
        assert second["prefix"] == EN + "git status"
        assert second["grammar"].rstrip().endswith("root ::= cont")
        # "git status -s &&" settled: a word ended, still `cont`.
        assert final["prefix"] == EN + "git status -s &&"
        assert final["grammar"] == second["grammar"]
        (fin,) = h.of("final")
        assert fin["text"] == "git status -s && ls -la."
    run(go())


def test_utterance_without_grammar_passes_none():
    async def go():
        d = FakeDecoder(EN + "hello")
        h = Harness(d)
        await h.text(type="start", utterance=1, language="en", min_step_ms=0)
        await h.audio(0.5)
        assert "grammar" not in d.calls[0]
    run(go())


def test_bad_grammar_is_a_bad_request():
    async def go():
        h = Harness(FakeDecoder())
        await h.text(type="start", utterance=1, grammar=7)
        await h.text(type="start", utterance=2, grammar='body ::= "x"')
        errors = h.of("error")
        assert [e["utterance"] for e in errors] == [1, 2]
        assert all(e["code"] == "bad_request" for e in errors)
    run(go())
