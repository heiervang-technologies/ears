"""Import the hover-page script (no .py suffix) as a module for the tests."""
from importlib.machinery import SourceFileLoader
from importlib.util import module_from_spec, spec_from_loader
from pathlib import Path

_path = str(Path(__file__).resolve().parent.parent / "hover-page")
_loader = SourceFileLoader("hover_page", _path)
hp = module_from_spec(spec_from_loader("hover_page", _loader))
_loader.exec_module(hp)
