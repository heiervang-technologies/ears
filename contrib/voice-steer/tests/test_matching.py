"""Offline tests for hover-page's matching, on candidates captured from tests/fixtures.

  python3 -m unittest discover -s tests          (from contrib/voice-steer)
  python3 tests/test_matching.py --report [--embed]   accuracy per case set
"""
import json, sys, unittest
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))
from cases import CASES, HELD_OUT, KNOWN_MISSES  # noqa: E402
from hp import hp  # noqa: E402

FIX = Path(__file__).resolve().parent / "fixtures"


def load(name):
    return json.loads((FIX / f"{name}.cands.json").read_text())


def pick(fixture, verb, target, embed=False):
    ranked = hp.rank(target, load(fixture), verb, embed=embed)
    if not ranked or ranked[0][0] < hp.THRESHOLD:
        return None, ranked
    return ranked[0][1]["key"], ranked


def accuracy(cases, embed=False):
    misses = []
    for fixture, verb, target, want in cases:
        got, ranked = pick(fixture, verb, target, embed)
        if got != want:
            top = ", ".join(f"{c['key']}={s:.2f}" for s, c in ranked[:3])
            misses.append(f"{fixture}/{verb} {target!r}: want {want}, got {got} [{top}]")
    return 1 - len(misses) / len(cases), misses


class Matching(unittest.TestCase):
    def test_cases(self):
        cases = [c for c in CASES + HELD_OUT if c[2] not in KNOWN_MISSES]
        acc, misses = accuracy(cases)
        self.assertEqual(misses, [], f"accuracy {acc:.0%}")

    def test_query_parsing(self):
        toks, content, roles = hp.parse_query("the hamburger menu")
        self.assertEqual(content, [])
        self.assertEqual(roles, {"menu"})
        self.assertEqual(hp.parse_query("search text box")[2], {"field"})

    def test_text_sim(self):
        self.assertEqual(hp.text_sim(["watch", "later"], "Watch later"), 1.0)
        self.assertGreater(hp.text_sim(["subscription"], "Subscriptions"), 0.8)
        self.assertLess(hp.text_sim(["weather"], "Watch later"), 0.3)

    def test_focus_only_considers_fields(self):
        for s, c in hp.rank("search", load("youtube"), "focus"):
            self.assertTrue(c["editable"], c)

    def test_scroll_amount(self):
        self.assertEqual(hp.scroll_amount([]), 1.0)
        self.assertEqual(hp.scroll_amount(["3"]), 3.0)
        self.assertEqual(hp.scroll_amount(["a", "lot"]), 3.0)
        self.assertLess(hp.scroll_amount(["a", "bit"]), 1)


if __name__ == "__main__":
    if "--report" in sys.argv:  # accuracy per set, fuzzy and (with --embed) EmbeddingGemma
        for mode in (False, True) if "--embed" in sys.argv else (False,):
            for label, cases in (("tuning", CASES), ("held-out", HELD_OUT)):
                acc, misses = accuracy(cases, embed=mode)
                n = len(cases)
                print(f"{'embed' if mode else 'fuzzy'} {label}: {acc:.1%} ({n - len(misses)}/{n})")
                for m in misses:
                    print("   ", m)
    else:
        unittest.main()
