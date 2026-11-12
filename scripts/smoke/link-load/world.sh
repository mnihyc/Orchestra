#!/usr/bin/env bash
# An owned stack of released programs, plus a scripted model and a connected fixture machine.
# Source after lib.sh, set SCENARIO and DEFAULT_PORT, then call proof_world and proof_done from EXIT.

proof_world() {
  : "${ORCH_SMOKE_RELEASE_A:?supply a packaged candidate; this proof never packages implicitly}"
  : "${ORCH_SMOKE_MOCK_MODEL:?supply the already-built orch-mock-llm fixture}"
  A="$ORCH_SMOKE_RELEASE_A"
  [ -f "$A/version.json" ] || abort "the packaged candidate manifest is missing"
  [ -x "$ORCH_SMOKE_MOCK_MODEL" ] || abort "the already-built model fixture is not executable"
  python3 - "$A/version.json" <<'PY'
import json, pathlib, sys
manifest = pathlib.Path(sys.argv[1])
release = json.loads(manifest.read_text())
if release.get('project') != 'orch' or not release.get('assets'):
    raise SystemExit('the supplied directory is not an Orch package')
for asset in release['assets']:
    name = asset['name']
    if pathlib.Path(name).name != name or not (manifest.parent / name).is_file():
        raise SystemExit('a declared candidate asset is missing or has an invalid name')
    if (manifest.parent / name).stat().st_size != asset['size']:
        raise SystemExit('a declared candidate asset has changed size')
PY
  WORK="$SMOKE_DIR/$SCENARIO"
  EVIDENCE="$SMOKE_BASE/evidence/$SMOKE_RUN/$SCENARIO"
  new_stack "$SCENARIO" "$DEFAULT_PORT" "$A"
  mkdir -p "$EVIDENCE" "$WORK/lab" "$WORK/workspace" "$WORK/peer" "$WORK/front"
  local dependency_proof=()
  if [ -n "${ORCH_SMOKE_DEPENDENCY_PROOF:-}" ]; then dependency_proof=(--proof "$ORCH_SMOKE_DEPENDENCY_PROOF"); fi
  python3 "$SMOKE_REPO/scripts/smoke/link-load/dependencies.py" --repo "$SMOKE_REPO" \
    --manifest "$A/version.json" --output "$EVIDENCE/dependencies" "${dependency_proof[@]}"
  chmod 700 "$WORK/lab"
  LAB="${PROJECT}lab"; FRONT_NAME="${PROJECT}front"; CLIENT="${PROJECT}client"
  MODEL_PID=""; LAB_STARTED=0; FRONT_STARTED=0; CLIENT_STARTED=0
  BRIDGE="$(bridge_address)"; FRONT_PORT=$((EDGE_PORT + 2)); MODEL_PORT=$((EDGE_PORT + 3))
  SYSTEM="https://proof.orch.test:$FRONT_PORT"
  sed -i \
    -e "s|^public_url = .*|public_url = \"$SYSTEM\"|" \
    -e "s|wss://gateway.orch.internal:9443|wss://gateway.orch.internal:$GATEWAY_PORT|g" \
    -e "s|wss://gateway:9443|wss://gateway:$GATEWAY_PORT|g" \
    -e "s|^pinned_bind = .*|pinned_bind = \"[::]:$GATEWAY_PORT\"|" "$STACK/config/orch.toml"
  sed -i "s|^ORCH_GATEWAY_PUBLISH=.*|ORCH_GATEWAY_PUBLISH=$BRIDGE:$GATEWAY_PORT:$GATEWAY_PORT|" "$STACK/.env"
  mkdir -p "$WORK/updater-tmp"
  TMPDIR="$WORK/updater-tmp" update_sh first-start --from "$A" > "$EVIDENCE/first-start.log" 2>&1
  setup_admin || abort "the proof's first administrator could not be made"
  wait_for 240 "the released bundled node is online" node_online
  ENV_IMAGE="$(python3 -c 'import json,sys; print(next(i["reference"] for i in json.load(open(sys.argv[1]))["images"] if i["name"] == "orch-env-base"))' "$A/version.json")"
  docker image inspect "$ENV_IMAGE" > "$EVIDENCE/environment-image.json"
  CONNECTOR="$(find "$A" -maxdepth 1 -name "orch-connector-linux-$(uname -m)-musl" -print -quit)"
  [ -n "$CONNECTOR" ] || abort "the candidate has no native Linux connector"
  sha256sum "$CONNECTOR" > "$EVIDENCE/connector.sha256"
  cp "$A/version.json" "$EVIDENCE/version.json"
  git -C "$SMOKE_REPO" rev-parse HEAD > "$EVIDENCE/commit.txt"
  # One explicit pinned endpoint keeps the shaped connector path attributable.
  # Enrollment's CA and machine identity still perform the ordinary TLS checks.
  printf '[connector]\nroles = ["agent", "environment", "provider"]\n[environment]\nroots = ["/workspace"]\nterminal = true\n[ports]\nallowed = ["place"]\n[update]\nauto = false\n[[endpoints]]\nurl = "wss://gateway.orch.internal:%s"\ntrust = "pinned"\n' "$GATEWAY_PORT" > "$WORK/lab/config.toml"
  docker run -d --name "$LAB" --label "org.orch.smoke.run=$SMOKE_RUN" \
    --network "${PROJECT}_control" --user "$(id -u):$(id -g)" \
    -v "$WORK/lab:/data" -v "$WORK/workspace:/workspace" -v "$WORK/peer:/peer" \
    -v "$CONNECTOR:/usr/local/bin/orch-connector:ro" \
    --entrypoint sleep "$ENV_IMAGE" infinity > /dev/null
  LAB_STARTED=1
  MACHINE_TOKEN="$(curl -fsS -H "authorization: Bearer $TOKEN" -H 'content-type: application/json' \
    -X POST "$EDGE/api/v1/machines/enrollment-tokens" -d '{"name_hint":"release-proof"}' | json_field token)"
  docker exec "$LAB" orch-connector --data-dir /data enroll --url "https://gateway.orch.internal:$GATEWAY_PORT" \
    --token "$MACHINE_TOKEN" --name release-proof > "$EVIDENCE/enroll.log" 2>&1
  unset MACHINE_TOKEN
  docker exec "$LAB" orch-connector --data-dir /data policy set provider.upstreams "[\"http://$BRIDGE:$MODEL_PORT/v1\"]" > /dev/null
  docker exec -d "$LAB" sh -c 'exec orch-connector --data-dir /data run > /data/run.log 2>&1'
  openssl rand -hex 16 > "$WORK/model.key"; chmod 600 "$WORK/model.key"
  "$ORCH_SMOKE_MOCK_MODEL" --listen "$BRIDGE:$MODEL_PORT" --credential-file "$WORK/model.key" \
    --model mock-large > "$EVIDENCE/model.log" 2>&1 &
  MODEL_PID=$!
  # The front uses only the stack's fixture authority; verification is never disabled in the byte probe.
  openssl req -new -newkey ec -pkeyopt ec_paramgen_curve:P-256 -nodes -subj /CN=proof.orch.test \
    -keyout "$WORK/front/key.pem" -out "$WORK/front/request.pem" 2> /dev/null
  printf 'subjectAltName=DNS:proof.orch.test\nextendedKeyUsage=serverAuth\n' > "$WORK/front/extensions"
  openssl x509 -req -in "$WORK/front/request.pem" -CA "$STACK/config/secrets/orch_ca_cert.pem" \
    -CAkey "$STACK/config/secrets/orch_ca_key.pem" -CAcreateserial -CAserial "$WORK/front/serial" \
    -days 2 -extfile "$WORK/front/extensions" -out "$WORK/front/leaf.pem" 2> /dev/null
  cat "$WORK/front/leaf.pem" "$STACK/config/secrets/orch_ca_cert.pem" > "$WORK/front/cert.pem"
  cat > "$WORK/front/nginx.conf" <<EOF
events {}
http { server {
  listen 127.0.0.1:$FRONT_PORT ssl;
  listen $BRIDGE:$FRONT_PORT ssl;
  ssl_certificate /front/cert.pem;
  ssl_certificate_key /front/key.pem;
  client_max_body_size 8m;
  location / {
    proxy_pass http://127.0.0.1:$EDGE_PORT;
    proxy_http_version 1.1;
    proxy_set_header Host \$http_host;
    proxy_set_header X-Forwarded-Proto https;
    proxy_set_header Upgrade \$http_upgrade;
    proxy_set_header Connection "upgrade";
    proxy_read_timeout 7200s;
    proxy_send_timeout 7200s;
    proxy_buffering off;
  }
} }
EOF
  NGINX_IMAGE="$(rg -o 'nginx:[^ ]*@sha256:[0-9a-f]+' "$STACK/compose.yml" | head -n 1)"
  [ -n "$NGINX_IMAGE" ] || abort "the release must pin its edge image"
  docker run -d --name "$FRONT_NAME" --label "org.orch.smoke.run=$SMOKE_RUN" --network host \
    -v "$WORK/front:/front:ro" -v "$WORK/front/nginx.conf:/etc/nginx/nginx.conf:ro" "$NGINX_IMAGE" > /dev/null
  FRONT_STARTED=1
  # Credentials stay in this mode-0600 fixture, outside retained evidence.
  (umask 077; printf '%s' "$TOKEN" > "$WORK/access-token")
  python3 "$SMOKE_REPO/scripts/smoke/link-load/world.py" --edge "$EDGE" --system "$SYSTEM" \
    --token-file "$WORK/access-token" --model-key "$WORK/model.key" --model "http://$BRIDGE:$MODEL_PORT/v1" \
    --out "$WORK/world.json" --ca "$STACK/config/secrets/orch_ca_cert.pem"
}

smoke_cleanup_owned_image() {
  [ "${NETEM_IMAGE_BUILD_STARTED:-0}" = 1 ] || return 0
  python3 "$SMOKE_REPO/scripts/smoke/link-load/image_cleanup.py" \
    --tag "$NETEM_IMAGE" --image-id "${NETEM_IMAGE_ID:-}" --run "$SMOKE_RUN" \
    --output "$EVIDENCE/netem-image-cleanup.json"
}

proof_qa_checkpoint() {
  [ "${WSS_QA_STARTED:-0}" = 1 ] || return 0
  if [ "$2" = pre_stop ]; then WSS_QA_PRESTOP_ATTEMPTED=1; fi
  python3 "$SMOKE_REPO/scripts/smoke/link-load/qa.py" checkpoint \
    --work "$WORK" --evidence "$EVIDENCE" --run "$SMOKE_RUN" --phase "$1" --boundary "$2"
}

proof_measurement_mode() {
  local extra=()
  if [ -n "${ORCH_SMOKE_NORMAL_BUILD_RECORD:-}" ]; then extra+=(--normal-record "$ORCH_SMOKE_NORMAL_BUILD_RECORD"); fi
  if [ -n "${ORCH_SMOKE_QA_MANIFEST:-}" ]; then extra+=(--qa-manifest "$ORCH_SMOKE_QA_MANIFEST"); fi
  python3 "$SMOKE_REPO/scripts/smoke/link-load/qa.py" normal --release "$A" --evidence "$EVIDENCE" \
    --run "$SMOKE_RUN" --project "$PROJECT" --api "$(compose ps -q api)" --gateway "$(compose ps -q gateway)" \
    --connector "$LAB" --gateway-port "$GATEWAY_PORT" \
    --machine "$(python3 -c 'import json,sys;print(json.load(open(sys.argv[1]))["machine"])' "$WORK/world.json")" "${extra[@]}"
  [ -n "${ORCH_SMOKE_QA_MANIFEST:-}" ] || return 0
  python3 "$SMOKE_REPO/scripts/smoke/link-load/qa.py" prepare --manifest "$ORCH_SMOKE_QA_MANIFEST" \
    --release "$A" --work "$WORK" --evidence "$EVIDENCE" --run "$SMOKE_RUN"
  python3 "$SMOKE_REPO/scripts/smoke/link-load/qa.py" override --work "$WORK" --evidence "$EVIDENCE" --run "$SMOKE_RUN"
  # Finish all normal gateway/API sessions before any QA recorder is installed.
  compose stop gateway api
  local expected current label machine budget source
  expected="$(python3 -c 'import json,sys;print(json.load(open(sys.argv[1]))["programs"]["connector"]["container_id"])' "$EVIDENCE/normal-bootstrap-identity.json")"
  current="$(docker inspect --format '{{.Id}}' "$LAB")"
  label="$(docker inspect --format '{{index .Config.Labels "org.orch.smoke.run"}}' "$LAB")"
  [ "$current" = "$expected" ] && [ "$label" = "$SMOKE_RUN" ] || abort "normal fixture connector ownership changed"
  docker rm -f "$LAB" > /dev/null
  LAB_STARTED=0
  SMOKE_COMPOSE_FILES="$STACK/compose.yml:$WORK/qa/compose.json"
  compose up -d --force-recreate --no-deps api gateway
  machine="$(python3 -c 'import json,sys;print(json.load(open(sys.argv[1]))["machine"])' "$WORK/world.json")"
  budget="$(python3 -c 'import json,sys;print(json.load(open(sys.argv[1]))["max_commands"])' "$EVIDENCE/qa-preflight.json")"
  source="$(python3 -c 'import json,sys;print(json.load(open(sys.argv[1]))["source"])' "$EVIDENCE/qa-preflight.json")"
  docker run -d --name "$LAB" --label "org.orch.smoke.run=$SMOKE_RUN" \
    --network "${PROJECT}_control" --user "$(id -u):$(id -g)" \
    -v "$WORK/lab:/data" -v "$WORK/workspace:/workspace" -v "$WORK/peer:/peer" \
    -v "$WORK/qa/bin/orch-connector:/usr/local/bin/orch-connector:ro" -v "$WORK/qa/connector:/qa-observers" \
    -e ORCH_TEST_WSS_OBSERVER_DIR=/qa-observers -e "ORCH_TEST_WSS_RUN=$SMOKE_RUN" \
    -e "ORCH_TEST_WSS_MACHINE=$machine" -e "ORCH_TEST_WSS_SOURCE=$source" -e "ORCH_TEST_WSS_MAX_COMMANDS=$budget" \
    --entrypoint sleep "$ENV_IMAGE" infinity > /dev/null
  LAB_STARTED=1
  docker exec -d "$LAB" sh -c 'exec orch-connector --data-dir /data run > /data/run.log 2>&1'
  python3 "$SMOKE_REPO/scripts/smoke/link-load/qa.py" ready --work "$WORK" --evidence "$EVIDENCE" --run "$SMOKE_RUN"
  WSS_QA_STARTED=1
  proof_qa_checkpoint idle start
}

proof_done() {
  local previous=$? final=0 cleanup=0 owned label remaining evidence_ready=0
  # Cleanup must finish collecting failures; an EXIT trap must not stop at the
  # first Docker error and then leave an apparently successful runner receipt.
  trap - EXIT
  set +e
  final="$previous"
  if [ "${SMOKE_STACK_PREPARED:-0}" = 1 ] && [ -n "${EVIDENCE:-}" ]; then
    if python3 "$SMOKE_REPO/scripts/smoke/cleanup.py" validate --repo "$SMOKE_REPO" \
      --base "$SMOKE_BASE" --run "$SMOKE_RUN" --name "$SCENARIO"; then
      evidence_ready=1
    else
      cleanup=1; final=1
    fi
  fi
  if [ -n "${WSS_RESOURCE_PID:-}" ]; then
    kill "$WSS_RESOURCE_PID" 2> /dev/null || true
    wait "$WSS_RESOURCE_PID" || { cleanup=1; final=1; }
  fi
  if [ "$previous" != 0 ]; then FAILED=$((FAILED + 1)); fi
  if [ "${WSS_QA_STARTED:-0}" = 1 ] && [ "$evidence_ready" = 1 ]; then
    if [ "${WSS_QA_PRESTOP_ATTEMPTED:-0}" != 1 ]; then
      proof_qa_checkpoint shutdown pre_stop || { FAILED=$((FAILED + 1)); final=1; }
    fi
    python3 "$SMOKE_REPO/scripts/smoke/link-load/qa.py" stop-connector \
      --work "$WORK" --evidence "$EVIDENCE" --run "$SMOKE_RUN" --container "$LAB" || { FAILED=$((FAILED + 1)); final=1; }
    compose stop gateway api || { cleanup=1; final=1; }
    python3 "$SMOKE_REPO/scripts/smoke/link-load/qa.py" final \
      --work "$WORK" --evidence "$EVIDENCE" --run "$SMOKE_RUN" || { FAILED=$((FAILED + 1)); final=1; }
  fi
  if [ "$evidence_ready" = 1 ]; then
    python3 - "$SMOKE_REPO" "$EVIDENCE/harness-final.json" <<'PY' || { FAILED=$((FAILED + 1)); final=1; }
import json,subprocess,sys
source=subprocess.check_output(['git','-C',sys.argv[1],'rev-parse','HEAD'],text=True).strip()
clean=not subprocess.check_output(['git','-C',sys.argv[1],'status','--porcelain'],text=True).strip()
with open(sys.argv[2],'w') as target: json.dump({'source':source,'clean':clean},target)
PY
    compose logs --no-color > "$EVIDENCE/stack.log" 2>&1 || true
    local record
    for record in "$WORK/client/"*.json "$WORK/peer/"*.json; do
      if [ -f "$record" ]; then cp "$record" "$EVIDENCE/"; fi
    done
    if [ "${LAB_STARTED:-0}" = 1 ]; then
      docker cp "$LAB:/data/run.log" "$EVIDENCE/connector.log" > /dev/null 2>&1 || true
    fi
  fi
  if [ "${ORCH_SMOKE_KEEP:-0}" != 1 ]; then
    if [ -n "${MODEL_PID:-}" ]; then
      kill "$MODEL_PID" 2> /dev/null || true
      wait "$MODEL_PID" 2> /dev/null
      kill -0 "$MODEL_PID" 2> /dev/null && cleanup=1
    fi
    for owned in "${CLIENT:-}" "${FRONT_NAME:-}" "${LAB:-}"; do
      [ -n "$owned" ] || continue
      remaining="$(docker ps -aq --filter "name=^/${owned}$")" || { cleanup=1; continue; }
      [ -n "$remaining" ] || continue
      label="$(docker inspect --format '{{index .Config.Labels "org.orch.smoke.run"}}' "$owned")"
      if [ "$label" != "$SMOKE_RUN" ]; then cleanup=1; continue; fi
      docker rm -f "$owned" > /dev/null || cleanup=1
    done
    if [ -n "${PROBE_PID:-}" ]; then
      kill "$PROBE_PID" 2> /dev/null || true
      wait "$PROBE_PID" 2> /dev/null
      kill -0 "$PROBE_PID" 2> /dev/null && cleanup=1
    fi
    remaining="$(docker ps -aq --filter "label=org.orch.smoke.run=$SMOKE_RUN")" || cleanup=1
    [ -z "$remaining" ] || cleanup=1
  fi
  if [ "$cleanup" = 0 ] && [ "${SMOKE_STACK_PREPARED:-0}" = 1 ] && [ -n "${STACK:-}" ]; then
    # finish verifies Compose absence before executing the preflighted literal
    # file cleanup. Any failure retains the fixture files and credentials.
    finish || final=1
    if [ "${ORCH_SMOKE_KEEP:-0}" != 1 ] && [ "${SMOKE_CLEANUP_COMPLETE:-0}" != 1 ]; then cleanup=1; fi
  fi
  if [ "${SMOKE_STACK_PREPARED:-0}" != 1 ]; then
    # No successful prelaunch ownership receipt exists. Do not describe a
    # failed/unperformed setup as completed teardown or touch prior-run evidence.
    cleanup=1; final=1
  fi
  if [ "$cleanup" != 0 ]; then
    FAILED=$((FAILED + 1)); final=1; WSS_GATE_QUALIFIED=false
  fi
  if [ "${ORCH_SMOKE_KEEP:-0}" = 1 ]; then
    WSS_GATE_QUALIFIED=false
    if [ "${ORCH_SMOKE_RELEASE_PROOFS:-0}" = 1 ]; then final=1; fi
  fi
  if [ "$FAILED" -gt 0 ] && [ "$final" = 0 ]; then final=1; fi
  if [ "$final" != 0 ]; then WSS_GATE_QUALIFIED=false; fi
  if [ "$evidence_ready" = 1 ]; then
    printf '{"complete":%s,"retained_by_request":%s,"exit":%s}\n' \
      "$([ "$cleanup" = 0 ] && [ "${ORCH_SMOKE_KEEP:-0}" != 1 ] && printf true || printf false)" \
      "$([ "${ORCH_SMOKE_KEEP:-0}" = 1 ] && printf true || printf false)" "$cleanup" > "$EVIDENCE/cleanup.json" || final=1
    if [ "$final" != 0 ]; then WSS_GATE_QUALIFIED=false; fi
    printf '{"checks":%s,"failures":%s,"process_exit":%s,"final_exit":%s,"executed":true,"wss_release_gate_qualified":%s}\n' \
      "$CHECKED" "$FAILED" "$previous" "$final" "${WSS_GATE_QUALIFIED:-null}" > "$EVIDENCE/runner.json" || final=1
  fi
  exit "$final"
}
