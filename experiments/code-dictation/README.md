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

## Try it in Neovim

```bash
./try.sh some_file.py      # or a .sh file; any other filetype gets plain text
```

- `Alt-v` (normal or insert mode; Space v is NvChad's vertical terminal): start talking. Press
  it again to stop.
- While you talk, the recording so far is re-transcribed every 500 ms and the
  translated line shows as yellow ghost text where it will land, with the words
  heard after it.
- The buffer's names are sent as context, the line is translated, and it is
  inserted with indentation: it replaces a blank current line, otherwise it goes
  below. Each utterance is one undo step (`u`).
- About a second later, beam-search hypotheses that translate differently (and
  the plain words, as an escape hatch) open as a completion menu over the line:
  `Ctrl-n`/`Ctrl-p` to pick, `Esc` to keep what was inserted.
- With the ears VAD running (needs the input claim, ears PR #215), no key is
  needed: while this Neovim has focus (on anything but a terminal or picker)
  it holds the claim, so ears types nothing and sends the words here. Partials show as
  ghost text, each utterance is translated and inserted, and ears commands
  act on the buffer: "enter" or "new line" opens a line, "scratch that"
  undoes, "back N" drops N words, "backtrack" clears the line. Focus another
  window and ears types normally there.
- Neovim's mode decides what an utterance is:
  - **insert mode:** dictation, or an insert command said on its own ("clear
    line", "backspace", "delete word");
  - **any other mode:** a Vim command from a closed list ("dee dee", "why
    why", "down five", "paste", "save", "go to line forty two"). Anything not
    on the list is ignored, never inserted.
- Inside dictation, "new line" breaks the line; a leading "indent" or
  "dedent" shifts it a level. "Double new line", "dedent twice" and
  "backspace three times" repeat. "Dunder init" is `__init__`.
- Surrounding syntax while dictating:
  - strings: "string hello world" → `"hello world"`, ending at "end string"
    or the end of the line; "f string total colon brace total" →
    `f"total: {total}"`; "quote ok quote" also works;
  - containers: "brackets one comma two" → `[1, 2]`, "braces string host end
    string colon string localhost" → `{"host": "localhost"}`, "parens a plus b
    close times c" → `(a + b) * c`, "empty list" / "empty dict";
  - calls and indexing: "print of x", "range ten", "items index key";
  - "close" closes the innermost bracket; anything still open closes at the
    end of the line. "open paren" / "close paren" etc. also work.
- Surrounding existing code, in normal mode (through nvim-surround): "quote
  word", "paren word", "quote line", "unquote", "delete parens", "single
  quotes", "change string", "change args".
- All phrases are configurable in `~/.config/ears/code.toml`; see
  `code.toml.example`.
- Push-to-talk without the VAD: put the VAD in standby first ("ears off").
- `dictate.py` is the bridge: `python3 dictate.py py file.wav scope.txt`.
