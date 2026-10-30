#!/bin/sh
# Installs the Orch connector for the person who runs it, on Linux or macOS.
#
#   curl -fsSL 'https://orch.example.com/api/v1/downloads/install.sh' | sh -s -- \
#     --url 'https://orch.example.com' --token 'oenr_….<pin>' \
#     [--name NAME] [--data-dir DIR] [--roles environment,agent] \
#     [--delegate environment,agent | --delegate-policy] [--dedicated] [--no-service]
#
# It fetches the connector for this system and processor from the Orch system named by --url,
# checks its size and SHA-256 against the system's release manifest (and its BLAKE3 too where
# `b3sum` is installed), places it in ~/.local/bin, enrolls this machine with the token, hands
# every limit over to Orch when --delegate-policy is given, and keeps the connector running as a
# per-user service: a systemd user unit on Linux, a launchd agent on macOS. It never needs
# administrator rights. Run again, it changes only what is not right yet: the same connector is
# left in place, a bound Enroll again token replaces its retained identity, and the
# service is written again and restarted. When it fails, it takes away what it placed itself.
set -eu

URL=""
TOKEN=""
NAME=""
ROLES=""
DELEGATE_SECTIONS=""
DATA_DIR=""
DELEGATE=0
DEDICATED=0
SERVICE=1

say() { printf '%s\n' "$*" >&2; }
die() {
  say "error: $*"
  exit 1
}

usage() {
  case "${LC_ALL:-${LC_MESSAGES:-${LANG:-en}}}" in
    zh*)
      cat >&2 <<'EOF'
为你的账号安装 Orch 连接器并注册这台机器；请勿使用 sudo。

  install.sh --url <系统地址> --token <注册令牌>
             [--name <名称>] [--data-dir <绝对路径>] [--roles environment,agent]
             [--delegate environment,agent | --delegate-policy] [--dedicated] [--no-service]

程序：~/.local/bin/orch-connector。数据：Linux 为 ~/.local/share/orch-connector
（遵循 XDG_DATA_HOME），macOS 为 ~/Library/Application Support/Orch/connector。
默认使用用户服务：Linux 的 systemd 用户单元，或 macOS 的 launchd 登录代理。
停止/启动：orch-connector service stop / start；Linux 未启用 lingering 时退出登录即停止。
前台运行：安装时加 --no-service，然后运行 orch-connector run，按 Ctrl+C 停止。
同一台机器可运行多个连接器；另加 --data-dir <绝对路径>，各有独立的身份和服务。
命名实例的后续命令都需使用相同的 --data-dir；已有服务需先停止，再前台运行。
卸载：orch-connector uninstall。先列出计划，再输入机器名称确认。
加 --purge 同时移除此实例的数据和它创建的目录；已有工作目录不删除。
加 --plan 只预览；--yes 跳过输入名称。最后一个实例移除后才删除共享程序。

  --url              系统地址：HTTPS；HTTP 仅限 127.0.0.0/8、[::1] 或 localhost
  --token            系统显示的一次性注册令牌（oenr_…）
  --name             机器在系统中的名称；省略时使用主机名
  --roles            environment、agent、provider，使用逗号分隔
  --delegate         将选中角色的设置交给 Orch；本机保留值仍然优先
  --data-dir         数据目录；指定路径的实例使用独立服务
  --delegate-policy  将所有设置交给 Orch；本机保留值仍然优先
  --dedicated        声明这台机器专供 Orch 使用，作为隔离边界
  --no-service       仅安装和注册；不创建服务，随后自行前台运行
  -h, --help         显示说明；不需要令牌，不下载或安装
EOF
      return
      ;;
  esac
  cat >&2 <<'EOF'
Installs an Orch connector for your account and enrolls this machine; do not use sudo.

  install.sh --url <system address> --token <enrollment token>
             [--name <name>] [--data-dir <dir>] [--roles environment,agent]
             [--delegate environment,agent | --delegate-policy] [--dedicated] [--no-service]

Program: ~/.local/bin/orch-connector. Data: ~/.local/share/orch-connector on Linux
(respects XDG_DATA_HOME), or ~/Library/Application Support/Orch/connector on macOS.
Default: a user service, through systemd on Linux or a launchd agent on macOS.
Stop/start: orch-connector service stop / start. Linux stops at logout without lingering.
Foreground: install with --no-service, then orch-connector run; Ctrl+C stops it.
Several connectors can run side by side: add --data-dir <absolute path> for another
identity and service; use that same option on its later commands. Stop any existing service first.
Uninstall: orch-connector uninstall. Read its plan, then type the machine's name.
Add --purge to remove its data and folders it made. Existing work folders stay.
Use --plan to preview or --yes to skip typing. The last instance removes the shared program.
Uninstall without --purge keeps data and identity; nested instances block their parent's purge.

  --url              HTTPS system address; HTTP only on 127.0.0.0/8, [::1] or localhost
  --token            the enrollment token the system showed (oenr_…)
  --name             what the machine is called in the system (the host's name if left out)
  --roles            environment,agent,provider (choose one or more, separated by commas)
  --delegate         the selected role sections handed to Orch, separated by commas
  --data-dir         another data directory, identity and service (platform default if left out)
  --delegate-policy  let every limit of this machine be changed from Orch
  --dedicated        declare this machine dedicated to Orch as its isolation boundary
  --no-service       install and enroll, and leave starting the connector to you
  -h, --help         show this help without a token, download or installation
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
    --name) needs_value "$1" $#; NAME=$2; shift 2 ;;
    --name=*) NAME=${1#--name=}; shift ;;
    --roles) needs_value "$1" $#; ROLES=$2; shift 2 ;;
    --roles=*) ROLES=${1#--roles=}; shift ;;
    --delegate) needs_value "$1" $#; DELEGATE_SECTIONS=$2; shift 2 ;;
    --delegate=*) DELEGATE_SECTIONS=${1#--delegate=}; shift ;;
    --data-dir) needs_value "$1" $#; DATA_DIR=$2; shift 2 ;;
    --data-dir=*) DATA_DIR=${1#--data-dir=}; shift ;;
    --delegate-policy) DELEGATE=1; shift ;;
    --dedicated) DEDICATED=1; shift ;;
    --no-service) SERVICE=0; shift ;;
    -h | --help) usage; exit 0 ;;
    *) die "$1 is not an argument of this installer (see --help)" ;;
  esac
done
[ -n "$URL" ] || die "--url is needed: the address of the Orch system"
[ -n "$TOKEN" ] || die "--token is needed: the enrollment token the system showed"
[ "$DELEGATE" = 0 ] || [ -z "$DELEGATE_SECTIONS" ] || die "use --delegate or --delegate-policy, not both"
case ",$ROLES," in
  *,node,*|*,Node,*|*,NODE,*) die "The node role is hosted by orch-node: add this computer as a host as well." ;;
esac
URL=${URL%/}
system_address_ok "$URL" || system_address_error --url
DOWNLOAD_PROTOCOLS='=https'
case "$URL" in [Hh][Tt][Tt][Pp]://*) DOWNLOAD_PROTOCOLS='=http,https' ;; esac

case "$(uname -s)" in
  Linux) OS=linux ;;
  Darwin) OS=macos ;;
  *) die "this installer is for Linux and macOS; on Windows the system offers install.ps1" ;;
esac
case "$(uname -m)" in
  x86_64 | amd64) ARCH=x86_64 ;;
  aarch64 | arm64) ARCH=aarch64 ;;
  *) die "no connector is made for a $(uname -m) processor" ;;
esac
[ -n "${HOME:-}" ] || die "HOME is not set: the connector is installed for a person, in their home"

# The tools: one that downloads, one that hashes.
if command -v curl > /dev/null 2>&1; then
  # Even a loopback HTTP origin cannot redirect a download to cleartext on another host.
  fetch() { curl -fsSL --proto "$DOWNLOAD_PROTOCOLS" --proto-redir '=https' --tlsv1.2 -o "$2" "$1"; }
elif command -v wget > /dev/null 2>&1; then
  fetch() { wget -q --max-redirect=0 -O "$2" "$1"; }
else
  die "curl or wget is needed to download the connector"
fi
if command -v sha256sum > /dev/null 2>&1; then
  sha256() { sha256sum "$1" | cut -d' ' -f1; }
elif command -v shasum > /dev/null 2>&1; then
  sha256() { shasum -a 256 "$1" | cut -d' ' -f1; }
elif command -v openssl > /dev/null 2>&1; then
  sha256() { openssl dgst -sha256 -r "$1" | cut -d' ' -f1; }
else
  die "sha256sum, shasum or openssl is needed to check the download"
fi

BIN_DIR="$HOME/.local/bin"
BIN="$BIN_DIR/orch-connector"
if [ -z "$DATA_DIR" ]; then
  if [ "$OS" = macos ]; then
    DATA_DIR="$HOME/Library/Application Support/Orch/connector"
  else
    DATA_DIR="${XDG_DATA_HOME:-$HOME/.local/share}/orch-connector"
  fi
  NAMED_DATA_DIR=0
else
  NAMED_DATA_DIR=1
fi

# What this run placed, taken away again if it fails.
WORK=$(mktemp -d 2> /dev/null || mktemp -d -t orch-install)
PLACED_BIN=0
PLACED_SERVICE=""
MADE_DATA_DIR=0
finish() {
  status=$?
  set +e
  if [ "$status" -ne 0 ]; then
    if [ "$PLACED_BIN" = 1 ]; then rm -f "$BIN"; fi
    case "$PLACED_SERVICE" in
      *.service) systemctl --user disable "${PLACED_SERVICE##*/}" > /dev/null 2>&1; rm -f "$PLACED_SERVICE" ;;
      *.plist) rm -f "$PLACED_SERVICE" ;;
    esac
    if [ "$MADE_DATA_DIR" = 1 ]; then rmdir "$DATA_DIR" 2> /dev/null; fi
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

# The system's release manifest, and the connector it lists for this machine.
say "Asking $URL for its release"
fetch "$URL/api/v1/downloads/version.json" "$WORK/version.json" ||
  die "$URL/api/v1/downloads/version.json could not be fetched: is $URL the system's address, and was it started from a release?"
flatten_json < "$WORK/version.json" > "$WORK/version.flat"
[ "$(mget project)" = orch ] || die "$URL did not answer with the release manifest of an Orch system"
VERSION=$(mget version)
WANTED="/api/v1/downloads/connector/$OS/$ARCH"
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
[ -n "$FOUND" ] || die "release $VERSION of this system has no connector for $OS on $ARCH; its operator can add one"
SIZE=$(mget "assets.$FOUND.size")
SHA256=$(mget "assets.$FOUND.sha256")
BLAKE3=$(mget "assets.$FOUND.blake3")
[ -n "$SHA256" ] || die "release $VERSION lists no SHA-256 for the connector, which this installer checks; it is older than this installer"

say "Downloading the connector $VERSION for $OS on $ARCH"
fetch "$URL$WANTED" "$WORK/orch-connector" || die "the connector could not be downloaded from $URL$WANTED"
GOT_SIZE=$(wc -c < "$WORK/orch-connector" | tr -d ' ')
[ "$GOT_SIZE" = "$SIZE" ] || die "the download is $GOT_SIZE bytes and the manifest lists $SIZE: it was cut or changed on the way"
[ "$(sha256 "$WORK/orch-connector")" = "$SHA256" ] ||
  die "the download does not have the SHA-256 the manifest lists: it was damaged or changed on the way"
if command -v b3sum > /dev/null 2>&1; then
  [ "$(b3sum --no-names "$WORK/orch-connector")" = "$BLAKE3" ] ||
    die "the download does not have the BLAKE3 hash the manifest lists: it was damaged or changed on the way"
fi
say "  checked: $SIZE bytes, SHA-256 as listed"

# Placed beside and renamed over, so that a running connector keeps the file it started from.
mkdir -p "$BIN_DIR"
if [ -f "$BIN" ] && [ "$(sha256 "$BIN")" = "$SHA256" ]; then
  say "The connector $VERSION is in $BIN already"
else
  [ -e "$BIN" ] || PLACED_BIN=1
  cp "$WORK/orch-connector" "$BIN_DIR/.orch-connector.new"
  chmod 755 "$BIN_DIR/.orch-connector.new"
  mv -f "$BIN_DIR/.orch-connector.new" "$BIN"
  say "Placed the connector $VERSION in $BIN"
fi

# A supplied token is always checked; an Enroll again token retains the existing machine.
connector() { "$BIN" --data-dir "$DATA_DIR" "$@"; }
record_installation() {
  set -- __record-installation "$@"
  [ "$NAMED_DATA_DIR" = 0 ] || set -- "$@" --named
  connector "$@"
}
[ -d "$DATA_DIR" ] || MADE_DATA_DIR=1
set -- enroll --url "$URL" --token "$TOKEN"
[ -z "$NAME" ] || set -- "$@" --name "$NAME"
[ -z "$ROLES" ] || set -- "$@" --roles "$ROLES"
[ -z "$DELEGATE_SECTIONS" ] || set -- "$@" --delegate "$DELEGATE_SECTIONS"
[ "$DELEGATE" = 0 ] || set -- "$@" --delegate-policy
[ "$DEDICATED" = 0 ] || set -- "$@" --dedicated
connector "$@" || die "enrolling did not succeed; use Enroll again on this machine's page for an existing identity"
MADE_DATA_DIR=0
[ "$DEDICATED" = 0 ] || connector dedicated true
# An enrolled machine keeps its program, whatever happens next.
PLACED_BIN=0
if [ "$DELEGATE" = 1 ]; then
  connector policy delegate all > /dev/null
  say "Every limit of this machine can now be changed from Orch"
fi
PENDING_SECTIONS=$DELEGATE_SECTIONS
while [ -n "$PENDING_SECTIONS" ]; do
  SECTION=${PENDING_SECTIONS%%,*}
  case "$PENDING_SECTIONS" in *,*) PENDING_SECTIONS=${PENDING_SECTIONS#*,} ;; *) PENDING_SECTIONS="" ;; esac
  case "$SECTION" in
    environment|agent|provider) SECTION="$SECTION.*" ;;
    environment.\*|agent.\*|provider.\*|\*) ;;
    all) SECTION="*" ;;
    *) die "--delegate names environment, agent or provider, or all" ;;
  esac
  connector policy delegate "$SECTION" > /dev/null
done

record_installation

# The service's name: one for the platform's own data directory, and one for each other directory.
SUFFIX=""
if [ "$NAMED_DATA_DIR" = 1 ]; then
  printf '%s' "$DATA_DIR" > "$WORK/data-dir"
  SUFFIX=$(sha256 "$WORK/data-dir" | cut -c1-8)
fi

# systemd_word TEXT: TEXT as one word of a unit's command line.
systemd_word() {
  printf '"%s"' "$(printf '%s' "$1" | sed -e 's/\\/\\\\/g' -e 's/"/\\"/g' -e 's/%/%%/g' -e 's/\$/$$/g')"
}
# xml_text TEXT: TEXT inside an XML element.
xml_text() {
  printf '%s' "$1" | sed -e 's/&/\&amp;/g' -e 's/</\&lt;/g' -e 's/>/\&gt;/g'
}

install_systemd_unit() {
  unit="orch-connector${SUFFIX:+-$SUFFIX}.service"
  unit_dir="${XDG_CONFIG_HOME:-$HOME/.config}/systemd/user"
  if ! command -v systemctl > /dev/null 2>&1 || ! systemctl --user show-environment > /dev/null 2>&1; then
    say "No systemd user manager is reachable here, so no service was installed."
    say "Start the connector yourself: $BIN --data-dir '$DATA_DIR' run"
    return 0
  fi
  mkdir -p "$unit_dir"
  [ -e "$unit_dir/$unit" ] || PLACED_SERVICE="$unit_dir/$unit"
  {
    printf '[Unit]\nDescription=Orch connector (%s)\nAfter=network-online.target\nWants=network-online.target\n\n' "$(printf '%s' "$DATA_DIR" | sed 's/%/%%/g')"
    printf '[Service]\nExecStart=%s --data-dir %s run\nRestart=on-failure\nRestartSec=10\n\n' "$(systemd_word "$BIN")" "$(systemd_word "$DATA_DIR")"
    printf '[Install]\nWantedBy=default.target\n'
  } > "$unit_dir/$unit"
  record_installation --service
  systemctl --user daemon-reload
  systemctl --user enable "$unit" > /dev/null 2>&1
  systemctl --user restart "$unit"
  PLACED_SERVICE=""
  say "The connector runs as the systemd user unit $unit"
  if command -v loginctl > /dev/null 2>&1 && [ "$(loginctl show-user "$(id -un)" -p Linger --value 2> /dev/null)" != yes ]; then
    say "  It stops when you log out. To keep it running: loginctl enable-linger $(id -un)"
  fi
}

install_launchd_agent() {
  label="dev.orch.connector${SUFFIX:+.$SUFFIX}"
  agents="$HOME/Library/LaunchAgents"
  plist="$agents/$label.plist"
  mkdir -p "$agents" "$DATA_DIR/logs"
  [ -e "$plist" ] || PLACED_SERVICE="$plist"
  cat > "$plist" <<EOF
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>Label</key><string>$(xml_text "$label")</string>
  <key>ProgramArguments</key>
  <array>
    <string>$(xml_text "$BIN")</string>
    <string>--data-dir</string>
    <string>$(xml_text "$DATA_DIR")</string>
    <string>run</string>
  </array>
  <key>RunAtLoad</key><true/>
  <key>KeepAlive</key><dict><key>SuccessfulExit</key><false/></dict>
  <key>ThrottleInterval</key><integer>10</integer>
  <key>EnvironmentVariables</key><dict>
    <key>ORCH_CONNECTOR_LOG_FILES_ONLY</key><string>1</string>
  </dict>
  <key>StandardErrorPath</key><string>$(xml_text "$DATA_DIR/logs/launchd.log")</string>
</dict>
</plist>
EOF
  domain="gui/$(id -u)"
  launchctl bootout "$domain/$label" > /dev/null 2>&1 || true
  record_installation --service
  launchctl bootstrap "$domain" "$plist"
  PLACED_SERVICE=""
  say "The connector runs as the launchd agent $label"
}

if [ "$SERVICE" = 1 ]; then
  if [ "$OS" = macos ]; then install_launchd_agent; else install_systemd_unit; fi
else
  say "No service was installed (--no-service). Start the connector with: $BIN --data-dir '$DATA_DIR' run"
fi

case ":${PATH:-}:" in
  *":$BIN_DIR:"*) ;;
  *) say "$BIN_DIR is not on your PATH; add it to run orch-connector by name" ;;
esac
say "Done: this machine is connected to $URL. Its page in Orch shows it within a minute."
