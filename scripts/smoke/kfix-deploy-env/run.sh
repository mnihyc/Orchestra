#!/usr/bin/env bash
# The updater protects the database password before start, update, rollback and
# restore, and refuses an exposed file whose owner must correct it.
set -Eeuo pipefail
# shellcheck source=../lib.sh
. "$(dirname "${BASH_SOURCE[0]}")/../lib.sh"

A="$(release_a)"
B="$(release_b)"
TAG_A="$(release_tag "$A")"
TAG_B="$(release_tag "$B")"
new_stack kfix-deploy-env 41060 "$A"
trap finish EXIT

# A supplied release may predate this checkout's updater. Test the current one
# against the real programs and database of those verified payloads.
install -m 755 "$SMOKE_REPO/deploy/update.sh" "$STACK/update.sh"
OWNER="$(id -u):$(id -g)"
password_reader() {
  docker run --rm --network none --user 65534:65534 \
    --mount "type=bind,src=$STACK,dst=/fixture,readonly" debian:bookworm-slim@sha256:7c7b2c966bc9ee8cedfeef67e0e279108992c77681fa595db4a9d65c06ccc587 \
    cat /fixture/.env > /dev/null 2>&1
}
env_owner() {
  docker run --rm --network none --user 0:0 \
    --mount "type=bind,src=$STACK,dst=/fixture" debian:bookworm-slim@sha256:7c7b2c966bc9ee8cedfeef67e0e279108992c77681fa595db4a9d65c06ccc587 \
    chown "$1" /fixture/.env
}

step "an exposed password the updater cannot protect stops before Compose"
chmod 644 "$STACK/.env"
check "a second account can traverse the fixture and read its exposed password" password_reader
env_owner 0:0
PROBE="$SMOKE_DIR/kfix-deploy-env/probe"
mkdir -p "$PROBE"
cat > "$PROBE/docker" <<'SH'
#!/usr/bin/env bash
printf 'called\n' > "${ORCH_COMPOSE_PROBE:?}"
exit 77
SH
chmod 755 "$PROBE/docker"
if PATH="$PROBE:$PATH" ORCH_COMPOSE_PROBE="$PROBE/called" update_sh first-start --from "$A" > "$PROBE/refusal.log" 2>&1; then
  fail "first-start refused the uncorrectable password file"
else
  pass "first-start refused the uncorrectable password file"
fi
check "the refusal names the command its owner must run" grep -q 'chmod 600 -- .env' "$PROBE/refusal.log"
check "Docker and Compose were never called" test ! -e "$PROBE/called"
env_owner "$OWNER"

step "first start corrects mode 0644 before using the password"
START_LOG="$SMOKE_DIR/kfix-deploy-env/first-start.log"
update_sh first-start --from "$A" > "$START_LOG" 2>&1
check "the first start reports protecting .env" grep -q 'set .env mode to 0600' "$START_LOG"
expect "the password file is mode 0600" 600 "$(stat -c %a "$STACK/.env")"
refuses "a second unprivileged account cannot read the password" password_reader
check "the running stack can use the password" ready "$TAG_A"
setup_admin
wait_for 240 "the bundled node is online" node_online

step "update and rollback preserve a private password file"
chmod 644 "$STACK/.env"
UPDATE_LOG="$SMOKE_DIR/kfix-deploy-env/update.log"
update_sh "$TAG_B" --from "$B" --yes > "$UPDATE_LOG" 2>&1
check "the update reports protecting .env" grep -q 'set .env mode to 0600' "$UPDATE_LOG"
expect "the update leaves mode 0600" 600 "$(stat -c %a "$STACK/.env")"
refuses "the updated password is still private" password_reader
update_sh rollback --yes > "$SMOKE_DIR/kfix-deploy-env/rollback.log" 2>&1
expect "the rollback leaves mode 0600" 600 "$(stat -c %a "$STACK/.env")"
check "the rollback still uses the password" ready "$TAG_A"

step "restore copies the password privately and uses it with the restored database"
KEPT="$SMOKE_DIR/kfix-deploy-env/kept"
mkdir -p "$KEPT"
update_sh backup > /dev/null
cp "$(dumps | tail -n 1)" "$KEPT/orch.dump"
cp -a "$STACK/config" "$KEPT/config"
install -m 600 "$STACK/.env" "$KEPT/.env"
cp -a "$STACK/runtime/data" "$KEPT/data"
cp -a "$STACK/runtime/node" "$KEPT/node"
compose down -v > /dev/null 2>&1
mv "$STACK" "$KEPT/old-deploy"
tar -xzf "$A/orch-deploy-$TAG_A.tar.gz" -C "$(dirname "$STACK")"
mv "$STACK/config" "$KEPT/example-config"
cp -a "$KEPT/config" "$STACK/config"
install -m 600 "$STACK/.env.example" "$STACK/.env"
install -m 600 "$KEPT/.env" "$STACK/.env"
mkdir -p "$STACK/runtime"
cp -a "$KEPT/data" "$STACK/runtime/data"
cp -a "$KEPT/node" "$STACK/runtime/node"
install -m 755 "$SMOKE_REPO/deploy/update.sh" "$STACK/update.sh"
update_sh first-start --from "$A" --restore-database "$KEPT/orch.dump" > "$SMOKE_DIR/kfix-deploy-env/restore.log" 2>&1
expect "restore leaves mode 0600" 600 "$(stat -c %a "$STACK/.env")"
refuses "the restored password is private" password_reader
check "the same person signs in to the restored database" sign_in
wait_for 240 "the restored stack still uses its password" ready "$TAG_A"
