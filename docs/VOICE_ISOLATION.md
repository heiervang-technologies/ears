# Voice isolation

Silero is the weak link in a noisy room, not the ASR model: Qwen3-ASR
transcribed speech at -32 dB under pink room noise word for word, while the
detector never opened a segment for it. Removing the noise before detection
fixes that, and costs nothing in transcription accuracy.

## Setup (PipeWire + RNNoise)

[`noise-suppression-for-voice`](https://github.com/werman/noise-suppression-for-voice)
is in Arch's `extra` repository. Its RNNoise plugin runs as a PipeWire filter
that publishes a denoised copy of your microphone as a new source:

```bash
sudo pacman -S noise-suppression-for-voice
mkdir -p ~/.config/pipewire/filter-chain.conf.d
cp contrib/pipewire/ears-rnnoise.conf ~/.config/pipewire/filter-chain.conf.d/
# edit target.object to your microphone (wpctl status / ears list)
systemctl --user enable --now filter-chain.service
```

Then select `ears_denoised_mic` with `ears select` or set
`device = "ears_denoised_mic"`. The default source does not change; other apps
keep the raw microphone.

The plugin's own voice gate is off (`VAD Threshold (%) = 0`): it removes
noise but does not decide what is speech, so it cannot cut word onsets.

## Measurements

Synthesized speech (three sentences, 15 s), 16 kHz, the `vad_calibrate`
example (`cargo run --example vad_calibrate -- FILE.wav...`). Segments found
out of 3; `p` is the mean speech probability over the voiced span.

| Input | Before (0.5 fixed) | Hysteresis | Hysteresis + gain |
|---|---|---|---|
| Speech, 0 dB | 3 (p 0.89-0.98) | 3 | 3 |
| Speech, -26 dB | 3, last onset 400 ms late | 3 | 3, onsets on time |
| Speech, -32 dB | 0 | 2 fragments (p 0.44, 0.53) | 3 (p 0.76-0.88) |
| -26 dB speech + room noise | 3, last onset 860 ms late | 3 | 3, onsets on time |
| -32 dB speech + room noise | 0 | 0 | 0 |
| ... denoised (RNNoise) | 0 | 0 | **3** (p 0.81-0.84) |
| -18 dB speech + loud noise | 3: sentence 2 split, sentence 3 missed | 4, one p 0.39 | 3 (p 0.50-0.61) |
| ... denoised (RNNoise) | 3 (p 0.86-0.93) | 3 | 3 (p 0.89-0.95) |
| Room noise, clicks, noise bursts | 0 | 0 | 0 |

Every transcription of the speech files, raw or denoised, came out identical
apart from punctuation.

No noise-only input opened a segment in any configuration, and real speech
under loud noise scored as low as 0.50 (0.39 for a fragment). That is why
`min_mean_probability` defaults to a conservative 0.4: it is a backstop for
coughs and clicks that pass the gate, not a noise filter. Denoising is.

## Short words and "Okay." on silence

Quick one-word answers (Kokoro TTS at 1.8x speed, -30 dB, 0.37-0.52 s) with
`vad_calibrate` (`MIN_SPEECH_MS` sets the minimum):

| min_speech_duration_ms | Before (0.5 fixed) | Hysteresis + gain |
|---|---|---|
| 300 | 2 of 5 (No, Stop, Sure missed) | 4 of 5 (Stop missed) |
| 200 | 4 of 5 (Stop missed) | 5 of 5 |
| 128 | 5 of 5 | 5 of 5 |

No breath, knock, click or room-noise file opened a segment at any of these.

"Okay." on silence comes from the forced language, not the detector. Through
`/v1/audio/transcriptions` with `language=en`, Qwen3-ASR returned "Okay." for
1 s of silence and of room noise, "Oh." for a breath, a knock and clicks, and
"I'm not sure." for denoised room noise. The same audio without a forced
language came back as `language None` every time, while "Yes", "No", "Okay",
"Stop" and "Sure" were still recognized as English. ears therefore checks
results of at most three words that way and drops them on `language None`.

## Not covered: other voices

RNNoise keeps speech, any speech: a TV, a call or a colleague still opens
segments. Keeping only *your* voice needs speaker verification (compare each
segment with an enrolled voiceprint and drop mismatches); that is tracked as a
separate experiment.

## Short words from across the room

Measured with `examples/vad_calibrate.rs` (`THRESHOLD`, `MIN_SPEECH_MS`)
on six reverberant "Over"s (148-332 ms, three tempos, two voices) in
noise, hysteresis plus gain, out of 6:

| word RMS | 0.5 / 200 ms | 0.5 / 160 ms | 0.4 / 160 ms | 0.3 / 160 ms |
|---|---|---|---|---|
| 0.004 | 0 | 2 | 4 | 5 |
| 0.008 | 4 | 5 | 6 | 6 |

None of the noise-only clips (knock, breath, burst, clicks, babble, room,
silence) produced a segment at any setting. Segments at 0.4 averaged
0.40-0.48 speech probability, so lower `min_mean_probability` to about
0.3 together with the threshold. Settings for picking up single-word
commands at a distance:

```toml
[vad]
speech_threshold = 0.4
min_speech_duration_ms = 160
min_mean_probability = 0.3
```

`examples/vad_drift.rs` checks whether Silero's recurrent state dulls
over a long session: the same quiet word every 20 s for 10 minutes peaked
at 0.88-0.95 throughout with the state carried over (0.98 with a fresh
state), so no periodic reset is needed.
