#!/bin/bash
# Sets up gVisor (runsc) beside runc on this host's Docker, so that Orch can offer environments that run in a kernel written
# in user space, on a host without KVM.
#
# Optional. Orch's installer installs no runtime; this is for an administrator who wants one. Run it as root. It is
# idempotent: run again, it says "already so" for what is in place, and a --dry-run says what it would do and changes
# nothing. It prints each thing it changed.
#
# What it does:
#   1. unpacks gVisor's release below /usr/local/lib/gvisor, after checking the SHA-256 the vendor publishes for it, and
#      links runsc and its containerd shim into /usr/local/bin;
#   2. registers the runtime "runsc" in /etc/docker/daemon.json and asks Docker to read it again, with these runsc flags:
#        --overlay2=none          what an environment installs outside its volumes is kept across a stop, as under runc.
#                                 gVisor's default keeps the root file system's changes in memory of the sandbox, and they
#                                 are lost whenever the environment stops. Without this flag say so in the node's configuration
#                                 (keeps_software_on_stop = false), which is what Orch assumes of gVisor.
#        --allow-suid             lets a program with the set-user-ID bit (sudo) become root inside the sandbox. gVisor's default
#                                 refuses, and then sudo fails in every environment ("effective uid is not 0"). The identity is
#                                 raised inside the sandbox's kernel only; a container made with no-new-privileges still cannot.
#
# Then tell the node what the runtime is, in its configuration, because Docker cannot (this is the operator's word):
#
#   [node.docker.runtimes.runsc]
#   boundary = "user_space_kernel"
#   keeps_software_on_stop = true

set -euo pipefail

GVISOR_RELEASE=${GVISOR_RELEASE:-release-20261012.0}
GVISOR_BASE=${GVISOR_BASE:-https://github.com/google/gvisor/releases/download}
GVISOR_DIR=${GVISOR_DIR:-/usr/local/lib/gvisor}
RUNTIME_NAME=${RUNTIME_NAME:-runsc}

usage() {
  sed -n '2,/^set -euo/p' "$0" | sed '$d' | sed 's/^# \{0,1\}//'
}

here=$(dirname "$(readlink -f "$0")")
# shellcheck source=common.sh
. "$here/common.sh"

case "$(uname -m)" in
  x86_64) ARCH=x86_64; SHA256=21c1f60222826204e57eda551efc68fcdfd94a3ba2ef3e01d2a43d8bc4429e51 ;;
  aarch64) ARCH=aarch64; SHA256=bc76435fe7ca56f918d9a61aa27013989fee26d5731ee0ed82b99bb1241faa20 ;;
  *) die "gVisor ${GVISOR_RELEASE} is set up here for x86_64 and aarch64 only" ;;
esac

need_root
need docker
need curl
need sha256sum
need zstd
need tar

target=$GVISOR_DIR/$GVISOR_RELEASE
# 1. The release.
if [ -x "$target/runsc" ]; then
  kept "gVisor $GVISOR_RELEASE is unpacked in $target"
else
  archive=/var/cache/orch/gvisor-$GVISOR_RELEASE-$ARCH.tar.zstd
  url=$GVISOR_BASE/$GVISOR_RELEASE/gvisor-$ARCH.tar.zstd
  if [ "$DRY_RUN" = 1 ]; then
    say "would change: gVisor $GVISOR_RELEASE is downloaded from $url, checked against its SHA-256 and unpacked in $target"
  else
    mkdir -p "$(dirname "$archive")" "$GVISOR_DIR"
    if ! { [ -f "$archive" ] && echo "$SHA256  $archive" | sha256sum -c --status -; }; then
      curl -fL --retry 3 -o "$archive.part" "$url"
      echo "$SHA256  $archive.part" | sha256sum -c - >/dev/null || { rm -f "$archive.part"; die "the download does not match the SHA-256 this script was made with"; }
      mv "$archive.part" "$archive"
    fi
    staging=$(mktemp -d)
    trap 'rm -rf "$staging"' EXIT
    tar --zstd -xf "$archive" -C "$staging"
    [ -x "$staging/runsc" ] || die "the archive holds no runsc"
    rm -rf "$target"
    mv "$staging" "$target"
    chmod 0755 "$target"
    changed "gVisor $GVISOR_RELEASE is unpacked in $target"
  fi
fi
for program in runsc containerd-shim-runsc-v1; do
  link=/usr/local/bin/$program
  if [ "$(readlink "$link" 2>/dev/null)" = "$target/$program" ]; then
    kept "$link points at gVisor $GVISOR_RELEASE"
  else
    do_it "$link is linked to $target/$program" ln -sfn "$target/$program" "$link"
  fi
done

# 2. The runtime, with the flags above.
register_runtime "$RUNTIME_NAME" "{\"path\": \"/usr/local/bin/runsc\", \"runtimeArgs\": [\"--overlay2=none\", \"--allow-suid\"]}"
reload_docker

say ""
say "Tell the node what the runtime is (Orch cannot read it from Docker):"
say "  [node.docker.runtimes.$RUNTIME_NAME]"
say "  boundary = \"user_space_kernel\""
say "  keeps_software_on_stop = true"
summary
