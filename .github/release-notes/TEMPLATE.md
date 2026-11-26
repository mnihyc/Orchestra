<!--
Template for the notes of a release. Copy it to <tag>.md in this directory (the release workflow publishes that file as the
release description and refuses a tag without it) and to the same place in CHANGELOG.md. Delete these comments when filling it.

Rules:
- One sentence per bullet, past tense or "can now", 30 words or fewer, with a link to the guide that explains it.
- Order is fixed: Upgrade notes, Breaking changes, Highlights, Changes, Known issues, Compatibility, Downloads.
- "Upgrade notes" is never empty: write "None." when nothing is needed.
- Never: batch or round numbers, branch names, candidate, integrator, proof, internal document names, decision
  references, the names of people, test counts, "first/last release", dates of measurements.
- Link with full addresses: a relative link does not work on a release page.
- The status line and the GitHub "pre-release" flag say the same thing.
-->

# Orch X.Y.Z

One sentence: what this release is about, for the person who updates.

**Status:** Pre-release | Stable. **Released:** YYYY-MM-DD. [Upgrade notes](#upgrade-notes) · [Downloads](#downloads) · [Full list of changes](https://github.com/mnihyc/Orchestra/compare/vA.B.C...vX.Y.Z)

## Upgrade notes

What you must do before, during or after updating, in the order you do it. Each item: who is affected, what to do, what happens if you do not. Start with the command.

- Update with `./update.sh vX.Y.Z` in the deployment directory (see [Upgrade](https://github.com/mnihyc/Orchestra/blob/main/docs/operator/upgrades.md)). Take a backup first: `./update.sh backup`.
- **If you use <feature>:** do <this> before updating, because <consequence>.

Write **None.** and the update command when nothing else is needed.

## Breaking changes

- **<Area>:** what changed, in one sentence. **Migration:** what to do (a command, a setting, a link).

Write **None.** when there are none.

## Highlights

Three to six bullets for the person who reads only this part. What you can do now, not how it works.

- You can now <do something>. [Getting started](https://github.com/mnihyc/Orchestra/blob/main/docs/user/getting-started.md)

## Changes

### Added
- <One sentence.> [Getting started](https://github.com/mnihyc/Orchestra/blob/main/docs/user/getting-started.md)

### Changed
- <One sentence.>

### Fixed
- <What you saw, then that it no longer happens.>

### Security
- <What was exposed, who was affected, what is fixed. Link the advisory when there is one.>

### API
- <Endpoint or header, what changed, whether existing clients are affected.>

## Known issues

One bullet each: what you see, when, and the workaround or "No workaround".

- <Symptom.> <Workaround.>

## Compatibility

| | Tested |
|---|---|
| Claude Code | x.y.z |
| Codex | x.y.z |
| Server, node and managed environments | Linux x86_64, Linux ARM64 |
| Connectors | Linux, macOS (Intel, Apple silicon), Windows (x86_64, ARM64) |
| Database | PostgreSQL N |

Minimum agent program versions, and what happens with a newer one: <one sentence>.

## Downloads

| File | For |
|---|---|
| `orch-deploy-vX.Y.Z.tar.gz` | The deployment directory (install and update) |
| `orch-server-linux-<arch>.zip`, `orch-frontend-dist.tar.gz`, `orchctl-linux-<arch>-musl` | The server, the web application, the command line |
| `orch-connector-<os>-<arch>` | The connector for a person's own machine |
| `orch-node-linux-<arch>-musl` | The node for a Linux host |
| `version.json` | The list of files with their sizes and checksums; `update.sh` verifies every file against it before installing |

To verify a download by hand: <command>.
