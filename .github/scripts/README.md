# Release automation helpers

The release workflow uses one helper per job: `release_gate.py` checks the exact
tag/HEAD, semantic version ordering, release notes and the most recent CI result
(a tag listed with its reason in `.github/release-ci-waivers.txt` may go without green CI;
the gate warns, and every other check holds);
`release_native.py` unit-tests, builds and runs each connector on its actual
platform; `release_package.py` puts all verified connectors into each server's
downloads; `release_assemble.py` rejects mixed commits and differing common files.
`release_image.py` builds and checks each native image's pinned labels before its
approved push. `release_publish.py` records the shared image index and creates a
draft. Existing published releases and image tags are never replaced.

Set the `release-images` environment to require an operator reviewer before
enabling `publish_images` on a manual run. A tag run makes a reviewable draft
without publishing the default environment image; its notes explicitly say that
publication is pending. A later approved run may complete only the image facts
of that exact draft after verifying every existing asset's SHA-256. Final release
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
