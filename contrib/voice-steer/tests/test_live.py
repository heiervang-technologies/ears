"""End-to-end tests: hover-page really clicks and types in fixture pages.

Runs only when HOVER_PAGE_LIVE_PORT points at a hover you do not mind being driven,
ideally a throwaway headless instance (see README). Each test opens its own tab.
"""
import os, subprocess, sys, time, unittest
from pathlib import Path

from hp import hp

PORT = os.environ.get("HOVER_PAGE_LIVE_PORT")
HERE = Path(__file__).resolve().parent
SCRIPT = HERE.parent / "hover-page"


def bidi(fn):
    b = hp.Bidi.connect(int(PORT))
    try:
        return fn(b)
    finally:
        b.close()


def page_eval(tab, js):
    return bidi(lambda b: b.run(b.tab_by_url(tab), js, sandbox=None))


@unittest.skipUnless(PORT, "set HOVER_PAGE_LIVE_PORT to a disposable hover's BiDi port")
class Live(unittest.TestCase):
    def open(self, fixture):
        url = (HERE / "fixtures" / fixture).as_uri()

        def go(b):
            ctx = b.call("browsingContext.create", {"type": "tab", "background": True})["context"]
            b.call("browsingContext.navigate", {"context": ctx, "url": url, "wait": "complete"})
            return url
        self.url = bidi(go)
        self.tab = fixture

    def tearDown(self):
        def go(b):
            tree = b.call("browsingContext.getTree", {"maxDepth": 0})
            for c in tree["contexts"]:
                if c["url"].startswith(self.url.split("#")[0].split("?")[0]):
                    b.call("browsingContext.close", {"context": c["context"]})
        if getattr(self, "tab", None):
            bidi(go)

    def hp(self, *args):
        r = subprocess.run([sys.executable, str(SCRIPT), "--port", PORT, "--tab", self.tab, *args],
                           capture_output=True, text=True, timeout=10)
        return r.returncode, (r.stdout + r.stderr).strip()

    def js(self, expr):
        time.sleep(0.15)  # let navigation / smooth scroll settle
        return page_eval(self.tab, f"return {expr}")

    def test_click_link(self):
        self.open("youtube.html")
        self.assertEqual(self.hp("click", "subscriptions")[0], 0)
        self.assertEqual(self.js("location.hash"), "#subs")
        self.assertEqual(self.hp("back")[0], 0)
        self.assertEqual(self.js("location.hash"), "")

    def test_search_submits(self):
        self.open("youtube.html")
        code, out = self.hp("type", "--submit", "search", "rust tutorials")
        self.assertEqual(code, 0, out)
        self.assertIn("search_query=rust+tutorials", self.js("location.href"))

    def test_form(self):
        self.open("form.html")
        self.assertEqual(self.hp("type", "email address", "a@b.c")[0], 0)
        self.assertEqual(self.hp("type", "comment box", "hello there")[0], 0)
        self.assertEqual(self.hp("click", "terms of service")[0], 0)
        self.assertEqual(self.js("document.querySelector('[data-t=email]').value"), "a@b.c")
        self.assertEqual(self.js("document.querySelector('[data-t=comment]').textContent"), "hello there")
        self.assertTrue(self.js("document.querySelector('[data-t=tos]').checked"))
        self.assertEqual(self.hp("click", "sign up")[0], 0)
        self.assertEqual(self.js("document.title"), "submitted:a@b.c")

    def test_focus_and_details(self):
        self.open("docs.html")
        self.assertEqual(self.hp("focus", "search docs")[0], 0)
        self.assertEqual(self.js("document.activeElement.id"), "docsearch")
        self.assertEqual(self.hp("click", "appendix")[0], 0)
        self.assertTrue(self.js("document.querySelector('details').open"))

    def test_scroll(self):
        self.open("youtube.html")
        self.assertEqual(self.hp("scroll", "down", "2")[0], 0)
        time.sleep(0.6)
        self.assertGreater(self.js("scrollY"), 100)
        self.assertEqual(self.hp("scroll", "top")[0], 0)
        time.sleep(0.6)
        self.assertEqual(self.js("scrollY"), 0)

    def test_dry_and_no_match(self):
        self.open("form.html")
        code, out = self.hp("--dry", "click", "terms of service")
        self.assertEqual(code, 0)
        self.assertTrue(out.startswith("would click checkbox"), out)
        self.assertFalse(self.js("document.querySelector('[data-t=tos]').checked"))
        self.assertEqual(self.hp("click", "delete my account")[0], 1)


if __name__ == "__main__":
    unittest.main()
