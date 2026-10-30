# Release automation helpers

The release workflow uses one helper per job: `release_gate.py` checks the exact
tag/HEAD, semantic version ordering, release notes and the most recent CI result
(a tag listed with its reason in `.github/release-ci-waivers.txt` may go without green CI;
the gate warns, and every other check holds);
`release_native.py` unit-tests, builds and runs each connector on its actual
platform; `release_frontend.py` builds the browser application once and records
the release identity and each file's SHA-256; `release_package.py` checks that
input, installs the locked npm packages for their notices, and puts all verified
connectors into each server's downloads; `release_assemble.py` verifies each downloaded
package against its original manifest, requires one server of each Linux architecture,
and rejects mixed commits and differing common files.
`release_image.py` builds and checks each native image's pinned labels before its
approved push. `release_publish.py` records the shared image index and creates a
draft. Existing published releases and image tags are never replaced.

`ci_test_shard.py` uses Cargo's target list to split the API and engine
integration programs, and assigns every other workspace package to one shard.
Unit, example and documentation tests run as well. Shards keep going after a
test program fails, use a separate PostgreSQL service, and report their results
independently.

To warm release caches before tagging, dispatch the release workflow from the
default branch with `warm_caches` enabled and the intended future tag. It builds
the same native, frontend and package jobs from that branch commit but never
creates a draft or publishes images. A normal release still requires an existing
tag, matching commit, release notes and green CI or a recorded waiver.

To repackage, dispatch the release workflow with the same tag and the numeric
`source_run_id` of an earlier, completed release run. It can have failed after
producing its native artifacts. The gate still checks the tag and CI, and every
native and frontend input must match the tag, commit, protocol and file hashes.
The artifacts are retained for 14 days. A run from before the shared frontend
was introduced can still supply native inputs: each package job builds and
checks its own frontend in that case. Image publication is a separate approved
run and cannot be requested while repackaging.

Set the `release-images` environment to require an operator reviewer before
enabling `publish_images` on a manual run. A tag run makes a reviewable draft
without publishing the default environment image; its notes explicitly say that
publication is pending. Before publishing the image index, a later approved run checks that an existing draft's tag
still names the same commit and that every existing asset has the expected SHA-256. It may complete only that draft's
image facts. Final release
publication remains the operator's act. There is no push or publication in local
helper tests.

The workflows use the repository's automatic `GITHUB_TOKEN`, passed to release
and issue commands as `GH_TOKEN`. They require no personal token or model
credential. CI reads repository contents; the harness canary also writes issues.
The release image jobs write packages, and the draft job writes release assets
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
