#!/usr/bin/env bash
# rollback: after an update, the previous release and the database as it was when
# that release was left come back together; what was written in between is lost
# on purpose and said so; a dump of the database as it stood is kept first, so
# that a second rollback goes forward again to where the first came from.
set -Eeuo pipefail
# shellcheck source-path=SCRIPTDIR/..
# shellcheck source=lib.sh
. "$(dirname "${BASH_SOURCE[0]}")/../lib.sh"

A="$(release_a)"; B="$(release_b)"
TAG_A="$(release_tag "$A")"; TAG_B="$(release_tag "$B")"
new_stack rollback 41208 "$A"
trap 'finish' EXIT

step "release $TAG_A, then $TAG_B, with the administrator made under $TAG_A"
update_sh first-start --from "$A" > /dev/null || fail "first-start"
setup_admin || fail "the first administrator was not made"
wait_for 240 "the bundled node is online" node_online
update_sh "$TAG_B" --from "$B" --yes > /dev/null || fail "the update to $TAG_B"
expect "the system reports $TAG_B" "${TAG_B#v}" "$(edge_version)"

step "something is written under $TAG_B"
sql "INSERT INTO users (id, username, display_name, avatar_color) VALUES (gen_random_uuid(), 'bob', 'Bob', 3)" > /dev/null
expect "there are two people" 2 "$(sql "SELECT count(*) FROM users")"

step "a rollback needs an answer and does not guess one"
refuses "it asks for --yes when nobody can be asked" bash -c "cd '$STACK' && ./update.sh rollback < /dev/null"
expect "so the system still reports $TAG_B" "${TAG_B#v}" "$(edge_version)"
expect "and nobody was lost" 2 "$(sql "SELECT count(*) FROM users")"

step "rollback to $TAG_A"
if update_sh rollback --yes > "$SMOKE_DIR/rollback/rollback.txt" 2>&1; then pass "the rollback finished"; else fail "the rollback finished"; cat "$SMOKE_DIR/rollback/rollback.txt" >&2; fi
check "it said what is lost" grep -q 'Everything written since then is lost' "$SMOKE_DIR/rollback/rollback.txt"
expect "the system reports $TAG_A" "${TAG_A#v}" "$(edge_version)"
check "every service reports $TAG_A" ready "$TAG_A"
expect "the database is as it was when $TAG_B was installed: one person" 1 "$(sql "SELECT count(*) FROM users")"
check "the administrator signs in" sign_in
expect "the record names $TAG_A as installed and $TAG_B as the previous" "$TAG_A $TAG_B" \
  "$(sed -n 's/^current_tag=//p' "$STACK/runtime/update/state") $(sed -n 's/^previous_tag=//p' "$STACK/runtime/update/state")"
check "no transaction is left open" test ! -e "$STACK/runtime/update/journal"

step "rollback again goes forward to $TAG_B, with what was there when it was left"
update_sh rollback --yes > /dev/null || fail "the second rollback"
expect "the system reports $TAG_B" "${TAG_B#v}" "$(edge_version)"
expect "both people are back" 2 "$(sql "SELECT count(*) FROM users")"
check "every service reports $TAG_B" ready "$TAG_B"
