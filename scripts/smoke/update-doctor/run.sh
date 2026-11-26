#!/usr/bin/env bash
# update-doctor: `./update.sh doctor` and the wait of an update build the same two checks. With no stack: update.sh runs in
# a directory of its own against a stand-in for docker and for orchctl, which say what they were asked, so that what the
# updater asks of them, prints, returns and leaves behind can be looked at, and so can the wait of an update (wait_ready)
# that goes through the same code.
set -Eeuo pipefail
# shellcheck source-path=SCRIPTDIR/..
# shellcheck source=lib.sh
. "$(dirname "${BASH_SOURCE[0]}")/../lib.sh"

WORK="$(mktemp -d "${TMPDIR:-/tmp}/orch-update-doctor.XXXXXX")"
STACK="$WORK/stack"
done_with() { rm -rf -- "$WORK"; rmdir -- "$SMOKE_DIR" 2> /dev/null || true; printf '\n%s: %d checks, %d failed\n' "$(basename "$0")" "$CHECKED" "$FAILED"; [ "$FAILED" -eq 0 ]; }
trap 'done_with' EXIT

mkdir -p "$WORK/bin" "$STACK/config" "$STACK/runtime/cli/current"
# What `docker compose` is asked for, answered as the stack would: the services, and the checks inside the API's container.
cat > "$WORK/bin/docker" << 'STANDIN'
#!/usr/bin/env bash
echo "docker $*" >> "${FAKE_LOG:-/dev/null}"
case "$1 $2" in "compose version" | "info ") exit 0 ;; esac
case "$*" in
  *"config --services"*)
    if [ -n "${FAKE_NODE:-}" ]; then printf 'postgres\napi\nnode\nfrontend\n'; else printf 'postgres\napi\nfrontend\n'; fi
    exit 0 ;;
  *"exec -T api /opt/orchctl/orchctl doctor"*)
    echo "inner: $*"
    echo "ok    api          the API answers (release ${FAKE_RELEASE:-0.1.3})"
    [ "${FAKE_INNER_EXIT:-0}" = 0 ] || echo "FAIL  worker       no heartbeat"
    exit "${FAKE_INNER_EXIT:-0}" ;;
  *" ps"*) echo "stand-in ps"; exit 0 ;;
esac
exit 0
STANDIN
cat > "$STACK/runtime/cli/current/orchctl" << 'STANDIN'
#!/usr/bin/env bash
[ "$1" != release-source ] || exit 0
echo "edge: $*"
echo "ok    edge-page    the page is served (release ${FAKE_RELEASE:-0.1.3} (build abc))"
[ "${FAKE_EDGE_EXIT:-0}" = 0 ] || echo "FAIL  edge-api     502"
exit "${FAKE_EDGE_EXIT:-0}"
STANDIN
chmod +x "$WORK/bin/docker" "$STACK/runtime/cli/current/orchctl"
cp "$SMOKE_REPO/deploy/update.sh" "$STACK/update.sh"
printf 'image: nginx:1\nimage: debian:1\n' > "$STACK/compose.yml"
printf 'POSTGRES_PASSWORD=x\n' > "$STACK/.env"; chmod 600 "$STACK/.env"
printf '[system]\npublic_url = "https://orch.example.org"\n' > "$STACK/config/orch.toml"

# doctor [NAME=VALUE...]: ./update.sh doctor with the stand-ins, its output in OUT and its status in CODE.
doctor() { OUT="$(cd "$STACK" && env PATH="$WORK/bin:$PATH" FAKE_LOG="$WORK/docker.log" "$@" ./update.sh doctor 2>&1)" && CODE=0 || CODE=$?; }
has() { grep -F -- "$1" <<< "$OUT" > /dev/null; }

step "the address of the edge check, and where it came from"
doctor ORCH_READY_URL=https://edge.env.example
expect "both checks pass: exit 0" 0 "$CODE"
check "the address and its source are named: the environment" has 'edge address: https://edge.env.example (from ORCH_READY_URL in the environment)'
check "the edge check went to that address, and to the three edge checks only" has 'edge: doctor --url https://edge.env.example --config config/orch.toml --secrets-dir config/secrets --only edge-page,edge-api,edge-link'
check "it says every check passed" has 'doctor: every check passed'
printf 'POSTGRES_PASSWORD=x\nORCH_READY_URL=https://edge.dotenv.example\n' > "$STACK/.env"
doctor ORCH_READY_URL=
check ".env names it when the environment does not" has 'edge address: https://edge.dotenv.example (from ORCH_READY_URL in .env)'
printf 'POSTGRES_PASSWORD=x\n' > "$STACK/.env"
doctor ORCH_READY_URL=
check "public_url names it when nothing else does" has 'edge address: https://orch.example.org (from [system] public_url in config/orch.toml)'
cp "$STACK/config/orch.toml" "$WORK/orch.toml.kept"; printf '[system]\n' > "$STACK/config/orch.toml"
doctor ORCH_READY_URL=
check "no address at all fails" test "$CODE" != 0
check "and says what to set" has 'public_url or ORCH_READY_URL'
cp "$WORK/orch.toml.kept" "$STACK/config/orch.toml"

step "the inside check, built once for doctor and for the wait of an update"
doctor ORCH_READY_URL=https://e.example
check "a Compose file with no node leaves the node out" has 'inner: compose exec -T api /opt/orchctl/orchctl doctor --skip node'
# The list of services is read whole: a reader that left at its first match made the node look absent now and then.
absent=0
for _ in $(seq 1 40); do
  doctor ORCH_READY_URL=https://e.example FAKE_NODE=1
  if has '--skip node'; then absent=$((absent + 1)); fi
done
expect "a Compose file with a node never has it left out (40 tries)" 0 "$absent"

step "a failed check fails doctor, and hides nothing of the other"
doctor ORCH_READY_URL=https://e.example FAKE_INNER_EXIT=1
check "the inside check failing: non-zero" test "$CODE" != 0
check "its failing line is printed" has 'FAIL  worker       no heartbeat'
check "the edge check ran anyway" has 'edge: doctor --url https://e.example'
doctor ORCH_READY_URL=https://e.example FAKE_EDGE_EXIT=1
check "the edge check failing: non-zero" test "$CODE" != 0
check "its failing line is printed" has 'FAIL  edge-api     502'
mv "$STACK/runtime/cli/current/orchctl" "$STACK/runtime/cli/current/orchctl.away"
doctor ORCH_READY_URL=https://e.example
check "no orchctl on this host: non-zero" test "$CODE" != 0
check "and it says why" has 'runtime/cli/current/orchctl is not installed here'
mv "$STACK/runtime/cli/current/orchctl.away" "$STACK/runtime/cli/current/orchctl"

step "doctor changes nothing"
rm -f "$WORK/docker.log"
doctor ORCH_READY_URL=https://e.example
check "no journal and no state were written" bash -c "test ! -e '$STACK/runtime/update/journal' && test ! -e '$STACK/runtime/update/state'"
check "nothing was started, stopped, built or pulled" bash -c "! grep -E 'compose (up|stop|down|start|run|build|pull|restart)' '$WORK/docker.log'"
refuses "it takes no argument" bash -c "cd '$STACK' && env PATH='$WORK/bin:$PATH' ./update.sh doctor now"
check "its usage block lists doctor" says "./update.sh doctor" bash -c "cd '$STACK' && env PATH='$WORK/bin:$PATH' ./update.sh help"
for locale in en zh_CN; do
  help_text=$(LANG="$locale" LC_ALL= LC_MESSAGES= "$STACK/update.sh" help)
  check "the updater introduces its help first ($locale)" test "${help_text%%$'\n'*}" = "$(sed -n '2s/^# //p' "$STACK/update.sh")"
done

step "the wait of an update goes through the same code and keeps its version check"
sed '$d' "$STACK/update.sh" > "$STACK/wait.sh"
# shellcheck disable=SC2016 # the call is written into the copy, which expands it
printf 'wait_ready "$1"\n' >> "$STACK/wait.sh"
chmod +x "$STACK/wait.sh"
wait_for_release() { (cd "$STACK" && env PATH="$WORK/bin:$PATH" ORCH_READY_URL=https://e.example ORCH_READY_TIMEOUT=4 "$@" ./wait.sh v0.1.3 2>&1); }
OUT="$(wait_for_release FAKE_RELEASE=0.1.3)" && CODE=0 || CODE=$?
expect "it passes when the edge serves the release" 0 "$CODE"
check "its inside check expects the release" has 'inner: compose exec -T api /opt/orchctl/orchctl doctor --expect-version 0.1.3 --skip node'
OUT="$(wait_for_release FAKE_RELEASE=0.1.2)" && CODE=0 || CODE=$?
check "it does not pass while the edge serves another release" test "$CODE" != 0
check "and says so" has 'did not serve release 0.1.3'
