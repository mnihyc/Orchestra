#!/usr/bin/env bash
# Real release commands publish backup metadata while the API is running and stopped; checks never install.
set -Eeuo pipefail
. "$(dirname "${BASH_SOURCE[0]}")/../lib.sh"
A="$(release_a)"
new_stack installation-records 41430 "$A"
WORK="$(dirname "$STACK")"
EVIDENCE="$SMOKE_BASE/evidence/$SMOKE_RUN/installation-records"
mkdir -p "$EVIDENCE" "$WORK/site/latest/download"
# What the configured source offers is a newer release's complete manifest, and only that: nothing is installed from it,
# and no scenario step reads a file it lists. A later release that is given (ORCH_SMOKE_RELEASE_B) is used as it is;
# otherwise the candidate's own manifest is made out as the next patch release, with its tag and the names that carry it
# changed, the way a source would offer a release that is not there yet.
if [ -n "${ORCH_SMOKE_RELEASE_B:-}" ]; then
  B="$ORCH_SMOKE_RELEASE_B"
else
  B="$WORK/later-release"
  mkdir -p "$B"
  python3 - "$A/version.json" "$B/version.json" <<'PY'
import json
import re
import sys

source, target = sys.argv[1:]
manifest = json.load(open(source))
found = re.fullmatch(r"v(\d+)\.(\d+)\.(\d+)(?:-[0-9A-Za-z.-]+)?", manifest["tag"])
if not found:
    sys.exit("the candidate's tag is not vMAJOR.MINOR.PATCH")
earlier, later = manifest["tag"], "v%s.%s.%d" % (found[1], found[2], int(found[3]) + 1)
manifest["tag"], manifest["version"] = later, later[1:]
for asset in manifest["assets"]:
    for key in ("name", "download_url"):
        asset[key] = asset[key].replace(earlier, later)
with open(target, "w") as handle:
    json.dump(manifest, handle, indent=2)
PY
fi
SOURCE_PID=""
leave_records() {
  local previous=$?
  if [ "$previous" != 0 ]; then FAILED=$((FAILED + 1)); fi
  if [ -n "$SOURCE_PID" ]; then kill "$SOURCE_PID" 2> /dev/null || true; fi
  compose logs --no-color > "$EVIDENCE/stack.log" 2>&1 || true
  printf '{"checks":%s,"failures":%s,"process_exit":%s}\n' "$CHECKED" "$FAILED" "$previous" > "$EVIDENCE/result.json"
  finish
}
trap leave_records EXIT
SOURCE_PORT=$((EDGE_PORT + 2))
SOURCE="http://$(bridge_address):$SOURCE_PORT"
cp "$B/version.json" "$WORK/site/latest/download/version.json"
python3 -m http.server "$SOURCE_PORT" --bind "$(bridge_address)" --directory "$WORK/site" > "$EVIDENCE/source.log" 2>&1 &
SOURCE_PID=$!
printf '\nORCH_RELEASE_URL=%s\n' "$SOURCE" >> "$STACK/.env"
update_sh first-start --from "$A" > "$EVIDENCE/first-start.log" 2>&1
setup_admin || abort "the fixture administrator could not be made"
api_record() {
  curl -fsS --max-time 45 -X "$1" -H "authorization: Bearer $TOKEN" "$EDGE/api/v1$2"
}

step "checks ship off; a manual lookup reports a newer release but installs nothing"
# Management's page itself looks the source up when it is opened (and keeps the answer for an hour, as a manual
# check does), so what the scheduled check made of the shipped setting is looked at before the page is first read.
check "no scheduled release facts exist with checks disabled" test "$(sql "SELECT count(*) FROM installation_records WHERE key = 'release_check'")" = 0
refuses "the off check made no manifest request" grep -F 'GET /latest/download/version.json' "$EVIDENCE/source.log"
BEFORE="$(edge_version)"
api_record GET /admin/installation-records > "$EVIDENCE/initial.json"
api_record POST /admin/releases/check > "$EVIDENCE/release.json"
check "the configured complete manifest reports the newer offer" jq -e '.newer == true and .command != null and .error == null' "$EVIDENCE/release.json"
expect "a check installs nothing" "$BEFORE" "$(edge_version)"
api_record POST /admin/releases/check > "$EVIDENCE/release-again.json"
expect "the page and both manual checks shared one lookup of the source" 1 "$(grep -cF 'GET /latest/download/version.json' "$EVIDENCE/source.log" || true)"
NOTICES="$(compose exec -T postgres psql -U orch -d orch -At -c "SELECT count(*) FROM notifications WHERE data->>'reason'='release_available'")"
expect "one notice for that offer" 1 "$NOTICES"
printf 'not a manifest' > "$WORK/site/latest/download/version.json"
# A lookup is not repeated within the hour after the last attempt, whoever asks. The hour is let pass in the
# record of the attempt (the facts it kept keep their own date), so that the next check does ask the source.
sql "UPDATE installation_records SET value = jsonb_set(value, '{attempted_at}', to_jsonb(to_char(((value->>'attempted_at')::timestamptz - interval '2 hours') AT TIME ZONE 'UTC', 'YYYY-MM-DD\"T\"HH24:MI:SS.MS\"Z\"'))) WHERE key = 'release_check'" > /dev/null
api_record POST /admin/releases/check > "$EVIDENCE/release-failure.json"
check "a bad source keeps its dated successful facts" python3 - "$EVIDENCE/release.json" "$EVIDENCE/release-failure.json" <<'PY'
import json,sys
good,bad = [json.load(open(path)) for path in sys.argv[1:]]
assert bad['error'] and good['checked_at'] == bad['checked_at'] and good['offered_version'] == bad['offered_version']
PY

step "the existing backup command records its completed pair while the API runs"
update_sh backup > "$EVIDENCE/backup-running.log" 2>&1
api_record GET /admin/installation-records > "$EVIDENCE/backup-running.json"
check "the Overview reports the actual database-only pair" jq -e '.backup.database_only and .backup.bytes > 0 and .backup.cause == "manual" and .backup_error == null' "$EVIDENCE/backup-running.json"

step "the same command publishes while the API is stopped, then startup imports the file"
compose stop api > /dev/null
update_sh backup > "$EVIDENCE/backup-stopped.log" 2>&1
compose start api > /dev/null
wait_for 120 "the API is ready after startup reconciliation" curl -fsS "$EDGE/health"
sign_in
api_record GET /admin/installation-records > "$EVIDENCE/backup-stopped.json"
NAME="$(jq -r '.backup.dump_name' "$EVIDENCE/backup-stopped.json")"
expect "startup imported the command's actual most recent dump" "$NAME" \
  "$(python3 -c 'import json,sys; print(json.load(open(sys.argv[1]))["success"]["record"]["dump_name"])' "$STACK/runtime/update/latest-database-backup.json")"
cp "$STACK/runtime/update/latest-database-backup.json" "$WORK/good-record.json"
python3 - "$STACK/runtime/update/latest-database-backup.json" <<'PY'
import json,pathlib,sys
p=pathlib.Path(sys.argv[1]); row=json.loads(p.read_text()); row['success']['system_id']='another-installation'; p.write_text(json.dumps(row))
PY
api_record GET /admin/installation-records > "$EVIDENCE/wrong-installation.json"
check "a wrong installation cannot advance the successful fact" jq -e '.backup_error != null and .backup != null' "$EVIDENCE/wrong-installation.json"
cp "$WORK/good-record.json" "$STACK/runtime/update/latest-database-backup.json"

step "metadata repair uses the completed pair and never takes another dump"
COUNT="$(find "$STACK/runtime/update-backups" -maxdepth 1 -name '*.dump' | wc -l)"
HASH="$(sha256sum "$STACK/runtime/update-backups/$NAME" | cut -d' ' -f1)"
update_sh backup --repair-record "$NAME" > "$EVIDENCE/repair.log" 2>&1
expect "repair did not make another dump" "$COUNT" "$(find "$STACK/runtime/update-backups" -maxdepth 1 -name '*.dump' | wc -l)"
expect "repair did not change the completed dump" "$HASH" "$(sha256sum "$STACK/runtime/update-backups/$NAME" | cut -d' ' -f1)"
check "repair names its metadata-only result" grep -F 'No new dump was taken' "$EVIDENCE/repair.log"
api_record GET /admin/installation-records > "$EVIDENCE/repaired.json"
check "repair retained the actual completion time" python3 - "$EVIDENCE/backup-stopped.json" "$EVIDENCE/repaired.json" <<'PY'
import json,sys
before,after=[json.load(open(p))['backup'] for p in sys.argv[1:]]
assert before['completed_at']==after['completed_at'] and before['hash']==after['hash']
PY
cp "$A/version.json" "$EVIDENCE/candidate.json"
say "Evidence: $EVIDENCE; held-clock age, source bounds and failure-preservation cases run in installation_records.rs."
