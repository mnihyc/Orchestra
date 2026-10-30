#!/bin/sh
# The same web edge serves HTTP and optional HTTPS; a renewed pair is adopted by graceful reload.
set -eu
# A short look adopts a new complete pair promptly; it does no network work.
CERTIFICATE_LOOK_SECONDS=1
main=/tmp/frontend-https.conf
ports=/tmp/ports-https.conf
: > "$main"
: > "$ports"
if [ -n "${ORCH_FRONTEND_HTTPS_BIND:-}" ]; then
  test -r /etc/orch/frontend-tls/current/chain.pem
  test -r /etc/orch/frontend-tls/current/key.pem
  for target in "$main" "$ports"; do
    if [ "$target" = "$main" ]; then
      printf 'listen 443 ssl default_server;\n' > "$target"
    else
      printf 'listen 443 ssl;\n' > "$target"
    fi
    cat >> "$target" <<'TLS'
ssl_certificate /etc/orch/frontend-tls/current/chain.pem;
ssl_certificate_key /etc/orch/frontend-tls/current/key.pem;
ssl_protocols TLSv1.2 TLSv1.3;
TLS
  done
  # Each generation is a complete pair. Reload preserves existing sockets and uses the new pair for new ones.
  (
    previous=$(readlink /etc/orch/frontend-tls/current)
    while sleep "$CERTIFICATE_LOOK_SECONDS"; do
      current=$(readlink /etc/orch/frontend-tls/current)
      if [ "$current" != "$previous" ]; then
        nginx -s reload
        previous=$current
      fi
    done
  ) &
fi
exec nginx -g 'daemon off;'
