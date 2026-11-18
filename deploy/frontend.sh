#!/bin/sh
# The same web edge serves HTTP and optional HTTPS; a renewed pair is adopted by graceful reload.
set -eu
# A short look adopts a new complete pair promptly; it does no network work.
CERTIFICATE_LOOK_SECONDS=1
main=/tmp/frontend-https.conf
ports=/tmp/ports-https.conf
: > "$main"
: > "$ports"
generation_map() {
  # Only an intrinsic generation name enters nginx syntax; it never comes from a request.
  case "$1" in *[!a-zA-Z0-9-]*) return 1;; esac
  printf 'map "" $orch_frontend_generation { default "%s"; }\n' "$1" > /tmp/frontend-generation-next.conf
  mv /tmp/frontend-generation-next.conf /tmp/frontend-generation.conf
}
generation_map ''
if [ -n "${ORCH_FRONTEND_HTTPS_BIND:-}" ]; then
  test -r /etc/orch/frontend-tls/current/chain.pem
  test -r /etc/orch/frontend-tls/current/key.pem
  initial=$(readlink /etc/orch/frontend-tls/current)
  generation_map "$initial"
  for target in "$main" "$ports"; do
    if [ "$target" = "$main" ]; then
      # HTTP/2 for the application and its API (one connection carries the scripts and every read). A program's own address,
      # in `ports`, keeps HTTP/1.1: its pages and sockets are the program's.
      printf 'listen 443 ssl default_server;\nhttp2 on;\n' > "$target"
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
    previous=$initial
    while sleep "$CERTIFICATE_LOOK_SECONDS"; do
      current=$(readlink /etc/orch/frontend-tls/current)
      if [ "$current" != "$previous" ]; then
        loaded=$(wget -T "$CERTIFICATE_LOOK_SECONDS" -qO- http://127.0.0.1:8081/frontend-generation 2>/dev/null || true)
        if [ "$loaded" = "$current" ]; then
          previous=$current
          continue
        fi
        if generation_map "$current" && nginx -t && nginx -s reload; then
          # Sending the signal does not prove that new workers started. The next look checks them.
          :
        else
          # Old workers keep the old complete pair. Retry the same candidate on the next look.
          printf '%s\n' 'The prepared frontend certificate could not be loaded' >&2
        fi
      fi
    done
  ) &
fi
exec nginx -g 'daemon off;'
