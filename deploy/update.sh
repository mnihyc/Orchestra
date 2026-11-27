#!/usr/bin/env bash
# update.sh: first start, verified updates, backup, rollback and recovery of an
# Orch stack that runs from this directory.
#
#   ./update.sh first-start [latest|vX.Y.Z] [--from DIR] [--restore-database DUMP]
#   ./update.sh latest|vX.Y.Z [--from DIR] [--yes]
#   ./update.sh backup [--repair-record <relative-dump-name> [--paired-blobs <relative-copy-directory>]]
#   ./update.sh rollback [--yes]
#   ./update.sh recover
#   ./update.sh status
#   ./update.sh doctor
#   ./update.sh prune [--yes]
#   ./update.sh dumps remove <name> [--yes]
#   ./update.sh uninstall [--purge] [--no-backup] [--plan|--yes]
#
# Readiness checks the committed public address from this host as well as the
# services inside the stack. ORCH_READY_URL may select the reachable edge URL
# when this host uses a different route to the same deployment. `doctor` runs
# the same two checks once, at any time, and prints every line of both with the
# address the edge check used; it changes nothing and exits non-zero when
# either fails.
#
# A release is a directory of files with a version.json that lists them, each
# with its size and BLAKE3 hash. --from DIR takes the files from a directory (a
# host that cannot reach the release site); without it they are fetched from
# ORCH_RELEASE_URL. Either way nothing is installed before every file has been
# checked against the manifest.
#
# Three payloads are installed: server (the programs, the schema, the rate
# catalogue), frontend (the built application) and cli (orchctl). Each lives in
# runtime/<payload>/releases/<tag>-<hash>/, and runtime/<payload>/current and
# runtime/<payload>/previous are links to the releases that run and that ran.
# A link is replaced by one rename, so it is never half made. .env, config/ and
# existing secrets are the operator's and are never touched. A missing port
# assertion key is added to an older complete secret set. The password file's
# permissions are checked on every run.
#
# The other files of the release's orch-deploy bundle (compose.yml, nginx.conf,
# frontend.sh, postgres/Dockerfile and the rest of this directory) follow the
# release: an update writes them in its own transaction, after the dump and before
# the start, and a rollback or a recover puts them back. A file that equals no
# published version of it is an edit of the operator's: its set of files is kept and
# the shipped files are written beside as <name>.new, which `status` lists until
# they are gone. This script is replaced by the release's at the end of a
# successful update and is never rolled back.
#
# An update, a rollback and a first start are transactions. runtime/update/journal
# records each step before it is taken, so that `recover` can finish or undo
# what a power cut or a Ctrl-C left half done.

set -Eeuo pipefail

# --- Constants ---------------------------------------------------------------

# The services that are stopped for an update; the database is not.
APP_SERVICES="api gateway worker node frontend"
# The web edge keeps serving the waiting page and migration observations during an update.
STOP_SERVICES="api gateway worker node"
# The payloads, in the order they are installed.
PAYLOADS="server frontend cli"
# How long an update waits for the stack to be ready and to report the release
# it was given: a start applies the schema, which on a large database is the
# long part.
READY_TIMEOUT="${ORCH_READY_TIMEOUT:-300}"
# How long the services get to stop. The gateway drains for ten seconds and a
# worker finishes the delivery it is making.
STOP_TIMEOUT=60
# The user and group the services run as when root runs this and nobody else
# owns the directory: an unprivileged id that root can still give files to.
ROOT_SERVICE_ID=10001
# Where releases are fetched from when no directory is given: <base>/download/vX.Y.Z/
# and <base>/latest/download/ hold version.json and the files it lists.
RELEASE_URL="${ORCH_RELEASE_URL:-}"
# The secrets a complete set has (orchctl compose-secrets makes them).
SECRET_FILES="orch_ca_cert.pem orch_ca_key.pem orch_master_key orch_vapid_private_key
node_enrollment_token service_api_cert.pem service_api_key.pem service_gateway_cert.pem
service_gateway_key.pem service_worker_cert.pem service_worker_key.pem orch_ports_assertion_key"

DEPLOY_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd -P)"
cd "$DEPLOY_DIR"
JOURNAL="runtime/update/journal"
STATE="runtime/update/state"
BACKUPS="runtime/update-backups"
DATA_DIR="./runtime/data"
POSTGRES_DATA_DIR="./runtime/postgres/data"

# --- Messages ----------------------------------------------------------------

say() { printf '%s\n' "$*"; }
# A count with its noun in the right number: 1 environment, 2 environments.
noun() { if [ "$1" = 1 ]; then printf '%s %s' "$1" "$2"; else printf '%s %ss' "$1" "$2"; fi; }
note() { printf 'update.sh: %s\n' "$*" >&2; }
die() {
  printf 'update.sh: %s\n' "$*" >&2
  if [[ "${ORCH_UPDATE_JOB:-}" =~ ^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$ ]]; then update_job_exit; fi
  exit 1
}
stamp() { date -u +%Y%m%dT%H%M%SZ; }

need() { command -v "$1" > /dev/null 2>&1 || die "$1 is needed and is not installed"; }

# --- The host ------------------------------------------------------------------

check_host() {
  local tool
  for tool in docker awk sed grep tar flock stat mv ln readlink realpath sha256sum base64 mktemp sort cut head tr date id sleep fold wc du find cp chmod chown; do
    need "$tool"
  done
  mv --version 2> /dev/null | grep -q 'GNU coreutils' || die "GNU coreutils are needed (mv -T replaces a link in one step)"
  docker compose version > /dev/null 2>&1 || die "the Docker Compose plugin is needed (docker compose)"
  docker info > /dev/null 2>&1 || die "Docker does not answer; is it running, and are you allowed to use it?"
}

compose() {
  local https_bind
  https_bind="${ORCH_FRONTEND_HTTPS_BIND:-$(env_get ORCH_FRONTEND_HTTPS_BIND)}"
  ORCH_FRONTEND_HTTPS_BIND="$https_bind" docker compose "$@"
}

# The image a compose.yml service is made from, found by the start of its name.
image_of() { sed -n "s/^[[:space:]]*image:[[:space:]]*\\($1[^[:space:]]*\\).*/\\1/p" compose.yml | head -n 1; }

# A carried-in image is selected by its verified content ID; Docker save/load need not retain registry aliases.
local_image() {
  if [ -n "${ORCH_OFFLINE_IMAGE_MAP:-}" ]; then
    python3 -I -c 'import json,sys; print(json.load(open(sys.argv[1]))[sys.argv[2]])' "$ORCH_OFFLINE_IMAGE_MAP" "$1" \
      || die "no verified carried-in image for $1"
  else
    printf '%s\n' "$1"
  fi
}

# --- .env -----------------------------------------------------------------------

# private_env: protect the password before any value is read or Compose runs.
private_env() {
  local mode
  [ -f .env ] || return 0
  mode="$(stat -Lc '%a' .env)" || die "cannot inspect .env; run chmod 600 -- .env as its owner"
  if [ "$mode" != 600 ]; then
    chmod 600 -- .env 2> /dev/null || die ".env is not private (mode $mode); run chmod 600 -- .env as its owner before starting"
    [ "$(stat -Lc '%a' .env)" = 600 ] || die ".env is not private; run chmod 600 -- .env as its owner before starting"
    say "set .env mode to 0600; only its owner can read the database password"
  fi
}

# env_get NAME: the value NAME has in .env (the last definition), without quotes.
env_get() {
  [ -f .env ] || return 0
  awk -v key="$1" '
    /^[[:space:]]*#/ || !/=/ { next }
    { name = $0; sub(/=.*/, "", name); gsub(/[[:space:]]/, "", name)
      if (name == key) { value = $0; sub(/^[^=]*=/, "", value) } }
    END { print value }' .env |
    sed -e 's/^[[:space:]]*//' -e 's/[[:space:]]*$//' -e 's/^"\(.*\)"$/\1/' -e "s/^'\\(.*\\)'\$/\\1/"
}

# check_password: the database password is strong enough to be used.
check_password() {
  local password lower distinct
  password="$(env_get POSTGRES_PASSWORD)"
  [ -n "$password" ] || die "POSTGRES_PASSWORD is not set in .env (see .env.example)"
  [ "${#password}" -ge 24 ] || die "POSTGRES_PASSWORD has ${#password} characters; at least 24 are needed"
  [[ "$password" =~ ^[A-Za-z0-9_-]+$ ]] || die "POSTGRES_PASSWORD may hold only letters, digits, - and _ (it is written into a URL)"
  lower="$(printf '%s' "$password" | tr '[:upper:]' '[:lower:]')"
  case "$lower" in
    *change* | *password* | *example* | *placeholder* | *secret* | *replace* | *your*)
      die "POSTGRES_PASSWORD looks like a placeholder; make a real one (openssl rand -base64 24 | tr '+/' '-_')" ;;
  esac
  distinct="$(printf '%s' "$password" | fold -w1 | sort -u | wc -l)"
  [ "$distinct" -ge 8 ] || die "POSTGRES_PASSWORD repeats too few characters to be a password"
}

# machine_facts: the user and group the services run as, the Docker group and
# this directory, written to .env once, where Compose reads them.
machine_facts() {
  local uid gid owner socket docker_gid block=""
  uid="$(id -u)"; gid="$(id -g)"
  if [ "$uid" -eq 0 ]; then
    owner="$(stat -c '%u:%g' "$DEPLOY_DIR")"
    if [ "${owner%%:*}" -ne 0 ]; then uid="${owner%%:*}"; gid="${owner##*:}"; else uid="$ROOT_SERVICE_ID"; gid="$ROOT_SERVICE_ID"; fi
  fi
  socket="${DOCKER_SOCKET:-$(env_get DOCKER_SOCKET)}"; socket="${socket:-/var/run/docker.sock}"
  [ -S "$socket" ] || die "no Docker socket at $socket; the node needs it (set DOCKER_SOCKET if it is elsewhere)"
  docker_gid="$(stat -c '%g' "$socket")"
  [ -n "$(env_get ORCH_HOME)" ] || block+="ORCH_HOME=$DEPLOY_DIR"$'\n'
  [ -n "$(env_get ORCH_UID)" ] || block+="ORCH_UID=$uid"$'\n'
  [ -n "$(env_get ORCH_GID)" ] || block+="ORCH_GID=$gid"$'\n'
  [ -n "$(env_get ORCH_DOCKER_GID)" ] || block+="ORCH_DOCKER_GID=$docker_gid"$'\n'
  # shellcheck disable=SC2016 # Compose expands the optional file when it reads .env.
  [ -n "$(env_get COMPOSE_FILE)" ] || block+='COMPOSE_FILE=compose.yml${ORCH_FRONTEND_HTTPS_BIND:+:compose.frontend-https.yml}'$'\n'
  if [ -n "$block" ]; then
    { printf '\n# Written by update.sh first-start: the facts of this machine.\n'; printf '%s' "$block"; } >> .env
    say "wrote the facts of this machine to .env:"; printf '%s' "$block" | sed 's/^/  /'
  fi
}

# load_facts: reads the machine facts and refuses a directory that moved.
load_facts() {
  ORCH_HOME="$(env_get ORCH_HOME)"; SERVICE_UID="$(env_get ORCH_UID)"; SERVICE_GID="$(env_get ORCH_GID)"
  if [ -z "$ORCH_HOME" ] || [ -z "$SERVICE_UID" ] || [ -z "$SERVICE_GID" ] || [ -z "$(env_get ORCH_DOCKER_GID)" ]; then
    die ".env lacks the facts of this machine (ORCH_HOME, ORCH_UID, ORCH_GID, ORCH_DOCKER_GID); ./update.sh first-start writes them"
  fi
  [ "$ORCH_HOME" = "$DEPLOY_DIR" ] ||
    die "ORCH_HOME in .env is $ORCH_HOME and this directory is $DEPLOY_DIR; it was moved. Set ORCH_HOME to the new place, and check that the Docker host sees the same path"
  if [ "$(id -u)" -ne 0 ] && [ "$SERVICE_UID" != "$(id -u)" ]; then
    die "ORCH_UID is $SERVICE_UID, but this user is $(id -u); run the updater as that user or as root so the services can write their directories"
  fi
  # Ask Compose for its interpolation inputs so quoting, .env references and shell overrides agree.
  local dirs name value
  dirs="$(compose config --environment | sed -n '/^ORCH_DATA_DIR=/p; /^ORCH_POSTGRES_DATA_DIR=/p')" ||
    die "Compose could not resolve the deployment data directories"
  while IFS='=' read -r name value; do
    case "$name" in
      ORCH_DATA_DIR) DATA_DIR="${value:-./runtime/data}" ;;
      ORCH_POSTGRES_DATA_DIR) POSTGRES_DATA_DIR="${value:-./runtime/postgres/data}" ;;
    esac
  done <<< "$dirs"
  for value in "$DATA_DIR" "$POSTGRES_DATA_DIR"; do
    case "$value" in /*|./*|../*) ;; *) die "data directories must be absolute paths or start with ./ or ../" ;; esac
  done
  DATA_DIR="$(realpath -m -- "$DATA_DIR")"
  POSTGRES_DATA_DIR="$(realpath -m -- "$POSTGRES_DATA_DIR")"
  case "$DEPLOY_DIR/" in "$DATA_DIR/"*|"$POSTGRES_DATA_DIR/"*) die "use dedicated data directories outside or below the deployment directory" ;; esac
  if [ "$DATA_DIR" = / ] || [ "$POSTGRES_DATA_DIR" = / ] || [[ "$DATA_DIR/" == "$POSTGRES_DATA_DIR/"* || "$POSTGRES_DATA_DIR/" == "$DATA_DIR/"* ]]; then
    die "server files and PostgreSQL need separate dedicated directories"
  fi
}

# --- Directories -----------------------------------------------------------------

# make_dir MODE PATH: a directory the services write in, owned by their user.
make_dir() {
  mkdir -p "$2"
  [ "$(id -u)" -ne 0 ] || chown "$SERVICE_UID:$SERVICE_GID" "$2"
  chmod "$1" "$2"
}

# A stable bind-mounted inode keeps purge and a new node run mutually exclusive.
ensure_node_lock() {
  local path="$DEPLOY_DIR/runtime/node.orch-instance-lock"
  if [ -L "$path" ]; then die "the node instance lock is not a real file"; fi
  if [ -e "$path" ] && [ ! -f "$path" ]; then die "the node instance lock is not a real file"; fi
  (umask 077; : >> "$path")
  [ "$(id -u)" -ne 0 ] || chown "$SERVICE_UID:$SERVICE_GID" "$path"
  chmod 600 "$path"
}

# First start grants the bundled node's shipped facilities in its single machine policy.
seed_node_policy() {
  local path=runtime/node/config.toml initial=runtime/node/.config.toml.initial
  if [ -e "$path" ] || [ -L "$path" ]; then return 0; fi
  (umask 077; cat config/node.toml > "$initial") || die "the bundled node policy could not be prepared"
  chmod 600 "$initial"
  [ "$(id -u)" -ne 0 ] || chown "$SERVICE_UID:$SERVICE_GID" "$initial"
  mv -- "$initial" "$path"
}

# records_mount_point [replace]: the directory in the server's data directory that the API's container mounts the
# installation records (runtime/update) on. Docker makes a missing mount point itself, owned by root, where the
# operator cannot change it; made here first, it is the services' user's. One an earlier start left to root is given
# back when root runs this. Otherwise it is empty and in a directory of ours: `replace` makes it again, and only
# app_start asks for that, as the API that has it mounted is recreated right after.
records_mount_point() {
  local path="$DATA_DIR/installation"
  if [ -d "$path" ] && [ "$(id -u)" -ne 0 ] && [ "$(stat -c %u -- "$path")" != "$SERVICE_UID" ]; then
    [ "${1:-}" = replace ] && rmdir -- "$path" 2> /dev/null || return 0
  fi
  make_dir 755 "$path"
}

prepare_dirs() {
  local kind
  private_env
  make_dir 700 runtime
  make_dir 700 runtime/postgres
  make_dir 700 "$POSTGRES_DATA_DIR"
  make_dir 750 "$DATA_DIR"
  make_dir 755 "$DATA_DIR/update-status"
  make_dir 750 "$DATA_DIR/frontend-tls"
  records_mount_point
  make_dir 700 runtime/node
  ensure_node_lock
  make_dir 700 runtime/update
  make_dir 755 config/ca-certificates
  make_dir 700 "$BACKUPS"
  for kind in $PAYLOADS; do make_dir 755 "runtime/$kind"; make_dir 755 "runtime/$kind/releases"; done
  # initdb wants the account it runs as to have a name.
  printf 'root:x:0:0:root:/root:/bin/sh\npostgres:x:%s:%s:PostgreSQL:/var/lib/postgresql:/bin/sh\n' \
    "$SERVICE_UID" "$SERVICE_GID" > runtime/postgres/passwd
  chmod 644 runtime/postgres/passwd
  # The secrets are read by their owner and by nobody else.
  if [ -d config/secrets ]; then [ "$(id -u)" -ne 0 ] || chown "$SERVICE_UID:$SERVICE_GID" config/secrets; chmod 700 config/secrets; fi
  # A staging directory that an interrupted run left is of no use to anyone.
  for kind in $PAYLOADS; do
    local leftover
    for leftover in "$DEPLOY_DIR/runtime/$kind/releases"/.staging-*; do
      if [ -e "$leftover" ]; then rm -rf -- "$leftover"; fi
    done
  done
}

# check_installation: every file Compose binds exists. Docker would otherwise
# make a directory of the same name, owned by root, in its place.
check_installation() {
  local file kind
  for file in compose.yml nginx.conf .env config/orch.toml runtime/postgres/passwd; do
    [ -f "$file" ] || die "$file is missing"
  done
  for file in $SECRET_FILES; do [ -f "config/secrets/$file" ] || die "config/secrets/$file is missing"; done
  for kind in $PAYLOADS; do [ -d "runtime/$kind/current/" ] || die "runtime/$kind/current is not a release; ./update.sh first-start installs one"; done
}

# --- The manifest ------------------------------------------------------------------

# flatten_json: JSON on stdin as `path<TAB>value` lines, one for each scalar.
flatten_json() {
  grep -oE '"([^"\\]|\\.)*"|-?[0-9]+(\.[0-9]+)?([eE][+-]?[0-9]+)?|true|false|null|[][{},:]' |
    awk '
      function unquote(t,   r) { r = substr(t, 2, length(t) - 2); gsub(/\\\//, "/", r); gsub(/\\"/, "\"", r); gsub(/\\\\/, "\\", r); return r }
      function emit(v,   i, p) { p = ""; for (i = 1; i <= d; i++) p = p (i > 1 ? "." : "") key[i]; print p "\t" v }
      function slot() { if (kind[d] == "a") { key[d] = idx[d]; idx[d]++ } }
      {
        t = $0
        if (t == "{") { slot(); d++; kind[d] = "o"; want[d] = "key"; key[d] = "" }
        else if (t == "[") { slot(); d++; kind[d] = "a"; idx[d] = 0; key[d] = "" }
        else if (t == "}" || t == "]") { d-- }
        else if (t == ",") { if (kind[d] == "o") want[d] = "key" }
        else if (t == ":") { want[d] = "value" }
        else if (substr(t, 1, 1) == "\"") {
          if (kind[d] == "o" && want[d] == "key") key[d] = unquote(t)
          else { slot(); emit(unquote(t)) }
        } else { slot(); emit(t) }
      }'
}

# mget PATH: a scalar of the manifest read by load_manifest.
mget() { awk -F'\t' -v key="$1" '$1 == key { print $2; exit }' "$MANIFEST_FLAT"; }

# blake3_of FILE: the hash of the file, with b3sum where there is one and with
# the release's own orchctl where there is not.
blake3_of() {
  if [ -n "${B3SUM:-}" ]; then "$B3SUM" --no-names "$1"; else "$HASHER" checksum "$1" | cut -d' ' -f1; fi
}

# release_arch: the architecture the assets of this host are named for.
release_arch() {
  case "$(uname -m)" in
    x86_64 | amd64) echo x86_64 ;;
    aarch64 | arm64) echo aarch64 ;;
    *) die "no release is made for $(uname -m)" ;;
  esac
}

# load_manifest DIR: reads DIR/version.json into MANIFEST_FLAT and sets TAG,
# VERSION, CONNECTOR_MINIMUM and the names of the three assets this host installs.
load_manifest() {
  local dir=$1 arch
  [ -f "$dir/version.json" ] || die "$dir/version.json is not there"
  MANIFEST_FLAT="$WORK/manifest.flat"
  flatten_json < "$dir/version.json" > "$MANIFEST_FLAT"
  [ "$(mget schema_version)" = 1 ] || die "version.json has schema_version $(mget schema_version), and this reads 1"
  [ "$(mget project)" = orch ] || die "version.json is not a release of Orch"
  TAG="$(mget tag)"; VERSION="$(mget version)"; CONNECTOR_MINIMUM="$(mget connector_minimum)"
  [[ "$TAG" =~ ^v[0-9]+\.[0-9]+\.[0-9]+(-[0-9A-Za-z.-]+)?$ ]] || die "the release tag $TAG is not vMAJOR.MINOR.PATCH"
  [ "$TAG" = "v$VERSION" ] || die "version.json says tag $TAG and version $VERSION"
  [[ "$CONNECTOR_MINIMUM" =~ ^[0-9]+\.[0-9]+\.[0-9]+(-[0-9A-Za-z.-]+)?$ ]] ||
    die "connector_minimum in version.json is not a release version"
  arch="$(release_arch)"
  # shellcheck disable=SC2034 # read as ASSET_<kind> through an indirect expansion
  ASSET_server="orch-server-linux-$arch.zip"
  # shellcheck disable=SC2034
  ASSET_frontend="orch-frontend-dist.tar.gz"
  # shellcheck disable=SC2034
  ASSET_cli="orchctl-linux-$arch-musl"
}

# asset_field NAME FIELD: a field of the asset called NAME in the manifest.
asset_field() {
  local index=0 name
  while name="$(mget "assets.$index.name")" && [ -n "$name" ]; do
    if [ "$name" = "$1" ]; then mget "assets.$index.$2"; return 0; fi
    index=$((index + 1))
  done
  return 1
}

# verify_release DIR: every file of the release that is in DIR is what the
# manifest says, and the three this host installs are there.
verify_release() {
  local dir=$1 index=0 name size hash path kind asset
  # The tool that hashes: b3sum, or the release's orchctl (checked for its
  # size first, since it is run before it has been checked for its hash).
  B3SUM="$(command -v b3sum || true)"
  if [ -z "$B3SUM" ]; then
    asset="$ASSET_cli"
    [ -f "$dir/$asset" ] || die "$dir/$asset is not there, and without b3sum it is what hashes the release"
    size="$(asset_field "$asset" size)" || die "version.json does not list $asset"
    [ "$(stat -c %s "$dir/$asset")" = "$size" ] || die "$asset is not the size version.json lists"
    HASHER="$WORK/orchctl"
    cp "$dir/$asset" "$HASHER"; chmod 755 "$HASHER"
  fi
  for kind in $PAYLOADS; do
    asset="ASSET_$kind"; asset="${!asset}"
    [ -f "$dir/$asset" ] || die "$dir/$asset is not there"
    asset_field "$asset" size > /dev/null || die "version.json does not list $asset"
  done
  while name="$(mget "assets.$index.name")" && [ -n "$name" ]; do
    index=$((index + 1))
    case "$name" in */* | .* | "") die "version.json lists $name, which is not a file name" ;; esac
    path="$dir/$name"
    [ -f "$path" ] || continue
    size="$(mget "assets.$((index - 1)).size")"; hash="$(mget "assets.$((index - 1)).blake3")"
    [ "$(stat -c %s "$path")" = "$size" ] || die "$name is $(stat -c %s "$path") bytes and version.json lists $size"
    [ "$(blake3_of "$path")" = "$hash" ] || die "$name does not have the BLAKE3 hash version.json lists; it was damaged or changed"
    say "  verified $name ($size bytes)"
  done
}

# --- Getting a release ----------------------------------------------------------------

# fetch_release SELECTOR: puts a release in RELEASE_DIR, verified. SELECTOR is
# latest, a tag, or empty when --from gave the directory.
fetch_release() {
  local selector=$1 base manifest_url index name url path kind asset
  if [ -n "${FROM:-}" ]; then
    [ -d "$FROM" ] || die "--from $FROM is not a directory"
    RELEASE_DIR="$(cd "$FROM" && pwd -P)"
    load_manifest "$RELEASE_DIR"
    if [ -n "$selector" ] && [ "$selector" != latest ] && [ "$selector" != "$TAG" ]; then
      die "$RELEASE_DIR holds release $TAG and $selector was asked for"
    fi
  else
    need curl
    [ -n "$RELEASE_URL" ] || die "no release site is set: give --from DIR, or set ORCH_RELEASE_URL (in .env or the environment)"
    base="${RELEASE_URL%/}"
    if [ "$selector" = latest ] || [ -z "$selector" ]; then manifest_url="$base/latest/download/version.json"; else manifest_url="$base/download/$selector/version.json"; fi
    RELEASE_DIR="$WORK/download"
    mkdir -p "$RELEASE_DIR"
    say "fetching $manifest_url"
    curl -fsSL --connect-timeout 30 --speed-time 30 --speed-limit 1 --retry 3 -o "$RELEASE_DIR/version.json" "$manifest_url" || die "cannot fetch $manifest_url"
    load_manifest "$RELEASE_DIR"
    for kind in $PAYLOADS; do
      asset="ASSET_$kind"; asset="${!asset}"
      url="$(asset_field "$asset" download_url)" || die "version.json does not list $asset"
      case "$url" in http://* | https://*) ;; *) url="${manifest_url%/*}/$url" ;; esac
      say "fetching $asset"
      curl -fSL --connect-timeout 30 --speed-time 30 --speed-limit 1 --retry 3 --continue-at - -o "$RELEASE_DIR/$asset" "$url" || die "cannot fetch $url"
    done
    # The bundle of the deployment files is fetched when an update wants it and the release lists it.
    asset="orch-deploy-$TAG.tar.gz"
    if [ "${WANT_BUNDLE:-0}" = 1 ] && url="$(asset_field "$asset" download_url)"; then
      case "$url" in http://* | https://*) ;; *) url="${manifest_url%/*}/$url" ;; esac
      say "fetching $asset"
      curl -fSL --connect-timeout 30 --speed-time 30 --speed-limit 1 --retry 3 --continue-at - -o "$RELEASE_DIR/$asset" "$url" || die "cannot fetch $url"
    fi
  fi
  say "checking release $TAG against version.json"
  verify_release "$RELEASE_DIR"
  # The bundle is the release's when the manifest lists it and it is here, and it has been checked above.
  BUNDLE=""
  if [ -f "$RELEASE_DIR/orch-deploy-$TAG.tar.gz" ] && asset_field "orch-deploy-$TAG.tar.gz" size > /dev/null; then BUNDLE="$RELEASE_DIR/orch-deploy-$TAG.tar.gz"; fi
}

# --- Unpacking -------------------------------------------------------------------------

# toolbox ARGS...: runs a command of the web edge's image (busybox) as the
# services' user, with the release and the staging directory mounted.
toolbox() {
  docker run --rm --user "$SERVICE_UID:$SERVICE_GID" --network none --entrypoint "$1" \
    -v "$TOOLBOX_INPUT:/in:ro" -v "$STAGE_PARENT:/out" "$TOOLBOX_IMAGE" "${@:2}"
}

# unpack KIND: puts the release's payload of KIND in releases/<tag>-<hash>/ and
# sets UNPACKED to that directory's name. A payload that is already there, whole,
# is not unpacked again.
unpack() {
  local kind=$1 asset hash name stage
  asset="ASSET_$kind"; asset="${!asset}"
  hash="$(asset_field "$asset" blake3)"
  name="$TAG-${hash:0:12}"
  UNPACKED="$name"
  STAGE_PARENT="$DEPLOY_DIR/runtime/$kind/releases"
  if [ -f "$STAGE_PARENT/$name/.complete" ]; then say "  $kind: $name is there"; return 0; fi
  stage=".staging-$$-$kind"
  mkdir -p "$STAGE_PARENT/$stage"
  case "$kind" in
    server)
      # ORCH_UPDATE_NO_UNZIP=1 takes the container path on a host that has unzip (to try it).
      if [ -z "${ORCH_UPDATE_NO_UNZIP:-}" ] && command -v unzip > /dev/null 2>&1; then
        unzip -q -o "$RELEASE_DIR/$asset" -d "$STAGE_PARENT/$stage"
      else
        # A root-run updater creates the stage as root; the container unpacks as the service user.
        [ "$(id -u)" -ne 0 ] || chown "$SERVICE_UID:$SERVICE_GID" "$STAGE_PARENT/$stage"
        # The original release directory may be private to the operator. Mount a private copy
        # the service user can read instead of requiring access to that directory.
        TOOLBOX_INPUT="$WORK/server-archive"
        mkdir -p "$TOOLBOX_INPUT"
        cp "$RELEASE_DIR/$asset" "$TOOLBOX_INPUT/$asset"
        [ "$(id -u)" -ne 0 ] || chown -R "$SERVICE_UID:$SERVICE_GID" "$TOOLBOX_INPUT"
        chmod 700 "$TOOLBOX_INPUT"
        chmod 600 "$TOOLBOX_INPUT/$asset"
        toolbox unzip -q "/in/$asset" -d "/out/$stage"
      fi
      chmod 755 "$STAGE_PARENT/$stage/bin/"*
      chmod 755 "$STAGE_PARENT/$stage/connectors/"orch-connector-* "$STAGE_PARENT/$stage/connectors/"*.sh 2> /dev/null || true
      ;;
    frontend)
      tar -xzf "$RELEASE_DIR/$asset" -C "$STAGE_PARENT/$stage" --no-same-owner ;;
    cli)
      cp "$RELEASE_DIR/$asset" "$STAGE_PARENT/$stage/orchctl"; chmod 755 "$STAGE_PARENT/$stage/orchctl" ;;
  esac
  [ "$(id -u)" -ne 0 ] || chown -R "$SERVICE_UID:$SERVICE_GID" "$STAGE_PARENT/$stage"
  find "$STAGE_PARENT/$stage" -type d -exec chmod 755 {} +
  printf '%s\n' "$hash" > "$STAGE_PARENT/$stage/.complete"
  if [ -e "$STAGE_PARENT/$name" ]; then rm -rf -- "$STAGE_PARENT/${name:?}"; fi
  mv -T "$STAGE_PARENT/$stage" "$STAGE_PARENT/$name"
  say "  $kind: unpacked $name"
}

# ensure_images: the images that the release names are on this Docker host, and
# are pulled if they are not. A release made on this host has them already; a
# published one names a registry. A failure to pull is said and is no reason to
# stop: the services do not need them, only the environments made from them.
ensure_images() {
  local index=0 reference digest present digest_image selected
  while reference="$(mget "images.$index.reference")" && [ -n "$reference" ]; do
    digest="$(mget "images.$index.digest")"
    index=$((index + 1))
    if [ -n "${ORCH_OFFLINE_IMAGE_MAP:-}" ]; then
      selected="$(local_image "$reference")"
      docker image inspect "$selected" > /dev/null 2>&1 || die "carried-in image $reference is missing; load it before running the updater"
      say "  verified local image $reference ($selected)"
      continue
    fi
    if ! docker image inspect "$reference" > /dev/null 2>&1; then
      say "  pulling $reference"
      docker pull --quiet "$reference" > /dev/null ||
        note "could not pull $reference; environments made from it wait until it is on this host (docker pull, or docker load)"
    fi
    present="$(docker image inspect --format '{{.Id}}' "$reference" 2> /dev/null || true)"
    if [ -n "$present" ] && [ -n "$digest" ] && [ "$present" != "$digest" ]; then
      # A registry digest identifies a manifest or index, while .Id is the image configuration's digest.
      # An already digest-qualified reference was looked up by that digest above. For a tag, resolve
      # the recorded digest separately and compare the image IDs that Docker would actually run.
      if [[ "$reference" == *"@$digest" ]]; then continue; fi
      if [[ "$reference" != *@* ]]; then
        digest_image="$(docker image inspect --format '{{.Id}}' "$reference@$digest" 2> /dev/null || true)"
        [ "$digest_image" = "$present" ] && continue
      fi
      note "$reference on this host is $present, and the release names $digest"
    fi
  done
}

# --- The links -----------------------------------------------------------------------

# link_get KIND NAME: the release a link points at (its directory name), or nothing.
link_get() { local target; target="$(readlink "runtime/$1/$2" 2> /dev/null || true)"; printf '%s' "${target##*/}"; }

# link_set KIND NAME RELEASE: points the link at the release in one step; an
# empty RELEASE removes the link.
link_set() {
  local path="runtime/$1/$2"
  if [ -z "$3" ]; then rm -f -- "$path"; return 0; fi
  [ -d "runtime/$1/releases/$3" ] || die "runtime/$1/releases/$3 is not there to point $2 at"
  ln -sfn "releases/$3" "$path.new"
  mv -T "$path.new" "$path"
}

# A bundled-node job records only its fixed phases beside that node's state. It never
# publishes native output, configuration or credentials. The shell path has no job.
update_job_state() {
  [ -n "${ORCH_UPDATE_JOB:-}" ] || return 0
  [[ "$ORCH_UPDATE_JOB" =~ ^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$ ]] || die "the update job identity is invalid"
  local directory="runtime/node/server-updates/$ORCH_UPDATE_JOB" stage="$1" reason="${2:-}"
  mkdir -p "$directory"
  { printf 'stage=%s\nreason=%s\nfrom=%s\nto=%s\n' "$stage" "$reason" "${J[from_tag]:-${SERVER_UPDATE_FROM:-}}" "${J[to_tag]:-${SERVER_UPDATE_TARGET:-}}"; } > "$directory/status.new"
  chmod 640 "$directory/status.new"
  if [ "$(id -u)" = 0 ]; then chown "$(stat -c '%u:%g' runtime/node)" "$directory" "$directory/status.new"; fi
  sync "$directory/status.new"
  mv -T "$directory/status.new" "$directory/status"
  sync "$directory"
}

update_job_exit() {
  [ -n "${ORCH_UPDATE_JOB:-}" ] || return 0
  local status="runtime/node/server-updates/$ORCH_UPDATE_JOB/status" stage
  stage="$(sed -n 's/^stage=//p' "$status" 2> /dev/null || true)"
  case "$stage" in updated | rolled_back | failed) return 0 ;; esac
  if [ -f "$JOURNAL" ]; then update_job_state recovering recovery_required
  else update_job_state failed update_failed; fi
}

# --- The journal -----------------------------------------------------------------------

declare -A J=()

journal_save() {
  local key tmp="$JOURNAL.new"
  : > "$tmp"
  for key in "${!J[@]}"; do printf '%s=%s\n' "$key" "${J[$key]}" >> "$tmp"; done
  sort -o "$tmp" "$tmp"
  sync "$tmp" 2> /dev/null || true
  mv -T "$tmp" "$JOURNAL"
}

journal_load() {
  local line
  J=()
  [ -f "$JOURNAL" ] || return 1
  while IFS= read -r line; do [ -z "$line" ] || J["${line%%=*}"]="${line#*=}"; done < "$JOURNAL"
}

# jset KEY VALUE...: sets and saves, so that a step is on disk before it is taken.
jset() {
  J["$1"]="${*:2}"
  journal_save
  if [ "$1" = step ]; then
    case "${*:2}" in
      begin | staged) update_job_state applying ;;
      stopping | dumping | activating | starting) update_job_state "${*:2}" ;;
      verifying) update_job_state checking ;;
      restoring | restarting | restoring_entered) update_job_state rolling_back health_check_failed ;;
    esac
  fi
  # For the smoke scenarios: stop dead right after a step has been recorded and
  # before it is taken, as a power cut would. Never set in a deployment.
  if [ "$1" = step ] && [ "${ORCH_UPDATE_CRASH_AT:-}" = "${*:2}" ]; then kill -KILL "$$"; fi
}
journal_clear() { J=(); rm -f -- "$JOURNAL"; }

# --- The state -------------------------------------------------------------------------

state_get() {
  if [ -f "$STATE" ]; then sed -n "s/^$1=//p" "$STATE" | head -n 1; fi
}

state_save() {
  { printf 'current_tag=%s\nprevious_tag=%s\nprevious_dump=%s\nupdated_at=%s\n' "$1" "$2" "$3" "$(stamp)"; } > "$STATE.new"
  mv -T "$STATE.new" "$STATE"
}

# --- The deployment files --------------------------------------------------------------

# The files of the orch-deploy bundle that are the product's (everything under deploy/ but config/) follow the release:
# an update brings them in the same transaction as the payloads, a rollback and a recover put them back. What the
# bundle holds, and what each published release shipped of it, is its FILE_SETS: the generation of the files, the
# oldest generation the payloads of the release work with, the sets of files that must agree with each other, and the
# SHA-256 of every file of every published release. A file that equals a published one is the product's; one that
# equals none is the operator's edit, and it is kept: its set stays as it is and the shipped files are written beside
# as <name>.new. Nothing here touches .env, config/ or runtime/.
FILES_DIR="runtime/update/files"
FILES_STATE="runtime/update/files.state"
# The generation of the first release with a managed deployment file set.
FILES_BASELINE=1

sha_of() { sha256sum -- "$1" | cut -d' ' -f1; }

# fstate_get KEY / fstate_set KEY VALUE / fstate_del KEY: what the updater remembers of the files (files.state):
#   files_generation     the generation of the installed set, as the last update left it
#   files_previous       the transaction whose saved files a rollback puts back
#   kept.<file>          SHA-256:GENERATION of the shipped file that <file> was last kept against
#   reviewed.<file>      the generation an edited <file> is taken to have: that of the shipped file whose .new was read
fstate_get() {
  [ -f "$FILES_STATE" ] || return 0
  awk -F= -v key="$1" '$1 == key { sub(/^[^=]*=/, ""); print; exit }' "$FILES_STATE"
}
fstate_set() {
  local tmp="$FILES_STATE.new"
  mkdir -p runtime/update
  { [ ! -f "$FILES_STATE" ] || awk -F= -v key="$1" '$1 != key' "$FILES_STATE"; printf '%s=%s\n' "$1" "$2"; } > "$tmp"
  mv -T "$tmp" "$FILES_STATE"
}
fstate_del() {
  local tmp="$FILES_STATE.new"
  [ -f "$FILES_STATE" ] || return 0
  awk -F= -v key="$1" '$1 != key' "$FILES_STATE" > "$tmp"
  mv -T "$tmp" "$FILES_STATE"
}

# write_file SOURCE TARGET: TARGET becomes the content of SOURCE in one rename, with SOURCE's mode and, where root
# runs this, the owner of what it replaces (or of this directory).
write_file() {
  local source=$1 target=$2 tmp="$2.update-tmp"
  mkdir -p "$(dirname "$target")"
  cp -- "$source" "$tmp"
  chmod --reference="$source" "$tmp"
  if [ "$(id -u)" -eq 0 ]; then
    if [ -e "$target" ]; then chown --reference="$target" "$tmp"; else chown --reference="$(dirname "$target")" "$tmp"; fi
  fi
  mv -T "$tmp" "$target"
}

# fs_load FILE: reads the FILE_SETS of a bundle into FS_GENERATION, FS_MINIMUM, FS_SET_OF (the set of a file), FS_MEMBERS
# (the files of a set) and FS_ROWS, a file of "TAG GENERATION SHA256 PATH" lines.
fs_load() {
  local file=$1 kind first rest generation sha path format=""
  FS_GENERATION=""; FS_MINIMUM=""; FS_ROWS="$WORK/files.rows"; : > "$FS_ROWS"
  declare -gA FS_SET_OF=() FS_MEMBERS=()
  while read -r kind first rest; do
    case "$kind" in
      '' | '#'*) ;;
      format) format="$first" ;;
      generation) FS_GENERATION="$first" ;;
      minimum) FS_MINIMUM="$first" ;;
      set)
        for path in $rest; do FS_SET_OF[$path]="$first"; FS_MEMBERS[$first]+="$path "; done ;;
      file)
        read -r generation sha path <<< "$rest"
        [[ "$generation" =~ ^[0-9]+$ && "$sha" =~ ^[0-9a-f]{64}$ && -n "$path" ]] || die "FILE_SETS holds a line that cannot be read: $kind $first $rest"
        printf '%s %s %s %s\n' "$first" "$generation" "$sha" "$path" >> "$FS_ROWS" ;;
      *) die "FILE_SETS holds a line of a kind this updater does not know: $kind" ;;
    esac
  done < "$file"
  [ "$format" = 1 ] || die "FILE_SETS is of format ${format:-none}, and this updater reads format 1"
  [[ "$FS_GENERATION" =~ ^[0-9]+$ && "$FS_MINIMUM" =~ ^[0-9]+$ ]] || die "FILE_SETS has no generation and minimum"
  [ "$FS_MINIMUM" -le "$FS_GENERATION" ] || die "FILE_SETS asks for a minimum generation above its own"
}

# fs_gen_of PATH SHA: the newest generation in which a published release shipped PATH with that content; nothing when
# none did, which is what an edited file is.
fs_gen_of() {
  awk -v path="$1" -v sha="$2" '$4 == path && $3 == sha { found = 1; if ($2 + 0 > best + 0) best = $2 } END { if (found) print best + 0 }' "$FS_ROWS"
}

# fs_newest_tag PATH SHA: the newest release that shipped PATH with that content, or nothing.
fs_newest_tag() {
  awk -v path="$1" -v sha="$2" '$4 == path && $3 == sha { print $1 }' "$FS_ROWS" | sort -V | tail -n 1
}

# compose_sources FILE: the places of this directory a Compose file binds, and the project it names; a release that moves
# one of them would start the services on an empty directory.
compose_sources() {
  sed -n -E 's/^[[:space:]]*-[[:space:]]*"?([$][{]ORCH_(POSTGRES_)?DATA_DIR:-[^}]*[}][^:"]*):.*/\1/p' "$1" | sort -u
  sed -n -E 's/^[[:space:]]*-[[:space:]]*"?((\.\/(runtime|config)\/|\$\{ORCH_HOME\}\/runtime\/)[^:"]*).*/\1/p' "$1" | sort -u
  sed -n 's/^name:[[:space:]]*\([^[:space:]#]*\).*/name \1/p' "$1" | head -n 1
}

# compose_images FILE: the images a Compose file names.
compose_images() { sed -n 's/^[[:space:]]*image:[[:space:]]*\([^[:space:]#]*\).*/\1/p' "$1" | sort -u; }

# files_guard_postgres: the PostgreSQL major of the new postgres/Dockerfile is the data directory's. A new Dockerfile
# only takes effect at the next build, whenever that is, and a build of another major would not open this data directory.
files_guard_postgres() {
  local new_major old_major
  new_major="$(sed -n 's/^ARG POSTGRES_IMAGE=postgres:\([0-9][0-9]*\).*/\1/p' "$FILES_STAGE/deploy/postgres/Dockerfile" | head -n 1)"
  old_major="$(head -n 1 "$POSTGRES_DATA_DIR/PG_VERSION" 2> /dev/null || true)"
  if [ -z "$new_major" ] || [ -z "$old_major" ]; then
    GUARD_SAYS="the PostgreSQL major of the new postgres/Dockerfile (${new_major:-unreadable}) cannot be compared with the data directory's (${old_major:-unreadable}); compare them, then merge the .new files"
    return 1
  fi
  if [ "$new_major" != "$old_major" ]; then
    GUARD_SAYS="the new postgres/Dockerfile builds PostgreSQL $new_major and the data directory is PostgreSQL $old_major; this release needs a dump and restore into $new_major (see its notes), then merge the .new files"
    return 1
  fi
}

# files_guard_binds: the new compose.yml binds every directory of this installation that the one in use binds, and names the
# same project.
files_guard_binds() {
  local moved
  moved="$(comm -23 <(compose_sources compose.yml | sort -u) <(compose_sources "$FILES_STAGE/deploy/compose.yml" | sort -u) | tr '\n' ' ')"
  if [ -n "${moved// /}" ]; then
    GUARD_SAYS="the new compose.yml no longer binds ${moved% } (or names another project): see where this release keeps its data, then merge compose.yml.new"
    return 1
  fi
}

# files_guard_images: every image the new compose.yml names is on this host, pulled here if it is not, before anything is
# stopped; a host that cannot fetch it must have it loaded.
files_guard_images() {
  local reference selected
  while IFS= read -r reference; do
    [ -n "$reference" ] || continue
    if [ -n "${ORCH_OFFLINE_IMAGE_MAP:-}" ]; then
      selected="$(local_image "$reference" 2> /dev/null)" || selected=""
      if [ -z "$selected" ] || ! docker image inspect "$selected" > /dev/null 2>&1; then
        GUARD_SAYS="the new compose.yml names the image $reference, which is not among the carried-in images; load it (the offline guide), then run the update again"
        return 1
      fi
      continue
    fi
    if ! docker image inspect "$reference" > /dev/null 2>&1; then
      say "  pulling $reference (the new compose.yml names it)"
      if ! docker pull --quiet "$reference" > /dev/null 2>&1; then
        GUARD_SAYS="the new compose.yml names the image $reference, which this host does not have and could not pull; pull or load it, then run the update again"
        return 1
      fi
    fi
  done < <(compose_images "$FILES_STAGE/deploy/compose.yml")
}

# files_in_list NAME LIST...: whether NAME is one of the words.
files_in_list() { local name=$1 word; shift; for word; do [ "$word" != "$name" ] || return 0; done; return 1; }

# decide_set NAME MEMBER...: what becomes of a set of files. Sets SET_ACTION (same, replace or keep), SET_GEN (the generation
# the set has afterwards), SET_REASON and SET_DIFFER (the members that are not the new release's). The installed files are
# compared one by one with what any published release shipped of each; one that equals none is an edit, and an edit keeps
# the whole set.
decide_set() {
  local name=$1; shift
  local member old new generation record reviewed recorded_generation minimum_generation=999999
  local -a differ=() edited=()
  SET_ACTION=same; SET_REASON=""; SET_DIFFER=()
  for member; do
    new="$(sha_of "$FILES_STAGE/deploy/$member")"
    if [ -e "$member" ]; then old="$(sha_of "$member")"; else old=-; fi
    SET_OLD[$member]="$old"; SET_NEW[$member]="$new"
    if [ "$old" != "$new" ]; then differ+=("$member"); fi
    if [ "$old" = - ]; then
      generation=0
    else
      generation=""
      if [ ! -L "$member" ]; then generation="$(fs_gen_of "$member" "$old")"; fi
      if [ -z "$generation" ]; then
        edited+=("$member")
        # An edited file has the generation of the shipped one whose .new its owner has read and removed.
        reviewed="$(fstate_get "reviewed.$member")"; reviewed="${reviewed:-$FILES_BASELINE}"
        record="$(fstate_get "kept.$member")"
        if [ -n "$record" ] && [ ! -e "$member.new" ]; then
          recorded_generation="${record#*:}"
          if [ "$recorded_generation" -gt "$reviewed" ]; then reviewed="$recorded_generation"; fi
          fstate_set "reviewed.$member" "$reviewed"
        fi
        generation="$reviewed"
      fi
    fi
    if [ "$generation" -lt "$minimum_generation" ]; then minimum_generation="$generation"; fi
  done
  SET_DIFFER=("${differ[@]}")
  if [ "${#differ[@]}" -eq 0 ]; then
    SET_GEN="$minimum_generation"
    return 0
  fi
  if [ "${#edited[@]}" -gt 0 ]; then
    SET_ACTION=keep; SET_GEN="$minimum_generation"; SET_REASON="edited here: ${edited[*]}"
    return 0
  fi
  GUARD_SAYS=""
  if [ "$name" = "$FILES_MAIN" ]; then
    if files_in_list postgres/Dockerfile "${differ[@]}" && ! files_guard_postgres; then SET_REASON="$GUARD_SAYS"
    elif files_in_list compose.yml "${differ[@]}" && ! files_guard_binds; then SET_REASON="$GUARD_SAYS"
    elif files_in_list compose.yml "${differ[@]}" && ! files_guard_images; then SET_REASON="$GUARD_SAYS"
    fi
  fi
  if [ -n "$SET_REASON" ]; then SET_ACTION=keep; SET_GEN="$minimum_generation"; return 0; fi
  SET_ACTION=replace; SET_GEN="$FS_GENERATION"
}

# keep_set MEMBER...: the shipped file of each member that is not the installed one is written beside it as <name>.new,
# unless it was kept against the same shipped file before: then its owner has read it, or will.
keep_set() {
  local member shipped record
  for member; do
    shipped="${SET_NEW[$member]}"
    [ "${SET_OLD[$member]}" != "$shipped" ] || continue
    record="$(fstate_get "kept.$member")"
    if [ "${record%%:*}" = "$shipped" ]; then continue; fi
    write_file "$FILES_STAGE/deploy/$member" "$member.new"
    fstate_set "kept.$member" "$shipped:$FS_GENERATION"
  done
}

# files_bundle: the release's bundle is unpacked, and its table read; FILES_HAVE is 1 when there is one to follow.
files_bundle() {
  FILES_HAVE=0
  FILES_STAGE="$WORK/files"
  [ -n "${BUNDLE:-}" ] || return 0
  mkdir -p "$FILES_STAGE"
  tar -xzf "$BUNDLE" -p --no-same-owner -C "$FILES_STAGE" deploy || die "the bundle of release $TAG could not be unpacked"
  [ -f "$FILES_STAGE/deploy/FILE_SETS" ] || return 0
  fs_load "$FILES_STAGE/deploy/FILE_SETS"
  FILES_HAVE=1
}

# plan_files: before anything is stopped, decides what becomes of every file of the bundle. Sets FILES_REPLACE (the files that
# will be written), FILES_GEN_AFTER (the generation of the set afterwards) and UPDATE_SH (what becomes of update.sh); writes
# the .new files; and stops, with the files to merge named, when the set that will be in use is older than the release's
# payloads work with. It writes nothing but .new files and the record of what was kept.
plan_files() {
  local minimum member set name shipped installed newest
  local -a followed=() plan_lines=() block_names=()
  FILES_REPLACE=(); FILES_GEN_AFTER=""; UPDATE_SH=none; FILES_MAIN=""
  declare -gA SET_OLD=() SET_NEW=()
  minimum="$(mget deploy_files.minimum)"; minimum="${minimum:-0}"
  [[ "$minimum" =~ ^[0-9]+$ ]] || die "version.json has a deploy_files minimum that is not a number"
  files_bundle
  if [ "$FILES_HAVE" = 0 ]; then
    FILES_GEN_AFTER="$(fstate_get files_generation)"; FILES_GEN_AFTER="${FILES_GEN_AFTER:-$FILES_BASELINE}"
    if [ -z "${BUNDLE:-}" ]; then say "files: release $TAG comes without its orch-deploy bundle; the deployment files are left as they are"
    else say "files: the bundle of release $TAG has no FILE_SETS; the deployment files are left as they are"; fi
  else
    [ "$FS_MINIMUM" = "$minimum" ] || die "version.json asks for deployment files of generation $minimum and the bundle says $FS_MINIMUM"
    FILES_MAIN="main"
    [ -n "${FS_MEMBERS[main]:-}" ] || die "FILE_SETS names no set called main"
    while IFS= read -r member; do followed+=("$member"); done < <(cd "$FILES_STAGE/deploy" && find . -type f ! -path './config/*' | sed 's|^\./||' | LC_ALL=C sort)
    for member in ${FS_MEMBERS[main]}; do
      [ -f "$FILES_STAGE/deploy/$member" ] || die "FILE_SETS names $member, which the bundle does not hold"
    done
    # The set the generation counts first, then each other file by itself; update.sh is the updater's own.
    # shellcheck disable=SC2086 # the members of the set, split on purpose
    decide_set main ${FS_MEMBERS[main]}
    FILES_GEN_AFTER="$SET_GEN"
    case "$SET_ACTION" in
      replace)
        for member in "${SET_DIFFER[@]}"; do
          FILES_REPLACE+=("$member")
          plan_lines+=("files: replacing $member (release $TAG's, generation $FS_GENERATION)")
        done ;;
      keep)
        keep_set "${SET_DIFFER[@]}"
        block_names=("${SET_DIFFER[@]/%/.new}")
        for member in "${SET_DIFFER[@]}"; do
          plan_lines+=("files: kept $member ($SET_REASON); the shipped one is $member.new")
        done ;;
    esac
    for member in "${followed[@]}"; do
      set="${FS_SET_OF[$member]:-$member}"
      [ "$set" != main ] || continue
      if [ "$member" = update.sh ]; then continue; fi
      decide_set "$member" "$member"
      case "$SET_ACTION" in
        replace) FILES_REPLACE+=("$member"); plan_lines+=("files: replacing $member (release $TAG's)") ;;
        keep) keep_set "$member"; plan_lines+=("files: kept $member ($SET_REASON); the shipped one is $member.new") ;;
      esac
    done
    # update.sh is replaced last, and only when the one here is a published one that is older than this release's.
    shipped="$(sha_of "$FILES_STAGE/deploy/update.sh")"; installed="$(sha_of update.sh)"
    SET_OLD[update.sh]="$installed"; SET_NEW[update.sh]="$shipped"
    if [ "$installed" != "$shipped" ]; then
      newest=""
      if [ ! -L update.sh ]; then newest="$(fs_newest_tag update.sh "$installed")"; fi
      if [ -z "$newest" ]; then
        keep_set update.sh
        plan_lines+=("files: kept update.sh (it differs from every published version); the shipped one is update.sh.new")
      elif [ "$(printf '%s\n%s\n' "$newest" "$TAG" | sort -V | tail -n 1)" = "$TAG" ]; then
        UPDATE_SH=replace
        plan_lines+=("files: update.sh is replaced when the update has finished")
      fi
    fi
  fi
  for name in "${plan_lines[@]}"; do say "$name"; done
  if [ "$FILES_GEN_AFTER" -lt "$minimum" ]; then
    if [ "$FILES_HAVE" = 0 ]; then
      die "release $TAG works with deployment files of generation $minimum or newer, and these are generation $FILES_GEN_AFTER; its orch-deploy bundle is not with it. Give the update the release directory that holds orch-deploy-$TAG.tar.gz (--from), or merge that bundle's files by hand. Nothing was stopped"
    fi
    die "release $TAG works with deployment files of generation $minimum or newer, and these are generation $FILES_GEN_AFTER; merge ${block_names[*]:-the shipped files} and run the update again. Nothing was stopped"
  fi
}

# files_notice: an update to the release that is installed does nothing; but the files it runs with may be older than its programs
# work with (an updater that did not follow them brought it), and that is said.
files_notice() {
  local minimum generation
  minimum="$(mget deploy_files.minimum)"
  generation="$(fstate_get files_generation)"; generation="${generation:-$FILES_BASELINE}"
  if [[ "$minimum" =~ ^[0-9]+$ ]] && [ "$generation" -lt "$minimum" ]; then
    say "files: release $TAG works with deployment files of generation $minimum or newer, and these are generation $generation; take the files of its orch-deploy bundle, merging what you changed"
  fi
}

# files_begin: the files of the plan are saved where a rollback and a recover find them, and the journal lists them, before the
# first is written. SOURCE is the directory the new content comes from.
files_begin() {
  local source=$1 member store="$FILES_DIR/${J[id]}" old new
  mkdir -p "$store/old" "$store/new"
  : > "$store/list"
  J[files]="${FILES_REPLACE[*]:-}"
  J[files_gen]="$FILES_GEN_AFTER"
  J[files_gen_before]="$(fstate_get files_generation)"
  for member in "${FILES_REPLACE[@]}"; do
    mkdir -p "$store/old/$(dirname "$member")" "$store/new/$(dirname "$member")"
    old=-; new="${SET_NEW[$member]}"
    if [ -e "$member" ]; then old="$(sha_of "$member")"; cp -p -- "$member" "$store/old/$member"; fi
    if [ "$new" != - ]; then cp -p -- "$source/$member" "$store/new/$member"; fi
    J["file_old.$member"]="$old"; J["file_new.$member"]="$new"
    printf '%s %s %s\n' "$member" "$old" "$new" >> "$store/list"
  done
  J[update_sh]=none
  if [ "${UPDATE_SH:-none}" = replace ]; then
    J[update_sh]=replace; J[update_sh_old]="${SET_OLD[update.sh]}"
    cp -p -- "$FILES_STAGE/deploy/update.sh" "$store/update.sh"
  fi
  if [ "$(id -u)" -eq 0 ]; then chown -R "$SERVICE_UID:$SERVICE_GID" "$store"; fi
}

# apply_files: the files of the journal are written, each in one rename; doing it twice does no harm.
apply_files() {
  local member store="$FILES_DIR/${J[id]}"
  for member in ${J[files]:-}; do
    if [ "${J[file_new.$member]}" = - ]; then rm -f -- "$member"; else write_file "$store/new/$member" "$member"; fi
  done
}

# files_database_start: a changed compose.yml may change the database's container; it is made again, with the database kept, so
# that it is what the Compose file in use says (a no-op when nothing about it changed).
files_database_start() {
  case " ${J[files]:-} " in *" compose.yml "*) database_start ;; esac
}

# restore_files: the files the journal lists go back to what they were, each only if it is still what the transaction wrote. One
# that was edited since stays, and the command says so.
restore_files() {
  local member current store="$FILES_DIR/${J[id]}"
  for member in ${J[files]:-}; do
    current=-
    if [ -e "$member" ]; then current="$(sha_of "$member")"; fi
    if [ "$current" = "${J[file_new.$member]}" ]; then
      if [ "${J[file_old.$member]}" = - ]; then rm -f -- "$member"; else write_file "$store/old/$member" "$member"; fi
    elif [ "$current" != "${J[file_old.$member]}" ]; then
      note "$member was left as it is: it is no longer what the transaction wrote"
    fi
  done
}

# plan_files_rollback: the files the last transaction replaced are put back where they still are what it wrote.
plan_files_rollback() {
  local previous member old new current
  FILES_REPLACE=(); FILES_SOURCE=""; UPDATE_SH=none
  declare -gA SET_OLD=() SET_NEW=()
  FILES_GEN_AFTER="$(fstate_get files_generation)"
  previous="$(fstate_get files_previous)"
  [ -n "$previous" ] && [ -f "$FILES_DIR/$previous/list" ] || return 0
  FILES_SOURCE="$FILES_DIR/$previous/old"
  FILES_ALL_BACK=1
  while read -r member old new; do
    current=-
    if [ -e "$member" ]; then current="$(sha_of "$member")"; fi
    if [ "$current" = "$new" ]; then
      FILES_REPLACE+=("$member"); SET_NEW[$member]="$old"
    elif [ "$current" != "$old" ]; then
      FILES_ALL_BACK=0
      say "files: $member stays as it is: it is no longer what the last update wrote (the earlier one is in $FILES_SOURCE/$member)"
    fi
  done < "$FILES_DIR/$previous/list"
  if [ "${FILES_ALL_BACK:-1}" = 1 ] && [ -f "$FILES_DIR/$previous/generation_before" ]; then FILES_GEN_AFTER="$(cat "$FILES_DIR/$previous/generation_before")"; fi
  if [ "${#FILES_REPLACE[@]}" -gt 0 ]; then say "files: putting back ${FILES_REPLACE[*]}"; fi
}

# files_finish: the transaction is done: the generation and the saved files are recorded, the files of older transactions are
# removed, and update.sh is replaced, as the last act.
files_finish() {
  local member name store="$FILES_DIR/${J[id]}" keep=""
  if [ -n "${J[files_gen]:-}" ]; then fstate_set files_generation "${J[files_gen]}"; fi
  if [ -n "${J[files]:-}" ]; then
    printf '%s\n' "${J[files_gen_before]:-$FILES_BASELINE}" > "$store/generation_before"
    fstate_set files_previous "${J[id]}"
    keep="${J[id]}"
  else
    fstate_del files_previous
  fi
  for member in ${J[files]:-}; do
    fstate_del "kept.$member"; fstate_del "reviewed.$member"
  done
  if [ -d "$FILES_DIR" ]; then
    for name in "$FILES_DIR"/*; do
      name="${name##*/}"
      [[ "$name" =~ ^[0-9A-Za-z][0-9A-Za-z._-]*$ ]] || continue
      if [ "$name" = "${J[id]}" ]; then continue; fi
      rm -rf -- "$DEPLOY_DIR/$FILES_DIR/${name:?}"
    done
  fi
  if [ "${J[update_sh]:-none}" = replace ] && [ -f "$store/update.sh" ]; then
    if [ "$(sha_of update.sh)" = "${J[update_sh_old]}" ]; then
      write_file "$store/update.sh" update.sh
      say "update.sh is now the one of release ${J[to_tag]}"
    else
      note "update.sh was changed while the update ran, and is left as it is; the shipped one is in $store/update.sh"
      keep="${J[id]}"
    fi
  fi
  if [ -z "$keep" ]; then rm -rf -- "$DEPLOY_DIR/$FILES_DIR/${J[id]:?}"; fi
}

# files_status: what `status` says of the deployment files.
files_status() {
  local generation key member
  generation="$(fstate_get files_generation)"
  say "deployment files: generation ${generation:-$FILES_BASELINE}$([ -n "$generation" ] || printf ' (assumed: no update has followed them yet)')"
  [ -f "$FILES_STATE" ] || return 0
  while IFS= read -r key; do
    member="${key#kept.}"
    if [ -e "$member.new" ]; then say "  kept: $member differs from the shipped file, which is $member.new; merge it and remove it"; fi
  done < <(awk -F= '$1 ~ /^kept\./ { print $1 }' "$FILES_STATE")
}

# files_record_first_start: a first start leaves the files as they were unpacked; what the bundle says of them is recorded, so
# that a later update that comes without a bundle can still tell what generation they are.
files_record_first_start() {
  local member generation="$FILES_BASELINE"
  files_bundle
  [ "$FILES_HAVE" = 1 ] || return 0
  generation="$FS_GENERATION"
  for member in ${FS_MEMBERS[main]:-}; do
    if [ ! -e "$member" ] || [ "$(sha_of "$member")" != "$(sha_of "$FILES_STAGE/deploy/$member")" ]; then generation="$FILES_BASELINE"; fi
  done
  fstate_set files_generation "$generation"
}
# --- The stack ---------------------------------------------------------------------------

# shellcheck disable=SC2086 # a list of service names, split on purpose
app_stop() { compose stop -t "$STOP_TIMEOUT" $STOP_SERVICES > /dev/null; }

# app_start: creates the application services again, so that they see the
# releases that the links point at now. A service that does not become healthy
# makes Compose say so and fail; that is for the readiness check to judge, with
# the whole picture, and not a reason to stop here.
app_start() {
  check_installation
  records_mount_point replace
  # shellcheck disable=SC2086 # a list of service names, split on purpose
  if ! compose up -d --quiet-pull --force-recreate --no-deps $APP_SERVICES > /dev/null; then
    note "docker compose could not bring every service up healthy; the readiness check says which"
  fi
}

database_start() {
  compose up -d --quiet-pull --wait postgres > /dev/null || die "the database did not become healthy; docker compose logs postgres"
}

# Build the service checks for both the one-shot doctor and the install readiness loop.
inner_doctor_command() {
  local version="${1:-}" skip=() services
  # The list is read whole before it is searched: grep -q leaves at its first match, and the writer of a pipe it
  # leaves is killed, which under pipefail made the node look absent now and then.
  services="$(compose config --services 2> /dev/null)" || services=""
  grep -qx node <<< "$services" || skip=(--skip node)
  INNER_DOCTOR=(compose exec -T api /opt/orchctl/orchctl doctor)
  [ -z "$version" ] || INNER_DOCTOR+=(--expect-version "$version")
  [ "${#skip[@]}" -eq 0 ] || INNER_DOCTOR+=("${skip[@]}")
}

edge_doctor_command() {
  EDGE_DOCTOR=(runtime/cli/current/orchctl doctor --url "$1"
    --config config/orch.toml --secrets-dir config/secrets
    --only "edge-page,edge-api,edge-link")
}

doctor_commands() {
  if [ -n "${ORCH_READY_URL:-}" ]; then
    EDGE_URL="$ORCH_READY_URL"; EDGE_SOURCE="ORCH_READY_URL in the environment"
  elif [ -n "$(env_get ORCH_READY_URL)" ]; then
    EDGE_URL="$(env_get ORCH_READY_URL)"; EDGE_SOURCE="ORCH_READY_URL in .env"
  else
    EDGE_URL="$(sed -n 's/^public_url[[:space:]]*=[[:space:]]*"\(.*\)".*/\1/p' config/orch.toml | head -n 1)"
    EDGE_SOURCE="[system] public_url in config/orch.toml"
  fi
  if [ -z "$EDGE_URL" ]; then
    note "readiness needs [system] public_url or ORCH_READY_URL for the operator's web edge"
    return 1
  fi
  inner_doctor_command
  edge_doctor_command "$EDGE_URL"
}

# wait_ready TAG: the services and the operator's web edge serve the release.
wait_ready() {
  local version="${1#v}" deadline=$((SECONDS + READY_TIMEOUT)) next=$((SECONDS + 15)) output edge_output edge_url operator_edge fails
  operator_edge="${ORCH_READY_URL:-$(env_get ORCH_READY_URL)}"
  inner_doctor_command "$version"
  INNER_DOCTOR+=(--skip edge)
  while :; do
    if output="$("${INNER_DOCTOR[@]}" 2>&1)"; then
      edge_url="$operator_edge"
      if [ -z "$edge_url" ]; then
        # Read again for every attempt: a file edit is only a seed, and publication can change while readiness waits.
        edge_url="$(compose exec -T api /opt/orchctl/orchctl public-address get 2>/dev/null)" || edge_url=
      fi
      if [ -z "$edge_url" ]; then
        edge_output='FAIL  ready-url        the committed public address could not be read through the local database'
      else
        edge_doctor_command "$edge_url"
        if edge_output="$("${EDGE_DOCTOR[@]}" 2>&1)"; then
          # The edge doctor reports the API version it actually reached. It may
          # reach a stale upstream even while the internal API runs this release.
          if printf '%s\n' "$edge_output" | grep -Fq "release $version (build "; then
            printf '%s\n%s\n' "$output" "$edge_output"
            return 0
          fi
          edge_output+=$'\n'"FAIL  edge-version     $edge_url did not serve release $version"
        fi
      fi
      output+=$'\n'"$edge_output"
    fi
    if [ "$SECONDS" -ge "$deadline" ]; then
      printf '%s\n' "$output" >&2
      note "the stack was not ready, or did not report release $1, within $READY_TIMEOUT s"
      compose ps >&2 || true
      return 1
    fi
    if [ "$SECONDS" -ge "$next" ]; then
      fails="$(printf '%s\n' "$output" | grep -E '^FAIL' | cut -c1-110 | head -n 2 | tr '\n' ' ')"
      say "  waiting for readiness: ${fails:-$(printf '%s' "$output" | head -n 1 | cut -c1-110)}"
      next=$((SECONDS + 15))
    fi
    sleep 3
  done
}

# --- The database ------------------------------------------------------------------------

# The bootstrap dump precedes the first schema. It has no application inventory and is never a recorded backup.
take_dump() {
  if [ "${J[kind]:-}" != first-start ]; then
    backup_command dump --out "/backups/$(basename "$1")" --records-dir /installation \
      --release "$(state_get current_tag)" --cause "${2:-pre-update}" || {
        if [ "${J[kind]:-}" = update ] || [ "${J[kind]:-}" = rollback ]; then
          die "database backup failed after services were stopped; resolve the cause and run ./update.sh recover to restart them"
        fi
        die "database backup failed; a completed pair, if any, can repair its Overview record without another dump"
      }
    return
  fi
  take_bootstrap_dump "$1"
}

# A short-lived instance of the existing PostgreSQL image supplies pg_dump/pg_restore and no new daemon.
backup_command() {
  ORCH_DATABASE_URL="postgres://orch:$(env_get POSTGRES_PASSWORD)@postgres:5432/orch" \
    compose run --rm --no-deps --entrypoint /cli/orchctl -e ORCH_DATABASE_URL \
    -v "$DEPLOY_DIR/runtime/cli/current:/cli:ro" -v "$DEPLOY_DIR/config/orch.toml:/etc/orch/orch.toml:ro" \
    -v "$DEPLOY_DIR/config/secrets/orch_ca_cert.pem:/run/secrets/orch_ca_cert.pem:ro" \
    -v "${BACKUP_DESTINATION:-$DEPLOY_DIR/$BACKUPS}:/backups" -v "$DEPLOY_DIR/runtime/update:/installation" \
    postgres backup --config /etc/orch/orch.toml --secrets-dir /run/secrets "$@"
}

# The one pre-schema safety dump; application backups use the matching snapshot/manifest writer above.
take_bootstrap_dump() {
  local out="$1" partial="$1.partial"
  case "$out" in "$DEPLOY_DIR/$BACKUPS/"* | "$BACKUPS/"*) ;; *) die "a dump goes under $BACKUPS, and not to $out" ;; esac
  if ! compose exec -T postgres pg_dump -U orch -d orch --format=custom --no-owner > "$partial"; then
    rm -f -- "$partial"; die "pg_dump failed; is the database running?"
  fi
  if [ ! -s "$partial" ] || ! compose exec -T postgres pg_restore --list < "$partial" > /dev/null; then
    rm -f -- "$partial"; die "the dump cannot be read back, so it is not kept"
  fi
  chmod 600 "$partial"
  mv -T "$partial" "$out"
}

# restore_database FILE: the database becomes what the dump holds. The services
# that use it are stopped.
restore_database() {
  [ -s "$1" ] || die "the dump $1 is not there to restore from"
  compose exec -T postgres psql -U orch -d postgres -v ON_ERROR_STOP=1 \
    -c 'DROP DATABASE IF EXISTS orch WITH (FORCE)' -c 'CREATE DATABASE orch OWNER orch' > /dev/null ||
    die "the database could not be made again"
  compose exec -T postgres pg_restore -U orch -d orch --no-owner --exit-on-error < "$1" ||
    die "the dump $1 could not be restored"
}

# sql QUERY: one value or a few rows from the database; a failed query is not an empty inventory.
sql() { compose exec -T postgres psql -X -v ON_ERROR_STOP=1 -U orch -d orch -At -F'|' -c "$1"; }

# --- What an update does to running work -------------------------------------------------

# warn_about_work: tells what the new release does to what is running, and sets
# NEEDS_ANSWER when it said anything, for the caller to ask whether to go on.
warn_about_work() {
  local running units prereleases kind count lines="" environments=0 providers=0
  running="$(sql "SELECT count(*) FROM threads WHERE state IN ('starting','running','waiting_approval')")" ||
    die "could not inspect running threads; no services were stopped"
  [[ "$running" =~ ^[0-9]+$ ]] || die "the running thread count could not be read; no services were stopped"
  units="$(sql "SELECT r.kind, count(*) FROM runtimes r JOIN machines m ON m.id = r.machine_id
                 WHERE r.state IN ('creating','running','stopped','stopping')
                   AND string_to_array(split_part(split_part(m.connector_version, '+', 1), '-', 1), '.')::numeric[]
                       < string_to_array(split_part('$CONNECTOR_MINIMUM', '-', 1), '.')::numeric[]
                 GROUP BY r.kind")" || die "could not inspect runtime connector versions; no services were stopped"
  while IFS='|' read -r kind count; do
    [ -n "$kind" ] || continue
    [[ "$count" =~ ^[0-9]+$ ]] || die "the runtime count could not be read; no services were stopped"
    case "$kind" in
      environment) environments="$count" ;;
      provider) providers="$count" ;;
      *) die "the runtime kind $kind could not be read; no services were stopped" ;;
    esac
  done <<< "$units"
  # The gateway compares prerelease identifiers too. Do not treat one with the
  # minimum's numeric version as current without checking those identifiers.
  prereleases="$(sql "SELECT count(*) FROM runtimes r JOIN machines m ON m.id = r.machine_id
                      WHERE r.state IN ('creating','running','stopped','stopping')
                        AND string_to_array(split_part(split_part(m.connector_version, '+', 1), '-', 1), '.')::numeric[]
                            = string_to_array(split_part('$CONNECTOR_MINIMUM', '-', 1), '.')::numeric[]
                        AND position('-' IN split_part(m.connector_version, '+', 1)) > 0")" ||
    die "could not inspect prerelease connector versions; no services were stopped"
  [[ "$prereleases" =~ ^[0-9]+$ ]] || die "the prerelease runtime count could not be read; no services were stopped"
  if [ "$environments" -gt 0 ] || [ "$providers" -gt 0 ]; then
    lines+="$(noun "$environments" environment) and $(noun "$providers" "managed provider") will need an update before they work again (release $TAG no longer accepts their connector).
"
  fi
  if [ "$prereleases" -gt 0 ]; then
    lines+="$prereleases runtimes use prerelease connectors on the minimum's numeric version; check whether they meet $CONNECTOR_MINIMUM before continuing.
"
  fi
  if [ "${running:-0}" -gt 0 ]; then
    lines+="$running threads are running. Their turns are not interrupted and their events arrive after the services are back; but if that takes longer than the 60 s that open exec tunnels wait, those tunnels are reset: the commands in them end, and the agent sees them fail.
"
  fi
  NEEDS_ANSWER=0
  [ -n "$lines" ] || return 0
  printf '%s' "$lines"
  NEEDS_ANSWER=1
}

# confirm QUESTION: yes with --yes, or when a person types y; otherwise stop.
confirm() {
  if [ "${YES:-0}" -eq 1 ]; then return 0; fi
  if [ -t 0 ]; then
    local answer
    read -r -p "$1 [y/N] " answer
    [[ "$answer" =~ ^[Yy] ]] || die "stopped; nothing was changed"
  else
    die "this needs an answer and there is nobody to ask: run it again with --yes to go on"
  fi
}

# --- Transactions --------------------------------------------------------------------------

# activate_payloads: the new releases become current and the ones that were current
# become previous, a link at a time. Every link is set from the journal's record, so
# doing it twice does no harm.
activate_payloads() {
  local kind
  for kind in $PAYLOADS; do
    link_set "$kind" previous "${J[${kind}_previous_after]}"
    link_set "$kind" current "${J[${kind}_current_after]}"
  done
}

# deactivate_payloads: every link is what the journal recorded before the transaction.
deactivate_payloads() {
  local kind
  for kind in $PAYLOADS; do
    link_set "$kind" current "${J[${kind}_current_before]}"
    link_set "$kind" previous "${J[${kind}_previous_before]}"
  done
}

# undo_with_database: puts the old releases and the old database back, and starts
# them. The failed release has left nothing but the dump to go by.
undo_with_database() {
  note "putting release ${J[from_tag]} back"
  jset step restoring
  app_stop || true
  deactivate_payloads
  restore_files
  files_database_start
  restore_database "${J[leaving_dump]}"
  jset step restarting
  app_start
  if wait_ready "${J[from_tag]}"; then
    note "release ${J[from_tag]} is running again, with the database as it was before; the dump is ${J[leaving_dump]}"
    journal_clear
  else
    note "release ${J[from_tag]} was put back and does not become ready either; docker compose logs, then ./update.sh recover"
    return 1
  fi
}

# finish_transaction: the new release runs; its links stay, and the record says so.
finish_transaction() {
  jset step "done"
  state_save "${J[to_tag]}" "${J[from_tag]}" "${J[leaving_dump]}"
  journal_clear
  prune_releases
  publish_storage_metadata
  files_finish
}

# Read the same registered system policy whether previewing or applying a cleanup.
release_keep_count() {
  local keep
  if ! keep="$(compose exec -T postgres psql -X -q -t -A -U orch -d orch -c \
    "SELECT COALESCE((SELECT value #>> '{}' FROM settings WHERE node_id IS NULL AND key = 'cleanup.release_keep_count'), '3')")"; then
    note "unused releases were kept: cleanup.release_keep_count could not be read"
    return 1
  fi
  [[ "$keep" =~ ^[0-9]+$ ]] || { note "unused releases were kept: cleanup.release_keep_count is not a whole number"; return 1; }
  printf '%s\n' "$keep"
}

# The automatic pass applies; the operator command supplies zero for its dry run.
prune_releases() {
  local apply="${1:-1}" keep kind current previous name retained directory identity bytes marker
  STORAGE_KEEP_COUNT=""
  if ! keep="$(release_keep_count)"; then return 0; fi
  STORAGE_KEEP_COUNT="$keep"
  [ "$keep" != 0 ] || return 0
  for kind in $PAYLOADS; do
    directory="$DEPLOY_DIR/runtime/$kind/releases"
    if [ ! -d "$DEPLOY_DIR/runtime" ] || [ -L "$DEPLOY_DIR/runtime" ]; then continue; fi
    if [ ! -d "$DEPLOY_DIR/runtime/$kind" ] || [ -L "$DEPLOY_DIR/runtime/$kind" ]; then continue; fi
    if [ ! -d "$directory" ] || [ -L "$directory" ]; then continue; fi
    identity="$(stat -c '%d:%i' -- "$directory")"
    current="$(link_get "$kind" current)"; previous="$(link_get "$kind" previous)"
    retained=0
    while IFS=$'\t' read -r -d '' _ name; do
      case "$name" in "$current" | "$previous") continue ;; esac
      case "$name" in .*) continue ;; esac
      # Only whole payloads that the updater published are eligible; links and staging paths stay.
      [[ "$name" =~ ^[0-9A-Za-z][0-9A-Za-z._-]*$ ]] || continue
      if [ ! -f "$directory/$name/.complete" ] || [ -L "$directory/$name/.complete" ] || [ -L "$directory/$name" ]; then continue; fi
      # A complete marker is one 64-character digest, optionally ending in a newline.
      [ "$(stat -c %s -- "$directory/$name/.complete")" -le 65 ] || continue
      marker="$(cat "$directory/$name/.complete")"
      [[ "$marker" =~ ^[0-9a-f]{64}$ ]] || continue
      retained=$((retained + 1))
      if [ "$retained" -gt "$keep" ]; then
        bytes="$(du -sb -- "$directory/$name" | cut -f1)"
        if [ "$apply" = 0 ]; then
          say "$(uninstall_words 'Would remove' '将移除') $kind/$name ($bytes $(uninstall_words bytes 字节))"
        else
          # Manual changes outside this lock cannot redirect a confirmed cleanup.
          if [ -L "$DEPLOY_DIR/runtime" ] || [ -L "$DEPLOY_DIR/runtime/$kind" ] || [ -L "$directory" ]; then die "the release directory changed; review prune again"; fi
          [ "$(stat -c '%d:%i' -- "$directory")" = "$identity" ] || die "the release directory changed; review prune again"
          if [ "$name" = "$(link_get "$kind" current)" ] || [ "$name" = "$(link_get "$kind" previous)" ]; then continue; fi
          if [ -L "$directory/$name" ] || [ -L "$directory/$name/.complete" ] || [ "$(cat "$directory/$name/.complete")" != "$marker" ]; then die "the release changed; review prune again"; fi
          say "$(uninstall_words Removing 正在移除) $kind/$name ($bytes $(uninstall_words bytes 字节))"
          rm -rf -- "${directory:?}/${name:?}"
        fi
      fi
    done < <(find "$directory" -mindepth 1 -maxdepth 1 -type d -printf '%T@\t%f\0' | sort -zrn)
  done
}

# Only the local program writes this metadata. The API mount remains read-only.
publish_storage_metadata() {
  local program="$DEPLOY_DIR/runtime/cli/current/orchctl" keep="${STORAGE_KEEP_COUNT:-}" arguments=()
  [ -x "$program" ] || { note "storage metadata was not refreshed: the installed CLI is unavailable"; return 0; }
  if [ -z "$keep" ]; then keep="$(release_keep_count)" || keep=""; fi
  [ -z "$keep" ] || arguments=(--keep "$keep")
  if ! "$program" operator-storage inventory --root "$DEPLOY_DIR" "${arguments[@]}"; then
    note "storage metadata could not be refreshed"
    return 0
  fi
  if [ "$(id -u)" = 0 ]; then chown "$SERVICE_UID:$SERVICE_GID" runtime/update/storage.json; fi
}

cmd_prune() {
  load_facts
  check_password
  [ ! -f "$JOURNAL" ] || die "an update is unfinished; run ./update.sh recover first"
  prune_releases "$YES"
  publish_storage_metadata
}

cmd_dumps_remove() {
  load_facts
  check_password
  [ ! -f "$JOURNAL" ] || die "an update is unfinished; run ./update.sh recover first"
  local arguments=() result=0
  [ "$YES" = 0 ] || arguments=(--yes)
  "$DEPLOY_DIR/runtime/cli/current/orchctl" operator-storage dump-remove --root "$DEPLOY_DIR" --name "$1" "${arguments[@]}" || result=$?
  publish_storage_metadata
  return "$result"
}

# transact KIND: the common course of an update and a rollback, from the journal
# that the caller filled in: stop, dump, activate, (restore), start, verify.
transact() {
  local from="${J[from_tag]}" to="${J[to_tag]}"
  jset step stopping
  app_stop || die "services could not all be stopped; run ./update.sh recover to restart the previous release"
  jset step dumping
  take_dump "${J[leaving_dump]}"
  # The files switch after the dump, which the installed compose.yml took, and before the start.
  jset step filing
  apply_files
  files_database_start
  jset step activating
  activate_payloads
  if [ "${J[kind]}" = rollback ]; then
    jset step restoring_entered
    restore_database "${J[entering_dump]}"
  fi
  jset step starting
  app_start
  jset step verifying
  if wait_ready "$to"; then
    finish_transaction
    update_job_state updated
    return 0
  fi
  update_job_state rolling_back health_check_failed
  if undo_with_database; then
    update_job_state rolled_back health_check_failed
    die "release $to did not become ready; $from is running again as it was"
  fi
  update_job_state failed recovery_required
  die "release $to did not become ready, and $from could not be brought back either"
}

# --- Commands ------------------------------------------------------------------------------

cmd_update() {
  local selector=$1 kind
  require_installed
  WANT_BUNDLE=1
  update_job_state fetching
  fetch_release "$selector"
  update_job_state verifying
  if [ "$TAG" = "$(state_get current_tag)" ]; then
    files_notice
    say "release $TAG is installed already; nothing to do"
    return 0
  fi
  say "installing release $TAG over $(state_get current_tag)"
  update_job_state applying
  for kind in $PAYLOADS; do unpack "$kind"; NEW_RELEASE[$kind]="$UNPACKED"; done
  # What becomes of the deployment files is decided now, before anything is stopped.
  plan_files
  ensure_images
  warn_about_work
  [ "$NEEDS_ANSWER" -eq 0 ] || confirm "Go on?"
  ensure_secrets "$DEPLOY_DIR/runtime/cli/releases/${NEW_RELEASE[cli]}"
  begin_transaction update "$(state_get current_tag)" "$TAG"
  for kind in $PAYLOADS; do
    J[${kind}_current_after]="${NEW_RELEASE[$kind]}"
    J[${kind}_previous_after]="${J[${kind}_current_before]}"
  done
  files_begin "$FILES_STAGE/deploy"
  J[leaving_dump]="$BACKUPS/orch-$(stamp)-before-$TAG.dump"
  jset step staged
  transact
  say "release $TAG is installed and ready. The previous release is kept: ./update.sh rollback puts it back."
}

cmd_rollback() {
  local kind
  require_installed
  [ -n "$(state_get previous_tag)" ] || die "there is no previous release to go back to"
  local back dump
  back="$(state_get previous_tag)"; dump="$(state_get previous_dump)"
  [ -s "$dump" ] || die "the dump of the database as release $back left it ($dump) is not there; a rollback needs it"
  for kind in $PAYLOADS; do
    [ -n "$(link_get "$kind" previous)" ] || die "runtime/$kind/previous is not there"
  done
  cat << EOF
Rolling back to release $back puts the database as it was when release $(state_get current_tag) was installed
(the dump $dump). Everything written since then is lost from the running system. A dump of the present
database is kept first, so that this can be undone: ./update.sh rollback again goes forward.
EOF
  load_manifest_of_installed "$back"
  # The files the last update replaced are put back where they are still what it wrote; update.sh is not rolled back.
  plan_files_rollback
  warn_about_work
  confirm "Roll back to $back?"
  ensure_secrets "$DEPLOY_DIR/runtime/cli/current"
  begin_transaction rollback "$(state_get current_tag)" "$back"
  for kind in $PAYLOADS; do
    J[${kind}_current_after]="${J[${kind}_previous_before]}"
    J[${kind}_previous_after]="${J[${kind}_current_before]}"
  done
  files_begin "${FILES_SOURCE:-/nonexistent}"
  J[leaving_dump]="$BACKUPS/orch-$(stamp)-before-rollback-from-$(state_get current_tag).dump"
  J[entering_dump]="$dump"
  jset step staged
  transact
  say "release $back is running again."
}

# The previous server payload carries the manifest used for its own downloads.
load_manifest_of_installed() {
  local wanted=$1 release
  release="$(link_get server previous)"
  [ -n "$release" ] || die "the previous server release is not there"
  load_manifest "runtime/server/releases/$release/connectors"
  [ "$TAG" = "$wanted" ] || die "the previous server manifest says $TAG, but the installed record says $wanted"
}

cmd_backup() {
  local out
  require_installed
  compose ps --status running postgres | grep -q postgres || die "the database is not running; start the stack first"
  if [ -n "${REPAIR_RECORD:-}" ]; then
    case "$REPAIR_RECORD" in *[!A-Za-z0-9_.-]* | .* | "") die "--repair-record needs a plain dump name in $BACKUPS" ;; esac
    [ -f "$BACKUPS/$REPAIR_RECORD" ] || die "that completed dump is not in $BACKUPS"
    if [ -n "${PAIRED_BLOBS:-}" ]; then
      "$DEPLOY_DIR/runtime/cli/current/orchctl" operator-storage pair --root "$DEPLOY_DIR" --name "$REPAIR_RECORD" --directory "$PAIRED_BLOBS"
      publish_storage_metadata
      say "$(uninstall_words 'Recorded the local paired blob copy for' '已记录本地配对内容副本：') $REPAIR_RECORD"
      return
    fi
    backup_command record --dump "/backups/$REPAIR_RECORD" \
      --manifest "/backups/${REPAIR_RECORD%.dump}.manifest.json" --records-dir /installation \
      || die "the pair's metadata could not be repaired; no new dump was taken"
    publish_storage_metadata
    return
  fi
  out="$BACKUPS/orch-$(stamp)-$$-$RANDOM-backup.dump"
  take_dump "$out" manual
  publish_storage_metadata
}

cmd_status() {
  say "installed: $(state_get current_tag || true) (previous: $(state_get previous_tag || true))"
  if journal_load; then say "an interrupted ${J[kind]} is recorded (step ${J[step]}): ./update.sh recover"; else say "no transaction is open"; fi
  compose ps 2> /dev/null || true
  files_status
}

# cmd_doctor: the two readiness checks, once: no waiting, no retry and no release to expect; every line as it comes.
# Both run whatever the first says, so that one failure does not hide the other. It writes nothing and changes nothing.
cmd_doctor() {
  local failed=0
  doctor_commands || return 1
  say "edge address: $EDGE_URL (from $EDGE_SOURCE)"
  say "inside the stack: orchctl doctor in the API's container"
  "${INNER_DOCTOR[@]}" 2>&1 || failed=1
  say "from this host: edge-page, edge-api and edge-link against $EDGE_URL"
  if [ -x runtime/cli/current/orchctl ]; then
    "${EDGE_DOCTOR[@]}" 2>&1 || failed=1
  else
    say "FAIL  edge             runtime/cli/current/orchctl is not installed here, so the edge cannot be checked from this host"
    failed=1
  fi
  if [ "$failed" = 0 ]; then say "doctor: every check passed"; else say "doctor: a check failed (see the lines above)"; fi
  return "$failed"
}

cmd_recover() {
  local step kind from to
  journal_load || { say "no transaction is open; nothing to recover"; return 0; }
  step="${J[step]}"; kind="${J[kind]}"; from="${J[from_tag]}"; to="${J[to_tag]}"
  say "an interrupted $kind to $to is recorded; it stopped at step $step"
  if [ "$kind" = first-start ]; then
    say "a first start can be taken up again: repeat the original ./update.sh first-start command with its options; the journal is cleared"
    journal_clear
    return 0
  fi
  require_installed allow-journal
  ensure_secrets "$DEPLOY_DIR/runtime/cli/current"
  case "$step" in
    begin | staged | stopping | dumping | filing | activating)
      # No service has run the new release, and the database is as it was: the
      # links and the files go back, and the old release runs.
      note "the new release never ran; starting release $from as it was"
      deactivate_payloads
      restore_files
      files_database_start
      # A cut at begin can precede assignment of the dump name.
      if [ -n "${J[leaving_dump]:-}" ]; then rm -f -- "${J[leaving_dump]}.partial"; fi
      app_start
      wait_ready "$from" || die "release $from does not become ready; docker compose logs"
      journal_clear
      say "release $from is running; the $kind to $to was undone" ;;
    restoring_entered | restoring | restarting)
      # The database was being replaced: it is made again from the dump of what was left.
      app_stop || true
      undo_with_database || die "could not bring release $from back"
      say "the $kind to $to was undone; release $from is running" ;;
    starting | verifying)
      # The new release may be up and fine. If it is, it stays.
      app_start
      if wait_ready "$to"; then
        finish_transaction
        say "release $to is running and ready; the $kind was finished"
      else
        undo_with_database || die "could not bring release $from back"
        say "release $to was not ready; the $kind was undone and release $from is running"
      fi ;;
    done)
      finish_transaction
      say "the $kind to $to had finished; the record is closed" ;;
    *) die "the journal holds an unknown step $step; keep $JOURNAL and ask for help" ;;
  esac
}

# require_installed: there is a stack to work on, and nothing is half done.
require_installed() {
  load_facts
  check_password
  [ -f "$STATE" ] || die "nothing is installed here (no $STATE); ./update.sh first-start installs a release"
  if [ "${1:-}" != allow-journal ] && [ -f "$JOURNAL" ]; then
    journal_load || true
    die "a ${J[kind]:-transaction} to ${J[to_tag]:-?} was interrupted (step ${J[step]:-?}); run ./update.sh recover first"
  fi
  [ -s "$POSTGRES_DATA_DIR/PG_VERSION" ] || die "the installed PostgreSQL cluster is missing from $POSTGRES_DATA_DIR; check the mount and ORCH_POSTGRES_DATA_DIR before updating"
  [ -d "$DATA_DIR/update-status" ] || die "the installed server files are missing from $DATA_DIR; check the mount and ORCH_DATA_DIR before updating"
  prepare_dirs
}

# begin_transaction KIND FROM TO: opens the journal with where every link points now.
begin_transaction() {
  local kind
  J=()
  J[id]="$(stamp)-$$"; J[kind]="$1"; J[from_tag]="$2"; J[to_tag]="$3"
  if [ -n "${ORCH_UPDATE_JOB:-}" ]; then J[job]="$ORCH_UPDATE_JOB"; fi
  for kind in $PAYLOADS; do
    J[${kind}_current_before]="$(link_get "$kind" current)"
    J[${kind}_previous_before]="$(link_get "$kind" previous)"
  done
  jset step begin
}

cmd_first_start() {
  local selector=${1:-} kind fingerprint url bind file
  load_facts_for_first_start
  check_password
  if [ -L runtime/server/current ] && [ -f "$STATE" ]; then die "a release is installed already ($(state_get current_tag)); ./update.sh latest or vX.Y.Z updates it"; fi
  if [ -f "$JOURNAL" ]; then journal_load || true; [ "${J[kind]:-}" = first-start ] || die "a ${J[kind]:-transaction} was interrupted; run ./update.sh recover first"; fi
  for file in compose.yml nginx.conf config/orch.toml config/node.toml; do [ -f "$file" ] || die "$file is missing"; done
  prepare_dirs
  seed_node_policy
  fetch_release "$selector"
  J=(); J[id]="$(stamp)-$$"; J[kind]="first-start"; J[from_tag]=""; J[to_tag]="$TAG"; jset step begin
  say "installing release $TAG"
  for kind in $PAYLOADS; do
    unpack "$kind"
    link_set "$kind" current "$UNPACKED"
  done
  ensure_images
  jset step secrets
  ensure_secrets "$DEPLOY_DIR/runtime/cli/current"
  check_installation
  jset step database
  database_start
  if [ -n "$RESTORE_DATABASE" ]; then
    jset step restoring_database
    say "restoring the database from $RESTORE_DATABASE"
    restore_database "$RESTORE_DATABASE"
  fi
  jset step dumping
  take_dump "$BACKUPS/orch-$(stamp)-first-start.dump"
  jset step starting
  app_start
  jset step verifying
  wait_ready "$TAG" || die "the stack did not become ready; docker compose logs shows why, and repeating the original ./update.sh first-start command takes it up again"
  state_save "$TAG" "" ""
  journal_clear
  files_record_first_start
  publish_storage_metadata
  fingerprint="$(authority_fingerprint)"
  url="$(compose exec -T api /opt/orchctl/orchctl public-address get 2>/dev/null)" || url=
  local browser_line="The committed browser address could not be read; the stack is ready."
  if [ -n "$url" ]; then browser_line="Open $url and make the first administrator at /setup."; fi
  bind="$(env_get ORCH_FRONTEND_BIND)"; bind="${bind:-127.0.0.1:5180}"
  cat << EOF

Orch $TAG is running.

  Authority fingerprint (SHA-256): $fingerprint
    A machine that enrolls shows this; compare it before trusting the system.
  $browser_line
  The stack's own web edge listens on http://$bind.
  Back up config/ (it holds the secrets) off this host now; losing config/secrets/orch_master_key or
  orch_ca_key.pem cannot be undone.
EOF
}

# load_facts_for_first_start: .env exists; the facts are written to it if they are not there.
load_facts_for_first_start() {
  [ -f .env ] || die ".env is missing: install -m 600 .env.example .env and set POSTGRES_PASSWORD"
  machine_facts
  load_facts
}

complete_secrets() {
  local file
  for file in $SECRET_FILES; do [ -f "config/secrets/$file" ] || return 1; done
}

# A complete older set can lack the port assertion key. compose-secrets adds it
# without replacing the existing keys, and refuses any partial original set.
ensure_secrets() {
  local cli_dir=$1 secrets_out
  if complete_secrets; then
    say "the secrets in config/secrets are all there"
  else
    say "making or completing the secrets"
    secrets_out="$(docker run --rm --user "$SERVICE_UID:$SERVICE_GID" --network none \
      -v "$DEPLOY_DIR/config:/config" -v "$cli_dir:/cli:ro" "$RUNTIME_IMAGE" \
      /cli/orchctl compose-secrets --secrets-dir /config/secrets --config /config/orch.toml)" || die "orchctl compose-secrets failed"
    printf '%s\n' "$secrets_out"
  fi
  chmod 700 config/secrets
  complete_secrets || die "config/secrets is incomplete after orchctl compose-secrets"
}

# authority_fingerprint: the SHA-256 of the authority's certificate, in hexadecimal.
authority_fingerprint() {
  sed -n '/BEGIN CERTIFICATE/,/END CERTIFICATE/p' config/secrets/orch_ca_cert.pem | grep -v -- '-----' | base64 -d | sha256sum | cut -d' ' -f1
}

# --- Uninstall ----------------------------------------------------------------------

uninstall_words() {
  case "${LC_ALL:-${LC_MESSAGES:-${LANG:-en}}}" in zh*) say "$2" ;; *) say "$1" ;; esac
}

uninstall_size() {
  local bytes
  bytes="$(du -sb -- "$1" 2> /dev/null | cut -f1)" || bytes="unknown"
  say "$bytes B"
}

uninstall_node() {
  compose run --rm --no-deps --entrypoint /opt/orch/bin/orch-node \
    -e "LANG=${LC_ALL:-${LC_MESSAGES:-${LANG:-en}}}" node uninstall --runtime-only "$@"
}

uninstall_backup() {
  local pair dump="orch-final.dump"
  pair="$DEPLOY_DIR/backups/uninstall-$(stamp)-$$"
  [ ! -e "$pair" ] || die "the final backup destination already exists"
  make_dir 700 "$DEPLOY_DIR/backups"
  make_dir 700 "$pair"
  BACKUP_DESTINATION="$pair" backup_command dump --out "/backups/$dump" --records-dir /installation \
    --release "$(state_get current_tag)" --cause manual || die "the final database backup failed; nothing was purged"
  if [ -d "$DATA_DIR" ]; then
    make_dir 700 "$pair/data"
    cp -a -- "$DATA_DIR/." "$pair/data/" || die "the paired blob copy failed; nothing was purged"
  fi
  cp -a -- config "$pair/config" || die "the configuration backup failed; nothing was purged"
  cp -p -- .env "$pair/.env" || die "the environment backup failed; nothing was purged"
  [ ! -d runtime/node ] || cp -a -- runtime/node "$pair/node" || die "the node identity copy failed; nothing was purged"
  printf 'Database: %s\nManifest: %s\nBlobs: data/\nConfiguration: config/ and .env\nNode identity: node/\nExecution volumes: not included; restore their separate backups.\n' \
    "$dump" "${dump%.dump}.manifest.json" > "$pair/CONTENTS.txt"
  say "Final backup: $pair"
}

cmd_uninstall() {
  local name project answer id object size runtime_identity config_identity secrets_identity data_path node_arguments=()
  load_facts
  [ -f "$STATE" ] || die "nothing is installed here"
  [ ! -f "$JOURNAL" ] || die "recover the interrupted update before uninstalling"
  if [ -L runtime ] || [ -L config ] || [ -L config/secrets ]; then die "runtime and secrets must be real installation directories"; fi
  [ -x runtime/cli/current/orchctl ] || die "the installed command line program is unavailable"
  name="$(runtime/cli/current/orchctl system-name --config config/orch.toml)" || die "the system's name cannot be read"
  [ -n "$name" ] || die "the system has no name"
  project="$(compose config --format json | flatten_json | awk -F'\t' '$1 == "name" { print $2; exit }')"
  [[ "$project" =~ ^[a-z0-9][a-z0-9_-]*$ ]] || die "the Compose project identity cannot be read"
  runtime_identity="$(stat -c '%d:%i' runtime)"
  config_identity="$(stat -c '%d:%i' config)"
  secrets_identity="$(stat -c '%d:%i' config/secrets)"
  uninstall_words "Uninstall plan: $name" "卸载计划：$name"
  while IFS=$'\t' read -r id object; do
    [ -n "$id" ] || continue
    size="$(docker container inspect --size --format '{{.SizeRw}}' "$id")" || die "a stack container could not be inspected"
    uninstall_words "  Remove: $object ($size B)" "  移除：$object（$size B）"
  done < <(docker ps -a --filter "label=com.docker.compose.project=$project" --format '{{.ID}}\t{{.Names}}')
  if [ "$PURGE" = 1 ]; then
    node_arguments+=(--purge)
    uninstall_words "  Remove: runtime/ ($(uninstall_size runtime))" "  移除：runtime/（$(uninstall_size runtime)）"
    uninstall_words "  Remove: config/secrets/ ($(uninstall_size config/secrets))" "  移除：config/secrets/（$(uninstall_size config/secrets)）"
    if [ "$NO_BACKUP" = 0 ]; then
      uninstall_words "  Keep: final database, blobs, configuration and node identity in backups/" "  保留：backups/ 中的最终数据库、内容、配置和节点身份副本"
    fi
    uninstall_words "  Host execution volumes are not included in the final backup." "  最终备份不包含主机执行卷。"
  else
    uninstall_words "  Keep: runtime/ and config/secrets/" "  保留：runtime/ 和 config/secrets/"
  fi
  for data_path in "$DATA_DIR" "$POSTGRES_DATA_DIR"; do
    case "$data_path" in "$DEPLOY_DIR/runtime/"*) continue ;; esac
    uninstall_words "  Keep: $data_path" "  保留：$data_path"
  done
  # A file lock is coordination, not inventory: this never starts the node's session.
  ensure_node_lock
  uninstall_node "${node_arguments[@]}" --plan || die "the node's inventory could not be planned"
  [ "$PLAN_ONLY" = 0 ] || return 0
  if [ "$YES" = 0 ]; then
    uninstall_words "Type the system name to confirm: $name" "输入系统名称以确认：$name"
    IFS= read -r answer || die "confirmation was not given; nothing was removed"
    [ "$answer" = "$name" ] || die "the name did not match; nothing was removed"
  fi
  # shellcheck disable=SC2086 # The service names are separate arguments.
  compose stop --timeout "$STOP_TIMEOUT" $APP_SERVICES
  if [ "$PURGE" = 1 ]; then
    if [ "$NO_BACKUP" = 0 ]; then uninstall_backup; fi
    uninstall_node --purge --yes || die "node objects remain; runtime and secrets are kept for a retry"
  fi
  compose down --timeout "$STOP_TIMEOUT" --remove-orphans || die "the stack could not be removed"
  if [ "$PURGE" = 1 ]; then
    if [ -L runtime ] || [ -L config ] || [ -L config/secrets ]; then die "an installation directory was replaced; files are kept"; fi
    if [ "$(stat -c '%d:%i' runtime)" != "$runtime_identity" ] || [ "$(stat -c '%d:%i' config)" != "$config_identity" ] || [ "$(stat -c '%d:%i' config/secrets)" != "$secrets_identity" ]; then die "an installation directory changed identity; files are kept"; fi
    rm -rf -- "$DEPLOY_DIR/runtime" "$DEPLOY_DIR/config/secrets"
  fi
  uninstall_words "The stack is removed." "系统服务已移除。"
}

usage() {
  sed -n '2,/^$/p' "${BASH_SOURCE[0]}" | sed 's/^# \{0,1\}//'
  case "${LC_ALL:-${LC_MESSAGES:-${LANG:-en}}}" in
    zh*)
      say './update.sh uninstall [--purge] [--no-backup] [--plan|--yes]'
      say '先列出计划，再输入系统名称确认。--plan 只预览；--yes 跳过输入名称。'
      say '--purge 清理节点运行时对象后移除 runtime/ 和 config/secrets/。'
      say '默认先在 backups/ 保留数据库、内容、配置和节点身份副本；--no-backup 跳过。'
      say '执行卷不包含在最终备份中。已有配置和 .env 保留。'
      say './update.sh doctor 只运行一次就绪检查（堆栈内和本机访问入口），逐行输出并说明入口地址的来源；任一失败即返回非零。'
      say './update.sh prune 预览未使用发布及大小；--yes 执行移除。'
      say './update.sh dumps remove <name> 移除已核实的转储、清单及配对内容副本。'
      ;;
    *)
      say './update.sh doctor runs the readiness checks once (inside the stack and the edge from this host), prints every line and the address the edge check used and where it came from, and fails if either fails.'
      say './update.sh prune previews unused releases and their sizes; --yes applies.'
      say './update.sh dumps remove <name> removes a verified dump, manifest and paired blob copy.'
      ;;
  esac
}

# --- Main --------------------------------------------------------------------------------------

main() {
  if [ -n "${ORCH_UPDATE_JOB:-}" ]; then
    [[ "$ORCH_UPDATE_JOB" =~ ^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$ ]] || die "the update job identity is invalid"
    local final_stage final_file="runtime/node/server-updates/$ORCH_UPDATE_JOB/status"
    final_stage="$(sed -n 's/^stage=//p' "$final_file" 2> /dev/null || true)"
    case "$final_stage" in
      updated | rolled_back | failed)
        : > "runtime/node/server-updates/$ORCH_UPDATE_JOB/parked"
        # The completed process already ran its ordinary cleanup. The node may now remove its helper.
        sleep infinity
        return ;;
    esac
  fi
  local command="${1:-help}" selector="" argument
  shift || true
  if [ "$command" = dumps ]; then
    [ "${1:-}" = remove ] || die "use ./update.sh dumps remove <name>"
    command=dumps-remove
    shift
  fi
  FROM=""; YES=0; RESTORE_DATABASE=""; REPAIR_RECORD=""; PAIRED_BLOBS=""; PURGE=0; NO_BACKUP=0; PLAN_ONLY=0
  while [ $# -gt 0 ]; do
    argument="$1"
    case "$argument" in
      --from) [ $# -ge 2 ] || die "--from wants a directory"; FROM="$2"; shift 2; continue ;;
      --from=*) FROM="${argument#--from=}" ;;
      --restore-database) [ $# -ge 2 ] || die "--restore-database wants a dump"; RESTORE_DATABASE="$2"; shift 2; continue ;;
      --restore-database=*) RESTORE_DATABASE="${argument#--restore-database=}" ;;
      --repair-record) [ $# -ge 2 ] || die "--repair-record wants a relative dump name"; REPAIR_RECORD="$2"; shift 2; continue ;;
      --repair-record=*) REPAIR_RECORD="${argument#--repair-record=}" ;;
      --paired-blobs) [ $# -ge 2 ] || die "--paired-blobs wants the completed local copy directory"; PAIRED_BLOBS="$2"; shift 2; continue ;;
      --paired-blobs=*) PAIRED_BLOBS="${argument#--paired-blobs=}" ;;
      --purge) PURGE=1 ;;
      --no-backup) NO_BACKUP=1 ;;
      --plan) PLAN_ONLY=1 ;;
      --yes | -y) YES=1 ;;
      -h | --help) usage; exit 0 ;;
      -*) die "unknown option $argument (./update.sh help)" ;;
      *) [ -z "$selector" ] || die "one release at a time: $selector and $argument"; selector="$argument" ;;
    esac
    shift
  done
  case "$command" in help | -h | --help) usage; exit 0 ;; esac
  if [ "$command" != uninstall ] && { [ "$PURGE" = 1 ] || [ "$NO_BACKUP" = 1 ] || [ "$PLAN_ONLY" = 1 ]; }; then die "--purge, --no-backup and --plan are for uninstall only"; fi
  [ "$NO_BACKUP" = 0 ] || [ "$PURGE" = 1 ] || die "--no-backup needs --purge"
  [ "$PLAN_ONLY" = 0 ] || [ "$YES" = 0 ] || die "--plan and --yes are separate choices"
  private_env
  if [ -z "${ORCH_OFFLINE_IMAGE_MAP:-}" ]; then ORCH_OFFLINE_IMAGE_MAP="$(env_get ORCH_OFFLINE_IMAGE_MAP)"; fi
  [ -z "$REPAIR_RECORD" ] || [ "$command" = backup ] || die "--repair-record is for backup only"
  [ -z "$PAIRED_BLOBS" ] || { [ "$command" = backup ] && [ -n "$REPAIR_RECORD" ]; } || die "--paired-blobs needs backup --repair-record"
  if [ -n "$RESTORE_DATABASE" ]; then
    [ "$command" = first-start ] || die "--restore-database is for first-start: it puts a dump in the database before the stack starts"
    [ -s "$RESTORE_DATABASE" ] || die "--restore-database $RESTORE_DATABASE is not a file with a dump in it"
    RESTORE_DATABASE="$(cd "$(dirname "$RESTORE_DATABASE")" && pwd -P)/$(basename "$RESTORE_DATABASE")"
  fi
  check_host

  exec 9> "$DEPLOY_DIR/.orch-update-lock"
  flock -n 9 || die "another update.sh is running here"
  mkdir -p runtime/update 2> /dev/null || true
  exec 8> runtime/update/lock
  flock -n 8 || die "another update.sh is running here"

  WORK="$(mktemp -d "${TMPDIR:-/tmp}/orch-update.XXXXXX")"
  trap 'update_job_exit; rm -rf -- "$WORK"' EXIT
  declare -gA NEW_RELEASE=()
  TOOLBOX_IMAGE="$(image_of nginx)"; RUNTIME_IMAGE="$(image_of debian)"
  if [ -z "$TOOLBOX_IMAGE" ] || [ -z "$RUNTIME_IMAGE" ]; then die "compose.yml names no nginx or debian image"; fi
  TOOLBOX_IMAGE="$(local_image "$TOOLBOX_IMAGE")"; RUNTIME_IMAGE="$(local_image "$RUNTIME_IMAGE")"
  if [ -z "$RELEASE_URL" ]; then RELEASE_URL="$(env_get ORCH_RELEASE_URL)"; fi
  if [ -z "$RELEASE_URL" ] && [ -x runtime/cli/current/orchctl ]; then
    RELEASE_URL="$(runtime/cli/current/orchctl release-source --config config/orch.toml)" \
      || die "the configured release source could not be read"
  fi
  export ORCH_RELEASE_URL="$RELEASE_URL"

  if [ -n "${ORCH_UPDATE_JOB:-}" ]; then
    [[ "$ORCH_UPDATE_JOB" =~ ^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$ ]] || die "the update job identity is invalid"
    case "$command" in v[0-9]*) ;; *) die "a bundled update selects one exact release" ;; esac
    SERVER_UPDATE_TARGET="$command"; SERVER_UPDATE_FROM="$(state_get current_tag)"
    local recorded_stage
    recorded_stage="$(sed -n 's/^stage=//p' "runtime/node/server-updates/$ORCH_UPDATE_JOB/status" 2> /dev/null || true)"
    case "$recorded_stage" in updated | rolled_back | failed) return ;; esac
    if [ -f "$JOURNAL" ]; then
      journal_load || die "the interrupted update journal could not be read"
      [ "${J[job]:-}" = "$ORCH_UPDATE_JOB" ] && [ "${J[to_tag]:-}" = "$command" ] || die "the unfinished update belongs to another operation; run ./update.sh recover"
      SERVER_UPDATE_FROM="${J[from_tag]}"
      update_job_state recovering recovery_required
      cmd_recover
      if [ "$(state_get current_tag)" = "$command" ]; then update_job_state updated
      else update_job_state rolled_back health_check_failed; fi
      return
    fi
    update_job_state queued
  fi

  case "$command" in
    first-start) cmd_first_start "$selector" ;;
    latest) [ -z "$selector" ] || die "latest takes no version"; cmd_update latest ;;
    v[0-9]*) [ -z "$selector" ] || die "$command takes no other release"; cmd_update "$command" ;;
    backup) cmd_backup ;;
    rollback) cmd_rollback ;;
    recover) load_facts; check_password; [ -f "$STATE" ] || [ -f "$JOURNAL" ] || { say "nothing is installed here"; exit 0; }; cmd_recover ;;
    status) cmd_status ;;
    doctor) [ -z "$selector" ] || die "doctor takes no argument"; cmd_doctor ;;
    prune) [ -z "$selector" ] || die "prune takes no release name"; cmd_prune ;;
    dumps-remove) [ -n "$selector" ] || die "dumps remove needs the dump name"; cmd_dumps_remove "$selector" ;;
    uninstall) [ -z "$selector" ] || die "uninstall takes no release"; cmd_uninstall ;;
    *) die "unknown command $command (./update.sh help)" ;;
  esac
  if [ -n "${ORCH_UPDATE_JOB:-}" ]; then
    update_job_state updated
    # Exit normally so the existing temporary-file cleanup runs. Docker restarts
    # the helper into the completed-state wait above until the node removes it.
  fi
}

[ "${BASH_SOURCE[0]}" != "$0" ] || { main "$@"; exit "$?"; }
