# Publish a release

Check a reviewed snapshot of the source, tag it, and publish its verified files. At the end the release has its files, its checksums and, if you chose to publish them, its environment images, and no published file can be replaced.

## Before you start

You need:

- **A reviewed snapshot** of the source on its own branch, with the release notes committed as `.github/release-notes/<tag>.md` (see the [release notes template](../../.github/release-notes/README.md)).
- **An unused version tag.** A tag is immutable: a new source commit needs a new version, never a moved tag. Tags whose CI failed and whose release was never published stay as they are.
- **The migrations sealed** in `migrations/RELEASED`, so that a later change to a released migration is refused ([migration policy](../../migrations/README.md)).
- **Rights to run workflows** on the repository. The workflows use the automatic `GITHUB_TOKEN` and need no personal token or model credential. The repository's Actions settings must permit the permissions the jobs declare: CI reads repository contents, the release CI job may dispatch a missing check, the harness canary also writes issues, the image jobs write packages, and the publish job writes release assets and the shared image index.
- **For environment images:** the `release-images` environment configured with required reviewers. Naming an environment in a workflow does not create its approval rules.

## 1. Seal the schema

In the dedicated snapshot checkout, seal every current migration's SHA-384, commit the release notes, then commit the snapshot:

```sh
python3 .github/scripts/release_contract.py seal-schema
```

The sealer changes only the ledger and refuses to rewrite a recorded checksum. The release gate requires the exact ledger of every SQL file, not a count of files. An existing database with different SQLx checksums is never upgraded by rewriting its history: qualify a compatible upgrade or restore before deployment.

After committing, run the read-only source check the rehearsal gate runs:

```sh
python3 .github/scripts/release_contract.py record --commit "$(git rev-parse HEAD)" \
  --tag "$TAG" --mode rehearsal --out .tmp/release-inputs.json
```

It needs a clean `HEAD`, checks the SQL files and their SHA-384 bytes against the committed ledger, and records the schema, the API roots and the platform inputs. A real release uses `--mode release`, which also requires the seal.

## 2. Rehearse

A rehearsal builds everything and publishes nothing. Push the snapshot branch and note its full commit SHA. From that clean checkout, set `REPO`, `SNAPSHOT` and `TAG` to the repository, the branch and the intended unused version:

```sh
SHA=$(git rev-parse HEAD)
gh workflow run release.yml --repo "$REPO" --ref "$SNAPSHOT" \
  -f tag="$TAG" -f rehearsal=true -f warm_caches=false -f publish_images=false
gh run list --repo "$REPO" --workflow ci.yml --branch "$SNAPSHOT" --commit "$SHA"
gh run list --repo "$REPO" --workflow release.yml --branch "$SNAPSHOT" --commit "$SHA"
```

Watch each run: `gh run watch <id> --repo "$REPO" --exit-status`. Keep the branch fixed.

**What you will see.** The rehearsal checks out the dispatched SHA and waits for the newest CI of that exact branch and SHA to be green. It builds all five native targets and both server architectures, runs the deployment scenarios of a fresh installation against its own x86-64 package (the `scenarios` job, which assembly waits for), and uploads `release-inputs`, the native inputs, one frontend, two server packages and the assembled `verified-release`. Download the last with `gh run download <release-run-id> --repo "$REPO" -n verified-release`. These are Actions artifacts kept for 14 days. They are not a GitHub Release or an image publication. A successful artifact build while CI is still pending is not a green rehearsal.

The rehearsal refuses a version tag that already names another commit, and refuses to mix rehearsal, cache warming and image publication.

The catalogue declares Linux x86-64 and ARM64, macOS Intel and Apple Silicon, and Windows x86-64 and ARM64 connectors. Linux also supplies both node and server architectures. Windows on ARM has its own connector, built on the `windows-11-arm` runner. Each native job unit-tests, builds and runs its actual connector, and a target not in the catalogue needs an explicit native runner binding before the gate accepts it.

## 3. Tag and publish

1. After the rehearsal and the other acceptance results all name the same snapshot, create a new tag at that exact SHA and push it through your reviewed publication process.
2. The tag starts release packaging. The release uses the newest full branch-push or manually dispatched CI run for the tagged commit. If that commit has no such run, the release starts one on the tag. Packaging can finish while CI runs, but publication waits for the most recent CI run for that commit, and the publisher checks CI and the remote tag again before it changes release assets. Failed or cancelled CI stops publication. If CI does not finish within five hours, the wait fails: run Release again after exact CI is green.

A rerun can finish a draft left by an interrupted upload, after checking every existing asset. A matching published release is left alone, and current CI is checked again before repairing **Latest**. The newest stable release is marked **Latest**; older releases and prereleases do not displace it. A version with a pre-release part (`v1.2.3-rc.1`), or whose notes give its status as `**Status:** Pre-release.`, is published as a prerelease. Published files and image tags are never replaced.

## Reuse earlier builds

To reuse native and frontend artifacts, dispatch Release for the same tag with `source_run_id` set to the number of an earlier completed Release run (it may have failed after producing useful artifacts). Every input must match the tag, commit, protocol and hashes, and CI for the tagged commit must still be green. The artifacts are kept for 14 days. A run from before the shared frontend can still supply native inputs: each package job then builds and checks its own frontend.

To warm the caches before the snapshot is frozen, dispatch Release from the default branch with `warm_caches` enabled and the intended future tag. It builds the same native, frontend and package jobs but publishes nothing, skips final assembly, and may use an unsealed schema. It does not qualify a snapshot.

To try only the deployment scenarios against the package of an earlier run, with the scripts of a branch and none of the other jobs: `gh workflow run release.yml --ref BRANCH -f scenarios_run_id=RUN_ID`. The scenarios that exercise an update, a rollback or a recovery need an earlier real release, so they run when an earlier run's package is named (`earlier_run_id`), and a failure there is reported without failing the job.

## Publish the environment images

Images are published separately, after the release.

1. Check that `release-images` has required reviewers.
2. Dispatch Release for the tag with `publish_images` enabled.
3. The approved run verifies the assembled assets, builds each Linux image on its own processor, and checks CI for the tagged commit and the remote tag again before the first architecture push. The last job verifies both image records, publishes their shared index and adds its digest to the release manifest.

A tag run publishes the release without the default environment image. Image publication cannot be combined with repackaging, rehearsal or cache warming.

## If it does not work

| You see | What to do |
|---|---|
| The gate refuses the tag | The tag names another commit, or has no release notes, or the schema is not sealed. Use an unused version and check each |
| Publication waits and then fails | CI for the tagged commit did not finish green in five hours. Run Release again after it does |
| Assembly stops on a mixed commit | The server packages disagree on shared files, or an input names another commit. Rebuild from one SHA |
| A run is half done | Dispatch Release again for the same tag: it resumes a draft when every existing asset matches |

How the helpers work, and how to test them locally, is in [.github/scripts](../../.github/scripts/README.md).

## Deployment files

`deploy/FILE_SETS` describes the files an update brings (see [Upgrades](../operator/upgrades.md#deployment-files)):
its `generation` is raised whenever a file of the `main` set differs from the latest published
release's, and its `minimum` only when the programs of the release need a change in those files, with
the reason in the release notes. The gate refuses a candidate that changes the set and leaves the
generation, or leaves the set and raises it. Packaging adds the release's own rows to the copy in its
bundle. Once the release is published, add its rows to the repository's table, from its tag:

```sh
python3 .github/scripts/release_contract.py record-published --tag vX.Y.Z --revision vX.Y.Z
```

and commit the result with the next change: an installation whose files came from that release is
recognised by them. A published release whose rows are missing makes its unedited files look like edits.
