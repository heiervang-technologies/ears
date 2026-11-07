#!/bin/bash
# (Re)start the ASR server of one GPU slot with its experiment's flags.
#   rig/serve.sh SLOT            reads /workspace/slots/SLOT/serve.env
# serve.env may set VLLM_EXTRA (extra `vllm serve` flags) and ENV_EXTRA
# (space-separated VAR=value). Baseline flags = production pod.
# Port 8000+SLOT. Waits until /v1/ears/stream answers, max 10 min.
set -uo pipefail
SLOT=$1; S=/workspace/slots/$SLOT; PORT=$((8000 + SLOT))
VLLM_EXTRA=""; ENV_EXTRA=""
[ -f $S/serve.env ] && . $S/serve.env
pkill -f "vllm serve.*--port $PORT" 2>/dev/null; sleep 3
cd $S
env CUDA_VISIBLE_DEVICES=$SLOT VLLM_PLUGINS=ears_stream HF_HUB_OFFLINE=1 $ENV_EXTRA \
  $S/venv/bin/vllm serve Qwen/Qwen3-ASR-1.7B \
    --trust-request-chat-template --dtype bfloat16 \
    --gpu-memory-utilization 0.30 --max-model-len 2048 \
    --host 0.0.0.0 --port $PORT $VLLM_EXTRA > $S/server.log 2>&1 &
for i in $(seq 120); do
  sleep 5
  if python3 - "$PORT" <<'PY' 2>/dev/null; then echo "slot $SLOT ready on $PORT"; exit 0; fi
import asyncio, json, sys, websockets
async def m():
    async with websockets.connect(f"ws://127.0.0.1:{sys.argv[1]}/v1/ears/stream", open_timeout=3) as ws:
        assert json.loads(await ws.recv())["type"] == "ready"
asyncio.run(m())
PY
  kill -0 $! 2>/dev/null || { echo "slot $SLOT: server died"; tail -20 $S/server.log; exit 1; }
done
echo "slot $SLOT: not ready after 10 min"; tail -20 $S/server.log; exit 1
