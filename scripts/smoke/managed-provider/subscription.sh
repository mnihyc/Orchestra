#!/usr/bin/env bash
# Explicitly gated subscription sign-in and restart proofs, sourced by the packaged-stack scenarios.
# Uses only the scenario's API, managed units and operator-provided JSON; never a machine's harness home.

# First image/build downloads already have a twenty-minute budget in the surrounding scenario.
SUBSCRIPTION_RUNTIME_WAIT_SECS=1200
# A vendor device flow can live for fifteen minutes; allow another fifteen for its expiry observation.
SUBSCRIPTION_SIGNIN_WAIT_SECS="${ORCH_SMOKE_SIGNIN_WAIT_SECS:-1800}"
# An operator can extend a human flow up to two hours, keeping even a wrong setting bounded.
SUBSCRIPTION_SIGNIN_MAX_WAIT_SECS=7200
# Match the scenario library's low-frequency polling while a human acts at the vendor.
SUBSCRIPTION_POLL_SECS=3
# Enough for one 64 KiB credential with JSON escaping, without accepting an unbounded upload.
SUBSCRIPTION_INPUT_MAX_BYTES=$((512 * 1024))
SUBSCRIPTION_PROVIDER=""
SUBSCRIPTION_TOPIC=""
SUBSCRIPTION_PROVIDERS=()

subscription_require() {
  local label="$1"; shift
  if "$@" > /dev/null 2>&1; then pass "$label"; else fail "$label"; return 1; fi
}

subscription_uuid() { [[ "$1" =~ ^[0-9a-fA-F]{8}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{12}$ ]]; }

subscription_make() {
  local kind="$1" agent="$2"
  SUBSCRIPTION_PROVIDER="$(call POST /providers "$(jq -cn --arg org "$ORG" --arg agent "$agent" --arg name "Subscription proof: $kind" '{kind:"subscription",name:$name,home_id:$org,agent_id:$agent}')" | jq -r .id)"
  subscription_require "$kind: a fresh managed subscription was made through Orch" subscription_uuid "$SUBSCRIPTION_PROVIDER" || return 1
  SUBSCRIPTION_PROVIDERS+=("$SUBSCRIPTION_PROVIDER")
  subscription_runtime_up() { [ "$(call GET "/providers/$SUBSCRIPTION_PROVIDER" | jq -r '.runtime.state')" = running ]; }
  wait_for "$SUBSCRIPTION_RUNTIME_WAIT_SECS" "$kind: its managed runtime is running" subscription_runtime_up
}

subscription_method() {
  local agent="$1" kind="$2" requested="${3:-}"
  call GET "/providers/setup?org_id=$ORG" | jq -er --arg agent "$agent" --arg kind "$kind" --arg requested "$requested" '
    [.subscriptions[] | select(.agent.id == $agent) | .methods[] | select(.kind == $kind)
      | select($requested == "" or .id == $requested)]
    | (map(select(.recommended))[0] // .[0]).id | select(type == "string" and length > 0)'
}

subscription_wait_login() {
  local provider="$1" wanted="$2" deadline=$((SECONDS + SUBSCRIPTION_SIGNIN_WAIT_SECS)) reply state shown="" next
  while [ "$SECONDS" -lt "$deadline" ]; do
    reply="$(call GET "/providers/$provider/login")" || return 1
    state="$(printf '%s' "$reply" | jq -r .state)"
    next="$(printf '%s' "$reply" | jq -c '{state,shows,asks}')"
    if [ "$next" != "$shown" ]; then
      shown="$next"
      # These are the same address/code/instructions the product displays, never an imported value.
      printf '%s' "$reply" | jq -r '.shows[]? | "\(.label): \(.value)"'
      if [ "$(printf '%s' "$reply" | jq '.asks // [] | length')" -gt 0 ]; then
        say "Complete the declared fields on $EDGE/resources/providers/$provider?tab=key"
      fi
    fi
    case "$state:$wanted" in
      failed:refused | expired:refused) pass "the native flow was rejected or expired"; return 0 ;;
      completed:completed) pass "the product's declared sign-in completed"; return 0 ;;
      completed:refused) fail "the refusal flow was authorized instead"; return 1 ;;
      failed:completed | expired:completed) fail "the authorized sign-in did not complete"; return 1 ;;
    esac
    sleep "$SUBSCRIPTION_POLL_SECS"
  done
  fail "the sign-in did not reach the required outcome within its explicit wait budget"
  return 1
}

subscription_native_login() {
  local provider="$1" agent="$2" method body
  method="$(subscription_method "$agent" interactive)" || { fail "this harness reported no interactive native method"; return 1; }
  body="$(jq -cn --arg method "$method" '{method:$method}')"
  subscription_require "the product begins its real native method" api POST "/providers/$provider/login" "$body" || return 1
  subscription_require "the native flow can be cancelled through Orch" api POST "/providers/$provider/login/cancel" || return 1
  subscription_cancelled() { [ "$(call GET "/providers/$provider/login" | jq -r .state)" = idle ]; }
  wait_for 60 "the cancelled flow is idle" subscription_cancelled || return 1
  say "Reject the next vendor sign-in, or let its shown expiry elapse. Do not authorize this first flow."
  subscription_require "a separate native refusal flow begins" api POST "/providers/$provider/login" "$body" || return 1
  subscription_wait_login "$provider" refused || return 1
  say "Now authorize the next native sign-in with the dedicated test account through the product."
  subscription_require "a fresh native sign-in begins after refusal" api POST "/providers/$provider/login" "$body" || return 1
  subscription_wait_login "$provider" completed
}

subscription_import_login() {
  local provider="$1" agent="$2" input="$3" file permission method status
  [ -n "$input" ] || { fail "an explicit operator-owned login JSON file is required"; return 1; }
  file="$(realpath -- "$input")" || return 1
  case "$file" in */.codex/* | */.claude/*) fail "a machine's harness home cannot supply this proof"; return 1 ;; esac
  [ -f "$file" ] && [ -r "$file" ] || return 1
  permission="$(stat -c %a -- "$file")" || return 1
  [ "$(stat -c %u -- "$file")" = "$(id -u)" ] || return 1
  [ "$((8#$permission & 077))" -eq 0 ] || { fail "the supplied credential JSON must be readable only by its owner"; return 1; }
  [ "$(wc -c < "$file")" -le "$SUBSCRIPTION_INPUT_MAX_BYTES" ] || return 1
  jq -e 'type == "object" and (.method | type == "string") and (.values | type == "object" and length > 0 and all(.[]; type == "string"))' "$file" > /dev/null 2>&1 || return 1
  method="$(jq -er .method "$file")" || return 1
  subscription_method "$agent" import "$method" > /dev/null || { fail "the supplied method is not a declared import"; return 1; }
  subscription_require "the scenario's API session is current before the private import" sign_in || return 1
  # Only the file name is an argument. No credential is a shell argument, environment value or transcript.
  status="$(curl -sS --max-time 60 -o "$REPLY_FILE" -w '%{http_code}' -X POST "$EDGE/api/v1/providers/$provider/login" \
    -H "authorization: Bearer $TOKEN" -H 'content-type: application/json' --data-binary "@$file")" || return 1
  case "$status" in 2??) ;; *) fail "the import was refused (HTTP $status); its body is withheld"; return 1 ;; esac
  subscription_wait_login "$provider" completed
}

subscription_context() {
  local project template
  project="$(call POST "/orgs/$ORG/projects" '{"name":"Subscription proof"}' | jq -r .id)"
  subscription_uuid "$project" || return 1
  SUBSCRIPTION_TOPIC="$(call POST "/projects/$project/topics" '{"name":"Native subscription turns"}' | jq -r .id)"
  subscription_uuid "$SUBSCRIPTION_TOPIC" || return 1
  if [ -z "${ENV_ID:-}" ]; then
    template="$(call GET /templates | jq -er '[.[] | select(.name == "Default" and .kind == "environment" and .enabled)][0].id')" || return 1
    ENV_ID="$(call POST /environments "$(jq -cn --arg home "$project" --arg template "$template" '{name:"Subscription proof",home_node_id:$home,template_id:$template}')" | jq -r .id)"
    subscription_uuid "$ENV_ID" || return 1
  fi
  subscription_environment_up() { [ "$(call GET "/environments/$ENV_ID" | jq -r .status)" = running ]; }
  wait_for "$SUBSCRIPTION_RUNTIME_WAIT_SECS" "the default managed environment is running" subscription_environment_up
}

subscription_turn() {
  local thread="$1" log="$2"
  subscription_require "the scenario's API session is current before following the turn" sign_in || return 1
  ORCH_TOKEN="$TOKEN" timeout 420 node "$FOLLOW" --api "$EDGE/api/v1" --ws "ws://127.0.0.1:$EDGE_PORT/api/v1/ws" \
    --thread "$thread" --approve once --timeout 400 -- "Use the shell tool to run printf ready, then reply with the single word ready." > "$log" || return 1
  subscription_require "the real subscription harness called a tool" jq -se 'any(.[]; .event == "item" and .kind == "tool_call")' "$log" || return 1
  subscription_require "the real subscription model answered" jq -se 'any(.[]; .event == "item" and .kind == "agent_message" and (.summary | test("ready"; "i")))' "$log"
}

subscription_ledger() {
  local thread="$1" today
  today="$(date -u +%F)"
  call GET "/usage/events?scope=node:$SUBSCRIPTION_TOPIC&from=$today&to=$today&thread_id=$thread&counted=true&limit=100"
}

subscription_case() {
  local mode="$1" kind="$2" agent="$3" input="$4" model="$5" provider runtime unit model_id thread before_rows after_rows first last log proof read_cut
  subscription_make "$kind" "$agent" || return 1
  provider="$SUBSCRIPTION_PROVIDER"
  runtime="$(sql "select id from runtimes where provider_id = '$provider' and state = 'running'")"
  subscription_uuid "$runtime" || return 1
  unit="$(docker ps --filter "label=orch.runtime_id=$runtime" --format '{{.Names}}')"
  [ -n "$unit" ] || return 1
  if [ "$mode" = native ]; then subscription_native_login "$provider" "$agent" || return 1
  else subscription_import_login "$provider" "$agent" "$input" || return 1; fi
  read_cut="$(date -u +%Y-%m-%dT%H:%M:%S.000Z)"
  subscription_require "$kind: current credential facts are read after sign-in" api POST "/providers/$provider/refresh" '{"retry":true}' || return 1
  call GET "/providers/$provider" | jq -e --arg cut "$read_cut" '.facts.credential_verified == true and .facts_at != null and .facts_at >= $cut' > /dev/null || { fail "$kind: no current authenticated credential observation was supplied"; return 1; }
  if [ -n "$model" ]; then
    model_id="$(call GET "/providers/$provider" | jq -r --arg model "$model" '[.models[] | select(.model_name == $model)][0].id // empty')"
    if [ -z "$model_id" ]; then
      subscription_require "$kind: the explicitly chosen model is entered" api POST "/providers/$provider/models" "$(jq -cn --arg model "$model" '{model_name:$model,display_name:$model}')" || return 1
    else
      subscription_require "$kind: the chosen model is explicitly enabled" api PATCH "/providers/$provider/models/$model_id" '{"enabled":true}' || return 1
    fi
    thread="$(call POST "/topics/$SUBSCRIPTION_TOPIC/threads" "$(jq -cn --arg kind "$kind" --arg provider "$provider" --arg agent "$agent" --arg env "$ENV_ID" --arg model "$model" \
      '{name:("Subscription proof: " + $kind),binding:{provider_id:$provider,agent_id:$agent,arrangement:"separated"},environment:{use:$env},directory:("subscription-" + $kind),model:$model}')" | jq -r .id)"
    subscription_uuid "$thread" || return 1
    log="$WORK/subscription-$mode-$kind-first.jsonl"
    subscription_turn "$thread" "$log" || { fail "$kind: the first real subscription turn did not complete"; return 1; }
    before_rows="$(subscription_ledger "$thread")"
    first="$(printf '%s' "$before_rows" | jq '[.items[] | select(.purpose == "turn" and .tokens.total_tokens > 0) | .turn_ordinal] | max // 0')"
    [ "$first" -gt 0 ] || { fail "$kind: the first turn has no counted ledger row"; return 1; }
  fi
  # This stop is an explicit smoke-proof action, after the turn ended; renewal never requests it.
  subscription_require "$kind: its managed runtime is stopped explicitly" api POST "/providers/$provider/stop" '{}' || return 1
  subscription_require "the packaged API is restarted" compose restart api || return 1
  subscription_api_up() { curl -fsS "$EDGE/health" > /dev/null; }
  wait_for 90 "the API answers after restart" subscription_api_up || return 1
  subscription_require "$kind: the same provider runtime is started" api POST "/providers/$provider/start" '{}' || return 1
  subscription_runtime_up() { [ "$(call GET "/providers/$provider" | jq -r '.runtime.state')" = running ]; }
  wait_for "$SUBSCRIPTION_RUNTIME_WAIT_SECS" "$kind: its runtime returns" subscription_runtime_up || return 1
  subscription_require "$kind: the runtime keeps the same unit" docker inspect "$unit" || return 1
  subscription_require "$kind: its runtime row is unchanged" test "$runtime" = "$(sql "select id from runtimes where provider_id = '$provider' and state = 'running'")" || return 1
  if [ -n "$model" ]; then
    log="$WORK/subscription-$mode-$kind-after-restart.jsonl"
    subscription_turn "$thread" "$log" || { fail "$kind: the real subscription turn after restart did not complete"; return 1; }
    after_rows="$(subscription_ledger "$thread")"
    last="$(printf '%s' "$after_rows" | jq '[.items[] | select(.purpose == "turn" and .tokens.total_tokens > 0) | .turn_ordinal] | max // 0')"
    [ "$last" -gt "$first" ] || { fail "$kind: restart produced no later counted turn"; return 1; }
    pass "$kind: the actual subscription and real harness completed a later counted turn after restart"
  else
    read_cut="$(date -u +%Y-%m-%dT%H:%M:%S.000Z)"
    subscription_require "$kind: its native credential is reread after restart" api POST "/providers/$provider/refresh" '{"retry":true}' || return 1
    call GET "/providers/$provider" | jq -e --arg cut "$read_cut" '.facts.credential_verified == true and .facts_at != null and .facts_at >= $cut' > /dev/null || return 1
  fi
  proof="$WORK/subscription-$mode-$kind-proof.json"
  jq -n --arg mode "$mode" --arg kind "$kind" --arg provider "$provider" --arg runtime "$runtime" --arg thread "${thread:-}" \
    --argjson first "${first:-0}" --argjson last "${last:-0}" '{mode:$mode,harness:$kind,provider_id:$provider,runtime_id:$runtime,thread_id:$thread,first_turn:$first,after_restart_turn:$last,api_restarted:true,same_runtime:true}' > "$proof" || return 1
  subscription_require "$kind: the proof leaves no pending login material" test "$(sql "select count(*) from providers where id = '$provider' and login_pending is not null")" = 0 || return 1
  subscription_require "$kind: the proof signs out its dedicated test account" api POST "/providers/$provider/logout" || return 1
  subscription_require "$kind: the proof's provider is deleted" api DELETE "/providers/$provider" || return 1
  subscription_removed() { [ -z "$(docker ps -aq --filter "label=orch.runtime_id=$runtime")" ]; }
  wait_for 300 "$kind: the proof's exact runtime is removed" subscription_removed
}

run_subscription_proofs() {
  local native="${ORCH_SMOKE_NATIVE_SIGNIN:-0}" imported="${ORCH_SMOKE_SUBSCRIPTION_IMPORT:-0}" mode
  [ "$native" = 1 ] || [ "$imported" = 1 ] || return 0
  [ "$native" != 1 ] || [ "$imported" != 1 ] || abort "choose native sign-in or import for one run"
  [[ "$SUBSCRIPTION_SIGNIN_WAIT_SECS" =~ ^[1-9][0-9]*$ ]] || abort "the sign-in wait must be positive seconds"
  [ "${#SUBSCRIPTION_SIGNIN_WAIT_SECS}" -le 4 ] && [ "$SUBSCRIPTION_SIGNIN_WAIT_SECS" -le "$SUBSCRIPTION_SIGNIN_MAX_WAIT_SECS" ] || abort "the sign-in wait exceeds two hours"
  command -v node > /dev/null || abort "node is needed to follow the real turns"
  command -v realpath > /dev/null || abort "realpath is needed to validate explicit input paths"
  umask 077
  if [ "$native" = 1 ]; then mode=native; else mode=import; fi
  if [ "$mode" = import ]; then
    [ -n "${ORCH_SMOKE_CODEX_MODEL:-}" ] && [ -n "${ORCH_SMOKE_CLAUDE_MODEL:-}" ] || abort "each import proof needs an explicitly chosen model"
  fi
  if [ -n "${ORCH_SMOKE_CODEX_MODEL:-}${ORCH_SMOKE_CLAUDE_MODEL:-}" ]; then subscription_context || abort "the subscription proof context could not be made"; fi
  step "explicit $mode proof with the product's managed subscriptions"
  subscription_case "$mode" codex "$CODEX_AGENT" "${ORCH_SMOKE_CODEX_LOGIN_JSON:-}" "${ORCH_SMOKE_CODEX_MODEL:-}" || abort "the Codex subscription proof did not pass"
  subscription_case "$mode" claude-code "$CLAUDE_AGENT" "${ORCH_SMOKE_CLAUDE_LOGIN_JSON:-}" "${ORCH_SMOKE_CLAUDE_MODEL:-}" || abort "the Claude Code subscription proof did not pass"
  if [ -n "$SUBSCRIPTION_TOPIC" ]; then subscription_require "the proof's thread topic is removed" api DELETE "/nodes/$SUBSCRIPTION_TOPIC" || abort "the proof topic could not be removed"; fi
}
