# Release automation helpers

The release workflow uses one helper per job: `release_gate.py` checks the exact
tag/HEAD and release notes; `release_ci.py` waits for the most recent CI run of
the exact tag and commit, and checks it again before publication;
`release_native.py` unit-tests, builds and runs each connector on its actual
platform; `release_frontend.py` builds the browser application once and records
the release identity and each file's SHA-256; `release_package.py` checks that
input, installs the locked npm packages for their notices, and puts all verified
connectors into each server's downloads; `release_assemble.py` verifies each downloaded
package against its original manifest, requires one server of each Linux architecture,
and rejects mixed commits and differing common files.
The deployment scenarios are not compiled for: the `scenarios` job of the release workflow
(`scenarios.yml`, one definition) downloads the x86-64 package that `package` made and runs the scenarios of
a fresh installation (`scripts/smoke/run-all.sh fresh`) against it, and `assemble` needs it. Only the scripted model
server of two scenarios is built there. The scenarios that prove an update, a rollback or a recovery need an
earlier real release, which this first release does not have: they are a reported step that never fails the job,
and run only when an earlier run's package is named (`earlier_run_id`). Dispatch the release workflow with
`scenarios_run_id` to try the scenarios alone against the package of an earlier run, with the scripts of the dispatched
branch and none of the other jobs: `gh workflow run release.yml --ref BRANCH -f scenarios_run_id=RUN_ID`.
`release_contract.py` records every sealed SQL source, the browser/key roots and
the native target catalogue, and supplies the workflow matrices.
Release package checks add `--check-source`: the manifest commit must be the clean
checkout, every schema file must match, and the deployment bundle must carry that
checkout's edge, scripts, configuration and documentation.
`release_image.py` builds and checks each native image's pinned labels, then
rechecks exact-tag CI and the remote tag before its approved push.
`release_publish.py` records the shared image index and publishes
the verified release. Published payloads and image tags are never replaced.

The manual `tag` input defaults to `v0.1.2` and can be overridden for another
reviewed version. Tag pushes use their actual tag. Automatic green-only
publication has no minimum release version.

`ci_test_shard.py` uses Cargo's target list to split the API and engine
integration programs, and assigns every other workspace package to one shard.
`ci_web.py` discovers the complete browser suite on the checked-out commit,
shares one verified production build, and checks the actual reports for missing
or duplicate cases. Stateful files remain together; independent layout rows
run on separate workers in separate jobs.
Unit, example and documentation tests run as well. Shards keep going after a
test program fails, use a separate PostgreSQL service, and report their results
independently.

Each shard builds its selected test executables once from Cargo's JSON artifact
messages, then runs each binary separately with a 45-minute default deadline.
Set `ORCH_CI_TEST_BINARY_TIMEOUT_SECONDS` or pass `--binary-timeout-seconds` to
`ci_test_shard.py` to choose another positive limit. The default leaves headroom
over scale binaries that take about 30 minutes, within the 180-minute shard job.
A timeout stops only that binary's own process group, reports the cases the
harness still names as running and separately lists unfinished cases, then
continues with the rest of the shard. Output remains streamed under Cargo's
usual `Running` and `test result:` lines; the shard fails if any binary fails.
Package working directories, build-script environment, binary fixtures and
library search paths follow the Cargo artifacts; database and stack settings
remain inherited. Doc tests still run through Cargo, and the engine unit run
keeps its `--test-threads=3` command.

The Linux Rust gate and both static musl compilation jobs use `ci_memory.sh` to
add 8 GiB of swap beside any swap the hosted runner already has. The helper
prints memory and disk capacity, refuses an existing swap path, and leaves the
file to the ephemeral runner's teardown. The Linux Rust gate also uses one
Cargo job. Every Rust platform gate disables development debug information and
incremental compilation and has a 210-minute job limit, leaving 45 minutes
above the slowest completed platform leg. macOS and Windows retain their
CPU-default Cargo concurrency and do not use the Linux swap helper.
The static jobs keep their optimized release compilation and one Cargo job;
release packaging and its compiler settings are unchanged.

For a complete snapshot dry run, seal the schema before committing the snapshot,
commit the intended version's release notes, then manually dispatch CI and Release
on that branch with `rehearsal` enabled. Release pins the dispatched SHA, requires
that branch's newest matching CI to be green, builds all five native targets and both
server architectures, runs the deployment scenarios against the x86-64 package, and uploads the
assembled `verified-release` artifact. It creates no tag, GitHub Release or GHCR
image. A version tag already naming another SHA is refused. The
[operator release guide](../../docs/operator/releases.md) has the command sequence.

To warm release caches before tagging, dispatch the release workflow from the
default branch with `warm_caches` enabled and the intended future tag. It builds
the same native, frontend and package jobs from that branch commit but never
publishes a release or images; it skips final assembly and can use an unsealed
schema. A normal release requires an existing tag, matching commit, sealed schema,
release notes, green exact-tag CI and the deployment scenarios of the release's own package. Rehearsal and warm-up
cannot be combined or used to enable image publication.

To repackage, dispatch the release workflow with the same tag and the numeric
`source_run_id` of an earlier, completed release run. It can have failed after
producing its native artifacts. The gate still checks the tag and CI, and every
native and frontend input must match the tag, commit, protocol and file hashes.
The artifacts are retained for 14 days. A run from before the shared frontend
was introduced can still supply native inputs: each package job builds and
checks its own frontend in that case. Image publication is a separate approved
run and cannot be requested while repackaging.

Set the `release-images` environment to require an operator reviewer before
enabling `publish_images` on a manual run. A tag run publishes the release
without the default environment image. A later approved run checks the release's
tag and asset hashes before adding the image facts to its manifest. An interrupted
draft can be resumed when all existing assets match; a matching published release
is left alone. There is no push or publication in local helper tests.

The workflows use the repository's automatic `GITHUB_TOKEN`, passed to release
and issue commands as `GH_TOKEN`. They require no personal token or model
credential. CI reads repository contents; the harness canary also writes issues.
The release image jobs write packages, and the publish job writes release assets
and the shared image index. Repository Actions settings must permit the
permissions declared by those jobs. Configure the `release-images` environment
with required reviewers before enabling image publication; naming the
environment in a workflow does not establish that its approval rules exist.

Local job equivalents are the same Python commands in the workflow, with
ORCH_RELEASE_VERSION, ORCH_RELEASE_TAG, ORCH_BUILD_NUMBER_DIR and CARGO_TARGET_DIR
set explicitly. On actual macOS/Windows runners, run `release_native.py`; Linux
cross-compilation is not platform evidence. The release inputs carry size,
BLAKE3, SHA-256, commit, protocol and native executable-format checks. The
archive tests use tiny scripted bytes and prove packaging, not platform execution.
Run `python3 -I -B -m unittest discover -s .github/scripts -p 'test_release_*.py'`
for the gate and mixed-build rejection cases. Run actionlint against the three
workflow files in an isolated, pinned tooling image in CI or release review.
