#!/usr/bin/env bash
# Gather an existing release and its verified dependencies; nothing is built or published.
set -euo pipefail
exec python3 -I -B "$(dirname "${BASH_SOURCE[0]}")/transfer.py" gather "$@"
