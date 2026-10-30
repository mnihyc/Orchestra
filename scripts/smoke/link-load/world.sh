#!/usr/bin/env bash
# An owned stack of released programs, plus a scripted model and a connected fixture machine.
# Source after lib.sh, set SCENARIO and DEFAULT_PORT, then call proof_world and proof_done from EXIT.

proof_world() {
  : "${ORCH_SMOKE_RELEASE_A:?supply a packaged candidate; this proof never packages implicitly}"
  : "${ORCH_SMOKE_MOCK_MODEL:?supply the already-built orch-mock-llm fixture}"
  A="$ORCH_SMOKE_RELEASE_A"
  new_stack "$SCENARIO" "$DEFAULT_PORT" "$A"
  WORK="$(dirname "$STACK")"
  EVIDENCE="$SMOKE_BASE/evidence/$SMOKE_RUN/$SCENARIO"
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
  update_sh first-start --from "$A" > "$EVIDENCE/first-start.log" 2>&1
  setup_admin || abort "the proof's first administrator could not be made"
  wait_for 240 "the released bundled node is online" node_online
  ENV_IMAGE="$(python3 -c 'import json,sys; print(next(i["reference"] for i in json.load(open(sys.argv[1]))["images"] if i["name"] == "orch-env-base"))' "$A/version.json")"
  docker image inspect "$ENV_IMAGE" > "$EVIDENCE/environment-image.json"
  CONNECTOR="$(find "$A" -maxdepth 1 -name "orch-connector-linux-$(uname -m)-musl" -print -quit)"
  [ -n "$CONNECTOR" ] || abort "the candidate has no native Linux connector"
  sha256sum "$CONNECTOR" > "$EVIDENCE/connector.sha256"
  cp "$A/version.json" "$EVIDENCE/version.json"
  git -C "$SMOKE_REPO" rev-parse HEAD > "$EVIDENCE/commit.txt"
  printf '[connector]\nroles = ["agent", "environment", "provider"]\n[environment]\nroots = ["/workspace"]\nports = true\n[update]\nauto = false\n' > "$WORK/lab/config.toml"
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

proof_done() {
  local previous=$? final=0
  final="$previous"
  if [ "$previous" != 0 ]; then FAILED=$((FAILED + 1)); fi
  if [ -n "${EVIDENCE:-}" ]; then
    compose logs --no-color > "$EVIDENCE/stack.log" 2>&1 || true
    local record
    for record in "$WORK/client/"*.json "$WORK/peer/"*.json; do
      if [ -f "$record" ]; then cp "$record" "$EVIDENCE/"; fi
    done
    if [ "${LAB_STARTED:-0}" = 1 ]; then
      docker cp "$LAB:/data/run.log" "$EVIDENCE/connector.log" > /dev/null 2>&1 || true
    fi
    printf '{"checks":%s,"failures":%s,"process_exit":%s,"executed":true}\n' "$CHECKED" "$FAILED" "$previous" > "$EVIDENCE/runner.json"
  fi
  if [ "${ORCH_SMOKE_KEEP:-0}" != 1 ]; then
    if [ -n "${MODEL_PID:-}" ]; then kill "$MODEL_PID" 2> /dev/null || true; fi
    if [ "${CLIENT_STARTED:-0}" = 1 ]; then docker rm -f "$CLIENT" > /dev/null; fi
    if [ "${FRONT_STARTED:-0}" = 1 ]; then docker rm -f "$FRONT_NAME" > /dev/null; fi
    if [ "${LAB_STARTED:-0}" = 1 ]; then docker rm -f "$LAB" > /dev/null; fi
  fi
  if [ -n "${STACK:-}" ]; then finish || final=1; fi
  if [ "$FAILED" -gt 0 ] && [ "$final" = 0 ]; then final=1; fi
  exit "$final"
}
