#!/usr/bin/env bash
# Released API/gateway/connector, 100 MiB each way, a packet cut, an abandoned stream and new admission.
set -Eeuo pipefail
if [ -n "${ORCH_SMOKE_WSS_INDEX:-}" ]; then
  proof_repo="$(cd "$(dirname "${BASH_SOURCE[0]}")/../../.." && pwd -P)"
  exec python3 "$proof_repo/scripts/smoke/link-load/verify.py" --release-index "$ORCH_SMOKE_WSS_INDEX" \
    --expect-source "$(git -C "$proof_repo" rev-parse HEAD)" \
    --output "${ORCH_SMOKE_WSS_QUALIFICATION:-$(dirname "$ORCH_SMOKE_WSS_INDEX")/wss-release-qualification.json}"
fi
. "$(dirname "${BASH_SOURCE[0]}")/../lib.sh"
. "$SMOKE_REPO/scripts/smoke/link-load/world.sh"
HERE="$SMOKE_REPO/scripts/smoke/link-load"
SCENARIO=linkload; DEFAULT_PORT=41400
# Shape one path: client plus connector shaping would add two300-ms round trips.
PLACEMENT="${ORCH_LINK_SHAPE:-client}"
case "$PLACEMENT" in client|connector|backplane) ;; *) abort "ORCH_LINK_SHAPE must be client, connector or backplane" ;; esac
# Each run is one normal or QA case on one leg. Only the complete three-leg
# normal/QA index can qualify the full gate; local exit0 is bounded case evidence.
WSS_GATE_QUALIFIED=false
WSS_RESOURCE_PID=""
trap proof_done EXIT
trap 'exit 130' INT
trap 'exit 143' TERM
proof_world
proof_measurement_mode

docker cp "$HERE/peer.py" "$LAB:/tmp/release-peer.py"
docker exec -d "$LAB" python3 /tmp/release-peer.py --port 41325 --out /peer
wait_for 30 "the native TCP fixture is listening beside the released connector" test -f "$WORK/peer/ready"
mkdir -p "$WORK/client" "$WORK/client-fixture"
python3 "$HERE/qa.py" fault --work "$WORK" --evidence "$EVIDENCE" --run "$SMOKE_RUN" --generation 0
cp "$WORK/world.json" "$WORK/client-fixture/world.json"
cp "$WORK/access-token" "$WORK/client-fixture/access-token"
cp "$STACK/config/secrets/orch_ca_cert.pem" "$WORK/client-fixture/ca.pem"
docker run -d --name "$CLIENT" --label "org.orch.smoke.run=$SMOKE_RUN" \
  --network "${PROJECT}_control" --add-host "proof.orch.test:$BRIDGE" --user "$(id -u):$(id -g)" \
  -v "$HERE:/proof:ro" -v "$WORK/client-fixture:/fixture:ro" -v "$WORK/client:/result" \
  --entrypoint sleep "$ENV_IMAGE" infinity > /dev/null
CLIENT_STARTED=1
API_CONTAINER="$(compose ps -q api)"; GATEWAY_CONTAINER="$(compose ps -q gateway)"
[ -n "$API_CONTAINER" ] && [ -n "$GATEWAY_CONTAINER" ] || abort "the owned API/gateway containers must be running"
SHAPED="$CLIENT"
if [ "$PLACEMENT" = connector ]; then SHAPED="$LAB"; fi
if [ "$PLACEMENT" = backplane ]; then SHAPED="$GATEWAY_CONTAINER"; fi
docker cp "$HERE/socket_facts.py" "$LAB:/tmp/release-socket-facts.py"
docker exec "$LAB" python3 /tmp/release-socket-facts.py --gateway-port "$GATEWAY_PORT" > "$EVIDENCE/connector-socket-before.json"
python3 - "$PLACEMENT" "$SHAPED" "$LAB" "$CLIENT" "$GATEWAY_PORT" "$EVIDENCE/run-identity.json" "$WORK/world.json" > "$EVIDENCE/shaping-placement.json" <<'PYIDENTITY'
import json, sys
identity, world = (json.load(open(path)) for path in sys.argv[6:8])
if identity.get('machine') != world['machine']:
    raise SystemExit('running identity does not match the enrolled fixture machine')
if identity.get('mode') == 'normal':
    process = identity['programs']['connector']['process']
elif identity.get('mode') == 'qa':
    process = identity['processes']['connector']
else:
    raise SystemExit('no measured connector identity is ready')
if type(process) is not int or process <= 0:
    raise SystemExit('the actual enrolled connector process was not identified')
print(json.dumps({'placement': sys.argv[1], 'shaped_container': sys.argv[2],
    'connector': sys.argv[3], 'client': sys.argv[4], 'gateway_port': int(sys.argv[5]),
    'connector_process': process, 'machine': world['machine'], 'run': identity['run'],
    'scope': 'whole owned container namespace', 'delay_each_direction_ms': 150,
    'loss_each_direction_percent': 1, 'application_queue_bound_observed': False}, indent=2))
PYIDENTITY
docker inspect --format '{{json .NetworkSettings.Networks}}' "$SHAPED" > "$EVIDENCE/shaped-networks.json"
docker inspect --format '{{json .Id}}' "$SHAPED" > "$EVIDENCE/shaped-container-id.json"
NETEM_IMAGE="${PROJECT}netem"
NETEM_IMAGE_ID=""
existing_netem="$(docker image ls --quiet --no-trunc "$NETEM_IMAGE")" || abort "cannot inspect the owned netem tag before building"
[ -z "$existing_netem" ] || abort "the netem tag already exists; choose a fresh run identity"
NETEM_IMAGE_BUILD_STARTED=1
docker build --label "org.orch.smoke.run=$SMOKE_RUN" --tag "$NETEM_IMAGE" --file "$HERE/netem.Dockerfile" "$HERE" > "$EVIDENCE/netem-build.log" 2>&1
NETEM_IMAGE_ID="$(docker image inspect --format '{{.Id}}' "$NETEM_IMAGE")"
printf '%s\n' "$NETEM_IMAGE_ID" > "$EVIDENCE/netem-built-image-id.txt"
docker image inspect "$NETEM_IMAGE" > "$EVIDENCE/netem-image.json"
docker run --rm --label "org.orch.smoke.run=$SMOKE_RUN" --network "container:$SHAPED" \
  --entrypoint python3 "$NETEM_IMAGE" -c 'import json,os;print(json.dumps(os.readlink("/proc/self/ns/net")))' > "$EVIDENCE/shaped-netns.json"
tc_own() { docker run --rm --label "org.orch.smoke.run=$SMOKE_RUN" --cap-add NET_ADMIN --network "container:$SHAPED" "$NETEM_IMAGE" "$@"; }
ip_own() { docker run --rm --label "org.orch.smoke.run=$SMOKE_RUN" --cap-add NET_ADMIN --network "container:$SHAPED" --entrypoint ip "$NETEM_IMAGE" "$@"; }
# IFB lives in the selected owned namespace. Never change the host root qdisc.
ip_own link add ifb0 type ifb > "$EVIDENCE/ifb.log" 2>&1 || abort "the selected isolated namespace needs kernel IFB support (see ifb.log)"
ip_own link set ifb0 up
tc_own qdisc add dev eth0 handle ffff: ingress
if [ "$PLACEMENT" = backplane ]; then
  docker inspect --format '{{json .NetworkSettings.Networks}}' "$API_CONTAINER" > "$EVIDENCE/api-networks.json"
  BACKPLANE_IP="$(python3 - "$EVIDENCE/api-networks.json" "${PROJECT}_control" <<'PY'
import ipaddress,json,sys
print(ipaddress.IPv4Address(json.load(open(sys.argv[1]))[sys.argv[2]]['IPAddress']))
PY
)"
  # Unclassified traffic stays in unshaped band1; only actual API:8444 enters band3.
  tc_own qdisc add dev eth0 root handle 1: prio bands 3 priomap 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0
  tc_own filter add dev eth0 parent 1: protocol ip prio 1 flower dst_ip "$BACKPLANE_IP" ip_proto tcp dst_port 8444 flowid 1:3
  tc_own filter add dev eth0 parent ffff: protocol ip prio 1 flower src_ip "$BACKPLANE_IP" ip_proto tcp src_port 8444 action mirred egress redirect dev ifb0
  gateway_facts() {
    local user
    user="$(docker inspect --format '{{.Config.User}}' "$GATEWAY_CONTAINER")"
    [ -n "$user" ] || abort "the owned gateway has no explicit process identity"
    docker run --rm --label "org.orch.smoke.run=$SMOKE_RUN" --network "container:$GATEWAY_CONTAINER" \
      --pid "container:$GATEWAY_CONTAINER" --user "$user" \
      -v "$HERE/socket_facts.py:/proof/socket_facts.py:ro" --entrypoint python3 "$NETEM_IMAGE" \
      /proof/socket_facts.py --target-port 8444 --process-name orch-gateway
  }
  gateway_facts > "$EVIDENCE/backplane-socket-before.json"
  python3 - "$EVIDENCE/shaping-placement.json" "$BACKPLANE_IP" <<'PY'
import json,sys
p=sys.argv[1]; record=json.load(open(p));record.update(scope='exact owned gateway to API backplane tuple',backplane_ip=sys.argv[2],backplane_port=8444)
open(p,'w').write(json.dumps(record,indent=2)+'\n')
PY
else
  tc_own filter add dev eth0 parent ffff: protocol all matchall action mirred egress redirect dev ifb0
fi
shape() {
  if [ "$PLACEMENT" = backplane ]; then
    tc_own qdisc replace dev eth0 parent 1:3 handle 30: netem delay 150ms loss "$3" rate "$1" limit 1024
  else
    tc_own qdisc replace dev eth0 root netem delay 150ms loss "$3" rate "$1" limit 1024
  fi
  tc_own qdisc replace dev ifb0 root netem delay 150ms loss "$3" rate "$2" limit 1024
}
shape 750kbit 1600kbit 1%
tc_own -s -j qdisc show dev eth0 > "$EVIDENCE/upload-qdisc.json"
tc_own -s -j qdisc show dev ifb0 > "$EVIDENCE/download-qdisc.json"
tc_own qdisc show dev eth0 > "$EVIDENCE/upload-qdisc.txt"
tc_own qdisc show dev ifb0 > "$EVIDENCE/download-qdisc.txt"
if [ "$PLACEMENT" = backplane ]; then
  tc_own -s -j filter show dev eth0 parent 1: > "$EVIDENCE/backplane-egress-filter.json"
  tc_own -s -j filter show dev eth0 parent ffff: > "$EVIDENCE/backplane-ingress-filter.json"
fi
python3 "$HERE/placement.py" "$EVIDENCE" --preflight
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
  local workload=()
  mapfile -t workload < <(python3 "$HERE/workload.py" "$PLACEMENT" "$name")
  [ "${#workload[@]}" -gt 0 ] || abort "the reviewed workload allocation could not be selected"
  printf '%s\n' "$name" > "$WORK/link-phase"
  proof_qa_checkpoint "$name" start
  sample_resources --once
  docker exec "$CLIENT" python3 /proof/client.py --world /fixture/world.json --out "/result/$name.json" --mixed "${workload[@]}" "$@" \
    > "$EVIDENCE/$name.log" 2>&1 || code=$?
  # Retain incomplete budget/progress observations before successful teardown removes WORK.
  local saved
  for saved in "$WORK/client/$name.json" "$WORK/client/$name.mixed.json"; do
    if [ -f "$saved" ]; then cp "$saved" "$EVIDENCE/"; fi
  done
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
  proof_qa_checkpoint "$1" drained
  local observed=()
  if [ "${WSS_QA_STARTED:-0}" = 1 ]; then observed=(--qa-evidence "$EVIDENCE" --phase "$1"); fi
  check "$1: independent counts and hashes agree, no producer stuck" python3 "$HERE/verify.py" \
    --client "$WORK/client/$1.json" --peer "$WORK/peer" --require-mixed \
    --qualification "$EVIDENCE/$1-qualification.json" "${observed[@]}"
  printf '%s-drained\n' "$1" > "$WORK/link-phase"
  sample_resources --once
}

step "one allocated duplex transfer with concurrent PTY/control, a stalled reader and a real packet cut"
probe concurrent &
PROBE_PID=$!
wait_for 120 "the actual port socket is carrying bytes" has_started concurrent
# Only the primary long transfer repeats the deliberate blackout. Short other-leg
# placement probes retain the agreed random loss without making their sole loaded sample a fault.
if [ "$PLACEMENT" = connector ]; then
  python3 "$HERE/qa.py" fault --work "$WORK" --evidence "$EVIDENCE" --run "$SMOKE_RUN" --generation 1
  shape 750kbit 1600kbit 100%
  date -u +%FT%TZ > "$EVIDENCE/packet-cut-start.txt"
  sleep 3 # The scenario's intentional network outage, not a presumed product completion.
  shape 750kbit 1600kbit 1%
  python3 "$HERE/qa.py" fault --work "$WORK" --evidence "$EVIDENCE" --run "$SMOKE_RUN" --generation 2
  date -u +%FT%TZ > "$EVIDENCE/packet-cut-end.txt"
fi
if wait "$PROBE_PID"; then pass "concurrent transfers returned"; else fail "concurrent evidence incomplete; inspect its recorded failure_domain"; exit 1; fi
PROBE_PID=""
joined concurrent

step "an abandoned stream closes, then a fresh admission carries distinct bytes"
probe cut
joined cut
probe reconnect
joined reconnect
tc_own -s -j qdisc show dev eth0 > "$EVIDENCE/profile-upload-qdisc-final.json"
tc_own -s -j qdisc show dev ifb0 > "$EVIDENCE/profile-download-qdisc-final.json"
tc_own qdisc show dev eth0 > "$EVIDENCE/profile-upload-qdisc-final.txt"
tc_own qdisc show dev ifb0 > "$EVIDENCE/profile-download-qdisc-final.txt"
if [ "$PLACEMENT" = backplane ]; then
  gateway_facts > "$EVIDENCE/backplane-socket-profile-end.json"
  tc_own -s -j filter show dev eth0 parent 1: > "$EVIDENCE/backplane-egress-filter.json"
  tc_own -s -j filter show dev eth0 parent ffff: > "$EVIDENCE/backplane-ingress-filter.json"
fi
step "the floor preserves bytes too, with no latency claim"
shape 75kbit 160kbit 1%
probe floor
joined floor
proof_qa_checkpoint shutdown pre_stop
cp "$WORK/client/"*.json "$EVIDENCE/"
cp "$WORK/peer/"*.json "$EVIDENCE/"
tc_own -s -j qdisc show dev eth0 > "$EVIDENCE/upload-qdisc-final.json"
tc_own -s -j qdisc show dev ifb0 > "$EVIDENCE/download-qdisc-final.json"
tc_own qdisc show dev eth0 > "$EVIDENCE/floor-upload-qdisc-final.txt"
tc_own qdisc show dev ifb0 > "$EVIDENCE/floor-download-qdisc-final.txt"
docker exec "$LAB" python3 /tmp/release-socket-facts.py --gateway-port "$GATEWAY_PORT" > "$EVIDENCE/connector-socket-final.json"
if [ "$PLACEMENT" = backplane ]; then gateway_facts > "$EVIDENCE/backplane-socket-final.json"; fi
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
