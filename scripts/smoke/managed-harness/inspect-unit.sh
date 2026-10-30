#!/bin/sh
# Run inside a unit (`docker exec -i UNIT sh -s -- GATEWAY_IP:PORT < inspect-unit.sh`): says how the harnesses there were
# started and where they have written, one fact to a line, for the scenario to check. It reads /proc and a few files
# and changes nothing.
#
#   claude-args: / codex-args:     the command line of the harness process
#   claude-env: / codex-env:       the variables of its environment that the adapters' rules are about
#   codex-config:                  a line of the configuration file in Codex's home (the home is the process's CODEX_HOME)
#   home-entry:                    anything at all in the account's home directory
#   outside-connection:            a TCP connection that is neither on the loopback nor to the gateway
gateway="${1:-}"

# Variables a harness is told in its environment, by the rules of its adapter; nothing else is printed (the
# environment holds tokens).
claude_variables='DISABLE_TELEMETRY|DISABLE_ERROR_REPORTING|DISABLE_FEEDBACK_COMMAND|DISABLE_UPDATES|CLAUDE_CODE_DISABLE_NONESSENTIAL_TRAFFIC|CLAUDE_CODE_DISABLE_FEEDBACK_SURVEY|CLAUDE_CODE_DISABLE_OFFICIAL_MARKETPLACE_AUTOINSTALL|CLAUDE_CONFIG_DIR|CLAUDE_CODE_PROJECT_DIR_NAME|CLAUDE_CODE_DISABLE_AUTO_MEMORY|ANTHROPIC_BASE_URL'
codex_variables='CODEX_HOME'

codex_home=""
for proc in /proc/[0-9]*; do
  [ -r "$proc/cmdline" ] || continue
  args="$(tr '\0' ' ' < "$proc/cmdline" 2> /dev/null)"
  program="${args%% *}"
  case "${program##*/}" in
    claude-code | claude)
      echo "claude-args: $args"
      tr '\0' '\n' < "$proc/environ" 2> /dev/null | grep -E "^($claude_variables)=" | sed 's/^/claude-env: /'
      ;;
    codex)
      case "$args" in
        *app-server*)
          echo "codex-args: $args"
          tr '\0' '\n' < "$proc/environ" 2> /dev/null | grep -E "^($codex_variables)=" | sed 's/^/codex-env: /'
          codex_home="$(tr '\0' '\n' < "$proc/environ" 2> /dev/null | sed -n 's/^CODEX_HOME=//p' | head -n 1)"
          ;;
      esac
      ;;
  esac
done

if [ -n "$codex_home" ] && [ -r "$codex_home/config.toml" ]; then
  sed -e 's/^[[:space:]]*//' -e 's/[[:space:]]*$//' -e '/^$/d' "$codex_home/config.toml" | sed 's/^/codex-config: /'
fi

# The home directory of the account is for the person's own files: a harness started by the connector keeps what it
# keeps in the home the connector gave it.
if [ -d "${HOME:-/home/agent}" ]; then
  find "${HOME:-/home/agent}" -mindepth 1 -maxdepth 2 2> /dev/null | sed 's/^/home-entry: /'
fi

# Established TCP connections (state 01) of the whole unit. IPv4 addresses are written in /proc as little-endian hex.
for table in /proc/net/tcp /proc/net/tcp6; do
  [ -r "$table" ] || continue
  awk -v gateway="$gateway" '
    function hex(text,   i, value) {
      value = 0
      text = tolower(text)
      for (i = 1; i <= length(text); i++) value = value * 16 + index("0123456789abcdef", substr(text, i, 1)) - 1
      return value
    }
    NR > 1 && $4 == "01" {
      split($3, remote, ":")
      address = remote[1]
      if (length(address) == 8) {
        text = sprintf("%d.%d.%d.%d:%d", hex(substr(address, 7, 2)), hex(substr(address, 5, 2)),
          hex(substr(address, 3, 2)), hex(substr(address, 1, 2)), hex(remote[2]))
      } else {
        text = "[" address "]:" remote[2]
      }
      if (text ~ /^127\./ || text == gateway || address == "00000000000000000000000001000000" || address ~ /^0000000000000000FFFF00000100007F/) next
      print "outside-connection: " text
    }
  ' "$table"
done
