# moonshine-stream

CPU-only live ASR for ears on machines without a GPU. It serves
[Moonshine Streaming](https://huggingface.co/moonshine-ai/moonshine-streaming)
behind the API ears already speaks: `/v1/audio/transcriptions`, the
`/v1/chat/completions` speech check, and the ears stream protocol
(`docs/STREAM_PROTOCOL.md`) at `/v1/ears/stream`. So `live_decoding =
"continuous"` streams to it without changes to ears.

## Why the streaming model

Moonshine base encodes the whole clip with full attention, so every live
update has to encode all the audio again. Moonshine Streaming was built for
live use:

- The frontend is causal and stateful, so audio goes in once.
- The encoder uses sliding-window attention: 16 frames (320 ms) of left
  context per layer and 16 frames of total lookahead. A frame is final once
  16 newer frames exist, so each update encodes only its new frames.
- The adapter and the cross-attention K/V are per frame, so memory only
  grows by appending.

`engine.py` runs the official split ONNX export (frontend, encoder, adapter,
cross_kv, decoder_kv) the same way the upstream C++ runtime does.

## Decoding

Each update decodes **exactly**. Nothing is forced. The previous hypothesis
is a speculative draft, checked in one batched pass. After the first token
that differs, the rest of the draft is proposed again from where the new
text rejoins it ("prompt lookup"), so a changed word or comma doesn't mean
regenerating the rest token by token.

Forcing settled text the way the Qwen3-ASR plugin does hurt this model badly:
it looped ("…in the universe are also its most advanced civilizations" ×8)
and lost all punctuation. The model decides punctuation and case for the
whole utterance late. Text therefore settles (`stable_chars`) only when all
of the following hold:

- it has stayed the same for 1 s of further audio,
- it is not among the last `rollback_words` words,
- it ends at a sentence end. Past 30 unsettled words, it settles at any word.

If a later decode contradicts settled text, that decode is redone with the
settled text forced. Settled text is therefore always a prefix of every later
partial and of the final, as the protocol promises.

## Numbers (tiny, 10 narration clips of about 22 s)

- Accuracy: 8 of 10 live finals equal batch decoding. One differs only in
  punctuation. One differs in one word, and there live is the correct one
  ("intelligent", where batch says "intelligence").
- Centurion: each 300 ms update takes about 20 ms; the final takes about
  25 ms after `end`.
- Hackbook (i5 M560, 2010, no AVX, 2 threads): each update takes 250 to
  330 ms. When streamed in real time, partials trail the audio by about
  240 ms, and the final arrives about 450 ms after `end` for a 23 s passage.
  Batch decoding runs at 0.15× real time (RTF 0.15).

The `.ort` weights are int8 with dynamic activation quantisation. Each
activation scale depends on the whole input tensor, so incremental and batch
encoder outputs differ by quantisation noise, just as they do upstream.

## Run

```sh
uv venv -p 3.14 ~/.venvs/moonshine-stream
uv pip install -p ~/.venvs/moonshine-stream/bin/python onnxruntime numpy tokenizers aiohttp websockets
# model: onnx/tiny/{frontend,encoder,adapter,cross_kv,decoder_kv}.ort,
#        streaming_config.json, tokenizer.json from moonshine-ai/moonshine-streaming
#        (revision da30bae714913eb0057e733bccb4700104afb1b0)
python server.py MODEL_DIR --port 30190 --threads 2
python bench.py MODEL_DIR clip.wav ...              # batch vs live, timings
python stream_test.py ws://127.0.0.1:30190/v1/ears/stream clip.wav
```

Point ears at it with `server = "http://127.0.0.1:30190/"` and
`live_decoding = "continuous"`.

## Limits

- English only. `language` and `context` are accepted and ignored.
- A grammar (bash mode) is ignored, and the chat endpoint returns plain text.
- At most 80 s per utterance, because the adapter has 4096 learned
  positions of 20 ms each.
