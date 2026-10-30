# The image a release's programs are compiled in: the distribution the Compose
# services run, with the pinned Rust toolchain and what building Orch needs.
#
# A program linked against a newer C library than the one it runs on does not
# start, so the server programs are built here and nowhere else. The same image
# builds the static programs (connector, node, orchctl) for the musl target:
# they are copied into machines of any distribution.
#
# `cargo xtask package` builds this image when it is missing and names it after
# the distribution and the toolchain, so a change to either makes a new image.

ARG RUNTIME_IMAGE=debian:bookworm-slim@sha256:7c7b2c966bc9ee8cedfeef67e0e279108992c77681fa595db4a9d65c06ccc587 # debian:bookworm-slim
FROM ${RUNTIME_IMAGE}

# The toolchain of rust-toolchain.toml; `package` passes its channel.
ARG RUST_TOOLCHAIN

# gcc, make and pkg-config for the crates that compile C (ring, zstd); the
# OpenSSL headers and static libraries for the web-push crate, which links
# OpenSSL (the build links them in, so the running image needs no libssl);
# musl-gcc for the same crates on the musl target; cmake and perl for aws-lc;
# git and curl because cargo and rustup use them.
RUN apt-get update \
 && apt-get install -y --no-install-recommends \
        build-essential pkg-config libssl-dev musl-tools cmake perl git curl ca-certificates \
 && rm -rf /var/lib/apt/lists/*

# The toolchain lives in the image, readable by any user: a build runs as the
# person who started it, so that what it writes can be removed by that person.
ENV RUSTUP_HOME=/opt/rustup \
    CARGO_HOME=/opt/cargo \
    PATH=/opt/cargo/bin:${PATH}
RUN curl --proto '=https' --tlsv1.2 -fsSL https://sh.rustup.rs \
      | sh -s -- -y --no-modify-path --profile minimal \
            --default-toolchain "${RUST_TOOLCHAIN}" --target x86_64-unknown-linux-musl --target aarch64-unknown-linux-musl \
 && chmod -R a+rX /opt/rustup /opt/cargo

# The toolchain is chosen here and not by rust-toolchain.toml, which would make
# rustup try to install missing parts into a directory the build may not write.
ENV RUSTUP_TOOLCHAIN=${RUST_TOOLCHAIN} \
    CC_x86_64_unknown_linux_musl=musl-gcc \
    CC_aarch64_unknown_linux_musl=musl-gcc

WORKDIR /work
