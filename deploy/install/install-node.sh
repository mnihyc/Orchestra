#!/bin/sh
# Installs orch-node on a Linux host, so that an Orch system can run managed environments there.
#
#   curl -fsSL 'https://orch.example.com/api/v1/downloads/install-node.sh' | sh -s -- \
#     --from 'https://orch.example.com' --url 'wss://gateway.example.com:9443' --token 'oenr_….<pin>' \
#     [--drivers docker,host] [--name NAME] [--delegate-policy] [--no-service]
#
# It fetches the node for this processor from the system named by --from (the system's own
# address), checks its size and SHA-256 against the system's release manifest
# (and its BLAKE3 too where `b3sum` is installed), places it in /usr/local/bin, writes the node's
# configuration (/etc/orch/orch.toml, its [node] table: the gateway's address, the data
# directory, the drivers), enrolls the host with the host token at --url, and runs the node as the
# systemd unit orch-node.service unless --no-service is selected. A node runs as root, which is what
# reaching Docker amounts to, and needs no inbound port. Run again, it changes only what is not
# right yet: the same node is left in place, a bound Enroll again token replaces the retained identity, an existing configuration is kept, and the unit is written again and restarted.
# On failure a newly installed program, configuration or service is removed where possible;
# an updated existing program or service is not restored to its old version.
set -eu

FROM=""
URL=""
TOKEN=""
NAME=""
DRIVERS="docker"
DELEGATE_POLICY=0
SERVICE=1

BIN=/usr/local/bin/orch-node
CONFIG_DIR=/etc/orch
CONFIG="$CONFIG_DIR/orch.toml"
DATA_DIR=/var/lib/orch-node
DATA_DIR_SET=0
UNIT=/etc/systemd/system/orch-node.service

say() { printf '%s\n' "$*" >&2; }
die() {
  say "error: $*"
  exit 1
}

usage() {
  case "${LC_ALL:-${LC_MESSAGES:-${LANG:-en}}}" in
    zh*)
      cat >&2 <<'EOF'
以 root 账号在 Linux 上安装 orch-node 并注册主机。

  install-node.sh --url <网关地址> --token <主机令牌>
                  [--from <系统地址>] [--drivers docker,host] [--name <名称>]
                  [--delegate-policy] [--no-service]

程序：/usr/local/bin/orch-node；配置：/etc/orch/orch.toml；数据：/var/lib/orch-node。
默认使用系统服务 orch-node.service；停止：systemctl stop orch-node.service；启动：systemctl start orch-node.service。
仅移除服务：运行 systemctl disable --now orch-node.service，移除单元文件后运行 systemctl daemon-reload；数据和身份保留。
直连网关的 9443 端口不提供下载；此时请用 --from 指定用户访问的系统地址。
省略 --from 时从 --url 推导地址，仅在该地址也提供系统下载时可用。
前台运行：安装时加 --no-service，然后以 root 运行
  /usr/local/bin/orch-node --config /etc/orch/orch.toml run
按 Ctrl+C 停止；已有服务需先停止，此模式无需 systemd。
每台机器只运行一个节点；在主机的“访问”页面共享给其他组织，不另建节点或数据目录。
卸载：orch-node uninstall。先列出计划，再输入主机名称确认。
加 --purge 同时清理此系统的运行时对象和节点数据；不加则保留数据。
加 --plan 只预览；--yes 跳过输入名称。保留其他系统的对象和安装前已有的配置。

  --url              注册并连接的网关地址（wss://…:9443 或 wss://…/link）
  --token            系统显示的一次性主机令牌（oenr_…）
  --from             系统下载地址：HTTPS；HTTP 仅限 127.0.0.0/8、[::1] 或 localhost
  --config           此节点已有的配置文件（默认 /etc/orch/orch.toml）
  --data-dir         此节点已有的数据目录（默认 /var/lib/orch-node）
  --drivers          节点加载的驱动，以逗号分隔；默认 docker
  --name             主机在系统中的名称；省略时使用机器名
  --delegate-policy  将节点设置交给 Orch；本机保留值仍然优先
  --no-service       仅安装和注册；不创建服务，随后自行前台运行
  -h, --help         显示说明；无需 root 或令牌，不下载或安装
EOF
      return
      ;;
  esac
  cat >&2 <<'EOF'
Installs orch-node as root on this Linux host and enrolls the host.

  install-node.sh --url <gateway address> --token <host token>
                  [--from <system address>] [--drivers docker,host] [--name <name>]
                  [--delegate-policy] [--no-service]

Program: /usr/local/bin/orch-node; configuration: /etc/orch/orch.toml; data: /var/lib/orch-node.
Default: the system service orch-node.service. Stop: systemctl stop orch-node.service; start: systemctl start orch-node.service.
Remove only the service: systemctl disable --now orch-node.service, remove its unit file,
then run systemctl daemon-reload. Data and identity stay.
For a direct gateway on port 9443, set --from to the system's web address; the gateway does not serve downloads.
Without --from, the origin is derived from --url and must also serve system downloads.
Foreground: install with --no-service, then run as root:
  /usr/local/bin/orch-node --config /etc/orch/orch.toml run
Ctrl+C stops it; stop any existing service first. This mode needs no systemd.
One node per machine: share its host through Access to serve other organizations;
do not install a second node or give it another --data-dir.
Uninstall: orch-node uninstall. Read its plan, then type the host's name.
Add --purge to remove this system's runtime objects and node data; otherwise data stays.
Use --plan to preview or --yes to skip typing. Other systems and supplied configuration stay.

  --url      the gateway address for enrollment and connections (wss://…:9443 or wss://…/link)
  --token    the host token the system showed (oenr_…)
  --from     HTTPS system download address; HTTP only on 127.0.0.0/8, [::1] or localhost
  --config   this node's existing configuration file (/etc/orch/orch.toml)
  --data-dir this node's existing identity directory (/var/lib/orch-node)
  --drivers  the drivers the node loads, separated by commas (docker)
  --name     what the host is called in the system (the machine's name if left out)
  --delegate-policy  hand over the node's settings to Orch, while explicit local values still win
  --no-service       install and enroll without a service; start it yourself in the foreground
  -h, --help         show this help without root, a token, a download or an installation
EOF
}

# Kept in both standalone shell installers: piping one file must need no unchecked helper download.
# Match the URL authority, never a path substring, DNS result, userinfo or an encoded host.
system_address_ok() {
  case "$1" in *[[:space:]]*) return 1 ;; esac
  printf '%s\n' "$1" | awk '
    function zero(group) { return length(group) >= 1 && length(group) <= 4 && group ~ /^0+$/ }
    function one(group) { return length(group) >= 1 && length(group) <= 4 && group ~ /^0*1$/ }
    function loopback6(host, halves, a, b, parts, left, right, i) {
      parts = split(host, halves, "::")
      if (parts == 1) {
        left = split(host, a, ":")
        if (left != 8 || !one(a[8])) return 0
        for (i = 1; i < 8; i++) if (!zero(a[i])) return 0
        return 1
      }
      if (parts != 2 || halves[2] == "") return 0
      left = halves[1] == "" ? 0 : split(halves[1], a, ":")
      right = split(halves[2], b, ":")
      if (left + right >= 8 || !one(b[right])) return 0
      for (i = 1; i <= left; i++) if (!zero(a[i])) return 0
      for (i = 1; i < right; i++) if (!zero(b[i])) return 0
      return 1
    }
    {
      if ($0 ~ /[[:space:]\\?#]/) exit 1
      scheme = tolower($0)
      if (scheme !~ /^https?:\/\//) exit 1
      sub(/:.*/, "", scheme)
      authority = substr($0, index($0, "://") + 3)
      sub(/\/.*/, "", authority)
      if (authority == "" || authority ~ /[@%]/) exit 1
      bracket = substr(authority, 1, 1) == "["
      if (bracket) {
        end = index(authority, "]")
        if (end < 3) exit 1
        host = substr(authority, 2, end - 2)
        port = substr(authority, end + 1)
      } else {
        host = authority
        sub(/:.*/, "", host)
        port = substr(authority, length(host) + 1)
        if (host == "" || host ~ /[][]/) exit 1
      }
      if (port != "" && (port !~ /^:[0-9]+$/ || substr(port, 2) + 0 > 65535)) exit 1
      if (scheme == "https") exit 0
      if (bracket) exit loopback6(host) ? 0 : 1
      if (tolower(host) == "localhost") exit 0
      count = split(host, octets, "[.]")
      if (count != 4 || octets[1] !~ /^127$/) exit 1
      for (i = 1; i <= count; i++)
        if (octets[i] !~ /^(0|[1-9][0-9]?[0-9]?)$/ || octets[i] + 0 > 255) exit 1
      exit 0
    }'
}

system_address_error() {
  case "${LC_ALL:-${LC_MESSAGES:-${LANG:-en}}}" in
    zh*) die "$1 需要 HTTPS；HTTP 仅限回环主机 127.0.0.0/8、::1 或 localhost。其他主机上的 HTTP 会明文传输注册与下载，请改用 HTTPS；地址不能包含账号、查询参数或片段。" ;;
    *) die "$1 needs HTTPS; HTTP is allowed only on loopback hosts (127.0.0.0/8, ::1 or localhost). HTTP elsewhere exposes enrollment and downloads; use HTTPS. Omit userinfo, a query or a fragment from the base address." ;;
  esac
}

needs_value() { [ "$2" -ge 2 ] || die "$1 needs a value"; }
while [ $# -gt 0 ]; do
  case "$1" in
    --url) needs_value "$1" $#; URL=$2; shift 2 ;;
    --url=*) URL=${1#--url=}; shift ;;
    --token) needs_value "$1" $#; TOKEN=$2; shift 2 ;;
    --token=*) TOKEN=${1#--token=}; shift ;;
    --from) needs_value "$1" $#; FROM=$2; shift 2 ;;
    --from=*) FROM=${1#--from=}; shift ;;
    --config) needs_value "$1" $#; CONFIG=$2; CONFIG_DIR=$(dirname "$CONFIG"); shift 2 ;;
    --config=*) CONFIG=${1#--config=}; CONFIG_DIR=$(dirname "$CONFIG"); shift ;;
    --data-dir) needs_value "$1" $#; DATA_DIR=$2; DATA_DIR_SET=1; shift 2 ;;
    --data-dir=*) DATA_DIR=${1#--data-dir=}; DATA_DIR_SET=1; shift ;;
    --drivers) needs_value "$1" $#; DRIVERS=$2; shift 2 ;;
    --drivers=*) DRIVERS=${1#--drivers=}; shift ;;
    --name) needs_value "$1" $#; NAME=$2; shift 2 ;;
    --name=*) NAME=${1#--name=}; shift ;;
    --delegate-policy) DELEGATE_POLICY=1; shift ;;
    --no-service) SERVICE=0; shift ;;
    -h | --help) usage; exit 0 ;;
    *) die "$1 is not an argument of this installer (see --help)" ;;
  esac
done
if printf '%s%s' "$CONFIG" "$DATA_DIR" | LC_ALL=C grep -q '[[:cntrl:]]'; then
  die "configuration and data paths contain no control characters"
fi
case "$CONFIG" in /*) ;; *) die "--config is an absolute path" ;; esac
case "$DATA_DIR" in /*) ;; *) die "--data-dir is an absolute path" ;; esac
[ -n "$URL" ] || die "--url is needed: the gateway's address"
[ -n "$TOKEN" ] || die "--token is needed: the host token the system showed"
case "$URL" in
  wss://?* | https://?*) ;;
  *) die "--url is a wss:// or https:// address; $URL is not one" ;;
esac
if [ -z "$FROM" ]; then
  FROM=$(printf '%s' "$URL" | sed -e 's#^wss://#https://#' -e 's#^\(https://[^/]*\).*#\1#')
fi
FROM=${FROM%/}
system_address_ok "$FROM" || system_address_error --from
DOWNLOAD_PROTOCOLS='=https'
case "$FROM" in [Hh][Tt][Tt][Pp]://*) DOWNLOAD_PROTOCOLS='=http,https' ;; esac
# The drivers as the configuration lists them: names of letters, digits, '-' and '_'.
DRIVER_LIST=""
OLD_IFS=$IFS
IFS=,
set -f
for driver in $DRIVERS; do
  case "$driver" in
    "" | *[!A-Za-z0-9_-]*) die "$driver is not the name of a driver" ;;
  esac
  DRIVER_LIST="${DRIVER_LIST:+$DRIVER_LIST, }\"$driver\""
done
set +f
IFS=$OLD_IFS
[ -n "$DRIVER_LIST" ] || die "--drivers names no driver"

[ "$(uname -s)" = Linux ] ||
  die "orch-node runs on Linux hosts; a $(uname -s) machine joins a system as a machine (Resources, Machines, Add machine)"
case "$(uname -m)" in
  x86_64 | amd64) ARCH=x86_64 ;;
  aarch64 | arm64) ARCH=aarch64 ;;
  *) die "no node is made for a $(uname -m) processor" ;;
esac
[ "$(id -u)" = 0 ] || die "the node is a service of the host: run this as root (pipe it to 'sudo sh -s --')"
if [ "$SERVICE" = 1 ]; then
  if ! command -v systemctl > /dev/null 2>&1 || [ ! -d /run/systemd/system ]; then
    die "this host does not run systemd; install with --no-service and start $BIN --config $CONFIG run under your own supervisor"
  fi
fi
case ",$DRIVERS," in
  *,docker,*) command -v docker > /dev/null 2>&1 || say "warning: the docker driver needs Docker, which is not installed here yet" ;;
esac

if command -v curl > /dev/null 2>&1; then
  fetch() { curl -fsSL --proto "$DOWNLOAD_PROTOCOLS" --proto-redir '=https' --tlsv1.2 -o "$2" "$1"; }
elif command -v wget > /dev/null 2>&1 && wget --help 2>&1 | grep -q -- '--max-redirect'; then
  fetch() { wget -q --max-redirect=0 -O "$2" "$1"; }
else
  case "${LC_ALL:-${LC_MESSAGES:-${LANG:-en}}}" in
    zh*) die "下载节点需要 curl 或支持 --max-redirect 的 wget；BusyBox wget 不支持此选项。" ;;
    *) die "downloading the node needs curl or a wget with --max-redirect; BusyBox wget does not support it" ;;
  esac
fi
if command -v sha256sum > /dev/null 2>&1; then
  sha256() { sha256sum "$1" | cut -d' ' -f1; }
elif command -v openssl > /dev/null 2>&1; then
  sha256() { openssl dgst -sha256 -r "$1" | cut -d' ' -f1; }
else
  die "sha256sum or openssl is needed to check the download"
fi

# What this run placed, taken away again if it fails.
WORK=$(mktemp -d)
PLACED_BIN=0
PLACED_CONFIG=0
PLACED_UNIT=0
finish() {
  status=$?
  set +e
  if [ "$status" -ne 0 ]; then
    if [ "$PLACED_UNIT" = 1 ]; then
      systemctl disable orch-node.service > /dev/null 2>&1
      rm -f "$UNIT"
      systemctl daemon-reload > /dev/null 2>&1
    fi
    if [ "$PLACED_CONFIG" = 1 ]; then rm -f "$CONFIG"; fi
    if [ "$PLACED_BIN" = 1 ]; then rm -f "$BIN"; fi
    say "what this run placed was taken away again"
  fi
  rm -rf "$WORK"
  exit "$status"
}
trap finish EXIT
trap 'exit 130' INT TERM

# flatten_json: JSON on standard input as `path<TAB>value` lines, one for each scalar.
flatten_json() {
  grep -oE '"([^"\\]|\\.)*"|-?[0-9]+(\.[0-9]+)?([eE][+-]?[0-9]+)?|true|false|null|[][{},:]' |
    awk '
      function unquote(t,   r) { r = substr(t, 2, length(t) - 2); gsub(/\\\//, "/", r); gsub(/\\"/, "\"", r); gsub(/\\\\/, "\\", r); return r }
      function emit(v,   i, p) { p = ""; for (i = 1; i <= d; i++) p = p (i > 1 ? "." : "") key[i]; print p "\t" v }
      function slot() { if (kind[d] == "a") { key[d] = idx[d]; idx[d]++ } }
      {
        t = $0
        if (t == "{") { slot(); d++; kind[d] = "o"; want[d] = "key"; key[d] = "" }
        else if (t == "[") { slot(); d++; kind[d] = "a"; idx[d] = 0; key[d] = "" }
        else if (t == "}" || t == "]") { d-- }
        else if (t == ",") { if (kind[d] == "o") want[d] = "key" }
        else if (t == ":") { want[d] = "value" }
        else if (substr(t, 1, 1) == "\"") {
          if (kind[d] == "o" && want[d] == "key") key[d] = unquote(t)
          else { slot(); emit(unquote(t)) }
        } else { slot(); emit(t) }
      }'
}
mget() { awk -F'\t' -v key="$1" '$1 == key { print $2; exit }' "$WORK/version.flat"; }

say "Asking $FROM for its release"
fetch "$FROM/api/v1/downloads/version.json" "$WORK/version.json" ||
  die "$FROM/api/v1/downloads/version.json could not be fetched: is $FROM the system's address (--from), and was it started from a release?"
flatten_json < "$WORK/version.json" > "$WORK/version.flat"
[ "$(mget project)" = orch ] || die "$FROM did not answer with the release manifest of an Orch system"
VERSION=$(mget version)
WANTED="/api/v1/downloads/node/$ARCH"
INDEX=0
FOUND=""
while :; do
  ADDRESS=$(mget "assets.$INDEX.download_url")
  [ -n "$ADDRESS" ] || break
  if [ "$ADDRESS" = "$WANTED" ]; then
    FOUND=$INDEX
    break
  fi
  INDEX=$((INDEX + 1))
done
[ -n "$FOUND" ] || die "release $VERSION of this system has no node for $ARCH"
SIZE=$(mget "assets.$FOUND.size")
SHA256=$(mget "assets.$FOUND.sha256")
BLAKE3=$(mget "assets.$FOUND.blake3")
[ -n "$SHA256" ] || die "release $VERSION lists no SHA-256 for the node, which this installer checks; it is older than this installer"

say "Downloading the node $VERSION for $ARCH"
fetch "$FROM$WANTED" "$WORK/orch-node" || die "the node could not be downloaded from $FROM$WANTED"
GOT_SIZE=$(wc -c < "$WORK/orch-node" | tr -d ' ')
[ "$GOT_SIZE" = "$SIZE" ] || die "the download is $GOT_SIZE bytes and the manifest lists $SIZE: it was cut or changed on the way"
[ "$(sha256 "$WORK/orch-node")" = "$SHA256" ] ||
  die "the download does not have the SHA-256 the manifest lists: it was damaged or changed on the way"
if command -v b3sum > /dev/null 2>&1; then
  [ "$(b3sum --no-names "$WORK/orch-node")" = "$BLAKE3" ] ||
    die "the download does not have the BLAKE3 hash the manifest lists: it was damaged or changed on the way"
fi
say "  checked: $SIZE bytes, SHA-256 as listed"

mkdir -p /usr/local/bin
if [ -f "$BIN" ] && [ "$(sha256 "$BIN")" = "$SHA256" ]; then
  say "The node $VERSION is in $BIN already"
else
  [ -e "$BIN" ] || PLACED_BIN=1
  install -m 0755 "$WORK/orch-node" "$BIN.new"
  mv -f "$BIN.new" "$BIN"
  say "Placed the node $VERSION in $BIN"
fi

# The configuration: written once; an operator's own is kept as it is.
toml_text() { printf '"%s"' "$(printf '%s' "$1" | sed -e 's/\\/\\\\/g' -e 's/"/\\"/g')"; }
if [ -f "$CONFIG" ]; then
  say "$CONFIG is there already and is kept as it is (its [node] table is the node's)"
else
  mkdir -p "$CONFIG_DIR"
  PLACED_CONFIG=1
  {
    printf '# The configuration of this host'"'"'s Orch node, written by install-node.sh.\n'
    printf '# See the [node] table of the deployment'"'"'s orch.toml for every key.\n'
    printf 'version = 1\n\n[node]\n'
    printf 'data_dir = %s\n' "$(toml_text "$DATA_DIR")"
    printf 'gateway_url = %s\n' "$(toml_text "$URL")"
    printf 'drivers = [%s]\n' "$DRIVER_LIST"
  } > "$CONFIG"
  chmod 0644 "$CONFIG"
  say "Wrote $CONFIG"
fi

run_node() {
  if [ "$DATA_DIR_SET" = 1 ]; then "$BIN" --config "$CONFIG" --data-dir "$DATA_DIR" "$@";
  else "$BIN" --config "$CONFIG" "$@"; fi
}
# A supplied token is checked against an existing identity too.
set -- enroll --url "$URL"
[ -z "$NAME" ] || set -- "$@" --name "$NAME"
[ "$DELEGATE_POLICY" = 0 ] || set -- "$@" --delegate-policy
ORCH_NODE_TOKEN=$TOKEN run_node "$@" ||
  die "enrolling did not succeed; use Enroll again on this host's page for an existing identity"
[ "$DELEGATE_POLICY" = 0 ] || run_node policy delegate all
WROTE_CONFIG=$PLACED_CONFIG
# An enrolled host keeps its program and its configuration, whatever happens next.
PLACED_BIN=0
PLACED_CONFIG=0

record_installation() {
  set -- __record-installation "$@"
  [ "$WROTE_CONFIG" = 0 ] || set -- "$@" --configuration
  run_node "$@"
}
if [ "$SERVICE" = 0 ]; then
  record_installation
  say "No service was installed (--no-service). Start the node as root:"
  printf '%s --config "%s"' "$BIN" "$(printf '%s' "$CONFIG" | sed -e 's/[\\"$`]/\\&/g')" >&2
  if [ "$DATA_DIR_SET" = 1 ]; then printf ' --data-dir "%s"' "$(printf '%s' "$DATA_DIR" | sed -e 's/[\\"$`]/\\&/g')" >&2; fi
  printf ' run\n' >&2
  say "Stop an existing orch-node.service before running in the foreground; Ctrl+C stops the foreground node."
  exit 0
fi

[ -e "$UNIT" ] || PLACED_UNIT=1
# A systemd quoted word preserves spaces, variables, percent signs and backslashes in retained paths.
systemd_word() { printf '"%s"' "$(printf '%s' "$1" | sed -e 's/\\/\\\\/g' -e 's/"/\\"/g' -e 's/%/%%/g' -e 's/\$/$$/g')"; }
# Environment= does not expand dollars, but it does expand percent specifiers.
proxy_environment() {
  for proxy_name in HTTPS_PROXY https_proxy HTTP_PROXY http_proxy ALL_PROXY all_proxy NO_PROXY no_proxy; do
    proxy_value=
    eval "proxy_value=\${$proxy_name-}"
    case "$proxy_value" in
      *[![:space:]]*) printf 'Environment="%s"\n' "$(printf '%s=%s' "$proxy_name" "$proxy_value" | sed -e 's/\\/\\\\/g' -e 's/"/\\"/g' -e 's/%/%%/g')" ;;
    esac
  done
}
data_arguments=""
[ "$DATA_DIR_SET" = 0 ] || data_arguments=" --data-dir $(systemd_word "$DATA_DIR")"
# A proxy may contain credentials; protect a new or existing unit before writing them.
(umask 077; : >> "$UNIT")
chmod 600 "$UNIT"
cat > "$UNIT" <<EOF
[Unit]
Description=Orch node
Documentation=$FROM
After=network-online.target docker.service
Wants=network-online.target

[Service]
ExecStart=$BIN --config $(systemd_word "$CONFIG")${data_arguments} run
$(proxy_environment)
Restart=on-failure
RestartSec=10

[Install]
WantedBy=multi-user.target
EOF
record_installation --service
systemctl daemon-reload
systemctl enable orch-node.service > /dev/null 2>&1
systemctl restart orch-node.service
PLACED_UNIT=0
say "Done: the node runs as orch-node.service. The host's page in Orch shows it within a minute."
