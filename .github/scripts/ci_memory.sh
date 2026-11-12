#!/usr/bin/env bash
# Memory headroom for the largest crate on ephemeral Linux CI runners.
set -euo pipefail

if [[ ${GITHUB_ACTIONS:-} != true || ${RUNNER_OS:-} != Linux ]]; then
  printf '%s\n' 'This helper is only for Linux GitHub Actions runners.' >&2
  exit 1
fi

ci_swap="${RUNNER_TEMP:?}/orch-ci-memory.swap"
if [[ -e "$ci_swap" || -L "$ci_swap" ]]; then
  printf '%s\n' 'The CI swap path already exists; refusing to replace it.' >&2
  exit 1
fi

free -h
df -h "$RUNNER_TEMP"
umask 077
# Another 8 GiB gives code generation headroom without changing the release profile.
# Keep the runner's existing swap; the VM and this file go away with the job.
fallocate -l 8G "$ci_swap"
mkswap "$ci_swap"
sudo swapon "$ci_swap"
swapon --show
free -h
