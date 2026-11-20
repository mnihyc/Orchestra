# Build numbers

`build-numbers/` holds one counter for each component that ships on its own: `server.txt`, `connector.txt`, `frontend.txt` and `cli.txt`. Each file holds one positive integer, which becomes the build number in the identity of that component's executable.

- A local build advances the counter and takes the new value.
- A build in GitHub Actions takes the committed value as it is, so every job of one workflow run agrees on the number.
- To build without moving the tracked counters, set `ORCH_BUILD_NUMBER_DIR` to another directory. [CONTRIBUTING.md](../CONTRIBUTING.md#development-checks) shows how.

The counters are read by `crates/tooling/build-support`, which every program calls from its `build.rs`. Packaging, manifest checksums and release validation are in `crates/tooling/xtask/src/release/` and `.github/workflows/release.yml`.
