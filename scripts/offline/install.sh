#!/usr/bin/env bash
# Verify carried-in files, then use the ordinary installer or updater.
set -euo pipefail
exec python3 -I -B "$(dirname "${BASH_SOURCE[0]}")/transfer.py" install "$@"
