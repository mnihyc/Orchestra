#!/usr/bin/env bash
# Shared helpers of the deployment smoke scenarios; each scenario's run.sh sources
# this file. A scenario makes a stack of its own from a packaged release, in a
# directory of its own, under a Compose project named orchsmoke<scenario>, runs
# what the updater promises, and says for each promise whether it was kept.
#
# What it needs: Docker with Compose, the pinned Rust toolchain (to package a
# release when none is given), curl and zip.
#
#   ORCH_SMOKE_RELEASE_A, ORCH_SMOKE_RELEASE_B  directories made by `cargo xtask
#       package`, of two releases (B newer than A). Without them the scenario
#       packages releases 0.1.0 and 0.2.0 into $ORCH_SMOKE_DIR/releases, once for
#       every run on the machine: one packaging at a time, made aside and put in
#       place whole, so that two runs never replace or remove each other's.
#   ORCH_SMOKE_DIR   where stacks and releases are made (default: .tmp/deploy-smoke
#       below the repository; the directory is ignored by git). Each run makes its
#       stacks below runs/<run>, and takes away only its own.
#   ORCH_SMOKE_RUN   the name of this run (default: the UTC time and the process),
#       which names its directory and its Compose projects.
#   ORCH_SMOKE_PORT  the first of the two ports a scenario publishes: the web edge
#       on 127.0.0.1 and the gateway's pinned listener on the Docker bridge's
#       address. Each scenario has a default of its own in 41200-41249.
#   ORCH_SMOKE_KEEP=1  leave the stack up at the end, to look at it.
#   ORCH_SMOKE_PROJECT_PREFIX  what the name of the Compose project begins with
#       (default orchsmoke and the run's name); the scenario's name follows it.

set -Eeuo pipefail

SMOKE_REPO="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd -P)"
SMOKE_BASE="${ORCH_SMOKE_DIR:-$SMOKE_REPO/.tmp/deploy-smoke}"
SMOKE_RUN="${ORCH_SMOKE_RUN:-$(date -u +%Y%m%d%H%M%S)p$$}"
case "$SMOKE_RUN" in
  *[!a-z0-9]* | "") printf 'smoke: ORCH_SMOKE_RUN is lower-case letters and digits, not %s\n' "$SMOKE_RUN" >&2; exit 2 ;;
esac
SMOKE_RELEASES="$SMOKE_BASE/releases"
SMOKE_DIR="$SMOKE_BASE/runs/$SMOKE_RUN"
mkdir -p "$SMOKE_DIR"
FAILED=0
CHECKED=0

say() { printf '%s\n' "$*"; }
pass() { CHECKED=$((CHECKED + 1)); printf 'PASS  %s\n' "$*"; }
fail() { CHECKED=$((CHECKED + 1)); FAILED=$((FAILED + 1)); printf 'FAIL  %s\n' "$*" >&2; }
step() { printf '\n== %s\n' "$*"; }
abort() { printf 'smoke: %s\n' "$*" >&2; exit 2; }

# check WHAT COMMAND...: the command succeeds.
check() {
  local what="$1"; shift
  if "$@" > /dev/null 2>&1; then pass "$what"; else fail "$what"; fi
}

# expect WHAT EXPECTED ACTUAL: the two are equal.
expect() {
  if [ "$2" = "$3" ]; then pass "$1"; else fail "$1: expected '$2', got '$3'"; fi
}

# says PATTERN COMMAND...: the command prints a line with PATTERN in it.
says() { local pattern="$1"; shift; "$@" 2>&1 | grep -q -- "$pattern"; }

# refuses WHAT COMMAND...: the command fails.
refuses() {
  local what="$1"; shift
  if "$@" > /dev/null 2>&1; then fail "$what (it succeeded)"; else pass "$what"; fi
}

# --- Releases -----------------------------------------------------------------

# release VERSION: the directory of the release, packaged now if it is not there. Runs
# share releases: one packages at a time (a second waits, then finds the release made),
# into a directory of its own that is renamed into place whole, so that no run sees a
# release half made, and the image a release tags is built once for every run.
release() {
  local dir="$SMOKE_RELEASES/v$1" partial lock
  if [ ! -f "$dir/version.json" ]; then
    mkdir -p "$SMOKE_RELEASES"
    exec {lock}> "$SMOKE_RELEASES/.packaging.lock"
    flock "$lock"
    if [ ! -f "$dir/version.json" ]; then
      partial="$SMOKE_RELEASES/.v$1.$SMOKE_RUN"
      rm -rf "$partial"
      say "packaging release $1 into $dir (a first run takes a while)" >&2
      if ! (cd "$SMOKE_REPO" && ORCH_RELEASE_VERSION="$1" cargo xtask package --out "$partial") >&2; then
        rm -rf "$partial"
        abort "cargo xtask package failed"
      fi
      # What is there without a manifest is what a run cut off before this one left.
      rm -rf "$dir"
      mv -T "$partial" "$dir"
    fi
    exec {lock}>&-
  fi
  printf '%s' "$dir"
}

release_a() { if [ -n "${ORCH_SMOKE_RELEASE_A:-}" ]; then printf '%s' "$ORCH_SMOKE_RELEASE_A"; else release 0.1.0; fi; }
release_b() { if [ -n "${ORCH_SMOKE_RELEASE_B:-}" ]; then printf '%s' "$ORCH_SMOKE_RELEASE_B"; else release 0.2.0; fi; }

# release_tag DIR: the tag the release names.
release_tag() { sed -n 's/.*"tag": *"\([^"]*\)".*/\1/p' "$1/version.json" | head -n 1; }

# broken_release SOURCE TARGET: a copy of a release whose schema holds a change that
# destroys the users' table and one that cannot be applied; the programs of it
# start, apply the first and stop at the second. The manifest lists the changed
# archive truthfully, so only its readiness can tell it from a good release.
broken_release() {
  local source="$1" target="$2" work
  rm -rf "$target"
  cp -al "$source" "$target"
  work="$(mktemp -d)"
  mkdir -p "$work/migrations"
  printf 'ALTER TABLE users RENAME TO users_gone;\n' > "$work/migrations/0002_a_change_that_ruins_the_users.sql"
  printf 'SELECT * FROM a_table_that_is_not_there;\n' > "$work/migrations/0003_a_change_that_cannot_be_applied.sql"
  # The files that change are copies of their own: the others are links to the release's.
  rm -f "$target/orch-server-linux-x86_64.zip" "$target/version.json"
  cp "$source/orch-server-linux-x86_64.zip" "$source/version.json" "$target/"
  (cd "$work" && zip -q "$target/orch-server-linux-x86_64.zip" migrations/*.sql)
  rm -rf "$work"
  (cd "$SMOKE_REPO" && cargo xtask package --out "$target" --manifest-only) >&2 || abort "the manifest of the broken release was not written"
}

# --- Stacks -------------------------------------------------------------------

# bridge_address: the address `host-gateway` stands for in a container.
bridge_address() { docker network inspect bridge --format '{{(index .IPAM.Config 0).Gateway}}'; }

# new_stack NAME DEFAULT_PORT RELEASE: a stack directory made from the release's deploy
# bundle, with an .env and a configuration for a stack that publishes on loopback.
# Sets STACK (the deploy directory), PROJECT, EDGE_PORT, GATEWAY_PORT and EDGE (its address).
new_stack() {
  local name="$1" port="${ORCH_SMOKE_PORT:-$2}" release="$3" tag
  tag="$(release_tag "$release")"
  STACK="$SMOKE_DIR/$name/deploy"
  PROJECT="${ORCH_SMOKE_PROJECT_PREFIX:-orchsmoke$SMOKE_RUN}$name"
  EDGE_PORT="$port"; GATEWAY_PORT=$((port + 1))
  EDGE="http://127.0.0.1:$EDGE_PORT"
  if docker ps -aq --filter "label=com.docker.compose.project=$PROJECT" | grep -q .; then abort "the project $PROJECT has containers still"; fi
  rm -rf "${SMOKE_DIR:?}/$name"
  mkdir -p "$SMOKE_DIR/$name"
  tar -xzf "$release/orch-deploy-$tag.tar.gz" -C "$SMOKE_DIR/$name"
  cp "$STACK/.env.example" "$STACK/.env"
  {
    printf 'POSTGRES_PASSWORD=%s\n' "$(openssl rand -base64 24 | tr '+/' '-_')"
    printf 'ORCH_FRONTEND_BIND=127.0.0.1:%s\n' "$EDGE_PORT"
    printf 'ORCH_GATEWAY_PUBLISH=%s:%s:9443\n' "$(bridge_address)" "$GATEWAY_PORT"
    printf 'COMPOSE_PROJECT_NAME=%s\n' "$PROJECT"
  } >> "$STACK/.env"
  # The edge of this stack is its own web edge, without a front: the public
  # address is its own, and the advertised public endpoint is the web edge's
  # service name inside the stack, which is where the checks run from.
  sed -i \
    -e "s|^public_url = .*|public_url = \"$EDGE\"|" \
    -e 's|^url = "wss://orch.example.com/link"|url = "ws://frontend/link"|' \
    "$STACK/config/orch.toml"
}

# update_sh ARGS...: update.sh of the stack.
# Managed scenarios use the edge's Docker name for connector downloads. The
# updater still checks the actual published edge from the host.
update_sh() { (cd "$STACK" && ORCH_READY_URL="${ORCH_READY_URL:-$EDGE}" ./update.sh "$@"); }

# compose ARGS...: docker compose in the stack.
compose() { (cd "$STACK" && docker compose "$@"); }

# doctor ARGS...: orchctl doctor in the API's container.
doctor() { compose exec -T api /opt/orchctl/orchctl doctor "$@"; }

# node_online: the bundled node has a session. It enrolls on its first start after the
# administrator exists, which takes up to a minute or two; a system that is used has long had it.
node_online() { doctor --only node | grep -q '^ok'; }

# doctor_fails_on ID...: orchctl doctor fails and names each of the checks.
doctor_fails_on() {
  local output id
  output="$(doctor 2>&1 || true)"
  for id in "$@"; do printf '%s\n' "$output" | grep -q "^FAIL  $id " || return 1; done
}

# ready TAG: every service is up and reports the release.
ready() { doctor --expect-version "${1#v}"; }

# finish: takes the stack down, removes it, and exits by what failed.
finish() {
  stop_site
  if [ "${ORCH_SMOKE_KEEP:-0}" = 1 ]; then
    say "kept: $STACK (docker compose -p $PROJECT; down -v and remove the directory to clean up)"
  else
    compose down -v > /dev/null 2>&1 || true
    rm -rf "$(dirname "$STACK")"
    # The run's own directory goes with its last stack; another run's is never touched.
    rmdir "$SMOKE_DIR" 2> /dev/null || true
  fi
  printf '\n%s: %d checks, %d failed\n' "$(basename "$0")" "$CHECKED" "$FAILED"
  [ "$FAILED" -eq 0 ]
}

# --- Through the web edge -----------------------------------------------------------

# edge_get PATH [CURL ARGS...]: the body of GET PATH through the web edge.
edge_get() { local path="$1"; shift; curl -fsS "$@" "$EDGE$path"; }

# edge_version: the release the API says it is, through the web edge.
edge_version() { edge_get /api/v1/meta | sed -n 's/.*"version":"\([^"]*\)".*/\1/p'; }

# json_field NAME: the value of a string member of the JSON on stdin.
json_field() { sed -n "s/.*\"$1\":\"\\([^\"]*\\)\".*/\\1/p" | head -n 1; }

ADMIN_NAME=ada
ADMIN_PASSWORD='correct horse battery'

# setup_admin: makes the first administrator through the edge; sets TOKEN.
setup_admin() {
  local reply
  reply="$(curl -fsS -H 'content-type: application/json' -X POST "$EDGE/api/v1/setup" \
    -d "{\"username\":\"$ADMIN_NAME\",\"display_name\":\"Ada\",\"password\":\"$ADMIN_PASSWORD\"}")" || return 1
  TOKEN="$(printf '%s' "$reply" | json_field access_token)"
  [ -n "$TOKEN" ]
}

# sign_in: signs the administrator in through the edge; sets TOKEN.
sign_in() {
  local reply
  reply="$(curl -fsS -H 'content-type: application/json' -X POST "$EDGE/api/v1/auth/login" \
    -d "{\"login\":\"$ADMIN_NAME\",\"password\":\"$ADMIN_PASSWORD\"}")" || return 1
  TOKEN="$(printf '%s' "$reply" | json_field access_token)"
  [ -n "$TOKEN" ]
}

# sql QUERY: one value from the stack's database.
sql() { compose exec -T postgres psql -U orch -d orch -At -c "$1"; }

# wait_for SECONDS WHAT COMMAND...: the command succeeds within the time.
wait_for() {
  local seconds="$1" what="$2" deadline; shift 2
  deadline=$((SECONDS + seconds))
  until "$@" > /dev/null 2>&1; do
    if [ "$SECONDS" -ge "$deadline" ]; then fail "$what (not within $seconds s)"; return 1; fi
    sleep 3
  done
  pass "$what"
}

# link_target KIND NAME: the release a payload link points to, or nothing.
link_target() { local target; target="$(readlink "$STACK/runtime/$1/$2" 2> /dev/null || true)"; printf '%s' "${target##*/}"; }

# dumps: the dumps of the stack, newest last.
dumps() { find "$STACK/runtime/update-backups" -name "*.dump" 2> /dev/null | sort; }

# the five headers the web edge puts on every response
has_security_headers() {
  local headers
  headers="$(curl -sS -D - -o /dev/null "$EDGE$1")"
  for name in content-security-policy x-content-type-options x-frame-options referrer-policy permissions-policy; do
    printf '%s' "$headers" | grep -qi "^$name:" || return 1
  done
}

# started_at: when each container of the stack was started, to see that nothing was.
started_at() {
  docker ps -aq --filter "label=com.docker.compose.project=$PROJECT" \
    | xargs docker inspect --format '{{.Name}} {{.State.StartedAt}}' | sort
}

# --- A release site --------------------------------------------------------------------

SITE_PID=""

# serve_site NAME PORT RELEASE...: a web server on loopback that lays releases out as a
# release site does: <base>/download/<tag>/version.json and the files beside it, and
# <base>/latest/download/ for the newest of those given last. The addresses in the
# manifests are bare names, which are read against the address of the manifest.
serve_site() {
  local name="$1" port="$2" release tag site
  shift 2
  site="$SMOKE_DIR/$name/site"
  rm -rf "${site:?}"; mkdir -p "$site/download" "$site/latest"
  for release in "$@"; do
    tag="$(release_tag "$release")"
    cp -al "$release" "$site/download/$tag"
    rm -f "$site/download/$tag/version.json"
    sed -E 's|"download_url": "[^"]*/([^/"]+)"|"download_url": "\1"|' "$release/version.json" > "$site/download/$tag/version.json"
    rm -f "$site/latest/download"; ln -s "../download/$tag" "$site/latest/download"
  done
  python3 -m http.server "$port" --bind 127.0.0.1 --directory "$site" > "$SMOKE_DIR/$name/site.log" 2>&1 &
  SITE_PID=$!
  wait_for 20 "the release site answers" curl -fsS "http://127.0.0.1:$port/latest/download/version.json"
}

stop_site() { if [ -n "$SITE_PID" ]; then kill "$SITE_PID" 2> /dev/null || true; SITE_PID=""; fi; }
