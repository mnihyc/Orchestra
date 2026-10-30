#!/usr/bin/env bash
# Runs in a unit, as the unit's user: the browser the image holds and the command line that drives it. Arguments: a scratch directory
# (inside the unit), the version of the browser the image says it holds, and its revision. One line for each thing, DONE at the end.
set -u
WORK="${1:?a scratch directory}"
BROWSER_VERSION="${2:?the version of the browser}"
BROWSER_REVISION="${3:?the revision of the browser}"
EVIDENCE="$WORK/evidence"
mkdir -p "$EVIDENCE" "$WORK/site" "$WORK/cli"

pass() { printf 'PASS  %s\n' "$*"; }
fail() { printf 'FAIL  %s\n' "$*"; }
info() { printf 'INFO  %s\n' "$*"; }

# The unit suppresses core files so a browser that aborts leaves no memory file in the workspace.
# directory. This script starts a browser that does, on purpose: it says what the unit allows and then allows none for itself.
info "core files in a unit: limit $(ulimit -c), written as '$(cat /proc/sys/kernel/core_pattern)' in the working directory of the process that crashed"
ulimit -c 0

# How long a browser may take to answer at its address after it is started: the longest the host waits for one.
START_SECONDS=40
# How long a browser may take to end after it is asked to quit through its protocol.
QUIT_SECONDS=10

CHROME=""
for candidate in /opt/ms-playwright/chromium-*/chrome-linux*/chrome; do CHROME="$candidate"; break; done
HTTP_PID=""
BROWSER_PID=""
cleanup() {
  playwright-cli kill-all > /dev/null 2>&1 || true
  if [ -n "$BROWSER_PID" ]; then kill "$BROWSER_PID" 2> /dev/null || true; fi
  if [ -n "$HTTP_PID" ]; then kill "$HTTP_PID" 2> /dev/null || true; fi
}
trap cleanup EXIT

# --- What is where -----------------------------------------------------------------------------------------------------------

if [ -x "$CHROME" ]; then pass "the browser is in the image: $CHROME"; else fail "no browser under /opt/ms-playwright"; echo DONE; exit 0; fi
case "$CHROME" in
  "/opt/ms-playwright/chromium-$BROWSER_REVISION/"*) pass "it is the revision the image says it holds ($BROWSER_REVISION)" ;;
  *) fail "the browser is $CHROME, not revision $BROWSER_REVISION" ;;
esac
for program in node npm playwright-cli "$CHROME"; do
  where="$(readlink -f "$(command -v "$program")")"
  case "$where" in
    /home/* | /workspace/*) fail "$program is in $where, on a volume that outlives the image" ;;
    *) pass "$(basename "$program") is in $where, outside the home and the workspace" ;;
  esac
done
found="$(find /home/agent /workspace -maxdepth 4 \( -name 'chromium*' -o -name 'chrome-linux*' -o -name 'node_modules' \) 2> /dev/null | head -n 3)"
if [ -z "$found" ]; then pass "nothing of the browser, Node.js or the command line is in the home or the workspace"; else fail "found on a volume: $found"; fi

expected="/opt/ms-playwright chromium 1"
shell_has="$PLAYWRIGHT_BROWSERS_PATH $PLAYWRIGHT_MCP_BROWSER $NO_UPDATE_NOTIFIER"
# shellcheck disable=SC2016 # the variables are the other shell's to expand
bare_has="$(env -i bash -lc 'echo "$PLAYWRIGHT_BROWSERS_PATH $PLAYWRIGHT_MCP_BROWSER $NO_UPDATE_NOTIFIER"')"
if [ "$shell_has" = "$expected" ]; then pass "the shell has the three variables of the image ($shell_has)"; else fail "the shell has '$shell_has'"; fi
if [ "$bare_has" = "$expected" ]; then pass "a login shell started with an empty environment gets them from /etc/profile.d"; else fail "a bare login shell has '$bare_has'"; fi
info "versions as the programs say: node $(node --version), npm $(npm --version), playwright-cli $(playwright-cli --version), $("$CHROME" --version | sed 's/ *$//')"

# --- The command line starts a browser of its own, with nothing configured ------------------------------------------------

cat > "$WORK/site/page.html" << 'EOF'
<!doctype html><meta charset="utf-8"><title>orch-smoke-page</title>
<style>body{font:28px sans-serif} p{margin:.3em 0}</style>
<p lang="zh-CN" id="zh">简体中文：你好，世界。今天天气很好。</p>
<p lang="zh-TW" id="tw">繁體中文：你好，世界。今天天氣很好。</p>
<p lang="ja" id="ja">日本語：こんにちは世界。直、骨、海。</p>
<p lang="ko" id="ko">한국어: 안녕하세요, 세계.</p>
<p id="emoji">Emoji: 😀 🚀 🇯🇵 ❤️</p>
<p id="other">Ελληνικά, Русский, עברית, العربية, ภาษาไทย, हिन्दी</p>
EOF
cat > "$WORK/fonts.js" << 'EOF'
async (page) => {
  const cdp = await page.context().newCDPSession(page);
  await cdp.send('DOM.enable'); await cdp.send('CSS.enable');
  const doc = await cdp.send('DOM.getDocument', { depth: -1 });
  const used = {};
  for (const id of ['zh', 'tw', 'ja', 'ko', 'emoji', 'other']) {
    const node = await cdp.send('DOM.querySelector', { nodeId: doc.root.nodeId, selector: '#' + id });
    const fonts = await cdp.send('CSS.getPlatformFontsForNode', { nodeId: node.nodeId });
    used[id] = fonts.fonts.map((font) => font.familyName);
  }
  return JSON.stringify(used);
}
EOF
HTTP_PORT="$(python3 -I -c 'import socket; s = socket.socket(); s.bind(("127.0.0.1", 0)); print(s.getsockname()[1])')"
python3 -I -m http.server "$HTTP_PORT" --bind 127.0.0.1 --directory "$WORK/site" > "$WORK/http.log" 2>&1 &
HTTP_PID=$!
for _ in $(seq 1 20); do curl -fsS --max-time 2 -o /dev/null "http://127.0.0.1:$HTTP_PORT/page.html" 2> /dev/null && break; sleep 0.25; done
PAGE="http://127.0.0.1:$HTTP_PORT/page.html"

cd "$WORK/cli" || exit 1
opened="$(timeout 120 playwright-cli open "$PAGE" 2>&1)"; status=$?
if [ "$status" = 0 ] && printf '%s' "$opened" | grep -q 'Page Title: orch-smoke-page'; then
  pass "playwright-cli open starts a browser and opens a page with nothing configured"
else
  fail "playwright-cli open (exit $status): $(printf '%s' "$opened" | head -n 6)"
fi
listed="$(playwright-cli list 2>&1)"
if printf '%s' "$listed" | grep -q 'browser-type: chrome-for-testing' && printf '%s' "$listed" | grep -q 'headed: false'; then
  pass "it is the browser of the image (Chrome for Testing), without a window"
else
  fail "the session is not the browser of the image: $(printf '%s' "$listed" | tr '\n' ' ')"
fi
fonts_said="$(playwright-cli run-code --filename="$WORK/fonts.js" 2>&1)"
python3 -I - "$fonts_said" << 'EOF'
import json, sys
# The result is the line after "### Result": a JSON string that itself holds JSON.
lines = sys.argv[1].splitlines()
try:
    used = json.loads(json.loads(lines[lines.index("### Result") + 1]))
except (ValueError, IndexError):
    print("FAIL  the fonts the page was drawn with could not be read: %s" % " ".join(lines[:6]))
    sys.exit(0)
expected = {
    "zh": "Noto Sans CJK SC",   # simplified Chinese
    "tw": "Noto Sans CJK TC",   # traditional Chinese
    "ja": "Noto Sans CJK JP",   # Japanese
    "ko": "Noto Sans CJK KR",   # Korean
    "emoji": "Noto Color Emoji",
}
for key, family in expected.items():
    if family in used.get(key, []):
        print("PASS  %-5s is drawn with %s" % (key, family))
    else:
        print("FAIL  %-5s is drawn with %s, not %s" % (key, used.get(key), family))
print("INFO  other scripts: %s" % ", ".join(sorted(set(used.get("other", [])))))
EOF
playwright-cli screenshot --filename="$EVIDENCE/fonts.png" > /dev/null 2>&1
if [ -s "$EVIDENCE/fonts.png" ]; then pass "a picture of the page in Chinese, Japanese, Korean and with emoji was taken (evidence/fonts.png)"; else fail "no screenshot of the page"; fi
playwright-cli close > /dev/null 2>&1

control="$(env -u PLAYWRIGHT_MCP_BROWSER timeout 60 playwright-cli -s=control open about:blank 2>&1)"; status=$?
if [ "$status" != 0 ] && printf '%s' "$control" | grep -q '/opt/google/chrome/chrome'; then
  pass "without PLAYWRIGHT_MCP_BROWSER the command line asks for Google Chrome and finds none: the variable is what settles the default"
else
  fail "without the variable the command line did not ask for Google Chrome (exit $status): $(printf '%s' "$control" | head -n 3)"
fi
playwright-cli kill-all > /dev/null 2>&1 || true

# --- The full browser, started as Orch starts one: a new profile, an address for driving, no window --------------------------------

# start_browser PROFILE ARGS...: starts the browser in the background with Orch's own arguments, then the given ones; its output is PROFILE.log.
start_browser() {
  local profile="$1"; shift
  mkdir -p "$profile"
  "$CHROME" --user-data-dir="$profile" --remote-debugging-port=0 --no-first-run --no-default-browser-check \
    --window-size=1280,800 --disable-background-timer-throttling --disable-backgrounding-occluded-windows --disable-renderer-backgrounding \
    "$@" about:blank > "$profile.log" 2>&1 &
  BROWSER_PID=$!
}

# wait_endpoint PROFILE: waits until the browser's address answers, or the browser ends, or the time is up. Sets ENDPOINT_PORT (empty when it
# did not answer) and TOOK (the seconds it waited). The jobs of the shell that end in a signal are not for this script's output.
wait_endpoint() {
  local profile="$1" waited=0
  ENDPOINT_PORT=""
  while [ "$waited" -lt $((START_SECONDS * 4)) ]; do
    if [ -s "$profile/DevToolsActivePort" ]; then
      ENDPOINT_PORT="$(head -n 1 "$profile/DevToolsActivePort")"
      if curl -fsS --max-time 2 -o /dev/null "http://127.0.0.1:$ENDPOINT_PORT/json/version" 2> /dev/null; then break; fi
      ENDPOINT_PORT=""
    fi
    if ! kill -0 "$BROWSER_PID" 2> /dev/null; then break; fi
    sleep 0.25; waited=$((waited + 1))
  done
  TOOK=$((waited / 4))
} 2> /dev/null

PROFILE="$WORK/profile"
SANDBOX_FLAG=""
start_browser "$PROFILE" --headless
wait_endpoint "$PROFILE"; PORT="$ENDPOINT_PORT"
if [ -n "$PORT" ]; then
  SANDBOX=yes
  pass "the full browser starts with its own sandbox (answered at 127.0.0.1:$PORT after about $TOOK s)"
else
  { wait "$BROWSER_PID"; } 2> /dev/null; ended=$?
  BROWSER_PID=""
  printed="$(cat "$PROFILE.log")"
  if printf '%s' "$printed" | grep -q 'No usable sandbox'; then
    SANDBOX=no
    pass "asked for its sandbox it cannot have one here, says so and ends (exit $ended) instead of hanging or failing otherwise"
    info "what it printed: $(printf '%s' "$printed" | grep 'No usable sandbox' | head -n 1 | cut -c1-230)"
  else
    SANDBOX=unknown
    fail "the browser did not come up and did not name the sandbox (exit $ended): $(printf '%s' "$printed" | head -n 5)"
  fi
  SANDBOX_FLAG="--no-sandbox"
  start_browser "$PROFILE" --headless --no-sandbox
  wait_endpoint "$PROFILE"; PORT="$ENDPOINT_PORT"
  if [ -n "$PORT" ]; then pass "started once more on the same profile with --no-sandbox it answers at 127.0.0.1:$PORT (after about $TOOK s)"; else fail "no answer at the browser's address with --no-sandbox: $(head -n 5 "$PROFILE.log")"; fi
fi
info "SANDBOX of the full browser as uid $(id -u) in this unit: $SANDBOX"

if [ -n "$PORT" ]; then
  version_json="$(curl -fsS --max-time 5 "http://127.0.0.1:$PORT/json/version" 2> /dev/null)"
  python3 -I - "$version_json" "$PROFILE/DevToolsActivePort" "$BROWSER_VERSION" << 'EOF'
import json, sys
answer, port_file, version = json.loads(sys.argv[1]), open(sys.argv[2]).read().split("\n"), sys.argv[3]
if answer.get("Browser") == "Chrome/" + version:
    print("PASS  the browser says it is Chrome/%s, the version the image holds" % version)
else:
    print("FAIL  the browser says %r, not Chrome/%s" % (answer.get("Browser"), version))
if "HeadlessChrome" in answer.get("User-Agent", ""):
    print("PASS  it says of itself that it has no window (HeadlessChrome in its user agent)")
else:
    print("FAIL  its user agent does not say it has no window: %s" % answer.get("User-Agent"))
# The file the browser writes into its profile is how the port is learned: the port, then the path of the browser's socket.
if len(port_file) >= 2 and port_file[0].isdigit() and port_file[1].startswith("/devtools/browser/") and answer.get("webSocketDebuggerUrl", "").endswith(port_file[1]):
    print("PASS  DevToolsActivePort in the profile names the port (%s) and the browser's socket" % port_file[0])
else:
    print("FAIL  DevToolsActivePort is not the port and the socket path: %r" % port_file)
EOF

  attached="$(timeout 90 playwright-cli -s=attached attach --cdp="http://127.0.0.1:$PORT" 2>&1)"; status=$?
  if [ "$status" = 0 ]; then pass "playwright-cli attach --cdp drives the browser that was started by hand"; else fail "attach (exit $status): $(printf '%s' "$attached" | head -n 6)"; fi
  went="$(timeout 60 playwright-cli -s=attached goto "$PAGE" 2>&1)"
  if printf '%s' "$went" | grep -q 'Page Title: orch-smoke-page'; then pass "and navigates a tab of it (goto) and reads the page's title"; else fail "goto through the attached session: $(printf '%s' "$went" | head -n 6)"; fi
  if curl -fsS --max-time 5 "http://127.0.0.1:$PORT/json/list" 2> /dev/null | grep -q 'page.html'; then
    pass "that tab is the browser's own: its address lists it at the same port"
  else
    fail "the tab the command line navigated is not in the browser's list"
  fi
  tabs="$(timeout 30 playwright-cli -s=attached tab-list 2>&1)"
  if printf '%s' "$tabs" | grep -q 'orch-smoke-page'; then pass "tab-list shows it"; else fail "tab-list does not show the page: $(printf '%s' "$tabs" | head -n 4)"; fi
  playwright-cli -s=attached detach > /dev/null 2>&1
  if kill -0 "$BROWSER_PID" 2> /dev/null && curl -fsS --max-time 5 -o /dev/null "http://127.0.0.1:$PORT/json/version" 2> /dev/null; then
    pass "detaching leaves the browser running, for whoever else is using it"
  else
    fail "the browser did not survive the command line's detaching"
  fi

  # One profile has one running browser: a second full browser on it is refused, and the first is not disturbed.
  second="$(timeout 60 "$CHROME" --headless --user-data-dir="$PROFILE" --remote-debugging-port=0 --no-first-run --no-default-browser-check $SANDBOX_FLAG about:blank 2>&1)"; status=$?
  if [ "$status" != 0 ] && printf '%s' "$second" | grep -q 'ProcessSingleton'; then
    pass "a second browser on the same profile is refused (exit $status: $(printf '%s' "$second" | grep 'ProcessSingleton' | head -n 1 | cut -c1-160))"
  else
    fail "a second browser on the profile was not refused (exit $status): $(printf '%s' "$second" | head -n 4)"
  fi
  if curl -fsS --max-time 5 -o /dev/null "http://127.0.0.1:$PORT/json/version" 2> /dev/null; then pass "the first browser still answers"; else fail "the first browser stopped answering"; fi

  # It ends when it is asked through its own protocol, and then the profile is free again.
  socket_url="$(printf '%s' "$version_json" | python3 -I -c 'import json, sys; print(json.load(sys.stdin)["webSocketDebuggerUrl"])')"
  node -e '
    const socket = new WebSocket(process.argv[1]);
    socket.onopen = () => socket.send(JSON.stringify({ id: 1, method: "Browser.close" }));
    socket.onclose = () => process.exit(0);
    socket.onerror = () => process.exit(1);
    setTimeout(() => process.exit(2), 8000);
  ' "$socket_url"
  for _ in $(seq 1 $((QUIT_SECONDS * 2))); do kill -0 "$BROWSER_PID" 2> /dev/null || break; sleep 0.5; done
  if kill -0 "$BROWSER_PID" 2> /dev/null; then fail "the browser did not end within $QUIT_SECONDS s of being asked through its protocol"; else pass "asked to quit through its protocol it ends"; fi
  BROWSER_PID=""
  start_browser "$PROFILE" --headless $SANDBOX_FLAG
  wait_endpoint "$PROFILE"
  if [ -n "$ENDPOINT_PORT" ]; then pass "the profile is free afterwards: a new browser starts on it"; else fail "a browser did not start on the profile after the first one ended"; fi
  kill "$BROWSER_PID" 2> /dev/null; { wait "$BROWSER_PID"; } 2> /dev/null; BROWSER_PID=""
fi
echo DONE
