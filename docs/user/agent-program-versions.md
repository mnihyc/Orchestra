---
group: operations
title: Agent program versions
screens: /admin/$section, /resources/agents/$id, /resources/environments, /resources/environments/$id, /resources/hosts/$id, /resources/providers, /resources/providers/$id, /project/$projectId/settings, /org/$slug, /thread/$threadId, /inbox
keywords: Codex, Claude Code, DeepSeek Harness, harness, version, compatibility, check, pin, exact, upgrade, downgrade, newest, shipped, mirror, offline, interrupt, batch
summary: Choose which version of an agent program runs, read its compatibility checks, and upgrade or downgrade it.
---

# Agent program versions

Each release of Orch comes with a default build of every built-in agent program. The release notes name them. This page shows how to read what Orch has checked, choose which build runs, and change it.

Open **Management → Agent programs**. Search the paged list by the name or identifier of a program, then choose it.

- **Builds** lists its versions and their checks.
- **Capabilities** groups what the reported build supports, with a reason beside each feature that is unavailable.
- **Settings** keeps the version policy, the build it resolves to and where it is used. Select the number of uses to see the environments and the managed providers that hold that build.

The default build and the newest build that was checked are separate facts. Changing the default does not change the build that comes with the release. Claude Code's **Bypass** also allows dangerous deletes without asking; see [How approvals work](how-approvals-work.md).

## Checks

Orch checks a build in the background, never when a thread starts. It checks once for each build, because a build never changes. **Check** runs the checks again.

1. It first checks the interface that Orch uses to drive the program.
2. It then runs the program's own test cases against a scripted model, in an isolated container with no network outside and no model credentials.

Select the status to read the interface names that are missing, the cases that failed and the features that are off. A case says what it proves, such as **Asks before running a command**. A feature that is off says what changes with this build and which fallback is used. The diagnostic of the checker stays underneath.

| Status | Meaning |
|---|---|
| Checking interface… | Reading the interface of the program |
| Checking compatibility… | Running the test cases of the program |
| Checked | The checks passed, or the release certified the build |
| Checked · N checks pending | The checks that ran passed. A later release added N checks that have not run yet. They run in the background by themselves, and nothing the build had shown is taken away meanwhile |
| Untested | No check has finished. The result says what is missing |
| Failed | A check failed. The feature that is off is shown first, with its case in the results |

A feature that is optional and failed is unavailable with that build; the other features keep working. A check that could not run is not a failure. That happens when there is no host to run it, or when the node of a host is of an earlier release and lacks the check. The build keeps what its last check showed, and the check that did not run says so and waits for a host that can run it. The update plan warns about a target that is untested or failed, and an operator can still choose it. A build whose checksum does not match is refused.

**Importing history.** Whether a program can import history on its own is checked for each program. Where a program exports its history, the check exports a short session and imports it into a fresh home of the program. Codex keeps the history of a thread in its own store, so its check forks a short conversation at a turn in the home where it was made and reopens it in a copy of that home.

If a build fails this check, **Carry** names the build and says that the program's own import is unavailable. The switch then uses the neutral import where there is one, carrying the conversation as items. When that is unavailable too, or the history will not fit, recent history goes in the hand-over notice and the full conversation goes as a file. Carry is not blocked. A profile cannot turn back on an import that a check of the build turned off.

A Codex thread resumes and forks with its own history in the home where it was made, and in a home that moves with its environment. Into another home, **Carry** brings the newest turns that fit the carry share of the window of the new model, and the full conversation as a file. Resuming a Codex session keeps its original standing instructions; a change to them takes effect in the next new session, and that does not fail a turn.

## Choose a policy

| Choice | The build it selects |
|---|---|
| Release default | The default build that comes with the release |
| Newest checked | The newest build whose checks passed; otherwise the one that comes with the release |
| Newest (untested) | The newest build that was published, with its real check status |
| Exact | The version you name |

**Exact** offers the versions Orch knows. Type another version and select **Fetch and check**. Orch reads where that release declares its source, verifies what it caches, and runs the check in the background. Without a host that is allowed to run the check, the build stays **Untested** with the reason. When the check ends, select **Use version**, which saves the **Exact** choice. Selecting **Exact** alone keeps a draft until you use the version. A release that is unavailable says why; Orch never puts another version in its place.

## Pins and inheritance

The system sets the policy. An organization can override it, then a project or a folder, then an environment in its own **Settings**. A lock above prevents lower overrides. The choice of each program is inherited separately.

- **The exact version of an agent profile** has priority. A thread never starts on a version different from the pin of its profile. If the environment holds another build, update it or choose a profile that matches. Profiles with conflicting pins cannot share one installation of a program in the same container.
- **Inherit** removes the pin of one program here and keeps the choices for the other programs. **Release default** is a choice of the build that comes with the release, made on purpose, and it can differ from the policy above.

**A connected machine of a person** keeps the program its owner installed. Orch shows the version reported, and the owner upgrades or downgrades it there. Orch also reads the release channel of every program such a machine reports, and checks the newest build on a host by itself. A build that the program updates to is therefore checked soon after it is published, and it shows **Compatibility not verified** until that check is done.

- A newer build keeps these working, whatever its line: editing a message that was sent, continuing from an earlier version of one, ending a background command from Orch, and the record of web searches.
- Editing and continuing (Claude Code), ending a background command and steering a running turn (Codex), and **Auto** approval (Codex), stay on for a build that is not checked. Each is off only when the check of that exact build failed it, and the page of the build names it. When a later build has changed one of them and no check has shown it yet, the act fails with the error of the program itself.
- A build of the tested line is **Supported**. A later line shows **Compatibility not verified**.
- The program's own import of history with Codex needs a passing check of that exact build. Without one, the conversation is carried as items.
- A program older than the oldest build Orch tests does not start. The release notes name that build.

An environment names each program it reports, even when its build has no check in the catalogue. An unknown check says **Check unavailable**, and a program whose name was never declared says **Program name not reported**.

## Upgrade or downgrade

Changing a policy leaves running units as they are. **Update available**
means the unit's build differs from what it would receive now. A change
shows both versions and says **newer** or **older**. For Orch's own connector and tools it
shows the releases, such as 0.1.2 → 0.1.3, and says **a newer build** when only the build of one
release differs; build and checksum strings are not shown. A changed runtime definition says that it
changed, and its checksums are behind **Copy ID**.

For one environment, set **Exact** in its settings to the older or newer build you want, then choose **Update…**. Before you confirm, read the check of the target, what stays, what stops and the threads affected.

For many, select environments or providers and choose **Update…**. The **Containers** tab of a host updates its containers together. **Update N environments…** in **Agent programs** selects the containers behind their own policy in force. The plan lists the containers that are skipped and why. Each container that is accepted gets its own job and its own way back.

When a host or a program has both pools, choose **Update scope**: environments or managed providers. Review and confirm the batch of each pool. Its progress goes on after **Hide**. Choosing the other pool shows a fresh plan with its running turns.

## Running turns

The plan offers **Wait**, with a number of minutes, or **Interrupt now**. **Interrupt now** is the forceful choice. The plan counts the running turns it affects. One choice applies to every member of a batch, and its first value comes from the settings for changes.

**Notify N people** sends one Inbox item to each affected person for the whole batch. Open the count to see the people. The notice names who asked for the update, counts the affected threads and opens the affected work.

An update keeps the persistent files and the conversations of the environment. The plan names any processes or changes of the writable layer that end. Its **If it fails** row says how the previous container comes back. Follow the progress of the job after you confirm. Hiding the dialog leaves it running.

## Mirrors and offline builds

When the hosts of the vendors cannot be reached, set **Release mirror**. The mirror serves the host and path of the vendor under its own address. Exact fetches use the same mirror. The release channels are read on a schedule for the policies that ask for newer builds and for the programs that connected machines report. A release-check interval of zero turns that schedule off.

An administrator can instead copy a build to an internal file server and add it in **Agent programs** with its URL and its SHA-256. The API for registering builds takes the same details. Then run **Check**. The node needs the image for isolated checks already loaded to finish the checks of the program.

## Built-in programs

- **Codex.** Its interface check examines the schema of the app server that Orch uses.
- **Claude Code.** Its interface check examines the command-line flags that Orch uses. Pause a Claude Code thread before you export its history. Orch streams large archives and refuses an export if its source files change while it copies.
- **DeepSeek Harness.** It uses the Agent Client Protocol. Its checks also verify the companion features Orch supplies. An extra that is unavailable does not hide the other capabilities of the build.
