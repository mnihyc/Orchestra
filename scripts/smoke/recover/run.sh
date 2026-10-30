#!/usr/bin/env bash
# recover: a transaction that was cut off, as by a power cut, is finished or undone
# and never left ambiguous. The updater is stopped dead (as by a power cut) right
# after it wrote a step in its journal, at four steps: before anything moved; with
# the new release running but not yet judged; and, in a rollback, with the
# database about to be replaced.
set -Eeuo pipefail
# shellcheck source-path=SCRIPTDIR/..
# shellcheck source=lib.sh
. "$(dirname "${BASH_SOURCE[0]}")/../lib.sh"

A="$(release_a)"; B="$(release_b)"
TAG_A="$(release_tag "$A")"; TAG_B="$(release_tag "$B")"
new_stack recover 41212 "$A"
trap 'finish' EXIT

# crash_at STEP ARGS...: update.sh, stopped dead right after it recorded STEP.
crash_at() {
  local step="$1"; shift
  if (cd "$STACK" && ORCH_UPDATE_CRASH_AT="$step" ./update.sh "$@" > /dev/null 2>&1); then return 1; fi
  [ -f "$STACK/runtime/update/journal" ] && grep -qx "step=$step" "$STACK/runtime/update/journal"
}

step "release $TAG_A is running with an administrator"
update_sh first-start --from "$A" > /dev/null || fail "first-start"
setup_admin || fail "the first administrator was not made"
wait_for 240 "the bundled node is online" node_online

step "cut off before any link moved: the update is undone"
check "the updater was stopped right after it wrote 'activating'" crash_at activating "$TAG_B" --from "$B" --yes
refuses "another update is refused until this is dealt with" update_sh "$TAG_B" --from "$B" --yes
check "status says that a transaction was cut off" says interrupted update_sh status
if update_sh recover; then pass "recover finished"; else fail "recover finished"; fi
expect "release $TAG_A is the one running" "${TAG_A#v}" "$(edge_version)"
check "every service reports $TAG_A" ready "$TAG_A"
check "no previous release was recorded" test ! -e "$STACK/runtime/server/previous"
check "no transaction is left open" test ! -e "$STACK/runtime/update/journal"
check "the administrator signs in" sign_in

step "cut off with the new release started and not yet judged: the update is finished"
check "the updater was stopped right after it wrote 'verifying'" crash_at verifying "$TAG_B" --from "$B" --yes
if update_sh recover; then pass "recover finished"; else fail "recover finished"; fi
expect "release $TAG_B is the one running" "${TAG_B#v}" "$(edge_version)"
check "every service reports $TAG_B" ready "$TAG_B"
expect "the record names $TAG_B as installed and $TAG_A as the previous" "$TAG_B $TAG_A" \
  "$(sed -n 's/^current_tag=//p' "$STACK/runtime/update/state") $(sed -n 's/^previous_tag=//p' "$STACK/runtime/update/state")"
check "no transaction is left open" test ! -e "$STACK/runtime/update/journal"
check "the administrator signs in" sign_in

step "a rollback cut off with the database about to be replaced: it is undone, and the database is as it was"
sql "INSERT INTO users (id, username, display_name, avatar_color) VALUES (gen_random_uuid(), 'bob', 'Bob', 3)" > /dev/null
check "the updater was stopped right after it wrote 'restoring_entered'" crash_at restoring_entered rollback --yes
if update_sh recover; then pass "recover finished"; else fail "recover finished"; fi
expect "release $TAG_B is the one running" "${TAG_B#v}" "$(edge_version)"
expect "both people are there" 2 "$(sql "SELECT count(*) FROM users")"
check "every service reports $TAG_B" ready "$TAG_B"
check "no transaction is left open" test ! -e "$STACK/runtime/update/journal"

step "recover with nothing open says so and changes nothing"
STARTED="$(started_at)"
check "it says that there is nothing to recover" says 'nothing to recover' update_sh recover
expect "no container was started again" "$STARTED" "$(started_at)"
