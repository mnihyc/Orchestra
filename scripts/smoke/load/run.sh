#!/usr/bin/env bash
# Released stack: 100 overlapping fake-adapter turns and thirty independent real browser processes.
set -Eeuo pipefail
. "$(dirname "${BASH_SOURCE[0]}")/../lib.sh"
. "$SMOKE_REPO/scripts/smoke/link-load/world.sh"
HERE="$SMOKE_REPO/scripts/smoke/load"
SCENARIO=load; DEFAULT_PORT=41410
trap proof_done EXIT
proof_world
BUDGETS="${ORCH_SMOKE_BUDGETS:-$HERE/budgets.json}"
cp "$BUDGETS" "$EVIDENCE/budgets.json"
python3 "$HERE/threads.py" --world "$WORK/world.json" --out "$EVIDENCE" --project "$PROJECT" \
  --lab "$LAB" --budgets "$BUDGETS" > "$EVIDENCE/threads.log" 2>&1 &
THREADS_PID=$!
code=0
node "$HERE/browsers.mjs" "$WORK/world.json" "$EVIDENCE" "$BUDGETS" > "$EVIDENCE/browsers.log" 2>&1 || code=$?
printf '{"phase":"browsers","exit":%s}\n' "$code" >> "$EVIDENCE/exits.jsonl"
if [ "$code" = 0 ]; then pass "thirty actual browser processes met the declared budgets"; else fail "browser load (see browsers.log)"; fi
code=0
wait "$THREADS_PID" || code=$?
printf '{"phase":"threads","exit":%s}\n' "$code" >> "$EVIDENCE/exits.jsonl"
if [ "$code" = 0 ]; then pass "100 turns overlapped and completed without lost or duplicate input/output"; else fail "thread load (see threads.log)"; fi
say "Evidence: $EVIDENCE"
