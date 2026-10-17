"""Python source -> what a person would say to dictate it.

The inverse of spoken.py, for synthetic training data: each line of real code
comes out as several spoken variants, each tagged with a style.

  terse     spoken.py's dialect, leaving out what goes without saying: the
            colon after a block head, closing brackets at the end of the line,
            quotes ("string hello"), comma spacing.
            "x equals foo of a comma string b"
  explicit  every symbol said, as with a dictation tool that knows no code.
            "x equals foo open paren a comma quote b quote close paren"
  natural   English with some variety (seeded per line).
            "set x to call foo with a and string b"
  noself    natural, with "self dot" left out inside methods: only the
            context can tell it back.

Where spoken.py has no way to say something (default parameters, base
classes, decorators, `**`, `<=`), terse uses the phrase a person would most
likely try ("def f taking x equals none", "class foo of base", "at
property", "star star", "less than or equal"); the round trip then shows it as
a gap in the rules. Things with no reasonable spoken form raise Unsayable.

Usage: python verbalize.py FILE.py        (prints each line and its variants)
"""
import ast
import io
import random
import re
import sys
import tokenize

import spoken


class Unsayable(Exception):
    """A construct the verbalizer has no way to say."""


CLOSE = "\0close"  # implied closing bracket: "close", or left out at the end


class Str:
    """A string literal, said differently per style once its neighbours are
    known ("string x" may only end at a comma, "close" or the line's end)."""

    def __init__(self, words, f=False):
        self.words, self.f = words, f


SMALL = ("zero one two three four five six seven eight nine ten eleven twelve thirteen "
         "fourteen fifteen sixteen seventeen eighteen nineteen").split()
TENS = "_ _ twenty thirty forty fifty sixty seventy eighty ninety".split()


def number_words(text):
    """An int literal as said: words up to ninety nine, digits beyond (the
    ASR writes those as digits anyway)."""
    if not text.isdigit() or (len(text) > 1 and text.startswith("0")):
        raise Unsayable("number: " + ("non-decimal" if text[:2].lower() in ("0x", "0o", "0b")
                                       else "underscores" if "_" in text else "other"))
    n = int(text)
    if n < 20:
        return [SMALL[n]]
    if n < 100:
        return [TENS[n // 10]] + ([SMALL[n % 10]] if n % 10 else [])
    return [text]


def name_words(name):
    """parse_args -> parse args, RequestHandler -> request handler,
    __init__ -> dunder init. A leading underscore goes unsaid: the name in
    scope brings it back."""
    if name.startswith("__") and name.endswith("__") and len(name) > 4:
        return ["dunder"] + name_words(name[2:-2])
    words = []
    for part in name.strip("_").split("_"):
        words += [w.lower() for w in
                  re.findall(r"[A-Z]+[0-9]*(?![a-z])|[A-Z]?[a-z]+[0-9]*|[0-9]+", part)]
    if not words:
        raise Unsayable("name: underscore only")
    return words


PUNCT = {".": "dot", ",": "comma", ":": "colon", ";": "semicolon", "/": "slash", "-": "dash",
         "_": "underscore", "%": "percent", "=": "equals", "+": "plus", "*": "star",
         "#": "hash", "@": "at sign", "~": "tilde", "$": "dollar", "|": "pipe",
         "(": "open paren", ")": "close paren", "[": "open bracket", "]": "close bracket",
         "{": "open brace", "}": "close brace", "<": "less than", ">": "greater than",
         "'": "single quote", '"': "quote", "!": "bang", "?": "question mark",
         "&": "ampersand", "\\": "backslash", "^": "caret", "`": "backtick",
         "\n": "backslash n", "\t": "backslash t", "\r": "backslash r"}


def text_words(s):
    """String contents as words: letters lowercased (case is not heard),
    punctuation by name."""
    if s and s.isspace():
        if len(s) > 4 or s.strip(" "):
            raise Unsayable("string: whitespace only")
        return ["space"] * len(s)
    out = []
    for m in re.finditer(r"[A-Za-z]+|[0-9]+| +|.", s, re.S):
        t = m.group()
        if t.isascii() and (t.isalpha() or t.isdigit()):
            out.append(t.lower())
        elif t.isspace() and t not in PUNCT:
            continue
        elif t in PUNCT:
            out += PUNCT[t].split()
        else:
            raise Unsayable("string: non-ascii" if not t.isascii() else "string: control char")
    return out


def compact(name):
    return re.sub(r"[^a-z0-9]", "", name.lower())


def casing(name):
    """The casing word that says how a name is written: spoken.py takes
    "pascal sax exception" for SAXException when that is in scope."""
    return spoken.shape(name)


HEADS = {"def", "class", "if", "elif", "else", "try", "finally", "while", "for", "with",
         "except", "return", "import", "from", "raise", "assert", "yield", "del", "global",
         "nonlocal", "async", "at"} | set(spoken.HEADS)


def hazard(word):
    """A word that spoken.py reads as code, not as part of a name."""
    return (spoken.structural(word) or word in spoken.PY_SYMBOLS or word in HEADS
            or word in spoken.NUMBERS or word in spoken.MISHEARD or word in spoken.CASES
            or word in ("underscore", "taking", "returns", "point"))


def leading(name):
    if name.startswith("__") and name.endswith("__"):
        return 0
    return len(name) - len(name.lstrip("_"))


def ambiguous(scope):
    """Names that need help to be said: {name: (underscores to say, casing
    word needed)}. Event and event in one file need a casing word, _inner
    and inner an "underscore"; so do names holding a word that means code
    (is_file, selective_len, TRY_NATIVE)."""
    by_key = {}
    for n in scope:
        if n.strip("_"):
            by_key.setdefault(compact(n), []).append(n)
    out = {}
    for n in scope:
        if not n.isidentifier() or not n.strip("_"):
            continue
        others = [o for o in by_key[compact(n)] if o != n]
        say_lead = any(leading(o) != leading(n) for o in others)
        other_case = any(o.strip("_") != n.strip("_") for o in others)
        words = name_words(n)
        risky = any(hazard(w) for w in words if w != "dunder") or (words[0] == "dunder" and len(words) == 1)
        if say_lead or other_case or risky:
            out[n] = (leading(n) if say_lead else 0, other_case or risky)
    return out


BINPREC = {ast.BitOr: 7, ast.BitXor: 8, ast.BitAnd: 9, ast.LShift: 10, ast.RShift: 10,
           ast.Add: 11, ast.Sub: 11, ast.Mult: 12, ast.Div: 12, ast.FloorDiv: 12,
           ast.Mod: 12, ast.MatMult: 12, ast.Pow: 14}


def precedence(node):
    """As in ast.unparse: higher binds tighter. A child below what its place
    needs is put in parens."""
    if isinstance(node, (ast.Tuple, ast.NamedExpr, ast.Yield, ast.YieldFrom)):
        return 0
    if isinstance(node, ast.Lambda):
        return 1
    if isinstance(node, ast.IfExp):
        return 2
    if isinstance(node, ast.BoolOp):
        return 3 if isinstance(node.op, ast.Or) else 4
    if isinstance(node, ast.UnaryOp):
        return 5 if isinstance(node.op, ast.Not) else 13
    if isinstance(node, ast.Compare):
        return 6
    if isinstance(node, ast.BinOp):
        return BINPREC[type(node.op)]
    if isinstance(node, ast.Await):
        return 15
    return 16


class Terse:
    """spoken.py's dialect. Each method returns tokens: words, CLOSE, Str."""
    style = "terse"
    BINOP = {ast.Add: "plus", ast.Sub: "minus", ast.Mult: "times", ast.Div: "slash",
             ast.FloorDiv: "slash slash", ast.Mod: "percent", ast.Pow: "star star",
             ast.MatMult: "at sign", ast.BitOr: "pipe", ast.BitAnd: "ampersand",
             ast.BitXor: "caret", ast.LShift: "shift left", ast.RShift: "shift right"}
    CMP = {ast.Eq: "equals equals", ast.NotEq: "not equals", ast.Lt: "less than",
           ast.LtE: "less than or equal", ast.Gt: "greater than",
           ast.GtE: "greater than or equal", ast.Is: "is", ast.IsNot: "is not",
           ast.In: "in", ast.NotIn: "not in"}
    UNARY = {ast.Not: "not", ast.USub: "minus", ast.UAdd: "plus", ast.Invert: "tilde"}

    def __init__(self, src="", rng=None, in_method=False, scope=()):
        self.src, self.rng = src, rng or random.Random(0)
        self.in_method = in_method
        self.condition = False
        self.ambiguous = ambiguous(scope) if scope else {}

    def ident(self, name):
        """A name as said: "pascal event" when event is also in scope,
        "underscore inner" when inner is, "snake is file" (is means code)."""
        words = name_words(name)
        if name not in self.ambiguous:
            return words
        lead, needs_case = self.ambiguous[name]
        core = name[leading(name):]
        style = casing(core) if needs_case and words[0] != "dunder" else None
        return ["underscore"] * lead + ([style] if style else []) + words

    def pick(self, *options):
        return options[0]

    def words(self, text):
        return text.split()

    # -- brackets ------------------------------------------------------
    def group(self, inner):
        return ["parens"] + inner + [CLOSE]

    def call_open(self):
        return ["of"]

    def call_close(self):
        return [CLOSE]

    def index_open(self):
        return ["index"]

    def index_close(self):
        return [CLOSE]

    def list_(self, elts):
        return ["brackets"] + elts + [CLOSE] if elts else ["empty", "list"]

    def dict_(self, items):
        return ["braces"] + items + [CLOSE] if items else ["empty", "dict"]

    def set_(self, elts):
        return ["braces"] + elts + [CLOSE]

    def tuple_(self, elts):
        return self.group(elts) if elts else ["empty", "tuple"]

    def head_end(self):
        return []

    def sep(self, parts, last=False):
        """Join token lists with "comma"."""
        out = []
        for k, p in enumerate(parts):
            if k:
                out.append("comma")
            out += p
        return out

    # -- expressions ---------------------------------------------------
    def expr(self, node, need=0):
        method = getattr(self, "e_" + type(node).__name__, None)
        if method is None:
            raise Unsayable(type(node).__name__)
        out = method(node)
        return self.group(out) if precedence(node) < need else out

    def e_Name(self, node):
        return self.ident(node.id)

    def e_Constant(self, node):
        v = node.value
        if v is None or isinstance(v, bool):
            return [str(v).lower()]
        if v is Ellipsis:
            return ["dot", "dot", "dot"]
        if isinstance(v, str):
            return self.string(text_words(v))
        if isinstance(v, bytes):
            return ["bytes"] + self.string(text_words(v.decode("latin-1")))
        text = ast.get_source_segment(self.src, node) if self.src else None
        text = text or repr(v)
        if isinstance(v, int):
            return number_words(text)
        if isinstance(v, float) and re.fullmatch(r"[0-9]*\.[0-9]+|[0-9]+\.[0-9]*", text):
            whole, frac = text.split(".")
            return (number_words(whole) if whole else []) + ["point"] + [SMALL[int(d)] for d in frac]
        raise Unsayable("number: " + type(v).__name__)

    def string(self, words):
        return [Str(words)]

    def e_JoinedStr(self, node):
        words = []
        for part in node.values:
            if isinstance(part, ast.Constant):
                words += text_words(part.value)
            else:
                if part.format_spec is not None or part.conversion != -1:
                    raise Unsayable("f-string: format spec or conversion")
                words += ["brace"] + self.finish(self.expr(part.value)).split()
        return [Str(words, f=True)]

    def e_TemplateStr(self, node):
        raise Unsayable("t-string")

    def e_Attribute(self, node):
        if self.style == "noself" and isinstance(node.value, ast.Name) and node.value.id == "self":
            return self.ident(node.attr)
        return self.expr(node.value, 16) + ["dot"] + self.ident(node.attr)

    def args(self, args, keywords):
        if len(args) == 1 and not keywords and isinstance(args[0], ast.GeneratorExp):
            return [self.comprehension(args[0])]
        parts = [self.expr(a, 1) for a in args]
        for kw in keywords:
            if kw.arg is None:
                parts.append(["star", "star"] + self.expr(kw.value, 1))
            else:
                parts.append(self.ident(kw.arg) + ["equals"] + self.expr(kw.value, 1))
        return parts

    def e_Call(self, node):
        parts = self.args(node.args, node.keywords)
        return self.expr(node.func, 16) + self.call_open() + self.sep(parts) + self.call_close()

    def e_Starred(self, node):
        return ["star"] + self.expr(node.value, 7)

    def e_Subscript(self, node):
        return self.expr(node.value, 16) + self.index_open() + self.expr(node.slice) + self.index_close()

    def e_Slice(self, node):
        out = self.expr(node.lower, 1) if node.lower else []
        out += ["colon"] + (self.expr(node.upper, 1) if node.upper else [])
        if node.step:
            out += ["colon"] + self.expr(node.step, 1)
        return out

    def e_BinOp(self, node):
        p = BINPREC[type(node.op)]
        right_assoc = isinstance(node.op, ast.Pow)
        return (self.expr(node.left, p + right_assoc) + self.words(self.binop(node.op))
                + self.expr(node.right, p + (not right_assoc)))

    def binop(self, op):
        return self.BINOP[type(op)]

    def e_UnaryOp(self, node):
        p = precedence(node)
        return self.words(self.unary(node.op)) + self.expr(node.operand, p)

    def unary(self, op):
        return self.UNARY[type(op)]

    def e_BoolOp(self, node):
        p = precedence(node)
        out = []
        for k, v in enumerate(node.values):
            out += ([type(node.op).__name__.lower()] if k else []) + self.expr(v, p + 1)
        return out

    def e_Compare(self, node):
        out = self.expr(node.left, 7)
        for op, right in zip(node.ops, node.comparators):
            out += self.words(self.cmp(op)) + self.expr(right, 7)
        return out

    def cmp(self, op):
        if self.condition and isinstance(op, ast.Eq):
            return "equals"  # spoken.py reads "=" in a condition as ==
        return self.CMP[type(op)]

    def e_IfExp(self, node):
        return self.expr(node.body, 3) + ["if"] + self.expr(node.test, 3) + ["else"] + self.expr(node.orelse, 2)

    def e_Lambda(self, node):
        params = self.params(node.args)
        return ["lambda"] + params + ["colon"] + self.expr(node.body, 1)

    def e_NamedExpr(self, node):
        return self.expr(node.target) + ["colon", "equals"] + self.expr(node.value, 1)

    def e_Await(self, node):
        return ["await"] + self.expr(node.value, 16)

    def e_Yield(self, node):
        return ["yield"] + (self.expr(node.value) if node.value else [])

    def e_YieldFrom(self, node):
        return ["yield", "from"] + self.expr(node.value, 1)

    def e_List(self, node):
        return self.list_(self.sep([self.expr(e, 1) for e in node.elts]))

    def e_Tuple(self, node):
        parts = [self.expr(e, 1) for e in node.elts]
        out = self.sep(parts) + (["comma"] if len(parts) == 1 else [])
        return out if out else self.tuple_([])

    def e_Set(self, node):
        return self.set_(self.sep([self.expr(e, 1) for e in node.elts]))

    def e_Dict(self, node):
        items = []
        for k, v in zip(node.keys, node.values):
            if k is None:
                items.append(["star", "star"] + self.expr(v, 1))
            else:
                items.append(self.expr(k, 1) + ["colon"] + self.expr(v, 1))
        return self.dict_(self.sep(items))

    def comprehension(self, node):
        if isinstance(node, ast.DictComp):
            out = self.expr(node.key, 1) + ["colon"] + self.expr(node.value, 1)
        else:
            out = self.expr(node.elt, 1)
        for gen in node.generators:
            out += (["async"] if gen.is_async else []) + ["for"] + self.expr(gen.target) + ["in"] + self.expr(gen.iter, 3)
            for cond in gen.ifs:
                out += ["if"] + self.expr(cond, 3)
        return out

    def e_ListComp(self, node):
        return self.list_(self.comprehension(node))

    def e_SetComp(self, node):
        return self.set_(self.comprehension(node))

    def e_DictComp(self, node):
        return self.dict_(self.comprehension(node))

    def e_GeneratorExp(self, node):
        return self.group(self.comprehension(node))

    # -- statements ----------------------------------------------------
    def params(self, a):
        """Parameters, with defaults and annotations."""
        parts = []
        positional = a.posonlyargs + a.args
        defaults = [None] * (len(positional) - len(a.defaults)) + a.defaults
        for k, (arg, default) in enumerate(zip(positional, defaults)):
            parts.append(self.param(arg, default))
            if a.posonlyargs and k == len(a.posonlyargs) - 1:
                parts.append(["slash"])
        if a.vararg:
            parts.append(["star"] + self.param(a.vararg, None))
        elif a.kwonlyargs:
            parts.append(["star"])
        for arg, default in zip(a.kwonlyargs, a.kw_defaults):
            parts.append(self.param(arg, default))
        if a.kwarg:
            parts.append(["star", "star"] + self.param(a.kwarg, None))
        return self.sep(parts)

    def param(self, arg, default):
        out = self.ident(arg.arg)
        if arg.annotation is not None:
            out += ["colon"] + self.expr(arg.annotation, 1)
        if default is not None:
            out += ["equals"] + self.expr(default, 1)
        return out

    def s_FunctionDef(self, node):
        if getattr(node, "type_params", None):
            raise Unsayable("type params")
        params = self.params(node.args)
        out = (["async"] if isinstance(node, ast.AsyncFunctionDef) else []) + ["def"] + self.ident(node.name)
        out += (["taking"] + params) if params else []
        if node.returns is not None:
            out += ["returns"] + self.expr(node.returns, 1)
        return out

    s_AsyncFunctionDef = s_FunctionDef

    def s_ClassDef(self, node):
        if getattr(node, "type_params", None):
            raise Unsayable("type params")
        out = ["class"] + self.ident(node.name)
        parts = self.args(node.bases, node.keywords)
        return out + (["of"] + self.sep(parts) if parts else [])

    def s_If(self, node, head="if"):
        self.condition = True
        return [head] + self.expr(node.test)

    def s_While(self, node):
        self.condition = True
        return ["while"] + self.expr(node.test)

    def s_For(self, node):
        return (["async"] if isinstance(node, ast.AsyncFor) else []) + ["for"] + self.expr(node.target) + ["in"] + self.expr(node.iter)

    s_AsyncFor = s_For

    def s_With(self, node):
        items = []
        for item in node.items:
            out = self.expr(item.context_expr, 1)
            if item.optional_vars is not None:
                out += ["as"] + self.expr(item.optional_vars)
            items.append(out)
        return (["async"] if isinstance(node, ast.AsyncWith) else []) + ["with"] + self.sep(items)

    s_AsyncWith = s_With

    def s_ExceptHandler(self, node):
        out = ["except"]
        if node.type is not None:
            out += self.expr(node.type)
        if node.name:
            out += ["as"] + self.ident(node.name)
        return out

    def s_Assign(self, node):
        out = []
        for t in node.targets:
            out += self.expr(t) + ["equals"]
        return out + self.expr(node.value)

    def s_AnnAssign(self, node):
        out = self.expr(node.target) + ["colon"] + self.expr(node.annotation, 1)
        return out + (["equals"] + self.expr(node.value) if node.value is not None else [])

    def s_AugAssign(self, node):
        return self.expr(node.target) + self.words(self.binop(node.op)) + ["equals"] + self.expr(node.value)

    def s_Expr(self, node):
        return self.expr(node.value)

    def s_Return(self, node):
        return ["return"] + (self.expr(node.value) if node.value else [])

    def s_Raise(self, node):
        out = ["raise"] + (self.expr(node.exc) if node.exc else [])
        return out + (["from"] + self.expr(node.cause) if node.cause else [])

    def s_Assert(self, node):
        self.condition = True
        out = ["assert"] + self.expr(node.test)
        self.condition = False
        return out + (["comma"] + self.expr(node.msg) if node.msg else [])

    def s_Delete(self, node):
        return ["del"] + self.sep([self.expr(t) for t in node.targets])

    def s_Pass(self, node):
        return ["pass"]

    def s_Break(self, node):
        return ["break"]

    def s_Continue(self, node):
        return ["continue"]

    def s_Global(self, node):
        return ["global"] + self.sep([name_words(n) for n in node.names])

    def s_Nonlocal(self, node):
        return ["nonlocal"] + self.sep([name_words(n) for n in node.names])

    def dotted(self, name):
        out = []
        for k, part in enumerate(name.split(".")):
            out += (["dot"] if k else []) + self.ident(part)
        return out

    def aliases(self, names):
        return self.sep([(["star"] if a.name == "*" else self.dotted(a.name))
                         + (["as"] + self.ident(a.asname) if a.asname else []) for a in names])

    def s_Import(self, node):
        return ["import"] + self.aliases(node.names)

    def s_ImportFrom(self, node):
        return ["from"] + ["dot"] * node.level + (self.dotted(node.module) if node.module else []) + ["import"] + self.aliases(node.names)

    def decorator(self, node):
        return ["at"] + self.expr(node)

    def keyword_line(self, word):
        return [word]  # else, try, finally

    # -- whole lines ---------------------------------------------------
    def line(self, item):
        """A Line from lines_of() -> the spoken text."""
        self.condition = False
        if item.kind == "decorator":
            tokens = self.decorator(item.node)
        elif item.kind in ("else", "try", "finally"):
            tokens = self.keyword_line(item.kind)
        elif item.kind == "elif":
            tokens = self.s_If(item.node, head="elif")
        else:
            method = getattr(self, "s_" + type(item.node).__name__, None)
            if method is None:
                raise Unsayable(type(item.node).__name__)
            tokens = method(item.node)
        if item.code.rstrip().endswith(":") and item.kind != "decorator":
            tokens += self.head_end()
        return self.finish(tokens)

    def finish(self, tokens):
        while tokens and tokens[-1] == CLOSE and tokens[-2:-1] != ["colon"]:
            tokens.pop()  # closing brackets at the end go without saying
            # (not after a slice colon: a colon at the end is the block's)
        out = []
        for k, t in enumerate(tokens):
            if isinstance(t, Str):
                out += self.render_string(t, tokens[k + 1] if k + 1 < len(tokens) else None)
            else:
                out += ["close"] if t == CLOSE else [t]
        return " ".join(out)

    def render_string(self, s, nxt):
        """ "string x" ends at a comma, "close" or the end of the line; a
        string holding one of those is quoted instead."""
        words = " ".join(w if isinstance(w, str) and w != CLOSE else "close" for w in s.words).split()
        if not words:
            return ["empty", "string"] if not s.f else ["f", "string", "end", "string"]
        stops = {"comma", "close", "quote"}
        if not s.f and stops & set(words):
            return ["quote"] + words + ["quote"]
        lead = ["f", "string"] if s.f else ["string"]
        if nxt is None or nxt in ("comma", CLOSE):
            return lead + words
        return lead + words + ["end", "string"]


class Explicit(Terse):
    """Every symbol said."""
    style = "explicit"
    CMP = {**Terse.CMP, ast.Eq: "equals equals"}

    def cmp(self, op):
        return self.CMP[type(op)]

    def group(self, inner):
        return ["open", "paren"] + inner + ["close", "paren"]

    def call_open(self):
        return ["open", "paren"]

    def call_close(self):
        return ["close", "paren"]

    def index_open(self):
        return ["open", "bracket"]

    def index_close(self):
        return ["close", "bracket"]

    def list_(self, elts):
        return ["open", "bracket"] + elts + ["close", "bracket"]

    def dict_(self, items):
        return ["open", "brace"] + items + ["close", "brace"]

    set_ = dict_

    def tuple_(self, elts):
        return self.group(elts)

    def head_end(self):
        return ["colon"]

    def s_FunctionDef(self, node):
        out = Terse.s_FunctionDef(self, node)
        k = out.index("taking") if "taking" in out else None
        params = out[k + 1:] if k is not None else []
        ret = []
        if "returns" in params:
            r = params.index("returns")
            params, ret = params[:r], ["arrow"] + params[r + 1:]
        elif k is None and "returns" in out:
            r = out.index("returns")
            out, ret = out[:r], ["arrow"] + out[r + 1:]
        head = out[:k] if k is not None else out
        return head + ["open", "paren"] + params + ["close", "paren"] + ret

    s_AsyncFunctionDef = s_FunctionDef

    def s_ClassDef(self, node):
        out = ["class"] + name_words(node.name)
        parts = self.args(node.bases, node.keywords)
        return out + (self.group(self.sep(parts)) if parts else [])

    def decorator(self, node):
        return ["at", "sign"] + self.expr(node)

    def render_string(self, s, nxt):
        words = " ".join(w if w != CLOSE else "close paren" for w in s.words).split()
        return (["f"] if s.f else []) + ["quote"] + words + ["quote"]


class Natural(Terse):
    """English, with variety: phrases are picked per line from a seeded RNG."""
    style = "natural"
    BINOP = {**Terse.BINOP, ast.Div: ("divided by", "over"), ast.FloorDiv: ("floor divided by", "integer divided by"),
             ast.Mod: ("mod", "modulo"), ast.Pow: ("to the power of", "power"), ast.Mult: ("times",)}
    CMP = {**Terse.CMP, ast.Eq: ("equals", "is equal to"), ast.NotEq: ("is not equal to", "does not equal"),
           ast.Lt: ("is less than", "less than"), ast.LtE: ("is at most", "is less than or equal to"),
           ast.Gt: ("is greater than", "greater than"), ast.GtE: ("is at least", "is greater than or equal to")}
    UNARY = {**Terse.UNARY, ast.USub: ("minus", "negative")}

    def pick(self, *options):
        return self.rng.choice(options)

    def ident(self, name):
        return name_words(name)  # no casing words: the context tells

    def words(self, text):
        return (text if isinstance(text, str) else self.pick(*text)).split()

    def cmp(self, op):
        return self.CMP[type(op)]

    def sep(self, parts, last=True):
        out = []
        for k, p in enumerate(parts):
            if k:
                out.append("and" if k == len(parts) - 1 and last and self.rng.random() < 0.5 else "comma")
            out += p
        return out

    def e_Call(self, node):
        func = self.expr(node.func, 16)
        parts = self.args(node.args, node.keywords)
        if not parts:
            return self.words(self.pick("call", "run")) + func + [CLOSE] if self.rng.random() < 0.5 else func + ["of", CLOSE]
        how = self.pick("of", "with") if isinstance(node.func, ast.Call) else self.pick("call", "of", "with")
        if how == "call":
            return ["call"] + func + ["with"] + self.sep(parts) + [CLOSE]
        return func + [how] + self.sep(parts) + [CLOSE]

    def e_Subscript(self, node):
        how = self.pick("at", "sub", "index")
        return self.expr(node.value, 16) + [how] + self.expr(node.slice) + [CLOSE]

    def list_(self, elts):
        if not elts:
            return self.words(self.pick("empty list", "an empty list"))
        return self.words(self.pick("a list of", "list of", "brackets")) + elts + [CLOSE]

    def dict_(self, items):
        if not items:
            return self.words(self.pick("empty dict", "an empty dictionary"))
        return self.words(self.pick("a dict of", "dict with", "braces")) + items + [CLOSE]

    def set_(self, elts):
        return self.words(self.pick("a set of", "set of")) + elts + [CLOSE]

    def string(self, words):
        return [Str(words)]

    def render_string(self, s, nxt):
        words = " ".join(w if w != CLOSE else "close" for w in s.words).split()
        if not words:
            return self.words(self.pick("empty string", "an empty string"))
        how = self.pick("string", "the string", "quote")
        if {"comma", "close", "quote"} & set(words):
            how = "quote"  # "string x" would end at the comma
        lead = (["f"] if s.f else []) + how.split()
        if how == "quote":
            return lead + words + self.words(self.pick("end quote", "quote"))
        return lead + words + ([] if nxt is None or nxt in ("comma", CLOSE) else ["end", "string"])

    def e_Lambda(self, node):
        params = self.params(node.args)
        return ["lambda"] + params + self.words(self.pick("colon", "returning")) + self.expr(node.body, 1)

    def s_Assign(self, node):
        targets = self.sep([self.expr(t) for t in node.targets], last=False)
        value = self.expr(node.value)
        how = self.pick("set", "equals", "let", "equals")
        if how == "set" and len(node.targets) == 1:
            return ["set"] + targets + ["to"] + value
        if how == "let" and len(node.targets) == 1:
            return ["let"] + targets + ["be"] + value
        return Terse.s_Assign(self, node)

    def s_AugAssign(self, node):
        target, value = self.expr(node.target), self.expr(node.value)
        if isinstance(node.op, ast.Add) and self.rng.random() < 0.6:
            return (["increment"] + target + ["by"] + value if self.rng.random() < 0.5
                    else ["add"] + value + ["to"] + target)
        if isinstance(node.op, ast.Sub) and self.rng.random() < 0.6:
            return (["decrement"] + target + ["by"] + value if self.rng.random() < 0.5
                    else ["subtract"] + value + ["from"] + target)
        return target + self.words(self.binop(node.op)) + ["equals"] + value

    def s_If(self, node, head="if"):
        self.condition = True
        if head == "elif":
            head = self.pick("else if", "elif", "otherwise if")
        return head.split() + self.expr(node.test) + (["then"] if self.rng.random() < 0.2 else [])

    def s_For(self, node):
        out = Terse.s_For(self, node)
        k = out.index("for")
        return out[:k + 1] + (["each"] if self.rng.random() < 0.4 else []) + out[k + 1:]

    def s_While(self, node):
        self.condition = True
        return self.words(self.pick("while", "loop while")) + self.expr(node.test)

    def s_FunctionDef(self, node):
        a = node.args
        method = self.in_method and (a.posonlyargs + a.args)[:1] and (a.posonlyargs + a.args)[0].arg in ("self", "cls")
        if method and self.rng.random() < 0.6:  # self goes without saying
            rest = (a.posonlyargs + a.args)[1:]
            a = ast.arguments(posonlyargs=[], args=rest, vararg=a.vararg, kwonlyargs=a.kwonlyargs,
                              kw_defaults=a.kw_defaults, kwarg=a.kwarg,
                              defaults=a.defaults[-len(rest):] if rest else [])
        params = self.params(a)
        lead = ["async"] if isinstance(node, ast.AsyncFunctionDef) else []
        lead += self.words(self.pick("define method", "def", "method") if method else self.pick("define function", "def", "define", "function"))
        out = lead + name_words(node.name)
        if params:
            out += self.words(self.pick("taking", "with parameters", "of")) + params
        elif self.rng.random() < 0.3:
            out += self.words(self.pick("with no arguments", "taking nothing"))
        if node.returns is not None:
            out += self.words(self.pick("returns", "returning")) + self.expr(node.returns, 1)
        return out

    s_AsyncFunctionDef = s_FunctionDef

    def s_ClassDef(self, node):
        out = self.words(self.pick("class", "define class", "new class")) + name_words(node.name)
        parts = self.args(node.bases, node.keywords)
        if parts:
            out += self.words(self.pick("extends", "inheriting from", "subclassing", "of")) + self.sep(parts)
        return out

    def s_ExceptHandler(self, node):
        out = Terse.s_ExceptHandler(self, node)
        return self.words(self.pick("except", "catch")) + out[1:]

    def s_Raise(self, node):
        out = Terse.s_Raise(self, node)
        return self.words(self.pick("raise", "throw")) + out[1:]

    def keyword_line(self, word):
        return self.words(self.pick("else", "otherwise")) if word == "else" else [word]

    def decorator(self, node):
        return self.words(self.pick("at", "decorator", "decorate with")) + self.expr(node)


class NoSelf(Natural):
    style = "noself"


STYLES = {cls.style: cls for cls in (Terse, Explicit, Natural, NoSelf)}


# -- picking the lines out of a file -------------------------------------
class Line:
    """One dictatable line: a simple statement, a block head, or a decorator."""

    def __init__(self, lineno, code, kind, node, in_method):
        self.lineno, self.code, self.kind, self.node = lineno, code, kind, node
        self.in_method = in_method

    def header_nodes(self):
        """The nodes said on this line (a block head says its test, not its body)."""
        n = self.node
        if n is None:
            return []
        if self.kind == "decorator":
            return [n]
        if isinstance(n, (ast.FunctionDef, ast.AsyncFunctionDef)):
            return [n.args] + ([n.returns] if n.returns else [])
        if isinstance(n, ast.ClassDef):
            return n.bases + n.keywords
        if isinstance(n, (ast.If, ast.While)):
            return [n.test]
        if isinstance(n, (ast.For, ast.AsyncFor)):
            return [n.target, n.iter]
        if isinstance(n, (ast.With, ast.AsyncWith)):
            return n.items
        if isinstance(n, ast.ExceptHandler):
            return [n.type] if n.type else []
        return [n]


KIND = {ast.Assign: "assign", ast.AnnAssign: "annassign", ast.AugAssign: "augassign",
        ast.Return: "return", ast.Import: "import", ast.ImportFrom: "from-import",
        ast.FunctionDef: "def", ast.AsyncFunctionDef: "async-def", ast.ClassDef: "class",
        ast.If: "if", ast.For: "for", ast.AsyncFor: "async-for", ast.While: "while",
        ast.With: "with", ast.AsyncWith: "async-with", ast.ExceptHandler: "except",
        ast.Raise: "raise", ast.Assert: "assert", ast.Delete: "del", ast.Pass: "pass",
        ast.Break: "break", ast.Continue: "continue", ast.Global: "global",
        ast.Nonlocal: "nonlocal", ast.Match: "match"}


def strip_comment(line):
    """A physical line without its trailing comment, or None if it does not
    tokenize on its own (part of a longer statement)."""
    try:
        for tok in tokenize.generate_tokens(io.StringIO(line.strip() + "\n").readline):
            if tok.type == tokenize.COMMENT:
                return line.strip()[:tok.start[1]].rstrip()
    except (tokenize.TokenError, SyntaxError, IndentationError):
        return None
    return line.strip()


def lines_of(src):
    """Every single-line statement, block head and decorator in a module.
    Yields Line; multi-line statements are skipped (kind "multiline")."""
    tree = ast.parse(src)
    phys = src.splitlines()
    methods = set()
    for cls in ast.walk(tree):
        if isinstance(cls, ast.ClassDef):
            for f in cls.body:
                if isinstance(f, (ast.FunctionDef, ast.AsyncFunctionDef)):
                    methods.update(range(f.lineno, (f.end_lineno or f.lineno) + 1))
    out = []
    for node in ast.walk(tree):
        if isinstance(node, ast.If) and phys[node.lineno - 1].lstrip().startswith("elif"):
            kind = "elif"
        elif isinstance(node, ast.Expr):
            v = node.value
            if isinstance(v, ast.Constant) and isinstance(v.value, str):
                continue  # docstrings are prose, not code
            kind = "call" if isinstance(v, ast.Call) else "await" if isinstance(v, ast.Await) else "expr"
        elif isinstance(node, (ast.stmt, ast.ExceptHandler)):
            kind = KIND.get(type(node), type(node).__name__.lower())
        else:
            continue
        for dec in getattr(node, "decorator_list", []):
            if dec.lineno == dec.end_lineno:
                code = strip_comment(phys[dec.lineno - 1])
                if code and code.startswith("@"):
                    out.append(Line(dec.lineno, code, "decorator", dec, dec.lineno in methods))
        body = getattr(node, "body", None)
        if body and isinstance(body, list) and isinstance(body[0], ast.AST):
            # a block head: on one line of its own, body below it
            if body[0].lineno == node.lineno or kind == "match":
                out.append(Line(node.lineno, phys[node.lineno - 1].strip(), "oneline-" + kind if kind != "match" else kind, node, False))
                continue
            code = strip_comment(phys[node.lineno - 1])
            if code is None or not code.endswith(":"):
                out.append(Line(node.lineno, phys[node.lineno - 1].strip(), "multiline", node, False))
                continue
        else:
            if node.lineno != node.end_lineno:
                out.append(Line(node.lineno, phys[node.lineno - 1].strip(), "multiline", node, False))
                continue
            code = ast.get_source_segment(src, node)
        out.append(Line(node.lineno, code, kind, node, node.lineno in methods))
    for k, text in enumerate(phys):
        m = re.match(r"\s*(else|try|finally)\s*:\s*(#.*)?$", text)
        if m:
            out.append(Line(k + 1, m.group(1) + ":", m.group(1), None, False))
    out.sort(key=lambda l: (l.lineno, l.kind != "decorator"))
    return out


def scope_of(tree):
    """Every name a file uses: what a dictation tool would have in scope."""
    names = set()
    for n in ast.walk(tree):
        if isinstance(n, ast.Name):
            names.add(n.id)
        elif isinstance(n, ast.Attribute):
            names.add(n.attr)
        elif isinstance(n, (ast.FunctionDef, ast.AsyncFunctionDef, ast.ClassDef)):
            names.add(n.name)
        elif isinstance(n, ast.arg):
            names.add(n.arg)
        elif isinstance(n, ast.alias):
            names.update((n.asname or n.name).split("."))
            names.update(n.name.split("."))
        elif isinstance(n, ast.keyword) and n.arg:
            names.add(n.arg)
        elif isinstance(n, ast.ExceptHandler) and n.name:
            names.add(n.name)
        elif isinstance(n, ast.ImportFrom) and n.module:
            names.update(n.module.split("."))
    return sorted(names)


FEATURES = {ast.Call: "call", ast.keyword: "kwarg", ast.Attribute: "attribute",
            ast.Subscript: "subscript", ast.Slice: "slice", ast.JoinedStr: "f-string",
            ast.Compare: "compare", ast.BoolOp: "boolop", ast.Lambda: "lambda",
            ast.ListComp: "comprehension", ast.SetComp: "comprehension",
            ast.DictComp: "comprehension", ast.GeneratorExp: "comprehension",
            ast.List: "list", ast.Dict: "dict", ast.Set: "set", ast.IfExp: "ternary",
            ast.Starred: "starred", ast.Await: "await", ast.NamedExpr: "walrus",
            ast.UnaryOp: "unary", ast.BinOp: "binop"}


def features(line):
    """Constructs on a line, for pass rates per construct."""
    tags = set()
    for root in line.header_nodes():
        for n in ast.walk(root):
            t = FEATURES.get(type(n))
            if t:
                tags.add(t)
            if isinstance(n, ast.BinOp):
                tags.add("op " + Terse.BINOP[type(n.op)])
            if isinstance(n, ast.Compare):
                tags.update("op " + Terse.CMP[type(o)] for o in n.ops)
            if isinstance(n, ast.Tuple) and n is not root:
                tags.add("tuple")
            if isinstance(n, ast.Constant):
                v = n.value
                if isinstance(v, str):
                    tags.add("string")
                elif isinstance(v, bytes):
                    tags.add("bytes")
                elif isinstance(v, float):
                    tags.add("float")
                elif isinstance(v, int) and not isinstance(v, bool):
                    tags.add("int" if v < 13 else "int>12")
            if isinstance(n, ast.arguments):
                if n.defaults or any(d is not None for d in n.kw_defaults):
                    tags.add("param default")
                if any(a.annotation for a in n.args + n.posonlyargs + n.kwonlyargs):
                    tags.add("annotation")
                if n.vararg or n.kwarg or n.kwonlyargs:
                    tags.add("star params")
    return tags


def verbalize(line, src="", seed=0, scope=()):
    """A Line -> [(style, spoken)], one per style that can say it, plus the
    errors of those that cannot."""
    out, errors = [], []
    for style, cls in STYLES.items():
        if style == "noself" and not (line.in_method and re.search(r"\bself\.", line.code)):
            continue
        rng = random.Random(f"{seed}:{line.code}:{style}")
        try:
            out.append((style, cls(src, rng, line.in_method, scope).line(line)))
        except Unsayable as e:
            errors.append((style, str(e)))
    return out, errors


if __name__ == "__main__":
    src = open(sys.argv[1]).read()
    scope = scope_of(ast.parse(src))
    for line in lines_of(src):
        if line.kind.startswith(("multiline", "oneline")):
            continue
        said, errors = verbalize(line, src, scope=scope)
        print(f"{line.lineno:4} {line.kind:10} {line.code}")
        for style, text in said:
            print(f"     {style:8} {text}")
        for style, err in errors:
            print(f"     {style:8} UNSAYABLE {err}")
