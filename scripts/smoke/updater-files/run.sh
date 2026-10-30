#!/usr/bin/env bash
# Exercise the updater's private input and service-user staging without starting a stack.
set -Eeuo pipefail

repo="$(cd "$(dirname "${BASH_SOURCE[0]}")/../../.." && pwd -P)"
updater="${ORCH_UPDATER_UNDER_TEST:-$repo/deploy/update.sh}"
image='debian:bookworm-slim@sha256:7c7b2c966bc9ee8cedfeef67e0e279108992c77681fa595db4a9d65c06ccc587'

docker run -i --rm --pull=never --network none --read-only --tmpfs /work:mode=755,size=16m \
  -v "$updater:/input/update.sh:ro" --entrypoint /bin/bash "$image" -s <<'INNER'
set -Eeuo pipefail
# Load the real functions without dispatching a deployment command.
sed '$d' /input/update.sh > /work/update-functions.sh
# shellcheck disable=SC1091 # written immediately above from the updater under test
source /work/update-functions.sh
DEPLOY_DIR=/work
WORK=/work/scratch
RELEASE_DIR=/work/operator-only
SERVICE_UID=65534
SERVICE_GID=65534
RUNTIME_IMAGE=unused
ORCH_UPDATE_NO_UNZIP=1
ASSET_server=server.zip
TAG=v1.0.0
mkdir -p "$WORK" "$RELEASE_DIR" "$DEPLOY_DIR/runtime/server/releases"
chmod 700 "$RELEASE_DIR"
printf 'the archive' > "$RELEASE_DIR/$ASSET_server"
chmod 600 "$RELEASE_DIR/$ASSET_server"
asset_field() { printf '%064d\n' 0; }
# The real container sees its bind mounts directly. Here the service user checks
# their ownership and reads the private copy before producing representative files.
toolbox() {
  export TEST_INPUT="${TOOLBOX_INPUT:-$RELEASE_DIR}" TEST_ASSET="$ASSET_server" TEST_STAGE="$STAGE_PARENT/$stage"
  su -s /bin/sh nobody -c '
    set -eu
    test -r "$TEST_INPUT/$TEST_ASSET"
    test -w "$TEST_STAGE"
    mkdir -p "$TEST_STAGE/bin" "$TEST_STAGE/connectors"
    printf executable > "$TEST_STAGE/bin/orch-api"
    printf executable > "$TEST_STAGE/connectors/orch-connector-test"
    printf installer > "$TEST_STAGE/connectors/install.sh"
  '
}
if su -s /bin/sh nobody -c 'test -r /work/operator-only/server.zip'; then
  printf 'the original release was not private\n' >&2
  exit 1
fi
unpack server
test -f "$DEPLOY_DIR/runtime/server/releases/$UNPACKED/.complete"
test -f "$DEPLOY_DIR/runtime/server/releases/$UNPACKED/bin/orch-api"
printf 'PASS: a root-run updater stages a private archive for the service user\n'

# Stand in for compose-secrets so the updater's decision can be tested without
# launching services. It adds the new key only to a complete older set.
mkdir -p /work/config/secrets
for secret in $SECRET_FILES; do
  [ "$secret" = orch_ports_assertion_key ] || printf 'keep %s\n' "$secret" > "/work/config/secrets/$secret"
done
docker() {
  local secret
  printf 'called\n' >> /work/secrets-calls
  for secret in $SECRET_FILES; do
    [ "$secret" = orch_ports_assertion_key ] || [ -f "/work/config/secrets/$secret" ] || return 1
  done
  printf 'new key\n' > /work/config/secrets/orch_ports_assertion_key
}
ensure_secrets /work/new-cli
test "$(cat /work/config/secrets/orch_master_key)" = 'keep orch_master_key'
test "$(cat /work/config/secrets/orch_ports_assertion_key)" = 'new key'
ensure_secrets /work/new-cli
test "$(wc -l < /work/secrets-calls)" -eq 1
printf 'PASS: an older complete secret set gains one persistent key and is then left alone\n'
mv /work/config /work/complete-config
mkdir -p /work/config/secrets
printf 'keep this key\n' > /work/config/secrets/orch_master_key
if (ensure_secrets /work/new-cli) > /work/partial-secrets.log 2>&1; then
  printf 'a partial original set was accepted\n' >&2
  exit 1
fi
test "$(cat /work/config/secrets/orch_master_key)" = 'keep this key'
test ! -e /work/config/secrets/orch_ports_assertion_key
printf 'PASS: a partial original secret set is refused without replacement\n'

# A cut at begin occurs before the transaction has assigned a dump name.
for payload in $PAYLOADS; do
  mkdir -p "/work/runtime/$payload/releases/old" "/work/runtime/$payload/releases/new"
  ln -s releases/new "/work/runtime/$payload/current"
done
journal_load() {
  local payload
  J=([kind]=update [step]=begin [from_tag]=v1.0.0 [to_tag]=v1.1.0)
  for payload in $PAYLOADS; do
    J[${payload}_current_before]=old
    J[${payload}_previous_before]=''
  done
}
require_installed() { :; }
ensure_secrets() { :; }
rm() { :; }
app_start() {
  local payload
  for payload in $PAYLOADS; do test "$(readlink "/work/runtime/$payload/current")" = releases/old; done
}
wait_ready() { test "$1" = v1.0.0; }
journal_clear() { printf 'cleared\n' > /work/journal-cleared; }
cmd_recover
test -f /work/journal-cleared
printf 'PASS: recovery from begin works before a dump name exists\n'

# An unavailable work inventory is not the same as zero running work.
sql() { return 1; }
TAG=v1.1.0
CONNECTOR_MINIMUM=1.0.0
if (warn_about_work) > /work/work-inventory-failure.log 2>&1; then
  printf 'an unreadable work inventory was treated as empty\n' >&2
  exit 1
fi
grep -q 'could not inspect' /work/work-inventory-failure.log
printf 'PASS: an update refuses to proceed when its work inventory is unavailable\n'
sql() {
  case "$1" in *'FROM threads'*) printf '0\n' ;; *) return 1 ;; esac
}
if (warn_about_work) > /work/runtime-inventory-failure.log 2>&1; then
  printf 'an unreadable runtime inventory was treated as empty\n' >&2
  exit 1
fi
grep -q 'could not inspect runtime connector versions' /work/runtime-inventory-failure.log
sql() {
  case "$1" in
    *'FROM threads'*) printf '0\n' ;;
    *'position('* ) printf '2\n' ;;
    *) : ;;
  esac
}
warn_about_work > /work/prerelease-warning.log
test "$NEEDS_ANSWER" -eq 1
grep -q '2 runtimes use prerelease connectors' /work/prerelease-warning.log
printf 'PASS: prerelease runtimes at the minimum are named before an update\n'
mkdir -p /work/invalid-release
cat > /work/invalid-release/version.json <<'MANIFEST'
{"schema_version":1,"project":"orch","tag":"v1.1.0","version":"1.1.0","connector_minimum":"1.0.0' OR TRUE--"}
MANIFEST
if (load_manifest /work/invalid-release) > /work/invalid-minimum.log 2>&1; then
  printf 'an invalid minimum connector version was accepted\n' >&2
  exit 1
fi
grep -q 'connector_minimum' /work/invalid-minimum.log
printf 'PASS: an invalid minimum connector version is refused\n'

# Pruning must keep both linked releases, the newest unused releases and
# anything it cannot establish as a complete release. Record removals only.
DEPLOY_DIR=/work/retention
mkdir -p "$DEPLOY_DIR/runtime/server/releases" /work/outside
cd "$DEPLOY_DIR"
PAYLOADS=server
for number in 0 1 2 3 4 5; do
  directory="runtime/server/releases/release-$number"
  mkdir -p "$directory"
  printf '%064d\n' 0 > "$directory/.complete"
  touch -d "2026-01-01 00:00:0$number UTC" "$directory"
done
ln -s releases/release-0 runtime/server/current
ln -s releases/release-1 runtime/server/previous
ln -s /work/outside runtime/server/releases/external-link
mkdir -p runtime/server/releases/incomplete runtime/server/releases/.staging-fixture
release_keep_count() { printf '2\n'; }
rm() { printf '%s\n' "$*" >> /work/prune-calls; }
prune_releases 0 > /work/prune-preview.log
grep -q 'Would remove server/release-2' /work/prune-preview.log
grep -q 'Would remove server/release-3' /work/prune-preview.log
test "$(wc -l < /work/prune-preview.log)" -eq 2
prune_releases 1 > /work/prune-apply.log
grep -q '/release-2' /work/prune-calls
grep -q '/release-3' /work/prune-calls
test "$(wc -l < /work/prune-calls)" -eq 2
test -L runtime/server/releases/external-link
test -d runtime/server/releases/incomplete
test -d runtime/server/releases/.staging-fixture
printf 'PASS: pruning preserves linked, newest, incomplete and external releases\n'

# Rollback reads the target release's own connector minimum before warning.
mkdir -p runtime/server/releases/release-1/connectors
cat > runtime/server/releases/release-1/connectors/version.json <<'MANIFEST'
{"schema_version":1,"project":"orch","tag":"v1.0.0","version":"1.0.0","connector_minimum":"0.5.0"}
MANIFEST
load_manifest_of_installed v1.0.0
if [ "$CONNECTOR_MINIMUM" != 0.5.0 ]; then
  printf 'rollback did not read the previous release connector minimum\n' >&2
  exit 1
fi
if (load_manifest_of_installed v9.9.9) > /work/rollback-wrong-tag.log 2>&1; then
  printf 'rollback accepted a manifest for the wrong release\n' >&2
  exit 1
fi
mv runtime/server/releases/release-1/connectors/version.json runtime/server/releases/release-1/connectors/version.saved
if (load_manifest_of_installed v1.0.0) > /work/rollback-missing-manifest.log 2>&1; then
  printf 'rollback accepted a missing manifest\n' >&2
  exit 1
fi
printf 'PASS: rollback checks the previous release connector minimum\n'

backup_command() { return 1; }
state_get() { printf 'v1.0.0\n'; }
J=([kind]=update)
if (take_dump runtime/update-backups/fixture.dump) > /work/backup-failure.log 2>&1; then
  printf 'a failed update backup was accepted\n' >&2
  exit 1
fi
if ! grep -q 'services were stopped.*update.sh recover' /work/backup-failure.log; then
  cat /work/backup-failure.log >&2
  exit 1
fi
J=()
if (take_dump runtime/update-backups/fixture.dump manual) > /work/manual-backup-failure.log 2>&1; then
  printf 'a failed manual backup was accepted\n' >&2
  exit 1
fi
if grep -q 'services were stopped' /work/manual-backup-failure.log; then
  printf 'a manual backup falsely said services were stopped\n' >&2
  exit 1
fi
printf 'PASS: failed update backups explain recovery without misreporting manual backups\n'
INNER
