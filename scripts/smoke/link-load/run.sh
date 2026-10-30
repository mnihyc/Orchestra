#!/usr/bin/env bash
# Released API/gateway/connector, 100 MiB each way, a packet cut, an abandoned stream and new admission.
set -Eeuo pipefail
. "$(dirname "${BASH_SOURCE[0]}")/../lib.sh"
. "$SMOKE_REPO/scripts/smoke/link-load/world.sh"
HERE="$SMOKE_REPO/scripts/smoke/link-load"
SCENARIO=linkload; DEFAULT_PORT=41400
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
NETEM_IMAGE="${PROJECT}netem"
docker build --tag "$NETEM_IMAGE" --file "$HERE/netem.Dockerfile" "$HERE" > "$EVIDENCE/netem-build.log" 2>&1
docker image inspect "$NETEM_IMAGE" > "$EVIDENCE/netem-image.json"
tc_own() { docker run --rm --cap-add NET_ADMIN --network "container:$CLIENT" "$NETEM_IMAGE" "$@"; }
ip_own() { docker run --rm --cap-add NET_ADMIN --network "container:$CLIENT" --entrypoint ip "$NETEM_IMAGE" "$@"; }
# IFB lives in the client namespace. If the host kernel lacks it, fail with that prerequisite named.
ip_own link add ifb0 type ifb > "$EVIDENCE/ifb.log" 2>&1 || abort "the isolated client needs kernel IFB support (see ifb.log)"
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

probe() {
  local name="$1" code=0; shift
  docker exec "$CLIENT" python3 /proof/client.py --world /fixture/world.json --out "/result/$name.json" "$@" \
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
    --client "$WORK/client/$1.json" --peer "$WORK/peer"
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
step "the floor preserves bytes too, with no latency claim"
shape 75kbit 160kbit 1%
probe floor --profile floor --streams 1 --bytes 1048576
joined floor
cp "$WORK/client/"*.json "$EVIDENCE/"
cp "$WORK/peer/"*.json "$EVIDENCE/"
tc_own -s -j qdisc show dev eth0 > "$EVIDENCE/upload-qdisc-final.json"
tc_own -s -j qdisc show dev ifb0 > "$EVIDENCE/download-qdisc-final.json"
say "Evidence: $EVIDENCE (the profile and floor are measured separately; no floor speed promise)"
