# Changelog

All notable changes to Orch, newest first. Each version has the same parts, in the form of [the template](.github/release-notes/TEMPLATE.md); the entry for a version is also its release page.

## 0.1.3

A preview of the next release: agents and their sub-agents in the thread, Claude Code through Orch from its first turn to Stop, shortcuts, and faster pages that do not stop to ask.

**Status:** Pre-release. **Released:** 2026-11-27. [Upgrade notes](#upgrade-notes) · [Downloads](#downloads) · [Full list of changes](https://github.com/mnihyc/Orchestra/compare/v0.1.2...v0.1.3)

### Upgrade notes

- Install it by its tag in a new directory: `./update.sh first-start v0.1.3`. `latest` still installs 0.1.2.
- This version requires a fresh installation with a fresh database. Settings, data and keys of 0.1.2 are not carried over.
- Set `[system] public_url` to the address people open.
- Issue fresh personal and service-account API keys. Each key includes the public key ID shown in lists and audit events.
- After the first start, the build checks of installed agent programs run by themselves.

### Breaking changes

- A fresh installation is required (see above).
- Configuration keys that were replaced or removed are refused at start like any unknown key, among them `tunnel_replay_buffer_bytes`. There is no compatibility layer for 0.1.x configuration files.

### Highlights

- **Agents and sub-agents in the thread.** Tool calls read with their results, edits as diffs, todos as a checklist; each sub-agent is a row to open and leave.
- **Claude Code through Orch.** Later turns start, **Stop** ends the work at once, and the composer says before you send when its sandbox cannot start on a machine.
- **Your message stays where you sent it** while a turn starts, under one clock, and stays marked "Not delivered" with **Copy** if the start fails.
- **Shortcuts** to documents, chats, threads and projects follow moves and renames and grant nothing; stars are your own shortcuts.
- **Acts that do not stop to ask.** Moving, archiving and switching off happen at once, with **Undo** and one line on what stops.
- **Faster pages that keep what they show** while they update, and turns that start sooner.

### Changes

#### Fixed
- A thread opens with its whole history when its provider, model, environment, agent profile or machine was deleted; one line names what is gone, with **Choose another**.
- New thread and Add provider work with their defaults for every member on the bundled host, and say before the act when a default cannot work. A thread whose environment failed says why, with **Retry** and **Run somewhere else**.
- Claude Code's check on the bundled host no longer fails on the node's own program path.
- Managed providers and environments can be created again; a provider that failed to start can be stopped, deleted or started again.
- A provider's runtime shows what it is doing while it is created and started, and a refresh that cannot finish says why instead of timing out.
- Add provider offers your own connected machine even when it lacks the Provider role, says which role it needs and changes it in place.
- **Add machine** moves on as soon as the machine is online.
- Removing machines and environments finishes even when one of them is gone or broken.
- A narrower place can only narrow a setting that a wider place set, never widen it.
- A pool's members fit the page at every width, each with its provider and a visible Remove.
- The model picker offers only the models that fit the use, recent ones first, with effort, speed and context window in one place.
- Ask works in organizations shared with you and in administrator mode; its limits are counted in tokens.
- Summaries run with the provider you chose and show what they were given.
- A failed turn states its cause once, in plain words.
- The thread's header stays at the top, and your own messages are easy to find in long conversations.
- Headers and summaries that took much of the screen open folded.
- Moving another person's thread to another environment asks whether the thread owner's provider may run there.
- **Move to another machine…** works for environments with recorded file changes or a home.
- Mail and webhook channels name the field that is wrong.

#### Changed
- Approvals offer **Allow for this thread** and **Deny with note**, and whoever manages a thread chooses its tool access.
- Add provider offers a Google Gemini preset; tool calls, grounded citations and Google's token counts are kept. Antigravity runs through Google's own `agy acp` program in an isolated home.
- Administrators can enable hCaptcha or reCAPTCHA for sign-in, registration, recovery and mailed codes.
- File relay is on by default: browser reads stream from the environment, and temporary copies between machines are released when the transfer ends.
- The node and the connector for Linux are self-contained programs, and the connector runs on Windows (x86-64 and ARM).
- The updater installs deployment files with the release; `./update.sh doctor` checks the stack and the public edge.
- Build checks have their own cases for editing a sent message, continuing from an earlier version and ending background commands, and a build that passed keeps what it proved when a later release adds cases.
- **Run the connection on** in Add provider lists environments as well as machines.
- The Default environment image includes what Claude Code's sandbox needs on Linux.
- The README, the changelog and the issue forms are rewritten, and a security policy and a code of conduct are added.

#### Removed
- Internal working notes are no longer in the repository or in the release archive.

### Known issues

- On a busy server, the first opening of Home, Providers and Machines can still take longer than a second, and the first page after about ten idle minutes is slower than the next ones.
- Google Gemini and Antigravity have not yet been tested against a live account.
- A connector whose disk filled up may stop reconnecting until it is restarted.
- A model pool cannot yet be chosen as a thread's model.
- Deleting many items at once is not in this preview.
- The usage page's records and breakdowns take more room than they should.
- The Files view and a thread's raw event log still clear while they are read again.
- Lines such as "Unrecognized Codex event …" can appear in conversations.
- A thread's native history is not imported across builds of an agent program; the conversation is carried over as items.
- Claude Code older than 2.1.319 and Codex older than 0.167.0 do not start.
- Copying a thread with a very long history can exceed the API's 15-second statement limit and fail.
- This preview was published before its whole automated test suite passed; some cases still fail.

The known issues of 0.1.2 are listed with that version.

### Compatibility

| | Tested |
|---|---|
| Claude Code | 2.1.322 (pinned) and 2.1.323 |
| Codex | 0.167.2 (pinned) and 0.169.0 |

### Downloads

| File | For |
|---|---|
| `orch-deploy-v0.1.3.tar.gz` | The deployment directory (install and update) |
| `orch-server-linux-<arch>.zip`, `orch-frontend-dist.tar.gz`, `orchctl-linux-<arch>-musl` | The server, the web application, the command line |
| `orch-connector-<os>-<arch>` | The connector for a person's own machine |
| `orch-node-linux-<arch>-musl` | The node for a Linux host |
| `version.json` | Every file with its size and checksum; `update.sh` checks each file against it before it installs anything |

## 0.1.2

First public release. For fresh installations.

**Status:** Pre-release. **Released:** 2026-11-13.

This release makes an agent's work easier to follow, gives organizations more control over shared work, and adds native connectors for Linux, macOS and Windows.

### Upgrade notes

- This release is for fresh installations. Install it on a new system: [Install Orch](docs/operator/deployment.md).
- Check these defaults before you invite people:

| Setting | Ships as | What it does |
|---|---|---|
| `chat.group_max_members` | 200 | The most people in one group chat. You can change it. |
| `threads.offline_deadline` | Not set | A disconnected turn waits until it can resume or a person stops it. Set a time to give it a final stop. |
| `org.terms` | Empty | People accept terms only when an organization writes some. |
| `api.idempotency_window` | 24 hours | How long a creation request's `Idempotency-Key` is kept. |
| Session-loss recovery | Continue once | How often a lost agent session continues by itself when its machine returns. |

### Breaking changes

None.

### Highlights

- You can now follow a thread in **Focused**, **Full** or **Live** view; your choice follows you across devices. [Follow a running thread](docs/user/running-thread-views.md)
- You can review the files a turn changed, compare changes across a thread and send line comments with your next message. [Restore files to before a message](docs/user/restore-files-before-a-message.md)
- You can limit where a provider's work runs, with **Where** on its **Use** entry. [Who can see and do what](docs/user/who-can-see-and-do-what.md)
- Group chats now have a name, a home and public or private visibility. [Give a group chat a home](docs/user/group-chats.md)
- Organizations can set terms that people accept when they join. [Accept organization terms](docs/user/organization-terms.md)
- You can switch a thread's environment, or move an environment, with a plan that shows what moves. [Switch a thread or move an environment](docs/user/switch-and-move-environments.md)
- Connectors now exist for Linux, macOS and Windows, on x86_64 and ARM64 (macOS: Intel and Apple silicon). [Connect a machine](docs/user/connect-a-machine.md)

### Changes

#### Threads and review
- Added **Focused**, **Full** and **Live** views for each thread. **Full** stays the default.
- The current step, elapsed time, queued messages and Stop now show together; waiting approvals stay above the composer in every view.
- Added **Review** of a turn's changed files, comparison across the thread or against uncommitted Git work, and line comments sent with your next message.
- Added restoring files from the checkpoint on your own message; what the checkpoint covers, and which files it cannot restore, is shown before you act.
- Model changes, permission-mode changes, compaction, hooks and rate limits now have a visible explanation.
- Early model-call retries appear in live status; persistent retries also produce a notice.
- MCP requests for information reach the existing approval controls.

#### Organizations and access
- Managers of a group chat can hand management over; the last manager stays. People with access to a group's home can join a public group; private groups use invitations and requests. [Give a group chat a home](docs/user/group-chats.md)
- Invitations show the effective terms, including inherited ones, and record acceptance at join, registration or single sign-on. If terms change before acceptance, the person sees the current text again. [Accept organization terms](docs/user/organization-terms.md)
- Admins can send a Markdown message to an organization's members, sub-organizations included, or to every active person. Recipients get an Inbox item. [Messages from admins](docs/user/messages-from-admins.md)
- A provider's **Use** entry can name environments, hosts or machines under **Where**; left out, it reads **Anywhere**. An entry is one grant: model, effort, agent setup and place must all be allowed by the same entry. [Limit where a provider's work runs](docs/user/who-can-see-and-do-what.md)
- Links and references you copy are now consistent; you can share viewing access when you send a reference. [Link sharing](docs/user/link-sharing.md)

#### Resources and environments
- Resource lists and selectors can be grouped by home, owner, originating machine or tag. Imported resources start in their machine's home. [Organize resources](docs/user/resource-groups.md)
- You can discover MCP servers from connected programs, import them from the web and switch them on where they are used. Secrets stay on the machine they came from. [Import MCP servers](docs/user/mcp-import-and-use.md)
- Moving a thread or an environment shows a plan of directories, mounts, files and MCP availability, and the move history names both physical locations.
- Provider-native web search is configurable within the provider's allowed options; usage and charges show where the provider reports them. [Provider web search](docs/user/provider-web-search.md)
- Restricted environments can use a configured parent HTTP proxy and keep their allowed-destination rules.

#### Agent programs
- A lost agent session can continue after its machine returns (once, by default). A disconnected turn can have a final stop time, shown in its banner.
- Claude Code sub-agent requests are counted once in a turn's usage, in whatever order their reports arrive.
- Adopting a Claude Code session begun under an older build brings its history and names the build that wrote it.
- Agent programs on your own machines keep their functions when they update themselves. Orch checks the newest build and switches a function off only when that check fails; until then the build shows **Compatibility not verified**. [Agent program versions](docs/user/agent-program-versions.md)
- Orch installs Claude Code 2.1.322 and Codex 0.167.2 into the environments it manages.

#### Reliability
- A browser tab hidden for minutes keeps its live connection.
- A runtime that the host refuses to start now blocks its thread at once, with the host's words; a thread waiting for its runtime wakes when the runtime's job ends.

#### Administration
- Admins can inspect uploads, snapshots, imports, exports and connector storage with their owners and states, and revoke sessions and tokens. Secret values stay hidden. [Inspect stored data](docs/user/admin-inspection.md)
- Admins can review usage across the system and individual model requests, including failures and retries. A missing measurement is shown as missing, not as zero.

#### API
- Creation requests accept an `Idempotency-Key`: sending the same request again returns the original; reusing the key for a different request returns a conflict. The key belongs to the account.
- Scoped API keys use `/openapi/v1`; browser sessions use `/api/v1`. Queued work keeps the restrictions, expiry and revocation of the credential that started it.
- Holder and machine-role replacements accept an optional `expected_revision`, so a client can detect a concurrent edit.

### Known issues

- Claude Code usage that Orch cannot measure is missing from totals, not estimated: a request that ends after the program's last result, an attempt the program dropped, a turn that ends without a result, and the rest of a sub-agent message cut off by Stop.
- A background sub-agent's question is refused after its parent's turn has ended. Keep the parent's turn waiting for sub-agents that may ask.
- A composer holding several queued items can be taller than its pane on a 400-pixel screen.
- A machine registered only for providers cannot register its own API provider from the machine page.
- Forking or copying a thread with a very long history (about 190,000 turns) can exceed the API's 15-second statement limit and fail.
- If a harness restart is reported while a start of the same harness is under way, that start can finish without running its setup again.
- A new build is checked on a host that can run it (online, of the build's system and processor, outside a personal home). An installation with only personal machines keeps a new build at **Compatibility not verified**, with its functions on.
- A thread's native history is not imported across builds of an agent program (Claude Code: a session exported under another build; Codex: a build without a passed check). The conversation is carried over as items, and the move says so.
- Moving another person's thread to another environment needs your own right to drive the thread and to use its provider there. The plan names what is missing and offers to ask for it.
- The build check has no case yet for editing a sent message or continuing from an earlier version on Claude Code. On a build that breaks one of these, the control stays and the act fails with its reason.
- **Move to another machine…** can fail for an environment on a machine whose agents changed files or that has a home: the move stops while importing, is undone, and the environment stays where it was ("the content store refused"). A move that carries only the workspace is not affected.
- After access to a thread is given, the page of the person who asked can show it as missing until it is reloaded.

### Compatibility

| | Tested |
|---|---|
| Claude Code | 2.1.322 (installed in managed environments) |
| Codex | 0.167.2 (installed in managed environments) |
| Newer vendor builds | Codex 0.169.0 and Claude Code 2.1.323 passed the interface stage of a build check and ran a turn on an installation from this package. They show **Compatibility not verified** until a host that can run a check has checked them. |
| Server, node, managed environments | Linux |
| Connectors | Linux, macOS, Windows, for their local roles and programs |

- Claude Code older than 2.1.319 and Codex older than 0.167.0 do not start.
- A stdio MCP server stays tied to the machine that holds its imported definition; another environment shows why it is not available.
- Provider search, sign-in and sandbox choices depend on the provider and program. Claude Code has no read-only sandbox session; use an enforced mode that fits the work.
- Files outside the recorded checkpoint coverage are restored through the choices that Review shows.

### Downloads

`orch-deploy-v0.1.2.tar.gz` (the deployment directory), `orch-server-linux-{x86_64,aarch64}.zip`, `orch-frontend-dist.tar.gz`, `orchctl-linux-{x86_64,aarch64}-musl`, the connectors `orch-connector-{linux-x86_64-musl, linux-aarch64-musl, macos-x86_64, macos-aarch64, windows-x86_64.exe, windows-aarch64.exe}`, the nodes `orch-node-linux-{x86_64,aarch64}-musl`, and `version.json`, which lists every file with its size and checksum. `update.sh` checks every file against it before it installs anything.
