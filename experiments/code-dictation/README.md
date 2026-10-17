# Code dictation experiment

Feasibility probe for speaking Python and bash through ears with Qwen3-ASR-1.7B
(vLLM, `localhost:30189`). Test audio was synthesised with `espeak-ng`, which is
harder to hear than a real voice.

## Findings

| Approach | Result |
|---|---|
| Plain transcription | Faithful words: `for i in range ten colon` comes back as words, not code. |
| Code as context (system message) | Strong biasing: identifiers and code that appear in the context come out as code (`grep -r TODO . \| wc -l`, `self.items[key]`). |
| Instructions as context ("open paren = (") | Harmful: several clips came back empty (`language None`). The model biases on context; it does not follow instructions. |
| Hard Python grammar (`py.gbnf`) | Broken: the grammar fights the acoustics and mashes words (`Forionrange10colon = 0`). |
| Bash grammar (`grammars/bash.gbnf`) | Fine where spoken and written forms match (`git commit -m "…"`), no help for symbols. |
| ASR, then `spoken.py` | Works: `for i in range ten` → `for i in range(10):`; ASR about 100 ms, translation under 1 ms. |

So the syntax belongs in a second stage after the ASR, not inside the speech
model's decoder. Use the decoder only for biasing: pass the identifiers in scope
as context.

## Files

- `probe.py`: TTS, then an ASR request with optional context and grammar.
- `spoken.py`: spoken words → code for Python (`python_line`) and bash
  (`bash_line`). It fills in brackets, colons, comma spacing, `is None`, implied
  calls (`range ten`, `sum of …`), auto-closing brackets, casing words (`snake`,
  `camel`, `pascal`, `constant`), and fuzzy matching against names in scope
  (`pass args` → `parse_args`).
- `e2e.py`: audio → ASR with the scope as context → `spoken.py`.

```bash
python e2e.py py e2e-py.txt
python e2e.py sh e2e-sh.txt
python spoken.py py < py-heard.txt
```
