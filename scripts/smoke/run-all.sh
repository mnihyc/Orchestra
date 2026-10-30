#!/usr/bin/env bash
# Runs every deployment scenario one after the other and says which of them kept all
# their promises. Each makes, and removes, a stack of its own.
set -u
here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
failed=""
scenarios="first-start update rollback recover restore installers release-distribution installation-records managed-environment managed-harness managed-provider shipped-image"
# The profile transfers and actual-duration soak take hours and need prebuilt fixtures/browser prerequisites.
if [ "${ORCH_SMOKE_RELEASE_PROOFS:-0}" = 1 ]; then scenarios="$scenarios link-load load soak"; fi
for scenario in $scenarios; do
  printf '\n######## %s\n' "$scenario"
  bash "$here/$scenario/run.sh" || failed="$failed $scenario"
done
if [ -n "$failed" ]; then printf '\nfailed:%s\n' "$failed" >&2; exit 1; fi
printf '\nall scenarios passed\n'
