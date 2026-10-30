#!/usr/bin/env bash
# installers: the command a person pastes from "Add machine", on Linux, against a running system
# behind an HTTPS front: the installer the system serves fetches the connector from the system,
# checks it against the system's manifest, places it in the person's home and enrolls the machine,
# which the system then lists; only its bound Enroll again token replaces a retained identity; a
# connector that does not match the manifest is refused and nothing is left behind. The macOS
# branch of install.sh, install.ps1 and install-node.sh (which needs root) have no native
# installation trial here; their offline help and parser coverage are kept separate and said so.
set -Eeuo pipefail
# shellcheck source-path=SCRIPTDIR/..
# shellcheck source=lib.sh
. "$(dirname "${BASH_SOURCE[0]}")/../lib.sh"

A="$(release_a)"
new_stack installers 41220 "$A"
# This native installation scenario exercises HTTPS; the offline address matrix also covers loopback HTTP.
FRONT_PORT=$((EDGE_PORT + 2))
FRONT_NAME="${PROJECT}-front"
leave() { docker rm -f "$FRONT_NAME" > /dev/null 2>&1 || true; finish; }
trap leave EXIT
WORK="$SMOKE_DIR/installers"
# Mint only commands whose configured addresses the installers can use.
sed -i -e "s|^public_url = .*|public_url = \"https://localhost:$FRONT_PORT\"|" \
  -e "s|^url = \"wss://gateway.example.com:9443\"|url = \"wss://127.0.0.1:$GATEWAY_PORT\"|" "$STACK/config/orch.toml"

step "installer help finishes without enrollment, platform access or installation"
check "both shell installers explain their lifecycle in English and Chinese" \
  sh "$SMOKE_REPO/scripts/smoke/installers/help.sh"
check "system addresses allow HTTP only on literal loopback hosts, for both download backends" \
  sh "$SMOKE_REPO/scripts/smoke/installers/addresses.sh"
check "Unix connector services retain the shell's proxy in private definitions" \
  python3 -I "$SMOKE_REPO/scripts/smoke/installers/test_service.py"
check "the node service retains its proxy and literal paths on a minimal host" \
  python3 -I "$SMOKE_REPO/scripts/smoke/installers/test_node_service.py"

step "a system, and an HTTPS front under the system's own authority"
update_sh first-start --from "$A" > "$WORK/first-start.log" 2>&1 || abort "first-start failed (see $WORK/first-start.log)"
setup_admin || abort "the first administrator could not be made"
SECRETS="$STACK/config/secrets"
FRONT="$WORK/front"
mkdir -p "$FRONT"
openssl req -new -newkey ec -pkeyopt ec_paramgen_curve:P-256 -nodes -subj /CN=localhost \
  -keyout "$FRONT/key.pem" -out "$FRONT/request.pem" 2> /dev/null
printf 'subjectAltName=DNS:localhost\nextendedKeyUsage=serverAuth\n' > "$FRONT/extensions"
openssl x509 -req -in "$FRONT/request.pem" -CA "$SECRETS/orch_ca_cert.pem" -CAkey "$SECRETS/orch_ca_key.pem" \
  -CAcreateserial -CAserial "$FRONT/serial" -days 2 -extfile "$FRONT/extensions" -out "$FRONT/leaf.pem" 2> /dev/null
# The authority goes along: a machine that enrolls finds it in the chain by the pin its token carries.
cat "$FRONT/leaf.pem" "$SECRETS/orch_ca_cert.pem" > "$FRONT/cert.pem"
cat > "$FRONT/nginx.conf" <<EOF
events {}
http {
  server {
    listen 127.0.0.1:$FRONT_PORT ssl;
    ssl_certificate /front/cert.pem;
    ssl_certificate_key /front/key.pem;
    client_max_body_size 8m;
    location / {
      proxy_pass http://127.0.0.1:$EDGE_PORT;
      proxy_http_version 1.1;
      proxy_set_header Host \$host;
      proxy_set_header X-Forwarded-Proto https;
    }
  }
}
EOF
NGINX_IMAGE="$(grep -o 'nginx:[^ ]*@sha256:[0-9a-f]*' "$STACK/compose.yml" | head -n 1)"
docker run -d --name "$FRONT_NAME" --network host -v "$FRONT:/front:ro" \
  -v "$FRONT/nginx.conf:/etc/nginx/nginx.conf:ro" "$NGINX_IMAGE" > /dev/null
SYSTEM="https://localhost:$FRONT_PORT"
export CURL_CA_BUNDLE="$SECRETS/orch_ca_cert.pem"
wait_for 60 "the system answers through the HTTPS front" curl -fsS "$SYSTEM/api/v1/meta"

# machine_token: a single-use token for a machine of the administrator's, as "Add machine" makes it.
machine_token() {
  curl -fsS -H "authorization: Bearer $TOKEN" -H 'content-type: application/json' \
    -X POST "$EDGE/api/v1/machines/enrollment-tokens" -d '{}' | json_field token
}
# install HOME TOKEN: the Linux command of "Add machine", for a person whose home is HOME, without
# the per-user service (no user manager runs here); what it says goes to HOME.log.
install() {
  mkdir -p "$1"
  curl -fsSL "$SYSTEM/api/v1/downloads/install.sh" |
    env -i PATH="$PATH" HOME="$1" CURL_CA_BUNDLE="$CURL_CA_BUNDLE" SSL_CERT_FILE="$CURL_CA_BUNDLE" \
      sh -s -- --url "$SYSTEM" --token "$2" --name "smoke-$(basename "$1")" --no-service > "$1.log" 2>&1
}

step "the connector is fetched, checked, placed and enrolled"
PERSON="$WORK/person"
FIRST="$(machine_token)"
[ -n "$FIRST" ] || abort "no machine token was made"
if install "$PERSON" "$FIRST"; then pass "the installer finished"; else fail "the installer finished (see $PERSON.log)"; fi
check "it checked the download against the manifest" grep -q 'SHA-256 as listed' "$PERSON.log"
check "the connector is in the person's ~/.local/bin" test -x "$PERSON/.local/bin/orch-connector"
check "the machine holds its identity" test -f "$PERSON/.local/share/orch-connector/identity/enrollment.json"
check "the system lists the machine" says 'smoke-person' curl -fsS -H "authorization: Bearer $TOKEN" "$EDGE/api/v1/machines"
check "the token is printed nowhere" bash -c "! grep -qF -- '$FIRST' '$PERSON.log'"

step "an enrolled directory rejects any token not bound to that same machine"
BEFORE="$(sha256sum "$PERSON/.local/bin/orch-connector" | cut -d' ' -f1)"
IDENTITY="$PERSON/.local/share/orch-connector/identity/enrollment.json"
MACHINE_ID="$(jq -r .machine_id "$IDENTITY")"
CONFIG="$PERSON/.local/share/orch-connector/config.toml"
CONFIG_BEFORE="$(sha256sum "$CONFIG" | cut -d' ' -f1)"
OTHER_TOKEN="$(machine_token)"
if install "$PERSON" "$OTHER_TOKEN"; then fail "the new-machine token is refused on a retained identity"; else pass "the new-machine token is refused on a retained identity"; fi
check "the refusal points to Enroll again" grep -q 'Enroll again' "$PERSON.log"
expect "the connector remains the verified file" "$BEFORE" "$(sha256sum "$PERSON/.local/bin/orch-connector" | cut -d' ' -f1)"
expect "local configuration is retained" "$CONFIG_BEFORE" "$(sha256sum "$CONFIG" | cut -d' ' -f1)"
expect "the retained machine id is unchanged" "$MACHINE_ID" "$(jq -r .machine_id "$IDENTITY")"

step "the bound Enroll again token replaces the retained identity"
AGAIN_TOKEN="$(curl -fsS -H "authorization: Bearer $TOKEN" -X POST "$EDGE/api/v1/machines/$MACHINE_ID/enrollment-tokens" | json_field token)"
if install "$PERSON" "$AGAIN_TOKEN"; then pass "the bound replacement is installed"; else fail "the bound replacement is installed"; fi
expect "the page keeps the same machine" "$MACHINE_ID" "$(jq -r .machine_id "$IDENTITY")"
expect "the local settings still match" "$CONFIG_BEFORE" "$(sha256sum "$CONFIG" | cut -d' ' -f1)"
check "the supplied replacement token is printed nowhere" bash -c "! grep -qF -- '$AGAIN_TOKEN' '$PERSON.log'"

step "a connector that does not match the manifest is refused and nothing is left"
SERVED="$(find -L "$STACK/runtime/server/current/connectors" -maxdepth 1 -name 'orch-connector-linux-*' | head -n 1)"
if [ -z "$SERVED" ]; then
  fail "the release serves a Linux connector"
elif ! cp "$SERVED" "$WORK/served-connector" || ! printf 'x' >> "$SERVED" 2> /dev/null; then
  say "NOT RUN  a changed connector: the installed release cannot be written here"
else
  STRANGER="$WORK/stranger"
  if install "$STRANGER" "$(machine_token)"; then fail "the changed connector is refused"; else pass "the changed connector is refused"; fi
  check "it says why" grep -q 'cut or changed on the way' "$STRANGER.log"
  check "no connector was left in the person's home" test ! -e "$STRANGER/.local/bin/orch-connector"
  check "no identity was made" test ! -e "$STRANGER/.local/share/orch-connector/identity"
  cp "$WORK/served-connector" "$SERVED"
fi

step "installer parsing and Windows offline help, without native installation"
check "install.sh is a valid POSIX shell script" sh -n "$SMOKE_REPO/deploy/install/install.sh"
check "install-node.sh is a valid POSIX shell script (it needs root to run, which this scenario has not)" \
  sh -n "$SMOKE_REPO/deploy/install/install-node.sh"
if command -v pwsh > /dev/null 2>&1; then
  check "install.ps1 parses" pwsh -NoProfile -Command \
    "\$errors = \$null; [void][System.Management.Automation.Language.Parser]::ParseFile('$SMOKE_REPO/deploy/install/install.ps1', [ref]\$null, [ref]\$errors); if (\$errors) { exit 1 }"
  check "Windows help needs no credentials and attempts no installation" \
    pwsh -NoProfile -File "$SMOKE_REPO/scripts/smoke/installers/help.ps1"
  check "Windows allows the same loopback addresses and refuses unchecked redirects" \
    pwsh -NoProfile -File "$SMOKE_REPO/scripts/smoke/installers/addresses.ps1"
  check "Windows ARM64 uses the x86-64 build and explains emulation" \
    pwsh -NoProfile -File "$SMOKE_REPO/scripts/smoke/installers/architecture.ps1"
  check "a failed Windows installation cannot start a foreground connector" \
    pwsh -NoProfile -File "$SMOKE_REPO/scripts/smoke/installers/foreground.ps1"
else
  say "NOT RUN  install.ps1: no PowerShell here; it is read on Windows only"
fi
say "NOT RUN  the macOS branch of install.sh and the per-user services: macOS and a user manager are not here"
