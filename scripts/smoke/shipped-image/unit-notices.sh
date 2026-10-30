#!/usr/bin/env bash
# Locale and third-party notice in the shipped environment.
set -u
failed=0
check() {
  if "$@"; then printf 'PASS  %s\n' "$*"; else printf 'FAIL  %s\n' "$*"; failed=1; fi
}
check test "${LANG:-}" = C.UTF-8
check test -r /usr/share/doc/orch-env-base/NOTICE.md
for component in 'Chrome for Testing' 'Node.js' 'Playwright' 'TigerVNC' 'openbox' 'Fonts' 'Debian'; do
  check grep -q "$component" /usr/share/doc/orch-env-base/NOTICE.md
done
check test -r /usr/local/LICENSE
check test -r /usr/share/doc/tigervnc-standalone-server/copyright
check test -r /usr/share/doc/openbox/copyright
printf 'DONE\n'
exit "$failed"
