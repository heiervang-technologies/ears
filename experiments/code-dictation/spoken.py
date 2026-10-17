"""Spoken code -> source code, the second stage after ASR.

The ASR model transcribes words faithfully ("for i in range ten colon"); this
layer turns them into code and fills in what goes without saying: brackets,
colons, commas, identifier casing, and names already in scope.
"""
import difflib
import re

NUMBERS = {w: str(i) for i, w in enumerate(
    "zero one two three four five six seven eight nine ten eleven twelve".split())}
NUMBERS.update({"twenty": "20", "hundred": "100", "thousand": "1000"})

# Multi-word phrases first; longest match wins.
SYMBOLS = {
    "open paren": "(", "close paren": ")", "paren": "(", "open bracket": "[",
    "close bracket": "]", "open brace": "{", "close brace": "}",
    "equals equals": "==", "double equals": "==", "not equals": "!=",
    "is not": "is not", "greater than": ">", "less than": "<",
    "plus equals": "+=", "minus equals": "-=", "single quote": "'",
    "dash dash": "--", "and and": "&&", "and then": "&&", "or else": "||",
    "colon": ":", "comma": ",", "dot": ".", "equals": "=", "plus": "+",
    "minus": "-", "times": "*", "star": "*", "slash": "/", "pipe": "|",
    "quote": '"', "dollar": "$", "dash": "-", "tilde": "~", "semicolon": ";",
    "percent": "%", "at sign": "@", "hash": "#", "underscore": "_",
}
CASES = {"snake", "camel", "pascal", "constant", "smash", "kebab", "spell"}
MISHEARD = {"closed": "close", "e": "", "z": "", "non": "none", "f": "def", "def.": "def"}

PY_KEYWORDS = {"in", "is", "not", "and", "or", "as", "if", "else", "for", "lambda",
               "None", "True", "False", "await", "yield", "from", "import"}
PY_CALLABLES = {"range", "len", "print", "str", "int", "float", "list", "dict", "set",
                "tuple", "open", "enumerate", "zip", "sorted", "sum", "min", "max",
                "isinstance", "repr", "type", "any", "all"}
PY_LITERALS = {"none": "None", "true": "True", "false": "False"}
# Statement starters that get misheard, or that are easier to say.
HEADS = {"they": "def", "deaf": "def", "death": "def", "define": "def",
         "function": "def", "alif": "elif", "elf": "elif", "else if": "elif"}
PY_BLOCKS = {"if", "elif", "while", "for", "with", "def", "class", "except"}


def words_of(asr_text):
    """Lowercase, drop ASR punctuation, spell numbers as digits."""
    text = asr_text.lower().replace("-", " dash ")
    text = re.sub(r"[.,!?\"]", " ", text)
    out = []
    for w in text.split():
        w = MISHEARD.get(w, w) if out or w != "f" else "def"
        if w:
            out.append(NUMBERS.get(w, w))
    return out


def lex(words, scope):
    """Words -> tokens: ("sym", s) | ("word", w) | ("num", n) | ("name", n)."""
    tokens, i = [], 0
    compact = {re.sub(r"[^a-z0-9]", "", s.lower()): s for s in scope}
    while i < len(words):
        for n in (3, 2, 1):  # longest symbol / scope match first
            phrase = " ".join(words[i:i + n])
            if len(words) - i >= n and phrase in SYMBOLS:
                tokens.append(("sym", SYMBOLS[phrase]))
                break
            joined = "".join(words[i:i + n])
            if len(words) - i >= n and n > 1 and (
                    m := [joined] if joined in compact
                    else difflib.get_close_matches(joined, compact, 1, 0.8)):
                tokens.append(("name", compact[m[0]]))  # "pass args" -> parse_args
                break
        else:
            n = 1
            w = words[i]
            if w in CASES and i + 1 < len(words):
                tokens.append(("case", w))
            elif w.isdigit():
                tokens.append(("num", w))
            elif w in compact:
                tokens.append(("name", compact[w]))
            elif len(w) >= 4 and (m := difflib.get_close_matches(w, compact, 1, 0.75)):
                tokens.append(("name", compact[m[0]]))  # "args" -> argv when in scope
            else:
                tokens.append(("word", w))
        i += n
    return tokens


def case(style, parts):
    if style == "camel":
        return parts[0] + "".join(p.title() for p in parts[1:])
    if style == "pascal":
        return "".join(p.title() for p in parts)
    if style == "constant":
        return "_".join(parts).upper()
    if style in ("smash", "spell"):
        return "".join(parts)
    if style == "kebab":
        return "-".join(parts)
    return "_".join(parts)


def python_expr(tokens, condition=False):
    """Render an expression; adjacent plain words become one snake_case name."""
    out, opened, i = [], [], 0

    def emit(piece, space=True):
        if out and space and not out[-1].endswith(("(", "[", "{", ".")) and piece not in ")]},.:([":
            out.append(" ")
        out.append(piece)

    while i < len(tokens):
        kind, val = tokens[i]
        if kind == "case":
            j = i + 1
            while j < len(tokens) and tokens[j][0] == "word":
                j += 1
            emit(case(val, [t[1] for t in tokens[i + 1:j]]))
            i = j
            continue
        if kind == "word" and val in PY_LITERALS:
            emit(PY_LITERALS[val])
        elif kind == "word" and val in ("of", "call", "with") and out:
            emit("(", space=False)
            opened.append(")")
        elif kind == "word" and val in ("index", "sub", "at") and out:
            emit("[", space=False)
            opened.append("]")
        elif kind == "word" and val in PY_KEYWORDS:
            emit(val)
        elif kind == "word" and val in PY_CALLABLES:
            emit(val)
            if i + 1 < len(tokens) and tokens[i + 1] == ("word", "of"):
                i += 1  # "sum of x" already says the call
            if i + 1 < len(tokens) and tokens[i + 1] != ("sym", "("):
                emit("(", space=False)  # "range ten" -> range(10)
                opened.append(")")
        elif kind == "word":
            j = i
            while j < len(tokens) and tokens[j][0] == "word" and tokens[j][1] not in (
                    PY_KEYWORDS | PY_CALLABLES | set(PY_LITERALS) | {"of", "index", "call", "sub", "at"}):
                j += 1
            emit("_".join(t[1] for t in tokens[i:j]))
            i = j
            continue
        elif kind == "sym":
            if val == "=" and condition:
                val = "=="
            if val in ("(", "["):
                opened.append(")" if val == "(" else "]")
                emit(val, space=False)
            elif val in (")", "]") and opened:
                opened.pop()
                emit(val)
            elif val in (",", ".", ":"):
                emit(val)
            else:
                emit(val)
        else:
            emit(val)
        i += 1
    while opened:  # unbalanced brackets close themselves at the end
        emit(opened.pop())
    text = "".join(out)
    text = re.sub(r",(?=\S)", ", ", text)
    text = re.sub(r"== None\b", "is None", text).replace("!= None", "is not None")
    return text.rstrip(":").strip()


def python_line(asr_text, scope=()):
    words = words_of(asr_text)
    if not words:
        return ""
    head, rest = HEADS.get(words[0], words[0]), words[1:]
    toks = lex(rest, scope)
    while toks and toks[-1] == ("sym", ":"):
        toks.pop()  # the colon goes without saying
    if head == "def":
        names = [t for t in toks]
        split = next((k for k, t in enumerate(names) if t in (("word", "taking"), ("word", "with"), ("sym", "("))), len(names))
        name = python_expr(names[:split])
        args = [t for t in names[split + 1:] if t not in (("sym", ")"), ("sym", ":"))]
        params = ", ".join(python_expr([t]) for t in args if t != ("sym", ","))
        return f"def {name}({params}):"
    if head == "class":
        name = case("pascal", [t[1] for t in toks if t[0] != "sym"])
        return f"class {name if len(toks) > 1 else toks[0][1] if toks[0][0] == 'name' else name}:"
    if head in ("else", "try", "finally"):
        return f"{head}:"
    if head in PY_BLOCKS:
        return f"{head} {python_expr(toks, condition=head in ('if', 'elif', 'while'))}:"
    if head in ("return", "import", "from", "raise", "assert", "yield", "del"):
        return f"{head} {python_expr(toks, condition=head == 'assert')}".strip()
    return python_expr(lex(words, scope))


def bash_line(asr_text, scope=()):
    words = words_of(asr_text)
    out, i, quoting = [], 0, False
    glue = False  # next piece attaches without a space
    while i < len(words):
        two = " ".join(words[i:i + 2])
        sym = SYMBOLS.get(two) if two in SYMBOLS else SYMBOLS.get(words[i])
        width = 2 if two in SYMBOLS else 1
        if sym == '"':
            if quoting:
                out[-1] = out[-1].rstrip()
                out.append('"')
            else:
                out.append(' "' if out else '"')
                glue = True
            quoting = not quoting
        elif quoting:
            out.append(("" if glue else " ") + words[i])
            glue = False
        elif sym in ("-", "--"):
            nxt = words[i + width] if i + width < len(words) else ""
            flag = sym + (nxt if sym == "-" else nxt)
            out.append(" " + flag)
            i += width + 1
            continue
        elif sym == "$":
            nxt = words[i + width] if i + width < len(words) else ""
            out.append((" " if out and not glue else "") + "$" + nxt.upper())
            glue = False
            i += width + 1
            continue
        elif sym == "~":
            out.append((" " if out and not glue else "") + "~")
            glue = True
        elif sym == "=":
            if out and out[-1].strip().isidentifier() and "export" in "".join(out):
                out[-1] = out[-1].upper()
            out.append("=")
            glue = True
        elif sym in ("/", ".", ":"):
            out.append(sym)
            glue = True
        elif sym in ("|", "&&", "||", ";", ">", "<"):
            out.append(f" {sym}")
            glue = False
        elif sym:
            out.append(" " + sym)
        else:
            w = words[i]
            out.append(("" if glue or not out else " ") + w)
            glue = False
        i += width
    return "".join(out).strip()


if __name__ == "__main__":
    import sys
    lang = sys.argv[1]
    for line in sys.stdin:
        line = line.rstrip("\n")
        print(f"{line!r:60} -> {(python_line if lang == 'py' else bash_line)(line)!r}")
