# Choose where agent program builds come from

Choose which build of each agent program Orch installs, where it is fetched from, and how Orch decides that a build works. At the end each agent program has a build policy that suits you, the builds are verified, and you can read the result of each build's checks.

Every agent program (Claude Code, Codex, DeepSeek Harness) has a manifest: its name in English and Chinese, how it is driven and launched, where its builds are published, what a build is checked for, the executors it brings and the files it reads. The programs Orch ships have their manifests in the release. A program that a connector's adapter plugin reports sends its own, and the system lists, checks and offers it like a shipped one. A reported program can never take the kind of a shipped program.

People choose a build for an environment, read its checks and update it from the screens: see [Agent program versions](../user/agent-program-versions.md). This page is for the operator who decides where the builds come from.

## Before you start

You need to be a system administrator. If this server cannot reach the vendors, plan a mirror or carry the builds in first ([Run Orch without internet access](offline.md)).

## 1. Choose the build policy

The system setting `agents.harness_versions` chooses, for each kind, the build that **ships** with the release, the **newest checked**, the **newest**, or an **exact** version. Organizations, projects and environments can override it, and a lock above prevents lower overrides ([Agent program versions](../user/agent-program-versions.md#pins-and-inheritance)).

Orch reads a program's channels only when a policy in force asks for newer builds, when `agents.harness_channel_hours` is due, or when a system administrator chooses **Refresh**. A release-check interval of zero turns the schedule off.

## 2. Know how a build is verified

A channel is one of these. A build the vendor gives nothing to verify is never kept, and a host's tool cache refuses a download whose checksum differs.

| Type | What is read | How a build is verified |
|------|--------------|-------------------------|
| GitHub release | The repository's newest releases, and one tag for an exact version | The SHA-256 GitHub keeps on the asset, or the checksum file of the release |
| npm package | The registry's document of the package | The tarball against the registry's integrity value, and the registry's signature against its published keys; the build then carries the SHA-256 Orch computed |
| PyPI project | The index's JSON API | The SHA-256 the index keeps for the file; yanked files are never kept |
| Manifest the vendor publishes | Each channel's file, then that version's manifest | The SHA-256 in the manifest, and the vendor's signature of it where the manifest declares one |

Where this server cannot reach a vendor, set `agents.harness_mirror` to a server that serves the same files under its own address, with each origin's host as the first part of the path. A mirror supplies bytes, never checksum authority ([Run Orch without internet access](offline.md#serve-agent-programs-from-an-internal-mirror)).

## 3. Read how a build is checked

A new build is checked once, in the background. A host of its system and processor puts it in its tool cache, asks it to print its interface, and runs the program's own test cases against a scripted model in a container with no network. A name that only one feature needs switches that feature off for the build. A name the adapter needs as a whole, or a failed case, fails the build. A program whose manifest lists no test case is checked by its interface alone.

Choose **Check** on the build to run it again, for example after you replace a build. Keep the complete result when you choose a recommended build.

**History import.** Whether a program can import history on its own is part of the check. A refusal of the import can pass its own check while `import_native` stays among the disabled features with an `unsupported` reason; it must write no session and make no model request.

- For a program that exports its history, a successful import must export two finished turns, start the same build in a fresh home, import the archive and read both original answers.
- Codex exports no file, because it keeps a thread's history in its own store. Its check makes three turns and forks the conversation through the second turn in the home where it was made, and reads the fork back as exactly those two turns. The fork's next request must carry them and not the third. It then stops the build, copies the home, starts the same build on the copy and resumes: the next request must carry all three answers.
- Every kept platform build of that kind and version must give that proof before **Carry** offers the program's own import. Missing or incomplete evidence keeps it off. A profile override cannot supply the evidence. A failure of an unrelated optional case does not erase a finished proof, but its own disabled features stay disabled.

People read the method in their fork or switch plan ([Carry a thread to another agent](../user/carry-a-thread.md#read-the-method)).

Codex's provider configuration takes `initialize_timeout_secs`: a positive wait for `initialize`, in seconds, 120 by default. Exceeding it ends the process and reports the failure with its exit reason.

## What the manifest tells Orch

The machine policy's executor choices combine the release's executor declarations with the executor plugins that machine has described. The page keeps a saved offer that is no longer reported visible, so you can keep or clear it. A profile's technical details use the program's executor declarations first, then the shared catalogue, and show one page of arrangements and tool sets at a time. Tool-set titles must be declared text in the reader's language; an identifier does not supply a missing title.

A descriptor lists the approval modes with its default first. A new thread inherits the place's mode when the program supports it, and otherwise starts with the first declared mode that is not **Auto**. Allowed-mode policies, setting locks and environment confinement still apply. **Auto** is never chosen as a fallback default.

## Add a program Orch does not ship

A program is a manifest (data), an adapter for its protocol (code) and, where it has a plugin system, a companion plugin. If it speaks a protocol Orch already has an adapter for, a manifest is enough. Otherwise it needs an adapter plugin that sends the manifest in its `hello`. Register the plugin (`orchctl plugin add`, `[[plugin]]` in the connector's configuration, or the machine's **Programs** page), and add its builds to the file of shipped builds or let a channel find them. The contract is in the [adapters guide](../development/extensions/adapters.md).

## If it does not work

| You see | What to do |
|---|---|
| A build shows **Untested** | No host can run its check. Connect a host of its platform, or import the build's checksums if you are offline, then choose **Check** |
| A build fails its checks | Open the status: it names the missing interface names, the failed cases and the features that are off. Use another build, or leave the feature off |
| A download is refused | Its checksum differs from the one verified. Do not override it; check the mirror or fetch the build again |
| A newer build is never offered | The policy in force does not ask for newer builds, or the channel cannot be read. Choose **Refresh** and check the mirror |
