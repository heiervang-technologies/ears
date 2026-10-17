"""Round trip: real code -> verbalize.py (terse, explicit) -> spoken.py -> code.

How much real code the spoken.py rules can take back. A line passes when its
AST matches the original; "lenient" also forgives the case and spacing
inside strings (words lose both). For each failing line, the smallest
sub-expression that fails on its own names the cause.

Usage: python roundtrip.py [--files N] [--lines N] [--json OUT]
"""
import argparse
import ast
import collections
import json
import random
import re
import sys

import corpus
import spoken
from verbalize import STYLES, Terse, Unsayable, features, lines_of, name_words, scope_of

SKIP = ("multiline", "oneline", "match")


def canon(code, lenient=False):
    """Code -> ast.dump, for a line that may be a block head on its own."""
    code = code.strip()
    if code in ("else:", "try:", "finally:"):
        return code
    if code.startswith("elif "):
        code = code[2:]
    if code.startswith("except"):
        src = "try:\n    pass\n" + code + "\n    pass"
    elif code.startswith("@"):
        src = code + "\ndef _(): pass"
    elif code.endswith(":"):
        src = code + "\n    pass"
    else:
        src = code
    tree = ast.parse(src)
    if lenient:
        for n in ast.walk(tree):
            if isinstance(n, ast.Constant) and isinstance(n.value, (str, bytes)):
                v = n.value.decode("latin-1") if isinstance(n.value, bytes) else n.value
                n.value = re.sub(r"[\W_]", "", v.lower())
    return ast.dump(tree)


def same(original, got, lenient=False):
    try:
        return canon(original, lenient) == canon(got, lenient)
    except SyntaxError:
        return False


def back(spoken_text, scope):
    """What spoken.py makes of it, as the plugin would insert it."""
    lines = spoken.code_lines("py", spoken_text, scope)
    if len(lines) != 1 or lines[0]["shift"]:
        return " / ".join(l["code"] for l in lines) + " [split by a layout word]"
    return lines[0]["code"]


STANDALONE = (ast.expr,)
NOT_ALONE = (ast.Slice, ast.Starred, ast.FormattedValue, ast.expr_context)


def label(node, said):
    """A short name for the construct a failing sub-expression is."""
    if isinstance(node, ast.Name):
        words = name_words(node.id)
        if any(spoken.structural(w) or w in spoken.SYMBOLS or w in spoken.HEADS for w in words):
            return "name: is a spoken keyword (" + node.id + ")" if len(words) == 1 else "name: holds a spoken keyword"
        if any(re.search(r"\d", w) for w in words):
            return "name: has digits"
        return "name: other"
    if isinstance(node, ast.Constant):
        v = node.value
        if isinstance(v, str):
            words = said.split()
            if any(w in spoken.NUMBERS or w.isdigit() for w in words[1:]):
                return "string: holds numbers"
            if any(w in ("dot", "slash", "colon", "dash") for w in words):
                return "string: punctuation spacing"
            if any(w in spoken.SYMBOLS or spoken.structural(w) for w in words[1:]):
                return "string: holds a spoken keyword"
            return "string: other"
        if isinstance(v, bool) or v is None:
            return "constant"
        if isinstance(v, int):
            return "int: " + ("13-19" if 13 <= v <= 19 else ">=100" if v >= 100 else "other")
        return type(v).__name__
    if isinstance(node, ast.BinOp):
        return "binop " + Terse.BINOP[type(node.op)]
    if isinstance(node, ast.Compare):
        return "compare " + "/".join(Terse.CMP[type(o)] for o in node.ops)
    if isinstance(node, ast.UnaryOp):
        return "unary " + Terse.UNARY[type(node.op)]
    if isinstance(node, ast.Call):
        if not node.args and not node.keywords:
            return "call: no args"
        if any(k.arg is None for k in node.keywords) or any(isinstance(a, ast.Starred) for a in node.args):
            return "call: star args"
        if isinstance(node.func, ast.Name) and node.func.id in spoken.PY_CALLABLES:
            return "call: builtin"
        return "call" + (": kwargs" if node.keywords else "")
    if isinstance(node, ast.Subscript):
        return "subscript" + (": slice" if isinstance(node.slice, ast.Slice) else ": tuple" if isinstance(node.slice, ast.Tuple) else "")
    if isinstance(node, (ast.ListComp, ast.SetComp, ast.DictComp, ast.GeneratorExp)):
        return "comprehension"
    return type(node).__name__.lower()


def culprit(line, src, scope):
    """The smallest sub-expression that fails on its own, else the statement."""
    nodes = []
    for root in line.header_nodes():
        for n in ast.walk(root):
            if isinstance(n, STANDALONE) and not isinstance(n, NOT_ALONE):
                nodes.append(n)
    nodes.sort(key=lambda n: len(ast.unparse(n)))
    for n in nodes:
        try:
            t = Terse(src, scope=scope)
            said = t.finish(t.expr(n))
        except Unsayable:
            continue
        code = ast.unparse(n)
        if not same(code, back(said, scope), lenient=True):
            # a lone name is the statement head: no fault of the name
            if isinstance(n, ast.Name) and back(said, scope) == said.replace(" ", "_"):
                continue
            return label(n, said)
    return "statement: " + line.kind + statement_detail(line)


def statement_detail(line):
    tags = features(line)
    for t in ("param default", "annotation", "star params"):
        if t in tags:
            return " (" + t + ")"
    n = line.node
    if isinstance(n, ast.ClassDef) and (n.bases or n.keywords):
        return " (bases)"
    if isinstance(n, (ast.FunctionDef, ast.AsyncFunctionDef)) and n.returns:
        return " (return annotation)"
    if isinstance(n, (ast.Assign, ast.AugAssign, ast.AnnAssign)):
        return ""
    if isinstance(n, ast.ImportFrom) and n.level:
        return " (relative)"
    return ""


def run(per_source, per_file, seed=0):
    rows = []
    rng = random.Random(seed)
    for source, paths in corpus.files(per_source, seed).items():
        for path in paths:
            try:
                src = open(path, encoding="utf-8").read()
                tree = ast.parse(src)
            except (SyntaxError, UnicodeDecodeError, ValueError):
                continue
            scope = scope_of(tree)
            lines = [l for l in lines_of(src)]
            for l in lines:
                if l.kind.startswith(SKIP):
                    rows.append({"source": source, "kind": l.kind, "skipped": True})
            lines = [l for l in lines if not l.kind.startswith(SKIP)]
            for line in rng.sample(lines, min(per_file, len(lines))):
                row = {"source": source, "path": path, "lineno": line.lineno, "kind": line.kind,
                       "code": line.code, "features": sorted(features(line))}
                for style in ("terse", "explicit"):
                    try:
                        said = STYLES[style](src, scope=scope).line(line)
                    except Unsayable as e:
                        row[style] = {"unsayable": str(e)}
                        continue
                    got = back(said, scope)
                    r = {"said": said, "got": got, "exact": same(line.code, got),
                         "lenient": same(line.code, got, lenient=True)}
                    if style == "terse" and not r["lenient"]:
                        r["culprit"] = culprit(line, src, scope)
                    row[style] = r
                rows.append(row)
    return rows


def pct(a, b):
    return f"{100 * a / b:5.1f}%" if b else "   - "


def report(rows, out=sys.stdout):
    done = [r for r in rows if not r.get("skipped")]
    skipped = collections.Counter(r["kind"].split("-")[0] for r in rows if r.get("skipped"))
    p = lambda *a: print(*a, file=out)
    p(f"lines sampled: {len(done)} (from {len(set(r['path'] for r in done))} files); "
      f"not single-line, not sampled: {dict(skipped)}")
    for style in ("terse", "explicit"):
        said = [r[style] for r in done if "said" in r[style]]
        uns = len(done) - len(said)
        p(f"{style:8} unsayable {pct(uns, len(done))}  exact {pct(sum(s['exact'] for s in said), len(done))}"
          f"  lenient {pct(sum(s['lenient'] for s in said), len(done))}")
    p("\nby source (terse, lenient):")
    for source in sorted(set(r["source"] for r in done)):
        rs = [r for r in done if r["source"] == source]
        p(f"  {source:14} {len(rs):5}  {pct(sum(r['terse'].get('lenient', False) for r in rs), len(rs))}")

    def table(key_fn, title):
        p(f"\n{title}:  n  terse-exact  terse-lenient  explicit-lenient")
        groups = collections.defaultdict(list)
        for r in done:
            for k in key_fn(r):
                groups[k].append(r)
        for k, rs in sorted(groups.items(), key=lambda kv: -len(kv[1])):
            p(f"  {k:24} {len(rs):5}  {pct(sum(r['terse'].get('exact', False) for r in rs), len(rs))}"
              f"  {pct(sum(r['terse'].get('lenient', False) for r in rs), len(rs))}"
              f"  {pct(sum(r['explicit'].get('lenient', False) for r in rs), len(rs))}")
    table(lambda r: [r["kind"]], "by statement")
    table(lambda r: r["features"], "by construct on the line")
    fails = [r for r in done if "said" in r["terse"] and not r["terse"]["lenient"]]
    causes = collections.Counter(r["terse"]["culprit"] for r in fails)
    uns = collections.Counter(r["terse"]["unsayable"] for r in done if "unsayable" in r["terse"])
    p(f"\nterse failures by cause ({len(fails)} lines; share of all sampled lines):")
    for cause, n in causes.most_common(40):
        ex = next(r for r in fails if r["terse"]["culprit"] == cause)
        p(f"  {n:5} {pct(n, len(done))}  {cause:42} e.g. {ex['code'][:60]!r}")
    p(f"\nunsayable (terse): {dict(uns.most_common())}")


if __name__ == "__main__":
    ap = argparse.ArgumentParser()
    ap.add_argument("--files", type=int, default=60, help="files per source")
    ap.add_argument("--lines", type=int, default=40, help="lines sampled per file")
    ap.add_argument("--seed", type=int, default=0)
    ap.add_argument("--json", help="write every row here")
    a = ap.parse_args()
    rows = run(a.files, a.lines, a.seed)
    if a.json:
        with open(a.json, "w") as f:
            for r in rows:
                f.write(json.dumps(r) + "\n")
    report(rows)
