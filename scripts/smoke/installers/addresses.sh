#!/bin/sh
# Exercise the actual installers up to their first download, with every write/network action stubbed.
# Integrator-only regression: no root, real server, temporary directory or program installation needed.
set -eu
address_test_dir=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)
address_test_root=$(CDPATH= cd -- "$address_test_dir/../../.." && pwd)
address_test_tab=$(printf '\t')

attempt() {
  LANG="$1" LC_ALL= LC_MESSAGES= sh -c '
    fixture_script=$1
    fixture_address=$2
    fixture_backend=$3
    fixture_kind=$4
    forbidden() { printf "D441_FORBIDDEN %s\n" "$1" >&2; exit 98; }
    uname() { case "$1" in -s) printf "Linux\n" ;; -m) printf "x86_64\n" ;; *) forbidden uname ;; esac; }
    id() { printf "0\n"; }
    command() {
      [ "$1" = -v ] || forbidden command
      case "$2" in
        curl) [ "$fixture_backend" = curl ] ;;
        wget|sha256sum|docker) return 0 ;;
        *) return 1 ;;
      esac
    }
    mktemp() { printf "/D441-no-directory-is-created\n"; }
    # The failed-download trap cleans only this nonexistent fixture path; no real rm is called.
    rm() { :; }
    mkdir() { forbidden mkdir; }
    rmdir() { forbidden rmdir; }
    cp() { forbidden cp; }
    mv() { forbidden mv; }
    install() { forbidden install; }
    systemctl() { forbidden systemctl; }
    launchctl() { forbidden launchctl; }
    curl() { printf "D441_DOWNLOAD curl\n"; printf "%s\n" "$@"; exit 73; }
    wget() { printf "D441_DOWNLOAD wget\n"; printf "%s\n" "$@"; exit 73; }
    if [ "$fixture_kind" = node-http-gateway ]; then
      set -- --url "$fixture_address" --from https://orch.example --token fixture-not-a-real-token --no-service
    elif [ "$fixture_kind" = node ]; then
      set -- --url wss://gateway.example:9443 --from "$fixture_address" --token fixture-not-a-real-token --no-service
    else
      set -- --url "$fixture_address" --token fixture-not-a-real-token --no-service
    fi
    . "$fixture_script"
  ' sh "$2" "$3" "$4" "$5" 2>&1
}

contains() { case "$1" in *"$2"*) return 0 ;; *) return 1 ;; esac; }
exercise_case() {
  expected=$1
  address=$2
  for address_test_lang in en zh_CN; do
    for address_test_kind in connector node; do
      case "$address_test_kind" in connector) installer=install.sh ;; node) installer=install-node.sh ;; esac
      for address_test_backend in curl wget; do
        status=0
        output=$(attempt "$address_test_lang" "$address_test_root/deploy/install/$installer" "$address" "$address_test_backend" "$address_test_kind") || status=$?
        if contains "$output" D441_FORBIDDEN; then
          printf '%s\n' "$output" >&2; exit 1
        fi
        if [ "$expected" = allow ]; then
          if [ "$status" != 73 ] || ! contains "$output" "D441_DOWNLOAD $address_test_backend"; then
            printf 'refused permitted address (%s/%s/%s): %s\n%s\n' "$address_test_lang" "$installer" "$address_test_backend" "$address" "$output" >&2; exit 1
          fi
          if [ "$address_test_backend" = curl ]; then
            protocols='=https'
            case "$address" in [Hh][Tt][Tt][Pp]://*) protocols='=http,https' ;; esac
            contains "$output" "$(printf '%s\n%s' --proto "$protocols")" || exit 1
            contains "$output" "$(printf '%s\n%s' --proto-redir '=https')" || exit 1
          else
            contains "$output" --max-redirect=0 || exit 1
          fi
        elif [ "$status" = 0 ] || contains "$output" D441_DOWNLOAD || ! contains "$output" 127.0.0.0/8 || ! contains "$output" HTTPS; then
          printf 'address was not refused plainly (%s/%s/%s): %s\n%s\n' "$address_test_lang" "$installer" "$address_test_backend" "$address" "$output" >&2; exit 1
        fi
      done
    done
  done
}
while IFS="$address_test_tab" read -r expected address; do
  case "$expected" in ''|'#'*) continue ;; esac
  exercise_case "$expected" "$address"
done < "$address_test_dir/addresses.tsv"
exercise_case deny "$(printf 'http://localhost\nhttp://evil.example')"
exercise_case deny "$(printf 'http://localhost\rhttp://evil.example')"
status=0
output=$(attempt en "$address_test_root/deploy/install/install-node.sh" http://localhost:41288 curl node-http-gateway) || status=$?
if [ "$status" = 0 ] || contains "$output" D441_DOWNLOAD || ! contains "$output" wss://; then
  printf 'the node gateway must retain its pinned TLS transport\n%s\n' "$output" >&2; exit 1
fi
printf '%s\n' 'PASS installer addresses: shared authority cases, both shell installers/locales/backends; no real downloads or writes'
