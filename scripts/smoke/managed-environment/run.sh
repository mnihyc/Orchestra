#!/usr/bin/env bash
# managed-environment: a release packaged from the tree, started by the updater, and used over HTTP until an agent
# has worked in an environment that was made from a template on the bundled host. Everything is asked as a person
# and an operator ask it (requests through the web edge, the socket through it, the updater, docker to look), and
# every step says what it found.
#
# The stack publishes its pinned listener on the Docker bridge's address at the port it listens on inside (so that
# a machine of the stack's network and a managed unit find it by the name the system advertises), and its public
# address is the web edge's name inside the stack (the node fetches the unit's connector from the address people
# reach the system at). Beside it run a machine of a person (a container of the stack's network, with a connector
# enrolled by a token, which holds the provider and the harness) and the scripted model server (a program on the
# host, on the bridge's address).
set -Eeuo pipefail
# shellcheck source-path=SCRIPTDIR/..
# shellcheck source=lib.sh
. "$(dirname "${BASH_SOURCE[0]}")/../lib.sh"
HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd -P)"

command -v jq > /dev/null || abort "jq is needed"
command -v node > /dev/null || abort "node is needed: the socket is followed with it"

A="$(release_a)"
TAG="$(release_tag "$A")"
new_stack managed-environment 41250 "$A"
WORK="$(dirname "$STACK")"
BRIDGE="$(bridge_address)"
MODEL_PORT=$((EDGE_PORT + 2))
SYSTEM_NAME="orch-smoke-managed-$EDGE_PORT"
LAB="${PROJECT}-lab-machine"
MODEL_PID=""
ENV_ID=""
UNIT=""

# The stack as an operator sets it for a host whose containers can reach the web edge by its name and the gateway
# on the bridge: the system's name (it labels what the node makes, which is how the end of the scenario finds it),
# the public address, and the port of the pinned listener in all the places that name it.
sed -i \
  -e "s|^name = \"Orch\"|name = \"$SYSTEM_NAME\"|" \
  -e "s|^public_url = .*|public_url = \"http://frontend\"|" \
  -e "s|wss://gateway.orch.internal:9443|wss://gateway.orch.internal:$GATEWAY_PORT|g" \
  -e "s|wss://gateway:9443|wss://gateway:$GATEWAY_PORT|g" \
  -e "s|^pinned_bind = .*|pinned_bind = \"[::]:$GATEWAY_PORT\"|" \
  "$STACK/config/orch.toml"
sed -i -e "s|^ORCH_GATEWAY_PUBLISH=.*|ORCH_GATEWAY_PUBLISH=$BRIDGE:$GATEWAY_PORT:$GATEWAY_PORT|" "$STACK/.env"

# Takes down what the scenario made beside the stack and what the node made for it, by exact names.
cleanup() {
  if [ "${ORCH_SMOKE_KEEP:-0}" = 1 ]; then
    say "kept: the machine $LAB, the model server (process $MODEL_PID) and the units of the system $SYSTEM_NAME"
    finish
    return
  fi
  if [ -n "$MODEL_PID" ] && kill -0 "$MODEL_PID" 2> /dev/null; then kill "$MODEL_PID" 2> /dev/null || true; fi
  docker rm -f "$LAB" > /dev/null 2>&1 || true
  # The units the node made for this system, whatever became of the scenario, and their volumes: they are not
  # the stack's, so taking the stack down leaves them.
  local unit volume
  for unit in $(docker ps -a --filter "label=orch.system=$SYSTEM_NAME" --format '{{.Names}}' 2> /dev/null); do
    docker rm -f "$unit" > /dev/null 2>&1 || true
  done
  if [ -n "$ENV_ID" ]; then
    for volume in $(docker volume ls -q 2> /dev/null | grep -F -e "$ENV_ID" -e "${UNIT:-no-unit}" || true); do
      docker volume rm "$volume" > /dev/null 2>&1 || true
    done
  fi
  # The network the node makes for its units is shared by the units of any node on this daemon: it goes only
  # if it is the driver's and nothing is on it.
  if [ "$(docker network inspect orch-env --format '{{index .Labels "orch.driver"}}' 2> /dev/null)" = docker ]; then
    docker network rm orch-env > /dev/null 2>&1 || true
  fi
  finish
}
trap 'cleanup' EXIT

# --- Asking the system ---------------------------------------------------------------------------------------

REPLY_FILE="$WORK/reply.json"
STATUS=""

# api METHOD PATH [BODY]: a request of the signed-in administrator through the web edge. The answer is in
# $REPLY_FILE and its status in $STATUS. A token that has run out is replaced once.
api() {
  local method="$1" path="$2" body="${3:-}" attempt args
  mkdir -p "$WORK"
  for attempt in 1 2; do
    args=(-sS --max-time 60 -o "$REPLY_FILE" -w '%{http_code}' -X "$method" "$EDGE/api/v1$path" -H "authorization: Bearer $TOKEN")
    if [ -n "$body" ]; then args+=(-H 'content-type: application/json' -d "$body"); fi
    STATUS="$(curl "${args[@]}")" || STATUS=000
    if [ "$STATUS" = 401 ] && [ "$attempt" = 1 ]; then sign_in; continue; fi
    break
  done
  case "$STATUS" in 2??) return 0 ;; *) return 1 ;; esac
}

# call METHOD PATH [BODY]: as api, prints the answer; says what was refused on standard error.
call() {
  api "$@" || printf 'HTTP %s %s %s: %s\n' "$STATUS" "$1" "$2" "$(head -c 500 "$REPLY_FILE")" >&2
  cat "$REPLY_FILE"
  case "$STATUS" in 2??) return 0 ;; *) return 1 ;; esac
}

# as TOKEN COMMAND...: the command with another person's token.
as() { local saved="$TOKEN"; TOKEN="$1"; shift; local code=0; "$@" || code=$?; TOKEN="$saved"; return "$code"; }

# --- 1. The release is started by the updater ----------------------------------------------------------------

step "the release is started by the updater and the services report"
if update_sh first-start --from "$A"; then pass "first-start finished"; else fail "first-start finished"; fi
check "orchctl doctor passes in the API's container" doctor --expect-version "${TAG#v}"
# A port of the stack can be held for a moment by a connection somebody else made (the ports a program is given
# for its own end of a connection are these too); the updater goes on without the web edge then, so it is asked
# for again.
edge_up() { compose up -d --no-deps frontend > /dev/null 2>&1; curl -fsS "$EDGE/health" > /dev/null 2>&1; }
wait_for 90 "the web edge answers" edge_up

step "the first administrator, through /setup"
if setup_admin; then pass "POST /api/v1/setup made the first administrator"; else fail "POST /api/v1/setup"; fi

# --- 2. The bundled host is online and usable --------------------------------------------------------------------

step "the bundled host is online, and the Default template is there"
wait_for 240 "the node is online" node_online
# A host is online before its facts are in: the node tells what drivers it has once its session is up.
host_has_a_driver() { [ "$(call GET /hosts | jq -r '.[0].drivers[0].name')" = docker ]; }
wait_for 120 "the host has told which drivers it has" host_has_a_driver
HOSTS="$(call GET /hosts)"
HOST_ID="$(printf '%s' "$HOSTS" | jq -r '.[0].id')"
expect "there is one host, the node's" "1" "$(printf '%s' "$HOSTS" | jq 'length')"
expect "it reports the Docker driver" "docker" "$(printf '%s' "$HOSTS" | jq -r '.[0].drivers[0].name')"
TEMPLATE_ID="$(call GET /templates | jq -r '[.[] | select(.name == "Default")][0].id')"
check "the Default template is listed" test -n "$TEMPLATE_ID" -a "$TEMPLATE_ID" != null
expect "its page names everyone for it, as the system's own host does" "everyone" \
  "$(call GET "/hosts/$HOST_ID" | jq -r '[.access.through[].holder.kind][0]')"

step "a person who is no administrator may use that host, and may not manage it"
BEA='bea'
BEA_PASSWORD='correct horse battery staple'
RESET="$(call POST /admin/users "{\"username\":\"$BEA\",\"display_name\":\"Bea\"}" | jq -r .reset_url)"
if curl -fsS -X POST "$EDGE/api/v1/auth/recovery/confirm" -H 'content-type: application/json' \
  -d "{\"token\":\"${RESET##*/reset/}\",\"password\":\"$BEA_PASSWORD\"}" > /dev/null; then pass "Bea set her password with the link"; else fail "Bea set her password with the link"; fi
BEA_TOKEN="$(curl -fsS -X POST "$EDGE/api/v1/auth/login" -H 'content-type: application/json' \
  -d "{\"login\":\"$BEA\",\"password\":\"$BEA_PASSWORD\"}" | jq -r .access_token)"
expect "she is listed the host, with the right to view and use it and no more" "host.use host.view" \
  "$(as "$BEA_TOKEN" call GET /hosts | jq -r '.[0].access.permissions | sort | join(" ")')"
expect "she is listed the Default template" "$TEMPLATE_ID" "$(as "$BEA_TOKEN" call GET /templates | jq -r '[.[] | select(.name == "Default")][0].id')"
BEA_ORG="$(as "$BEA_TOKEN" call GET /me | jq -r .personal_org_id)"
BEA_PROJECT="$(as "$BEA_TOKEN" call POST "/orgs/$BEA_ORG/projects" '{"name":"Bea project"}' | jq -r .id)"
BEA_TOPIC="$(as "$BEA_TOKEN" call POST "/projects/$BEA_PROJECT/topics" '{"name":"Bea topic"}' | jq -r .id)"
expect "the host is a place for an environment of hers" "true" \
  "$(as "$BEA_TOKEN" call GET "/topics/$BEA_TOPIC/places" | jq -r '[.[] | select(.kind == "host" and .host.id == "'"$HOST_ID"'")][0].usable')"
refuses "she may not change it" as "$BEA_TOKEN" api PATCH "/hosts/$HOST_ID" '{"labels":["hers"],"expected_revision":1}'
expect "and is told she may not" "403" "$STATUS"

# --- 3. An org, a project, a topic, the scripted model server and a provider on a key -------------------------

step "an org, a project and a topic"
ORG="$(call POST /orgs '{"name":"Acme","slug":"acme"}' | jq -r .id)"
PROJECT_ID="$(call POST "/orgs/$ORG/projects" '{"name":"Web"}' | jq -r .id)"
TOPIC="$(call POST "/projects/$PROJECT_ID/topics" '{"name":"Landing page"}' | jq -r .id)"
check "they were made" test -n "$ORG" -a -n "$PROJECT_ID" -a -n "$TOPIC"

step "the scripted model server, and a machine of the person that holds the provider"
(cd "$SMOKE_REPO" && cargo build -q -p orch-testkit --bin orch-mock-llm) || abort "orch-mock-llm was not built"
MOCK="${CARGO_TARGET_DIR:-$SMOKE_REPO/target}/debug/orch-mock-llm"
mkdir -p "$WORK/lab/data"
MODEL_KEY="$(openssl rand -hex 16)"
printf '%s\n' "$MODEL_KEY" > "$WORK/lab/model.key"; chmod 600 "$WORK/lab/model.key"
"$MOCK" --listen "$BRIDGE:$MODEL_PORT" --credential-file "$WORK/lab/model.key" --model mock-large --model mock-small \
  > "$WORK/lab/model.log" 2>&1 &
MODEL_PID=$!
model_answers() { curl -fsS -H "authorization: Bearer $MODEL_KEY" "http://$BRIDGE:$MODEL_PORT/v1/models" > /dev/null; }
model_wants_its_key() { ! curl -fsS "http://$BRIDGE:$MODEL_PORT/v1/models" > /dev/null 2>&1; }
wait_for 20 "the scripted model server answers with the key" model_answers
check "and does not without it" model_wants_its_key
CONNECTOR="$(find "$A" -maxdepth 1 -name 'orch-connector-linux-*-musl' | head -n 1)"
printf '[connector]\nroles = ["agent", "provider"]\n' > "$WORK/lab/data/config.toml"
docker run -d --name "$LAB" --network "${PROJECT}_control" --user "$(id -u):$(id -g)" \
  -v "$WORK/lab/data:/data" -v "$CONNECTOR:/usr/local/bin/orch-connector:ro" \
  debian:bookworm-slim@sha256:7c7b2c966bc9ee8cedfeef67e0e279108992c77681fa595db4a9d65c06ccc587 sleep infinity > /dev/null || abort "the machine of the person did not start"
MACHINE_TOKEN="$(call POST /machines/enrollment-tokens '{"name_hint":"lab"}' | jq -r .token)"
lab() { docker exec "$LAB" orch-connector --data-dir /data "$@"; }
if lab enroll --url "https://gateway.orch.internal:$GATEWAY_PORT" --token "$MACHINE_TOKEN" --name lab > "$WORK/lab/enroll.log" 2>&1; then
  pass "the connector enrolled at the pinned listener by the name the system advertises"
else
  fail "the connector enrolled"; cat "$WORK/lab/enroll.log" >&2
fi
lab policy set provider.upstreams "[\"http://$BRIDGE:$MODEL_PORT/v1\"]" > /dev/null
docker exec -d "$LAB" sh -c 'exec orch-connector --data-dir /data run > /data/run.log 2>&1'
machine_online() { [ "$(call GET /machines | jq -r '[.[] | select(.name == "lab")][0].online')" = true ]; }
wait_for 60 "the machine is online" machine_online
LAB_ID="$(call GET /machines | jq -r '[.[] | select(.name == "lab")][0].id')"
# A machine is online before its facts are in, and its provider is registered against what they say its policy lets it
# forward: the answer to a provider asked for too soon is a refusal.
lab_has_told_its_policy() { [ "$(call GET /machines | jq -r '[.[] | select(.name == "lab")][0].facts.policy["provider.upstreams"] | length')" -ge 1 ]; }
wait_for 60 "the machine has told the system what it may forward" lab_has_told_its_policy

step "a provider on a key whose upstream is the scripted model server"
check "the organisation allows endpoints without TLS, as the scripted server has" api PUT "/nodes/$ORG/settings/providers.allow_http_upstreams" '{"value":true}'
PROVIDER="$(call POST "/machines/$LAB_ID/providers" "$(jq -cn --arg org "$ORG" --arg base "http://$BRIDGE:$MODEL_PORT/v1" \
  '{kind:"api",home_id:$org,name:"Scripted model",upstreams:[{protocol:"openai-responses",base_url:$base,options:{}}]}')" | jq -r .id)"
check "the provider was registered from the machine" test -n "$PROVIDER" -a "$PROVIDER" != null
expect "it waits for a key" "needs_setup" "$(call GET "/providers/$PROVIDER" | jq -r .status)"
check "its key was given to the system, which keeps it sealed" api PUT "/providers/$PROVIDER/key" "$(jq -cn --arg key "$MODEL_KEY" '{api_key:$key}')"
SUGGESTED="$(call GET "/providers/$PROVIDER/models/suggestions" | jq -r '[.[].model_name] | sort | join(",")')"
expect "the models the scripted server lists come back through the machine, which opened the key the system sealed to it" \
  "mock-large,mock-small" "$SUGGESTED"
check "one is added with its rate" api POST "/providers/$PROVIDER/models" \
  '{"model_name":"mock-large","display_name":"Mock Large","parameters":{"rate":{"input_per_mtok":"2.50","cached_input_per_mtok":"0.25","output_per_mtok":"15"}}}'
expect "the provider is ready" "ready" "$(call GET "/providers/$PROVIDER" | jq -r .status)"
AGENT="$(call GET /agents | jq -r '[.[] | select(.harness_kind == "fake")][0].id')"
check "the scripted agent's profile is there" test -n "$AGENT" -a "$AGENT" != null

# --- 4. An environment from the Default template on the bundled host ------------------------------------------

step "POST /environments from the Default template on the bundled host"
MADE="$(call POST /environments "$(jq -cn --arg home "$PROJECT_ID" --arg template "$TEMPLATE_ID" --arg host "$HOST_ID" \
  '{name:"Main",home_node_id:$home,template_id:$template,host_id:$host}')")"
ENV_ID="$(printf '%s' "$MADE" | jq -r .id)"
JOB_ID="$(printf '%s' "$MADE" | jq -r '.job.id // empty')"
check "the environment was made and a job runs" test -n "$ENV_ID" -a "$ENV_ID" != null -a -n "$JOB_ID"
expect "it is being provisioned" "provisioning" "$(printf '%s' "$MADE" | jq -r .status)"
deadline=$((SECONDS + 300))
while [ "$SECONDS" -lt "$deadline" ]; do
  JOB="$(call GET "/jobs/$JOB_ID")" || break
  case "$(printf '%s' "$JOB" | jq -r .state)" in succeeded | failed | cancelled) break ;; esac
  sleep 1
done
expect "the job succeeded" "succeeded" "$(printf '%s' "$JOB" | jq -r .state)"
expect "it went through the phases of making a unit, in order" "schedule image volumes tools create enroll attach verify commit" \
  "$(printf '%s' "$JOB" | jq -r '[.progress.phases[].name] | join(" ")')"
expect "and every one is done" "done" "$(printf '%s' "$JOB" | jq -r '[.progress.phases[].state] | unique | join(" ")')"
ENV="$(call GET "/environments/$ENV_ID")"
expect "the environment is running" "running" "$(printf '%s' "$ENV" | jq -r .status)"
expect "it is its first unit" "1" "$(printf '%s' "$ENV" | jq -r .generation)"
expect "it is on the bundled host" "$HOST_ID" "$(printf '%s' "$ENV" | jq -r .host.id)"

step "a container is on the daemon, and the connector in it is the release's"
UNIT="$(docker ps --filter "label=orch.system=$SYSTEM_NAME" --format '{{.Names}}' | head -n 1)"
check "a container of the system is running" test -n "$UNIT"
expect "it is the only one" "1" "$(docker ps -q --filter "label=orch.system=$SYSTEM_NAME" | wc -l | tr -d ' ')"
expect "it starts the connector that is mounted in it" "/opt/orch/bin/orch-connector" "$(docker inspect "$UNIT" --format '{{index .Config.Entrypoint 0}}')"
RELEASE_HASH="$(sha256sum "$CONNECTOR" | cut -d' ' -f1)"
UNIT_HASH="$(docker exec "$UNIT" sha256sum /opt/orch/bin/orch-connector | cut -d' ' -f1)"
expect "that program is the connector of the release, byte for byte" "$RELEASE_HASH" "$UNIT_HASH"
check "its workspace is the volume of the environment" bash -c "docker inspect '$UNIT' --format '{{range .Mounts}}{{.Name}} {{end}}' | grep -q 'orch-ws-$ENV_ID'"
expect "the connector in it enrolled and reports the environment to the system" "linux" "$(printf '%s' "$ENV" | jq -r .facts.os)"

# --- 5. A thread, a turn, stop and start, removal ------------------------------------------------------------------

step "a thread bound to that environment, and a turn of the scripted agent through the web edge"
THREAD="$(call POST "/topics/$TOPIC/threads" "$(jq -cn --arg agent "$AGENT" --arg provider "$PROVIDER" --arg env "$ENV_ID" \
  '{name:"In a container",binding:{agent_id:$agent,provider_id:$provider},model:"mock-large",environment:{use:$env},directory:"web"}')" | jq -r .id)"
check "the thread was made on a profile whose harness had never run" test -n "$THREAD" -a "$THREAD" != null
NODE="$(call GET "/nodes/$THREAD")"
expect "its harness runs with the provider and its tools are carried out in the environment" "separated" "$(printf '%s' "$NODE" | jq -r .binding.arrangement)"
expect "its directory is below the workspace of the unit" "/workspace/web" "$(printf '%s' "$NODE" | jq -r .cwd)"
SCRIPT=$'#!fake\nwhere\nask command ls\nrun ls => 0 web\nedit web/notes.txt +3 -0\nsay done'
TURN="$WORK/turn.jsonl"
if ORCH_TOKEN="$TOKEN" timeout 200 node "$HERE/follow.mjs" --api "$EDGE/api/v1" --ws "ws://127.0.0.1:$EDGE_PORT/api/v1/ws" \
  --thread "$THREAD" --approve once --timeout 180 -- "$SCRIPT" > "$TURN"; then pass "the turn completed"; else fail "the turn completed"; cat "$TURN" >&2; fi
check "the agent says where it works: in the unit's workspace" jq -e 'select(.event == "item" and .kind == "agent_message" and .summary == "directory: /workspace/web, cwd: /workspace/web")' "$TURN"
check "it asked to run a command and the template's way of approving let it" jq -e 'select(.event == "item" and .kind == "approval" and .status == "approved")' "$TURN"
check "the command is in the timeline with its output" jq -e 'select(.event == "item" and .kind == "tool_call" and .summary == "ls -> exit 0 web")' "$TURN"
check "so is the change of a file" jq -e 'select(.event == "item" and .kind == "tool_call" and .summary == "modify web/notes.txt")' "$TURN"
check "and the agent's last word" jq -e 'select(.event == "item" and .kind == "agent_message" and .summary == "done")' "$TURN"
# The Default template also offers the executor of Codex, which a unit has no build of: that one is refused, and
# is not what this is about.
tool_sets_refused() {
  compose logs --no-color api 2> /dev/null | sed 's/\x1b\[[0-9;]*m//g' \
    | grep -c -E 'an executor could not be started executor=(claude-code|plain) ' || true
}
expect "the tool sets the template offers were started as the executor that carries them, and none was refused" "0" "$(tool_sets_refused)"

step "the environment is stopped and started again, and what was written in its workspace is still there"
docker exec "$UNIT" sh -c 'mkdir -p /workspace/web && echo "written before the stop" > /workspace/web/notes.txt'
check "the stop was accepted" api POST "/environments/$ENV_ID/stop" '{"interrupt":false}'
container_stopped() { [ "$(docker inspect -f '{{.State.Running}}' "$UNIT")" = false ]; }
wait_for 60 "the container is stopped" container_stopped
expect "the environment says stopped" "stopped" "$(call GET "/environments/$ENV_ID" | jq -r .status)"
START_JOB="$(call POST "/environments/$ENV_ID/start" | jq -r .id)"
check "the start is a job" test -n "$START_JOB" -a "$START_JOB" != null
environment_running() { [ "$(call GET "/environments/$ENV_ID" | jq -r .status)" = running ]; }
wait_for 180 "the environment is running again" environment_running
expect "it is the same unit" "$UNIT" "$(docker ps --filter "label=orch.system=$SYSTEM_NAME" --format '{{.Names}}' | head -n 1)"
expect "the file is still there" "written before the stop" "$(docker exec "$UNIT" cat /workspace/web/notes.txt)"
expect "the generation did not move" "1" "$(call GET "/environments/$ENV_ID" | jq -r .generation)"

step "the environment is removed, and its container goes"
refuses "an environment with a thread bound to it is not removed" api DELETE "/environments/$ENV_ID?confirm=Main"
expect "and the reason is told" "resource.in_use" "$(jq -r .code "$REPLY_FILE")"
check "the topic is deleted, and its thread with it" api DELETE "/nodes/$TOPIC"
check "the removal is accepted" api DELETE "/environments/$ENV_ID?confirm=Main"
container_gone() { [ -z "$(docker ps -aq --filter "label=orch.system=$SYSTEM_NAME")" ]; }
wait_for 120 "the container is gone" container_gone
volumes_gone() { [ -z "$(docker volume ls -q | grep -F -e "$ENV_ID" -e "$UNIT" || true)" ]; }
wait_for 60 "so are its volumes" volumes_gone
expect "the errors the API logged in all this" "0" "$(compose logs --no-color api 2> /dev/null | sed 's/\x1b\[[0-9;]*m//g' | grep -c ' ERROR ' || true)"
