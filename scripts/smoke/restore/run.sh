#!/usr/bin/env bash
# restore: what the backup procedure keeps is enough to put the system back on a
# clean directory, as the same system. The database comes back from its dump
# before the stack starts, the authority is the old one (so the secrets are not
# made again and machines would reconnect), and the bundled node, which keeps
# its identity beside the stack, reconnects without enrolling again.
set -Eeuo pipefail
# shellcheck source-path=SCRIPTDIR/..
# shellcheck source=lib.sh
. "$(dirname "${BASH_SOURCE[0]}")/../lib.sh"

A="$(release_a)"
TAG_A="$(release_tag "$A")"
new_stack restore 41216 "$A"
KEPT="$SMOKE_DIR/restore-kept"
leave() { local status=0; finish || status=$?; rm -rf "${KEPT:?}"; exit "$status"; }
trap leave EXIT

step "a system with a person in it, and its backup"
update_sh first-start --from "$A" > /dev/null || fail "first-start"
setup_admin || fail "the first administrator was not made"
wait_for 240 "the bundled node is online" node_online
FINGERPRINT="$(sha256sum "$STACK/config/secrets/orch_ca_cert.pem" | cut -d' ' -f1)"
expect "a backup is taken while everything runs" "ok" "$(update_sh backup > /dev/null && echo ok)"
rm -rf "$KEPT"; mkdir -p "$KEPT"
cp "$(dumps | tail -n 1)" "$KEPT/orch.dump"
cp -a "$STACK/config" "$KEPT/config"
cp -a "$STACK/.env" "$KEPT/.env"
cp -a "$STACK/runtime/data" "$KEPT/data"
cp -a "$STACK/runtime/node" "$KEPT/node"
check "the dump reads back" bash -c "cd '$STACK' && docker compose exec -T postgres pg_restore --list < '$KEPT/orch.dump' | grep -q users"

step "the system is lost"
compose down -v > /dev/null 2>&1
rm -rf "$(dirname "$STACK")"
mkdir -p "$(dirname "$STACK")"
tar -xzf "$A/orch-deploy-$TAG_A.tar.gz" -C "$(dirname "$STACK")"

step "put back on a clean directory of the same release"
rm -rf "$STACK/config"
cp -a "$KEPT/config" "$STACK/config"
cp -a "$KEPT/.env" "$STACK/.env"
mkdir -p "$STACK/runtime"
cp -a "$KEPT/data" "$STACK/runtime/data"
cp -a "$KEPT/node" "$STACK/runtime/node"
refuses "a dump can be restored by a first start and by nothing else" update_sh backup --restore-database "$KEPT/orch.dump"
# On a host that has no unzip, a container of the web edge'"'"'s image unpacks the server archive; this one is made to.
if (cd "$STACK" && ORCH_UPDATE_NO_UNZIP=1 ./update.sh first-start --from "$A" --restore-database "$KEPT/orch.dump") > "$SMOKE_DIR/restore/first-start.txt" 2>&1; then pass "first-start with the dump finished"; else fail "first-start with the dump finished"; cat "$SMOKE_DIR/restore/first-start.txt" >&2; fi
check "it restored the database" grep -q 'restoring the database from' "$SMOKE_DIR/restore/first-start.txt"
check "it found the secrets complete and made none" grep -q 'the secrets in config/secrets are all there' "$SMOKE_DIR/restore/first-start.txt"
expect "the authority is the one of the lost system" "$FINGERPRINT" "$(sha256sum "$STACK/config/secrets/orch_ca_cert.pem" | cut -d' ' -f1)"
check "the same person signs in with the same password" sign_in
expect "the system reports $TAG_A" "${TAG_A#v}" "$(edge_version)"
wait_for 240 "the bundled node reconnects with the identity it kept" node_online
check "every service reports $TAG_A" ready "$TAG_A"
expect "there is one person" 1 "$(sql "SELECT count(*) FROM users")"
