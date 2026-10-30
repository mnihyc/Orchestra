#!/usr/bin/env bash
# Runs in a unit, as the unit's user, with one word: `given` for a unit made as the Default template makes one, `taken-away` for one
# whose template sets no_new_privileges. It says what sudo does, one line for each thing, and ends with DONE.
set -u
MODE="${1:?given or taken-away}"

pass() { printf 'PASS  %s\n' "$*"; }
fail() { printf 'FAIL  %s\n' "$*"; }
info() { printf 'INFO  %s\n' "$*"; }
skip() { printf 'SKIP  %s\n' "$*"; }

if [ "$(id -u)" = 1000 ]; then pass "the unit's user is the account the template names (uid 1000)"; else fail "the unit's user is uid $(id -u), not 1000"; fi
no_new_privs="$(awk '/^NoNewPrivs:/ {print $2}' /proc/self/status)"

if [ "$MODE" = given ]; then
  if [ "$no_new_privs" = 0 ]; then pass "the unit does not set no_new_privileges, as the Default template does not"; else fail "NoNewPrivs is $no_new_privs in a unit that should not set it"; fi
  if sudo -n true; then pass "sudo asks for no password"; else fail "sudo wants something: $(sudo -n true 2>&1 | head -n 2)"; fi
  if [ "$(sudo -n id -u 2> /dev/null)" = 0 ]; then pass "what sudo runs is root"; else fail "sudo does not give root"; fi
  if [ "$(id -u)" = 1000 ]; then pass "the user is still itself afterwards"; else fail "the user changed"; fi
  if sh -c 'echo x > /etc/orch-smoke-by-the-user' 2> /dev/null; then fail "the user can write in /etc without root"; else pass "without sudo the user cannot write in /etc"; fi
  sudo -n sh -c 'echo x > /etc/orch-smoke-by-root'
  if [ "$(stat -c %u /etc/orch-smoke-by-root 2> /dev/null)" = 0 ]; then pass "a file written through sudo in /etc belongs to root"; else fail "no root-owned file in /etc"; fi
  sudo -n rm -f /etc/orch-smoke-by-root
  root_home="$(sudo -n sh -c 'echo $HOME')"
  if [ "$root_home" = /root ]; then pass "root works in its own home, so a tool run as root does not leave root-owned files in the user's"; else fail "root's HOME is $root_home"; fi
  kept="$(sudo -n sh -c 'echo "$PLAYWRIGHT_BROWSERS_PATH $PLAYWRIGHT_MCP_BROWSER $NO_UPDATE_NOTIFIER"')"
  if [ "$kept" = "/opt/ms-playwright chromium 1" ]; then pass "root keeps the three variables that find the browser ($kept)"; else fail "root has '$kept' of the three variables"; fi
  if sudo -n -l | grep -q 'NOPASSWD: ALL'; then pass "sudo -l says what the image promises: everything, without a password"; else fail "sudo -l does not say NOPASSWD: ALL"; fi
  # Installing what the work needs is what it is for. It needs the Debian mirrors; a place without a route to them says so and is not failed.
  if curl -fsS --max-time 15 -o /dev/null http://deb.debian.org/debian/dists/bookworm/InRelease 2> /dev/null; then
    if sudo -n apt-get update -qq > /dev/null 2>&1 && sudo -n apt-get install -y -qq --no-install-recommends tree > /dev/null 2>&1 && tree --version > /dev/null 2>&1; then
      pass "a package is installed with apt through sudo, and the user runs what it installed ($(tree --version | head -n 1))"
    else
      fail "apt could not install a package through sudo"
    fi
  else
    skip "no route to the Debian mirrors from here, so installing a package was not tried"
  fi
else
  if [ "$no_new_privs" = 1 ]; then pass "the unit sets no_new_privileges"; else fail "NoNewPrivs is $no_new_privs in a unit that sets it"; fi
  said="$(sudo -n true 2>&1)"; status=$?
  if [ "$status" != 0 ]; then pass "sudo does not run (exit $status)"; else fail "sudo ran in a unit that sets no_new_privileges"; fi
  case "$said" in
    *"no new privileges"*) pass "and says why: $(printf '%s' "$said" | head -n 1)" ;;
    *) fail "sudo failed without naming the flag: $said" ;;
  esac
  if [ "$(id -u)" = 1000 ]; then pass "the user is still the unit's user"; else fail "the user changed"; fi
  if [ "$(playwright-cli --version 2> /dev/null)" != "" ]; then pass "what the image holds still runs for the user without root (playwright-cli $(playwright-cli --version))"; else fail "playwright-cli does not run"; fi
fi
echo DONE
