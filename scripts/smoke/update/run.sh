#!/usr/bin/env bash
# update: an installed release advances to a newer one in a transaction that
# stops the services, keeps a dump of the database, switches the payloads, starts,
# and verifies that the stack is ready and reports the new release. A release
# that does not become ready is taken back: the previous payloads and the
# database as the dump holds it. What is configured, and what people entered,
# is kept either way.
set -Eeuo pipefail
# shellcheck source-path=SCRIPTDIR/..
# shellcheck source=lib.sh
. "$(dirname "${BASH_SOURCE[0]}")/../lib.sh"

A="$(release_a)"; B="$(release_b)"
TAG_A="$(release_tag "$A")"; TAG_B="$(release_tag "$B")"
new_stack update 41204 "$A"
trap 'finish' EXIT

step "the release $TAG_A is installed and has an administrator"
update_sh first-start --from "$A" > /dev/null || fail "first-start"
setup_admin || fail "the first administrator was not made"
wait_for 240 "the bundled node is online" node_online
expect "the system reports $TAG_A" "${TAG_A#v}" "$(edge_version)"
SECRETS_BEFORE="$(cd "$STACK/config" && find . -type f | sort | xargs sha256sum | sha256sum)"
CONFIG_BEFORE="$(sha256sum "$STACK/compose.yml" "$STACK/nginx.conf" "$STACK/.env" "$STACK/config/orch.toml" | sha256sum)"
FRONTEND_A="$(link_target frontend current)"

step "a release whose schema cannot be applied is taken back"
BAD="$SMOKE_DIR/update/broken"
broken_release "$B" "$BAD"
DUMPS_BEFORE="$(dumps | wc -l)"
if ORCH_READY_TIMEOUT=75 update_sh "$TAG_B" --from "$BAD" --yes; then fail "the update to the broken release reported success"; else pass "the update to the broken release failed"; fi
expect "the previous release is running again" "${TAG_A#v}" "$(edge_version)"
check "the stack is ready as release $TAG_A" ready "$TAG_A"
expect "the payload of the server is the previous one" "$TAG_A" "$(link_target server current | sed 's/-[0-9a-f]*$//')"
expect "the frontend is the previous one" "$FRONTEND_A" "$(link_target frontend current)"
check "no previous release is recorded: nothing was installed" test ! -e "$STACK/runtime/server/previous"
check "a dump of the database before the update is kept" test "$(dumps | wc -l)" -gt "$DUMPS_BEFORE"
check "the users' table is back, as the dump had it" test "$(sql "SELECT count(*) FROM users")" = 1
check "the administrator can still sign in" sign_in
check "no transaction is left open" test ! -e "$STACK/runtime/update/journal"
expect "the installed release is still $TAG_A" "$TAG_A" "$(sed -n 's/^current_tag=//p' "$STACK/runtime/update/state")"
rm -rf "$BAD"

step "the update to $TAG_B"
DUMPS_BEFORE="$(dumps | wc -l)"
# The interactive question is not asked when nothing is running; --yes only matters to scripts.
if update_sh "$TAG_B" --from "$B" --yes; then pass "the update finished"; else fail "the update finished"; fi
expect "the system reports $TAG_B" "${TAG_B#v}" "$(edge_version)"
check "every service reports $TAG_B" ready "$TAG_B"
check "the administrator signs in with the same password" sign_in
check "the new frontend is served" bash -c "curl -fsS '$EDGE/' | grep -qi '<html'"
check "the new release is current and the old one previous" \
  bash -c "[ \"\$(readlink '$STACK/runtime/server/current')\" != \"\$(readlink '$STACK/runtime/server/previous')\" ] && readlink '$STACK/runtime/server/previous' | grep -q '^releases/$TAG_A-'"
check "a dump of the database as release $TAG_A left it was kept" test "$(dumps | wc -l)" -gt "$DUMPS_BEFORE"
LATEST_DUMP="$(dumps | tail -n 1)"
check "that dump reads back" bash -c "cd '$STACK' && docker compose exec -T postgres pg_restore --list < '$LATEST_DUMP' | grep -q users"
expect "configuration and secrets are untouched" "$SECRETS_BEFORE" "$(cd "$STACK/config" && find . -type f | sort | xargs sha256sum | sha256sum)"
expect "compose.yml, nginx.conf, .env and orch.toml are untouched" "$CONFIG_BEFORE" "$(sha256sum "$STACK/compose.yml" "$STACK/nginx.conf" "$STACK/.env" "$STACK/config/orch.toml" | sha256sum)"
expect "the record names the installed and the previous release" "$TAG_B $TAG_A" \
  "$(sed -n 's/^current_tag=//p' "$STACK/runtime/update/state") $(sed -n 's/^previous_tag=//p' "$STACK/runtime/update/state")"
wait_for 180 "the node is online on the new release" bash -c "cd '$STACK' && docker compose exec -T api /opt/orchctl/orchctl doctor --only node | grep -q '^ok'"

step "an update to what is installed already does nothing, and a backup stops nothing"
STARTED="$(started_at)"
check "it says that it is installed already" says 'installed already' update_sh "$TAG_B" --from "$B" --yes
if update_sh backup > /dev/null; then pass "a backup was taken"; else fail "a backup was taken"; fi
expect "no container was started again by either" "$STARTED" "$(started_at)"

step "the same update, fetched from a release site instead of a directory"
# Back to $TAG_A, and then to $TAG_B again as `latest`, from a web server that lays its releases out as a site does.
update_sh rollback --yes > /dev/null || fail "the rollback that makes room for it"
expect "the system reports $TAG_A" "${TAG_A#v}" "$(edge_version)"
serve_site update "$((EDGE_PORT + 2))" "$A" "$B"
refuses "a site that holds no such release is refused" bash -c "cd '$STACK' && ORCH_RELEASE_URL='http://127.0.0.1:$((EDGE_PORT + 2))' ./update.sh v9.9.9 --yes"
if (cd "$STACK" && ORCH_RELEASE_URL="http://127.0.0.1:$((EDGE_PORT + 2))" ./update.sh latest --yes); then pass "update.sh latest fetched, checked and installed it"; else fail "update.sh latest fetched, checked and installed it"; fi
expect "the system reports $TAG_B" "${TAG_B#v}" "$(edge_version)"
check "every service reports $TAG_B" ready "$TAG_B"
