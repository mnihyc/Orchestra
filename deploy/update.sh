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
#   ./update.sh prune [--yes]
#   ./update.sh dumps remove <name> [--yes]
#   ./update.sh uninstall [--purge] [--no-backup] [--plan|--yes]
#
# Readiness checks the operator's public_url from this host as well as the
# services inside the stack. ORCH_READY_URL may select the reachable edge URL
# when this host uses a different route to the same deployment.
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
# A link is replaced by one rename, so it is never half made. compose.yml,
# nginx.conf, .env, config/orch.toml and the secrets are never changed by an
# update. The password file's permissions are checked on every run.
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
service_gateway_key.pem service_worker_cert.pem service_worker_key.pem"

DEPLOY_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd -P)"
cd "$DEPLOY_DIR"
JOURNAL="runtime/update/journal"
STATE="runtime/update/state"
BACKUPS="runtime/update-backups"

# --- Messages ----------------------------------------------------------------

say() { printf '%s\n' "$*"; }
note() { printf 'update.sh: %s\n' "$*" >&2; }
die() { printf 'update.sh: %s\n' "$*" >&2; exit 1; }
stamp() { date -u +%Y%m%dT%H%M%SZ; }

need() { command -v "$1" > /dev/null 2>&1 || die "$1 is needed and is not installed"; }

# --- The host ------------------------------------------------------------------

check_host() {
  local tool
  for tool in docker awk sed grep tar flock stat mv ln readlink sha256sum base64 mktemp sort cut head tr date id sleep fold wc du find cp chmod chown; do
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
  socket="${DOCKER_SOCKET:-/var/run/docker.sock}"
  [ -S "$socket" ] || die "no Docker socket at $socket; the node needs it (set DOCKER_SOCKET if it is elsewhere)"
  docker_gid="$(stat -c '%g' "$socket")"
  [ -n "$(env_get ORCH_HOME)" ] || block+="ORCH_HOME=$DEPLOY_DIR"$'\n'
  [ -n "$(env_get ORCH_UID)" ] || block+="ORCH_UID=$uid"$'\n'
  [ -n "$(env_get ORCH_GID)" ] || block+="ORCH_GID=$gid"$'\n'
  [ -n "$(env_get ORCH_DOCKER_GID)" ] || block+="ORCH_DOCKER_GID=$docker_gid"$'\n'
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
  [ ! -L "$path" ] && { [ ! -e "$path" ] || [ -f "$path" ]; } || die "the node instance lock is not a real file"
  (umask 077; : >> "$path")
  [ "$(id -u)" -ne 0 ] || chown "$SERVICE_UID:$SERVICE_GID" "$path"
  chmod 600 "$path"
}

prepare_dirs() {
  local kind
  private_env
  make_dir 700 runtime
  make_dir 700 runtime/postgres
  make_dir 700 runtime/postgres/data
  make_dir 750 runtime/data
  make_dir 755 runtime/data/update-status
  make_dir 750 runtime/data/frontend-tls
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
  fi
  say "checking release $TAG against version.json"
  verify_release "$RELEASE_DIR"
}

# --- Unpacking -------------------------------------------------------------------------

# toolbox ARGS...: runs a command of the web edge's image (busybox) as the
# services' user, with the release and the staging directory mounted.
toolbox() {
  docker run --rm --user "$SERVICE_UID:$SERVICE_GID" --network none --entrypoint "$1" \
    -v "$RELEASE_DIR:/in:ro" -v "$STAGE_PARENT:/out" "$TOOLBOX_IMAGE" "${@:2}"
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
      if [ -z "${ORCH_UPDATE_NO_UNZIP:-}" ] && command -v unzip > /dev/null 2>&1; then unzip -q -o "$RELEASE_DIR/$asset" -d "$STAGE_PARENT/$stage"
      else toolbox unzip -q "/in/$asset" -d "/out/$stage"; fi
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
  local index=0 reference digest present selected
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

# --- The stack ---------------------------------------------------------------------------

# shellcheck disable=SC2086 # a list of service names, split on purpose
app_stop() { compose stop -t "$STOP_TIMEOUT" $STOP_SERVICES > /dev/null; }

# app_start: creates the application services again, so that they see the
# releases that the links point at now. A service that does not become healthy
# makes Compose say so and fail; that is for the readiness check to judge, with
# the whole picture, and not a reason to stop here.
app_start() {
  check_installation
  # shellcheck disable=SC2086 # a list of service names, split on purpose
  if ! compose up -d --quiet-pull --force-recreate --no-deps $APP_SERVICES > /dev/null; then
    note "docker compose could not bring every service up healthy; the readiness check says which"
  fi
}

database_start() {
  compose up -d --quiet-pull --wait postgres > /dev/null || die "the database did not become healthy; docker compose logs postgres"
}

# wait_ready TAG: the services and the operator's web edge serve the release.
wait_ready() {
  local version="${1#v}" deadline=$((SECONDS + READY_TIMEOUT)) next=$((SECONDS + 15)) output edge_output edge_url fails skip=()
  edge_url="${ORCH_READY_URL:-$(env_get ORCH_READY_URL)}"
  if [ -z "$edge_url" ]; then
    edge_url="$(sed -n 's/^public_url[[:space:]]*=[[:space:]]*"\(.*\)".*/\1/p' config/orch.toml | head -n 1)"
  fi
  if [ -z "$edge_url" ]; then
    note "readiness needs [system] public_url or ORCH_READY_URL for the operator's web edge"
    return 1
  fi
  compose config --services | grep -qx node || skip=(--skip node)
  while :; do
    if output="$(compose exec -T api /opt/orchctl/orchctl doctor --expect-version "$version" ${skip[@]+"${skip[@]}"} 2>&1)"; then
      if edge_output="$(runtime/cli/current/orchctl doctor --url "$edge_url" \
        --config config/orch.toml --secrets-dir config/secrets \
        --only edge-page,edge-api,edge-link 2>&1)"; then
        # The edge doctor reports the API version it actually reached. It may
        # reach a stale upstream even while the internal API runs this release.
        if printf '%s\n' "$edge_output" | grep -Fq "release $version (build "; then
          printf '%s\n%s\n' "$output" "$edge_output"
          return 0
        fi
        edge_output+=$'\n'"FAIL  edge-version     $edge_url did not serve release $version"
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
      --release "$(state_get current_tag)" --cause "${2:-pre-update}" || die "database backup failed; a completed pair, if any, can repair its Overview record without another dump"
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

# sql QUERY: one value or a few rows from the database, or nothing if it cannot be asked.
sql() { compose exec -T postgres psql -U orch -d orch -At -F'|' -c "$1" 2> /dev/null || true; }

# --- What an update does to running work -------------------------------------------------

# warn_about_work: tells what the new release does to what is running, and sets
# NEEDS_ANSWER when it said anything, for the caller to ask whether to go on.
warn_about_work() {
  local running units kind count lines="" environments=0 providers=0
  running="$(sql "SELECT count(*) FROM threads WHERE state IN ('starting','running','waiting_approval')")"
  units="$(sql "SELECT r.kind, count(*) FROM runtimes r JOIN machines m ON m.id = r.machine_id
                 WHERE r.state IN ('creating','running','stopped','stopping')
                   AND string_to_array(split_part(m.connector_version, '-', 1), '.')::int[]
                       < string_to_array(split_part('$CONNECTOR_MINIMUM', '-', 1), '.')::int[]
                 GROUP BY r.kind")"
  while IFS='|' read -r kind count; do
    case "$kind" in environment) environments="$count" ;; provider) providers="$count" ;; esac
  done <<< "$units"
  if [ "$environments" -gt 0 ] || [ "$providers" -gt 0 ]; then
    lines+="$environments environments and $providers provider runtimes will need an update before they work again (release $TAG no longer accepts their connector).
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
  local apply="${1:-1}" keep kind current previous name age retained directory identity bytes marker
  STORAGE_KEEP_COUNT=""
  if ! keep="$(release_keep_count)"; then return 0; fi
  STORAGE_KEEP_COUNT="$keep"
  [ "$keep" != 0 ] || return 0
  for kind in $PAYLOADS; do
    directory="$DEPLOY_DIR/runtime/$kind/releases"
    [ -d "$DEPLOY_DIR/runtime" ] && [ ! -L "$DEPLOY_DIR/runtime" ] || continue
    [ -d "$DEPLOY_DIR/runtime/$kind" ] && [ ! -L "$DEPLOY_DIR/runtime/$kind" ] || continue
    [ -d "$directory" ] && [ ! -L "$directory" ] || continue
    identity="$(stat -c '%d:%i' -- "$directory")"
    current="$(link_get "$kind" current)"; previous="$(link_get "$kind" previous)"
    retained=0
    while IFS=$'\t' read -r -d '' age name; do
      case "$name" in "$current" | "$previous") continue ;; esac
      case "$name" in .*) continue ;; esac
      # Only whole payloads that the updater published are eligible; links and staging paths stay.
      [[ "$name" =~ ^[0-9A-Za-z][0-9A-Za-z._-]*$ ]] || continue
      [ -f "$directory/$name/.complete" ] && [ ! -L "$directory/$name/.complete" ] && [ ! -L "$directory/$name" ] || continue
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
          [ ! -L "$DEPLOY_DIR/runtime" ] && [ ! -L "$DEPLOY_DIR/runtime/$kind" ] && [ ! -L "$directory" ] || die "the release directory changed; review prune again"
          [ "$(stat -c '%d:%i' -- "$directory")" = "$identity" ] || die "the release directory changed; review prune again"
          [ "$name" != "$(link_get "$kind" current)" ] && [ "$name" != "$(link_get "$kind" previous)" ] || continue
          [ ! -L "$directory/$name" ] && [ ! -L "$directory/$name/.complete" ] && [ "$(cat "$directory/$name/.complete")" = "$marker" ] || die "the release changed; review prune again"
          say "$(uninstall_words Removing 正在移除) $kind/$name ($bytes $(uninstall_words bytes 字节))"
          rm -rf -- "$directory/$name"
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
  app_stop
  jset step dumping
  take_dump "${J[leaving_dump]}"
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
    return 0
  fi
  if undo_with_database; then
    die "release $to did not become ready; $from is running again as it was"
  fi
  die "release $to did not become ready, and $from could not be brought back either"
}

# --- Commands ------------------------------------------------------------------------------

cmd_update() {
  local selector=$1 kind
  require_installed
  fetch_release "$selector"
  [ "$TAG" != "$(state_get current_tag)" ] || { say "release $TAG is installed already; nothing to do"; return 0; }
  say "installing release $TAG over $(state_get current_tag)"
  for kind in $PAYLOADS; do unpack "$kind"; NEW_RELEASE[$kind]="$UNPACKED"; done
  ensure_images
  warn_about_work
  [ "$NEEDS_ANSWER" -eq 0 ] || confirm "Go on?"
  begin_transaction update "$(state_get current_tag)" "$TAG"
  for kind in $PAYLOADS; do
    J[${kind}_current_after]="${NEW_RELEASE[$kind]}"
    J[${kind}_previous_after]="${J[${kind}_current_before]}"
  done
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
  warn_about_work
  confirm "Roll back to $back?"
  begin_transaction rollback "$(state_get current_tag)" "$back"
  for kind in $PAYLOADS; do
    J[${kind}_current_after]="${J[${kind}_previous_before]}"
    J[${kind}_previous_after]="${J[${kind}_current_before]}"
  done
  J[leaving_dump]="$BACKUPS/orch-$(stamp)-before-rollback-from-$(state_get current_tag).dump"
  J[entering_dump]="$dump"
  jset step staged
  transact
  say "release $back is running again."
}

# load_manifest_of_installed TAG: for a rollback the version to ask for is the one
# that is going to be entered; there is no manifest to read, and no connector
# minimum to warn about.
load_manifest_of_installed() { TAG="$1"; CONNECTOR_MINIMUM="0.0.0"; }

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
}

cmd_recover() {
  local step kind from to
  journal_load || { say "no transaction is open; nothing to recover"; return 0; }
  step="${J[step]}"; kind="${J[kind]}"; from="${J[from_tag]}"; to="${J[to_tag]}"
  say "an interrupted $kind to $to is recorded; it stopped at step $step"
  if [ "$kind" = first-start ]; then
    say "a first start can be taken up again: run ./update.sh first-start; the journal is cleared"
    journal_clear
    return 0
  fi
  require_installed allow-journal
  case "$step" in
    begin | staged | stopping | dumping | activating)
      # No service has run the new release, and the database is as it was: the
      # links go back, and the old release runs.
      note "the new release never ran; starting release $from as it was"
      deactivate_payloads
      rm -f -- "${J[leaving_dump]}.partial"
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
  prepare_dirs
}

# begin_transaction KIND FROM TO: opens the journal with where every link points now.
begin_transaction() {
  local kind
  J=()
  J[id]="$(stamp)-$$"; J[kind]="$1"; J[from_tag]="$2"; J[to_tag]="$3"
  for kind in $PAYLOADS; do
    J[${kind}_current_before]="$(link_get "$kind" current)"
    J[${kind}_previous_before]="$(link_get "$kind" previous)"
  done
  jset step begin
}

cmd_first_start() {
  local selector=${1:-} kind fingerprint url bind file secrets_out
  load_facts_for_first_start
  check_password
  if [ -L runtime/server/current ] && [ -f "$STATE" ]; then die "a release is installed already ($(state_get current_tag)); ./update.sh latest or vX.Y.Z updates it"; fi
  if [ -f "$JOURNAL" ]; then journal_load || true; [ "${J[kind]:-}" = first-start ] || die "a ${J[kind]:-transaction} was interrupted; run ./update.sh recover first"; fi
  for file in compose.yml nginx.conf config/orch.toml; do [ -f "$file" ] || die "$file is missing"; done
  prepare_dirs
  fetch_release "$selector"
  J=(); J[id]="$(stamp)-$$"; J[kind]=first-start; J[from_tag]=; J[to_tag]="$TAG"; jset step begin
  say "installing release $TAG"
  for kind in $PAYLOADS; do
    unpack "$kind"
    link_set "$kind" current "$UNPACKED"
  done
  ensure_images
  jset step secrets
  if complete_secrets; then say "the secrets in config/secrets are all there"; else
    say "making the secrets"
    secrets_out="$(docker run --rm --user "$SERVICE_UID:$SERVICE_GID" --network none \
      -v "$DEPLOY_DIR/config:/config" -v "$DEPLOY_DIR/runtime/cli/current:/cli:ro" "$RUNTIME_IMAGE" \
      /cli/orchctl compose-secrets --secrets-dir /config/secrets --config /config/orch.toml)" || die "orchctl compose-secrets failed"
    printf '%s\n' "$secrets_out"
  fi
  chmod 700 config/secrets
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
  wait_ready "$TAG" || die "the stack did not become ready; docker compose logs shows why, and ./update.sh first-start takes it up again"
  state_save "$TAG" "" ""
  journal_clear
  publish_storage_metadata
  fingerprint="$(authority_fingerprint)"
  url="$(sed -n 's/^public_url[[:space:]]*=[[:space:]]*"\(.*\)".*/\1/p' config/orch.toml | head -n 1)"
  bind="$(env_get ORCH_FRONTEND_BIND)"; bind="${bind:-127.0.0.1:5180}"
  cat << EOF

Orch $TAG is running.

  Authority fingerprint (SHA-256): $fingerprint
    A machine that enrolls shows this; compare it before trusting the system.
  Open $url (the stack's own web edge listens on http://$bind),
  and make the first administrator at /setup.
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
  local pair="$DEPLOY_DIR/backups/uninstall-$(stamp)-$$" dump="orch-final.dump"
  [ ! -e "$pair" ] || die "the final backup destination already exists"
  make_dir 700 "$DEPLOY_DIR/backups"
  make_dir 700 "$pair"
  BACKUP_DESTINATION="$pair" backup_command dump --out "/backups/$dump" --records-dir /installation \
    --release "$(state_get current_tag)" --cause manual || die "the final database backup failed; nothing was purged"
  if [ -d runtime/data ]; then
    make_dir 700 "$pair/data"
    cp -a -- runtime/data/. "$pair/data/" || die "the paired blob copy failed; nothing was purged"
  fi
  cp -a -- config "$pair/config" || die "the configuration backup failed; nothing was purged"
  cp -p -- .env "$pair/.env" || die "the environment backup failed; nothing was purged"
  [ ! -d runtime/node ] || cp -a -- runtime/node "$pair/node" || die "the node identity copy failed; nothing was purged"
  printf 'Database: %s\nManifest: %s\nBlobs: data/\nConfiguration: config/ and .env\nNode identity: node/\nExecution volumes: not included; restore their separate backups.\n' \
    "$dump" "${dump%.dump}.manifest.json" > "$pair/CONTENTS.txt"
  say "Final backup: $pair"
}

cmd_uninstall() {
  local name project answer id object size runtime_identity config_identity secrets_identity node_arguments=()
  load_facts
  [ -f "$STATE" ] || die "nothing is installed here"
  [ ! -f "$JOURNAL" ] || die "recover the interrupted update before uninstalling"
  [ ! -L runtime ] && [ ! -L config ] && [ ! -L config/secrets ] || die "runtime and secrets must be real installation directories"
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
  # A file lock is coordination, not inventory: this never starts the node's session.
  ensure_node_lock
  uninstall_node "${node_arguments[@]}" --plan || die "the node's inventory could not be planned"
  [ "$PLAN_ONLY" = 0 ] || return 0
  if [ "$YES" = 0 ]; then
    uninstall_words "Type the system name to confirm: $name" "输入系统名称以确认：$name"
    IFS= read -r answer || die "confirmation was not given; nothing was removed"
    [ "$answer" = "$name" ] || die "the name did not match; nothing was removed"
  fi
  compose stop --timeout "$STOP_TIMEOUT" $APP_SERVICES
  if [ "$PURGE" = 1 ]; then
    if [ "$NO_BACKUP" = 0 ]; then uninstall_backup; fi
    uninstall_node --purge --yes || die "node objects remain; runtime and secrets are kept for a retry"
  fi
  compose down --timeout "$STOP_TIMEOUT" --remove-orphans || die "the stack could not be removed"
  if [ "$PURGE" = 1 ]; then
    [ ! -L runtime ] && [ ! -L config ] && [ ! -L config/secrets ] || die "an installation directory was replaced; files are kept"
    [ "$(stat -c '%d:%i' runtime)" = "$runtime_identity" ] && [ "$(stat -c '%d:%i' config)" = "$config_identity" ] && [ "$(stat -c '%d:%i' config/secrets)" = "$secrets_identity" ] || die "an installation directory changed identity; files are kept"
    rm -rf -- "$DEPLOY_DIR/runtime" "$DEPLOY_DIR/config/secrets"
  fi
  uninstall_words "The stack is removed." "系统服务已移除。"
}

usage() {
  case "${LC_ALL:-${LC_MESSAGES:-${LANG:-en}}}" in
    zh*)
      say './update.sh uninstall [--purge] [--no-backup] [--plan|--yes]'
      say '先列出计划，再输入系统名称确认。--plan 只预览；--yes 跳过输入名称。'
      say '--purge 清理节点运行时对象后移除 runtime/ 和 config/secrets/。'
      say '默认先在 backups/ 保留数据库、内容、配置和节点身份副本；--no-backup 跳过。'
      say '执行卷不包含在最终备份中。已有配置和 .env 保留。'
      say './update.sh prune 预览未使用发布及大小；--yes 执行移除。'
      say './update.sh dumps remove <name> 移除已核实的转储、清单及配对内容副本。'
      ;;
    *)
      say './update.sh prune previews unused releases and their sizes; --yes applies.'
      say './update.sh dumps remove <name> removes a verified dump, manifest and paired blob copy.'
      ;;
  esac
  sed -n '2,/^$/p' "${BASH_SOURCE[0]}" | sed 's/^# \{0,1\}//'
}

# --- Main --------------------------------------------------------------------------------------

main() {
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
  trap 'rm -rf -- "$WORK"' EXIT
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

  case "$command" in
    first-start) cmd_first_start "$selector" ;;
    latest) [ -z "$selector" ] || die "latest takes no version"; cmd_update latest ;;
    v[0-9]*) [ -z "$selector" ] || die "$command takes no other release"; cmd_update "$command" ;;
    backup) cmd_backup ;;
    rollback) cmd_rollback ;;
    recover) load_facts; check_password; [ -f "$STATE" ] || [ -f "$JOURNAL" ] || { say "nothing is installed here"; exit 0; }; cmd_recover ;;
    status) cmd_status ;;
    prune) [ -z "$selector" ] || die "prune takes no release name"; cmd_prune ;;
    dumps-remove) [ -n "$selector" ] || die "dumps remove needs the dump name"; cmd_dumps_remove "$selector" ;;
    uninstall) [ -z "$selector" ] || die "uninstall takes no release"; cmd_uninstall ;;
    *) die "unknown command $command (./update.sh help)" ;;
  esac
}

main "$@"
