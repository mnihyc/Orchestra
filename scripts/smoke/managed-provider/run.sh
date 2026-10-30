#!/usr/bin/env bash
# managed-provider: a release packaged from the tree, started by the updater, and used over HTTP until a subscription of each
# of the two real harnesses has a runtime of its own on the bundled host, has been signed in by the method that imports a
# credential (with a credential that is not real: what is tried is the flow, and the harness accepting the form of the
# file), has been stopped and started by hand without losing the sign-in, has been signed out, and has been deleted with its
# unit and its volume. Nothing is asked of a model, and no real credential is given.
#
# The stack is the one managed-harness makes (the pinned listener on the Docker bridge's address at the port it listens on
# inside, the system's own name, the web edge's name as the public address).
set -Eeuo pipefail
# shellcheck source-path=SCRIPTDIR/..
# shellcheck source=lib.sh
. "$(dirname "${BASH_SOURCE[0]}")/../lib.sh"
HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd -P)"

command -v jq > /dev/null || abort "jq is needed"

A="$(release_a)"
TAG="$(release_tag "$A")"
new_stack managed-provider 41350 "$A"
WORK="$(dirname "$STACK")"
BRIDGE="$(bridge_address)"
SYSTEM_NAME="orch-smoke-provider-$EDGE_PORT"
PROVIDERS=()
# Docker reserves 125 for an inspector it could not start. Keep that separate
# from grep's 1, which means a successful search found no credential.
INSPECTION_UNAVAILABLE=125

sed -i \
  -e "s|^name = \"Orch\"|name = \"$SYSTEM_NAME\"|" \
  -e "s|^public_url = .*|public_url = \"http://frontend\"|" \
  -e "s|wss://gateway.orch.internal:9443|wss://gateway.orch.internal:$GATEWAY_PORT|g" \
  -e "s|wss://gateway:9443|wss://gateway:$GATEWAY_PORT|g" \
  -e "s|^pinned_bind = .*|pinned_bind = \"[::]:$GATEWAY_PORT\"|" \
  "$STACK/config/orch.toml"
sed -i -e "s|^ORCH_GATEWAY_PUBLISH=.*|ORCH_GATEWAY_PUBLISH=$BRIDGE:$GATEWAY_PORT:$GATEWAY_PORT|" "$STACK/.env"

FOLLOW="$HERE/../managed-environment/follow.mjs"
ENV_ID=""
# Takes down what the node made for the scenario, by exact names: the units of the system, and the volumes of the providers.
cleanup() {
  if [ "${ORCH_SMOKE_KEEP:-0}" = 1 ]; then
    say "kept: the units of the system $SYSTEM_NAME"
    finish
    return
  fi
  local unit provider volume
  for unit in $(docker ps -a --filter "label=orch.system=$SYSTEM_NAME" --format '{{.Names}}' 2> /dev/null); do
    docker rm -f "$unit" > /dev/null 2>&1 || true
    docker volume rm "$unit-run" > /dev/null 2>&1 || true
  done
  if [ -n "$ENV_ID" ]; then
    for volume in $(docker volume ls -q 2> /dev/null | grep -F -e "$ENV_ID" || true); do
      docker volume rm "$volume" > /dev/null 2>&1 || true
    done
  fi
  for provider in "${PROVIDERS[@]:-}"; do
    if [ -n "$provider" ]; then
      docker volume rm "orch-state-$provider" > /dev/null 2>&1 || true
    fi
  done
  for provider in "${SUBSCRIPTION_PROVIDERS[@]-}"; do
    if [ -n "$provider" ]; then docker volume rm "orch-state-$provider" > /dev/null 2>&1 || true; fi
  done
  finish
}
trap 'cleanup' EXIT

REPLY_FILE="$WORK/reply.json"
STATUS=""

# api METHOD PATH [BODY]: a request of the signed-in administrator through the web edge. The answer is in $REPLY_FILE and its
# status in $STATUS. A token that has run out is replaced once.
api() {
  local method="$1" path="$2" body="${3:-}" attempt args
  mkdir -p "$WORK"
  for attempt in 1 2; do
    args=(-sS --max-time 90 -o "$REPLY_FILE" -w '%{http_code}' -X "$method" "$EDGE/api/v1$path" -H "authorization: Bearer $TOKEN")
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
CLAUDE_VERSION="$(listed claude-code version)"
check "Codex and Claude Code are listed" test -n "$CODEX_VERSION" -a "$CODEX_VERSION" != null -a -n "$CLAUDE_VERSION" -a "$CLAUDE_VERSION" != null

step "the release is started by the updater and the services report"
if update_sh first-start --from "$A"; then pass "first-start finished"; else fail "first-start finished"; fi
check "orchctl doctor passes in the API's container" doctor --expect-version "${TAG#v}"
edge_up() { compose up -d --no-deps frontend > /dev/null 2>&1; curl -fsS "$EDGE/health" > /dev/null 2>&1; }
wait_for 90 "the web edge answers" edge_up

step "the first administrator, the bundled host, and the unchanged shipped provider template"
if setup_admin; then pass "POST /api/v1/setup made the first administrator"; else fail "POST /api/v1/setup"; fi
wait_for 240 "the node is online" node_online
host_has_a_driver() { [ "$(call GET /hosts | jq -r '.[0].drivers[0].name')" = docker ]; }
wait_for 120 "the host has told which drivers it has" host_has_a_driver
ORG="$(call POST /orgs '{"name":"Acme","slug":"acme"}' | jq -r .id)"
check "an org was made" test -n "$ORG" -a "$ORG" != null
TEMPLATES="$(call GET /templates)"
SEEDED="$(printf '%s' "$TEMPLATES" | jq -c '[.[] | select(.name == "Provider runtime")][0]')"
check "the seeded provider template is listed" test "$SEEDED" != null
expect "the seeded template is of the pool of providers" "provider" "$(printf '%s' "$SEEDED" | jq -r '.kind')"
TEMPLATE="$(printf '%s' "$SEEDED" | jq -r .id)"
expect "the shipped provider template starts at its original revision" "1" "$(printf '%s' "$SEEDED" | jq -r .revision)"
CODEX_AGENT="$(call GET /agents | jq -r '[.[] | select(.harness_kind == "codex")][0].id')"
CLAUDE_AGENT="$(call GET /agents | jq -r '[.[] | select(.harness_kind == "claude-code")][0].id')"
check "the profiles of the two harnesses are there" test -n "$CODEX_AGENT" -a "$CODEX_AGENT" != null -a -n "$CLAUDE_AGENT" -a "$CLAUDE_AGENT" != null

# --- 2. A subscription of each harness, and the unit that is made for it ----------------------------------------------

# A credential that is not real, in the form the harness takes. The marker is in each, and is looked for below.
MARKER_CODEX="access-token-not-real-$(openssl rand -hex 8)"
MARKER_CLAUDE="sk-ant-oat01-not-real-$(openssl rand -hex 16)"
b64url() { openssl base64 -A | tr '+/' '-_' | tr -d '='; }
ID_TOKEN="$(printf '{"alg":"RS256"}' | b64url).$(printf '{"email":"dana@example.com","https://api.openai.com/auth":{"chatgpt_plan_type":"team","chatgpt_account_id":"acct-not-real"}}' | b64url).$(printf '"not-a-signature"' | b64url)"
CODEX_AUTH="$(jq -cn --arg id "$ID_TOKEN" --arg at "$MARKER_CODEX" \
  '{auth_mode:"chatgpt",OPENAI_API_KEY:null,tokens:{id_token:$id,access_token:$at,refresh_token:"refresh-token-not-real",account_id:"acct-not-real"},last_refresh:"2026-10-11T10:00:00Z"}')"

# state_of PROVIDER FIELD: a member of the provider's runtime.
runtime_of() { call GET "/providers/$1" | jq -r ".runtime.$2"; }
# unit_of RUNTIME: the unit the node made for a runtime, by the label every unit carries.
unit_of() { docker ps -a --filter "label=orch.runtime_id=$1" --format '{{.Names}}' | head -n 1; }

# The provider image has no shell or inspection tools. Inspect its mounts and
# process namespace from the same unprivileged account in a separate read-only
# container, using the distribution the services already run. No tool is copied
# into the provider and the inspector has no network or extra capabilities.
read_unit() {
  local unit="$1" program="$2" account state_volume
  shift 2
  if ! account="$(docker inspect "$unit" --format '{{.Config.User}}')"; then
    return "$INSPECTION_UNAVAILABLE"
  fi
  [ -n "$account" ] || return "$INSPECTION_UNAVAILABLE"
  if ! state_volume="$(docker inspect "$unit" --format '{{json .Mounts}}' | jq -er '.[] | select(.Type == "volume" and .Destination == "/var/lib/orch") | .Name')"; then
    return "$INSPECTION_UNAVAILABLE"
  fi
  docker volume inspect "$state_volume" > /dev/null || return "$INSPECTION_UNAVAILABLE"
  docker run --rm --name "${PROJECT}-inspect" --network none --read-only \
    --cap-drop ALL --security-opt no-new-privileges --user "$account" \
    --mount "type=volume,source=$state_volume,target=/var/lib/orch,readonly" --pid "container:$unit" \
    --entrypoint "$program" debian:bookworm-slim@sha256:7c7b2c966bc9ee8cedfeef67e0e279108992c77681fa595db4a9d65c06ccc587 "$@"
}

credential_count() {
  local unit="$1" marker="$2" files status
  files="$(read_unit "$unit" grep -rlF -- "$marker" /var/lib/orch)" && status=0 || status=$?
  case "$status" in
    0) printf '%s\n' "$files" | wc -l | tr -d ' ' ;;
    1) printf '0' ;;
    *) return "$status" ;;
  esac
}

# make_provider KIND AGENT METHOD VALUES MARKER: the whole life of one provider, in order.
life_of() {
  local kind="$1" agent="$2" method="$3" values="$4" marker="$5" provider job unit runtime
  step "a subscription of $kind, and the job that makes its runtime"
  provider="$(call POST /providers "$(jq -cn --arg org "$ORG" --arg agent "$agent" --arg name "A subscription of $kind" \
    '{kind:"subscription",name:$name,home_id:$org,agent_id:$agent}')" | jq -r .id)"
  check "$kind: the provider was made" test -n "$provider" -a "$provider" != null
  PROVIDERS+=("$provider")
  job="$(call GET "/providers/$provider" | jq -r '.runtime.job.id // empty')"
  check "$kind: a job makes its runtime" test -n "$job"
  local deadline=$((SECONDS + 1200)) state=""
  while [ "$SECONDS" -lt "$deadline" ]; do
    JOB="$(call GET "/jobs/$job")" || break
    state="$(printf '%s' "$JOB" | jq -r .state)"
    case "$state" in succeeded | failed | cancelled) break ;; esac
    sleep 3
  done
  expect "$kind: the job succeeded" "succeeded" "$state"
  [ "$state" = succeeded ] || { printf '%s\n' "$JOB" | jq -c '.error' >&2; return 1; }
  expect "$kind: it went through the phases of making a provider's unit, in order" "schedule image volumes tools create enroll verify commit" \
    "$(printf '%s' "$JOB" | jq -r '[.progress.phases[].name] | join(" ")')"
  runtime="$(sql "select id from runtimes where provider_id = '$provider' and state = 'running'")"
  unit="$(unit_of "$runtime")"
  expect "$kind: the runtime uses the shipped template without a provider override" "$TEMPLATE" \
    "$(sql "select template_id from runtimes where id = '$runtime'")"
  check "$kind: a unit of the system is running for it" test -n "$unit"
  expect "$kind: its account is the one the unchanged template declares" \
    "$(printf '%s' "$SEEDED" | jq -r '.spec.user | "\(.uid):\(.gid)"')" \
    "$(docker inspect "$unit" --format '{{.Config.User}}')"
  expect "$kind: its runtime says it is running" "running" "$(runtime_of "$provider" state)"
  expect "$kind: the unit is named by the provider's runtime" "orch-provider-$(printf '%s' "$runtime" | tail -c 12)" "$unit"
  check "$kind: its state volume is kept under the provider's name" bash -c "docker volume ls -q | grep -qx 'orch-state-$provider'"
  mount_is_read_only() { docker inspect "$unit" --format '{{range .Mounts}}{{.Destination}} {{.RW}}{{"\n"}}{{end}}' | grep -qx "$1 false"; }
  check "$kind: its harness build is mounted in it read-only" mount_is_read_only "/opt/orch/tools/$kind"
  expect "$kind: its root is read-only" "true" "$(docker inspect "$unit" --format '{{.HostConfig.ReadonlyRootfs}}')"
  check "$kind: the kernel forbids gaining privileges" read_unit "$unit" grep -Eq '^NoNewPrivs:[[:space:]]+1$' /proc/1/status
  check "$kind: the kernel gives it no capabilities" read_unit "$unit" grep -Eq '^CapBnd:[[:space:]]+0+$' /proc/1/status
  refuses "$kind: its image offers no shell" docker exec "$unit" /bin/sh -c true
  check "$kind: the read-only inspector can read its state" read_unit "$unit" stat /var/lib/orch
  expect "$kind: the connector in it plays the roles of a provider's runtime and of a harness's place" "agent,provider" \
    "$(sql "select array_to_string(array(select unnest(roles) order by 1), ',') from machines where id = (select machine_id from providers where id = '$provider')")"
  expect "$kind: the provider is not signed in" "needs_setup" "$(call GET "/providers/$provider" | jq -r .status)"

  step "$kind: signed in by importing a credential that is not real"
  local reply
  reply="$(call POST "/providers/$provider/login" "$(jq -cn --arg method "$method" --argjson values "$values" '{method:$method,values:$values}')")" || true
  say "INFO $kind: the answer to the sign-in is: $(printf '%s' "$reply" | jq -c '{state,method,error}')"
  login_is_over() { case "$(call GET "/providers/$provider/login" | jq -r .state)" in completed | failed | expired) true ;; *) false ;; esac; }
  wait_for 180 "$kind: the sign-in is over" login_is_over || true
  expect "$kind: the harness took the credential" "completed" "$(call GET "/providers/$provider/login" | jq -r .state)"
  # The harness is asked what it holds once the sign-in is over, and the provider keeps what it answers as its facts. A
  # provider that has no model enabled still has to be set up, so its standing does not move; its facts say what was read.
  provider_facts_are_in() { [ "$(call GET "/providers/$provider" | jq -r '.facts_at // empty')" != "" ]; }
  if [ "$kind" = codex ]; then
    wait_for 120 "$kind: the harness was asked what it holds, and the provider keeps the answer as its facts" provider_facts_are_in || true
    expect "$kind: the facts say who the credential is for and its plan, as the file does" "dana@example.com team" \
      "$(call GET "/providers/$provider" | jq -r '[.facts.identity, .facts.plan] | join(" ")')"
  else
    sleep 20
  fi
  say "INFO $kind: the provider stands at: $(call GET "/providers/$provider" | jq -c '{status,status_detail,facts}')"
  expect "$kind: nothing of the sign-in waits for the unit: no sealed value and no record" "0 0" \
    "$(sql "select count(*) from secrets where owner_kind = 'provider' and owner_id = '$provider' and name like 'login:%'") $(sql "select count(*) from providers where id = '$provider' and login_pending is not null")"
  expect "$kind: the credential is in one file in the unit's own volume" "1" "$(credential_count "$unit" "$marker")"
  local file
  file="$(read_unit "$unit" grep -rlF -- "$marker" /var/lib/orch | head -n 1)"
  expect "$kind: and only its account can read that file" "600" "$(read_unit "$unit" stat -c %a "$file")"
  where_the_credential_is_not "$kind" "$provider" "$unit" "$marker"

  step "$kind: stopped by hand, the unit and the sign-in are kept; started by hand, it is back"
  check "$kind: the runtime is stopped" api POST "/providers/$provider/stop" '{}'
  expect "$kind: its runtime says it is stopped" "stopped" "$(runtime_of "$provider" state)"
  expect "$kind: the unit is stopped and not removed" "false" "$(docker inspect "$unit" --format '{{.State.Running}}' 2> /dev/null || true)"
  check "$kind: the state volume is there" bash -c "docker volume ls -q | grep -qx 'orch-state-$provider'"
  expect "$kind: the provider has no machine while it is stopped" "" "$(sql "select coalesce(machine_id::text, '') from providers where id = '$provider'")"
  check "$kind: the runtime is started" api POST "/providers/$provider/start" '{}'
  runtime_is_back() { [ "$(runtime_of "$provider" state)" = running ] && [ -z "$(runtime_of "$provider" 'job.id // empty')" ]; }
  wait_for 300 "$kind: its runtime is running again" runtime_is_back || true
  expect "$kind: in the same unit" "$unit" "$(unit_of "$runtime")"
  expect "$kind: the sign-in survived the stop" "1" "$(credential_count "$unit" "$marker")"

  step "$kind: signed out, and then deleted with its unit and its volume"
  check "$kind: the harness is signed out" api POST "/providers/$provider/logout"
  expect "$kind: nothing of the credential is left in the unit" "0" "$(credential_count "$unit" "$marker")"
  expect "$kind: the provider needs a sign-in again" "needs_setup" "$(call GET "/providers/$provider" | jq -r .status)"
  check "$kind: the provider is deleted" api DELETE "/providers/$provider"
  unit_is_gone() { [ -z "$(docker ps -aq --filter "label=orch.runtime_id=$runtime")" ]; }
  wait_for 300 "$kind: its unit is gone" unit_is_gone || true
  volume_is_gone() { ! docker volume ls -q | grep -qx "orch-state-$provider"; }
  wait_for 120 "$kind: and its state volume" volume_is_gone || true
}

# where_the_credential_is_not KIND PROVIDER UNIT MARKER: looked for where it must not be, by a search that prints nothing.
where_the_credential_is_not() {
  local kind="$1" provider="$2" unit="$3" marker="$4"
  holds() { "$@" 2> /dev/null | grep -aF -- "$marker" > /dev/null; }
  doesnt_hold() {
    local -a statuses
    "$@" 2> /dev/null | grep -aF -- "$marker" > /dev/null
    statuses=("${PIPESTATUS[@]}")
    [ "${statuses[0]}" = 0 ] && [ "${statuses[1]}" = 1 ]
  }
  database_dump() { compose exec -T postgres pg_dump -U orch orch; }
  services_logs() { compose logs --no-color 2>&1; }
  unit_processes() { read_unit "$unit" sh -c 'cat /proc/[0-9]*/environ /proc/[0-9]*/cmdline'; }
  unit_description() { docker inspect "$unit" && docker logs "$unit" 2>&1; }
  a_line_with_it() { printf 'a line with %s in it\n' "$marker"; }
  check "$kind: the search does find it where it is" holds a_line_with_it
  check "$kind: it is not in the database" doesnt_hold database_dump
  check "$kind: nor in the logs of the services" doesnt_hold services_logs
  check "$kind: nor in the environment or the command line of the processes of the unit" doesnt_hold unit_processes
  check "$kind: nor in what the daemon says of the unit and its logs" doesnt_hold unit_description
}

life_of codex "$CODEX_AGENT" auth_file "$(jq -cn --arg auth "$CODEX_AUTH" '{auth_json:$auth}')" "$MARKER_CODEX"
life_of claude-code "$CLAUDE_AGENT" setup_token "$(jq -cn --arg token "$MARKER_CLAUDE" '{token:$token}')" "$MARKER_CLAUDE"


# --- 3. A real model, once each (only with ORCH_PROVIDER_REAL_ENV=<file with OPENROUTER_API_KEY>) ------------------------

if [ -n "${ORCH_PROVIDER_REAL_ENV:-}" ]; then
  command -v node > /dev/null || abort "node is needed: the socket is followed with it"
  step "each real harness runs in the unchanged provider runtime and makes one request"
  # The key is read inside the subshell only, goes to curl on its standard input and is printed nowhere.
  create_real_provider() {
    (
      set -a
      # shellcheck source=/dev/null
      . "$ORCH_PROVIDER_REAL_ENV"
      set +a
      jq -cn --arg org "$ORG" '{kind:"api",name:"A real model",home_id:$org,
        upstreams:[{protocol:"openai-responses",base_url:"https://openrouter.ai/api/v1",options:{}},{protocol:"anthropic-messages",base_url:"https://openrouter.ai/api",options:{}}],
        api_key: env.OPENROUTER_API_KEY}' |
        curl -sS --max-time 60 -o "$REPLY_FILE" -w '%{http_code}' -X POST "$EDGE/api/v1/providers" -H "authorization: Bearer $TOKEN" -H 'content-type: application/json' -d @-
    )
  }
  expect "the provider was made, with its key" "200" "$(create_real_provider)"
  REAL="$(jq -r .id "$REPLY_FILE")"
  PROVIDERS+=("$REAL")
  real_runtime_is_up() { [ "$(runtime_of "$REAL" state)" = running ]; }
  wait_for 600 "its runtime is running" real_runtime_is_up || true
  check "a model is added" api POST "/providers/$REAL/models" '{"model_name":"openai/gpt-5.6-luna","display_name":"GPT 5.6 Luna","parameters":{"rate":{"input_per_mtok":"0.20","cached_input_per_mtok":"0.02","output_per_mtok":"1.20"}}}'
  check "and another" api POST "/providers/$REAL/models" '{"model_name":"anthropic/claude-haiku-5.5","display_name":"Claude Haiku 5.5","parameters":{"rate":{"input_per_mtok":"0.10","cached_input_per_mtok":"0.01","output_per_mtok":"0.50"}}}'
  real_provider_is_ready() { [ "$(call GET "/providers/$REAL" | jq -r .status)" = ready ]; }
  wait_for 120 "the provider is ready: its runtime asked the upstream for its models with the key" real_provider_is_ready || true
  HOST_ID="$(call GET /hosts | jq -r '.[0].id')"
  DEFAULT_ID="$(printf '%s' "$TEMPLATES" | jq -r '[.[] | select(.name == "Default")][0].id')"
  PROJECT_ID="$(call POST "/orgs/$ORG/projects" '{"name":"Web"}' | jq -r .id)"
  TOPIC="$(call POST "/projects/$PROJECT_ID/topics" '{"name":"Landing page"}' | jq -r .id)"
  MADE="$(call POST /environments "$(jq -cn --arg home "$PROJECT_ID" --arg template "$DEFAULT_ID" --arg host "$HOST_ID" '{name:"Main",home_node_id:$home,template_id:$template,host_id:$host}')")"
  ENV_ID="$(printf '%s' "$MADE" | jq -r .id)"
  ENV_JOB="$(printf '%s' "$MADE" | jq -r '.job.id // empty')"
  env_job_is_over() { case "$(call GET "/jobs/$ENV_JOB" | jq -r .state)" in succeeded | failed | cancelled) true ;; *) false ;; esac; }
  wait_for 900 "the environment is made" env_job_is_over || true
  expect "the environment is running" "running" "$(call GET "/environments/$ENV_ID" | jq -r .status)"
  turn() {
    ORCH_TOKEN="$TOKEN" timeout 420 node "$FOLLOW" --api "$EDGE/api/v1" --ws "ws://127.0.0.1:$EDGE_PORT/api/v1/ws" \
      --thread "$1" --approve once --timeout 400 -- "$2" > "$3"
  }
  today="$(date -u +%F)"
  for pair in "codex:$CODEX_AGENT:openai/gpt-5.6-luna:Codex" "claude-code:$CLAUDE_AGENT:anthropic/claude-haiku-5.5:Claude Code"; do
    IFS=: read -r kind agent model title <<< "$pair"
    if ! real_provider_is_ready; then
      fail "$kind: the provider became unavailable before its real turn"
      continue
    fi
    thread="$(call POST "/topics/$TOPIC/threads" "$(jq -cn --arg name "$title, a real model" --arg agent "$agent" --arg provider "$REAL" --arg env "$ENV_ID" --arg model "$model" --arg dir "real-$kind" \
      '{name:$name,binding:{agent_id:$agent,provider_id:$provider,arrangement:"separated"},model:$model,environment:{use:$env},directory:$dir}')" | jq -r .id)"
    check "$kind: a thread on the real model" test -n "$thread" -a "$thread" != null
    if turn "$thread" "Reply with the single word: ready" "$WORK/real-$kind.jsonl"; then pass "$kind: the turn completed"; else fail "$kind: the turn completed"; cat "$WORK/real-$kind.jsonl" >&2; fi
    check "$kind: the model answered" jq -e 'select(.event == "item" and .kind == "agent_message" and (.summary | test("ready"; "i")))' "$WORK/real-$kind.jsonl"
    expect "$kind: its harness ran with the provider" "1" \
      "$(sql "select count(*) from harnesses h join providers p on p.id = h.provider_id where h.provider_id = '$REAL' and h.agent_id = '$agent' and h.machine_id = p.machine_id")"
    rows="$(call GET "/usage/events?scope=node:$TOPIC&from=$today&to=$today&thread_id=$thread&counted=true&limit=100" | jq -c '[.items[]]')"
    say "INFO $kind: the ledger counted $(printf '%s' "$rows" | jq length) requests to the real model"
    check "$kind: a request is in the ledger with what it cost, counted by the forwarder in the provider's runtime" jq -e 'length >= 1 and all(.[]; .tokens.input_tokens > 0 and .credits != null and .origin == "forwarder")' <<< "$rows"
  done
  # Where the key must not be: read in the subshell of each question, handed to grep as a file descriptor and printed nowhere.
  key_is_in() {
    (
      set -a
      # shellcheck source=/dev/null
      . "$ORCH_PROVIDER_REAL_ENV"
      set +a
      "$@" 2> /dev/null | grep -aF -f /dev/fd/3 3< <(printf '%s\n' "$OPENROUTER_API_KEY") > /dev/null
    )
  }
  key_is_not_in() {
    (
      set -a
      # shellcheck source=/dev/null
      . "$ORCH_PROVIDER_REAL_ENV"
      set +a
      local -a statuses
      "$@" 2> /dev/null | grep -aF -f /dev/fd/3 3< <(printf '%s\n' "$OPENROUTER_API_KEY") > /dev/null
      statuses=("${PIPESTATUS[@]}")
      [ "${statuses[0]}" = 0 ] && [ "${statuses[1]}" = 1 ]
    )
  }
  ENV_UNIT="$(docker ps --filter "label=orch.system=$SYSTEM_NAME" --filter "label=orch.kind=environment" --format '{{.Names}}' | head -n 1)"
  REAL_UNIT="$(unit_of "$(sql "select id from runtimes where provider_id = '$REAL' and state = 'running'")")"
  env_unit_files() { docker exec "$ENV_UNIT" tar -c -C / workspace home var/lib/orch tmp run/orch; }
  env_unit_processes() { docker exec "$ENV_UNIT" sh -c 'cat /proc/[0-9]*/environ /proc/[0-9]*/cmdline'; }
  unit_descriptions() { docker inspect "$ENV_UNIT" "$REAL_UNIT" && docker logs "$ENV_UNIT" 2>&1 && docker logs "$REAL_UNIT" 2>&1; }
  real_unit_processes() { read_unit "$REAL_UNIT" sh -c 'cat /proc/[0-9]*/environ /proc/[0-9]*/cmdline'; }
  services_logs() { compose logs --no-color 2>&1; }
  database_dump() { compose exec -T postgres pg_dump -U orch orch; }
  key_in_a_line() { printf 'a line with %s in it\n' "$OPENROUTER_API_KEY"; }
  check "the search does find the key where it is" key_is_in key_in_a_line
  check "the key is not in the files of the environment's unit" key_is_not_in env_unit_files
  check "nor in the environment or command line of its processes" key_is_not_in env_unit_processes
  check "nor in the environment or command line of the processes of the provider's unit" key_is_not_in real_unit_processes
  check "nor in what the daemon says of the units and their logs" key_is_not_in unit_descriptions
  check "nor in the logs of the services" key_is_not_in services_logs
  check "nor in the database, where it waits for its runtime sealed" key_is_not_in database_dump
fi

# The explicit gates supply human/vendor authority; the default scenario never runs real sign-ins.
# shellcheck source=subscription.sh
. "$HERE/subscription.sh"
run_subscription_proofs

expect "the shipped provider template was never replaced or edited by the scenario" \
  "$(printf '%s' "$SEEDED" | jq -c '{id,kind,driver,spec,revision,enabled}')" \
  "$(call GET "/templates/$TEMPLATE" | jq -c '{id,kind,driver,spec,revision,enabled}')"

step "what the system logged in all this"
expect "the errors the API logged" "0" "$(compose logs --no-color api 2> /dev/null | sed 's/\x1b\[[0-9;]*m//g' | grep -c ' ERROR ' || true)"
