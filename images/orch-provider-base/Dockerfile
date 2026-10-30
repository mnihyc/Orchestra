# The provider runs the tool cache's static Codex and native Claude Code builds.
# C/C++ runtime libraries let Claude Code start without adding a shell or tools.
# Pin the multi-platform index so a host builds the same base on each processor.
FROM gcr.io/distroless/cc-debian13:nonroot@sha256:e792ab3d241a468a4fd7519ddbbebe66b49b5f365771716ea688ad40b6c6f1c2

LABEL org.opencontainers.image.title="Orch provider runtime" \
      org.opencontainers.image.description="C/C++ runtime libraries, certificates and an unprivileged account for the provider harnesses" \
      org.opencontainers.image.licenses="Apache-2.0" \
      org.opencontainers.image.base.name="gcr.io/distroless/cc-debian13:nonroot" \
      org.opencontainers.image.base.digest="sha256:e792ab3d241a468a4fd7519ddbbebe66b49b5f365771716ea688ad40b6c6f1c2"
