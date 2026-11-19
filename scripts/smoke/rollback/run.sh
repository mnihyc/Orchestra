#!/usr/bin/env bash
# rollback: after an update, the previous release and the database as it was when
# that release was left come back together; what was written in between is lost
# on purpose and said so; a dump of the database as it stood is kept first, so
# that a second rollback goes forward again to where the first came from.
set -Eeuo pipefail
# shellcheck source-path=SCRIPTDIR/..
# shellcheck source=lib.sh
. "$(dirname "${BASH_SOURCE[0]}")/../lib.sh"

# The earlier release holds the nginx.conf of v0.1.1 (no key routes), the later is a newer generation of the deployment files
# with an updater of its own (see the update scenario).
deploy_variant "$(release_a)" a --generation 1 --minimum 1 --strip-openapi
A="$SMOKE_VARIANTS/a"
{ cat "$SMOKE_REPO/deploy/update.sh"; printf '# the updater of the next release\n'; } > "$SMOKE_VARIANTS/next-update.sh"
deploy_variant "$(release_b)" b --generation 2 --minimum 2 --updater "$SMOKE_VARIANTS/next-update.sh" --publish "$A"
B="$SMOKE_VARIANTS/b"
TAG_A="$(release_tag "$A")"; TAG_B="$(release_tag "$B")"
new_stack rollback 41208 "$A"
trap 'finish' EXIT

step "release $TAG_A, then $TAG_B, with the administrator made under $TAG_A"
update_sh first-start --from "$A" > /dev/null || fail "first-start"
setup_admin || fail "the first administrator was not made"
wait_for 240 "the bundled node is online" node_online
README_A="$(sha256sum < "$STACK/README.md")"
update_sh "$TAG_B" --from "$B" --yes > /dev/null || fail "the update to $TAG_B"
expect "the system reports $TAG_B" "${TAG_B#v}" "$(edge_version)"
check "the update brought the release's nginx.conf and its updater" bash -c "cd '$STACK' && grep -q 'location /openapi/' nginx.conf && cmp -s update.sh '$SMOKE_VARIANTS/next-update.sh'"

step "something is written under $TAG_B"
sql "INSERT INTO users (id, username, display_name, avatar_color) VALUES (gen_random_uuid(), 'bob', 'Bob', 3)" > /dev/null
expect "there are two people" 2 "$(sql "SELECT count(*) FROM users")"

step "a rollback needs an answer and does not guess one"
refuses "it asks for --yes when nobody can be asked" bash -c "cd '$STACK' && ./update.sh rollback < /dev/null"
expect "so the system still reports $TAG_B" "${TAG_B#v}" "$(edge_version)"
expect "and nobody was lost" 2 "$(sql "SELECT count(*) FROM users")"

step "a file edited here since the update stays when the update is rolled back, and the command says so"
printf '# an edit made after the update\n' >> "$STACK/nginx.conf"
EDITED_NGINX="$(sha256sum < "$STACK/nginx.conf")"

step "rollback to $TAG_A"
if update_sh rollback --yes > "$SMOKE_DIR/rollback/rollback.txt" 2>&1; then pass "the rollback finished"; else fail "the rollback finished"; cat "$SMOKE_DIR/rollback/rollback.txt" >&2; fi
check "it said what is lost" grep -q 'Everything written since then is lost' "$SMOKE_DIR/rollback/rollback.txt"
check "it said that the edited nginx.conf stays" grep -q 'nginx.conf stays as it is: it is no longer what the last update wrote' "$SMOKE_DIR/rollback/rollback.txt"
expect "the edited nginx.conf is as it was left" "$EDITED_NGINX" "$(sha256sum < "$STACK/nginx.conf")"
expect "the files the update replaced and nobody edited are the earlier release's: README.md" "$README_A" "$(sha256sum < "$STACK/README.md")"
check "the updater stays the newer one, and performed the rollback" cmp -s "$STACK/update.sh" "$SMOKE_VARIANTS/next-update.sh"
expect "the system reports $TAG_A" "${TAG_A#v}" "$(edge_version)"
check "every service reports $TAG_A" ready "$TAG_A"
expect "the database is as it was when $TAG_B was installed: one person" 1 "$(sql "SELECT count(*) FROM users")"
check "the administrator signs in" sign_in
expect "the record names $TAG_A as installed and $TAG_B as the previous" "$TAG_A $TAG_B" \
  "$(sed -n 's/^current_tag=//p' "$STACK/runtime/update/state") $(sed -n 's/^previous_tag=//p' "$STACK/runtime/update/state")"
check "no transaction is left open" test ! -e "$STACK/runtime/update/journal"

step "rollback again goes forward to $TAG_B, with what was there when it was left"
update_sh rollback --yes > /dev/null || fail "the second rollback"
check "the files go forward too: README.md is the later release's again" test "$(sha256sum < "$STACK/README.md")" != "$README_A"
expect "the system reports $TAG_B" "${TAG_B#v}" "$(edge_version)"
expect "both people are back" 2 "$(sql "SELECT count(*) FROM users")"
check "every service reports $TAG_B" ready "$TAG_B"
