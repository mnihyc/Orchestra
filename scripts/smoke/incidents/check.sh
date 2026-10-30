#!/usr/bin/env bash
# Read-only incident inventory of an explicitly selected existing Compose deployment.
set -euo pipefail

stack_dir=
project_name=
while [ $# -gt 0 ]; do
  case "$1" in
    --stack) stack_dir=${2:?--stack requires the deployment directory}; shift 2 ;;
    --project) project_name=${2:?--project requires the existing Compose project}; shift 2 ;;
    *) echo "Usage: bash scripts/smoke/incidents/check.sh --stack DIR --project NAME" >&2; exit 2 ;;
  esac
done
if [ -z "$stack_dir" ] || [ -z "$project_name" ]; then echo "Select the existing stack and project explicitly" >&2; exit 2; fi
stack_dir=$(realpath "$stack_dir")
if [ ! -f "$stack_dir/compose.yml" ] || [ ! -f "$stack_dir/.env" ]; then
  echo "The selected directory is not an initialized deployment" >&2
  exit 2
fi
compose=(docker compose --project-directory "$stack_dir" --env-file "$stack_dir/.env" -f "$stack_dir/compose.yml" -p "$project_name")
"${compose[@]}" ps
"${compose[@]}" exec -T api /opt/orchctl/orchctl doctor
