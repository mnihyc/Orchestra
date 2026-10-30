#!/usr/bin/env bash
# shipped-image: the image every default environment is made from, run as the driver runs a unit (an init process, the account the template
# names, the home and the workspace on volumes) and looked at from inside, with no server and no release: root through sudo without a
# password and what a template that sets no_new_privileges does to it; the browser and the command line that drives it, with fonts for
# Chinese, Japanese, Korean and emoji, and what the browser's own sandbox does under Docker's defaults; a display started on a free X display
# with the options a display is started with, a viewer that connects to it, and a browser with a window that shows there.
#
# The image is the one named by ORCH_SMOKE_IMAGE when there is one of that name; otherwise it is built from the recipe by `docker build` alone
# (ORCH_SMOKE_NO_CACHE=1 builds it without Docker's cache) under that name, orch-env-base:smoke-shipped-image when none is given, and removed
# at the end.
set -Eeuo pipefail
# shellcheck source-path=SCRIPTDIR/..
# shellcheck source=lib.sh
. "$(dirname "${BASH_SOURCE[0]}")/../lib.sh"
HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd -P)"

command -v docker > /dev/null || abort "docker is needed"

NAME="${ORCH_SMOKE_PROJECT_PREFIX:-orchsmoke$SMOKE_RUN}shipped-image"
UNIT="$NAME-unit"
LOCKED="$NAME-locked"
OPEN="$NAME-open"
VOLUME_WORKSPACE="$NAME-workspace"
VOLUME_HOME="$NAME-home"
WORK="$SMOKE_DIR/shipped-image"
# Where the scenario's files are put inside the unit.
UNIT_WORK=/tmp/shipped-image
# The account, and where a unit keeps its two volumes: what the Default template says.
UNIT_USER=1000:1000
BUILT_IMAGE=""

cleanup() {
  if [ "${ORCH_SMOKE_KEEP:-0}" = 1 ]; then
    say "kept: the containers $UNIT, $LOCKED and $OPEN, the volumes $VOLUME_WORKSPACE and $VOLUME_HOME and the directory $WORK"
  else
    docker rm -f "$UNIT" "$LOCKED" "$OPEN" > /dev/null 2>&1 || true
    docker volume rm "$VOLUME_WORKSPACE" "$VOLUME_HOME" > /dev/null 2>&1 || true
    if [ -n "$BUILT_IMAGE" ]; then docker rmi "$BUILT_IMAGE" > /dev/null 2>&1 || true; fi
    rm -rf "${WORK:?}"
  fi
  printf '\n%s: %d checks, %d failed\n' "$(basename "$0")" "$CHECKED" "$FAILED"
  if [ "$FAILED" -ne 0 ]; then exit 1; fi
}

for existing in "$UNIT" "$LOCKED" "$OPEN"; do
  if docker container inspect "$existing" > /dev/null 2>&1; then abort "the container $existing is there already"; fi
done
for existing in "$VOLUME_WORKSPACE" "$VOLUME_HOME"; do
  if docker volume inspect "$existing" > /dev/null 2>&1; then abort "the volume $existing is there already"; fi
done
rm -rf "${WORK:?}"
mkdir -p "$WORK/evidence"
trap 'cleanup' EXIT

# --- The image ---------------------------------------------------------------------------------------------------------------------

IMAGE="${ORCH_SMOKE_IMAGE:-orch-env-base:smoke-shipped-image}"
if ! docker image inspect "$IMAGE" > /dev/null 2>&1; then
  step "docker build of the recipe, alone${ORCH_SMOKE_NO_CACHE:+, without the cache}"
  # shellcheck disable=SC2086 # the cache option is one word or nothing
  if docker build --tag "$IMAGE" --build-arg VERSION=smoke ${ORCH_SMOKE_NO_CACHE:+--no-cache} "$SMOKE_REPO/images/orch-env-base" > "$WORK/build.log" 2>&1; then
    BUILT_IMAGE="$IMAGE"
    pass "the image builds from its recipe with no argument but the version"
  else
    tail -n 30 "$WORK/build.log" >&2
    abort "docker build failed (the log is $WORK/build.log)"
  fi
fi
docker image inspect "$IMAGE" > /dev/null 2>&1 || abort "there is no image $IMAGE"
say "image: $IMAGE ($(docker image inspect --format '{{.Id}}' "$IMAGE"))"

# label NAME: the value of a label of the image.
label() { docker image inspect --format "{{index .Config.Labels \"$1\"}}" "$IMAGE"; }
NODE_VERSION="$(label orch.image.node)"
PLAYWRIGHT_CLI_VERSION="$(label orch.image.playwright-cli)"
PLAYWRIGHT_VERSION="$(label orch.image.playwright)"
BROWSER_VERSION="$(label orch.image.browser.version)"
BROWSER_REVISION="$(label orch.image.browser.revision)"
TIGERVNC_VERSION="$(label orch.image.tigervnc)"
OPENBOX_VERSION="$(label orch.image.openbox)"

# --- A unit ------------------------------------------------------------------------------------------------------------------------

# make_unit NAME ARGS...: a container the way the Docker driver makes a unit: an init process, the account of the template, a working directory
# that is the root. Only the connector is not started: nothing here talks to a server.
make_unit() {
  local name="$1"; shift
  docker run --detach --name "$name" --init --ulimit core=0:-1 --user "$UNIT_USER" --workdir / "$@" "$IMAGE" sleep infinity > /dev/null
}
step "a unit made as the Default template makes one"
make_unit "$UNIT" --mount "type=volume,src=$VOLUME_WORKSPACE,dst=/workspace" --mount "type=volume,src=$VOLUME_HOME,dst=/home/agent"
# A template that sets no_new_privileges.
make_unit "$LOCKED" --security-opt no-new-privileges
# A container that allows what the browser's own sandbox needs, which Docker's default profile does not: not what the Default template makes,
# but it shows that the sandbox is the container's to give and the image does not stand in its way.
make_unit "$OPEN" --security-opt seccomp=unconfined
wait_for 30 "the units are running" docker exec "$UNIT" true
docker exec "$LOCKED" true
docker exec "$OPEN" true

# unit SAYS: a command in the unit, as its user.
unit() { docker exec "$UNIT" "$@"; }
expect "the unit's user is uid 1000" 1000 "$(unit id -u)"
check "the home directory is a volume" docker exec "$UNIT" mountpoint -q /home/agent
check "the workspace is a volume" docker exec "$UNIT" mountpoint -q /workspace

step "what the image says it holds is what it holds"
expect "Node.js is the version of its label" "v$NODE_VERSION" "$(unit node --version)"
expect "the command line is the version of its label" "$PLAYWRIGHT_CLI_VERSION" "$(unit playwright-cli --version)"
expect "the Playwright it depends on is the version of its label" "$PLAYWRIGHT_VERSION" \
  "$(unit node -p "require('/usr/local/lib/node_modules/@playwright/cli/node_modules/playwright-core/package.json').version")"
expect "the browser is the version of its label" "Google Chrome for Testing $BROWSER_VERSION" \
  "$(unit sh -c '/opt/ms-playwright/chromium-*/chrome-linux*/chrome --version' | sed 's/ *$//')"
check "its directory is the revision of its label ($BROWSER_REVISION)" docker exec "$UNIT" sh -c "test -x /opt/ms-playwright/chromium-$BROWSER_REVISION/chrome-linux*/chrome"
expect "the VNC server is the version of its label" "$TIGERVNC_VERSION" "$(unit sh -c 'Xvnc -version 2>&1' | sed -n 's/^Xvnc TigerVNC \([0-9.]*\) .*/\1/p')"
expect "the window manager is the version of its label" "$OPENBOX_VERSION" "$(unit openbox --version | sed -n 's/^Openbox //p;1q')"
check "what the image held before is still there (shell, git, search, compiler, Python, curl)" \
  docker exec "$UNIT" bash -c 'git --version && rg --version && gcc --version && python3 --version && curl --version && jq --version'
check "the image names itself and its version for whoever reads its labels" test -n "$(label org.opencontainers.image.title)" -a -n "$(label org.opencontainers.image.version)"

# --- What runs in the unit -----------------------------------------------------------------------------------------------------------

GOT_DONE=0
# relay: counts as the scenario's own what a script of the unit prints, a line for each thing: PASS, FAIL, SKIP, INFO and at the end DONE.
relay() {
  local line
  GOT_DONE=0
  while IFS= read -r line; do
    case "$line" in
      "PASS  "*) pass "${line#PASS  }" ;;
      "FAIL  "*) fail "${line#FAIL  }" ;;
      "SKIP  "*) say "SKIP  ${line#SKIP  }" ;;
      "INFO  "*) say "      ${line#INFO  }" ;;
      DONE) GOT_DONE=1 ;;
      *) say "      $line" ;;
    esac
  done
}
# in_unit CONTAINER SCRIPT ARGS...: puts a script of this directory into the container, runs it there as the container's user and counts what it
# says. (It is a file there, not the standard input of a shell, so that no program it starts can read the rest of it.)
in_unit() {
  local container="$1" script="$2"; shift 2
  docker exec "$container" mkdir -p "$UNIT_WORK"
  docker cp "$HERE/$script" "$container:$UNIT_WORK/$script"
  relay < <(timeout 900 docker exec --workdir /tmp "$container" bash "$UNIT_WORK/$script" "$@" 2>&1 || true)
  if [ "$GOT_DONE" != 1 ]; then fail "$script did not run to its end"; fi
}

step "root without a password"
expect "the image licence label refers to its bundled component notices" "LicenseRef-Orch-Env-Base" "$(label org.opencontainers.image.licenses)"
in_unit "$UNIT" unit-notices.sh
in_unit "$UNIT" unit-root.sh given
step "root taken away by a template that sets no_new_privileges"
in_unit "$LOCKED" unit-root.sh taken-away

step "the browser and the command line that drives it"
unit mkdir -p "$UNIT_WORK"
docker cp "$HERE/rfb.py" "$UNIT:$UNIT_WORK/rfb.py"
in_unit "$UNIT" unit-browser.sh "$UNIT_WORK" "$BROWSER_VERSION" "$BROWSER_REVISION"

step "the browser's own sandbox, where the container allows it"
in_unit "$OPEN" unit-sandbox.sh "$UNIT_WORK"

step "a display"
in_unit "$UNIT" unit-display.sh "$UNIT_WORK" "$TIGERVNC_VERSION" "$OPENBOX_VERSION"

docker cp "$UNIT:$UNIT_WORK/evidence/." "$WORK/evidence/" > /dev/null 2>&1 || true
if [ "${ORCH_SMOKE_KEEP:-0}" = 1 ]; then say "pictures of what was drawn: $WORK/evidence"; fi
