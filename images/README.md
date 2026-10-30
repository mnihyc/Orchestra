# Container recipes

| Recipe | What it makes |
|--------|---------------|
| [orch-env-base](orch-env-base/README.md) | The image the Default template's environments are made from: a working machine for an agent, with `sudo` and no password for its user, a browser with Playwright's command line and the fonts it draws with, and a VNC server with a small window manager (about 1.6 GiB; its README lists the versions and what other parts of Orch may rely on) |
| [orch-builder](orch-builder/Dockerfile) | The image a release's programs are compiled in: the distribution the Compose services run, with the pinned Rust toolchain and what building Orch needs |
| [orch-embeddings](orch-embeddings/README.md) | The image of the provider template Local embeddings: llama.cpp's server with EmbeddingGemma 2 for search by meaning, built on the host that runs it, the server's build and the model checked by their SHA-256 and never part of a release |
| [harness-canary](harness-canary/Dockerfile) | The harness image of the daily canary: the newest Codex and Claude Code (or the versions a run names) on the environment image's Node.js, for the adapters' Docker tests; made for one run and never shipped |

The control plane itself runs on stock images (Debian and nginx) with the release mounted into them, and its
database on the pinned PostgreSQL image with the pgvector extension added, which Compose makes from
[deploy/postgres](../deploy/postgres/README.md); no other server image is made. The builder exists because a program compiled against a newer C library
than the one it runs on does not start: `cargo xtask package` builds the server programs in it, and the
static programs (connector, node, `orchctl`) for the musl target, and names the image after the distribution
and the toolchain, so that a change to either makes a new one.
