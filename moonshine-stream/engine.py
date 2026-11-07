"""Incremental Moonshine Streaming inference on ONNX Runtime.

Runs the official split export (frontend, encoder, adapter, cross_kv,
decoder_kv) the way the upstream C++ runtime does, so new audio costs only
its own encoding:

* **Frontend** is stateful (sample and conv buffers): audio goes in once.
* **Encoder** attends within sliding windows, 16 frames of left context per
  layer and 16 frames of total lookahead. A frame is final once 16 newer
  frames exist; only the new frames are encoded, with their left context.
* **Adapter and cross K/V** are per frame, so memory and the decoder's
  cross-attention cache are append-only.
* **Decoder** re-runs per update. Settled tokens are forced; the rest of the
  previous hypothesis is a speculative draft, verified in the same batched
  pass, so the autoregressive loop only runs from the first changed token.
"""

import json
import math
from pathlib import Path

import numpy as np
import onnxruntime as ort

try:
    from tokenizers import Tokenizer
except ImportError:  # no wheel for armv7 musl (Alpine): decode in Python
    Tokenizer = None

SAMPLE_RATE = 16000
# The exported frontend graphs need chunks in multiples of 80 samples (a
# remainder is silently dropped by graphs exported before 2026-08-23).
FRAME_SAMPLES = 80
CHUNK = 1280
# Encoder frames are 20 ms.
FRAME_S = 0.020
# Upstream's cap against hallucination loops.
TOKENS_PER_SECOND = 6.5
MAX_TOKENS = 448
# The adapter's learned positions: 4096 frames, about 82 s.
MAX_FRAMES = 4096


class PieceDecoder:
    """Decode-only stand-in for `tokenizers.Tokenizer` with this model's
    tokenizer.json (SentencePiece-style BPE: "▁" for a space, <0xNN>
    byte fallback, one leading space stripped, special tokens skipped).
    Enough for the engine, which only decodes and reads the vocabulary."""

    def __init__(self, path):
        t = json.loads(Path(path).read_text())
        self.vocab = dict(t["model"]["vocab"])
        for added in t.get("added_tokens", []):
            self.vocab.setdefault(added["content"], added["id"])
        self.pieces = {i: p for p, i in self.vocab.items()}
        self.special = {a["id"] for a in t.get("added_tokens", []) if a.get("special")}

    @staticmethod
    def _bytes(run):
        """A run of <0xNN> pieces: its UTF-8 text, or (as tokenizers'
        ByteFallback does) one U+FFFD per byte if the run is not valid."""
        try:
            return bytes(run).decode("utf-8")
        except UnicodeDecodeError:
            return "\ufffd" * len(run)

    def get_vocab(self):
        return dict(self.vocab)

    def decode(self, ids):
        out, pending = [], bytearray()
        for i in ids:
            if i in self.special:
                continue
            piece = self.pieces.get(i)
            if piece is None:
                continue
            if len(piece) == 6 and piece.startswith("<0x") and piece.endswith(">"):
                pending.append(int(piece[3:5], 16))
                continue
            if pending:
                out.append(self._bytes(pending))
                pending.clear()
            out.append(piece.replace("\u2581", " "))
        if pending:
            out.append(self._bytes(pending))
        text = "".join(out)
        return text.removeprefix(" ")


class Model:
    def __init__(self, model_dir, threads=None):
        d = Path(model_dir)
        self.cfg = json.loads((d / "streaming_config.json").read_text())
        so = ort.SessionOptions()
        if threads:
            so.intra_op_num_threads = threads
            so.inter_op_num_threads = 1

        def load(name):
            for ext in (".ort", ".onnx"):
                p = d / (name + ext)
                if p.exists():
                    return ort.InferenceSession(str(p), so, providers=["CPUExecutionProvider"])
            raise FileNotFoundError(d / name)

        self.frontend = load("frontend")
        self.encoder = load("encoder")
        self.adapter = load("adapter")
        self.cross_kv = load("cross_kv")
        self.decoder = load("decoder_kv")
        path = d / "tokenizer.json"
        self.tok = Tokenizer.from_file(str(path)) if Tokenizer else PieceDecoder(path)
        self.bos = self.cfg["bos_id"]
        self.eos = self.cfg["eos_id"]
        vocab = self.tok.get_vocab()
        self.word_start = np.zeros(self.cfg["vocab_size"], bool)
        self.sentence_end = np.zeros(self.cfg["vocab_size"], bool)
        for piece, i in vocab.items():
            if i >= len(self.word_start):
                continue
            self.word_start[i] = piece.startswith("▁")
            self.sentence_end[i] = piece.rstrip("\"')").endswith((".", "?", "!"))

    def text(self, tokens):
        return self.tok.decode(list(tokens))

    def stream(self):
        return Stream(self)

    def transcribe(self, audio):
        """Whole clip at once (float32, 16 kHz mono)."""
        s = Stream(self)
        s.feed(audio)
        s.finish()
        return self.text(s.decode())


class Stream:
    """One utterance: audio in, hypotheses out."""

    def __init__(self, model):
        self.m = model
        c = model.cfg
        self.sample_buffer = np.zeros((1, 79), np.float32)
        self.sample_len = np.zeros(1, np.int64)
        self.conv1 = np.zeros((1, c["d_model_frontend"], 4), np.float32)
        self.conv2 = np.zeros((1, c["c1"], 4), np.float32)
        self.frame_count = np.zeros(1, np.int64)
        self.pending = np.zeros(0, np.float32)
        self.features = np.zeros((0, c["encoder_dim"]), np.float32)
        self.emitted = 0
        kv = (c["depth"], 1, c["nheads"], 0, c["head_dim"])
        self.k_cross = np.zeros(kv, np.float32)
        self.v_cross = np.zeros(kv, np.float32)
        self.empty_kv = np.zeros(kv, np.float32)
        self.samples = 0
        self.finished = False
        # Stats of the last decode.
        self.last_draft_accepted = 0
        self.last_steps = 0

    @property
    def frames(self):
        """Encoder frames in memory (20 ms each)."""
        return self.k_cross.shape[3]

    def feed(self, audio):
        """Append float32 samples; runs the frontend on whole chunks."""
        self.samples += len(audio)
        buf = np.concatenate([self.pending, np.asarray(audio, np.float32)])
        n = len(buf) // CHUNK * CHUNK
        if n:
            self._frontend(buf[:n])
        self.pending = buf[n:]

    def _frontend(self, audio):
        out = self.m.frontend.run(None, {
            "audio_chunk": audio[None, :],
            "sample_buffer": self.sample_buffer,
            "sample_len": self.sample_len,
            "conv1_buffer": self.conv1,
            "conv2_buffer": self.conv2,
            "frame_count": self.frame_count,
        })
        feats, self.sample_buffer, self.sample_len, self.conv1, self.conv2, self.frame_count = out
        if feats.shape[1]:
            self.features = np.concatenate([self.features, feats[0]])

    def finish(self):
        """No more audio: flush the remainder and the encoder lookahead."""
        if self.finished:
            return
        self.finished = True
        if len(self.pending):
            pad = -len(self.pending) % FRAME_SAMPLES
            self._frontend(np.concatenate([self.pending, np.zeros(pad, np.float32)]))
            self.pending = np.zeros(0, np.float32)
        self.encode()

    def encode(self):
        """Encode every frame that is final; returns how many were added."""
        c = self.m.cfg
        total = min(len(self.features), MAX_FRAMES)
        stable = total if self.finished else max(0, total - c["total_lookahead"])
        new = stable - self.emitted
        if new <= 0:
            return 0
        start = max(0, self.emitted - 16 * c["depth"])
        encoded = self.m.encoder.run(None, {"features": self.features[None, start:total]})[0][0]
        fresh = encoded[self.emitted - start:self.emitted - start + new]
        memory = self.m.adapter.run(None, {
            "encoded": fresh[None],
            "pos_offset": np.array([self.emitted], np.int64),
        })[0]
        k, v = self.m.cross_kv.run(None, {"memory": memory})
        self.k_cross = np.concatenate([self.k_cross, k], axis=3)
        self.v_cross = np.concatenate([self.v_cross, v], axis=3)
        self.emitted = stable
        return new

    def _run(self, tokens, k_self, v_self):
        logits, k, v = self.m.decoder.run(["logits", "out_k_self", "out_v_self"], {
            "token": np.array([tokens], np.int64),
            "k_self": k_self,
            "v_self": v_self,
            "out_k_cross": self.k_cross,
            "out_v_cross": self.v_cross,
        })
        return logits[0], k, v

    def decode(self, forced=(), draft=()):
        """Greedy transcript tokens (no BOS/EOS).

        `forced` is kept as is; `draft` is accepted as far as the model
        agrees with it. Both run in one batched pass.
        """
        forced, draft = list(forced), list(draft)
        self.last_draft_accepted = self.last_steps = 0
        if self.frames == 0:
            return forced
        limit = min(math.ceil(self.frames * FRAME_S * TOKENS_PER_SECOND), MAX_TOKENS)
        seq = [self.m.bos] + forced + draft
        logits, k, v = self._run(seq, self.empty_kv, self.empty_kv)
        preds = logits.argmax(-1)
        out = forced
        accepted = 0
        for j, t in enumerate(draft):
            if preds[len(forced) + j] != t:
                break
            accepted += 1
        out += draft[:accepted]
        # Self-attention is causal: the cache of the accepted prefix is exact.
        n = 1 + len(out)
        k, v = k[:, :, :, :n], v[:, :, :, :n]
        nxt = int(preds[n - 1])
        self.last_draft_accepted = accepted
        self.last_steps = 0
        # Past a divergence the rest of the draft usually still follows (a
        # word or a comma changed, not the sentence): propose it again from
        # where the new text rejoins it and verify it in one pass.
        tail = draft[accepted:]
        while nxt != self.m.eos and len(out) < limit:
            out.append(nxt)
            proposal = lookup(out, tail)[:limit - len(out)]
            logits, k, v = self._run([nxt] + proposal, k, v)
            preds = logits.argmax(-1)
            ok = 0
            while ok < len(proposal) and preds[ok] == proposal[ok]:
                ok += 1
            out += proposal[:ok]
            self.last_draft_accepted += ok
            n = 1 + len(out)
            k, v = k[:, :, :, :n], v[:, :, :, :n]
            nxt = int(preds[ok])
            self.last_steps += 1
        return out


def lookup(out, draft, ngram=2, most=24):
    """The draft tokens that follow the first place the draft reads like
    the end of `out`, or nothing."""
    key = out[-ngram:]
    for j in range(len(draft) - len(key) + 1):
        if draft[j:j + len(key)] == key:
            return draft[j + len(key):j + len(key) + most]
    return []



def common_prefix(a, b):
    n = 0
    for x, y in zip(a, b):
        if x != y:
            break
        n += 1
    return n


class Live:
    """A live utterance: exact decodes, and a settled prefix that only grows.

    Every update decodes without forcing (the previous hypothesis is only a
    draft), so a hypothesis is what batch decoding of the same audio gives.
    Text settles once it has stayed the same for `settle_s` of further audio,
    is not among the last `rollback_words` words, and ends a sentence; past
    `max_unsettled_words` it settles at any word. Forcing settled text (as
    Qwen3-ASR does) settles words before the audio after them is heard: the
    small model then loses punctuation and can loop. It also decides
    punctuation and case for the whole utterance late, so text settled
    mid-sentence is often contradicted. Should a decode ever contradict
    settled text, it is redone with that text forced, so settled text is a
    prefix of every later hypothesis.
    """

    def __init__(self, model, rollback_words=3, settle_s=1.0, max_unsettled_words=30):
        self.m = model
        self.stream = model.stream()
        self.rollback_words = rollback_words
        self.settle_s = settle_s
        self.max_unsettled_words = max_unsettled_words
        self.tokens = []
        self.stable = 0
        self.history = []  # (audio seconds, tokens) of earlier hypotheses
        self.conflicts = 0

    @property
    def audio_s(self):
        return self.stream.samples / SAMPLE_RATE

    def update(self, audio=None, final=False):
        """Feed audio (float32) and decode; returns the token hypothesis."""
        s = self.stream
        if audio is not None and len(audio):
            s.feed(audio)
        if final:
            s.finish()
        else:
            s.encode()
        settled = self.tokens[:self.stable]
        tokens = s.decode((), self.tokens)
        if tokens[:self.stable] != settled:
            self.conflicts += 1
            tokens = s.decode(settled, self.tokens[self.stable:])
        self.tokens = tokens
        if not final:
            self._settle()
        return tokens

    def _settle(self):
        now = self.audio_s
        self.history.append((now, self.tokens))
        # The newest hypothesis at least settle_s old, and everything since.
        old = [i for i, (t, _) in enumerate(self.history) if t <= now - self.settle_s]
        if not old:
            return
        del self.history[:old[-1]]
        n = min(common_prefix(h, self.tokens) for _, h in self.history)
        starts = [i for i, t in enumerate(self.tokens[:n + 1]) if self.m.word_start[t]]
        # Cut at a word start, so the settled text is whole words.
        cut = starts[-1] if starts and n < len(self.tokens) else n
        all_starts = [i for i, t in enumerate(self.tokens) if self.m.word_start[t]]
        if self.rollback_words and len(all_starts) >= self.rollback_words:
            cut = min(cut, all_starts[-self.rollback_words])
        elif self.rollback_words:
            cut = 0
        sentences = [i for i in all_starts if self.stable < i <= cut and self.m.sentence_end[self.tokens[i - 1]]]
        unsettled = sum(1 for i in all_starts if i >= self.stable)
        if sentences:
            cut = sentences[-1]
        elif unsettled <= self.max_unsettled_words:
            return
        self.stable = max(self.stable, cut)

    def text(self):
        return self.m.text(self.tokens)

    def stable_text(self):
        return self.m.text(self.tokens[:self.stable])
