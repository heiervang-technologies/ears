"""PieceDecoder, the stand-in for `tokenizers` where it has no wheel (armv7 musl).

    python -m unittest test_piece_decoder     (from moonshine-stream/)

Checked against tokenizers 0.23.3 on the tiny model's tokenizer.json: 50,000
sequences (random ids and pure byte-fallback runs), 0 mismatches.
"""

import json
import tempfile
import unittest
from pathlib import Path

import engine

VOCAB = {"<unk>": 0, "<s>": 1, "</s>": 2, "▁hello": 3, "▁world": 4, ".": 5,
         "<0xC3>": 6, "<0xA6>": 7, "<0xFF>": 8, "▁gr": 9, "øt": 10}


def decoder():
    t = {"model": {"type": "BPE", "vocab": VOCAB},
         "added_tokens": [{"id": 1, "content": "<s>", "special": True},
                          {"id": 2, "content": "</s>", "special": True}]}
    d = tempfile.TemporaryDirectory()
    path = Path(d.name) / "tokenizer.json"
    path.write_text(json.dumps(t))
    dec = engine.PieceDecoder(path)
    d.cleanup()
    return dec


class PieceDecoderTest(unittest.TestCase):
    def test_spaces_and_leading_strip(self):
        self.assertEqual(decoder().decode([3, 4, 5]), "hello world.")

    def test_special_tokens_are_skipped(self):
        self.assertEqual(decoder().decode([1, 3, 2]), "hello")

    def test_byte_fallback_fuses_utf8(self):
        self.assertEqual(decoder().decode([3, 6, 7]), "helloæ")   # C3 A6 = æ

    def test_invalid_byte_run_is_one_replacement_per_byte(self):
        self.assertEqual(decoder().decode([3, 6, 8]), "hello��")

    def test_vocab_and_unknown_ids(self):
        d = decoder()
        self.assertEqual(d.get_vocab()["▁gr"], 9)
        self.assertEqual(d.decode([9, 10, 999]), "grøt")


if __name__ == "__main__":
    unittest.main()
