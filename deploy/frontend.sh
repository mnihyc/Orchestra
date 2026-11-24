#!/bin/sh
# The same web edge serves HTTP and optional HTTPS; a renewed pair is adopted by graceful reload.
set -eu
main=/tmp/frontend-https.conf
ports=/tmp/ports-https.conf
policy=/etc/orch/frontend-tls/transport.include
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
    previous="$initial:$(cksum "$policy")"
    signaled=
    while :; do
      interval=$(sed -n 's/^# reload_secs=//p' "$policy")
      case "$interval" in ''|0|*[!0-9]*) echo 'The HTTPS policy has no positive reload interval.' >&2; exit 1;; esac
      sleep "$interval"
      generation=$(readlink /etc/orch/frontend-tls/current)
      current="$generation:$(cksum "$policy")"
      if [ "$current" != "$previous" ]; then
        loaded=$(wget -T "$interval" -qO- http://127.0.0.1:8081/frontend-generation 2>/dev/null || true)
        if [ "$signaled" = "$current" ] && [ "$loaded" = "$generation" ]; then
          previous=$current
          continue
        fi
        if generation_map "$generation" && nginx -t && nginx -s reload; then
          # Sending the signal does not prove that new workers started. The next look checks them.
          signaled=$current
        else
          # Old workers keep the old complete pair. Retry the same candidate on the next look.
          printf '%s\n' 'The prepared frontend certificate could not be loaded' >&2
        fi
      fi
    done
  ) &
fi
# The content policy's room for a human check's service, which the web edge includes; empty where none is chosen.
sh /etc/orch/frontend-csp.sh "${ORCH_FRONTEND_HUMAN_CHECK:-}" > /tmp/frontend-csp.conf
exec nginx -g 'daemon off;'
