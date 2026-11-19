#!/usr/bin/env bash
# managed-harness: a release packaged from the tree, started by the updater, and used over HTTP until each of the two
# real harnesses has worked in a unit made from the Default template on the bundled host. The builds come from the
# release's own list (the vendors' addresses and hashes), are downloaded and checked by the host's node, and are seen
# read-only in the unit by the connector that runs there; the key stays with a provider on another machine, which the
# harness in the unit reaches through the system. The model is the scripted model server; with ORCH_HARNESS_REAL_ENV
# naming a file that holds OPENROUTER_API_KEY, each harness also makes one small request to a real model.
#
# The stack is the one managed-environment makes (the pinned listener on the Docker bridge's address at the port it
# listens on inside, the system's own name, the web edge's name as the public address), beside it a machine of a person
# that holds only the provider role, and the scripted model server on the host.
set -Eeuo pipefail
# shellcheck source-path=SCRIPTDIR/..
# shellcheck source=lib.sh
. "$(dirname "${BASH_SOURCE[0]}")/../lib.sh"
HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd -P)"
# The page that follows a turn on the socket is the one managed-environment uses.
FOLLOW="$HERE/../managed-environment/follow.mjs"

command -v jq > /dev/null || abort "jq is needed"
command -v node > /dev/null || abort "node is needed: the socket is followed with it"

A="$(release_a)"
TAG="$(release_tag "$A")"
new_stack managed-harness 41300 "$A"
WORK="$(dirname "$STACK")"
BRIDGE="$(bridge_address)"
MODEL_PORT=$((EDGE_PORT + 2))
SYSTEM_NAME="orch-smoke-harness-$EDGE_PORT"
LAB="${PROJECT}-lab-machine"
MODEL_PID=""
ENV_ID=""
UNIT=""
TODAY="$(date -u +%F)"

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
  local unit volume provider
  for unit in $(docker ps -a --filter "label=orch.system=$SYSTEM_NAME" --format '{{.Names}}' 2> /dev/null); do
    docker rm -f "$unit" > /dev/null 2>&1 || true
    docker volume rm "$unit-run" > /dev/null 2>&1 || true
  done
  if [ -n "$ENV_ID" ]; then
    for volume in $(docker volume ls -q 2> /dev/null | grep -F -e "$ENV_ID" -e "${UNIT:-no-unit}" || true); do
      docker volume rm "$volume" > /dev/null 2>&1 || true
    done
  fi
  for provider in "${SUBSCRIPTION_PROVIDERS[@]-}"; do
    if [ -n "$provider" ]; then docker volume rm "orch-state-$provider" > /dev/null 2>&1 || true; fi
  done
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

# --- 1. The release is started by the updater ----------------------------------------------------------------

step "the release lists a build of each harness for Linux on this processor"
ARCH="$(uname -m)"
case "$ARCH" in aarch64 | arm64) ARCH=aarch64 ;; *) ARCH=x86_64 ;; esac
listed() { jq -r --arg kind "$1" --arg arch "$ARCH" '[.harness_builds[] | select(.kind == $kind and .os == "linux" and .arch == $arch and .is_default)][0] | .'"$2" "$A/version.json"; }
CODEX_VERSION="$(listed codex version)"
CODEX_SHA="$(listed codex sha256)"
CLAUDE_VERSION="$(listed claude-code version)"
CLAUDE_SHA="$(listed claude-code sha256)"
check "Codex is listed, with an address on a host that publishes it and a hash" test -n "$CODEX_VERSION" -a "$CODEX_VERSION" != null -a "${#CODEX_SHA}" = 64
check "Claude Code is listed, with an address on a host that publishes it and a hash" test -n "$CLAUDE_VERSION" -a "$CLAUDE_VERSION" != null -a "${#CLAUDE_SHA}" = 64

step "the release is started by the updater and the services report"
if update_sh first-start --from "$A"; then pass "first-start finished"; else fail "first-start finished"; fi
check "orchctl doctor passes in the API's container" doctor --expect-version "${TAG#v}"
edge_up() { compose up -d --no-deps frontend > /dev/null 2>&1; curl -fsS "$EDGE/health" > /dev/null 2>&1; }
wait_for 90 "the web edge answers" edge_up

step "the first administrator, and the bundled host with the Default template"
if setup_admin; then pass "POST /api/v1/setup made the first administrator"; else fail "POST /api/v1/setup"; fi
wait_for 240 "the node is online" node_online
# A host is online before its facts are in: the node tells what drivers it has once its session is up.
host_has_a_driver() { [ "$(call GET /hosts | jq -r '.[0].drivers[0].name')" = docker ]; }
wait_for 120 "the host has told which drivers it has" host_has_a_driver
HOSTS="$(call GET /hosts)"
HOST_ID="$(printf '%s' "$HOSTS" | jq -r '.[0].id')"
TEMPLATE_ID="$(call GET /templates | jq -r '[.[] | select(.name == "Default")][0].id')"
check "there is a host and the Default template is listed" test -n "$HOST_ID" -a "$HOST_ID" != null -a -n "$TEMPLATE_ID" -a "$TEMPLATE_ID" != null
KINDS="$(call GET /harness-kinds)"
expect "the system lists the default build of Codex it will put in a unit" "$CODEX_VERSION" \
  "$(printf '%s' "$KINDS" | jq -r '[.[] | select(.kind == "codex")][0].builds[] | select(.is_default) | .version')"
expect "and that of Claude Code" "$CLAUDE_VERSION" \
  "$(printf '%s' "$KINDS" | jq -r '[.[] | select(.kind == "claude-code")][0].builds[] | select(.is_default) | .version')"

# --- 2. An org, a project, a topic, the scripted model server and a provider that keeps the key -------------------

step "an org, a project and a topic"
ORG="$(call POST /orgs '{"name":"Acme","slug":"acme"}' | jq -r .id)"
PROJECT_ID="$(call POST "/orgs/$ORG/projects" '{"name":"Web"}' | jq -r .id)"
TOPIC="$(call POST "/projects/$PROJECT_ID/topics" '{"name":"Landing page"}' | jq -r .id)"
check "they were made" test -n "$ORG" -a -n "$PROJECT_ID" -a -n "$TOPIC"

step "the scripted model server, and a machine of the person that holds the provider and nothing else"
(cd "$SMOKE_REPO" && cargo build -q -p orch-testkit --bin orch-mock-llm) || abort "orch-mock-llm was not built"
MOCK="${CARGO_TARGET_DIR:-$SMOKE_REPO/target}/debug/orch-mock-llm"
mkdir -p "$WORK/lab/data"
MODEL_KEY="$(openssl rand -hex 16)"
printf '%s\n' "$MODEL_KEY" > "$WORK/lab/model.key"; chmod 600 "$WORK/lab/model.key"
"$MOCK" --listen "$BRIDGE:$MODEL_PORT" --credential-file "$WORK/lab/model.key" --model mock-large --model mock-small \
  > "$WORK/lab/model.log" 2>&1 &
MODEL_PID=$!
model_answers() { curl -fsS -H "authorization: Bearer $MODEL_KEY" "http://$BRIDGE:$MODEL_PORT/v1/models" > /dev/null; }
wait_for 20 "the scripted model server answers with the key" model_answers
CONNECTOR="$(find "$A" -maxdepth 1 -name 'orch-connector-linux-*-musl' | head -n 1)"
printf '[connector]\nroles = ["provider"]\n' > "$WORK/lab/data/config.toml"
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
# The upstreams the person lets this machine's provider reach: the scripted server in both formats, and the real
# interfaces for the step that asks for them.
lab policy set provider.upstreams "[\"http://$BRIDGE:$MODEL_PORT/v1\",\"http://$BRIDGE:$MODEL_PORT\",\"https://openrouter.ai/api/v1\",\"https://openrouter.ai/api\"]" > /dev/null
docker exec -d "$LAB" sh -c 'exec orch-connector --data-dir /data run > /data/run.log 2>&1'
machine_online() { [ "$(call GET /machines | jq -r '[.[] | select(.name == "lab")][0].online')" = true ]; }
wait_for 60 "the machine is online" machine_online
LAB_ID="$(call GET /machines | jq -r '[.[] | select(.name == "lab")][0].id')"
# A machine is online before its facts are in, and its provider is registered against what they say its policy lets it
# forward: the answer to a provider asked for too soon is a refusal.
lab_has_told_its_policy() { [ "$(call GET /machines | jq -r '[.[] | select(.name == "lab")][0].facts.policy["provider.upstreams"] | length')" -ge 1 ]; }
wait_for 60 "the machine has told the system what it may forward" lab_has_told_its_policy

step "a provider on a key whose upstream is the scripted model server, in the two formats the harnesses speak"
check "the organisation allows endpoints without TLS, as the scripted server has" api PUT "/nodes/$ORG/settings/providers.allow_http_upstreams" '{"value":true}'
PROVIDER="$(call POST "/machines/$LAB_ID/providers" "$(jq -cn --arg org "$ORG" --arg base "http://$BRIDGE:$MODEL_PORT" \
  '{kind:"api",home_id:$org,name:"Scripted model",upstreams:[{protocol:"openai-responses",base_url:($base+"/v1"),options:{}},{protocol:"anthropic-messages",base_url:$base,options:{}}]}')" | jq -r .id)"
check "the provider was registered from the machine" test -n "$PROVIDER" -a "$PROVIDER" != null
check "its key was given to the system, which keeps it sealed" api PUT "/providers/$PROVIDER/key" "$(jq -cn --arg key "$MODEL_KEY" '{api_key:$key}')"
check "a model is added with its rate" api POST "/providers/$PROVIDER/models" \
  '{"model_name":"mock-large","display_name":"Mock Large","parameters":{"rate":{"input_per_mtok":"2.50","cached_input_per_mtok":"0.25","output_per_mtok":"15"}}}'
expect "the provider is ready" "ready" "$(call GET "/providers/$PROVIDER" | jq -r .status)"
CODEX_AGENT="$(call GET /agents | jq -r '[.[] | select(.harness_kind == "codex")][0].id')"
CLAUDE_AGENT="$(call GET /agents | jq -r '[.[] | select(.harness_kind == "claude-code")][0].id')"
check "the profiles of the two harnesses are there" test -n "$CODEX_AGENT" -a "$CODEX_AGENT" != null -a -n "$CLAUDE_AGENT" -a "$CLAUDE_AGENT" != null

# --- 3. An environment from the Default template: the unit has the builds --------------------------------------------

step "POST /environments from the Default template on the bundled host"
MADE="$(call POST /environments "$(jq -cn --arg home "$PROJECT_ID" --arg template "$TEMPLATE_ID" --arg host "$HOST_ID" \
  '{name:"Main",home_node_id:$home,template_id:$template,host_id:$host}')")"
ENV_ID="$(printf '%s' "$MADE" | jq -r .id)"
JOB_ID="$(printf '%s' "$MADE" | jq -r '.job.id // empty')"
check "the environment was made and a job runs" test -n "$ENV_ID" -a "$ENV_ID" != null -a -n "$JOB_ID"
deadline=$((SECONDS + 900))
while [ "$SECONDS" -lt "$deadline" ]; do
  JOB="$(call GET "/jobs/$JOB_ID")" || break
  case "$(printf '%s' "$JOB" | jq -r .state)" in succeeded | failed | cancelled) break ;; esac
  sleep 2
done
expect "the job succeeded" "succeeded" "$(printf '%s' "$JOB" | jq -r .state)"
expect "it went through the phases of making a unit, in order" "schedule image volumes tools create enroll attach verify commit" \
  "$(printf '%s' "$JOB" | jq -r '[.progress.phases[].name] | join(" ")')"
ENV="$(call GET "/environments/$ENV_ID")"
expect "the environment is running" "running" "$(printf '%s' "$ENV" | jq -r .status)"

step "the node holds both builds, whole and as the release names them, and the unit sees them"
NODE_TOOLS="$STACK/runtime/node/tools"
expect "the cache holds Codex under its version, verified against the hash the release lists" "$CODEX_SHA" "$(tr -d '[:space:]' < "$NODE_TOOLS/codex/$CODEX_VERSION/.complete" 2> /dev/null || true)"
expect "and Claude Code" "$CLAUDE_SHA" "$(tr -d '[:space:]' < "$NODE_TOOLS/claude-code/$CLAUDE_VERSION/.complete" 2> /dev/null || true)"
UNIT="$(docker ps --filter "label=orch.system=$SYSTEM_NAME" --format '{{.Names}}' | head -n 1)"
check "a container of the system is running" test -n "$UNIT"
mount_is_read_only() { docker inspect "$UNIT" --format '{{range .Mounts}}{{.Destination}} {{.RW}}{{"\n"}}{{end}}' | grep -qx "$1 false"; }
check "Codex is mounted in it read-only" mount_is_read_only /opt/orch/tools/codex
check "and Claude Code" mount_is_read_only /opt/orch/tools/claude-code
# Asked in a throwaway home, so that nothing of a harness is written in the account's own, which is looked at below.
expect "Codex runs there and says which build it is" "codex-cli $CODEX_VERSION" "$(docker exec -e HOME=/tmp "$UNIT" /opt/orch/tools/codex/bin/codex --version 2> /dev/null | head -n 1)"
expect "Claude Code runs there and says which build it is" "$CLAUDE_VERSION (Claude Code)" "$(docker exec -e HOME=/tmp -e DISABLE_UPDATES=1 "$UNIT" /opt/orch/tools/claude-code/claude-code --version 2> /dev/null | head -n 1)"
reported() { sql "select elem->>'version' from machines, jsonb_array_elements(facts->'harnesses') as elem where placement = 'managed' and elem->>'kind' = '$1' limit 1"; }
expect "the connector in the unit reports Codex among the harnesses it has" "$CODEX_VERSION" "$(reported codex)"
expect "and Claude Code" "$CLAUDE_VERSION" "$(reported claude-code)"
exec_server_is_available() { [ "$(call GET "/environments/$ENV_ID" | jq -r '[.facts.executors[] | select(.kind == "codex-exec-server")][0].available')" = true ]; }
wait_for 60 "the environment lists the exec server of Codex among its available executors, now that the unit has the build" exec_server_is_available

# --- 4. A thread for each harness: the harness runs in the unit, the key stays with the provider ------------------

# The text of a turn is a prompt and a script for the scripted model; `{{result}}` is what the tools of the round before
# printed.
read -r -d '' CODEX_TURN <<'EOF' || true
Make a file and change it.
#!mock
say I will make a file in the workspace.
tool exec_command {"cmd":"echo made-by-codex > proof.txt && pwd && cat proof.txt"}
then
tool exec_command {"cmd":"apply_patch <<'PATCH'\n*** Begin Patch\n*** Update File: proof.txt\n@@\n-made-by-codex\n+changed-by-codex\n*** End Patch\nPATCH"}
then
say The file now says changed-by-codex. The last command printed: {{result}}
EOF
read -r -d '' CLAUDE_TURN <<'EOF' || true
Make a file and change it.
#!mock
say I will make a file in the workspace.
tool Bash {"command":"echo made-by-claude > proof.txt && pwd && cat proof.txt","description":"Make the file"}
then
tool Read {"file_path":"/workspace/claude/proof.txt"}
then
tool Edit {"file_path":"/workspace/claude/proof.txt","old_string":"made-by-claude","new_string":"changed-by-claude"}
then
say The file now says changed-by-claude.
EOF

# thread NAME AGENT DIRECTORY: a thread of the harness in the environment, its key at the provider.
make_thread() {
  call POST "/topics/$TOPIC/threads" "$(jq -cn --arg name "$1" --arg agent "$2" --arg provider "$PROVIDER" --arg env "$ENV_ID" --arg dir "$3" \
    '{name:$name,binding:{agent_id:$agent,provider_id:$provider,arrangement:"key_separated"},model:"mock-large",environment:{use:$env},directory:$dir}')" | jq -r .id
}
# turn THREAD TEXT FILE OPTION: one turn through the socket of the web edge, every approval decided as the person would,
# with the option of that name: each harness's adapter names the ways to allow a request.
turn() {
  ORCH_TOKEN="$TOKEN" timeout 420 node "$FOLLOW" --api "$EDGE/api/v1" --ws "ws://127.0.0.1:$EDGE_PORT/api/v1/ws" \
    --thread "$1" --approve "$4" --timeout 400 -- "$2" > "$3"
}

step "a thread of Codex whose harness runs in the unit, and a turn of it"
CODEX_THREAD="$(make_thread 'Codex in a container' "$CODEX_AGENT" codex)"
check "the thread was made on a profile whose harness had never run" test -n "$CODEX_THREAD" -a "$CODEX_THREAD" != null
NODE="$(call GET "/nodes/$CODEX_THREAD")"
expect "the harness runs in the environment and the key stays with the provider" "key_separated" "$(printf '%s' "$NODE" | jq -r .binding.arrangement)"
expect "its directory is below the workspace of the unit" "/workspace/codex" "$(printf '%s' "$NODE" | jq -r .cwd)"
check "the person wants to decide what it runs" api PUT "/nodes/$CODEX_THREAD/settings/approval.mode" '{"value":"ask"}'
if turn "$CODEX_THREAD" "$CODEX_TURN" "$WORK/codex-turn.jsonl" once; then pass "the turn completed"; else fail "the turn completed"; cat "$WORK/codex-turn.jsonl" >&2; fi
expect "the person decided two requests, the command and the change of the file, and allowed both" "2" \
  "$(jq -s '[.[] | select(.event == "approval" and .option == "once" and .state == "approved")] | length' "$WORK/codex-turn.jsonl")"
expect "no step of the turn failed" "0" "$(jq -s '[.[] | select(.event == "item" and .status == "failed")] | length' "$WORK/codex-turn.jsonl")"
check "the command ran in the directory of the thread, in the workspace of the unit, and ended well" jq -e 'select(.event == "item" and .kind == "tool_call" and .status == "completed" and (.summary | test("-> exit 0 /workspace/codex")))' "$WORK/codex-turn.jsonl"
check "the change of the file is in the timeline as the change of a file" jq -e 'select(.event == "item" and .kind == "tool_call" and .status == "completed" and (.summary | test("proof.txt")) and (.summary | test("exit") | not))' "$WORK/codex-turn.jsonl"
check "and the agent's last word is what the scripted model made of the output of the patch" jq -e 'select(.event == "item" and .kind == "agent_message" and (.summary | test("changed-by-codex. The last command printed: [\\s\\S]*Updated the following files:\\nM proof.txt")))' "$WORK/codex-turn.jsonl"
expect "the file is on the unit's disk as the harness changed it" "changed-by-codex" "$(docker exec "$UNIT" cat /workspace/codex/proof.txt 2> /dev/null || true)"
expect "and belongs to the account of the unit" "agent" "$(docker exec "$UNIT" stat -c %U /workspace/codex/proof.txt 2> /dev/null || true)"

# Looked at, not yet judged: what a thread of Codex is bound as when nobody names an arrangement.
UNNAMED="$(call POST "/topics/$TOPIC/threads" "$(jq -cn --arg agent "$CODEX_AGENT" --arg provider "$PROVIDER" --arg env "$ENV_ID" \
  '{name:"Codex, no arrangement named",binding:{agent_id:$agent,provider_id:$provider},model:"mock-large",environment:{use:$env},directory:"unnamed"}')" | jq -r .id)"
say "INFO a thread of Codex made without naming an arrangement is bound as: $(call GET "/nodes/$UNNAMED" | jq -c '{arrangement: .binding.arrangement, executor: .binding.isolation.executor}')"

step "a thread of Claude Code whose harness runs in the unit, and a turn of it"
CLAUDE_THREAD="$(make_thread 'Claude Code in a container' "$CLAUDE_AGENT" claude)"
check "the thread was made on a profile whose harness had never run" test -n "$CLAUDE_THREAD" -a "$CLAUDE_THREAD" != null
NODE="$(call GET "/nodes/$CLAUDE_THREAD")"
expect "the harness runs in the environment and the key stays with the provider" "key_separated" "$(printf '%s' "$NODE" | jq -r .binding.arrangement)"
check "the person wants to decide what it runs" api PUT "/nodes/$CLAUDE_THREAD/settings/approval.mode" '{"value":"ask"}'
if turn "$CLAUDE_THREAD" "$CLAUDE_TURN" "$WORK/claude-turn.jsonl" allow_once; then pass "the turn completed"; else fail "the turn completed"; cat "$WORK/claude-turn.jsonl" >&2; fi
expect "the person decided two requests, the command and the edit, and allowed both" "2" \
  "$(jq -s '[.[] | select(.event == "approval" and .option == "allow_once" and .state == "approved")] | length' "$WORK/claude-turn.jsonl")"
expect "no step of the turn failed" "0" "$(jq -s '[.[] | select(.event == "item" and .status == "failed")] | length' "$WORK/claude-turn.jsonl")"
check "the command ran in the directory of the thread, in the workspace of the unit, and ended well" jq -e 'select(.event == "item" and .kind == "tool_call" and .status == "completed" and (.summary | test("-> exit 0")) and (.summary | test("/workspace/claude")))' "$WORK/claude-turn.jsonl"
check "the edit of the file is in the timeline as the change of a file" jq -e 'select(.event == "item" and .kind == "tool_call" and .status == "completed" and (.summary | test("proof.txt")) and (.summary | test("exit") | not))' "$WORK/claude-turn.jsonl"
check "and the agent's last word" jq -e 'select(.event == "item" and .kind == "agent_message" and (.summary | test("changed-by-claude")))' "$WORK/claude-turn.jsonl"
expect "the file is on the unit's disk as the harness changed it" "changed-by-claude" "$(docker exec "$UNIT" cat /workspace/claude/proof.txt 2> /dev/null || true)"
expect "and belongs to the account of the unit" "agent" "$(docker exec "$UNIT" stat -c %U /workspace/claude/proof.txt 2> /dev/null || true)"

# --- 5. What was used, and what was kept ---------------------------------------------------------------------------------

step "the use is in the ledger, counted where the key is"
usage_of() { call GET "/usage/events?scope=node:$TOPIC&from=$TODAY&to=$TODAY&thread_id=$1&counted=true&limit=100"; }
for pair in "codex:$CODEX_THREAD" "claude-code:$CLAUDE_THREAD"; do
  kind="${pair%%:*}"; thread="${pair#*:}"
  rows="$(usage_of "$thread" | jq -c '[.items[]]')"
  say "INFO $kind: the ledger counted $(printf '%s' "$rows" | jq length) requests for the thread"
  check "$kind: the ledger has rows for the thread, with tokens, counted on the model it ran on" jq -e 'length > 0 and all(.[]; .tokens.input_tokens > 0 and .model == "mock-large" and .harness_kind == "'"$kind"'")' <<< "$rows"
  expect "$kind: they were counted by the forwarder on the arrangement the thread has" "forwarder key_separated" \
    "$(printf '%s' "$rows" | jq -r '[.[].origin] | unique | join(",")') $(printf '%s' "$rows" | jq -r '[.[].arrangement] | unique | join(",")')"
done

step "the harnesses were started as their adapters say, and wrote where they declare"
docker exec -i "$UNIT" sh -s -- "$BRIDGE:$GATEWAY_PORT" < "$HERE/inspect-unit.sh" > "$WORK/unit-inspection.txt" 2> "$WORK/unit-inspection.err" || true
inspection() { grep -- "$1" "$WORK/unit-inspection.txt" > /dev/null; }
check "Codex was started as an app-server on its standard streams" inspection '^codex-args:.*app-server --listen stdio://'
check "Claude Code was started with its own configuration directory and no other setting source than the project's" inspection '^claude-args:.*--setting-sources project'
for variable in DISABLE_TELEMETRY DISABLE_ERROR_REPORTING DISABLE_FEEDBACK_COMMAND DISABLE_UPDATES CLAUDE_CODE_DISABLE_NONESSENTIAL_TRAFFIC CLAUDE_CODE_DISABLE_FEEDBACK_SURVEY CLAUDE_CODE_DISABLE_OFFICIAL_MARKETPLACE_AUTOINSTALL; do
  check "Claude Code has $variable=1" inspection "^claude-env: $variable=1\$"
done
check "Claude Code keeps its home where the connector put it, in the data directory" inspection '^claude-env: CLAUDE_CONFIG_DIR=/var/lib/orch/'
check "Codex keeps its home where the connector put it, in the data directory" inspection '^codex-env: CODEX_HOME=/var/lib/orch/'
check "the home of Codex says it does not look for an update" inspection '^codex-config: check_for_update_on_startup = false$'
for table in analytics feedback; do
  check "the home of Codex switches $table off" bash -c "grep -A1 '^codex-config: \[$table\]\$' '$WORK/unit-inspection.txt' | grep -q 'enabled = false'"
done
for exporter in exporter metrics_exporter trace_exporter; do
  check "the home of Codex sends no $exporter anywhere" inspection "^codex-config: $exporter = \"none\"\$"
done
check "and keeps no memory of its own" inspection '^codex-config: memories = false$'
# The files an account is given by its image are not a harness's.
check "nothing of either harness is in the account's home" bash -c "! grep '^home-entry:' '$WORK/unit-inspection.txt' | grep -v -E '/home/agent/\.(bash_logout|bashrc|profile)\$'"
check "the unit made no connection but its link to the gateway and the ones inside it" bash -c "! grep -q '^outside-connection:' '$WORK/unit-inspection.txt'"
# What the daemon records of the unit's own layer: the places it mounted things at, and temporary places. The volumes are not in it.
docker diff "$UNIT" > "$WORK/unit.diff" 2> /dev/null || true
check "outside its volumes the unit changed nothing but the places its mounts are at and temporary places" bash -c "! grep -v -E '^[ACD] /(tmp|run|var/tmp|opt/orch|var/lib/orch)(/|\$)|^C /(opt|usr|usr/sbin|var|var/lib)\$|^A /usr/sbin/docker-init\$' '$WORK/unit.diff' | grep -q ."

# --- 6. A real model, once each (only when the person asks for it) -------------------------------------------------------

if [ -n "${ORCH_HARNESS_REAL_ENV:-}" ]; then
  step "once each against a real model: the key goes to the provider on the other machine and nowhere else"
  # The key is read inside this subshell only, goes to curl on its standard input, and is printed nowhere.
  REAL_PROVIDER="$(call POST "/machines/$LAB_ID/providers" "$(jq -cn --arg org "$ORG" \
    '{kind:"api",home_id:$org,name:"A real model",upstreams:[{protocol:"openai-responses",base_url:"https://openrouter.ai/api/v1",options:{}},{protocol:"anthropic-messages",base_url:"https://openrouter.ai/api",options:{}}]}')" | jq -r .id)"
  check "a provider on the real interfaces was registered from the machine" test -n "$REAL_PROVIDER" -a "$REAL_PROVIDER" != null
  put_key() {
    (
      set -a
      # shellcheck source=/dev/null
      . "$ORCH_HARNESS_REAL_ENV"
      set +a
      jq -cn '{api_key: env.OPENROUTER_API_KEY}' | curl -sS --max-time 60 -o "$REPLY_FILE" -w '%{http_code}' -X PUT "$EDGE/api/v1/providers/$REAL_PROVIDER/key" \
        -H "authorization: Bearer $TOKEN" -H 'content-type: application/json' -d @-
    )
  }
  expect "its key was given to the system" "204" "$(put_key)"
  check "the model of Codex is added" api POST "/providers/$REAL_PROVIDER/models" '{"model_name":"openai/gpt-5.6-luna","display_name":"GPT 5.6 Luna","parameters":{"rate":{"input_per_mtok":"0.20","cached_input_per_mtok":"0.02","output_per_mtok":"1.20"}}}'
  check "the model of Claude Code is added" api POST "/providers/$REAL_PROVIDER/models" '{"model_name":"anthropic/claude-haiku-5.5","display_name":"Claude Haiku 5.5","parameters":{"rate":{"input_per_mtok":"0.10","cached_input_per_mtok":"0.01","output_per_mtok":"0.50"}}}'
  # The machine that keeps the key asks the upstream for its models when it is given the key; a provider that is not ready by
  # then has a key the upstream refuses or an upstream out of reach, and no request is made to a model.
  real_provider_is_ready() { [ "$(call GET "/providers/$REAL_PROVIDER" | jq -r .status)" = ready ]; }
  wait_for 60 "the upstream took the key: the provider is ready" real_provider_is_ready || true
  for pair in "codex:$CODEX_AGENT:openai/gpt-5.6-luna:Codex" "claude-code:$CLAUDE_AGENT:anthropic/claude-haiku-5.5:Claude Code"; do
    real_provider_is_ready || { say "the provider is not ready: no request is made to $pair"; continue; }
    IFS=: read -r kind agent model title <<< "$pair"
    thread="$(call POST "/topics/$TOPIC/threads" "$(jq -cn --arg name "$title, a real model" --arg agent "$agent" --arg provider "$REAL_PROVIDER" --arg env "$ENV_ID" --arg model "$model" --arg dir "real-$kind" \
      '{name:$name,binding:{agent_id:$agent,provider_id:$provider,arrangement:"key_separated"},model:$model,environment:{use:$env},directory:$dir}')" | jq -r .id)"
    check "$kind: a thread on the real model" test -n "$thread" -a "$thread" != null
    if turn "$thread" "Reply with the single word: ready" "$WORK/real-$kind.jsonl" once; then pass "$kind: the turn completed"; else fail "$kind: the turn completed"; cat "$WORK/real-$kind.jsonl" >&2; fi
    check "$kind: the model answered" jq -e 'select(.event == "item" and .kind == "agent_message" and (.summary | test("ready"; "i")))' "$WORK/real-$kind.jsonl"
    rows="$(usage_of "$thread" | jq -c '[.items[]]')"
    say "INFO $kind: the ledger counted $(printf '%s' "$rows" | jq length) requests to the real model"
    check "$kind: one request is in the ledger with what it cost" jq -e 'length >= 1 and all(.[]; .tokens.input_tokens > 0 and .credits != null)' <<< "$rows"
  done
  # Where the key must not be. It is read in the subshell of each question, handed to grep as a file descriptor and printed
  # nowhere: the only answer is whether what a command prints holds it. (A pipeline's status is grep's here: tar and the
  # others complain about things they cannot read.)
  key_is_in() {
    (
      set +o pipefail
      set -a
      # shellcheck source=/dev/null
      . "$ORCH_HARNESS_REAL_ENV"
      set +a
      "$@" 2> /dev/null | grep -aqF -f /dev/fd/3 3< <(printf '%s\n' "$OPENROUTER_API_KEY")
    )
  }
  key_is_not_in() { ! key_is_in "$@"; }
  unit_files() { docker exec "$UNIT" tar -c -C / workspace home var/lib/orch tmp run/orch; }
  unit_processes() { docker exec "$UNIT" sh -c 'cat /proc/[0-9]*/environ /proc/[0-9]*/cmdline'; }
  unit_description() { docker inspect "$UNIT"; docker logs "$UNIT" 2>&1; }
  services_logs() { compose logs --no-color 2>&1; }
  lab_logs() { docker logs "$LAB" 2>&1; docker exec "$LAB" cat /data/run.log; }
  # A control: the search finds the key in a line that holds it.
  key_in_a_line() { printf 'a line with %s in it\n' "$OPENROUTER_API_KEY"; }
  check "the search does find the key where it is" key_is_in key_in_a_line
  check "the key is not in the unit's files" key_is_not_in unit_files
  check "nor in the environment or the command line of its processes" key_is_not_in unit_processes
  check "nor in what the daemon says of the unit and its logs" key_is_not_in unit_description
  check "nor in the logs of the services" key_is_not_in services_logs
  check "nor in the logs of the machine that keeps it" key_is_not_in lab_logs
fi

# --- 7. Removal ----------------------------------------------------------------------------------------------------------

# The same product-only native/import proof on this scenario's released stack and Default environment.
# shellcheck source=../managed-provider/subscription.sh
. "$HERE/../managed-provider/subscription.sh"
run_subscription_proofs

# A scenario that is kept to be looked at keeps its unit too.
if [ "${ORCH_SMOKE_KEEP:-0}" = 1 ]; then
  say "kept: the environment $ENV_ID and its unit, to be looked at"
  exit 0
fi

step "the environment is removed, and its container goes"
check "the topic is deleted, and its threads with it" api DELETE "/nodes/$TOPIC"
check "the removal is accepted" api DELETE "/environments/$ENV_ID?confirm=Main"
container_gone() { [ -z "$(docker ps -aq --filter "label=orch.system=$SYSTEM_NAME")" ]; }
wait_for 120 "the container is gone" container_gone
volumes_gone() { [ -z "$(docker volume ls -q | grep -F -e "$ENV_ID" -e "$UNIT" || true)" ]; }
wait_for 60 "so are its volumes" volumes_gone
expect "the errors the API logged in all this" "0" "$(compose logs --no-color api 2> /dev/null | sed 's/\x1b\[[0-9;]*m//g' | grep -c ' ERROR ' || true)"
