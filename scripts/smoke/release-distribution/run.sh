#!/usr/bin/env bash
# Released updater fetches a site and pulls an absent image; an unavailable image is reported honestly.
set -Eeuo pipefail
. "$(dirname "${BASH_SOURCE[0]}")/../lib.sh"

A="$(release_a)"; B="$(release_b)"
TAG_A="$(release_tag "$A")"; TAG_B="$(release_tag "$B")"
new_stack distribution 41240 "$A"
EVIDENCE="$SMOKE_BASE/evidence/$SMOKE_RUN/distribution"
mkdir -p "$EVIDENCE"
REGISTRY="${PROJECT}registry"
# Official registry 3.0.0, by its multi-platform index: only this scenario's tagged images are removed.
REGISTRY_IMAGE="registry:3.0.0@sha256:6c5666b861f3505b116bb9aa9b25175e71210414bd010d92035ff64018f9457e"
REGISTRY_PORT=$((EDGE_PORT + 3))
PULL_REF="127.0.0.1:$REGISTRY_PORT/orch-env-base:$SMOKE_RUN"
MISSING_REF="127.0.0.1:$REGISTRY_PORT/orch-env-base:missing$SMOKE_RUN"
registry_started=0
distribution_done() {
  local previous=$?
  if [ "$previous" != 0 ]; then FAILED=$((FAILED + 1)); fi
  if [ "$registry_started" = 1 ]; then
    docker logs "$REGISTRY" > "$EVIDENCE/registry.log" 2>&1 || true
    if [ "${ORCH_SMOKE_KEEP:-0}" != 1 ]; then docker rm -f "$REGISTRY" > /dev/null; fi
  fi
  printf '{"checks":%s,"failures":%s,"non_linux_install_update":"unperformed","published_site":"unperformed"}\n' "$CHECKED" "$FAILED" > "$EVIDENCE/result.json"
  finish
}
trap distribution_done EXIT

SOURCE_IMAGE="$(python3 -c 'import json,sys; print(next(image["reference"] for image in json.load(open(sys.argv[1]))["images"] if image["name"]=="orch-env-base"))' "$B/version.json")"
docker image inspect "$SOURCE_IMAGE" > /dev/null || abort "the release's actual default environment image is needed"
docker run -d --name "$REGISTRY" --label "org.orch.smoke.run=$SMOKE_RUN" -p "127.0.0.1:$REGISTRY_PORT:5000" "$REGISTRY_IMAGE" > /dev/null
registry_started=1
wait_for 60 "the local registry answers" curl -fsS "http://127.0.0.1:$REGISTRY_PORT/v2/"
docker tag "$SOURCE_IMAGE" "$PULL_REF"
docker push "$PULL_REF" > "$EVIDENCE/push.log" 2>&1
DIGEST="$(docker buildx imagetools inspect "$PULL_REF" --format '{{json .Manifest}}' | python3 -c 'import json,sys; print(json.load(sys.stdin)["digest"])')"
# This tag is this run's alone; the source release image and its tags are kept.
docker image rm "$PULL_REF" > /dev/null
GOOD="$SMOKE_DIR/distribution/good"
BAD="$SMOKE_DIR/distribution/unavailable"
python3 "$SMOKE_REPO/scripts/smoke/release-distribution/prepare.py" --source "$B" --out "$GOOD" --reference "$PULL_REF" --digest "$DIGEST"
python3 "$SMOKE_REPO/scripts/smoke/release-distribution/prepare.py" --source "$B" --out "$BAD" --reference "$MISSING_REF" --digest "$DIGEST"
cp "$GOOD/version.json" "$EVIDENCE/offered.json"
git -C "$SMOKE_REPO" rev-parse HEAD > "$EVIDENCE/commit.txt"
printf '%s\n' "$REGISTRY_IMAGE" > "$EVIDENCE/registry-image.txt"

step "install the first release from its supplied archive"
update_sh first-start --from "$A" > "$EVIDENCE/first-start.log" 2>&1 || fail "the supplied archive is installed"
setup_admin || fail "the administrator is made"
wait_for 240 "the bundled node is online" node_online

step "fetch the next release from a served release site and pull its missing image"
refuses "the registry tag is absent from the consumer's image list" docker image inspect "$PULL_REF"
serve_site distribution "$((EDGE_PORT + 2))" "$A" "$GOOD"
if ORCH_RELEASE_URL="http://127.0.0.1:$((EDGE_PORT + 2))" update_sh latest --yes > "$EVIDENCE/site-update.log" 2>&1; then pass "the fetched release is installed"; else fail "the fetched release is installed"; fi
expect "the reported version is the fetched release" "${TAG_B#v}" "$(edge_version)"
check "the updater fetched the manifest" grep -F "fetching http://127.0.0.1:$((EDGE_PORT + 2))/latest/download/version.json" "$EVIDENCE/site-update.log"
check "the updater pulled the missing registry tag" grep -F "pulling $PULL_REF" "$EVIDENCE/site-update.log"
check "the pulled image exists" docker image inspect "$PULL_REF"
check "the pulled image names the registry digest" says "$DIGEST" docker image inspect --format '{{json .RepoDigests}}' "$PULL_REF"
check "every service reports the fetched release" ready "$TAG_B"

step "an unavailable image is reported while the verified application update stays usable"
update_sh rollback --yes > "$EVIDENCE/rollback.log" 2>&1 || fail "rollback to the first release"
stop_site
serve_site distribution "$((EDGE_PORT + 2))" "$A" "$BAD"
if ORCH_RELEASE_URL="http://127.0.0.1:$((EDGE_PORT + 2))" update_sh latest --yes > "$EVIDENCE/unavailable-update.log" 2>&1; then pass "the application updates while its optional image is unavailable"; else fail "the application update remains usable"; fi
check "the actual missing image is named in the refusal" grep -F "could not pull $MISSING_REF" "$EVIDENCE/unavailable-update.log"
refuses "an unavailable image is never reported as installed" docker image inspect "$MISSING_REF"
check "the application is ready with the fetched release" ready "$TAG_B"
say "UNPERFORMED: published release site and macOS/Windows install/update/run proof; native workflow runners must supply it."
say "Evidence: $EVIDENCE"
