# The image the Default template's environments are made from.
#
# It is a plain working machine for an agent and nothing of Orch's: the connector
# and the harnesses are put into a unit from the node's tool cache, read-only, so
# any image works and this is the one that is seeded. What it holds is what a
# coding agent reaches for first: a shell, git, a search, a C compiler and
# Python. Beside that it holds three things that a template made from a smaller
# image goes without:
#
#   - sudo for its user, without a password, so that whoever works there installs
#     what the work needs. A template that sets no_new_privileges takes it away.
#   - a browser, with the command line that drives it (Playwright's) and the
#     Node.js that runs on, the libraries the browser draws with, and fonts for
#     Chinese, Japanese, Korean and emoji.
#   - a VNC server (TigerVNC's Xvnc) and a small window manager (openbox), so that
#     a display can be opened and a browser can have a window.
#
# Every version of the second and third is pinned below, checked after it is
# installed, and written into the image's labels; images/orch-env-base/README.md
# lists them. A team that needs more can edit or copy this Dockerfile in its
# template; one that needs less can choose a smaller image.
#
# The Default template builds this recipe on its host. Release packaging builds
# the same recipe as orch-env-base:<version> and records its digest.

ARG BASE_IMAGE=debian:bookworm-slim@sha256:7c7b2c966bc9ee8cedfeef67e0e279108992c77681fa595db4a9d65c06ccc587
FROM ${BASE_IMAGE} AS orch-base

# Node.js, a line its vendor still maintains (long-term support until April 2028).
# The tarball is the vendor's own, and its SHA-256 is the one in the SHASUMS256.txt
# of that release, one for each processor the image is built for.
ARG NODE_VERSION=24.21.0
ARG NODE_SHA256_AMD64=fd8e59d5a511510f6a298afb548f18c7d2b1be404d8b4a27d94fbe49f56cb2d6
ARG NODE_SHA256_ARM64=6ad1325edbdb5649c379b75a237147a666c95d4f9ae8d340fef2d1575d289ad2

# Playwright's command line, the Playwright it depends on, and the browser that
# Playwright asks for: its revision, and the version of Chrome for Testing that
# revision is. A newer command line asks for another browser, so the three move
# together, and the build stops where they do not agree.
ARG PLAYWRIGHT_CLI_VERSION=0.1.22
ARG PLAYWRIGHT_VERSION=1.64.0-alpha-1790635538000
ARG BROWSER_REVISION=1247
ARG BROWSER_VERSION=155.0.8059.12

# What serves and manages a display. Debian carries them under the upstream
# version with its own revision after it; the upstream version is the pin.
ARG TIGERVNC_VERSION=1.12.0
ARG OPENBOX_VERSION=3.6.1

# Where the browser lives, and how the command line is told to use it. They are
# the image's own environment, so that every user and every process of a unit has
# them. Outside the home directory on purpose: the home is a volume that outlives
# the image, and a browser installed there would be hidden by an older home or
# left behind by an update.
#   PLAYWRIGHT_BROWSERS_PATH  where the command line looks for browsers
#   PLAYWRIGHT_MCP_BROWSER    the browser it uses unless told another: the one
#                             Playwright installs (Chrome for Testing). Without
#                             this it asks for Google Chrome, which this image
#                             does not hold
#   NO_UPDATE_NOTIFIER        the command line looks up its newest version on
#                             npm once a day unless told not to
ENV PLAYWRIGHT_BROWSERS_PATH=/opt/ms-playwright \
    PLAYWRIGHT_MCP_BROWSER=chromium \
    NO_UPDATE_NOTIFIER=1 \
    LANG=C.UTF-8

SHELL ["/bin/bash", "-o", "pipefail", "-c"]

# What the agent works with. ripgrep and git are what harnesses search and
# change files with; build-essential and python3 are what most repositories
# build and test with; the rest is what a person at a shell would be
# surprised to find missing.
RUN apt-get update \
 && apt-get install -y --no-install-recommends \
        bash build-essential ca-certificates curl git jq less \
        openssh-client procps python3 python3-pip python3-venv ripgrep unzip xz-utils zip \
 && rm -rf /var/lib/apt/lists/*

# The account the agent runs as, and the two places an environment keeps its
# volumes (the template's workspace_mount and home_mount). The ids are the
# template's: a volume made for one environment is opened by the next.
RUN groupadd --gid 1000 agent \
 && useradd --uid 1000 --gid 1000 --create-home --home-dir /home/agent --shell /bin/bash agent \
 && mkdir -p /workspace \
 && chown agent:agent /workspace

# Root without a password for that account, and nobody else. The environment
# that sudo drops (it keeps very little) is kept for the three variables above,
# so that what root runs finds the browser as well; a login shell started with an
# empty environment gets them from /etc/profile.d. A sudoers file that does not
# parse would leave nobody able to be root, so the build has visudo read it.
RUN apt-get update \
 && apt-get install -y --no-install-recommends sudo \
 && rm -rf /var/lib/apt/lists/* \
 && { \
        echo 'agent ALL=(ALL:ALL) NOPASSWD: ALL'; \
        echo 'Defaults:agent env_keep += "PLAYWRIGHT_BROWSERS_PATH PLAYWRIGHT_MCP_BROWSER NO_UPDATE_NOTIFIER"'; \
    } > /etc/sudoers.d/90-agent \
 && chmod 0440 /etc/sudoers.d/90-agent \
 && visudo --check --file /etc/sudoers.d/90-agent \
 && { \
        echo '# The browser the command line uses, for a shell that was not given the image environment.'; \
        for name in PLAYWRIGHT_BROWSERS_PATH PLAYWRIGHT_MCP_BROWSER NO_UPDATE_NOTIFIER; do \
            printf 'export %s="${%s:-%s}"\n' "$name" "$name" "${!name}"; \
        done; \
    } > /etc/profile.d/orch-browser.sh

# What a browser draws with and shows text with, from the distribution: the
# libraries are the ones Playwright's own installer names for Chromium on this
# release of Debian (its list, so that the browser it installs starts), the fonts
# are Noto's for Chinese, Japanese and Korean (one family, with the variant of
# each language), colour emoji, metric-compatible Latin ones, and Unifont as the
# last resort, so that a script nobody thought of is drawn and not boxed.
RUN apt-get update \
 && apt-get install -y --no-install-recommends \
        libasound2 libatk-bridge2.0-0 libatk1.0-0 libatspi2.0-0 libcairo2 libcups2 libdbus-1-3 \
        libdrm2 libgbm1 libglib2.0-0 libnspr4 libnss3 libpango-1.0-0 libx11-6 libxcb1 \
        libxcomposite1 libxdamage1 libxext6 libxfixes3 libxkbcommon0 libxrandr2 \
        libfontconfig1 libfreetype6 fontconfig \
        fonts-noto-cjk fonts-noto-color-emoji fonts-liberation fonts-unifont \
 && rm -rf /var/lib/apt/lists/*

# Node.js, from the vendor's tarball into /usr/local, after its hash is checked.
RUN set -eux; \
    case "$(dpkg --print-architecture)" in \
        amd64) node_arch=x64;   node_sha256="${NODE_SHA256_AMD64}" ;; \
        arm64) node_arch=arm64; node_sha256="${NODE_SHA256_ARM64}" ;; \
        *) echo "no pinned Node.js for $(dpkg --print-architecture)" >&2; exit 1 ;; \
    esac; \
    curl -fsSL --retry 3 -o /tmp/node.tar.xz \
        "https://nodejs.org/dist/v${NODE_VERSION}/node-v${NODE_VERSION}-linux-${node_arch}.tar.xz"; \
    echo "${node_sha256}  /tmp/node.tar.xz" | sha256sum --check -; \
    tar -xJf /tmp/node.tar.xz -C /usr/local --strip-components=1 --no-same-owner; \
    rm /tmp/node.tar.xz; \
    test "$(node --version)" = "v${NODE_VERSION}"

# Playwright's command line. Its two dependencies are fixed by it to one build of
# Playwright, which decides what browser it asks for.
RUN set -eux; \
    npm install --global --ignore-scripts --no-fund --no-audit "@playwright/cli@${PLAYWRIGHT_CLI_VERSION}"; \
    npm cache clean --force; \
    rm -rf /root/.npm /tmp/*; \
    test "$(playwright-cli --version)" = "${PLAYWRIGHT_CLI_VERSION}"; \
    test "$(node -p "require('/usr/local/lib/node_modules/@playwright/cli/node_modules/playwright-core/package.json').version")" = "${PLAYWRIGHT_VERSION}"

# The browser, by the command line's own installer, into PLAYWRIGHT_BROWSERS_PATH:
# Chrome for Testing as that Playwright asks for it, with the ffmpeg it records
# video with. Not the headless shell (a separate program that the command line
# starts only for a browser it was not told the channel of, which the variable
# above does), and not Firefox or WebKit. The build checks that it is the browser
# the pins above name and that nothing it needs is missing.
RUN set -eux; \
    PLAYWRIGHT_DOWNLOAD_NO_PROGRESS=1 playwright-cli install-browser chromium --no-shell; \
    browser="$(ls /opt/ms-playwright/chromium-${BROWSER_REVISION}/chrome-linux*/chrome)"; \
    test "$(ls -d /opt/ms-playwright/chromium-* | wc -l)" = 1; \
    test "$("${browser}" --version | sed 's/ *$//')" = "Google Chrome for Testing ${BROWSER_VERSION}"; \
    if ldd "${browser}" | grep 'not found'; then exit 1; fi; \
    chmod -R a+rX /opt/ms-playwright; \
    rm -rf /tmp/*

# A display: TigerVNC's X server with VNC in it, and openbox to move and close
# windows on it. xfonts-base is the font a bare X server cannot start without
# (Debian only recommends it); the X utilities are what a person at a shell asks
# a display with (xdpyinfo, xwininfo, xprop, xrandr), and xauth is what programs
# that start a client of it expect to find. /tmp/.X11-unix is the directory an X
# server puts its socket in, made here by root so that a server run by the agent
# does not have to make it, and so does not warn that it cannot.
RUN apt-get update \
 && apt-get install -y --no-install-recommends \
        "tigervnc-standalone-server=${TIGERVNC_VERSION}+dfsg-*" "openbox=${OPENBOX_VERSION}-*" \
        xfonts-base x11-utils x11-xserver-utils xauth \
 && rm -rf /var/lib/apt/lists/* \
 && install -d -m 1777 /tmp/.X11-unix \
 && Xvnc -version 2>&1 | grep --fixed-strings "TigerVNC ${TIGERVNC_VERSION} " > /dev/null \
 && test "$(openbox --version | sed -n 1p)" = "Openbox ${OPENBOX_VERSION}"

# The release's version and the base image are named last: a build argument is
# part of what a step is made from, and a new version of the release would
# otherwise make every step above it again.
ARG BASE_IMAGE
ARG VERSION=dev

# This notice is written here so the recipe needs no build context.
RUN install -d /usr/share/doc/orch-env-base \
 && printf '%s\n' \
    '# Software in orch-env-base' \
    '' \
    'The image combines software under different terms. `LicenseRef-Orch-Env-Base`' \
    'in its image label refers to this notice; it does not grant one licence for' \
    'everything in the image. The Dockerfile and this notice are Apache-2.0.' \
    '' \
    '| Component | Terms and notices |' \
    '|-----------|-------------------|' \
    '| Google Chrome for Testing, downloaded by Playwright | Google'"'"'s executable distribution of Chrome, under the [Google Terms of Service](https://policies.google.com/terms) and [Chrome Additional Terms](https://www.google.com/chrome/terms/). It has no open source licence for the executable as a whole. The licences of its included open source components are available in `chrome://credits`; its terms are also in `chrome://terms`. It is not an Apache-2.0 distribution or the Chromium project. |' \
    '| Node.js | MIT for Node.js itself and separate licences for its incorporated dependencies. The complete bundled notice is `/usr/local/LICENSE`; [the upstream licence](https://github.com/nodejs/node/blob/main/LICENSE) identifies those components and their terms. |' \
    '| Playwright'"'"'s command line (`@playwright/cli`) and Playwright | Apache-2.0. Their installed packages, including dependencies, are under `/usr/local/lib/node_modules/@playwright/cli`; each package'"'"'s `LICENSE` and `NOTICE`, where supplied, apply to that package. [Playwright source and licence](https://github.com/microsoft/playwright). |' \
    '| FFmpeg, downloaded by Playwright for recording | LGPL-2.1-or-later. The downloaded build keeps its licence at `/opt/ms-playwright/ffmpeg-*/COPYING.LGPLv2.1`; its `ffmpeg-linux -L` command reports the build'"'"'s terms and configuration. Preserve that licence and make the corresponding source for the distributed build available. [Playwright'"'"'s FFmpeg build recipe](https://github.com/microsoft/playwright/tree/main/browser_patches/ffmpeg) identifies its source and build options. |' \
    '| TigerVNC, including Xvnc | GPL-2.0-or-later for TigerVNC'"'"'s own code, with separately licensed X server and library code. Debian'"'"'s complete copyright and licence record is `/usr/share/doc/tigervnc-standalone-server/copyright`, with records for its other packages beside it. [TigerVNC source](https://github.com/TigerVNC/tigervnc). |' \
    '| openbox | GPL-2.0-or-later, with the exceptions and third-party components described in `/usr/share/doc/openbox/copyright`. [openbox source](https://github.com/danakj/openbox). |' \
    '| Fonts | Noto CJK, Noto Color Emoji and Liberation use SIL Open Font License 1.1. DejaVu includes the Bitstream Vera licence and public-domain changes. GNU Unifont includes GPL terms with a font embedding exception; individual files and contributors have their own records. The authoritative records are `/usr/share/doc/fonts-noto-cjk/copyright`, `/usr/share/doc/fonts-noto-color-emoji/copyright`, `/usr/share/doc/fonts-liberation/copyright`, `/usr/share/doc/fonts-dejavu-core/copyright` and `/usr/share/doc/fonts-unifont/copyright`. |' \
    '| Debian packages and the distribution | Each package keeps its copyright and licence record under `/usr/share/doc/<package>/copyright`. Licence texts shared by packages are under `/usr/share/common-licenses`. There is no single licence covering all packages. [Debian copyright records](https://www.debian.org/doc/debian-policy/ch-docs.html#copyright-information). |' \
    '' \
    'The pinned versions and download locations are listed in the image README.' \
    'The source of Debian packages is available from Debian'"'"'s source archives and' \
    'the upstream projects; versions installed in this image can be listed with' \
    '`dpkg-query -W`. Keep the bundled licence and copyright files when distributing' \
    'the image, and apply each component'"'"'s own terms to its redistribution.' > /usr/share/doc/orch-env-base/NOTICE.md

# What is in the image, for whoever reads it from outside; the versions are the
# pins above, which the build has checked against what was installed.
LABEL org.opencontainers.image.title="orch-env-base" \
      org.opencontainers.image.description="The default working environment for Orch's agents" \
      org.opencontainers.image.version="${VERSION}" \
      org.opencontainers.image.licenses="LicenseRef-Orch-Env-Base" \
      orch.image.recipe.license="Apache-2.0" \
      org.opencontainers.image.base.name="${BASE_IMAGE}" \
      orch.image.node="${NODE_VERSION}" \
      orch.image.playwright-cli="${PLAYWRIGHT_CLI_VERSION}" \
      orch.image.playwright="${PLAYWRIGHT_VERSION}" \
      orch.image.browser="chromium" \
      orch.image.browser.revision="${BROWSER_REVISION}" \
      orch.image.browser.version="${BROWSER_VERSION}" \
      orch.image.tigervnc="${TIGERVNC_VERSION}" \
      orch.image.openbox="${OPENBOX_VERSION}"

# The shell the steps above ran with is the recipe's own; an image that starts FROM
# this one runs its steps with the default again.
SHELL ["/bin/sh", "-c"]

USER agent
ENV HOME=/home/agent
WORKDIR /workspace

# The driver starts the connector; with nothing started there is a shell.
CMD ["/bin/bash"]
