# Local embeddings: llama.cpp's server with EmbeddingGemma 2 (Q8_0), answering the Embeddings format on the
# runtime's own loopback, for search by meaning. The server's build and the model are fetched on the host that
# builds this image and checked by their SHA-256 before anything uses them; neither is part of Orch. It builds
# with the classic builder and BuildKit alike, for x86_64 and arm64.
FROM debian:bookworm-slim@sha256:7c7b2c966bc9ee8cedfeef67e0e279108992c77681fa595db4a9d65c06ccc587 AS fetch
ADD https://github.com/ggml-org/llama.cpp/releases/download/b11514/llama-b11514-bin-ubuntu-x64.tar.gz /tmp/llama-x86_64.tar.gz
ADD https://github.com/ggml-org/llama.cpp/releases/download/b11514/llama-b11514-bin-ubuntu-arm64.tar.gz /tmp/llama-aarch64.tar.gz
ADD https://huggingface.co/ggml-org/embeddinggemma-2-GGUF/resolve/58faf90940886df2d62be555c8765d00dac362c5/embeddinggemma-2-Q8_0.gguf /opt/models/embeddinggemma-2-Q8_0.gguf
RUN printf '%s  %s\n' \
        b2b617fb28e9d1444235c30bb09a3704789a1840e16ead58a5f7be6eeae8beef /tmp/llama-x86_64.tar.gz \
        54ad6ca00d11b9fbeea134aa5b13642b8b4f840188d2f6929d08e49cbce16ea9 /tmp/llama-aarch64.tar.gz \
        2188ac1deca4b77dffefd603c2776a9d76d9d74ec01841392982ebb840b09135 /opt/models/embeddinggemma-2-Q8_0.gguf \
      | sha256sum -c - \
 && mkdir -p /opt/llama \
 && tar -xzf "/tmp/llama-$(uname -m).tar.gz" -C /opt/llama --strip-components=1 \
 && chmod 0644 /opt/models/embeddinggemma-2-Q8_0.gguf \
 && printf '%s\n' \
      '#!/bin/bash' \
      '# Starts the server in the background on the address its LLAMA_ARG_* variables name, then waits until it' \
      '# answers, for at most EMBEDDINGS_START_SECS seconds; its output goes to /tmp/llama-server.log.' \
      'set -u' \
      'LD_LIBRARY_PATH=/opt/llama nohup /opt/llama/llama-server > /tmp/llama-server.log 2>&1 < /dev/null &' \
      'for (( waited = 0; waited < ${EMBEDDINGS_START_SECS:-120}; waited++ )); do' \
      '  if { exec 3<> "/dev/tcp/${LLAMA_ARG_HOST:-127.0.0.1}/${LLAMA_ARG_PORT:-8080}"; } 2> /dev/null; then' \
      '    printf "GET /health HTTP/1.0\r\n\r\n" >&3' \
      '    read -r status <&3 || status=""' \
      '    exec 3>&-' \
      '    case "$status" in *" 200 "*) echo "the embeddings server answers"; exit 0 ;; esac' \
      '  fi' \
      '  sleep 1' \
      'done' \
      'echo "the embeddings server did not answer within ${EMBEDDINGS_START_SECS:-120} seconds:"' \
      'tail -n 5 /tmp/llama-server.log' \
      'exit 1' \
      > /opt/llama/serve \
 && chmod 0755 /opt/llama/serve

FROM debian:bookworm-slim@sha256:7c7b2c966bc9ee8cedfeef67e0e279108992c77681fa595db4a9d65c06ccc587
# The server's build needs the OpenMP runtime and OpenSSL 3, from the distribution's signed packages.
RUN apt-get update \
 && apt-get install -y --no-install-recommends libgomp1 libssl3 \
 && rm -rf /var/lib/apt/lists/*
COPY --from=fetch /opt/llama /opt/llama
COPY --from=fetch /opt/models /opt/models
LABEL org.opencontainers.image.title="Orch local embeddings" \
      org.opencontainers.image.description="llama.cpp's server (b11514) with EmbeddingGemma 2 Q8_0 for Orch's search by meaning" \
      org.opencontainers.image.licenses="Apache-2.0 AND MIT" \
      orch.image.llama-cpp="b11514" \
      orch.image.model="ggml-org/embeddinggemma-2-GGUF@58faf90940886df2d62be555c8765d00dac362c5:embeddinggemma-2-Q8_0.gguf"
