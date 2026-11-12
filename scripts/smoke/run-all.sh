#!/usr/bin/env bash
# Runs deployment scenarios one after the other and says which of them kept all their promises. Each makes,
# and removes, a stack of its own.
#
#   run-all.sh            every scenario: the two sets below, one after the other
#   run-all.sh fresh      the scenarios that prove a fresh installation and its operation
#   run-all.sh upgrade    the scenarios that prove an update, a rollback or a recovery from an earlier release
#
# `fresh` needs one packaged release (ORCH_SMOKE_RELEASE_A; installation-records makes the later manifest it
# offers itself). `upgrade` needs two real ones, the earlier as ORCH_SMOKE_RELEASE_A and the later as
# ORCH_SMOKE_RELEASE_B. Without the variables a scenario packages the releases it is missing, which takes hours.
set -u
here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
failed=""
fresh="first-start restore installers installation-records managed-environment managed-harness managed-provider shipped-image"
upgrade="update rollback recover release-distribution"
# The profile transfers and actual-duration soak take hours and need prebuilt fixtures/browser prerequisites.
if [ "${ORCH_SMOKE_RELEASE_PROOFS:-0}" = 1 ]; then fresh="$fresh link-load load soak"; fi
sets=""
for set in "${@:-all}"; do
  case "$set" in
    fresh) sets="$sets $fresh" ;;
    upgrade) sets="$sets $upgrade" ;;
    all) sets="$sets $fresh $upgrade" ;;
    *) printf 'run-all.sh: %s is not a set of scenarios (fresh, upgrade or all)\n' "$set" >&2; exit 2 ;;
  esac
done
for scenario in $sets; do
  printf '\n######## %s\n' "$scenario"
  bash "$here/$scenario/run.sh" || failed="$failed $scenario"
done
if [ -n "$failed" ]; then printf '\nfailed:%s\n' "$failed" >&2; exit 1; fi
printf '\nall scenarios passed\n'
