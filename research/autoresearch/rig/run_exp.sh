#!/bin/bash
# Run one experiment on one slot: restart its server, evaluate, log a row.
#   rig/run_exp.sh SLOT EXP_NAME "description"
# Appends to /workspace/results/results.tsv:
#   exp slot commit final_rt_p50 final_rt_p90 decode_p50 wer ls-clean ls-other cmd code secs description
set -uo pipefail
SLOT=$1; EXP=$2; DESC=${3:-}; S=/workspace/slots/$SLOT; OUT=/workspace/results
mkdir -p $OUT
COMMIT=$(git -C $S/ears rev-parse --short HEAD)$(git -C $S/ears diff --quiet || echo +dirty)
if ! bash /workspace/ears/research/autoresearch/rig/serve.sh $SLOT > $OUT/$EXP.serve.log 2>&1; then
  printf '%s\t%s\t%s\tcrash\t\t\t\t\t\t\t\t\t%s\n' "$EXP" "$SLOT" "$COMMIT" "$DESC" >> $OUT/results.tsv
  echo "crash: see $OUT/$EXP.serve.log"; exit 1
fi
LINE=$(python3 /workspace/ears/research/autoresearch/eval/eval.py --url http://127.0.0.1:$((8000+SLOT)) \
  --manifest /workspace/data/manifest.jsonl --out $OUT/$EXP.json 2>$OUT/$EXP.eval.err) || { echo "eval failed"; tail $OUT/$EXP.eval.err; exit 1; }
echo "$LINE"
python3 - "$OUT/$EXP.json" "$EXP" "$SLOT" "$COMMIT" "$DESC" >> $OUT/results.tsv <<'PY'
import json, sys
r = json.load(open(sys.argv[1])); l = r.get("latency", {}); w = r.get("wer", {}); s = w.get("sets", {})
g = lambda k: s.get(k, {}).get("wer", "")
print("\t".join(map(str, [sys.argv[2], sys.argv[3], sys.argv[4], l.get("final_rt_p50"), l.get("final_rt_p90"),
      l.get("decode_p50"), w.get("pooled"), g("ls-clean"), g("ls-other"), g("cmd"), g("code"), r.get("seconds"), sys.argv[5]])))
PY
