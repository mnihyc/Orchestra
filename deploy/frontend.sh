#!/bin/sh
# The same web edge serves HTTP and optional HTTPS; a renewed pair is adopted by graceful reload.
set -eu
main=/tmp/frontend-https.conf
ports=/tmp/ports-https.conf
policy=/etc/orch/frontend-tls/transport.include
: > "$main"
: > "$ports"
if [ -n "${ORCH_FRONTEND_HTTPS_BIND:-}" ]; then
  test -r /etc/orch/frontend-tls/current/chain.pem
  test -r /etc/orch/frontend-tls/current/key.pem
  test -r "$policy"
  for target in "$main" "$ports"; do
    if [ "$target" = "$main" ]; then
      # HTTP/2 for the application and its API (one connection carries the scripts and every read). A program's own address,
      # in `ports`, keeps HTTP/1.1: its pages and sockets are the program's.
      printf 'listen 443 ssl default_server;\ninclude /etc/orch/frontend-tls/transport.include;\n' > "$target"
    else
      printf 'listen 443 ssl;\n' > "$target"
    fi
    cat >> "$target" <<'TLS'
ssl_certificate /etc/orch/frontend-tls/current/chain.pem;
ssl_certificate_key /etc/orch/frontend-tls/current/key.pem;
ssl_protocols TLSv1.2 TLSv1.3;
TLS
  done
  # Complete certificate pairs and committed transport policies reload together; existing sockets finish normally.
  (
    previous="$(readlink /etc/orch/frontend-tls/current):$(cksum "$policy")"
    while :; do
      interval=$(sed -n 's/^# reload_secs=//p' "$policy")
      case "$interval" in ''|0|*[!0-9]*) echo 'The HTTPS policy has no positive reload interval.' >&2; exit 1;; esac
      sleep "$interval"
      current="$(readlink /etc/orch/frontend-tls/current):$(cksum "$policy")"
      if [ "$current" != "$previous" ]; then
        if nginx -t && nginx -s reload; then previous=$current; fi
      fi
    done
  ) &
fi
exec nginx -g 'daemon off;'
