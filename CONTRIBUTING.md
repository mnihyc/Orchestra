# Contributing

Read the [architecture guide](docs/development/architecture.md) before adding
a package. Each concept has one home; a new driver or agent implementation
belongs behind its contract. The [component inventory](docs/development/components.md)
records the component homes and the remaining reserved implementations. Work
behind the existing contracts and keep each decision in its assigned layer.

## Development checks

The pinned Rust toolchain includes rustfmt and Clippy. Dependency checks use
`cargo-deny` 0.20.2 (install with `cargo install cargo-deny --version 0.20.2 --locked`).

```sh
cargo xtask verify
```

This runs formatting, Clippy with warnings denied, compilation of all workspace
targets, rustdoc with warnings denied, and `cargo deny`.
The individual commands are:

```sh
cargo fmt --all -- --check
cargo clippy --workspace --all-targets --locked -- -D warnings
cargo check --workspace --all-targets --locked
cargo doc --workspace --no-deps --locked
cargo deny --locked check
```

Set `RUSTDOCFLAGS="-D warnings"` when invoking the documentation check on its
own. CI repeats the Rust checks on Linux, macOS (Arm64 and Intel) and Windows and cross-builds
the CLI and machine executables for the declared Linux musl targets.
The frontend uses Node 24 and its lockfile. Run `npm ci`, then `npm run
typecheck`, `npm run lint`, `npm test` and `npm run test:e2e` inside `frontend/`;
`npm run build` produces its distributable application. A local pass does not establish a pass
on another platform or a completed product integration gate.

## Changes

A released migration is never edited: a change is a new numbered file. `migrations/RELEASED`
records the SHA-384 checksum and filename of each migration shipped at a tag (the same digest SQLx
records). The release integrator fills this ledger at the tag; until the first tag it is empty.
The `orch-db` released-migration test refuses a missing or changed listed file. Preserve earlier
ledger rows when adding a release. Development databases made before the first release must be
recreated; changing their applied checksum is not an upgrade path.

- Inherit package metadata, lints and dependencies from the root manifest.
  Commit `Cargo.lock`. Explain each new dependency in the change description.
- Keep executables small. Put reusable behavior in the appropriate library
  layer and concrete extension implementations under `extensions/<kind>/`.
- Return typed errors from libraries. Production code avoids `unwrap`,
  `expect`, unchecked unsafe code and unfinished branches. An exceptional
  lint allowance must include its reason. Reserved extension modules are labelled
  and must remain unavailable until they implement and prove their contract.
- Add tests only for business rules and observable workflow, concurrency and
  failure behavior. Do not test file layout, code structure or implementation
  shape. Native tests that lack their required runner must record an unrun result;
  fixture evidence remains distinct from a real harness or deployment proof.
  Test doubles must not be described as proof of an actual provider or service.
- Update public documentation with the change. Keep credentials, runtime
  state, local planning documents and scratch experiments out of commits.

## Build identity

`orchctl --version` reports the release version. `orchctl version` also shows
the component build number; `orchctl version --json` returns the same
compiled identity for scripts. Runtime environment variables cannot replace it.

`ORCH_RELEASE_VERSION` overrides the workspace version. `ORCH_RELEASE_TAG`
or a GitHub Actions tag supplies the release tag; if both a version and a
tag are provided they must agree. Versions use `MAJOR.MINOR.PATCH`, optionally
with a prerelease suffix. Build metadata is kept in the separate counter.

Counter files distinguish separately shipped components. Rust entry points use
the build-support helper and the frontend embeds the identity of its build. A local
build-script invocation advances its counter under an operating-system file
lock, then embeds the new value. Cargo caching determines when that script
runs; this is not a count of shell commands. Changes to the counter can
also cause the script to run again. Concurrent invocations receive different
numbers. GitHub Actions reads the committed number without advancing it, so
every target in one release agrees. Missing or malformed CI counters fail
the build.

For development that should leave committed counters alone, set
`ORCH_BUILD_NUMBER_DIR` to an **absolute** scratch directory, for example:

```sh
mkdir -p .tmp/build-numbers
ORCH_BUILD_NUMBER_DIR="$PWD/.tmp/build-numbers" cargo xtask verify
```

The Windows equivalent is `$env:ORCH_BUILD_NUMBER_DIR = "$PWD/.tmp/build-numbers"`.
Lock files remain in the counter directory and are ignored by Git. Do not
remove one while a build is running: waiters must lock the same file. Locks
are released by the OS when the holder exits, including on a crash. A failed
write preserves the previous counter; these local counters are not a
distributed identifier or a guarantee against storage hardware failure.

## Release policy

GitHub Actions is the authority for distributed builds. Packaging, installers,
the updater, deployment smoke scenarios and harness canaries have their own
gates. The scheduled workflow's current status belongs to its workflow file;
the native conformance runner is `scripts/harness-canary/conformance` and retains
the exact builds and named results. A failing, unrun or fixture-only gate is
reported explicitly before a release is certified or artifacts are published.
