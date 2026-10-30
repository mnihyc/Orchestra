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
case "$SECURITY" in *"script-src 'self' 'wasm-unsafe-eval'"*"connect-src 'self'"*) pass "the content policy is the one the application is built for" ;; *) fail "the content policy: $SECURITY" ;; esac

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
wait_for 180 "the node is online" bash -c "cd '$STACK' && docker compose exec -T api /opt/orchctl/orchctl doctor --only node | grep -q '^ok'"
wait_for 60 "orchctl doctor passes on the whole stack" ready "$TAG"
expect "the node container stays up" "running" "$(compose ps --format '{{.State}}' node)"

step "when services are stopped, doctor fails and names them"
compose stop worker gateway node > /dev/null 2>&1
# A heartbeat is fresh for 45 s after the last one, so the first lines to fail are the ones that know at once.
wait_for 150 "orchctl doctor fails on the worker, the gateway and the node, by name" doctor_fails_on worker gateway node
refuses "and its exit status says so" doctor
compose start worker gateway node > /dev/null 2>&1
wait_for 180 "orchctl doctor passes again when they are back" ready "$TAG"

step "first-start is not for a system that has a release"
refuses "first-start refuses to run over an installation" update_sh first-start --from "$A"
