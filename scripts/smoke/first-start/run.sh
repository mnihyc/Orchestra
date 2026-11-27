#!/usr/bin/env bash
# first-start: a clean directory and a release become a running system. Nothing is
# installed from files that do not match the release's manifest; the secrets are
# made once; the stack answers through its web edge with the release's version,
# its headers and its cookies; a first administrator can be made and sign in; the
# realtime socket upgrades through the edge; the services report; and the bundled
# node enrolls as soon as there is an administrator to enroll it.
set -Eeuo pipefail
# shellcheck source-path=SCRIPTDIR/..
# shellcheck source=lib.sh
. "$(dirname "${BASH_SOURCE[0]}")/../lib.sh"

A="$(release_a)"
TAG="$(release_tag "$A")"
new_stack first-start 41200 "$A"
trap 'finish' EXIT
data_disk="$SMOKE_DIR/first-start/second disk"
printf "ORCH_DATA_DIR='%s'\nORCH_POSTGRES_DATA_DIR='%s'\n" "$data_disk/server" "$data_disk/postgres" >> "$STACK/.env"

step "a release that does not match its manifest is refused before anything is installed"
DAMAGED="$SMOKE_DIR/first-start/damaged"
rm -rf "$DAMAGED"; cp -al "$A" "$DAMAGED"
rm -f "$DAMAGED/orch-frontend-dist.tar.gz"; cp "$A/orch-frontend-dist.tar.gz" "$DAMAGED/"
printf 'x' >> "$DAMAGED/orch-frontend-dist.tar.gz"
refuses "first-start stops on a file that is not the one the manifest lists" update_sh first-start --from "$DAMAGED"
check "nothing was installed" test ! -e "$STACK/runtime/server/current"
check "no container was started" test -z "$(docker ps -aq --filter "label=com.docker.compose.project=$PROJECT")"
rm -rf "$DAMAGED"

step "a partial set of secrets is refused and nothing is mended"
printf 'left over from some other system\n' > "$STACK/config/secrets/orch_master_key"
refuses "first-start stops on a directory that holds a part of the secrets" update_sh first-start --from "$A"
check "no container was started" test -z "$(docker ps -aq --filter "label=com.docker.compose.project=$PROJECT")"
expect "the file that was there is as it was" "left over from some other system" "$(cat "$STACK/config/secrets/orch_master_key")"
rm -f "$STACK/config/secrets/orch_master_key"

step "first-start from the release"
POSTGRES_PASSWORD_BEFORE="$(grep '^POSTGRES_PASSWORD=' "$STACK/.env")"
if update_sh first-start --from "$A"; then pass "first-start finished"; else fail "first-start finished"; fi
check "the .env keeps what the operator wrote" grep -qxF "$POSTGRES_PASSWORD_BEFORE" "$STACK/.env"
check "the facts of the machine were added to .env" grep -q '^ORCH_UID=' "$STACK/.env"
for kind in server frontend cli; do
  check "$kind is installed as the release's payload" test -d "$STACK/runtime/$kind/current"
done
step "first start puts server files and PostgreSQL on the operator's chosen disk"
check "PostgreSQL initialized the selected directory" test -s "$data_disk/postgres/PG_VERSION"
compose exec -T api sh -c 'echo visible > /var/lib/orch/update-status/disk-check'
check "the API writes on the selected disk" grep -qx visible "$data_disk/server/update-status/disk-check"
check "the web edge serves the same update-status directory" compose exec -T frontend grep -qx visible /var/lib/orch-update-status/disk-check
check "the updater finds an installed release on the selected disk" says 'installed already' update_sh "$TAG" --from "$A" --yes
check "the default server directory was not created" test ! -e "$STACK/runtime/data"
check "the default PostgreSQL directory was not created" test ! -e "$STACK/runtime/postgres/data"

check "there is no previous release yet" test ! -e "$STACK/runtime/server/previous"
check "a dump was taken before the stack started" test -n "$(dumps)"

step "the secrets"
for file in orch_ca_cert.pem orch_ca_key.pem orch_master_key orch_vapid_private_key node_enrollment_token \
  service_api_cert.pem service_api_key.pem service_gateway_cert.pem service_gateway_key.pem \
  service_worker_cert.pem service_worker_key.pem; do
  check "config/secrets/$file is there" test -s "$STACK/config/secrets/$file"
done
check "the secrets directory is private to its owner" test "$(stat -c %a "$STACK/config/secrets")" = 700

step "through the web edge"
expect "/health answers ok" "ok" "$(edge_get /health)"
expect "/api/v1/meta reports the release" "${TAG#v}" "$(edge_version)"
check "every response carries the security headers" has_security_headers /
check "the headers are on an API response too" has_security_headers /api/v1/meta
check "the application's page is served" bash -c "curl -fsS '$EDGE/' | grep -qi '<html'"
expect "the page is not kept by browsers" "no-store" "$(curl -sS -D - -o /dev/null "$EDGE/" | tr -d '\r' | sed -n 's/^[Cc]ache-[Cc]ontrol: *//p')"
expect "an internal address is not served" "404" "$(curl -sS -o /dev/null -w '%{http_code}' "$EDGE/internal/anything")"
SECURITY="$(curl -sS -D - -o /dev/null "$EDGE/" | tr -d '\r' | sed -n 's/^[Cc]ontent-[Ss]ecurity-[Pp]olicy: *//p')"
case "$SECURITY" in *"script-src 'self' 'nonce-"*"' 'wasm-unsafe-eval'"*"connect-src 'self'"*) pass "the content policy is the one the application is built for" ;; *) fail "the content policy: $SECURITY" ;; esac

step "the services report"
check "orchctl doctor passes in the API's container" doctor --expect-version "${TAG#v}"
expect "the node is waited for until there is an administrator" "wait" "$(doctor --only node | awk '$2 == "node" {print $1}')"

step "the first administrator, and signing in"
if setup_admin; then pass "POST /api/v1/setup made the first administrator"; else fail "POST /api/v1/setup made the first administrator"; fi
COOKIES="$(curl -sS -D - -o /dev/null -H 'content-type: application/json' -X POST "$EDGE/api/v1/auth/login" \
  -d "{\"login\":\"$ADMIN_NAME\",\"password\":\"$ADMIN_PASSWORD\"}" | tr -d '\r' | grep -i '^set-cookie:')"
case "$COOKIES" in *orch_session=*HttpOnly*Secure*SameSite=Strict*) pass "signing in sets a session cookie that is HttpOnly, Secure and SameSite=Strict" ;; *) fail "the session cookie: $COOKIES" ;; esac
if sign_in; then pass "signing in works through the edge"; else fail "signing in works through the edge"; fi
refuses "a wrong password is refused" curl -fsS -H 'content-type: application/json' -X POST "$EDGE/api/v1/auth/login" \
  -d "{\"login\":\"$ADMIN_NAME\",\"password\":\"not the password\"}"

step "browser notifications after first sign-in"
push_key_offered() {
  edge_get /api/v1/meta | jq -e '.push_public_key | type == "string" and length > 0'
}
push_channel_offered() {
  # The list answers a page: its rows are under `items`.
  edge_get /api/v1/notification-channels -H "authorization: Bearer $TOKEN" |
    jq -e '[.items[] | select(.kind == "push" and .owner == "system")] |
           length == 1 and .[0].enabled == true and .[0].route.enabled == false and .[0].route.needs == "device"'
}
check "the web edge offers a browser registration key" push_key_offered
check "the first account can choose the system push channel and has no subscribed device" push_channel_offered

step "the edge, tried from inside the stack as a browser and a machine would"
# From the API's container the web edge is the service `frontend`, and the pinned endpoint
# resolves by its name there.
if ORCH_ACCESS_TOKEN="$TOKEN" compose exec -T -e ORCH_ACCESS_TOKEN api /opt/orchctl/orchctl doctor \
  --url http://frontend --only edge --idle-secs 10 > "$SMOKE_DIR/first-start/edge.txt" 2>&1; then
  pass "orchctl doctor --url passes: page, API call, signed-in socket held idle, 8 MiB body, link upgrade, endpoints"
else
  fail "orchctl doctor --url"; cat "$SMOKE_DIR/first-start/edge.txt" >&2
fi
grep -E '^(ok|part|FAIL)' "$SMOKE_DIR/first-start/edge.txt" | cut -c1-170 || true

step "the bundled node enrolls once there is an administrator, and stays"
bundled_host_online() {
  [ "$(sql "SELECT count(*) FROM hosts h JOIN machines m ON m.id=h.machine_id WHERE h.bundled_with_server AND m.online AND m.status='active' AND 'node'=ANY(m.roles)")" = 1 ]
}
wait_for 180 "the fresh installation has its own enrolled, online host" bundled_host_online
wait_for 60 "orchctl doctor passes on the whole stack" ready "$TAG"
expect "the node container stays up" "running" "$(compose ps --format '{{.State}}' node)"

step "the first administrator uses the bundled host as themself, and nobody else does"
# A sign-in of its own, whose administrator switch is off, as a browser's first one is: Add environment's and Add
# provider's defaults place on the hosts this session lists as usable.
uses_bundled_host() {
  local reply token
  reply="$(curl -fsS -H 'content-type: application/json' -X POST "$EDGE/api/v1/auth/login" \
    -d "{\"login\":\"$ADMIN_NAME\",\"password\":\"$ADMIN_PASSWORD\"}")" || return 1
  token="$(printf '%s' "$reply" | json_field access_token)"
  edge_get /api/v1/hosts -H "authorization: Bearer $token" |
    jq -e '[.[] | select(.bundled_with_server and (.access.permissions | index("host.use")) and ((.access.admin_only // []) | index("host.use") | not))] | length == 1'
}
check "with As admin off, the first administrator may use the bundled host" uses_bundled_host
expect "its Use row names the first administrator alone" "$ADMIN_NAME" \
  "$(sql "SELECT string_agg(u.username, ',') FROM permission_entries pe JOIN hosts h ON h.id=pe.object_id JOIN users u ON u.id=pe.holder_id WHERE h.bundled_with_server AND pe.permission='host.use' AND pe.page='live' AND pe.removed_at IS NULL")"

step "./update.sh doctor, once, from the host"
DOCTOR_OUT="$(update_sh doctor 2>&1)" && DOCTOR_CODE=0 || DOCTOR_CODE=$?
expect "update.sh doctor passes on a stack that is ready" 0 "$DOCTOR_CODE"
case "$DOCTOR_OUT" in *"edge address: $EDGE (from ORCH_READY_URL in the environment)"*) pass "it names the address the edge check used, and where it came from" ;; *) fail "the edge address and its source: $DOCTOR_OUT" ;; esac
case "$DOCTOR_OUT" in *"all 3 checks passed"*"doctor: every check passed"*) pass "it prints the lines of the edge check, and says every check passed" ;; *) fail "the edge lines: $DOCTOR_OUT" ;; esac
check "it changed nothing: no transaction is open" test ! -e "$STACK/runtime/update/journal"

step "when services are stopped, doctor fails and names them"
compose stop worker gateway node > /dev/null 2>&1
# A heartbeat is fresh for 45 s after the last one, so the first lines to fail are the ones that know at once.
wait_for 150 "orchctl doctor fails on the worker, the gateway and the node, by name" doctor_fails_on worker gateway node
refuses "and its exit status says so" doctor
DOCTOR_OUT="$(update_sh doctor 2>&1)" && DOCTOR_CODE=0 || DOCTOR_CODE=$?
check "update.sh doctor fails too while they are stopped" test "$DOCTOR_CODE" != 0
for service in worker gateway node; do
  check "update.sh doctor names the stopped $service" grep -q "^FAIL  $service " <<< "$DOCTOR_OUT"
done
compose start worker gateway node > /dev/null 2>&1
wait_for 180 "orchctl doctor passes again when they are back" ready "$TAG"

step "first-start is not for a system that has a release"
refuses "first-start refuses to run over an installation" update_sh first-start --from "$A"
