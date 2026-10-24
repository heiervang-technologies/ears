"""Grammar-constrained stream decodes (bash mode).

A stream decode forces the settled transcript into the prompt, so the
model writes only a continuation and a grammar constrains only that. The
exact constraint would be the grammar's derivative by the settled text, a
new grammar per decode; xgrammar compiles one in seconds on Qwen3's
151k-token vocab, which no live decode can wait for. Instead the grammar
names a few fixed entry rules, the settled text is scanned forward to pick
one, and each resulting grammar is compiled once and stays in vLLM's cache:

* nothing settled yet: `root` (the model writes the language header too);
* settled text ends a word: `cont`;
* settled text ends inside an open "..." or '...' string: `cont_dq` or
  `cont_sq`.

A grammar without the rule a decode needs leaves that decode unconstrained
rather than forcing text into a rule that cannot follow what settled.
"""

from __future__ import annotations

import re
from importlib import resources

MAX_GRAMMAR_BYTES = 64 * 1024

_RULE = re.compile(r"^\s*([A-Za-z_][\w-]*)\s*::=", re.M)
_ROOT = re.compile(r"^(\s*)root(\s*::=)", re.M)
# The original `root` is kept under this name: other rules may refer to it.
_RENAMED_ROOT = "ears_unforced_root"

ENTRY_FOR_QUOTE = {None: "cont", '"': "cont_dq", "'": "cont_sq"}


def builtin(name: str) -> str:
    """A grammar shipped with the plugin (`bash`)."""
    return resources.files(__package__).joinpath("grammars", f"{name}.gbnf").read_text()


def open_quote(text: str) -> str | None:
    """The quote character `text` ends inside of, if any (bash quoting, no
    escapes inside single quotes, backslash escapes elsewhere)."""
    state = None
    escaped = False
    for c in text:
        if escaped:
            escaped = False
        elif state == "'":
            if c == "'":
                state = None
        elif c == "\\":
            escaped = True
        elif state == '"':
            if c == '"':
                state = None
        elif c in "\"'":
            state = c
    return state


class StreamGrammar:
    """One client grammar and its per-entry variants (grammar texts)."""

    def __init__(self, text: str):
        if len(text.encode("utf-8")) > MAX_GRAMMAR_BYTES:
            raise ValueError(f"grammar is over {MAX_GRAMMAR_BYTES} bytes")
        self.rules = set(_RULE.findall(text))
        if "root" not in self.rules:
            raise ValueError("grammar has no root rule")
        self.text = text
        renamed = _ROOT.sub(rf"\1{_RENAMED_ROOT}\2", text, count=1)
        self._variants = {
            entry: f"{renamed.rstrip()}\n\nroot ::= {entry}\n"
            for entry in ENTRY_FOR_QUOTE.values()
            if entry in self.rules
        }

    def for_prefix(self, settled: str) -> str | None:
        """Grammar text for a decode continuing `settled` (None: unconstrained)."""
        if not settled:
            return self.text
        return self._variants.get(ENTRY_FOR_QUOTE[open_quote(settled)])

    def variants(self) -> list[str]:
        """Every grammar text a decode may use, for warming the compile cache."""
        return [self.text, *self._variants.values()]


_cache: dict[str, StreamGrammar] = {}


def parse(text: str) -> StreamGrammar:
    """Parsed grammar, shared across connections (a client sends the same
    text with every utterance)."""
    grammar = _cache.get(text)
    if grammar is None:
        grammar = StreamGrammar(text)
        if len(_cache) >= 16:
            _cache.pop(next(iter(_cache)))
        _cache[text] = grammar
    return grammar
