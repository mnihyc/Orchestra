#!/usr/bin/env bash
# Released API/gateway/connector, 100 MiB each way, a packet cut, an abandoned stream and new admission.
set -Eeuo pipefail
. "$(dirname "${BASH_SOURCE[0]}")/../lib.sh"
. "$SMOKE_REPO/scripts/smoke/link-load/world.sh"
HERE="$SMOKE_REPO/scripts/smoke/link-load"
SCENARIO=linkload; DEFAULT_PORT=41400
# Shape exactly one path. Two150-ms legs would be600-ms RTT, not the300-ms profile.
PLACEMENT="${ORCH_LINK_SHAPE:-client}"
case "$PLACEMENT" in client|connector) ;; *) abort "ORCH_LINK_SHAPE must be client or connector" ;; esac
# This runner can collect real-byte/mixed-progress evidence, but currently lacks
# whole-session application queue observations. Never call its exit0 a full gate.
WSS_GATE_QUALIFIED=false
WSS_RESOURCE_PID=""
trap proof_done EXIT
proof_world

docker cp "$HERE/peer.py" "$LAB:/tmp/release-peer.py"
docker exec -d "$LAB" python3 /tmp/release-peer.py --port 41325 --out /peer
wait_for 30 "the native TCP fixture is listening beside the released connector" test -f "$WORK/peer/ready"
mkdir -p "$WORK/client" "$WORK/client-fixture"
cp "$WORK/world.json" "$WORK/client-fixture/world.json"
cp "$WORK/access-token" "$WORK/client-fixture/access-token"
cp "$STACK/config/secrets/orch_ca_cert.pem" "$WORK/client-fixture/ca.pem"
docker run -d --name "$CLIENT" --label "org.orch.smoke.run=$SMOKE_RUN" \
  --network "${PROJECT}_control" --add-host "proof.orch.test:$BRIDGE" --user "$(id -u):$(id -g)" \
  -v "$HERE:/proof:ro" -v "$WORK/client-fixture:/fixture:ro" -v "$WORK/client:/result" \
  --entrypoint sleep "$ENV_IMAGE" infinity > /dev/null
CLIENT_STARTED=1
SHAPED="$CLIENT"
if [ "$PLACEMENT" = connector ]; then SHAPED="$LAB"; fi
docker cp "$HERE/socket_facts.py" "$LAB:/tmp/release-socket-facts.py"
docker exec "$LAB" python3 /tmp/release-socket-facts.py --gateway-port "$GATEWAY_PORT" > "$EVIDENCE/connector-socket-before.json"
python3 - "$EVIDENCE/connector-socket-before.json" <<'PY'
import json, sys
facts = json.load(open(sys.argv[1]))
if not facts['gateway_tcp_connections'] or not facts['connector_processes']:
    raise SystemExit('the fixture has no actual connector TCP session to its sole pinned gateway')
PY
python3 - "$PLACEMENT" "$SHAPED" "$LAB" "$CLIENT" "$GATEWAY_PORT" > "$EVIDENCE/shaping-placement.json" <<'PY'
import json, sys
print(json.dumps({'placement': sys.argv[1], 'shaped_container': sys.argv[2],
    'connector': sys.argv[3], 'client': sys.argv[4], 'gateway_port': int(sys.argv[5]),
    'scope': 'whole owned container namespace', 'delay_each_direction_ms': 150,
    'loss_each_direction_percent': 1, 'application_queue_bound_observed': False}, indent=2))
PY
docker inspect --format '{{json .NetworkSettings.Networks}}' "$SHAPED" > "$EVIDENCE/shaped-networks.json"
docker inspect --format '{{json .Id}}' "$SHAPED" > "$EVIDENCE/shaped-container-id.json"
docker exec "$SHAPED" python3 -c 'import json,os;print(json.dumps(os.readlink("/proc/self/ns/net")))' > "$EVIDENCE/shaped-netns.json"
NETEM_IMAGE="${PROJECT}netem"
docker build --tag "$NETEM_IMAGE" --file "$HERE/netem.Dockerfile" "$HERE" > "$EVIDENCE/netem-build.log" 2>&1
docker image inspect "$NETEM_IMAGE" > "$EVIDENCE/netem-image.json"
tc_own() { docker run --rm --cap-add NET_ADMIN --network "container:$SHAPED" "$NETEM_IMAGE" "$@"; }
ip_own() { docker run --rm --cap-add NET_ADMIN --network "container:$SHAPED" --entrypoint ip "$NETEM_IMAGE" "$@"; }
# IFB lives in the selected owned namespace. Never change the host root qdisc.
ip_own link add ifb0 type ifb > "$EVIDENCE/ifb.log" 2>&1 || abort "the selected isolated namespace needs kernel IFB support (see ifb.log)"
ip_own link set ifb0 up
tc_own qdisc add dev eth0 handle ffff: ingress
tc_own filter add dev eth0 parent ffff: protocol all matchall action mirred egress redirect dev ifb0
shape() {
  tc_own qdisc replace dev eth0 root netem delay 150ms loss "$3" rate "$1" limit 1024
  tc_own qdisc replace dev ifb0 root netem delay 150ms loss "$3" rate "$2" limit 1024
}
shape 750kbit 1600kbit 1%
tc_own -s -j qdisc show dev eth0 > "$EVIDENCE/upload-qdisc.json"
tc_own -s -j qdisc show dev ifb0 > "$EVIDENCE/download-qdisc.json"
API_CONTAINER="$(compose ps -q api)"; GATEWAY_CONTAINER="$(compose ps -q gateway)"
[ -n "$API_CONTAINER" ] && [ -n "$GATEWAY_CONTAINER" ] || abort "the owned API/gateway containers must be running"
printf 'idle\n' > "$WORK/link-phase"
sample_resources() {
  python3 "$HERE/monitor.py" --connector "$LAB" --containers "$LAB" "$API_CONTAINER" "$GATEWAY_CONTAINER" \
    --gateway-port "$GATEWAY_PORT" --out "$EVIDENCE/resources.jsonl" --phase-file "$WORK/link-phase" "$@"
}
sample_resources --once
python3 "$HERE/monitor.py" --connector "$LAB" --containers "$LAB" "$API_CONTAINER" "$GATEWAY_CONTAINER" \
  --gateway-port "$GATEWAY_PORT" --out "$EVIDENCE/resources.jsonl" --phase-file "$WORK/link-phase" &
WSS_RESOURCE_PID=$!

probe() {
  local name="$1" code=0; shift
  printf '%s\n' "$name" > "$WORK/link-phase"
  sample_resources --once
  docker exec "$CLIENT" python3 /proof/client.py --world /fixture/world.json --out "/result/$name.json" --mixed "$@" \
    > "$EVIDENCE/$name.log" 2>&1 || code=$?
  printf '{"phase":"%s","exit":%s}\n' "$name" "$code" >> "$EVIDENCE/exits.jsonl"
  return "$code"
}
has_result() { test -f "$WORK/client/$1.json"; }
has_started() { test -f "$WORK/client/$1.started"; }
peers_closed() {
  python3 - "$WORK/client/$1.json" "$WORK/peer" <<'PY'
import json, pathlib, sys
rows = json.load(open(sys.argv[1]))['streams']
raise SystemExit(not all((pathlib.Path(sys.argv[2]) / (row['seed'] + '.json')).exists() for row in rows))
PY
}
joined() {
  wait_for 310 "both ends of $1 closed" peers_closed "$1"
  check "$1: independent counts and hashes agree, no producer stuck" python3 "$HERE/verify.py" \
    --client "$WORK/client/$1.json" --peer "$WORK/peer" --require-mixed \
    --qualification "$EVIDENCE/$1-qualification.json"
  printf '%s-drained\n' "$1" > "$WORK/link-phase"
  sample_resources --once
}

step "three concurrent streams, each 100 MiB in each direction, with a stalled reader and a real packet cut"
probe concurrent --streams 3 --stall &
PROBE_PID=$!
wait_for 120 "all three actual port sockets are carrying bytes" has_started concurrent
shape 750kbit 1600kbit 100%
date -u +%FT%TZ > "$EVIDENCE/packet-cut-start.txt"
sleep 3 # The scenario's intentional network outage, not a presumed product completion.
shape 750kbit 1600kbit 1%
date -u +%FT%TZ > "$EVIDENCE/packet-cut-end.txt"
if wait "$PROBE_PID"; then pass "concurrent transfers returned"; else fail "concurrent transfer process failed"; fi
joined concurrent

step "an abandoned stream closes, then a fresh admission carries distinct bytes"
probe cut --streams 1 --cut
joined cut
probe reconnect --streams 1
joined reconnect
tc_own -s -j qdisc show dev eth0 > "$EVIDENCE/profile-upload-qdisc-final.json"
tc_own -s -j qdisc show dev ifb0 > "$EVIDENCE/profile-download-qdisc-final.json"
step "the floor preserves bytes too, with no latency claim"
shape 75kbit 160kbit 1%
probe floor --profile floor --streams 1 --bytes 1048576
joined floor
cp "$WORK/client/"*.json "$EVIDENCE/"
cp "$WORK/peer/"*.json "$EVIDENCE/"
tc_own -s -j qdisc show dev eth0 > "$EVIDENCE/upload-qdisc-final.json"
tc_own -s -j qdisc show dev ifb0 > "$EVIDENCE/download-qdisc-final.json"
docker exec "$LAB" python3 /tmp/release-socket-facts.py --gateway-port "$GATEWAY_PORT" > "$EVIDENCE/connector-socket-final.json"
check "the selected namespace carried actual bidirectional packet loss" python3 "$HERE/placement.py" "$EVIDENCE"
qualification_status=0
python3 "$HERE/verify.py" --release-guard "$EVIDENCE" --release-mode "${ORCH_SMOKE_RELEASE_PROOFS:-0}" || qualification_status=$?
WSS_GATE_QUALIFIED="$(python3 - "$EVIDENCE/wss-release-qualification.json" <<'PY'
import json, sys
print('true' if json.load(open(sys.argv[1])).get('wss_release_gate_qualified') is True else 'false')
PY
)"
if [ "$qualification_status" != 0 ]; then
  fail "release mode requires complete WSS qualification; missing observations are recorded in wss-release-qualification.json"
fi
say "Partial WSS evidence: $EVIDENCE (placement=$PLACEMENT; exact ACK timing may remain unknown; application queue bounds are unobserved)"
