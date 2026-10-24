"""Route utterances through the real voice-cmd with hover-page.toml loaded in front of
voice-cmd's own commands (as ~/.config/voice-commands is): page phrases resolve to the
right argv, existing hover/Hyprland phrases keep winning, dictation passes through.

Skipped when voice-cmd is not installed ($VOICE_CMD overrides the lookup).
"""
import os, shutil, unittest
from importlib.machinery import SourceFileLoader
from importlib.util import module_from_spec, spec_from_loader
from pathlib import Path

HERE = Path(__file__).resolve().parent.parent
VOICE_CMD = os.environ.get("VOICE_CMD") or shutil.which("voice-cmd")
vc = None
if VOICE_CMD:
    real = Path(VOICE_CMD).resolve()
    os.environ["VOICE_CMD_DIRS"] = f"{HERE}:{real.parent / 'commands'}"
    os.environ["VOICE_CMD_APP"] = "hover"
    loader = SourceFileLoader("voice_cmd", str(real))
    vc = module_from_spec(spec_from_loader("voice_cmd", loader))
    loader.exec_module(vc)

# utterance (as ASR might give it) -> (command name, argv after "hover-page")
PAGE = {
    "Click on subscriptions.": ("page.click", ["--notify", "click", "subscriptions"]),
    "click the search button": ("page.click", ["--notify", "click", "the search button"]),
    "Tap watch later": ("page.click", ["--notify", "click", "watch later"]),
    "press the sign up button": ("page.click", ["--notify", "click", "the sign up button"]),
    "Go to settings.": ("page.click", ["--notify", "click", "settings"]),
    "go to the search bar": ("page.click", ["--notify", "click", "the search bar"]),
    "Focus the search box.": ("page.focus", ["--notify", "focus", "the search box"]),
    "Menu.": ("page.menu", ["--notify", "click", "menu"]),
    "open the menu": ("page.menu", ["--notify", "click", "menu"]),
    "Hamburger menu": ("page.menu", ["--notify", "click", "menu"]),
    "open the main menu": ("page.menu", ["--notify", "click", "menu"]),
    "open the account menu": ("page.open-menu", ["--notify", "click", "account menu"]),
    "Search for rust tutorials.": ("page.search", ["--notify", "type", "--submit", "search", "rust tutorials"]),
    "search lofi beats": ("page.search", ["--notify", "type", "--submit", "search", "lofi beats"]),
    "search youtube for cat videos": ("page.search", ["--notify", "type", "--submit", "search", "cat videos"]),
    "Type into first name: Markus.": ("page.type-in", ["--notify", "type", "--split", "first name markus"]),
    "type in search rust in action": ("page.type-in", ["--notify", "type", "--split", "search rust in action"]),
    "type hello world into the comment box": ("page.type-into-field", ["--notify", "type", "comment box", "hello world"]),
    "type hi there in the comment box": ("page.type-into-field", ["--notify", "type", "comment box", "hi there"]),
    "Scroll down.": ("page.scroll", ["scroll", "down", ""]),
    "scroll down three": ("page.scroll", ["scroll", "down", "3"]),
    "scroll up a bit": ("page.scroll", ["scroll", "up", "a bit"]),
    "Scroll down two pages.": ("page.scroll", ["scroll", "down", "2"]),
    "page down": ("page.page-updown", ["scroll", "down"]),
    "go to the top": ("page.scroll-ends", ["scroll", "top"]),
    "scroll to bottom": ("page.scroll-ends", ["scroll", "bottom"]),
    "Go back.": ("page.back", ["back"]),
    "back": ("page.back", ["back"]),
    "go forward": ("page.forward", ["forward"]),
    "Reload the page.": ("page.reload", ["reload"]),
    "refresh": ("page.reload", ["reload"]),
    "okay, now click on history, please": ("page.click", ["--notify", "click", "history"]),
}

# existing phrases that must keep their command
EXISTING = {
    "open the sidebar": "hover.expand",
    "open": "hover.expand",
    "expand it": "hover.expand",
    "show the sidebar": "hover.sidebar",
    "toggle sidebar": "hover.sidebar",
    "go to workspace 3": "hypr.workspace",
    "focus left": "hypr.focus",
    "next space": "hover.space-next",
    "pin this tab": "hover.pin",
    "copy the link": "hover.copy-url",
    "open the browser": "app.browser",
    "tidy up my tabs": "hover.tidy",
    "split view": "hover.split",
    "close the window": "hypr.close",
    "full screen": "hypr.fullscreen",
}

# dictation that must not be swallowed by a page command
DICTATION = [
    "click on the link in the email to confirm your account please",
    "I clicked on it yesterday",
    "search for the bug in the parser before we ship the release tomorrow",
    "we should go back to the drawing board on this one",
    "type checking in rust is great",
    "press enter",
    "back 3",
    "the menu at that restaurant was great",
    "scroll down the page until you see the comments section below",
    "go to the store and buy some milk and eggs for breakfast",
]


@unittest.skipUnless(vc, "voice-cmd not installed")
class Phrases(unittest.TestCase):
    def match(self, text):
        return vc.match(text, {"hovered": "hover", "focused": "hover"})

    def test_page_phrases(self):
        for text, (name, argv) in PAGE.items():
            with self.subTest(text=text):
                hit = self.match(text)
                self.assertIsNotNone(hit)
                self.assertEqual((hit["name"], hit["argv"][1:]), (name, argv))
                self.assertEqual(hit["argv"][0], "hover-page")

    def test_existing_commands_still_win(self):
        for text, name in EXISTING.items():
            with self.subTest(text=text):
                hit = self.match(text)
                self.assertEqual(hit and hit["name"], name)

    def test_dictation_passes_through(self):
        for text in DICTATION:
            with self.subTest(text=text):
                hit = self.match(text)
                self.assertFalse(hit and hit["name"].startswith("page."), hit)

    def test_page_commands_need_hover(self):
        hit = vc.match("click on subscriptions", {"hovered": "Alacritty", "focused": "Alacritty"})
        self.assertIsNone(hit)


if __name__ == "__main__":
    unittest.main()
