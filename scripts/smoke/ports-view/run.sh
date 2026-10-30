#!/usr/bin/env bash
# Released frontend and API: pure target reads, stable address and environment capability grants.
set -Eeuo pipefail
. "$(dirname "${BASH_SOURCE[0]}")/../lib.sh"
. "$SMOKE_REPO/scripts/smoke/link-load/world.sh"
SCENARIO=ports-view; DEFAULT_PORT=41460
trap proof_done EXIT
proof_world
code=0
node "$SMOKE_REPO/scripts/smoke/ports-view/browser.mjs" "$WORK/world.json" "$EVIDENCE" > "$EVIDENCE/browser.log" 2>&1 || code=$?
printf '{"phase":"ports-browser","exit":%s}\n' "$code" >> "$EVIDENCE/exits.jsonl"
if [ "$code" = 0 ]; then pass "the released Ports panel reads and changes the actual producer"; else fail "Ports browser walk (see browser.log)"; fi
say "Evidence: $EVIDENCE"
