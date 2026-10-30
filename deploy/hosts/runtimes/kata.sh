#!/bin/bash
# Sets up Kata Containers (runtime-rs with QEMU) beside runc on this host's Docker, so that Orch can offer environments
# that each run in a virtual machine with a kernel of their own.
#
# Optional. Orch's installer installs no runtime; this is for an administrator who wants one. Run it as root. It is
# idempotent: run again, it says "already so" for what is in place, and a --dry-run says what it would do and changes
# nothing. It prints each thing it changed.
#
# What it needs: Docker 26 or later, an x86_64 or aarch64 host with /dev/kvm (nested virtualisation when the host is itself a
# virtual machine), and the network to fetch the release.
#
# What it does:
#   1. loads the kernel modules vhost_vsock and vhost_net, and keeps them loaded across a restart;
#   2. unpacks Kata's static release below /opt/kata, after checking the SHA-256 the vendor publishes for it;
#   3. installs the one program Docker's containerd starts for the runtime, which names the QEMU configuration;
#   4. registers the runtime "kata" in /etc/docker/daemon.json and asks Docker to read it again.
#
# Then tell the node what the runtime is, in its configuration, because Docker cannot (this is the operator's word):
#
#   [node.docker.runtimes.kata]
#   boundary = "own_kernel"

set -euo pipefail

KATA_VERSION=4.3.1
KATA_BASE=${KATA_BASE:-https://github.com/kata-containers/kata-containers/releases/download}
KATA_DIR=${KATA_DIR:-/opt/kata}
RUNTIME_NAME=${RUNTIME_NAME:-kata}
# The name the shim program has for containerd: containerd turns the runtime type io.containerd.kata-qemu-rs.v2 into this file.
SHIM_TYPE=io.containerd.kata-qemu-rs.v2
SHIM_PATH=${SHIM_PATH:-/usr/local/bin/containerd-shim-kata-qemu-rs-v2}
KATA_CONFIG=$KATA_DIR/share/defaults/kata-containers/runtime-rs/configuration-qemu-runtime-rs.toml

usage() {
  sed -n '2,/^set -euo/p' "$0" | sed '$d' | sed 's/^# \{0,1\}//'
}

here=$(dirname "$(readlink -f "$0")")
# shellcheck source=common.sh
. "$here/common.sh"

case "$(uname -m)" in
  x86_64) ARCH=amd64; SHA256=1463a2b86845f4d3d8e87f2f764ec59856fc0fee3520967a79ade32762be2e97 ;;
  aarch64) ARCH=arm64; SHA256=2c5bf7cde334ad5307d482f45cceda496f91c9b6b96f5278c47f4f4eac1bbae6 ;;
  *) die "Kata ${KATA_VERSION} is set up here for x86_64 and aarch64 only" ;;
esac

need_root
need docker
need curl
need sha256sum
need zstd
need tar

[ -e /dev/kvm ] || die "/dev/kvm is not there: this host has no hardware virtualisation to give (or its virtual machine has nested virtualisation off)"
docker_major=$(docker version --format '{{.Server.Version}}' 2>/dev/null | cut -d. -f1 || true)
if [ -z "${docker_major:-}" ] || [ "$docker_major" -lt 26 ]; then
  die "Docker 26 or later is needed (Kata's own tests with Docker start there); this is ${docker_major:-not answering}"
fi

# 1. The modules Kata talks to its guest through.
modules_file=/etc/modules-load.d/orch-kata.conf
for module in vhost_vsock vhost_net; do
  if grep -q "^$module " /proc/modules 2>/dev/null; then
    kept "the kernel module $module is loaded"
  else
    do_it "the kernel module $module is loaded now" modprobe "$module"
  fi
done
if [ -f "$modules_file" ] && grep -qx vhost_vsock "$modules_file" && grep -qx vhost_net "$modules_file"; then
  kept "$modules_file keeps both modules loaded"
else
  do_it "$modules_file is written, so that both modules load at boot" sh -c "mkdir -p '$(dirname "$modules_file")' && printf 'vhost_vsock\nvhost_net\n' > '$modules_file'"
fi

# 2. The release.
if [ -f "$KATA_DIR/VERSION" ] && [ "$(cat "$KATA_DIR/VERSION")" = "$KATA_VERSION" ]; then
  kept "Kata $KATA_VERSION is unpacked in $KATA_DIR"
else
  archive=/var/cache/orch/kata-static-$KATA_VERSION-$ARCH.tar.zst
  url=$KATA_BASE/$KATA_VERSION/kata-static-$KATA_VERSION-$ARCH.tar.zst
  if [ "$DRY_RUN" = 1 ]; then
    say "would change: Kata $KATA_VERSION is downloaded from $url, checked against its SHA-256 and unpacked below $KATA_DIR"
  else
    mkdir -p "$(dirname "$archive")"
    if ! { [ -f "$archive" ] && echo "$SHA256  $archive" | sha256sum -c --status -; }; then
      curl -fL --retry 3 -o "$archive.part" "$url"
      echo "$SHA256  $archive.part" | sha256sum -c - >/dev/null || { rm -f "$archive.part"; die "the download does not match the SHA-256 this script was made with"; }
      mv "$archive.part" "$archive"
    fi
    staging=$(mktemp -d)
    trap 'rm -rf "$staging"' EXIT
    tar --zstd -xf "$archive" -C "$staging"
    [ -d "$staging/opt/kata" ] || die "the archive holds no opt/kata"
    rm -rf "$KATA_DIR.old"
    [ -d "$KATA_DIR" ] && mv "$KATA_DIR" "$KATA_DIR.old"
    mv "$staging/opt/kata" "$KATA_DIR"
    rm -rf "$KATA_DIR.old"
    changed "Kata $KATA_VERSION is unpacked below $KATA_DIR"
  fi
fi

# 3. The program containerd starts. Kata 4 accepts only the configuration files it ships, named by this variable.
wanted=$(printf '#!/bin/sh\n# Made by Orch'"'"'s kata.sh: the shim of the runtime %s, with the QEMU configuration it ships.\nexport KATA_CONF_FILE=%s\nexec %s/runtime-rs/bin/containerd-shim-kata-v2 "$@"\n' "$RUNTIME_NAME" "$KATA_CONFIG" "$KATA_DIR")
if [ -x "$SHIM_PATH" ] && [ "$(cat "$SHIM_PATH")" = "$wanted" ]; then
  kept "$SHIM_PATH starts Kata's shim with its QEMU configuration"
else
  do_it "$SHIM_PATH is written" sh -c "printf '%s\n' \"\$1\" > '$SHIM_PATH' && chmod 0755 '$SHIM_PATH'" sh "$wanted"
fi

# 4. The runtime, in Docker's configuration.
register_runtime "$RUNTIME_NAME" "{\"runtimeType\": \"$SHIM_TYPE\"}"
reload_docker

say ""
say "Tell the node what the runtime is (Orch cannot read it from Docker):"
say "  [node.docker.runtimes.$RUNTIME_NAME]"
say "  boundary = \"own_kernel\""
summary
