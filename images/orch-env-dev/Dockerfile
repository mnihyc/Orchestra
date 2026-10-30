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
# `cargo xtask package` builds it as orch-env-dev:<version>, the tag the API seeds the Developer tools template with,
# from the base image of the same version.

ARG BASE_IMAGE=orch-env-base:dev
FROM ${BASE_IMAGE}

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
COPY orch-dockerd /usr/local/bin/orch-dockerd
RUN chmod 0755 /usr/local/bin/orch-dockerd \
 && usermod --append --groups docker agent \
 && test -x /usr/local/bin/orch-dockerd

# The release's version and the base image are named last: a build argument is part of what a step is made from.
ARG BASE_IMAGE
ARG VERSION=dev

COPY NOTICE.md /usr/share/doc/orch-env-dev/NOTICE.md

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
