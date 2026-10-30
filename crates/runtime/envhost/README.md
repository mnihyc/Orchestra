# orch-envhost

Environment-side files, tools, processes and interactive services.

The release environment runtime is declared for Linux. macOS and Windows code
paths remain available for target-runner work, but attachment is refused until
their native containment, process, shell and reconnect checks have been proved.
Compiling another target is a prerequisite and does not certify those effects.

The instruction-root control directory is prepared empty under the local write policy, without replacing a file, link or user content. It is excluded from ordinary workspace reads, listings, manifests and snapshots even when ignore patterns ask to include it.

Files views and agents attaching workspace files use the same `fs.fetch` command.
Its optional `max_bytes` narrows the host's file limit; it cannot raise it. Paths
are relative to the environment's workspace or a named attached mount and obey
the same ignore and symlink containment checks. A pinned regular file is hashed
and streamed in bounded pieces through the command's content upload, including
an empty file. The result names the stored hash, size and version, never inline
bytes. A file that changes during reading or transfer is refused for a retry;
an outside path, a detached environment and an oversized file are refused before
upload. Registering, pinning and attaching the blob remain the caller's work
after its access and ordinary upload-kind checks.

The environment role of a connector is implemented: for each command about an environment there is one function that takes the command's body from the link contract and gives its result, for files, snapshots and moves, watches and manifests, shells that outlive their screens, processes, Git state, resource samples, mounts, executors and tool servers, and none of them can leave the workspace root, lose a newer version of a file or signal a process the environment did not start. Where content has to travel the functions take the two small traits of `blobs`, so the package never fetches or sends file content itself. Tool servers listen on loopback; the port socket factories connect to the currently owned local program. An executor is started by its kind and keeps the address and the token it answers at, and is started again on demand when it no longer answers. A tool server is run for the environment as a stdio child process whose MCP is offered as streamable HTTP on the loopback behind a token of its own, or is a server reached by an address; its tools are filtered as its definition says, its progress and its cancellations pass through, and one that crashes is started again within its budget and reported failed past it. Linux is complete and tested; the macOS and Windows files have the same functions and have been type-checked and linted for their targets but never run. The tool servers are tested with a stand-in that is the test program itself, started as a child, called, killed and started again.

The Linux port reader observes current IPv4/IPv6 TCP listeners and bound UDP in the environment's scope, keeps first-seen observations and shares one five-second watch. Its inside socket factories recheck ownership and report distinct UDP peers; real process and socket tests cover owned replacement, foreign refusal, Internal endpoints, half-close and zero-length datagrams. Other platforms explicitly report unsupported discovery.

Browsers are discovered with one sandbox probe, opened on kept profiles outside the workspace, joined by profile, and adopted only through checked local endpoints. One protocol connection supplies tabs, adaptive pictures, input and page questions; owned processes and auxiliary window displays end with the environment. Shipped-image tests exercise cookies, concurrent joins, confinement, shared Playwright control, native input, sharp still images and Linux connected policy; macOS and Windows browser use has not been run.

Source and publication operations keep their frozen choices and native receipts in the home-owned file-state store. Recorded states keep raw files, link text and executable bits independently of the project's Git index and refs; restore plans recheck the entire included namespace and retain a complete undo state. Worktrees have private files and index, and copying them out verifies an independent repository before a session opens. Unsupported external Git metadata blocks that copy with a reason.

`fs_record_capture` seals a state and its frozen reply, verifies the existing
root/manifest/tree relation and acquires an open bounded manifest plus a pin in
one recording critical section. Its bounded verification buffers are released
before returning; a waiting publication owns a file handle rather than the whole
manifest. Upload reads only that immutable file through the ordinary verified
content transport. The independent pin survives upload failure, cancellation and
terminal journal-write retries while its command owner is live. A durable pin
uses that command's existing recording budget for crash protection; live ownership
continues protecting collection even if finalization outlasts that timestamp.
Detach and destructive data import/removal refuse immediately with retryable
`busy` while such a lease exists. Ordinary workspace changes may proceed after
capture. A durable started receipt prevents an interrupted, missing or expired
state from capturing newer workspace bytes under its old identity. Complete
retries reopen the same root and source. File-state/store regression sources cover
binary data, failed-upload retry after mutation, corrupt/missing state, live and
crash-pin expiry, cancellation and collection after lease release.

Destructive admission and recorded-state pin admission use one store gate.
Detach reserves it before lifecycle hooks and retires that attachment's store
handle before removing the attachment; a delayed unkeyed read holding an older
handle cannot acquire a pin afterward. In-place replacement also fences old
handles, while already-admitted immutable publications keep their pins and input.
Import holds exclusive admission through
download, materialization and verification. Its blocking materialization clones
retain the reservation even if the async caller ends. No mutex is held across a
transfer, and a conflicting operation receives Busy rather than waiting on an
upload. A deliberate new attachment opens a new handle through the same physical
store gate.

Directory creation persists each parent entry before publishing descendants.
Existing directory names are flushed again when retried after a failed barrier;
object fanout and first-pin directories use that same mechanism. Metadata replay
also completes a previously refused directory barrier, and root recovery writes
the same root/reference durably before handoff. The focused durability sources
inject parent and post-rename directory-sync refusals and reopen retained raw
state after abrupt child-process exit. These are authored assertions, not a
claim of executed tests or simulated kernel-cache loss.

Portable moves measure and copy the declared home and closed recorded-state roots through the same
checked snapshots and materialization. Project ignore files cannot hide this data, connector identity
and credentials stay outside the recorded-state selection, and imports recheck the live write gate.
The raw-byte, journal, ignore-file and refusal regressions in `data_transfer` are authored and unrun.

The source suites cover actual Git/files, HTTPS authentication, publication, interrupted restore, expiry, safe cleanup and store carry. The capture/restore measurement test prints path and byte totals, durations and physical store growth for a code tree and the shipped byte boundary. These additions are written for the integrator's rounds; no writer-side build or test was run.

A first recording waits for its requested scope's existing index worker, including when recording is
re-enabled, before its immutable reply is stored. Retries keep their original objects and result;
neither a later save nor a completed worker changes an old recording identity. Native command tests
cover default nested-directory capture, restore, undo and the first request without a retry loop.

A complete empty index publishes its initial whole-tree manifest too. After that, unchanged trees
stay quiet. Native and real-connector API sources require the uploaded empty manifest and a successful
empty file-name search, so full indexing cannot silently leave the server without its first list.

Live view subscriptions own port/foreground sampling and Git reads. Git subscriptions reuse the
workspace's coalesced native file notices, serialize reads per watched directory and emit only changed
statuses; unwatch removes them. A watched directory immediately reports its initial entries with a reset
and truthful truncation flag. Native regression sources cover initial state, changes and stopping watches.

| Module | Planned responsibility |
|--------|------------------------|
| [environment](src/environment.rs) | Attach and detach, workspace roots and environment facts. |
| [environment/git_watches](src/environment/git_watches.rs) | Git subscriptions share the workspace's native file notices and retain only their last status. |
| [error](src/error.rs) | The failures an environment host reports, and the code a command answers with for each. |
| [blobs](src/blobs.rs) | Where content goes to and comes from: the two small traits the connector implements with the link's content transfer, and stand-ins for tests. |
| [paths](src/paths.rs) | Containment in the workspace root, symlinks, case sensitivity and portable path rules. |
| [directories](src/directories.rs) | A directory for each thread under the workspace root: where its agent starts and its views open, with `.orch` inside it for what Orch and the agent keep there (uploads, shares, the agent's notes) under an ignore file of its own; and not a boundary. |
| [ignore](src/ignore.rs) | Configured patterns and nested .orch_ignore files. |
| [walk](src/walk.rs) | Walking a tree of the workspace without following links, skipping what the ignore rules leave out. |
| [files](src/files.rs) | Listing, stat, fetch, atomic conflict-aware writes, rename, copy, the search inside files, and what an archive holds. |
| [directory_pages](src/directory_pages.rs) | Immutable directory metadata, sorted with bounded memory into anonymous temporary files. |
| [directory_pages/captures](src/directory_pages/captures.rs) | Shared in-flight directory capture, independent readers and joined blocking producer exit. |
| [directory_pages/observations_tests](src/directory_pages/observations_tests.rs) | Native immutable observation intervals, early invalidation and shared capture ownership. |
| [file_states](src/file_states.rs) | Complete file-state baselines prepared from the index and refreshed only for watcher changes. |
| [data_transfer](src/data_transfer.rs) | Closed home and recorded-state transfers through ordinary checked snapshots and content, without connector identity or credentials. |
| [media](src/media.rs) | Media types from a file's name and its first bytes, and whether its content is text. |
| [search](src/search.rs) | The search inside files: the lines of text files under a directory that hold a word or match a bounded pattern. |
| [archive](src/archive.rs) | What an archive holds, read where it lies and without unpacking it. |
| [mounts](src/mounts.rs) | Attached mounts and their modes: reported state, live listing and reading, and further roots for executors. |
| [watch](src/watch.rs) | Recursive watching and fallback scans. |
| [index](src/index.rs) | File version and on-demand hash cache; live-only mode for a tree too large to index. |
| [instruction_root](src/instruction_root.rs) | Empty workspace instruction-root preparation without following links or replacing user content. |
| [manifest](src/manifest.rs) | Sorted whole-tree manifests and publication. |
| [measure](src/measure.rs) | Bounded selection counts, bytes and largest directories before a snapshot job. |
| [snapshot](src/snapshot.rs) | Selection, hashing, deduplicated upload and partial snapshots. |
| [state_store](src/state_store.rs) | Immutable raw Git objects and recorded roots outside the workspace's repositories. |
| [materialize](src/materialize.rs) | Write files, metadata and links from available blobs: a moved workspace, or files delivered with a message. |
| [exact_materialize](src/exact_materialize.rs) | Verified saved files are published once into an unused directory without merging later names. |
| [processes](src/processes.rs) | Scoped process discovery, attribution by thread, safe signaling, and suspending and resuming around a move. |
| [ports](src/ports.rs) | Current native port bindings in an environment's process or private network scope. |
| [ports_sockets](src/ports_sockets.rs) | Inside TCP and UDP sockets opened only after fresh scoped port ownership checks. |
| [resources](src/resources.rs) | Resource sampling and driver limits. |
| [process_limits](src/process_limits.rs) | Proved user resource controls and aggregate groups for environment and provider processes. |
| [restore](src/restore.rs) | Complete namespace restore plans and durable guarded file restoration. |
| [executors](src/executors.rs) | Executor availability and lifecycle; what the exec node offers, in its order, and the tool set a session gets from it. |
| [mcp](src/mcp.rs) | Preset hosting, servers imported from a harness's own configuration and run from the connector's copy, servers an environment was given as its own, relay, tool filters and restart limits. |
| [mcp_host](src/mcp_host.rs) | Tool servers run for an environment: stdio with secret variables, or HTTP with optional secret-backed bearer authentication on every request; tools filtered, safe failure reasons, and individual restart within the crash budget. |
| [terminal](src/terminal.rs) | Interactive shell PTYs that outlive their tunnels: the last of their output kept for whoever takes one up again and for the environment's history when one closes, one attachment at a time, resize, activity, and the report when one ends unattended. |
| [git](src/git.rs) | Read-only Git status and diffs. |
| [git_actions](src/git_actions.rs) | Controlled Git actions and repository-scoped command credentials. |
| [git_https](tests/git_https.rs) | Real HTTPS Git authentication keeps source and environment credentials in their own repositories. |
| [git_publication](src/git_publication.rs) | Selected-path commits and fast-forward pushes with durable effect reconciliation. |
| [git_worktrees](src/git_worktrees.rs) | Job-owned worktrees keep their own files and index while sharing the source repository. |
| [git_copy](src/git_copy.rs) | Copied Git metadata keeps each checkout's files and repository connected. |
| [platform/posix](src/platform/posix.rs) | File operations relative to a directory handle, shared by Linux and macOS. |
| [platform/linux](src/platform/linux.rs) | Linux paths, process groups and PTYs. |
| [platform/macos](src/platform/macos.rs) | macOS filesystem, processes and PTYs. |
| [platform/windows](src/platform/windows.rs) | Windows paths, job objects and ConPTY. |
| [browser_cdp](src/browser_cdp.rs) | A single loopback protocol connection to a browser, with ordered calls and live events. |
| [browser_discovery](src/browser_discovery.rs) | Installed browser and display programs, discovered in order with one sandbox probe. |
| [browsers](src/browsers.rs) | Kept browser profiles and the one browser running on each, started or announced. |
| [browser_screen](src/browser_screen.rs) | A watched tab's pictures and input, with link adaptation and a sharp still image. |
| [browser_window](src/browser_window.rs) | Virtual displays used by browsers with a window, kept until the environment ends. |
| [displays](src/displays/mod.rs) | Display registrations reuse browser windows and keep adopted passwords off public reports. |
| [displays/rfb](src/displays/rfb.rs) | Local RFB sign-in answers server passwords without exposing them to a viewer. |
| [displays/windows](src/displays/windows.rs) | Private virtual-screen records recover only verified owned process identities. |

Process watching also reports immediately after a successful signal, while its regular samples retain their configured interval. Real connector feed tests check this report alongside file changes and resource samples.
