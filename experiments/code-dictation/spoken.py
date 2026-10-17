"""Spoken code -> source code, the second stage after ASR.

The ASR model transcribes words faithfully ("for i in range ten colon"); this
layer turns them into code and fills in what goes without saying: brackets,
colons, commas, identifier casing, and names already in scope.
"""
import difflib
import re

NUMBERS = {w: str(i) for i, w in enumerate(
    "zero one two three four five six seven eight nine ten eleven twelve thirteen fourteen "
    "fifteen sixteen seventeen eighteen nineteen".split())}
NUMBERS.update({w: str(10 * (i + 2)) for i, w in enumerate(
    "twenty thirty forty fifty sixty seventy eighty ninety".split())})
NUMBERS.update({"hundred": "100", "thousand": "1000"})

# Multi-word phrases first; longest match wins.
SYMBOLS = {
    "open paren": "(", "close paren": ")", "paren": "(",
    # how the ASR model writes them back
    "open parentheses": "(", "open parenthesis": "(", "parentheses": "(", "parenthesis": "(",
    "close parentheses": ")", "close parenthesis": ")", "closing paren": ")",
    "closing parentheses": ")", "closing parenthesis": ")", "closing bracket": "]",
    "closing brace": "}", "quotes": '"', "double quote": '"', "open bracket": "[",
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
# Python operators, for lex only.
PY_SYMBOLS = {**SYMBOLS, "star star": "**", "double star": "**", "slash slash": "//",
              "less than or equal": "<=", "less than or equal to": "<=",
              "greater than or equal": ">=", "greater than or equal to": ">=",
              "times equals": "*=", "slash equals": "/=", "pipe equals": "|=",
              "ampersand": "&", "caret": "^", "shift left": "<<", "shift right": ">>",
              "bang": "!", "question mark": "?", "arrow": "->",
              "slash slash equals": "//=", "percent equals": "%=", "star star equals": "**=",
              "ampersand equals": "&=", "caret equals": "^=", "shift left equals": "<<=",
              "shift right equals": ">>=", "at sign equals": "@="}
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
    text = re.sub(r"(?<=\d)\.(?=\d)", " point ", text)
    text = re.sub(r"[.,!?\"]", " ", text)
    out = []
    for w in text.split():
        w = MISHEARD.get(w, w) if out or w != "f" else "def"
        if w:
            out.append(NUMBERS.get(w, w))
    out = join_numbers(out)
    for k in range(len(out) - 2, 0, -1):  # 60 point 0 -> 60.0
        if out[k] == "point" and out[k - 1].isdigit() and out[k + 1].isdigit():
            j = k + 1
            while j < len(out) and out[j].isdigit():
                j += 1
            out[k - 1:j] = [out[k - 1] + "." + "".join(out[k + 1:j])]
    return out


def structural(word):
    """Words that shape the code ("of", "string", "if", "print") are never
    taken for, or merged into, a name in scope: "print of" must not become
    `print`, swallowing the call."""
    return (word in RESERVED or word in PY_KEYWORDS or word in PY_CALLABLES
            or word in PY_LITERALS or word in CASES or word in OPENERS
            or word in {"f", "brace", "end", "quote", "with", "taking", "returns"})


def name_parts(name):
    """How many words a name is said as: parse_args and ParseArgs are two."""
    return len(re.findall(r"[A-Z]+[0-9]*(?![a-z])|[A-Z]?[a-z]+[0-9]*|[0-9]+", name)) or 1


def shape(name):
    """The casing word that fits how a name is written."""
    core = name.strip("_")
    if name.startswith("__") and name.endswith("__") and len(name) > 4:
        return "dunder"
    if core.isupper():
        return "constant"
    if core[:1].isupper():
        return "pascal"
    return "camel" if any(c.isupper() for c in core) else "snake"


def cased(style, words, scope):
    """A casing word and the words after it -> the name in scope they say
    ("pascal sax exception" is SAXException), else None."""
    exact = case(style, words)
    if exact in scope:
        return exact
    joined = "".join(words)
    fits = [n for n in scope if shape(n) == style and re.sub(r"[^a-z0-9]", "", n.lower()) == joined]
    return min(fits, key=lambda n: (len(n) - len(n.lstrip("_")), n)) if fits else None


def name_span(words, scope, stops):
    """How many leading words are the name in a def or class head: a casing
    word runs as far as a name in scope ("camel parse with context"), else
    the name ends at a stop word."""
    lead = 0
    while lead < len(words) and words[lead] == "underscore":
        lead += 1
    if lead < len(words) - 1 and words[lead] in CASES:
        for k in range(len(words) - lead - 1, 0, -1):
            if cased(words[lead], words[lead + 1:lead + 1 + k], scope):
                return lead + 1 + k
    return split_at(words, *stops)


def lex(words, scope):
    """Words -> tokens: ("sym", s) | ("word", w) | ("num", n) | ("name", n)."""
    tokens, i = [], 0
    compact = {re.sub(r"[^a-z0-9]", "", s.lower()): s for s in scope
               if not structural(s.lower())}
    # Only names made of several words can be said as several words: "pass
    # args" may be parse_args, but "i was" is not `was`. A fuzzy match needs
    # as many words as the name has: "dot charmap encode" is not
    # charmap_encode, swallowing the dot.
    multi = {k: v for k, v in compact.items() if name_parts(v) > 1}
    by_parts = {}
    for k, v in multi.items():
        by_parts.setdefault(name_parts(v), []).append(k)
    single = [k for k, v in compact.items() if name_parts(v) == 1]
    in_scope = set(scope)
    raw = False  # after a casing word ("dunder init"), words stay words
    while i < len(words):
        if raw and not structural(words[i]) and words[i] not in PY_SYMBOLS and not words[i].isdigit():
            tokens.append(("word", words[i]))
            i += 1
            continue
        raw = False
        for n in (6, 5, 4, 3, 2, 1):  # longest symbol / scope match first
            if len(words) - i < n:
                continue
            phrase = " ".join(words[i:i + n])
            if phrase in PY_SYMBOLS:
                tokens.append(("sym", PY_SYMBOLS[phrase]))
                break
            joined = "".join(words[i:i + n])
            if n > 1 and not any(map(structural, words[i:i + n])) and (
                    m := [joined] if joined in multi
                    else [] if any(w in PY_SYMBOLS for w in words[i:i + n])
                    else difflib.get_close_matches(joined, by_parts.get(n, []), 1, 0.8)):
                tokens.append(("name", multi[m[0]]))  # "pass args" -> parse_args
                break
        else:
            n = 1
            w = words[i]
            if w in CASES and i + 1 < len(words):
                # "snake is file" is is_file when that is in scope: the
                # casing word lets a name hold words that mean code
                run = next((k for k in range(min(8, len(words) - i - 1), 0, -1)
                            if cased(w, words[i + 1:i + 1 + k], in_scope)), 0)
                if run:
                    tokens.append(("name", cased(w, words[i + 1:i + 1 + run], in_scope)))
                    n += run
                else:
                    tokens.append(("case", w))
                    raw = True
            elif w.isdigit() or re.fullmatch(r"\d+\.\d+", w):
                tokens.append(("num", w))
            elif structural(w):
                tokens.append(("word", w))
            elif w in compact:
                tokens.append(("name", compact[w]))
            elif len(w) >= 4 and (m := difflib.get_close_matches(w, single, 1, 0.75)):
                tokens.append(("name", compact[m[0]]))  # "args" -> argv when in scope
            else:
                tokens.append(("word", w))
        i += n
    # "underscore inner" is _inner, "underscore underscore x" is __x
    out, lead = [], 0
    for t in tokens:
        if t == ("sym", "_"):
            lead += 1
            continue
        if lead and (t[0] == "name" or t[0] == "word" and not structural(t[1])):
            v = str(t[1])
            out.append(("name", "_" * lead + (v if v.endswith("__") else v.lstrip("_"))))
        else:
            out += [("sym", "_")] * lead + [t]
        lead = 0
    return out + [("sym", "_")] * lead


def ident(words, scope, pascal=False):
    """Words that can only be one name (a def, a class, an imported name) ->
    that name: a casing word, a name in scope, or snake_case (PascalCase for
    a class)."""
    lead = 0
    while lead < len(words) - 1 and words[lead] == "underscore":
        lead += 1  # "underscore ast" -> _ast
    if lead:
        name = ident(words[lead:], scope, pascal)
        return name if name.endswith("__") else "_" * lead + name.lstrip("_")
    if not words:
        return ""
    if words[0] in CASES and len(words) > 1:
        return cased(words[0], words[1:], set(scope)) or case(words[0], words[1:])
    joined = "".join(words)
    known = [n for n in scope if re.sub(r"[^a-z0-9]", "", n.lower()) == joined]
    if pascal:
        known = [n for n in known if n.lstrip("_")[:1].isupper()]
    if known:
        return known[-1]
    if len(words) > 1:
        sized = {re.sub(r"[^a-z0-9]", "", n.lower()): n for n in scope
                 if name_parts(n) == len(words) and (not pascal or n.lstrip("_")[:1].isupper())}
        m = difflib.get_close_matches(joined, sized, 1, 0.8)
        if m:
            return sized[m[0]]
    return case("pascal" if pascal else "snake", words)


def split_at(words, *stops):
    """Index of the first stop word (or two-word stop), else len(words)."""
    for k in range(len(words)):
        if words[k] in stops or " ".join(words[k:k + 2]) in stops:
            return k
    return len(words)


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
# Said inside a "string ...", these end it and still act: "close" also
# closes the bracket around it, "comma" starts the next argument, and a
# spoken closing bracket closes it.
STRING_STOPS = {("word", "close"), ("sym", ","), ("sym", ")"), ("sym", "]"), ("sym", "}")}
RESERVED = {"of", "index", "call", "sub", "at", "close", "string", "empty"} | set(OPENERS)


def string_literal(tokens, i, f=False, quoted=False):
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
        if not quoted and (kind, val) in STRING_STOPS:
            break  # left for the caller
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

    ident = re.compile(r"[A-Za-z_][A-Za-z0-9_]*$")

    def value(piece, space=True):
        """Emit a value. Right after a name it is that name's argument:
        `say "hi"` or `foo x` is no Python 3, so it means say("hi"), foo(x)."""
        if out and ident.search(out[-1]) and out[-1] not in PY_KEYWORDS:
            open_("(", ")")
            space = False
        emit(piece, space)

    while i < len(tokens):
        kind, val = tokens[i]
        nxt = tokens[i + 1] if i + 1 < len(tokens) else (None, None)
        if kind == "case":
            j = i + 1
            while j < len(tokens) and tokens[j][0] == "word" and (j == i + 1 or not structural(tokens[j][1])):
                j += 1  # "dunder new of cls": the name ends at "of"
            emit(case(val, [t[1] for t in tokens[i + 1:j]]))
            i = j
            continue
        if (kind, val) == ("sym", '"') or (kind, val) == ("word", "string"):
            literal, i = string_literal(tokens, i + 1, quoted=kind == "sym")
            value(literal)
            continue
        if (kind, val) == ("word", "bytes") and nxt == ("word", "string"):
            literal, i = string_literal(tokens, i + 2)
            value("b" + literal)
            continue
        if (kind, val) == ("word", "f") and nxt == ("word", "string"):
            literal, i = string_literal(tokens, i + 2, f=True)
            value(literal)
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
            said_of = nxt == ("word", "of")
            if said_of:
                i += 1  # "sum of x" already says the call
            after = tokens[i + 1] if i + 1 < len(tokens) else (None, None)
            starts_value = after[0] in ("name", "num", "case") or after in (("sym", '"'), ("sym", "-")) or (
                after[0] == "word" and after[1] not in PY_KEYWORDS - {"not", "lambda", "None", "True", "False"}
                and after[1] not in RESERVED - {"string", "empty"} | set(OPENERS))
            if (said_of or starts_value) and after != ("sym", "("):
                open_("(", ")")  # "range ten" -> range(10); "type comma" is the name
        elif kind == "word":
            j = i
            while j < len(tokens) and tokens[j][0] == "word" and tokens[j][1] not in (
                    PY_KEYWORDS | PY_CALLABLES | set(PY_LITERALS) | RESERVED):
                j += 1
            value("_".join(t[1] for t in tokens[i:max(j, i + 1)]))
            i = max(j, i + 1)
            continue
        elif kind == "sym":
            if val == "=" and condition and not opened:
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
            value(val)  # names in scope and numbers
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
    if head == "async" and rest:
        return "async " + python_line(" ".join(rest), scope)
    if head == "at" and rest:  # a decorator: "at property", "at sign app dot route of ..."
        return "@" + python_expr(lex(rest[1:] if rest[0] == "sign" else rest, scope))
    if head == "def":
        # "def f taking self comma x equals none comma star args returns int"
        k = name_span(rest, set(scope), ("taking", "with", "returns", "arrow", "open paren", "open parentheses",
                                         "open parenthesis", "paren", "parentheses", "parenthesis"))
        name = ident(rest[:k], scope)
        toks = lex(rest[k:], scope)
        while toks and toks[-1] == ("sym", ":"):
            toks.pop()
        end = next((k for k, t in enumerate(toks) if t in (("word", "returns"), ("sym", "->"))), len(toks))
        args = toks[1:end] if toks and toks[0] in (("word", "taking"), ("word", "with"), ("sym", "(")) else []
        if args and args[-1] == ("sym", ")"):
            args = args[:-1]  # said "open paren ... close paren"
        returns = f" -> {python_expr(toks[end + 1:])}" if end < len(toks) else ""
        return f"def {name}({python_expr(args)}){returns}:"
    if head == "class":
        # "class request handler of base handler": the name is PascalCase
        k = name_span(rest, set(scope), ("of", "open paren", "open parentheses", "open parenthesis", "paren",
                                         "parentheses", "parenthesis"))
        name = ident(rest[:k], scope, pascal=True)
        bases = [t for t in lex(rest[k:], scope)[1:] if t != ("sym", ":")]
        if bases and bases[-1] == ("sym", ")"):
            bases = bases[:-1]
        return f"class {name}({python_expr(bases)}):" if bases else f"class {name}:"
    if head in ("import", "from") and (head == "import" or "import" in rest):
        # every word here is part of a name: "from typing import any comma type alias"
        def dotted(ws):
            parts, k = [], 0
            while k < len(ws) and ws[k] == "dot":
                parts.append(".")  # relative: "from dot console import ..."
                k += 1
            names = []
            for w in ws[k:]:
                if w == "dot" or not names:
                    names.append([])
                if w != "dot":
                    names[-1].append(w)
            return "".join(parts) + ".".join(ident(n, scope) for n in names if n)

        def aliases(ws):
            out, groups, cur = [], [], []
            for w in ws:
                if w == "comma":
                    groups.append(cur)
                    cur = []
                else:
                    cur.append(w)
            groups.append(cur)
            for g in groups:
                if g == ["star"]:
                    out.append("*")
                    continue
                a = split_at(g, "as")
                alias = f" as {ident(g[a + 1:], scope)}" if a < len(g) else ""
                out.append(dotted(g[:a]) + alias)
            return ", ".join(o for o in out if o)

        if head == "import":
            return f"import {aliases(rest)}"
        k = rest.index("import")
        return f"from {dotted(rest[:k])} import {aliases(rest[k + 1:])}"
    if head in ("else", "try", "finally"):
        return f"{head}:"
    if head in PY_BLOCKS:
        return f"{head} {python_expr(toks, condition=head in ('if', 'elif', 'while'))}:"
    if head in ("return", "import", "from", "raise", "assert", "yield", "del", "global", "nonlocal"):
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


# Formatting, in any mode: ruff for Python, otherwise the buffer's formatter.
FORMAT = "<Cmd>lua _G.ears_format()<CR>"
FORMAT_PHRASES = {"ruff": FORMAT, "rough": FORMAT, "format code": FORMAT,
                  "format": FORMAT, "format file": FORMAT}

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
    **FORMAT_PHRASES,
    "visual": "v", "visual line": "V", "escape": "<Esc>", "save": ":w<CR>",
    # Moving around; "go to <name>", "next function" etc. are in navigation().
    "start of line": "^", "end of line": "$", "end of word": "e", "first line": "gg",
    "last line": "G", "page down": "<C-d>", "page up": "<C-u>", "match": "%",
    "matching bracket": "%", "next block": "}", "previous block": "{",
    "go back": "<C-o>", "jump back": "<C-o>", "go forward": "<C-i>", "jump forward": "<C-i>",
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
    # Moving without leaving insert mode.
    "cursor left": "<Left>", "cursor right": "<Right>", "cursor up": "<Up>",
    "cursor down": "<Down>", "word left": "<C-Left>", "word right": "<C-Right>",
    "end of line": "<End>", "start of line": "<C-o>^",
    **FORMAT_PHRASES,
}
REPEATS = {"double": 2, "triple": 3, "quadruple": 4, "twice": 2, "thrice": 3}


# Navigation by what is there, not by keys: "go to parse args", "back to
# total", "after colon", "next function", "go to def main". The plugin runs
# the search (_G.ears_find), so it works in insert mode without leaving it,
# and "next" / "previous" repeat it.
SEEK = {"go to": "", "jump to": "", "find": "", "before": "", "back to": "b",
        "after": "e", "back after": "be"}
DEFINES = {"py": r"\<\(def\|class\)\s\+", "sh": r"\<\(function\s\+\)\?"}
BLOCKS = {
    "py": {"function": r"^\s*\(async\s\+\)\?def\>", "class": r"^\s*class\>"},
    "sh": {"function": r"^\s*\(function\s\+\w\+\|\w\+\s*()\)"},
}
KINDS = {"function": "function", "def": "function", "method": "function",
         "definition": "function", "class": "class"}
CLOSERS = r"""[\])}"']"""


def find_keys(pattern, how="", count=None):
    return f'<Cmd>lua _G.ears_find([==[{pattern}]==], "{how}", {count or 1})<CR>'


def target(words, scope, lang):
    """Spoken target -> a Vim pattern: a symbol ("colon"), a name in scope
    ("pass args" -> parse_args), or the words as one name, any case."""
    phrase = " ".join(words)
    if phrase in SYMBOLS:
        return r"\V" + SYMBOLS[phrase].replace("\\", "\\\\")
    if words and words[0] in KINDS and len(words) > 1 and lang in DEFINES:
        return DEFINES[lang] + target(words[1:], scope, lang).removeprefix(r"\V")
    compact = {re.sub(r"[^a-z0-9]", "", s.lower()): s for s in scope}
    joined = "".join(words)
    m = [joined] if joined in compact else difflib.get_close_matches(joined, compact, 1, 0.75)
    if m:
        return r"\<" + compact[m[0]] + r"\>"
    return r"\c\<" + r"_\?".join(words) + r"\>"


def navigation(words, scope, lang):
    """Navigation said as the whole utterance -> keys, else None."""
    count, words = take_count(words)
    phrase = " ".join(words)
    if phrase in ("jump out", "skip out", "out"):
        return find_keys(CLOSERS, "e", count)
    if phrase in INSIDE:
        return find_keys(INSIDE[phrase], "e", count)
    if len(words) == 2 and words[0] in ("next", "previous") and words[1] in KINDS:
        pattern = BLOCKS.get(lang, {}).get(KINDS[words[1]])
        return pattern and find_keys(pattern, "" if words[0] == "next" else "b", count)
    for lead in sorted(SEEK, key=len, reverse=True):
        if phrase.startswith(lead + " "):
            return find_keys(target(phrase[len(lead) + 1:].split(), scope, lang), SEEK[lead], count)
    return None


# Editing by what is there: a verb, then what to act on.
#   "select total", "delete from print to colon", "change inside string",
#   "copy function", "move line to after return", "copy argv to after total".
VERBS = {"select": "v", "highlight": "v", "mark": "v", "delete": "d", "cut": "d",
         "copy": "y", "yank": "y", "change": "c", "replace": "c"}
# Text objects: "inside parens" (the default) or "around parens".
OBJECTS = {"parens": "(", "parentheses": "(", "parenthesis": "(", "args": "(", "arguments": "(",
           "call": "(", "brackets": "[", "list": "[", "index": "[", "braces": "{",
           "dict": "{", "string": '"', "quotes": '"', "double quotes": '"',
           "single quotes": "'", "word": "w", "block": "p", "paragraph": "p"}
LINES = {"line", "this line", "the line", "lines"}
PLACES = {"to after": "after", "after": "after", "below": "after", "under": "after",
          "to before": "before", "before": "before", "above": "before", "over": "before",
          "to": "after"}
# In visual mode: what to do with the selection.
VISUAL = {"delete": "d", "cut": "d", "copy": "y", "yank": "y", "change": "c",
          "indent": ">", "dedent": "<", "the indent": "<", "upper case": "U", "lower case": "u",
          "paren it": "S)", "wrap in parens": "S)", "bracket it": "S]", "brace it": "S}",
          "quote it": 'S"', "single quote it": "S'", "escape": "<Esc>", "join": "J"}
# Insert mode, in code: put the cursor inside the next pair or argument.
INSIDE = {"inside parens": r"\V(", "inside brackets": r"\V[", "inside braces": r"\V{",
          "inside string": r"\V\"", "inside quotes": r"\V\"", "inside args": r"\V(",
          "next argument": r",\s*", "next arg": r",\s*"}


def lua(fn, *args):
    def arg(a):
        return "nil" if a is None else str(a) if isinstance(a, int) else f"[==[{a}]==]"
    return f"<Cmd>lua _G.{fn}({', '.join(map(arg, args))})<CR>"


def editing(words, scope, lang, visual=False):
    """A verb with what it acts on -> keys, else None."""
    phrase = " ".join(words)
    if visual:
        if phrase in VISUAL:
            return VISUAL[phrase]
        for verb in ("move", "copy"):
            for place in sorted(PLACES, key=len, reverse=True):
                if phrase.startswith(f"{verb} {place} ") or phrase.startswith(f"{verb} selection {place} "):
                    rest = phrase.split(f" {place} ", 1)[1]
                    return lua("ears_move", "selection", target(rest.split(), scope, lang),
                               PLACES[place], int(verb == "copy"))
    if len(words) > 2 and words[0] == "move" and words[1] in ("line", "lines") and words[2] in ("up", "down"):
        count = next((int(w) for w in words[3:] if w.isdigit()), 1)
        return f"<Cmd>move .{'-' + str(count + 1) if words[2] == 'up' else '+' + str(count)}<CR>=="
    if phrase in ("duplicate line", "duplicate"):
        return "yyp"
    if words and words[0] in ("move", "copy"):
        rest = " ".join(words[1:])
        for place in sorted(PLACES, key=len, reverse=True):
            if f" {place} " in f" {rest} " and not rest.startswith(place + " "):
                what, where = rest.split(f" {place} ", 1)
                source = "line" if what in LINES else target(what.split(), scope, lang)
                return lua("ears_move", source, target(where.split(), scope, lang),
                           PLACES[place], int(words[0] == "copy"))
        if words[0] == "move":
            return None
    if not words or words[0] not in VERBS or len(words) < 2:
        return None
    verb, rest = VERBS[words[0]], words[1:]
    what = " ".join(rest)
    if what in LINES:
        return "V" if verb == "v" else verb * 2
    if what in KINDS or what in ("this function", "the function", "this class"):
        return lua("ears_block", verb, KINDS[what.split()[-1]])
    side = "i"
    if rest[0] in ("inside", "in", "around"):
        side, what = ("a" if rest[0] == "around" else "i"), " ".join(rest[1:])
    if what in OBJECTS:
        return verb + side + OBJECTS[what]
    if rest[0] == "from" and " to " in what:
        a, b = what[len("from "):].split(" to ", 1)
        return lua("ears_act", verb, target(a.split(), scope, lang), target(b.split(), scope, lang))
    return lua("ears_act", verb, target(rest, scope, lang), None)


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


def normal_keys(heard, scope=(), lang=None, visual=False):
    """A normal-mode utterance -> keys, or None when it is not a command
    (then nothing happens: dictation never lands in normal mode)."""
    conf = config()
    count, words = take_count(plain(heard).split())
    phrase = " ".join(words)
    table = {plain(k): v for k, v in conf["normal"].items()}
    keys = table.get(phrase) if not (visual and phrase in VISUAL) else None
    if keys is None and (edit := editing(plain(heard).split(), scope, lang, visual)):
        return edit
    if keys is None and (seek := navigation(plain(heard).split(), scope, lang)):
        return seek
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


def insert_keys(heard, scope=(), lang=None):
    """A whole insert-mode utterance that is a command -> keys, else None.
    Navigation by name only in code: in prose, "go to the shop" is text."""
    count, words = take_count(plain(heard).split())
    keys = {plain(k): v for k, v in config()["insert"].items()}.get(" ".join(words))
    if keys:
        return keys * min(count or 1, 50)
    return navigation(plain(heard).split(), scope, lang) if lang in BLOCKS else None


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
