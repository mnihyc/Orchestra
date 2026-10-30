#!/usr/bin/env bash
# Readiness follows the actual web edge; broken or absent candidates restore
# both the previous release and the database, while a good candidate completes.
set -Eeuo pipefail
# shellcheck source=../lib.sh
. "$(dirname "${BASH_SOURCE[0]}")/../lib.sh"
HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd -P)"
A="$(release_a)"
B="$(release_b)"
TAG_A="$(release_tag "$A")"
TAG_B="$(release_tag "$B")"
new_stack kfix-deploy-edge 41062 "$A"
UPDATE_PID=""
leave() { if [ -n "$UPDATE_PID" ]; then kill "$UPDATE_PID" 2> /dev/null || true; wait "$UPDATE_PID" 2> /dev/null || true; fi; finish; }
trap leave EXIT
install -m 755 "$SMOKE_REPO/deploy/update.sh" "$STACK/update.sh"
cat > "$STACK/compose.override.yml" <<'YAML'
services:
  frontend:
    command: ["sh", "-c", "if test -f /usr/share/nginx/html/kfix-deploy-disable-edge; then exit 0; fi; exec nginx -g 'daemon off;'"]
YAML

step "a healthy release passes through the operator URL"
update_sh first-start --from "$A" > "$SMOKE_DIR/kfix-deploy-edge/first-start.log" 2>&1
check "the application page is served" curl -fsS "$EDGE/"
expect "the public API serves the installed release" "${TAG_A#v}" "$(edge_version)"
for id in edge-page edge-api edge-link; do
  check "readiness tried $id through the real edge" grep -q "^ok    $id " "$SMOKE_DIR/kfix-deploy-edge/first-start.log"
done
setup_admin
wait_for 240 "the bundled node is online before the dump" node_online
BEFORE="$(link_target server current)"

for variant in broken missing; do
  step "a $variant candidate web edge is refused and rolled back"
  BAD="$SMOKE_DIR/kfix-deploy-edge/$variant-release"
  python3 -I -B "$HERE/prepare.py" "$B" "$BAD" "$variant" "$STACK/runtime/cli/current/orchctl"
  ORCH_READY_TIMEOUT=45 update_sh "$TAG_B" --from "$BAD" --yes > "$SMOKE_DIR/kfix-deploy-edge/$variant.log" 2>&1 &
  UPDATE_PID=$!
  # Record a candidate-only database write after it starts. Rolling back just
  # the payload links would leave this table and fail the restore assertion.
  DEADLINE=$((SECONDS + 180))
  while ! grep -q '^step=verifying$' "$STACK/runtime/update/journal" 2> /dev/null; do
    kill -0 "$UPDATE_PID" 2> /dev/null || { fail "the candidate did not reach readiness verification"; break; }
    [ "$SECONDS" -lt "$DEADLINE" ] || { fail "the candidate never reached readiness verification"; break; }
    sleep 1
  done
  if kill -0 "$UPDATE_PID" 2> /dev/null; then
    sql 'CREATE TABLE kfix_deploy_edge_candidate (value integer)' > /dev/null
    pass "the candidate wrote to its running database"
  fi
  if wait "$UPDATE_PID"; then fail "the $variant candidate failed readiness"; else pass "the $variant candidate failed readiness"; fi
  UPDATE_PID=""
  check "the updater reports returning the previous release" grep -q 'running again as it was' "$SMOKE_DIR/kfix-deploy-edge/$variant.log"
  expect "the payload links returned to the prior release" "$BEFORE" "$(link_target server current)"
  expect "the updater still records the prior release" "$TAG_A" "$(sed -n 's/^current_tag=//p' "$STACK/runtime/update/state")"
  expect "the candidate-only database write was undone" "" "$(sql "SELECT to_regclass('public.kfix_deploy_edge_candidate')")"
  check "the prior administrator signs in again" sign_in
  expect "the prior API release is served through the operator URL" "${TAG_A#v}" "$(edge_version)"
  check "the prior application page is served" curl -fsS "$EDGE/"
  check "the completed undo leaves no open journal" test ! -e "$STACK/runtime/update/journal"
done

step "a good edge/API/gateway completes an update"
update_sh "$TAG_B" --from "$B" --yes > "$SMOKE_DIR/kfix-deploy-edge/good-update.log" 2>&1
expect "the public API serves the good candidate" "${TAG_B#v}" "$(edge_version)"
for id in edge-page edge-api edge-link; do
  check "the good update passes $id" grep -q "^ok    $id " "$SMOKE_DIR/kfix-deploy-edge/good-update.log"
done
