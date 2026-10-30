# The image the Developer tools template's environments are made from.
#
# It is the Default environment image with three things added that a coding agent needs when it builds software or
# runs services of its own:
#
#   - Docker Engine, with the build and compose plugins, for an environment whose runtime has a kernel of its own and
#     that was given Docker inside. Nothing starts it: `orch-dockerd` does, as root, where Orch prepared the place for
#     Docker's data (`/var/lib/docker`: a file system of its own in a file of the state volume, or a directory of it), and says so where it did not.
#   - QEMU, the system emulators for x86_64 and aarch64 and its utilities. Under a runtime with a kernel of its own
#     there is no KVM, so virtual machines run by software emulation: they work, slowly. With the `kvm` capability
#     on a plain container they are fast.
#   - The tools a program with native parts is built with: cmake, pkg-config, autoconf, automake and libtool, and the
#     development files of OpenSSL, zlib and libffi. The compiler, make and git are already in the base image.
#
# Every version is pinned below, checked after it is installed, and written into the image's labels;
# images/orch-env-dev/README.md lists them.
#
# This stage follows the Default recipe in the Developer tools template and in
# the release build. No local Default image or build context is needed.

FROM orch-base

# Docker's own packages for this release of Debian. The versions are the ones of Docker's repository
# (https://download.docker.com/linux/debian/dists/bookworm/stable/), with the revision after them.
ARG DOCKER_VERSION=29.9.2
ARG CONTAINERD_VERSION=2.4.2
ARG CONTAINERD_PACKAGE_REVISION=3~debian.12~bookworm
ARG BUILDX_VERSION=0.38.1
ARG COMPOSE_VERSION=5.6.1

# The vendor's signing key: the key id is the one its documentation gives, and the build stops when the key it
# downloads is another.
ARG DOCKER_SIGNING_FINGERPRINT=9DC858229FC7DD38854AE2D88D81803C0EBFCD88

# QEMU's upstream line is also a label; its complete Debian version below pins the security revision.
ARG QEMU_VERSION=7.2
ARG QEMU_PACKAGE_VERSION=1:7.2+dfsg-7+deb12u18+b3
ARG CMAKE_VERSION=3.25.1-1
ARG PKG_CONFIG_VERSION=1.8.1-1
ARG AUTOCONF_VERSION=2.71-3
ARG AUTOMAKE_VERSION=1:1.16.5-1.3
ARG LIBTOOL_VERSION=2.4.7-7~deb12u1
ARG LIBSSL_DEV_VERSION=3.0.22-1~deb12u2
ARG ZLIB_DEV_VERSION=1:1.2.13.dfsg-1
ARG LIBFFI_DEV_VERSION=3.4.4-1

USER root
SHELL ["/bin/bash", "-o", "pipefail", "-c"]

RUN set -eux; \
    apt-get update; \
    apt-get install -y --no-install-recommends gnupg; \
    install -m 0755 -d /etc/apt/keyrings; \
    curl -fsSL --retry 3 https://download.docker.com/linux/debian/gpg -o /tmp/docker.asc; \
    gpg --show-keys --with-colons /tmp/docker.asc | awk -F: '$1 == "fpr" {print $10; exit}' | grep --fixed-strings "${DOCKER_SIGNING_FINGERPRINT}"; \
    gpg --dearmor --output /etc/apt/keyrings/docker.gpg /tmp/docker.asc; \
    rm /tmp/docker.asc; \
    chmod a+r /etc/apt/keyrings/docker.gpg; \
    echo "deb [arch=$(dpkg --print-architecture) signed-by=/etc/apt/keyrings/docker.gpg] https://download.docker.com/linux/debian bookworm stable" > /etc/apt/sources.list.d/docker.list; \
    apt-get update; \
    apt-get install -y --no-install-recommends \
        "docker-ce=5:${DOCKER_VERSION}-1~debian.12~bookworm" \
        "docker-ce-cli=5:${DOCKER_VERSION}-1~debian.12~bookworm" \
        "containerd.io=${CONTAINERD_VERSION}-${CONTAINERD_PACKAGE_REVISION}" \
        "docker-buildx-plugin=${BUILDX_VERSION}-1~debian.12~bookworm" \
        "docker-compose-plugin=${COMPOSE_VERSION}-1~debian.12~bookworm" \
        iptables e2fsprogs kmod pigz xz-utils; \
    apt-get purge -y gnupg; \
    apt-get autoremove -y; \
    rm -rf /var/lib/apt/lists/*; \
    test "$(docker --version | sed -E 's/^Docker version ([0-9.]+),.*/\1/')" = "${DOCKER_VERSION}"; \
    test "$(docker buildx version | awk '{print $2}')" = "v${BUILDX_VERSION}"; \
    test "$(docker compose version --short)" = "${COMPOSE_VERSION}" || test "$(docker compose version --short)" = "v${COMPOSE_VERSION}"; \
    test "$(containerd --version | awk '{print $3}')" = "v${CONTAINERD_VERSION}"

RUN apt-get update \
 && apt-get install -y --no-install-recommends \
        "qemu-system-x86=${QEMU_PACKAGE_VERSION}" "qemu-system-arm=${QEMU_PACKAGE_VERSION}" "qemu-utils=${QEMU_PACKAGE_VERSION}" \
        "cmake=${CMAKE_VERSION}" "pkg-config=${PKG_CONFIG_VERSION}" \
        "autoconf=${AUTOCONF_VERSION}" "automake=${AUTOMAKE_VERSION}" "libtool=${LIBTOOL_VERSION}" \
        "libssl-dev=${LIBSSL_DEV_VERSION}" "zlib1g-dev=${ZLIB_DEV_VERSION}" "libffi-dev=${LIBFFI_DEV_VERSION}" \
 && rm -rf /var/lib/apt/lists/* \
 && for pinned in \
        "qemu-system-x86=${QEMU_PACKAGE_VERSION}" "qemu-system-arm=${QEMU_PACKAGE_VERSION}" "qemu-utils=${QEMU_PACKAGE_VERSION}" \
        "cmake=${CMAKE_VERSION}" "pkg-config=${PKG_CONFIG_VERSION}" \
        "autoconf=${AUTOCONF_VERSION}" "automake=${AUTOMAKE_VERSION}" "libtool=${LIBTOOL_VERSION}" \
        "libssl-dev=${LIBSSL_DEV_VERSION}" "zlib1g-dev=${ZLIB_DEV_VERSION}" "libffi-dev=${LIBFFI_DEV_VERSION}"; do \
        test "$(dpkg-query -W -f='${Version}' "${pinned%%=*}")" = "${pinned#*=}"; \
    done \
 && qemu-system-x86_64 --version | head -n 1 | grep --fixed-strings "version ${QEMU_VERSION}." \
 && qemu-system-aarch64 --version | head -n 1 | grep --fixed-strings "version ${QEMU_VERSION}." \
 && qemu-img --version | head -n 1 | grep --fixed-strings "version ${QEMU_VERSION}." \
 && cmake --version | head -n 1 | grep --fixed-strings "cmake version" \
 && pkg-config --exists openssl zlib libffi \
 && command -v autoconf automake libtoolize > /dev/null

# What starts Docker where Orch has prepared its place, and says so where it has not. dockerd runs as root, and its
# socket belongs to the agent's group so that `docker` needs no sudo. Where the runtime shares a kernel the daemon
# is not offered, and `mountpoint` fails.
# Its text is written here so the recipe needs no build context.
RUN install -d /usr/local/bin \
 && printf '%s\n' \
    '#!/bin/sh' \
    '# Starts Docker inside this environment, once, and waits until it answers.' \
    '#' \
    '# Orch prepares Docker'"'"'s data place (/var/lib/docker, a file system of its own in a file of the state volume) where the' \
    '# environment holds Docker inside, every time it starts. Without that place a daemon would write to a shared file' \
    '# system that overlays cannot be built on, so this program refuses to start one.' \
    'set -eu' \
    '' \
    '# The CLI can wait indefinitely for an unresponsive API, even after the daemon has opened its socket. Bound the' \
    '# complete startup, including every readiness probe. A stuck child gets five seconds to exit after the deadline.' \
    'if timeout --kill-after=5s 60s sh -s <<'"'"'START_DOCKER'"'"'' \
    'set -eu' \
    '' \
    'if timeout --kill-after=1s 5s docker info >/dev/null 2>&1; then' \
    '  exit 0' \
    fi \
    'if ! mountpoint -q /var/lib/docker 2>/dev/null; then' \
    '  echo "Docker'"'"'s place is not prepared here: ask for Docker inside first (the capability), then run this again." >&2' \
    '  exit 1' \
    fi \
    'root=""' \
    'if [ "$(id -u)" != 0 ]; then root="sudo"; fi' \
    'started_pid=""' \
    '# A file system that outlives a stop (the container'"'"'s own layer is kept under some runtimes) keeps the last daemon'"'"'s' \
    '# process number and socket. Keep them when that daemon is still running: an API timeout is not proof it is gone.' \
    'daemon_alive() {' \
    '  pid="$($root cat /var/run/docker.pid 2>/dev/null || true)"' \
    '  case "$pid" in' \
    '    '"'"''"'"'|*[!0-9]*) ;;' \
    '    *) if [ "$($root cat "/proc/$pid/comm" 2>/dev/null || true)" = dockerd ] && $root kill -0 "$pid" 2>/dev/null; then return 0; fi ;;' \
    '  esac' \
    '  # The process we just launched can still be loading before it writes docker.pid. Its absence is not an exit.' \
    '  [ -n "$started_pid" ] && $root kill -0 "$started_pid" 2>/dev/null' \
    '}' \
    'start_daemon() {' \
    '  $root rm -f /var/run/docker.pid /var/run/docker.sock' \
    '  # A runtime with a kernel written in user space (gVisor) has no firewall for Docker to configure: the daemon is then' \
    '  # started without it. Containers started inside should use --network=host to reach the network; -p still serves a port.' \
    '  firewall=""' \
    '  if ! $root iptables -t nat -L >/dev/null 2>&1; then firewall="--iptables=false"; fi' \
    '  # ORCH_DOCKERD_ARGS: more arguments for the daemon, for a runtime that needs them.' \
    '  started_pid="$($root sh -c "nohup dockerd -G agent $firewall \${ORCH_DOCKERD_ARGS:-} >>/var/log/dockerd.log 2>&1 </dev/null & echo \$!")"' \
    '}' \
    starts=0 \
    'if ! daemon_alive; then' \
    '  start_daemon' \
    '  starts=1' \
    fi \
    'while ! timeout --kill-after=1s 5s docker info >/dev/null 2>&1; do' \
    '  if ! daemon_alive; then' \
    '    # A managed containerd can exit during a cold start. Retry once after an actual process exit, within the same' \
    '    # minute; keep its diagnostic log and never start a second daemon beside a live one.' \
    '    if [ "$starts" -ge 2 ]; then' \
    '      echo "Docker exited while starting: see /var/log/dockerd.log" >&2' \
    '      exit 1' \
    '    fi' \
    '    echo "Docker exited while starting; retrying within the same minute. See /var/log/dockerd.log" >&2' \
    '    start_daemon' \
    '    starts=$((starts + 1))' \
    '  fi' \
    '  sleep 1' \
    done \
    START_DOCKER \
    then \
    '  exit 0' \
    else \
    '  status=$?' \
    fi \
    'case "$status" in' \
    '  124|137) echo "Docker did not start in a minute: see /var/log/dockerd.log" >&2 ;;' \
    esac \
    'exit "$status"' > /usr/local/bin/orch-dockerd \
 && chmod 0755 /usr/local/bin/orch-dockerd
RUN usermod --append --groups docker agent \
 && test -x /usr/local/bin/orch-dockerd

# The release's version and the base image are named last: a build argument is part of what a step is made from.
ARG BASE_IMAGE
ARG VERSION=dev

# This notice is written here so the recipe needs no build context.
RUN install -d /usr/share/doc/orch-env-dev \
 && printf '%s\n' \
    '# Software in orch-env-dev' \
    '' \
    'The image is the Default environment image (see `/usr/share/doc/orch-env-base/NOTICE.md` for its software) with the' \
    'software below added. `LicenseRef-Orch-Env-Dev` in its image label refers to this notice and to that one; it does not' \
    'grant one licence for everything in the image. The Dockerfile, `orch-dockerd` and this notice are Apache-2.0.' \
    '' \
    '| Component | Terms and notices |' \
    '|-----------|-------------------|' \
    '| Docker Engine, the Docker command line, buildx and compose | Apache-2.0. Each package'"'"'s licence and notices are under `/usr/share/doc/<package>/`; [Docker Engine source and licence](https://github.com/moby/moby). |' \
    '| containerd | Apache-2.0. [containerd source and licence](https://github.com/containerd/containerd). |' \
    '| QEMU | GPL-2.0-or-later for QEMU'"'"'s own code, with separately licensed components; Debian'"'"'s complete record is `/usr/share/doc/qemu-system-x86/copyright`, with records for the other QEMU packages beside it. [QEMU source](https://www.qemu.org/). |' \
    '| Build tools and development files (cmake, pkg-config, autoconf, automake, libtool, OpenSSL, zlib, libffi) | Each package keeps its copyright and licence record under `/usr/share/doc/<package>/copyright`. |' \
    '' \
    'The source of Debian packages is available from Debian'"'"'s source archives; the versions installed in this image can be' \
    'listed with `dpkg-query -W`. Keep the bundled licence and copyright files when distributing the image, and apply each' \
    'component'"'"'s own terms to its redistribution.' > /usr/share/doc/orch-env-dev/NOTICE.md

LABEL org.opencontainers.image.title="orch-env-dev" \
      org.opencontainers.image.description="The Default environment with Docker Engine, QEMU and build tools, for Developer tools" \
      org.opencontainers.image.version="${VERSION}" \
      org.opencontainers.image.licenses="LicenseRef-Orch-Env-Dev" \
      orch.image.recipe.license="Apache-2.0" \
      org.opencontainers.image.base.name="${BASE_IMAGE}" \
      orch.image.docker="${DOCKER_VERSION}" \
      orch.image.containerd="${CONTAINERD_VERSION}" \
      orch.image.buildx="${BUILDX_VERSION}" \
      orch.image.compose="${COMPOSE_VERSION}" \
      orch.image.qemu="${QEMU_VERSION}" \
      orch.image.has-docker="true" \
      orch.image.has-qemu="true"

SHELL ["/bin/sh", "-c"]

USER agent
ENV HOME=/home/agent
WORKDIR /workspace

CMD ["/bin/bash"]
