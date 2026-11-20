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

# The two releases are packaged from one tree, so their deployment files are alike. The earlier one is made to hold the
# nginx.conf of v0.1.1 (without the routes the keys need), and the later to be a newer generation of the files, which asks
# for it and brings an updater of its own: what an update does with the files, and what it does not.
A_PACKAGED="$(release_a)"; B_PACKAGED="$(release_b)"
deploy_variant "$A_PACKAGED" a --generation 1 --minimum 1 --strip-openapi
A="$SMOKE_VARIANTS/a"
{ cat "$SMOKE_REPO/deploy/update.sh"; printf '# the updater of the next release\n'; } > "$SMOKE_VARIANTS/next-update.sh"
{ cat "$SMOKE_REPO/deploy/frontend.sh"; printf '# the frontend entry point of the next release\n'; } > "$SMOKE_VARIANTS/next-frontend.sh"
deploy_variant "$B_PACKAGED" b --generation 2 --minimum 2 --replace "frontend.sh=$SMOKE_VARIANTS/next-frontend.sh" --updater "$SMOKE_VARIANTS/next-update.sh" --publish "$A"
B="$SMOKE_VARIANTS/b"
TAG_A="$(release_tag "$A")"; TAG_B="$(release_tag "$B")"
new_stack update 41204 "$A"
trap 'finish' EXIT

step "the release $TAG_A is installed and has an administrator"
update_sh first-start --from "$A" > /dev/null || fail "first-start"
setup_admin || fail "the first administrator was not made"
wait_for 240 "the bundled node is online" node_online
expect "the system reports $TAG_A" "${TAG_A#v}" "$(edge_version)"
SECRETS_BEFORE="$(cd "$STACK/config" && find . -type f | sort | xargs sha256sum | sha256sum)"
CONFIG_BEFORE="$(sha256sum "$STACK/.env" "$STACK/config/orch.toml" | sha256sum)"
COMPOSE_BEFORE="$(sha256sum < "$STACK/compose.yml")"
NGINX_A="$(sha256sum < "$STACK/nginx.conf")"
UPDATER_A="$(sha256sum < "$STACK/update.sh")"
FRONTEND_A="$(link_target frontend current)"
check "the edge of release $TAG_A does not hand the key routes to the API" bash -c "! curl -sS -o /dev/null -w '%{content_type}' '$EDGE/openapi/v1/nothing-here' | grep -q json"

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
expect "the deployment files the failed update wrote are the earlier ones again: nginx.conf" "$NGINX_A" "$(sha256sum < "$STACK/nginx.conf")"
expect "and so is the updater, which is replaced only by an update that finished" "$UPDATER_A" "$(sha256sum < "$STACK/update.sh")"
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
expect ".env and orch.toml are untouched" "$CONFIG_BEFORE" "$(sha256sum "$STACK/.env" "$STACK/config/orch.toml" | sha256sum)"
expect "compose.yml, which the release did not change, was not written" "$COMPOSE_BEFORE" "$(sha256sum < "$STACK/compose.yml")"
check "the installed deployment files match the new release" deployment_files_match "$B/orch-deploy-$TAG_B.tar.gz"
check "nginx.conf is the release's: it hands the key routes to the API" nginx_serves_keys
check "the earlier nginx.conf was saved for a rollback" test -f "$STACK/runtime/update/files/$(sed -n 's/^files_previous=//p' "$STACK/runtime/update/files.state")/old/nginx.conf"
check "the updater is the release's, replaced at the end" cmp -s "$STACK/update.sh" "$SMOKE_VARIANTS/next-update.sh"
check "status says what generation the files are" says 'deployment files: generation 2' update_sh status
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
check "the rollback put the earlier nginx.conf back" test "$(sha256sum < "$STACK/nginx.conf")" = "$NGINX_A"
check "and the updater stayed the newer one: a rollback never rolls it back" cmp -s "$STACK/update.sh" "$SMOKE_VARIANTS/next-update.sh"

step "an nginx.conf edited here keeps the set, and a release that asks for the newer files stops before it stops anything"
printf '# an edit of this installation\n' >> "$STACK/nginx.conf"
EDITED_NGINX="$(sha256sum < "$STACK/nginx.conf")"
STARTED="$(started_at)"
if update_sh "$TAG_B" --from "$B" --yes > "$SMOKE_DIR/update/stopped.txt" 2>&1; then fail "the update to $TAG_B went on over an edited set that is older than it needs"; else pass "the update stopped"; fi
check "it said what the release needs and what is here" grep -q 'works with deployment files of generation 2 or newer, and these are generation 1' "$SMOKE_DIR/update/stopped.txt"
check "it named the file to merge" grep -q 'merge nginx.conf.new' "$SMOKE_DIR/update/stopped.txt"
check "it said that nothing was stopped" grep -q 'Nothing was stopped' "$SMOKE_DIR/update/stopped.txt"
expect "no container was started or stopped" "$STARTED" "$(started_at)"
check "no transaction was opened" test ! -e "$STACK/runtime/update/journal"
expect "the edited nginx.conf is as it was" "$EDITED_NGINX" "$(sha256sum < "$STACK/nginx.conf")"
check "the shipped nginx.conf is beside it, with the key routes" grep -q 'location /openapi/' "$STACK/nginx.conf.new"
check "status lists it" says 'nginx.conf differs from the shipped file' update_sh status
mv "$STACK/nginx.conf.new" "$STACK/nginx.conf"
if update_sh "$TAG_B" --from "$B" --yes > /dev/null; then pass "merged by its owner (the .new file gone), the update goes on"; else fail "the update after the merge"; fi
expect "the system reports $TAG_B" "${TAG_B#v}" "$(edge_version)"
check "nginx.conf is the merged one" nginx_serves_keys
check "status lists nothing kept" bash -c "! (cd '$STACK' && ./update.sh status | grep -q 'differs from the shipped file')"
update_sh rollback --yes > /dev/null || fail "the rollback back to $TAG_A"
expect "the system reports $TAG_A" "${TAG_A#v}" "$(edge_version)"

serve_site update "$((EDGE_PORT + 2))" "$A" "$B"
refuses "a site that holds no such release is refused" bash -c "cd '$STACK' && ORCH_RELEASE_URL='http://127.0.0.1:$((EDGE_PORT + 2))' ./update.sh v9.9.9 --yes"
if (cd "$STACK" && ORCH_RELEASE_URL="http://127.0.0.1:$((EDGE_PORT + 2))" ./update.sh latest --yes); then pass "update.sh latest fetched, checked and installed it"; else fail "update.sh latest fetched, checked and installed it"; fi
expect "the system reports $TAG_B" "${TAG_B#v}" "$(edge_version)"
check "every service reports $TAG_B" ready "$TAG_B"
