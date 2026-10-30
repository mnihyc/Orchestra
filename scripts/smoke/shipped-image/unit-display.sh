#!/usr/bin/env bash
# Runs in a unit, as the unit's user: a display, started as Orch starts one. Arguments: a scratch directory (inside the unit, holding
# rfb.py), the version of TigerVNC and of openbox the image says it holds. One line for each thing, DONE at the end.
set -u
WORK="${1:?a scratch directory}"
TIGERVNC_VERSION="${2:?the version of TigerVNC}"
OPENBOX_VERSION="${3:?the version of openbox}"
RFB="$WORK/rfb.py"
EVIDENCE="$WORK/evidence"
mkdir -p "$EVIDENCE"

pass() { printf 'PASS  %s\n' "$*"; }
fail() { printf 'FAIL  %s\n' "$*"; }
info() { printf 'INFO  %s\n' "$*"; }

# A browser that cannot have its sandbox ends with a signal and, in a unit, leaves a core file: this script asks for none.
ulimit -c 0

# How long a server may take to listen after it is started.
LISTEN_SECONDS=20
# How long a window may take to show on a display after its browser is started: a browser on a loaded machine is slow.
WINDOW_SECONDS=40
# The size of a display unless it is asked for another.
GEOMETRY=1280x800

XVNC_PID=""
CONTROL_PID=""
WM_PID=""
BROWSER_PID=""
cleanup() {
  for pid in "$BROWSER_PID" "$WM_PID" "$CONTROL_PID" "$XVNC_PID"; do
    if [ -n "$pid" ]; then kill "$pid" 2> /dev/null || true; fi
  done
}
trap cleanup EXIT

# free_display: the number of the first X display that has neither a socket nor a lock, as the host finds the next free one.
free_display() {
  local n
  for n in $(seq 1 99); do
    if [ ! -e "/tmp/.X11-unix/X$n" ] && [ ! -e "/tmp/.X$n-lock" ]; then printf '%s' "$n"; return 0; fi
  done
  return 1
}
# free_port: a port of the loopback address that nothing listens on.
free_port() { python3 -I -c 'import socket; s = socket.socket(); s.bind(("127.0.0.1", 0)); print(s.getsockname()[1])'; }
# listens PORT: whether something accepts a connection on the loopback address at that port.
listens() { python3 -I -c 'import socket, sys; s = socket.socket(); s.settimeout(2); sys.exit(s.connect_ex(("127.0.0.1", int(sys.argv[1]))))' "$1" 2> /dev/null; }
# wait_listening PORT PID: waits until the port accepts connections, or the server ends, or the time is up.
wait_listening() {
  local waited=0
  while [ "$waited" -lt $((LISTEN_SECONDS * 4)) ]; do
    if listens "$1"; then return 0; fi
    kill -0 "$2" 2> /dev/null || return 1
    sleep 0.25; waited=$((waited + 1))
  done
  return 1
}
# field NAME TEXT: the value of a key=value line.
field() { printf '%s\n' "$2" | sed -n "s/^$1=//p" | head -n 1; }

# --- The server -----------------------------------------------------------------------------------------------------------------

version_said="$(Xvnc -version 2>&1 | sed -n 's/^Xvnc TigerVNC \([0-9.]*\) .*/\1/p')"
if [ "$version_said" = "$TIGERVNC_VERSION" ]; then pass "Xvnc is TigerVNC $version_said, the version the image holds ($(command -v Xvnc))"; else fail "Xvnc says '$version_said', not $TIGERVNC_VERSION"; fi

DISPLAY_NUMBER="$(free_display)"
PORT="$(free_port)"
# The options a display is started with: the size and depth asked for, the loopback address only, the screen shared among its viewers, no
# password of its own, and nothing copied in it sent to a viewer.
Xvnc ":$DISPLAY_NUMBER" -geometry "$GEOMETRY" -depth 24 -rfbport "$PORT" -localhost -AlwaysShared -SecurityTypes None -SendCutText=0 > "$WORK/xvnc.log" 2>&1 &
XVNC_PID=$!
if wait_listening "$PORT" "$XVNC_PID"; then
  pass "Xvnc starts on the free display :$DISPLAY_NUMBER and listens at 127.0.0.1:$PORT"
else
  fail "Xvnc did not listen: $(head -n 6 "$WORK/xvnc.log")"
  echo DONE; exit 0
fi
if grep -q 'euid != 0' "$WORK/xvnc.log"; then fail "Xvnc warned that it cannot make /tmp/.X11-unix: $(grep 'euid' "$WORK/xvnc.log")"; else pass "it needs nothing made for it: /tmp/.X11-unix is there ($(stat -c '%U %a' /tmp/.X11-unix))"; fi

# nothing but the loopback address: the socket is bound to 127.0.0.1, and the unit's own address is refused
hex_port="$(printf '%04X' "$PORT")"
if awk -v want="0100007F:$hex_port" '$2 == want && $4 == "0A" {found = 1} END {exit !found}' /proc/net/tcp; then pass "the socket is bound to 127.0.0.1 and nothing wider (-localhost)"; else fail "no listening socket on 127.0.0.1:$PORT in /proc/net/tcp"; fi
own="$(python3 -I -c 'import socket; print(socket.gethostbyname(socket.gethostname()))')"
case "$own" in
  127.*) info "the unit has no address of its own, so the refusal of connections from outside the loopback address was not tried" ;;
  *)
    if python3 -I -c 'import socket, sys; s = socket.socket(); s.settimeout(3); sys.exit(0 if s.connect_ex((sys.argv[1], int(sys.argv[2]))) != 0 else 1)' "$own" "$PORT"; then
      pass "a connection to the unit's own address ($own) is refused: only the unit's own programs can reach a display"
    else
      fail "the display answers at $own"
    fi
    ;;
esac

# --- A viewer -------------------------------------------------------------------------------------------------------------------

described="$(python3 -I "$RFB" info "$PORT" 2>&1)"
if [ "$(field version "$described")" = "RFB 003.008" ]; then pass "a client connects and speaks the protocol's version 3.8 ($(field name "$described"))"; else fail "no client could connect: $described"; fi
if [ "$(field security "$described")" = None ]; then pass "the only way it offers of being asked who is there is none: no password of its own (-SecurityTypes None)"; else fail "it offers '$(field security "$described")'"; fi
if [ "$(field size "$described")" = "$GEOMETRY" ] && [ "$(field depth "$described")" = 24 ]; then pass "the screen is $GEOMETRY, 24 bits deep"; else fail "the screen is $(field size "$described") at depth $(field depth "$described")"; fi

shared="$(python3 -I "$RFB" share "$PORT" 2>&1)"
if [ "$(field first_served_after "$shared")" = yes ] && [ "$(field second_served "$shared")" = yes ]; then
  pass "a second viewer that asks for the screen to itself does not take it from the first: both are served (-AlwaysShared)"
else
  fail "the viewers were not both served: $shared"
fi
CONTROL_PORT="$(free_port)"
CONTROL_DISPLAY="$(free_display)"
if [ "$CONTROL_DISPLAY" = "$DISPLAY_NUMBER" ]; then CONTROL_DISPLAY=$((DISPLAY_NUMBER + 1)); fi
Xvnc ":$CONTROL_DISPLAY" -geometry 640x480 -depth 24 -rfbport "$CONTROL_PORT" -localhost -SecurityTypes None > "$WORK/xvnc-control.log" 2>&1 &
CONTROL_PID=$!
if wait_listening "$CONTROL_PORT" "$CONTROL_PID"; then
  control="$(python3 -I "$RFB" share "$CONTROL_PORT" 2>&1)"
  if [ "$(field first_served_after "$control")" = no ]; then
    pass "control: a server started without -AlwaysShared does drop the first viewer, so the check above can tell"
  else
    fail "control: the first viewer was kept by a server that is not always shared: $control"
  fi
else
  fail "the control server did not start: $(head -n 4 "$WORK/xvnc-control.log")"
fi
kill "$CONTROL_PID" 2> /dev/null; { wait "$CONTROL_PID"; } 2> /dev/null; CONTROL_PID=""

# --- The window manager and a window --------------------------------------------------------------------------------------------

export DISPLAY=":$DISPLAY_NUMBER"
if [ "$(xdpyinfo 2> /dev/null | sed -n 's/^ *dimensions: *\([0-9x]*\) pixels.*/\1/p')" = "$GEOMETRY" ]; then pass "a program that opens a window finds the display by DISPLAY=$DISPLAY (xdpyinfo)"; else fail "xdpyinfo does not answer on $DISPLAY"; fi
before="$(python3 -I "$RFB" shot "$PORT" "$EVIDENCE/display-empty.png" 2>&1)"
openbox > "$WORK/openbox.log" 2>&1 &
WM_PID=$!
wm_name=""
for _ in $(seq 1 20); do
  check="$(xprop -root _NET_SUPPORTING_WM_CHECK 2> /dev/null | sed -n 's/.*window id # \(0x[0-9a-f]*\).*/\1/p')"
  if [ -n "$check" ]; then wm_name="$(xprop -id "$check" _NET_WM_NAME 2> /dev/null | sed -n 's/.*= "\(.*\)"/\1/p')"; break; fi
  sleep 0.25
done
if [ "$wm_name" = Openbox ]; then pass "openbox runs on the display as its window manager ($(openbox --version | sed -n 1p))"; else fail "no window manager of the name Openbox on the display (found '$wm_name'): $(head -n 3 "$WORK/openbox.log")"; fi
if [ "$(openbox --version | sed -n 1p)" = "Openbox $OPENBOX_VERSION" ]; then pass "it is the version the image holds"; else fail "openbox is '$(openbox --version | sed -n 1p)', not $OPENBOX_VERSION"; fi

CHROME=""
for candidate in /opt/ms-playwright/chromium-*/chrome-linux*/chrome; do CHROME="$candidate"; break; done
PROFILE="$WORK/profile-window"
TITLE=orch-smoke-window
PAGE="data:text/html;charset=utf-8,<title>$TITLE</title><body style=\"background:white;font:48px sans-serif\"><p>Window 窗口 ウィンドウ 창 😀</p>"
# start_window_browser FLAG: a browser with a window on the display, as the preset for one starts it.
start_window_browser() {
  mkdir -p "$PROFILE"
  "$CHROME" --user-data-dir="$PROFILE" --remote-debugging-port=0 --no-first-run --no-default-browser-check --window-size=1100,700 --window-position=40,40 \
    ${1:+"$1"} "$PAGE" > "$PROFILE.log" 2>&1 &
  BROWSER_PID=$!
}
# wait_window: waits for the browser's window to show on the display, or for the browser to end, or for the time to be up. Sets WINDOW_LINE to its
# line of xwininfo (empty when there was none). The jobs of the shell that end in a signal are not for this script's output.
wait_window() {
  local waited=0
  WINDOW_LINE=""
  while [ "$waited" -lt $((WINDOW_SECONDS * 2)) ]; do
    WINDOW_LINE="$(xwininfo -root -tree 2> /dev/null | grep "$TITLE" | head -n 1 | sed 's/^ *//')"
    if [ -n "$WINDOW_LINE" ]; then return 0; fi
    kill -0 "$BROWSER_PID" 2> /dev/null || return 1
    sleep 0.5; waited=$((waited + 1))
  done
  return 1
} 2> /dev/null
start_window_browser ""
wait_window
if [ -z "$WINDOW_LINE" ]; then
  # Where the unit cannot give the browser its sandbox it ends, and is started once more without, as for a browser without a window.
  if grep -q 'No usable sandbox' "$PROFILE.log"; then
    info "the browser with a window also needs --no-sandbox here, as the one without"
    { wait "$BROWSER_PID"; } 2> /dev/null
    start_window_browser "--no-sandbox"
    wait_window
  fi
fi
if [ -n "$WINDOW_LINE" ]; then
  pass "a browser with a window shows on the display: $WINDOW_LINE"
else
  fail "no window of the browser on the display: $(head -n 5 "$PROFILE.log")"
fi
if [ -s "$PROFILE/DevToolsActivePort" ] && curl -fsS --max-time 5 "http://127.0.0.1:$(head -n 1 "$PROFILE/DevToolsActivePort")/json/version" 2> /dev/null | grep -q '"Browser": "Chrome/'; then
  pass "that browser has an address for driving too, and does not call itself headless"
else
  fail "the browser with a window has no answering address"
fi
sleep 2
after="$(python3 -I "$RFB" shot "$PORT" "$EVIDENCE/display-window.png" 2>&1)"
if [ "$(field colours "$after")" -gt 50 ] 2> /dev/null && [ "$(field colours "$before")" -lt 10 ] 2> /dev/null; then
  pass "a viewer sees it: the screen had $(field colours "$before") colours before the window and has $(field colours "$after") with it (evidence/display-window.png)"
else
  fail "the screen did not change for a viewer: $(field colours "$before") colours before, $(field colours "$after") after"
fi

# --- It ends with nothing left behind ----------------------------------------------------------------------------------------------

kill "$BROWSER_PID" 2> /dev/null; { wait "$BROWSER_PID"; } 2> /dev/null; BROWSER_PID=""
kill "$WM_PID" 2> /dev/null; { wait "$WM_PID"; } 2> /dev/null; WM_PID=""
kill "$XVNC_PID" 2> /dev/null; { wait "$XVNC_PID"; } 2> /dev/null; XVNC_PID=""
if [ ! -e "/tmp/.X$DISPLAY_NUMBER-lock" ] && [ ! -e "/tmp/.X11-unix/X$DISPLAY_NUMBER" ]; then pass "stopped, the server leaves neither its lock nor its socket"; else fail "the server left its lock or its socket"; fi
if ! listens "$PORT"; then pass "and nothing listens at its port"; else fail "something still listens at $PORT"; fi
echo DONE
