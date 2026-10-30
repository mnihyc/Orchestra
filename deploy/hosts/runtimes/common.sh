# shellcheck shell=bash
# Shared by kata.sh and gvisor.sh: what an administrator's runtime script prints, and the one way each changes the daemon's
# configuration. Sourced, never run.

CHANGED=()
KEPT=()

say() { printf '%s\n' "$*"; }
changed() { CHANGED+=("$*"); say "changed: $*"; }
kept() { KEPT+=("$*"); say "already so: $*"; }

die() { printf 'error: %s\n' "$*" >&2; exit 1; }

# Whether this run only says what it would do.
DRY_RUN=0
for argument in "$@"; do
  case "$argument" in
    --dry-run) DRY_RUN=1 ;;
    -h|--help) usage; exit 0 ;;
    *) die "unknown option $argument (the options are --dry-run and --help)" ;;
  esac
done

need_root() {
  if [ "$(id -u)" != 0 ] && [ "$DRY_RUN" = 0 ]; then
    die "run this as root (an administrator's act): it changes the host's Docker configuration"
  fi
}

need() {
  command -v "$1" >/dev/null 2>&1 || die "$1 is needed and was not found"
}

# do_it <what> <command...>: runs the command, or only says it where this is a dry run.
do_it() {
  local what=$1
  shift
  if [ "$DRY_RUN" = 1 ]; then
    say "would change: $what"
    return 0
  fi
  "$@"
  changed "$what"
}

DOCKER_CONFIG_FILE=${DOCKER_CONFIG_FILE:-/etc/docker/daemon.json}

# register_runtime <name> <json of the runtime's entry>: makes the daemon's configuration list the runtime as given; nothing
# else in the file changes. Says so where it was so already.
register_runtime() {
  local name=$1 entry=$2
  need python3
  if [ ! -f "$DOCKER_CONFIG_FILE" ]; then
    if [ "$DRY_RUN" = 1 ]; then say "would change: $DOCKER_CONFIG_FILE is written with the runtime $name"; return 0; fi
    mkdir -p "$(dirname "$DOCKER_CONFIG_FILE")"
    printf '{}\n' >"$DOCKER_CONFIG_FILE"
  fi
  local result
  result=$(python3 - "$DOCKER_CONFIG_FILE" "$name" "$entry" "$DRY_RUN" <<'PY'
import json, sys
path, name, entry, dry = sys.argv[1], sys.argv[2], json.loads(sys.argv[3]), sys.argv[4] == "1"
with open(path) as handle:
    text = handle.read()
config = json.loads(text) if text.strip() else {}
runtimes = config.setdefault("runtimes", {})
if runtimes.get(name) == entry:
    print("same")
else:
    runtimes[name] = entry
    if not dry:
        with open(path + ".orch-new", "w") as handle:
            json.dump(config, handle, indent=2, sort_keys=True)
            handle.write("\n")
    print("changed")
PY
)
  if [ "$result" = same ]; then
    kept "the runtime $name is registered in $DOCKER_CONFIG_FILE"
    return 0
  fi
  if [ "$DRY_RUN" = 1 ]; then say "would change: the runtime $name is registered in $DOCKER_CONFIG_FILE"; return 0; fi
  mv "$DOCKER_CONFIG_FILE.orch-new" "$DOCKER_CONFIG_FILE"
  changed "the runtime $name is registered in $DOCKER_CONFIG_FILE"
  RELOAD_DOCKER=1
}

# reload_docker: asks the daemon to read its configuration again, which makes it list a new runtime without stopping a
# container; a daemon that does not take the reload is restarted only when --restart was given.
reload_docker() {
  [ "${RELOAD_DOCKER:-0}" = 1 ] || return 0
  if command -v systemctl >/dev/null 2>&1 && systemctl is-active --quiet docker; then
    if systemctl reload docker; then
      changed "Docker was asked to read its configuration again (systemctl reload docker)"
    else
      say "Docker did not take the reload: run 'systemctl restart docker' when its containers may restart"
    fi
  else
    say "Docker is not run by systemd here: send it SIGHUP or restart it so that it lists the new runtime"
  fi
}

summary() {
  say ""
  if [ "$DRY_RUN" = 1 ]; then
    say "dry run: nothing was changed"
  elif [ "${#CHANGED[@]}" = 0 ]; then
    say "nothing changed: this host was already set up"
  else
    say "${#CHANGED[@]} change(s) made"
  fi
}
