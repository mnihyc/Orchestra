# The harness image of the daily canary (.github/workflows/harness-canary.yml): the newest builds of Codex and
# Claude Code, or the versions the run is given, on the Node.js release the environment image pins, so that the
# adapters' Docker tests hold those builds exactly as they hold the pinned ones in `orch-dev-harness`. Nothing of
# it is shipped; it is made for one run and left with the runner.
ARG NODE_IMAGE=node:24.21.0-bookworm-slim@sha256:d6aa754f16b3197301076f047b5def2f02ea1dbbc2ca920407d46d7ec7f87b20 # node:24.21.0-bookworm-slim
FROM ${NODE_IMAGE}

ARG CODEX_VERSION=latest
ARG CLAUDE_VERSION=latest

RUN npm install --global --no-fund --no-audit \
        "@openai/codex@${CODEX_VERSION}" \
        "@anthropic-ai/claude-code@${CLAUDE_VERSION}" \
    && npm cache clean --force \
    && codex --version \
    && claude --version
