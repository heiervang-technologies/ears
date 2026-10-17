"""Spoken code -> source code, the second stage after ASR.

The ASR model transcribes words faithfully ("for i in range ten colon"); this
layer turns them into code and fills in what goes without saying: brackets,
colons, commas, identifier casing, and names already in scope.
"""
import difflib
import re

NUMBERS = {w: str(i) for i, w in enumerate(
    "zero one two three four five six seven eight nine ten eleven twelve".split())}
NUMBERS.update({w: str(10 * (i + 2)) for i, w in enumerate(
    "twenty thirty forty fifty sixty seventy eighty ninety".split())})
NUMBERS.update({"hundred": "100", "thousand": "1000"})

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
CASES = {"snake", "camel", "pascal", "constant", "smash", "kebab", "spell", "dunder"}
MISHEARD = {"closed": "close", "non": "none", "def.": "def"}

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


def join_numbers(words):
    """["40", "2"] -> ["42"]: "forty two" is one number."""
    out = []
    for w in words:
        if out and w.isdigit() and len(w) == 1 and out[-1].isdigit() and len(out[-1]) == 2 \
                and out[-1].endswith("0") and w != "0":
            out[-1] = str(int(out[-1]) + int(w))
        else:
            out.append(w)
    return out


def words_of(asr_text):
    """Lowercase, drop ASR punctuation, spell numbers as digits."""
    text = asr_text.lower().replace("-", " dash ")
    text = re.sub(r"[.,!?\"]", " ", text)
    out = []
    for w in text.split():
        w = MISHEARD.get(w, w) if out or w != "f" else "def"
        if w:
            out.append(NUMBERS.get(w, w))
    return join_numbers(out)


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
    if style == "dunder":
        return "__" + "_".join(parts) + "__"  # "dunder init" -> __init__
    if style == "kebab":
        return "-".join(parts)
    return "_".join(parts)


# Surrounding syntax, as words. Openers close themselves at the end of the
# line, or at "close" (the innermost one).
OPENERS = {"brackets": ("[", "]"), "square": ("[", "]"), "braces": ("{", "}"),
           "curly": ("{", "}"), "parens": ("(", ")")}
EMPTY = {"list": "[]", "dict": "{}", "string": '""', "tuple": "()", "set": "set()"}
STRING_ENDS = {("end", "string"), ("end", "quote"), ("close", "quote")}
RESERVED = {"of", "index", "call", "sub", "at", "close", "string", "empty"} | set(OPENERS)


def string_literal(tokens, i, f=False):
    """Words after "string" (or a quote) up to the closing quote, "end
    string", or the end of the line -> a string literal. In an f-string,
    "brace NAME" is {name}. Returns (literal, next index)."""
    parts, j = [], i
    while j < len(tokens):
        kind, val = tokens[j]
        if (kind, val) == ("sym", '"'):
            j += 1
            break
        if kind == "word" and j + 1 < len(tokens) and (val, tokens[j + 1][1]) in STRING_ENDS:
            j += 2
            break
        if f and (kind, val) == ("word", "brace") and j + 1 < len(tokens):
            parts.append("{" + str(tokens[j + 1][1]) + "}")
            j += 2
            continue
        glue = kind == "sym" and val in ".,:;!?/"
        parts.append(("\0" if glue else "") + str(val))
        j += 1
    text = " ".join(parts).replace(" \0", "").replace("\0", "")
    return ("f" if f else "") + '"' + text + '"', j


def python_expr(tokens, condition=False):
    """Render an expression; adjacent plain words become one snake_case name."""
    out, opened, i = [], [], 0

    def emit(piece, space=True):
        """space: True (unless punctuation), False, or "force"."""
        if out and not out[-1].endswith(("(", "[", "{", ".")) and (
                space == "force" or space and piece.strip() not in ")]},.:(["):
            out.append(" ")
        out.append(piece)

    def open_(o, c, space=False):
        emit(o, space=space)
        opened.append(c)

    while i < len(tokens):
        kind, val = tokens[i]
        nxt = tokens[i + 1] if i + 1 < len(tokens) else (None, None)
        if kind == "case":
            j = i + 1
            while j < len(tokens) and tokens[j][0] == "word":
                j += 1
            emit(case(val, [t[1] for t in tokens[i + 1:j]]))
            i = j
            continue
        if (kind, val) == ("sym", '"') or (kind, val) == ("word", "string"):
            literal, i = string_literal(tokens, i + 1)
            emit(literal)
            continue
        if (kind, val) == ("word", "f") and nxt == ("word", "string"):
            literal, i = string_literal(tokens, i + 2, f=True)
            emit(literal)
            continue
        if (kind, val) == ("word", "empty") and nxt[1] in EMPTY:
            emit(EMPTY[nxt[1]])
            i += 2
            continue
        if kind == "word" and val in PY_LITERALS:
            emit(PY_LITERALS[val])
        elif kind == "word" and val in OPENERS:
            open_(*OPENERS[val], space="force")
        elif (kind, val) == ("word", "close") and opened:
            emit(opened.pop())
        elif kind == "word" and val in ("of", "call", "with") and out:
            open_("(", ")")
        elif kind == "word" and val in ("index", "sub", "at") and out:
            open_("[", "]")
        elif kind == "word" and val in PY_KEYWORDS:
            emit(val)
        elif kind == "word" and val in PY_CALLABLES:
            emit(val)
            if nxt == ("word", "of"):
                i += 1  # "sum of x" already says the call
            if i + 1 < len(tokens) and tokens[i + 1] != ("sym", "("):
                open_("(", ")")  # "range ten" -> range(10)
        elif kind == "word":
            j = i
            while j < len(tokens) and tokens[j][0] == "word" and tokens[j][1] not in (
                    PY_KEYWORDS | PY_CALLABLES | set(PY_LITERALS) | RESERVED):
                j += 1
            emit("_".join(t[1] for t in tokens[i:max(j, i + 1)]))
            i = max(j, i + 1)
            continue
        elif kind == "sym":
            if val == "=" and condition:
                val = "=="
            if val in ("(", "[", "{"):
                open_(val, {"(": ")", "[": "]", "{": "}"}[val], space="force" if val == "{" else False)
            elif val in (")", "]", "}") and opened:
                opened.pop()
                emit(val)
            elif val == ":" and opened and opened[-1] == "}":
                emit(": ")  # dict entries
            else:
                emit(val)
        else:
            emit(val)
        i += 1
    while opened:  # unbalanced brackets close themselves at the end
        emit(opened.pop())
    text = "".join(out)
    text = re.sub(r",(?=\S)", ", ", text)
    text = re.sub(r": +", ": ", text)
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



# Spoken layout words, overridable in ~/.config/ears/code.toml:
#   new_line = ["new line", "newline", "next line"]
#   indent = ["indent"]
#   dedent = ["dedent", "the indent", "outdent", "unindent"]
LAYOUT = {"new_line": ["new line", "newline", "next line", "you lie"],  # heard for "newline"
          "indent": ["indent"],
          # "dedent" is often heard as "the indent"; "outdent" and "unindent"
          # are harder to mishear.
          "dedent": ["dedent", "the indent", "de indent", "deindent", "outdent", "unindent"]}


# Normal-mode commands: phrase -> keys. A count ("down five", "triple dee
# dee", "paste three times") goes before the keys, except for ":" commands,
# which run that many times. Overridable under [normal] in code.toml.
NORMAL = {
    "delete line": "dd", "diddy": "dd", "didi": "dd", "deedee": "dd", "teddy": "dd",
    "clear line": "cc<Esc>", "clear": "cc<Esc>", "erase": "cc<Esc>",
    "yank line": "yy", "copy line": "yy", "change line": "cc",
    "paste": "p", "paste above": "P", "undo": "u", "redo": "<C-r>",
    "insert": "i", "append": "A", "open": "o", "open below": "o", "open above": "O",
    "down": "j", "up": "k", "left": "h", "right": "l", "word": "w", "back word": "b",
    "end": "$", "start": "^", "top": "gg", "bottom": "G", "line": "G", "go to line": "G",
    "center": "zz", "join": "J", "indent": ">>", "dedent": "<<", "the indent": "<<",
    "outdent": "<<", "unindent": "<<", "next": "n",
    "previous": "N",
    # Surrounding existing code (nvim-surround) and editing inside it.
    "quote word": 'ysiw"', "single quote word": "ysiw'", "paren word": "ysiw)",
    "bracket word": "ysiw]", "brace word": "ysiw}", "quote line": 'yss"', "paren line": "yss)",
    "unquote": 'ds"', "delete quotes": 'ds"', "delete parens": "ds)", "delete brackets": "ds]",
    "single quotes": "cs\"'", "double quotes": "cs'\"",
    "change string": 'ci"', "change inside quotes": 'ci"', "change args": "ci(",
    "change inside parens": "ci(", "change inside brackets": "ci[", "delete args": "di(",
    "visual": "v", "visual line": "V", "escape": "<Esc>", "save": ":w<CR>",
}
# Spoken letter names; an utterance made only of letters is typed as keys:
# "dee dee" -> dd, "why why" -> yy, "gee gee" -> gg.
LETTERS = {"dee": "d", "why": "y", "gee": "g", "see": "c", "pee": "p", "you": "u",
           "ex": "x", "jay": "j", "kay": "k", "oh": "o", "eye": "i", "double you": "w",
           "bee": "b", "ee": "e", "en": "n", "zee": "z", "zed": "z", "are": "r", "vee": "v"}
# Insert-mode commands, said as the whole utterance: phrase -> keys. A count
# repeats them ("backspace three times"). Overridable under [insert].
INSERT = {
    "clear line": "<Esc>cc", "clear": "<Esc>cc", "backspace": "<BS>",
    "delete word": "<C-w>", "delete back": "<C-w>", "escape": "<Esc>", "normal mode": "<Esc>",
}
REPEATS = {"double": 2, "triple": 3, "quadruple": 4, "twice": 2, "thrice": 3}


def config():
    import os
    import tomllib
    path = os.path.expanduser("~/.config/ears/code.toml")
    conf = {**LAYOUT, "normal": dict(NORMAL), "insert": dict(INSERT)}
    if os.path.exists(path):
        with open(path, "rb") as f:
            user = tomllib.load(f)
        conf.update({k: v for k, v in user.items() if k in LAYOUT})
        conf["normal"].update(user.get("normal", {}))
        conf["insert"].update(user.get("insert", {}))
    return conf


def layout():
    return config()


def plain(text):
    return " ".join(join_numbers([NUMBERS.get(w, w) for w in re.sub(r"[^a-z0-9 ]", " ", text.lower()).split()]))


def take_count(words):
    """Strip a repeat from either end: "triple x", "x twice", "x 3 times",
    "x times 3", "down 5". Returns (count or None, remaining words)."""
    count = None
    if words and words[0] in REPEATS:
        count, words = REPEATS[words[0]], words[1:]
    elif words and words[-1] in REPEATS:
        count, words = REPEATS[words[-1]], words[:-1]
    elif len(words) > 2 and words[-1] == "times" and words[-2].isdigit():
        count, words = int(words[-2]), words[:-2]
    elif len(words) > 2 and words[-2] == "times" and words[-1].isdigit():
        count, words = int(words[-1]), words[:-2]
    elif len(words) > 1 and words[-1].isdigit():
        count, words = int(words[-1]), words[:-1]
    return count, words


def normal_keys(heard):
    """A normal-mode utterance -> keys, or None when it is not a command
    (then nothing happens: dictation never lands in normal mode)."""
    conf = config()
    count, words = take_count(plain(heard).split())
    phrase = " ".join(words)
    table = {plain(k): v for k, v in conf["normal"].items()}
    keys = table.get(phrase)
    if keys is None and words:
        spelled = re.sub(r"\bdouble you\b", "w", phrase).split()
        letters = [LETTERS.get(w, w if len(w) == 1 and w.isalpha() else None) for w in spelled]
        if all(letters):
            keys = "".join(letters)
        elif len(words) == 1 and re.fullmatch(r"[a-z]{2,3}", words[0]) and len(set(words[0])) == 1:
            keys = words[0]  # heard "dd" or "yy" as one word
    if keys is None:
        return None
    if count and keys.startswith(":"):
        return keys * count
    return (str(count) if count else "") + keys


def insert_keys(heard):
    """A whole insert-mode utterance that is a command -> keys, else None."""
    count, words = take_count(plain(heard).split())
    keys = {plain(k): v for k, v in config()["insert"].items()}.get(" ".join(words))
    return keys * min(count or 1, 50) if keys else None


def expand_repeats(text, phrases):
    """ "double new line" / "new line twice" / "new line 3 times" -> repeated."""
    words = plain(text)
    for p in sorted(phrases, key=len, reverse=True):
        q = re.escape(plain(p))
        words = re.sub(rf"\b(double|triple|quadruple) ({q})\b",
                       lambda m: " ".join([m.group(2)] * REPEATS[m.group(1)]), words)
        words = re.sub(rf"\b({q}) (twice|thrice)\b",
                       lambda m: " ".join([m.group(1)] * REPEATS[m.group(2)]), words)
        words = re.sub(rf"\b({q}) (\d+) times\b",
                       lambda m: " ".join([m.group(1)] * min(int(m.group(2)), 10)), words)
    return words


def code_lines(lang, heard, scope=()):
    """One utterance -> lines: [{"code", "shift"}]. A new-line phrase splits
    it; a trailing one leaves an empty line to continue on. Leading indent and
    dedent phrases shift that line by one level each (+1 / -1)."""
    words = layout()
    translate = {"py": python_line, "sh": bash_line}.get(lang, lambda t, s: t.strip())
    layout_phrases = words["new_line"] + words["indent"] + words["dedent"]
    if any(r in heard.lower() for r in list(REPEATS) + ["times"]):
        heard = expand_repeats(heard, layout_phrases)
    breaks = "|".join(re.escape(p) for p in words["new_line"])
    parts = [p.strip(" .,!?") for p in re.split(rf"\b(?:{breaks})\b[.,!?]*", heard, flags=re.I)]
    shifts = [(p, 1) for p in words["indent"]] + [(p, -1) for p in words["dedent"]]
    lines = []
    for k, part in enumerate(parts):
        shift, text = 0, " ".join(re.sub(r"[.,!?]", " ", part).split())
        while True:
            low = text.lower()
            hit = next(((p, d) for p, d in shifts if low == p or low.startswith(p + " ")), None)
            if not hit:
                break
            shift += hit[1]
            text = text[len(hit[0]):].strip()
        if text:
            lines.append({"code": translate(text, scope), "shift": shift})
        elif shift or k > 0:  # "new line new line" leaves a blank line between
            lines.append({"code": "", "shift": shift})
    return lines
