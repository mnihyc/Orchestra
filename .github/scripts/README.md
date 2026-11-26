# Release and CI helpers

The Python and shell helpers that the workflows in `.github/workflows` call. Each release job uses one helper. [Publish a release](../../docs/development/releasing.md) explains how to run a release; this page says what each helper does.

## Release helpers

| Helper | What it does |
|--------|--------------|
| `release_contract.py` | Records every sealed SQL source, the browser and key API roots and the catalogue of native targets, and supplies the matrices of the workflows. `seal-schema` writes `migrations/RELEASED`; `record` checks a clean `HEAD` against it |
| `release_gate.py` | Checks the exact tag and `HEAD`, and the release notes |
| `release_ci.py` | Waits for the most recent CI run of the exact tag and commit, and checks it again before publication |
| `release_native.py` | Unit-tests, builds and runs each connector on its own platform |
| `release_frontend.py` | Builds the browser application once and records the release identity and each file's SHA-256 |
| `release_package.py` | Checks that input, installs the locked npm packages for their notices, and puts all verified connectors into each server's downloads |
| `release_assemble.py` | Verifies each downloaded package against its manifest, requires one server of each Linux architecture, and rejects mixed commits and differing common files |
| `release_image.py` | Builds each environment image and checks its pinned labels, then checks the CI for the tagged commit and the remote tag again before its approved push |
| `release_rehearsal.py`, `release_warmup.py` | Pin a branch snapshot for a complete rehearsal that creates no tag and publishes nothing, and identify the default-branch build used only to warm caches |
| `release_source.py` | Checks that reused artifacts come from an earlier completed release workflow |
| `release_observe.py` | Observes the packaged files on their native runner and keeps two platform records |
| `release_publish.py` | Records the shared image index and publishes the verified release. Published files and image tags are never replaced |

Package checks take `--check-source`: the manifest commit must be the clean checkout, every schema file must match, and the deployment bundle must carry that checkout's edge, scripts, configuration and documentation. The release inputs carry size, BLAKE3, SHA-256, commit, protocol and executable-format checks.

The `scenarios` job of the release workflow (`scenarios.yml`) downloads the x86-64 package that `package` made and runs the scenarios of a fresh installation (`scripts/smoke/run-all.sh fresh`) against it. Assembly needs it. Only the scripted model server of two scenarios is built there.

## CI helpers

- `ci_test_shard.py` uses Cargo's target list to split the API and engine test programs, and gives every other workspace package to one shard. Each shard builds its selected test executables once from Cargo's JSON artifact messages and runs each binary on its own with a 45-minute default deadline. Set `ORCH_CI_TEST_BINARY_TIMEOUT_SECONDS` or pass `--binary-timeout-seconds` for another positive limit. A timeout stops only that binary's process group, reports the cases the harness still names as running and lists the unfinished ones, then continues with the rest of the shard. The shard fails if any binary fails. Unit, example and documentation tests run as well, in shards that use a separate PostgreSQL service and report separately. Doc tests still run through Cargo, and the engine unit run keeps its `--test-threads=3` command.
- `ci_web.py` discovers the complete browser suite on the checked-out commit, shares one verified production build, and checks the actual reports for missing or duplicate cases. Stateful files stay together. Independent layout rows run on separate workers in separate jobs.
- `ci_targeted.py` runs the checks that a `.ci-targets` file names, each one even after a failure, and fails when any failed. `ci_fixtures.py` prepares the images and static executables the assigned native tests need. `ci_apt_mirror.sh` puts package mirrors that answer now at the head of the runner's mirror list. `ci_memory_watch.py` reports sampled whole-runner memory around a command.
- `ci_memory.sh` adds 8 GiB of swap beside any swap the hosted runner has. It prints memory and disk capacity, refuses an existing swap path, and leaves the file to the runner's teardown. The Linux Rust gate and both static musl compilation jobs use it. The Linux Rust gate uses one Cargo job. Every Rust platform gate turns off development debug information and incremental compilation and has a 210-minute job limit. macOS and Windows keep Cargo's default concurrency and do not use the swap helper.

## Run and test them locally

The local equivalent of a job is the same Python command as in the workflow, with `ORCH_RELEASE_VERSION`, `ORCH_RELEASE_TAG`, `ORCH_BUILD_NUMBER_DIR` and `CARGO_TARGET_DIR` set explicitly. Run `release_native.py` on real macOS and Windows runners: cross-compiling from Linux is not evidence for a platform.

```sh
python3 -I -B -m unittest discover -s .github/scripts -p 'test_release_*.py'
```

The unit tests cover the gate and the rejection of mixed builds. They use tiny scripted archives, so they test packaging, not platform execution, and they push and publish nothing. Run `actionlint` on the three workflow files in an isolated, pinned tooling image in CI or release review.
