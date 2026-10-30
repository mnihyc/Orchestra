#!/usr/bin/env bash
# The requested duration is retained; a shortened run never claims the 24-hour proof.
set -Eeuo pipefail
. "$(dirname "${BASH_SOURCE[0]}")/../lib.sh"
. "$SMOKE_REPO/scripts/smoke/link-load/world.sh"
HERE="$SMOKE_REPO/scripts/smoke/soak"
SCENARIO=soak; DEFAULT_PORT=41420
trap proof_done EXIT
proof_world
BUDGETS="${ORCH_SMOKE_BUDGETS:-$SMOKE_REPO/scripts/smoke/load/budgets.json}"
cp "$BUDGETS" "$EVIDENCE/budgets.json"
code=0
python3 "$HERE/run.py" --world "$WORK/world.json" --out "$EVIDENCE" --budgets "$BUDGETS" \
  --stack "$STACK" --project "$PROJECT" --lab "$LAB" --run "$SMOKE_RUN" \
  --seconds "${ORCH_SMOKE_SOAK_SECONDS:-1800}" > "$EVIDENCE/soak.log" 2>&1 || code=$?
printf '{"phase":"soak","exit":%s}\n' "$code" >> "$EVIDENCE/exits.jsonl"
if [ "$code" = 0 ]; then pass "the declared-duration soak reconciled every accepted input"; else fail "soak (see soak.log and its named fault evidence)"; fi
say "Evidence: $EVIDENCE; only a full observed 86400 seconds can satisfy the 24-hour obligation."
