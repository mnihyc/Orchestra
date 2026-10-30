# Developer tools environment image

The image the Developer tools template's environments are made from: the [default environment
image](../orch-env-base/README.md) of the same release with three things added that a coding agent needs when it
builds software or runs services of its own. It is about 0.6 GiB larger than the default image (2.2 GB on disk,
where the default is 1.6 GiB). It holds nothing of Orch's, as the default image does not.

- **Docker Engine** with the build and compose plugins, for an environment whose runtime has a kernel of its own and
  that was given the *Docker inside* capability. Nothing starts it by itself: `orch-dockerd` does, as root, where Orch
  prepared the place for Docker's data (`/var/lib/docker`: a file system of its own in a file of the state volume under
  a virtual machine, a directory of the state volume under gVisor), and says so where it did not. Docker's socket
  belongs to the agent's group, so `docker` needs no `sudo`. Where the kernel is written in user space the daemon
  runs without a firewall (`--iptables=false`, which `orch-dockerd` chooses by itself); containers started inside
  should use `--network=host` to reach the network. Startup has a one-minute deadline, including API checks;
  a timed-out check retains a live daemon rather than starting a second one. If the startup process exits, the helper
  retries once within that same minute and appends to its log. See `/var/log/dockerd.log` if it fails. Orch prepares
  `/var/run/docker` as temporary process state on every boot; images and containers stay in `/var/lib/docker`.
  If Docker's bridge subnet overlaps the host or a client network, replies to those clients can follow the wrong
  route. Choose a non-overlapping `bip` in `/etc/docker/daemon.json` inside the environment before starting Docker
  (or pass `--bip` through `ORCH_DOCKERD_ARGS`); restart the inner daemon after changing it.
- **QEMU**, the system emulators for x86_64 and aarch64 and its utilities. Under a runtime with a kernel of its own
  there is no KVM, so virtual machines run by software emulation: they work, slowly. On a plain container the `kvm`
  capability makes them fast.
- **Build tools**: cmake, pkg-config, autoconf, automake and libtool, and the development files of OpenSSL, zlib and
  libffi. The compiler, make and git are already in the default image.

`cargo xtask package` builds it as `orch-env-dev:<version>` on the default image of that version, which is the tag
the Developer tools template is seeded with, and refuses an image whose labels do not say the versions the recipe
pins.

## What it holds

| Item | Version | Pinned by (build argument) | Label |
|------|---------|----------------------------|-------|
| Docker Engine and CLI | 29.9.2 | `DOCKER_VERSION` | `orch.image.docker` |
| containerd | 2.4.2, package revision `3~debian.12~bookworm` | `CONTAINERD_VERSION`, `CONTAINERD_PACKAGE_REVISION` | `orch.image.containerd` |
| Docker Buildx | 0.38.1 | `BUILDX_VERSION` | `orch.image.buildx` |
| Docker Compose | 5.6.1 | `COMPOSE_VERSION` | `orch.image.compose` |
| QEMU (Debian bookworm's) | 7.2, package `1:7.2+dfsg-7+deb12u18+b3` | `QEMU_VERSION`, `QEMU_PACKAGE_VERSION` | `orch.image.qemu` |

The recipe also pins its added build packages to the versions measured in the proof image: cmake `3.25.1-1`,
pkg-config `1.8.1-1`, autoconf `2.71-3`, automake `1:1.16.5-1.3`, libtool `2.4.7-7~deb12u1`,
libssl-dev `3.0.22-1~deb12u2`, zlib1g-dev `1:1.2.13.dfsg-1` and libffi-dev `3.4.4-1`.
Each has a named build argument in the Dockerfile. Update the pins together with the proof image; a missing package
revision fails the build instead of silently selecting another one.

Docker's packages come from Docker's own repository for this release of Debian, whose signing key's fingerprint is
checked (`DOCKER_SIGNING_FINGERPRINT`). The image also says what it has, in the labels `orch.image.has-docker` and
`orch.image.has-qemu`.

Licences of what it adds are in [NOTICE.md](NOTICE.md).
