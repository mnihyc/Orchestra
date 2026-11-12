#!/usr/bin/env bash
# Put package mirrors that answer now at the head of the runner's mirror list.
#
# Hosted Linux runners read Ubuntu packages through a mirror list whose names have at times not
# resolved for hours, which no number of repeated installations outlasts. apt checks every index
# against the distribution's own keys, so which mirror serves a file never changes what is installed.
# A mirror that does not answer is taken off the list for this job; when none answers the list is
# left as it is and the caller's own repetition applies.
set -euo pipefail

list="${ORCH_CI_APT_MIRROR_LIST:-/etc/apt/apt-mirrors.txt}"
if [[ ! -f "$list" ]]; then
  printf '%s\n' "No mirror list at $list; apt keeps its own sources."
  exit 0
fi

codename=$(. /etc/os-release && printf '%s' "$VERSION_CODENAME")
# Ubuntu publishes amd64 and i386 under /ubuntu and every other architecture under /ubuntu-ports.
case "$(dpkg --print-architecture)" in
  amd64 | i386) path=ubuntu ;;
  *) path=ubuntu-ports ;;
esac
# Full public mirrors on names unrelated to the distribution's and to one another.
others=("https://mirror.us.leaseweb.net/$path/" "https://mirrors.ocf.berkeley.edu/$path/")
if [[ $path == ubuntu ]]; then
  others=("https://mirrors.edge.kernel.org/ubuntu/" "${others[@]}")
fi

mapfile -t listed < <(awk 'NF && $1 !~ /^#/ {print $1}' "$list")
answering=()
for mirror in "${listed[@]}" "${others[@]}"; do
  # Twenty seconds covers a slow mirror; a name that does not resolve fails at once.
  if curl --fail --silent --show-error --location --max-time 20 --output /dev/null \
      "${mirror%/}/dists/$codename/InRelease"; then
    printf '%s\n' "answers: $mirror"
    answering+=("$mirror")
  else
    printf '%s\n' "does not answer: $mirror"
  fi
done
if [[ ${#answering[@]} -eq 0 ]]; then
  printf '%s\n' 'No package mirror answers now; the list is left as it is.'
  exit 0
fi

rewritten=$(mktemp)
priority=0
for mirror in "${answering[@]}"; do
  priority=$((priority + 1))
  printf '%s\tpriority:%s\n' "$mirror" "$priority" >> "$rewritten"
done
if [[ -w "$list" ]]; then
  cat "$rewritten" > "$list"
else
  sudo tee "$list" < "$rewritten" > /dev/null
fi
rm -f -- "$rewritten"
printf '%s\n' "Package mirrors in use, in order:"
cat "$list"
