#!/usr/bin/env bash
# Runs in a unit whose container allows what the browser's own sandbox needs (the kernel's user namespaces, which Docker's default profile
# refuses), as the unit's user. Argument: a scratch directory inside the unit. The full browser is started without --no-sandbox: a browser
# that cannot have its sandbox ends instead of answering, so that it answers is the proof that it has one. DONE at the end.
set -u
WORK="${1:?a scratch directory}"
mkdir -p "$WORK"

pass() { printf 'PASS  %s\n' "$*"; }
fail() { printf 'FAIL  %s\n' "$*"; }
info() { printf 'INFO  %s\n' "$*"; }

# How long the browser may take to answer at its address after it is started.
START_SECONDS=40

ulimit -c 0
CHROME=""
for candidate in /opt/ms-playwright/chromium-*/chrome-linux*/chrome; do CHROME="$candidate"; break; done
PROFILE="$WORK/profile-sandboxed"
mkdir -p "$PROFILE"
info "seccomp in this unit: $(awk '/^Seccomp:/ {print $2}' /proc/self/status) (0 is none, 2 is a filter), uid $(id -u)"
"$CHROME" --headless --user-data-dir="$PROFILE" --remote-debugging-port=0 --no-first-run --no-default-browser-check about:blank > "$PROFILE.log" 2>&1 &
BROWSER_PID=$!
answer=""
for _ in $(seq 1 $((START_SECONDS * 4))); do
  if [ -s "$PROFILE/DevToolsActivePort" ]; then
    answer="$(curl -fsS --max-time 2 "http://127.0.0.1:$(head -n 1 "$PROFILE/DevToolsActivePort")/json/version" 2> /dev/null)" && break
    answer=""
  fi
  kill -0 "$BROWSER_PID" 2> /dev/null || break
  sleep 0.25
done
if [ -n "$answer" ]; then
  pass "where the container allows user namespaces the browser starts without --no-sandbox, so it has its own sandbox"
else
  fail "the browser did not start without --no-sandbox where the container allows user namespaces: $(grep -v 'dbus\|GLib' "$PROFILE.log" | head -n 3 | cut -c1-200)"
fi
kill "$BROWSER_PID" 2> /dev/null
{ wait "$BROWSER_PID"; } 2> /dev/null
echo DONE
