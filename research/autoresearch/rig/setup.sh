#!/bin/bash
# One-time setup on the rented rig (image vllm/vllm-openai:v0.28.0, same as
# the production qwen3-asr pod). Run as root from /workspace/ears checkout.
#   bash research/autoresearch/rig/setup.sh [N_SLOTS]
# Makes one venv per GPU slot (system site packages + editable plugin from
# /workspace/slots/<n>/ears), so each slot can run its own plugin edits.
set -euo pipefail
N=${1:-$(nvidia-smi -L | wc -l)}
R=/workspace
apt-get update -qq && apt-get install -y -qq ffmpeg git >/dev/null
pip install --no-cache-dir -q librosa soundfile av websockets
huggingface-cli download Qwen/Qwen3-ASR-1.7B >/dev/null
for i in $(seq 0 $((N-1))); do
  S=$R/slots/$i
  mkdir -p $S
  [ -d $S/ears ] || git clone -q $R/ears $S/ears
  [ -d $S/venv ] || python3 -m venv --system-site-packages $S/venv
  $S/venv/bin/pip install -q --no-deps -e $S/ears/vllm-plugin
done
python3 $R/ears/research/autoresearch/data/fetch_librispeech.py $R/data
echo "setup done: $N slots"
