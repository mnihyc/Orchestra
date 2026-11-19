---
title: Agent program versions
screens: /admin/$section, /resources/agents/$id, /resources/environments, /resources/environments/$id, /resources/hosts/$id, /resources/providers, /resources/providers/$id, /project/$projectId/settings, /org/$slug, /thread/$threadId, /inbox
keywords: Codex, Claude Code, DeepSeek Harness, harness, version, compatibility, check, pin, exact, upgrade, downgrade, newest, shipped, mirror, offline, interrupt, batch
---

# Agent program versions

Open **Management → Agent programs**. Search the paged list by a program’s name
or identifier, then choose it. **Builds** lists its versions and checks.
**Capabilities** groups what its reported build supports, with a reason beside
each unavailable feature. **Settings** keeps the version policy, its resolved
build and current use together. Select a use count to see the environments
and provider runtimes holding that build. The shipped and newest checked builds
are separate facts; changing the default leaves the shipped build unchanged.

The shipped builds are Codex 0.165.0 and Claude Code 2.1.312.
Claude Code's Bypass also allows dangerous deletes without asking. See
[How approvals work](how-approvals-work.md).

## Checks

Orch checks a build in the background. It first checks the interface its
adapter uses, then runs the program's cases against a scripted model in an
isolated unit, without external network access or model credentials. Checks
happen once for an immutable build, never when a thread starts. **Check** runs
them again.

Select the status to read the missing interface names, native cases and
features switched off. Cases say what they prove, such as **Asks before
running a command**. A disabled feature says what changes with this build
and which fallback is used. The checker's diagnostic stays underneath.

Native import is checked per program. Where a program exports its history,
the check exports a short session and imports it into a fresh program home.
Codex keeps a thread's history in its own store, so its check forks a short
conversation at a turn in the home where it was made and reopens it in a copy
of that home. If a build fails its check, **Carry** names the build and says
native import is unavailable. The switch uses neutral
import where available, carrying the conversation as items. When that is
unavailable or the history will not fit, recent history goes in the handoff
notice and the full conversation travels as a file. This does not block Carry.
Profile overrides cannot restore native import that a build check disabled.

A Codex thread resumes and forks natively in the home where it was made, and in
a home that moves with its environment. Into a different home, **Carry** brings
the newest turns that fit the place's carry share of the new model's window, and
the full conversation as a file.

Resuming a Codex session keeps its original standing instructions. A change to
those instructions takes effect in the next new session. This expected resume
behavior does not fail a turn.

| Status | Meaning |
| --- | --- |
| Checking interface… | Reading the program's interface |
| Checking conformance… | Running the native cases |
| Checked | The checks passed, or this release certified the build |
| Untested | No completed check; the result says what is missing |
| Failed | A check failed; the disabled feature is shown first, with its case in the results |

A failed optional feature is unavailable with that build. Other features
continue to work. An untested or failed target has a warning in its update
plan. Operators can still choose it. A checksum mismatch is refused.
Exact input checkpoints are unavailable for Codex 0.165.0 while its native
goal-state and item-history checks remain unproved.

## Choose a policy

| Choice | Build selected |
| --- | --- |
| Shipped | The default build in the release |
| Newest checked | The newest build whose checks passed; otherwise the shipped default |
| Newest (untested) | The newest published build, with its actual check status |
| Exact | The version you name |

Exact offers known versions. Type another version and select **Fetch and
check** to read that release's declared source and follow cache verification
and the background check. Without an eligible check host it remains Untested
with the reason. Select **Use version** when the check ends. This saves the
Exact choice. Selecting Exact alone keeps a draft until you use the version.
An unavailable release says why; Orch does not substitute a different version.

## Pins and inheritance

The system supplies the policy. An org may override it, then a project or
folder, then an environment in its own **Settings**. A lock above prevents
lower overrides. Each program's choice inherits separately.

An agent profile's exact version takes priority. A thread never starts on a
different version from its profile pin. If the environment holds another
build, update it or choose a matching profile. Conflicting profile pins
cannot share one program installation in the same unit.

Choose **Inherit** for a program to remove its local pin while keeping the
other programs' choices. **Shipped** is an explicit choice of the default
build, which can differ from the policy above.

A connected personal machine keeps the program installed by its owner.
Orch shows the reported version. Its owner upgrades or downgrades it there.

An environment names each reported program even when its build has no catalogue check. An unknown check says
**Check unavailable**; a program whose name has not been declared says **Program name not reported**.

## Upgrade or downgrade

Changing a policy leaves running units as they are. **Update available**
means the unit's build differs from what it would receive now. A change
shows both versions and says **newer** or **older**.

For one environment, set Exact in its settings to the desired older or
newer build, then choose **Update…**. Review the target's check, what stays,
what stops and the affected threads before confirming.

For many, select environments or providers and choose **Update…**. A host's
**Runtimes** tab updates its units together. **Update N environments…** in Agent programs
selects the units behind their own effective policy. The plan lists skipped
units and their reasons. Each accepted unit gets its own job and rollback.

When a host or program includes both pools, choose **Update scope**:
environments or provider runtimes. Review and confirm each pool's batch.
Its progress continues after **Hide**. Choosing the other pool shows a fresh
plan and its current running turns.

## Running turns

The plan offers **Wait** with a duration in minutes, or **Interrupt now**.
Interrupt is the force option. The plan counts the running turns affected.
One choice applies to every member of a batch. Its initial value comes
from the existing change settings.

**Notify N people** sends one Inbox item per affected person for the batch.
Open the count to inspect its people. A notice names who requested the update,
counts the affected threads and opens affected work.

Updates preserve the environment's persistent files and conversations.
The plan names any processes or writable-layer changes that end. Its
**If it fails** row says how the previous unit is restored. Follow
the job's progress after confirming; hiding the dialog leaves it running.

## Mirrors and offline builds

Set **Release mirror** when vendor hosts cannot be reached. The mirror
serves the vendor's host and path under its own address. Exact fetches use
the same mirror. Scheduled channel reads occur only for policies asking
for newer builds; a zero release-check interval turns that schedule off.

An administrator can instead copy a build to an internal file server, then
add it through Agent programs with that URL and its SHA-256. Orch's existing
build-registration API accepts the same metadata. Run **Check**. The node
needs the preloaded isolated-check image to complete native checks.

## Built-in programs

### Codex

Its interface check examines the app-server schema used by its adapter.

### Claude Code

Its interface check examines the command-line flags used by its adapter.

### DeepSeek Harness

It uses the Agent Client Protocol. Its checks also verify the companion
features supplied by Orch; an unavailable extra does not hide the other
capabilities of the build.

Pause a Claude Code thread before exporting its native history. Orch streams large archives and refuses an export if its source files change during copying.
