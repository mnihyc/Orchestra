# Publishing releases

The manual workflow's `tag` input defaults to `v0.1.2`; an explicit input selects
another reviewed version. Tag pushes use the pushed tag. Every eligible version
uses the automatic green-only pipeline, with no version floor. Keep existing tags
immutable, including tags whose CI failed and whose release was never published.

Freeze a reviewed source snapshot before release qualification. Commit
`.github/release-notes/<tag>.md` and seal every current migration's SHA-384 in
`migrations/RELEASED` before making the snapshot commit:

```sh
python3 .github/scripts/release_contract.py seal-schema
```

The sealer changes only the ledger and refuses to rewrite a recorded checksum.
Review and commit that metadata with the candidate. The gate requires an exact
ledger of every SQL file; it does not substitute a file count for the schema.
Seal in the dedicated snapshot checkout. Before the first deployment is confirmed,
the development source's ledger stays empty. After committing the snapshot, run
the same read-only source check as the rehearsal gate:

```sh
python3 .github/scripts/release_contract.py record --commit "$(git rev-parse HEAD)" \
  --tag v0.1.2 --mode rehearsal --out .tmp/release-inputs.json
```

It requires clean HEAD, checks the full SQL set and SHA-384 bytes against the
committed ledger, and records the schema, API roots and platform inputs. Actual
release runs use `--mode release`, which also requires that seal.
The [migration policy](../../migrations/README.md) explains released-file immutability.
An existing database with different SQLx checksums is not upgraded by rewriting
its history; qualify a compatible upgrade or restore before deployment.

For a dry run without a tag, push the reviewed snapshot branch and record its
full commit SHA. From that clean checkout, set `REPO`, `SNAPSHOT` and `TAG` to the
repository, snapshot branch and intended unused version, then run:

```sh
SHA=$(git rev-parse HEAD)
gh workflow run ci.yml --repo "$REPO" --ref "$SNAPSHOT"
gh workflow run release.yml --repo "$REPO" --ref "$SNAPSHOT" \
  -f tag="$TAG" -f rehearsal=true -f warm_caches=false -f publish_images=false
gh run list --repo "$REPO" --workflow ci.yml --branch "$SNAPSHOT" --commit "$SHA"
gh run list --repo "$REPO" --workflow release.yml --branch "$SNAPSHOT" --commit "$SHA"
```

Use the returned run IDs with `gh run watch <id> --repo "$REPO" --exit-status`.
Keep the branch fixed. Rehearsal checks out the dispatch SHA and waits for the
newest CI of that exact branch and SHA to be a successful manual run, which
includes deployment scenarios. It refuses a used version tag naming another
commit and mixed rehearsal/cache/image modes. It uploads `release-inputs`, native
inputs, one frontend, two server packages and the assembled `verified-release`;
download the last with `gh run download <release-run-id> --repo "$REPO" -n verified-release`.
These are Actions artifacts, retained for 14 days, not a GitHub Release or GHCR
publication. A successful artifact build while CI is still pending is not a green
rehearsal. Required performance, browser, native and deployment acceptance evidence
must all name the same frozen candidate.

The catalogue declares Linux x86-64/ARM64, macOS Intel/Apple Silicon and Windows
x86-64/ARM64 connectors. Linux also supplies both node and server architectures.
Windows on ARM gets its own native connector, built on the `windows-11-arm` runner.
Each native job unit-tests, builds and runs its actual connector. Unknown catalogue
targets need an explicit native runner binding before the gate accepts them.

Every job checks out the gate SHA. `ORCH_RELEASE_VERSION` and `ORCH_RELEASE_TAG`
set the intended version even while rehearsal has no Git tag; the shared build
support requires them to agree. GitHub Actions reads each component's committed
build number, including inside the Linux builder. Scratch counter directories
keep local packaging from changing tracked inputs. Frontend/native input manifests
pin identity, commit and file hashes. Release checks verify those inputs, every
archive and the exact candidate schema/deployment sources; both server packages
must agree on shared files before assembly.

Only after qualification, create a new immutable version tag at that exact SHA
and push it through the reviewed publication process. This starts both full CI
(including deployment scenarios) and release packaging. Packaging can finish
while CI runs, but publication waits for the most recent CI run for the exact
tag and commit. The publisher checks CI and the remote tag again before changing
release assets. A green branch rehearsal cannot satisfy the tag's CI gate.
Failed or cancelled CI stops publication. If CI does not finish within five
hours, the wait fails; rerun Release after exact CI finishes green. A new source
commit requires renewed qualification and an unused version, never a moved tag.

A rerun can finish a draft left by an interrupted upload after checking every
existing asset. A matching published release is left alone, with current CI
rechecked before repairing Latest. The newest stable release is marked Latest;
older releases and prereleases do not displace it. Published payloads and image
tags are never replaced.

To reuse native and frontend artifacts, dispatch Release for the same tag with
the numeric `source_run_id` of an earlier completed Release run. It may have
failed after producing useful artifacts. Every input must match the tag, commit,
protocol and hashes, and exact-tag CI must still be green. A cache-only warm-up
from the default branch can build ahead of the freeze, but skips final assembly
and publishes nothing. It does not qualify a snapshot.

Environment images are published separately. Configure required reviewers for
the `release-images` environment, then dispatch Release for the tag with
`publish_images` enabled. The approved run verifies assembled assets, builds each
Linux image on its own processor and rechecks exact-tag CI and the remote tag
before the first architecture push. The final job verifies both image records,
publishes their shared index and adds its digest to the release manifest. Image
publication cannot be combined with repackaging, rehearsal or cache warming.
