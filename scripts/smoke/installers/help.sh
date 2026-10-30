#!/bin/sh
# Offline installer help must finish without credentials, root, platform probes or side effects.
# Run only by the integrator. This fixture writes no files and makes no network requests.
set -eu

installer_test_dir=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)
installer_test_root=$(CDPATH= cd -- "$installer_test_dir/../../.." && pwd)

help_for() {
  LANG="$1" LC_ALL= LC_MESSAGES= sh -c '
    installer=$1
    option=$2
    blocked() { printf "help attempted %s\n" "$1" >&2; exit 97; }
    id() { blocked id; }
    uname() { blocked uname; }
    curl() { blocked curl; }
    wget() { blocked wget; }
    mktemp() { blocked mktemp; }
    mkdir() { blocked mkdir; }
    cp() { blocked cp; }
    mv() { blocked mv; }
    install() { blocked install; }
    systemctl() { blocked systemctl; }
    launchctl() { blocked launchctl; }
    set -- "$option"
    . "$installer"
  ' sh "$2" "$3" 2>&1
}

expect_text() {
  case "$1" in
    *"$2"*) ;;
    *) printf 'installer help is missing: %s\n' "$2" >&2; exit 1 ;;
  esac
}

for installer_test_lang in en zh_CN; do
  for installer_test_flag in --help -h; do
    connector_help=$(help_for "$installer_test_lang" "$installer_test_root/deploy/install/install.sh" "$installer_test_flag")
    node_help=$(help_for "$installer_test_lang" "$installer_test_root/deploy/install/install-node.sh" "$installer_test_flag")
    expect_text "$connector_help" '~/.local/bin/orch-connector'
    expect_text "$connector_help" '~/Library/Application Support/Orch/connector'
    expect_text "$connector_help" 'XDG_DATA_HOME'
    expect_text "$connector_help" 'service stop'
    expect_text "$connector_help" 'service uninstall'
    expect_text "$connector_help" '--data-dir'
    expect_text "$connector_help" '--no-service'
    expect_text "$connector_help" 'Ctrl+C'
    expect_text "$node_help" '/usr/local/bin/orch-node'
    expect_text "$node_help" '/etc/orch/orch.toml'
    expect_text "$node_help" '/var/lib/orch-node'
    expect_text "$node_help" '--no-service'
    expect_text "$node_help" 'systemctl disable --now orch-node.service'
    expect_text "$node_help" 'systemctl daemon-reload'
    expect_text "$node_help" 'Ctrl+C'
    case "$installer_test_lang" in
      en)
        expect_text "$connector_help" 'your account'
        expect_text "$connector_help" 'Several connectors'
        expect_text "$node_help" 'as root'
        expect_text "$node_help" 'One node per machine'
        ;;
      zh_CN)
        expect_text "$connector_help" '你的账号'
        expect_text "$connector_help" '多个连接器'
        expect_text "$node_help" 'root 账号'
        expect_text "$node_help" '每台机器只运行一个节点'
        ;;
    esac
  done
done
printf '%s\n' 'PASS installer help: both languages and aliases, without installation or platform access'
