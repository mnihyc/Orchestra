#!/usr/bin/env bash
# update-files: the files of the orch-deploy bundle follow the release. With no stack: update.sh is read into a shell in a
# directory of its own, against a stand-in for docker, and the functions that decide, write, record and put back the files
# are called with bundles made here, so that what is decided, written, kept and left behind can be looked at. What a real
# transaction does with them (stop, dump, switch, start, verify, roll back, recover) is in the update, rollback and recover
# scenarios.
set -Eeuo pipefail
# shellcheck source-path=SCRIPTDIR/..
# shellcheck source=lib.sh
. "$(dirname "${BASH_SOURCE[0]}")/../lib.sh"

WORK="$(mktemp -d "${TMPDIR:-/tmp}/orch-update-files.XXXXXX")"
STACK="$WORK/stack"
BUNDLES="$WORK/bundles"
done_with() { rm -rf -- "$WORK"; rmdir -- "$SMOKE_DIR" 2> /dev/null || true; printf '\n%s: %d checks, %d failed\n' "$(basename "$0")" "$CHECKED" "$FAILED"; [ "$FAILED" -eq 0 ]; }
trap 'done_with' EXIT

mkdir -p "$WORK/bin" "$STACK/config" "$STACK/runtime/postgres/data" "$STACK/runtime/update" "$BUNDLES"
# The images this host has, and those a pull can fetch: one reference a line.
: > "$WORK/images.have"; : > "$WORK/images.pullable"
cat > "$WORK/bin/docker" << 'STANDIN'
#!/usr/bin/env bash
echo "docker $*" >> "${FAKE_LOG:-/dev/null}"
case "$1 $2" in
  "image inspect") grep -Fx -- "$3" "$FAKE_IMAGES" > /dev/null ;;
  "pull --quiet")
    if grep -Fx -- "$3" "$FAKE_PULLABLE" > /dev/null; then echo "$3" >> "$FAKE_IMAGES"; exit 0; fi
    exit 1 ;;
  *) exit 0 ;;
esac
STANDIN
chmod +x "$WORK/bin/docker"
cp "$SMOKE_REPO/deploy/update.sh" "$STACK/update.sh"
cp "$STACK/update.sh" "$WORK/update.sh.installed"
printf 'POSTGRES_PASSWORD=x\n' > "$STACK/.env"; chmod 600 "$STACK/.env"
printf '[system]\npublic_url = "https://orch.example.org"\n' > "$STACK/config/orch.toml"
printf '17\n' > "$STACK/runtime/postgres/data/PG_VERSION"

# The shell that update.sh is read into: it knows the three things a plan needs of a release (its tag, what its manifest says of
# the files, its bundle), and a way to open a transaction in the journal.
cat > "$WORK/prelude.sh" << 'PRELUDE'
WORK="$(mktemp -d)"; trap 'rm -rf -- "$WORK"' EXIT
plan() { TAG=$1; MANIFEST_FLAT="$WORK/manifest.flat"; printf 'deploy_files.minimum\t%s\n' "$2" > "$MANIFEST_FLAT"; BUNDLE=${3:-}; plan_files; }
begin() { J=(); J[id]="$1"; J[to_tag]="$TAG"; J[from_tag]=v0.1.3; files_begin "$FILES_STAGE/deploy"; jset step staged; }
begin_back() { J=(); J[id]="$1"; J[to_tag]=v0.1.3; J[from_tag]="$TAG"; files_begin "${FILES_SOURCE:-/nonexistent}"; jset step staged; }
PRELUDE

# in_updater CODE: update.sh read into a shell in the installation, with the stand-in docker, and CODE run in it.
in_updater() {
  # shellcheck disable=SC2016 # the shell it starts expands them
  (cd "$STACK" && env PATH="$WORK/bin:$PATH" FAKE_LOG="$WORK/docker.log" FAKE_IMAGES="$WORK/images.have" FAKE_PULLABLE="$WORK/images.pullable" \
    bash -c 'set -Eeuo pipefail; source ./update.sh; source "$1"; eval "$2"' _ "$WORK/prelude.sh" "$1" 2>&1)
}
has() { grep -F -- "$1" <<< "$OUT" > /dev/null; }
lacks() { ! grep -F -- "$1" <<< "$OUT" > /dev/null; }
same() { cmp -s "$1" "$2"; }

# --- Releases made here ----------------------------------------------------------------------------------------------------

# files_of DIR ROUTES POSTGRES_MAJOR [DATA_SOURCE [NGINX_IMAGE]]: the files of one version of the deployment set, small ones.
files_of() {
  local dir=$1 routes=$2 major=$3 data=${4:-./runtime/data} nginx_image=${5:-nginx:1.30@sha256:n1}
  mkdir -p "$dir/postgres" "$dir/hosts/runtimes" "$dir/install"
  printf 'name: orch\nservices:\n  api:\n    image: debian:bookworm@sha256:d1\n    volumes:\n      - %s:/var/lib/orch\n      - ./runtime/postgres/data:/data\n      - ./config/orch.toml:/etc/orch/orch.toml:ro\n  frontend:\n    image: %s\n' "$data" "$nginx_image" > "$dir/compose.yml"
  printf 'events {}\n# routes: %s\n' "$routes" > "$dir/nginx.conf"
  printf '#!/bin/sh\nexec nginx\n' > "$dir/frontend.sh"
  printf 'services:\n  frontend:\n    ports: ["443:443"]\n' > "$dir/compose.frontend-https.yml"
  # shellcheck disable=SC2016 # the Dockerfile's own variable
  printf 'ARG POSTGRES_IMAGE=postgres:%s-alpine@sha256:p1\nFROM %s\n' "$major" '${POSTGRES_IMAGE}' > "$dir/postgres/Dockerfile"
  printf '# common\n' > "$dir/hosts/runtimes/common.sh"
  printf '# gvisor\n' > "$dir/hosts/runtimes/gvisor.sh"
  printf '# kata\n' > "$dir/hosts/runtimes/kata.sh"
  printf 'readme for %s\n' "$routes" > "$dir/README.md"
  printf '# installer\n' > "$dir/install/install.sh"
  printf 'ORCH_RELEASE_URL=\n' > "$dir/.env.example"
}

MAIN="compose.yml compose.frontend-https.yml nginx.conf frontend.sh postgres/Dockerfile hosts/runtimes/common.sh hosts/runtimes/gvisor.sh hosts/runtimes/kata.sh"

# rows TAG GENERATION DIR: the FILE_SETS lines of one release's files.
rows() {
  local tag=$1 generation=$2 dir=$3 path
  while IFS= read -r path; do printf 'file %s %s %s %s\n' "$tag" "$generation" "$(sha256sum "$dir/$path" | cut -d' ' -f1)" "$path"; done \
    < <(cd "$dir" && find . -type f ! -path ./FILE_SETS | sed 's|^\./||' | sort)
}

# bundle NAME TAG GENERATION MINIMUM DIR PUBLISHED: BUNDLES/NAME/orch-deploy-<tag>.tar.gz of the files in DIR, with a FILE_SETS
# that carries what the published releases of the file PUBLISHED shipped, and this one's own.
bundle() {
  local name=$1 tag=$2 generation=$3 minimum=$4 dir=$5 published=$6 out="$BUNDLES/$1" stage
  stage="$(mktemp -d "$WORK/stage.XXXXXX")"
  mkdir -p "$stage/deploy" "$out"
  cp -a "$dir/." "$stage/deploy/"
  { printf 'format 1\ngeneration %s\nminimum %s\nset main %s\n' "$generation" "$minimum" "$MAIN"; cat "$published"; rows "$tag" "$generation" "$dir"; } > "$stage/deploy/FILE_SETS"
  tar -czf "$out/orch-deploy-$tag.tar.gz" -C "$stage" deploy
  rm -rf -- "$stage"
}

# The files of v0.1.3 (generation 1), as installed. The updater in use is a published one too: its SHA-256 is a row.
files_of "$WORK/v0.1.3" 'api' 17
rows v0.1.3 1 "$WORK/v0.1.3" > "$WORK/published.v0.1.3"
printf 'file v0.1.3 1 %s update.sh\n' "$(sha256sum "$STACK/update.sh" | cut -d' ' -f1)" >> "$WORK/published.v0.1.3"
{ printf 'format 1\ngeneration 1\nminimum 1\nset main %s\n' "$MAIN"; cat "$WORK/published.v0.1.3"; } > "$WORK/v0.1.3/FILE_SETS"
printf 'file v0.1.3 1 %s FILE_SETS\n' "$(sha256sum "$WORK/v0.1.3/FILE_SETS" | cut -d' ' -f1)" >> "$WORK/published.v0.1.3"
# The next release's updater is this one with a line more.
NEW_UPDATER="$WORK/new-update.sh"
{ cat "$STACK/update.sh"; printf '# the updater of the next release\n'; } > "$NEW_UPDATER"; chmod 755 "$NEW_UPDATER"
# v0.1.4 adds a route to nginx.conf and a file under install/.
files_of "$WORK/v0.1.4" 'api openapi' 17; cp "$NEW_UPDATER" "$WORK/v0.1.4/update.sh"
printf '# a new installer\n' > "$WORK/v0.1.4/install/install-node.sh"
bundle gen2 v0.1.4 2 1 "$WORK/v0.1.4" "$WORK/published.v0.1.3"
bundle gen2min2 v0.1.4 2 2 "$WORK/v0.1.4" "$WORK/published.v0.1.3"
G2="$BUNDLES/gen2/orch-deploy-v0.1.4.tar.gz"; G2MIN2="$BUNDLES/gen2min2/orch-deploy-v0.1.4.tar.gz"
tar -xOzf "$G2" deploy/FILE_SETS > "$WORK/v0.1.4/FILE_SETS"
{ cat "$WORK/published.v0.1.3"; rows v0.1.4 2 "$WORK/v0.1.4"; } > "$WORK/published.v0.1.4"
printf 'file v0.1.4 2 %s FILE_SETS\n' "$(sha256sum "$WORK/v0.1.4/FILE_SETS" | cut -d' ' -f1)" >> "$WORK/published.v0.1.4"
# v0.1.5 routes one more, and asks for generation 3.
files_of "$WORK/v0.1.5" 'api openapi ws' 17; cp "$NEW_UPDATER" "$WORK/v0.1.5/update.sh"
bundle gen3 v0.1.5 3 3 "$WORK/v0.1.5" "$WORK/published.v0.1.4"
G3="$BUNDLES/gen3/orch-deploy-v0.1.5.tar.gz"

# install_set DIR: the installation holds the files of DIR (and the updater it has been given), and no memory of an update.
install_set() {
  rm -rf "$STACK/runtime/update/files" "$STACK/runtime/update/files.state" "$STACK/runtime/update/journal"
  find "${STACK:?}" -mindepth 1 -maxdepth 1 ! -name runtime ! -name config ! -name .env ! -name update.sh -exec rm -rf -- {} +
  cp -a "$1/." "$STACK/"
  cp "$WORK/update.sh.installed" "$STACK/update.sh"
}
printf 'nginx:1.30@sha256:n1\ndebian:bookworm@sha256:d1\n' > "$WORK/images.have"
install_set "$WORK/v0.1.3"

# --- 1. A set of published files is replaced whole --------------------------------------------------------------------------

step "files that equal a published version are replaced, in one set, and nothing else is touched"
printf 'POSTGRES_PASSWORD=untouched\n' > "$STACK/.env"; chmod 600 "$STACK/.env"
ENV_BEFORE="$(sha256sum "$STACK/.env" "$STACK/config/orch.toml" | sha256sum)"
OUT="$(in_updater "plan v0.1.4 1 '$G2'; echo \"replace: \${FILES_REPLACE[*]} gen: \$FILES_GEN_AFTER update.sh: \$UPDATE_SH\"")" && CODE=0 || CODE=$?
expect "the plan is made: exit 0" 0 "$CODE"
check "it says which files it will replace and from which release" has "files: replacing nginx.conf (release v0.1.4's, generation 2)"
check "the files that did not change are not listed; the others follow one by one" has 'replace: nginx.conf FILE_SETS README.md install/install-node.sh gen: 2 update.sh: replace'
check "the updater is to be replaced when the update has finished" has 'files: update.sh is replaced when the update has finished'
check "the plan wrote nothing" same "$STACK/nginx.conf" "$WORK/v0.1.3/nginx.conf"
OUT="$(in_updater "plan v0.1.4 1 '$G2'; begin one; apply_files; echo applied")" || true
check "the files were applied" has 'applied'
check "nginx.conf is the release's" same "$STACK/nginx.conf" "$WORK/v0.1.4/nginx.conf"
check "the installed file table is the one in the release bundle" same "$STACK/FILE_SETS" "$WORK/v0.1.4/FILE_SETS"
check "README.md is the release's" same "$STACK/README.md" "$WORK/v0.1.4/README.md"
check "a file the release adds is there" same "$STACK/install/install-node.sh" "$WORK/v0.1.4/install/install-node.sh"
check "compose.yml, which did not change, was not written" same "$STACK/compose.yml" "$WORK/v0.1.3/compose.yml"
check "the earlier nginx.conf was saved where a rollback finds it" same "$STACK/runtime/update/files/one/old/nginx.conf" "$WORK/v0.1.3/nginx.conf"
check "the journal lists the files before and after" grep -q '^files=nginx.conf FILE_SETS README.md install/install-node.sh$' "$STACK/runtime/update/journal"
check "the updater is not replaced before the end" same "$STACK/update.sh" "$WORK/update.sh.installed"
expect "the operator's files are untouched" "$ENV_BEFORE" "$(sha256sum "$STACK/.env" "$STACK/config/orch.toml" | sha256sum)"
OUT="$(in_updater 'journal_load; files_finish; echo finished')" || true
check "the updater was replaced at the end" same "$STACK/update.sh" "$NEW_UPDATER"
check "it says so" has 'update.sh is now the one of release v0.1.4'
check "the generation and the saved files are recorded" bash -c "grep -q '^files_generation=2$' '$STACK/runtime/update/files.state' && grep -q '^files_previous=one$' '$STACK/runtime/update/files.state'"
check "the permissions of the files are the bundle's" test "$(stat -c %a "$STACK/update.sh")" = 755

# --- 2. An edit keeps the whole set -----------------------------------------------------------------------------------------

step "an edit keeps the whole set, and the shipped files are written beside, once"
install_set "$WORK/v0.1.3"
printf 'events {}\n# routes: api\n# my own timeout\n' > "$STACK/nginx.conf"
EDITED="$(sha256sum < "$STACK/nginx.conf")"
OUT="$(in_updater "plan v0.1.4 1 '$G2'; echo \"replace: \${FILES_REPLACE[*]:-nothing} gen: \$FILES_GEN_AFTER\"")" && CODE=0 || CODE=$?
expect "the plan is made: exit 0" 0 "$CODE"
check "one line says what was kept and where the shipped file is" has 'files: kept nginx.conf (edited here: nginx.conf); the shipped one is nginx.conf.new'
check "the set is not replaced; the other files still follow" has 'replace: FILE_SETS README.md install/install-node.sh gen: 1'
expect "the edited file is as it was" "$EDITED" "$(sha256sum < "$STACK/nginx.conf")"
check "the shipped nginx.conf is nginx.conf.new" same "$STACK/nginx.conf.new" "$WORK/v0.1.4/nginx.conf"
OUT="$(in_updater 'files_status')"
check "status lists it" has 'nginx.conf differs from the shipped file, which is nginx.conf.new'
rm -f "$STACK/nginx.conf.new"
OUT="$(in_updater "plan v0.1.4 1 '$G2'")" || true
check "once its .new is removed it is not written again for the same shipped file" test ! -e "$STACK/nginx.conf.new"
OUT="$(in_updater 'files_status')"
check "and status says nothing of it" lacks 'nginx.conf.new'
OUT="$(in_updater "plan v0.1.5 3 '$G3'")" && CODE=0 || CODE=$?
check "a later shipped file is written again" test -e "$STACK/nginx.conf.new"
check "it is the later one" same "$STACK/nginx.conf.new" "$WORK/v0.1.5/nginx.conf"
check "a release that needs generation 3 stops, as the set kept is older" test "$CODE" != 0

# --- 3. The oldest generation the payloads work with ------------------------------------------------------------------------

step "a kept set older than the payloads need stops the update before anything is stopped, and names the files to merge"
rm -f "$STACK"/*.new "$STACK/runtime/update/files.state"
OUT="$(in_updater "plan v0.1.4 2 '$G2MIN2'")" && CODE=0 || CODE=$?
check "it stops" test "$CODE" != 0
check "it says what the release needs and what is here" has 'release v0.1.4 works with deployment files of generation 2 or newer, and these are generation 1'
check "it names the files to merge" has 'merge nginx.conf.new and run the update again'
check "it says nothing was stopped" has 'Nothing was stopped'
check "the shipped file is there to merge" test -e "$STACK/nginx.conf.new"
rm -f "$STACK/nginx.conf.new"
OUT="$(in_updater "plan v0.1.4 2 '$G2MIN2'; echo \"gen: \$FILES_GEN_AFTER\"")" && CODE=0 || CODE=$?
expect "merged by its owner (the .new removed), the kept set is the generation it was merged against, and the update goes on" 0 "$CODE"
check "with that generation" has 'gen: 2'
check "and the files are as they were" same "$STACK/README.md" "$WORK/v0.1.3/README.md"
OUT="$(in_updater "plan v0.1.4 1 '$G2'")" && CODE=0 || CODE=$?
expect "a release that asks only for generation 1 goes on with the kept set" 0 "$CODE"

step "an old set that is not kept is replaced, so it is not too old"
install_set "$WORK/v0.1.3"
OUT="$(in_updater "plan v0.1.4 2 '$G2MIN2'; echo \"gen: \$FILES_GEN_AFTER\"")" && CODE=0 || CODE=$?
expect "the plan is made" 0 "$CODE"
check "the set will be the release's generation" has 'gen: 2'

step "a release without its bundle follows no file, and the check still runs from version.json"
rm -f "$STACK/runtime/update/files.state"
OUT="$(in_updater "plan v0.1.4 1 ''")" && CODE=0 || CODE=$?
expect "it goes on" 0 "$CODE"
check "one line says so" has 'files: release v0.1.4 comes without its orch-deploy bundle; the deployment files are left as they are'
OUT="$(in_updater "plan v0.1.4 2 ''")" && CODE=0 || CODE=$?
check "but a release that needs generation 2 stops" test "$CODE" != 0
check "and says where the bundle is to come from" has 'its orch-deploy bundle is not with it. Give the update the release directory'
OUT="$(in_updater "fstate_set files_generation 2; plan v0.1.4 2 ''")" && CODE=0 || CODE=$?
expect "the generation the last update recorded is what it is checked against" 0 "$CODE"

# --- 4. The guards ----------------------------------------------------------------------------------------------------------

step "a set that would break the installation is kept, with one line to say what to do"
install_set "$WORK/v0.1.3"
files_of "$WORK/r1" 'api openapi' 18; cp "$NEW_UPDATER" "$WORK/r1/update.sh"
bundle r1 v0.1.4 2 1 "$WORK/r1" "$WORK/published.v0.1.3"
OUT="$(in_updater "plan v0.1.4 1 '$BUNDLES/r1/orch-deploy-v0.1.4.tar.gz'")" && CODE=0 || CODE=$?
expect "R1: another PostgreSQL major is not an error" 0 "$CODE"
check "R1: it says what it found and what to do" has 'the new postgres/Dockerfile builds PostgreSQL 18 and the data directory is PostgreSQL 17; this release needs a dump and restore into 18'
check "R1: the set stays" same "$STACK/nginx.conf" "$WORK/v0.1.3/nginx.conf"
check "R1: the shipped files are beside" test -e "$STACK/postgres/Dockerfile.new" -a -e "$STACK/nginx.conf.new"
install_set "$WORK/v0.1.3"
files_of "$WORK/r2" 'api openapi' 17 ./runtime/other-data; cp "$NEW_UPDATER" "$WORK/r2/update.sh"
bundle r2 v0.1.4 2 1 "$WORK/r2" "$WORK/published.v0.1.3"
OUT="$(in_updater "plan v0.1.4 1 '$BUNDLES/r2/orch-deploy-v0.1.4.tar.gz'")" && CODE=0 || CODE=$?
check "R2: a Compose file that stops binding ./runtime/data is kept" has 'the new compose.yml no longer binds ./runtime/data'
check "R2: and says what to do" has 'then merge compose.yml.new'
install_set "$WORK/v0.1.3"
files_of "$WORK/r3" 'api openapi' 17 ./runtime/data 'nginx:1.31@sha256:n3'; cp "$NEW_UPDATER" "$WORK/r3/update.sh"
bundle r3 v0.1.4 2 1 "$WORK/r3" "$WORK/published.v0.1.3"
: > "$WORK/docker.log"
OUT="$(in_updater "plan v0.1.4 1 '$BUNDLES/r3/orch-deploy-v0.1.4.tar.gz'")" && CODE=0 || CODE=$?
check "R3: an image that is not here and cannot be pulled keeps the set" has 'names the image nginx:1.31@sha256:n3, which this host does not have and could not pull'
check "R3: the pull was tried, in the plan, before anything" grep -q 'docker pull --quiet nginx:1.31@sha256:n3' "$WORK/docker.log"
install_set "$WORK/v0.1.3"
printf 'nginx:1.31@sha256:n3\n' > "$WORK/images.pullable"
OUT="$(in_updater "plan v0.1.4 1 '$BUNDLES/r3/orch-deploy-v0.1.4.tar.gz'")" && CODE=0 || CODE=$?
check "R3: an image that can be pulled is pulled in staging, and the set is replaced" has 'files: replacing compose.yml (release v0.1.4'"'"'s, generation 2)'
check "R3: it is on the host now" grep -Fxq 'nginx:1.31@sha256:n3' "$WORK/images.have"
install_set "$WORK/v0.1.3"
: > "$WORK/images.pullable"; printf 'nginx:1.30@sha256:n1\ndebian:bookworm@sha256:d1\n' > "$WORK/images.have"
OUT="$(in_updater "ORCH_OFFLINE_IMAGE_MAP='$WORK/map.json'; printf '{}' > '$WORK/map.json'; plan v0.1.4 1 '$BUNDLES/r3/orch-deploy-v0.1.4.tar.gz'")" && CODE=0 || CODE=$?
check "R3: with carried-in images, one that is not among them keeps the set" has 'which is not among the carried-in images'

# --- 5. The updater is its own -------------------------------------------------------------------------------------------------

step "update.sh is replaced only by a newer published one, and an edited one is kept"
install_set "$WORK/v0.1.3"
printf '# an operator tweak\n' >> "$STACK/update.sh"
OUT="$(in_updater "plan v0.1.4 1 '$G2'; echo \"update.sh: \$UPDATE_SH\"")" && CODE=0 || CODE=$?
check "an edited updater is kept" has 'files: kept update.sh (it differs from every published version); the shipped one is update.sh.new'
check "it is not to be replaced" has 'update.sh: none'
check "the shipped one is beside" same "$STACK/update.sh.new" "$NEW_UPDATER"
install_set "$WORK/v0.1.3"
{ cat "$WORK/update.sh.installed"; printf '# the updater of v0.1.5\n'; } > "$WORK/newer-update.sh"
{ cat "$WORK/published.v0.1.4"; printf 'file v0.1.5 3 %s update.sh\n' "$(sha256sum "$WORK/newer-update.sh" | cut -d' ' -f1)"; } > "$WORK/published.newer"
cp "$WORK/newer-update.sh" "$STACK/update.sh"
bundle older v0.1.4 2 1 "$WORK/v0.1.4" "$WORK/published.newer"
OUT="$(in_updater "plan v0.1.4 1 '$BUNDLES/older/orch-deploy-v0.1.4.tar.gz'; echo \"update.sh: \$UPDATE_SH\"")" && CODE=0 || CODE=$?
check "an updater newer than the release is left alone" has 'update.sh: none'
check "and nothing is said of it" lacks 'update.sh'"'"'s'
check "nor of a replacement" lacks 'update.sh is replaced'

# --- 6. Rolling back, and going forward again --------------------------------------------------------------------------------

step "a rollback puts the files back where they are still what the update wrote, and never rolls the updater back"
install_set "$WORK/v0.1.3"
in_updater "plan v0.1.4 1 '$G2'; begin up1; apply_files; journal_load; files_finish" > /dev/null || true
echo '# edited after the update' >> "$STACK/nginx.conf"
EDITED_AFTER="$(sha256sum < "$STACK/nginx.conf")"
OUT="$(in_updater "TAG=v0.1.4; plan_files_rollback; begin_back rb1; apply_files; echo \"back: \${FILES_REPLACE[*]}\"; files_finish")" || true
check "the file the operator edited since stays, and the command says so" has 'files: nginx.conf stays as it is: it is no longer what the last update wrote'
check "and so does what it left of it" bash -c "test \"\$(sha256sum < '$STACK/nginx.conf')\" = '$EDITED_AFTER'"
check "the other files are the earlier release's" same "$STACK/README.md" "$WORK/v0.1.3/README.md"
check "a file the update added is removed" test ! -e "$STACK/install/install-node.sh"
check "the updater stays the newer one" same "$STACK/update.sh" "$NEW_UPDATER"
check "the rollback is recorded as what the next one puts back" grep -q '^files_previous=rb1$' "$STACK/runtime/update/files.state"
check "and the generation stays, as a file of the update is still in use" grep -q '^files_generation=2$' "$STACK/runtime/update/files.state"
check "the files of the update before are gone: one set of saved files is kept" test ! -e "$STACK/runtime/update/files/up1"

step "a rollback of an update that nobody edited puts every file back, and again goes forward"
install_set "$WORK/v0.1.3"
in_updater "plan v0.1.4 1 '$G2'; begin up2; apply_files; journal_load; files_finish" > /dev/null || true
OUT="$(in_updater "TAG=v0.1.4; plan_files_rollback; begin_back rb2; apply_files; journal_load; files_finish; echo \"gen: \$FILES_GEN_AFTER\"")" || true
check "nginx.conf is the earlier release's" same "$STACK/nginx.conf" "$WORK/v0.1.3/nginx.conf"
check "the earlier file table is restored" same "$STACK/FILE_SETS" "$WORK/v0.1.3/FILE_SETS"
check "README.md is the earlier release's" same "$STACK/README.md" "$WORK/v0.1.3/README.md"
check "the generation is the one before" grep -q '^files_generation=1$' "$STACK/runtime/update/files.state"
OUT="$(in_updater "TAG=v0.1.3; plan_files_rollback; begin_back fw1; apply_files; journal_load; files_finish")" || true
check "forward again: nginx.conf is the later release's" same "$STACK/nginx.conf" "$WORK/v0.1.4/nginx.conf"
check "forward again: the file the update added is back" same "$STACK/install/install-node.sh" "$WORK/v0.1.4/install/install-node.sh"
check "forward again: the generation is the update's" grep -q '^files_generation=2$' "$STACK/runtime/update/files.state"

# --- 7. Recover ---------------------------------------------------------------------------------------------------------------

step "a transaction cut off while the files were written puts every file back; one edited since stays"
install_set "$WORK/v0.1.3"
in_updater "plan v0.1.4 1 '$G2'; begin cut1" > /dev/null || true
# The cut: the first file of the list was written, and the rest were not.
(cd "$STACK" && cp runtime/update/files/cut1/new/nginx.conf nginx.conf)
OUT="$(in_updater 'journal_load; restore_files; echo restored')" || true
check "the written file is the earlier one again" same "$STACK/nginx.conf" "$WORK/v0.1.3/nginx.conf"
check "the files not written are as they were" same "$STACK/README.md" "$WORK/v0.1.3/README.md"
check "no file is left that the update was going to add" test ! -e "$STACK/install/install-node.sh"
in_updater "plan v0.1.4 1 '$G2'; begin cut2; apply_files" > /dev/null || true
echo '# edited by hand' >> "$STACK/nginx.conf"
OUT="$(in_updater 'journal_load; restore_files')" || true
check "a file edited since stays, and the recover says so" has 'nginx.conf was left as it is: it is no longer what the transaction wrote'
check "the others are put back" same "$STACK/README.md" "$WORK/v0.1.3/README.md"
check "and a file the transaction added is removed" test ! -e "$STACK/install/install-node.sh"

step "an update to the release that is installed does nothing, and says when its files are older than its programs need"
install_set "$WORK/v0.1.3"
OUT="$(in_updater "TAG=v0.1.4; MANIFEST_FLAT=\"\$WORK/m\"; printf 'deploy_files.minimum\t2\n' > \"\$MANIFEST_FLAT\"; files_notice")"
check "it says so" has 'works with deployment files of generation 2 or newer, and these are generation 1'
OUT="$(in_updater "TAG=v0.1.4; MANIFEST_FLAT=\"\$WORK/m\"; printf 'deploy_files.minimum\t1\n' > \"\$MANIFEST_FLAT\"; files_notice")"
expect "and says nothing when they are not" "" "$OUT"

# --- 8. The compose.yml change reaches the database's container -----------------------------------------------------------------

step "a changed compose.yml makes the database's container again, and an unchanged one does not"
OUT="$(in_updater 'database_start() { echo "database_start called"; }; J=(); J[files]="nginx.conf compose.yml"; files_database_start; J[files]="nginx.conf"; files_database_start; echo done')"
expect "it is made again once, for the change of compose.yml" 1 "$(grep -c 'database_start called' <<< "$OUT")"

# --- 9. A script replaced while it runs ----------------------------------------------------------------------------------------

step "the updater replaced by a rename, or written over, does not run in halves"
mkdir -p "$WORK/self"
cat > "$WORK/self/update.sh" << 'SELF'
#!/usr/bin/env bash
set -Eeuo pipefail
main() { sleep 1; echo "first half of version one"; sleep 1; echo "second half of version one"; }
[ "${BASH_SOURCE[0]}" != "$0" ] || { main "$@"; exit "$?"; }
SELF
cp "$WORK/self/update.sh" "$WORK/self/original"
chmod +x "$WORK/self/update.sh"
printf '#!/usr/bin/env bash\necho "version two ran"\n%s\n' "$(head -c 2000 /dev/zero | tr '\0' '#')" > "$WORK/self/next"
chmod +x "$WORK/self/next"
"$WORK/self/update.sh" > "$WORK/self/out" 2>&1 &
RUNNING=$!
sleep 0.4
mv -T "$WORK/self/next" "$WORK/self/update.sh"
wait "$RUNNING" && CODE=0 || CODE=$?
expect "replaced by a rename while it runs, it finishes: exit 0" 0 "$CODE"
expect "and says what version one says, and nothing of version two" $'first half of version one\nsecond half of version one' "$(cat "$WORK/self/out")"
cp "$WORK/self/original" "$WORK/self/update.sh"
"$WORK/self/update.sh" > "$WORK/self/out" 2>&1 &
RUNNING=$!
sleep 0.4
{ printf '#!/usr/bin/env bash\necho "version two ran"\n'; head -c 3000 /dev/zero | tr '\0' '#'; printf '\n'; } > "$WORK/self/update.sh"
wait "$RUNNING" && CODE=0 || CODE=$?
expect "written over in place while it runs, it still finishes: exit 0" 0 "$CODE"
check "and nothing of the new text runs" bash -c "! grep -q 'version two ran' '$WORK/self/out'"
check "update.sh itself ends with the call of main and the exit after it" bash -c "tail -n 1 '$SMOKE_REPO/deploy/update.sh' | grep -Fq 'main \"\$@\"; exit \"\$?\"'"

# --- 10. The first hop ----------------------------------------------------------------------------------------------------------

step "from the updater of a published release: the command of the upgrade notes checks the digest before the file is used"
SITE="$WORK/site"; mkdir -p "$SITE/download/v0.1.4"
cp "$NEW_UPDATER" "$SITE/download/v0.1.4/update.sh"
DIGEST="$(sha256sum "$NEW_UPDATER" | cut -d' ' -f1)"
# The command is the one in the guide, between its markers, with its placeholders filled in.
FIRST_HOP="$(sed -n '/<!-- first-hop -->/,/<!-- \/first-hop -->/p' "$SMOKE_REPO/docs/operator/upgrades.md" | sed '/^<!--/d; /^```/d')"
hop() {
  printf '%s\n' "$FIRST_HOP" | sed -e "s|/opt/orch/deploy|$STACK|" -e "s|<base>|file://$SITE|" -e "s|<tag>|v0.1.4|g" -e "s|<sha256>|$1|" \
    -e 's|^\./update.sh .*||' > "$WORK/hop.sh"
  bash "$WORK/hop.sh"
}
install_set "$WORK/v0.1.3"
refuses "a digest that is not the file's: the command fails" hop 0000000000000000000000000000000000000000000000000000000000000000
check "and the updater in use is as it was" same "$STACK/update.sh" "$WORK/update.sh.installed"
rm -f "$STACK/update.sh.next"
check "the right digest: the command succeeds" hop "$DIGEST"
check "and the updater is the one of the release, executable" bash -c "cmp -s '$STACK/update.sh' '$NEW_UPDATER' && test -x '$STACK/update.sh'"
install_set "$WORK/v0.1.3"; printf 'kind=update\nstep=activating\n' > "$STACK/runtime/update/journal"
refuses "with an unfinished update it refuses, whatever the digest" hop "$DIGEST"
check "and the updater in use is as it was" same "$STACK/update.sh" "$WORK/update.sh.installed"

# --- 11. The directory the installation records are mounted on -----------------------------------------------------------------

step "the installation records' mount point is the stack user's before the API's container starts"
# The API's container mounts runtime/update inside the server's data directory, and Docker makes a missing mount point
# there itself, owned by root. A call that fails still prints, so that its check says what it found.
rm -rf -- "$STACK/runtime/data"
OUT="$(in_updater 'SERVICE_UID=$(id -u); SERVICE_GID=$(id -g); prepare_dirs; stat -c "%u %a" runtime/data/installation')" || true
expect "the directories made before every start hold it, the stack user's" "$(id -u) 755" "$(tail -n 1 <<< "$OUT")"
# One an earlier start left to Docker: here, a directory that is not the services' user's (they are someone else).
mkdir -p "$STACK/runtime/data/installation"; chmod 700 "$STACK/runtime/data/installation"
OUT="$(in_updater 'SERVICE_UID=4242; SERVICE_GID=4242; prepare_dirs; echo prepared')" || true
expect "the directories made while the API runs leave it as it is, and go on" "prepared 700" "$(tail -n 1 <<< "$OUT") $(stat -c %a "$STACK/runtime/data/installation")"
OUT="$(in_updater 'SERVICE_UID=4242; SERVICE_GID=4242; check_installation() { :; }; compose() { echo "compose $1 finds $(stat -c %a runtime/data/installation)" >&2; }; app_start')" || true
expect "the start of the services makes it again, before Compose recreates them" "compose up finds 755" "$(tail -n 1 <<< "$OUT")"
touch "$STACK/runtime/data/installation/kept"; chmod 700 "$STACK/runtime/data/installation"
OUT="$(in_updater 'SERVICE_UID=4242; SERVICE_GID=4242; check_installation() { :; }; compose() { :; }; app_start; echo started')" || true
expect "one that holds anything is left as it is" "started 700" "$(tail -n 1 <<< "$OUT") $(stat -c %a "$STACK/runtime/data/installation")"
check "with what it holds" test -e "$STACK/runtime/data/installation/kept"
