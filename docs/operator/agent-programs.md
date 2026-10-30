# Agent programs

Where an agent program's builds come from, how each is verified and checked, and how to add a program Orch does not
ship. A program is a manifest (data), an adapter for its protocol (code) and, where it has a plugin system, a companion
plugin. Shared code names none of them.

## What the system knows of a program

Every program has a manifest. The programs Orch ships have theirs in the release; a program a connector's adapter
plugin reports sends its manifest in its `hello`, and the system lists it, checks its builds and offers it like a
shipped one. Its kind is shown beside its name wherever an administrator reads the list, and it can never take the
kind of a shipped program, so a reported program cannot pass itself off as another.

A manifest says:
- the kind and the program's name in English and Chinese;
- which protocol adapter drives it, and how it is launched;
- where its builds are published, by type of place (below), and so whether one version can be fetched alone;
- what a build is checked for: the names of its interface, and the native cases;
- the executors it brings, the files it reads, and what Orch keeps it from doing unasked.

The machine policy's executor choices combine the release's executor declarations with executor
plugins that machine has described. Offer values and older aliases come from those declarations;
the page keeps an unreported saved offer visible so an operator can retain or clear it. Editing
another policy key does not read the executor list.

Profile technical details use the program's executor declarations first, then the shared executor
catalogue for other executors. Tool-set titles must be declared text in the reader's language;
an identifier does not supply a missing title. The details show one page of arrangements and
tool sets at a time.

A descriptor lists approval modes with its default first. New threads inherit the place's mode when the
program supports it; otherwise they start with the first declared mode other than Auto. Allowed-mode policies,
setting locks and environment confinement still apply. Auto is never selected as a fallback default.

## Where builds come from

`agents.harness_versions` chooses, for each kind, the shipped build, the newest checked, the newest, or an exact
version. Orch reads a program's channels only when an effective choice asks for newer builds, when the interval
`agents.harness_channel_hours` is due, or when a system administrator presses Refresh. A channel is one of:

| Type | What is read | How a build is verified |
|------|--------------|-------------------------|
| GitHub release | The repository's newest releases, and one tag for an exact version | The SHA-256 GitHub keeps on the asset, or the checksum file of the release |
| npm package | The registry's document of the package | The tarball against the registry's integrity value, and the registry's signature against its published keys; the build then carries the SHA-256 Orch computed |
| PyPI project | The index's JSON API | The SHA-256 the index keeps for the file; yanked files are never kept |
| Manifest the vendor publishes | Each channel's file, then that version's manifest | The SHA-256 in the manifest, and the vendor's signature of it where the manifest declares one |

A build the vendor gives nothing to verify is never kept, and a host's tool cache refuses a download whose checksum
differs. Where this server cannot reach a vendor, `agents.harness_mirror` names a server that serves the same files
under its own address, with each origin's host as the first part of the path.

## How a build is checked

Codex provider configuration accepts `initialize_timeout_secs`: a positive initialize wait in seconds, default 120; exceeding it still ends the process and reports the initialize failure with its exit reason.

A new build is checked once, in the background: a host of its system and processor puts it in its tool cache,
asks it to print its interface (`node.tool.check`), and runs the program's native cases against the scripted model in
a unit with no network (`node.tool.conformance`). A name only one feature needs switches that feature off for the
build; a name the adapter needs as a whole, or a failed case, fails the build. A program whose manifest lists no
native case is checked by its interface alone.

## Adding a program

1. If it speaks a protocol Orch has an adapter for, write its manifest and nothing else: the connector hosts it
   through that protocol's adapter. A program with no such protocol needs an adapter plugin (see the guide to
   agent adapters), which sends the manifest in its `hello`.
2. Register the plugin (`orchctl plugin add`, `[[plugin]]` in the connector's configuration, or the machine's
   Programs page). The connector gives it the tool cache's build of its program where the manifest says it needs one,
   and a home Orch manages, exactly as for a shipped program.
3. Add its builds to the file of shipped builds, or let a channel find them.

`cargo xtask names` fails the build when shared code names a program or a vendor of one.
