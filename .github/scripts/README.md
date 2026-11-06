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
`release_image.py` builds and checks each native image's pinned labels before its
approved push. `release_publish.py` records the shared image index and publishes
the verified release. Published payloads and image tags are never replaced.

`ci_test_shard.py` uses Cargo's target list to split the API and engine
integration programs, and assigns every other workspace package to one shard.
`ci_web.py` discovers the complete browser suite on the checked-out commit,
shares one verified production build, and checks the actual reports for missing
or duplicate cases. Stateful files remain together; independent layout rows
run on separate workers in separate jobs.
Unit, example and documentation tests run as well. Shards keep going after a
test program fails, use a separate PostgreSQL service, and report their results
independently.

To warm release caches before tagging, dispatch the release workflow from the
default branch with `warm_caches` enabled and the intended future tag. It builds
the same native, frontend and package jobs from that branch commit but never
publishes a release or images. A normal release requires an existing tag,
matching commit, release notes and green CI.

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
