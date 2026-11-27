---
group: getting-started
summary: Find task guides for setup, agent work, sharing and managing Orch.
---

# User guide

Start with one person working beside an agent and a colleague who joins the same work: [Getting started](getting-started.md) takes you through it. The guides explain the screens that exist, the permissions they ask for and what you see as a result, and they say plainly what is **Not available yet**.

This guide is also the help inside the application. Each page belongs to the screens it explains, and it opens from the hints of those screens and from the finder. Every page has a [Chinese edition](zh-CN/README.md) under the same file name.

## Get started

- [Getting started](getting-started.md): setup, sign-in, one's own organization, provider/models, first topic and invitation.
- [Work and collaboration](workflows.md): threads, the workbench and its code view, tools, documents, sharing, approvals and announcements.
- [Connect a machine](connect-a-machine.md): account, service or foreground, another connector instance and clean removal.
- [Add a provider of any kind](add-a-provider.md): subscription, key, model endpoint and your own GPU adapter.
- [Temporary environments](temporary-environments.md): create, watch the deadline, keep or export before removal.

## Work with agents and people

- [Follow progress and arrange status items](follow-live-work.md): counts, copies, cancellation and personal status layout.
- [Follow a running thread](running-thread-views.md)
- [Thread states](thread-states.md): the states of a thread, unread results and controls for background work
- [The message queue](message-queue.md): edit, send now, steer, interrupt and reorder waiting messages.
- [Send a message later](send-later.md): save a message for a future time
- [Plan and goals](plan-before-changes.md): Plan, Proceed and Keep planning under the current approval mode, and goals that keep an agent working until it is done.
- [Pause, switch and remove](pause-and-switch.md): immediate pause, waiting messages, separate switching and removal.
- [Switch agent programs with history](switch-agent-programs.md): Carry, Summary, None and the reviewed handoff.
- [Switch threads](switch-threads.md): reviewed provider-wide batches, target availability and the system batch setting.
- [Switch a thread or move an environment](switch-and-move-environments.md)
- [Carry a thread to another agent](carry-a-thread.md): how a fork or switch carries the conversation you chose
- [Restore files to before a message](restore-files-before-a-message.md)
- [Let the agent schedule future work](agent-schedules.md)
- [Let agents open ports without asking](let-agents-open-ports.md): who can reach the services of an environment, and which openings agents may request
- [How approvals work](how-approvals-work.md): what a request would do and who may decide it
- [Paste and upload](paste-and-upload.md): where pasted text, uploaded files, attachments and references go
- [People and mentions](people-and-mentions.md): profiles, quick actions and explicit notices.
- [Give a group chat a home](group-chats.md)
- [Read notices and send announcements](notices-and-announcements.md): Inbox, recipients, delivery and history.
- [Read an administrator message](administrator-messages.md): sender, time and contents of a message from your Inbox
- [Messages from admins](messages-from-admins.md)
- [Send a link or reference](link-sharing.md)
- [Give agents versioned skills](use-skills.md): import, review, publish and enable versions.
- [Import and switch on MCP servers](mcp-import-and-use.md)
- [Personal read tools](personal-read-tools.md): help in any thread, and whether it may read with your access
- [Delegate a capability to another program](delegated-capabilities.md): a sign-in of a program for an image service

## Documents and files

- [Write documents as files](write-documents-as-files.md): Orch Markdown, paste, importing Markdown, HTML, Word and archives, and how agents write, import and commit documents.
- [Files and attachments](files-and-attachments.md): shared previews and viewers, chip sizes, saved document choices and what agents receive.
- [Copy and follow references](references.md): previews, document descriptions, files and lines.
- [Related items](related-items.md): documents, chats and threads by meaning, place and references, for people and agents.
- [Document links](document-links.md): what refers to a document with the line around each mention, what it refers to, the graph, and the tabs a document shows.
- [Read and refresh summaries](summaries.md): resource paragraphs, chat catch-up, independent titles, usage and Ask drafts.

## Find and ask

- [Finding things](finding-things.md): finder, word and semantic search, filters, result order and Show more.
- [Ask about your work](ask.md): read-only questions, cited sources, one organization per conversation, history, floating windows and retention.
- [Semantic search](semantic-search.md): ordinary embeddings providers, index states, disk limits and model changes.

## Access and sharing

- [Who can see and do what](who-can-see-and-do-what.md): ask for access, decide who may make things, share with the right holders
- [Accept organization terms](organization-terms.md)
- [Organize resources by home and origin](resource-groups.md)
- [Duplicate a resource or conversation](duplicate-resources.md): an independent copy, and where it belongs
- [Import and export](import-and-export.md): packages, source files, review, reports and undo.
- [Move a project between orgs or deployments](move-a-project-between-orgs.md): export an Orch package, import it, choose conflicts and people, read the report, undo, and bring in plain files.

## Providers and agent programs

- [Choose and read provider details](provider-details.md): where a provider's details come from, balances, resets
- [Provider web search](provider-web-search.md)
- [Model call recovery](model-call-recovery.md): rules, eligible candidates, visible retries and continuations, repeat guards and summary waits.
- [Agent program versions](agent-program-versions.md): compatibility checks, inherited policies, exact pins, upgrades, downgrades and batch updates.
- [Agent program builds](agent-program-builds.md): checksum sources, mirror publishing and compatibility checks.
- [Register your own program](register-your-program.md): agent adapters and provider sources, with the same inline registration on their setup screens.
- [DeepSeek Harness](deepseek-harness.md): what it is, providers and keys, what works and what the plugin adds, privacy defaults and limits.

## Environments, hosts and machines

- [Resources](resources.md): providers, profiles, environments, machines and movement.
- [Resource library](resource-library.md): giving, reported use, nested views and each list's previous state.
- [Read resource cards and choices](resource-cards.md): inspect an option, its observations and applicable limits.
- [Choose where environments run](choose-where-environments-run.md): the host and the container runtime, and how strongly an environment is isolated
- [Write an environment template](write-an-environment-template.md): start from an example, read each configuration field, and choose a template for a place.
- [Run commands after a container starts](commands-after-start.md): commands that run inside a container before it is ready
- [Change an environment while it runs](change-a-running-environment.md): which changes apply live and which need a restart or a recreation
- [Choose and check mount copies](check-mount-copies.md): per-mount choices and dated content comparisons.
- [Choose where a machine keeps data](choose-machine-storage.md): named storages, shared unit settings and replacing paths in host templates.
- [Read a host’s driver features](host-driver-features.md): available operations, refusal reasons and measurements for a host or pool.
- [Recover work after a machine is lost](recover-lost-work.md): declare permanent loss, recover several environments, choose a destination, notify people and remove a host.
- [Storage cleanup](storage-cleanup.md): unused caches, retained data and unused releases.

## Settings, usage and account

- [Settings and usage](settings-and-usage.md): inherited settings, personal controls, notifications, consumption and limits.
- [Speed, first output and errors](speed-first-output-and-errors.md): request performance and timing in a turn
- [Zero in settings](numeric-settings.md): Off, immediate actions and numeric zero values.
- [Manage your account and profile](account-and-profile.md): recovery, profile, sign-in, tokens and personal choices.
- [Update Orch and its machines](updates.md): automatic updates, machine and node actions, local policy, environment updates and the server's host command.
- [Inspect stored data, sessions and tokens](admin-inspection.md)
- [Navigation and tabs](navigation.md): Tree and Recent, the wide table, temporary bars and pinned tabs.
- [Reading views and details](reading-views-and-details.md): opening screens, cards, menus, long names and compact settings.

## Reference

- [Settings reference](settings-reference.md): every setting, where it can be set and what it changes
- [State reference](state-reference.md): what each state beside a thread, provider or machine means
- [Error reference](error-reference.md): what each error means and what to do next
- [Option field reference](option-reference.md): every request option a provider endpoint accepts
