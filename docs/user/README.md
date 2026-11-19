# User guide

Start with one person working beside an agent and a colleague joining that
same work. The articles explain the implemented screens, permission boundaries
and visible outcomes, while retaining explicit limits for unverified native
behavior and reserved implementations.

The user guide has two readers and one source. It is published with the
repository, and it is the help inside the application: every article here is
built into the web application, belongs to the screens it explains, and is
opened from the hints of those screens and from the finder. An article is
therefore written with the screen it explains, and the build will fail on a
hint that links to an article or a heading that is not here.

- [Getting started](getting-started.md): setup, sign-in, one's own organization, provider/models, first topic and invitation.
- [Work and collaboration](workflows.md): threads, the workbench and its code view, tools, documents, sharing, approvals and announcements.
- [Files and attachments](files-and-attachments.md): shared previews and viewers, chip sizes, saved document choices and what agents receive.
- [Write documents as files](write-documents-as-files.md): Orch Markdown, paste, importing Markdown, HTML, Word and archives, and how agents write, import and commit documents.
- [Import and export](import-and-export.md): packages, source files, review, reports and undo.
- [Recover work after a machine is lost](recover-lost-work.md): declare permanent loss, recover several environments, choose a destination, notify people and remove a host.
- [Resources](resources.md): providers, profiles, environments, machines and movement.
- [Resource library](resource-library.md): giving, reported use, nested views and each list's previous state.
- [Connect a machine](connect-a-machine.md): account, service or foreground, another connector instance and clean removal.
- [Agent program builds](agent-program-builds.md): checksum sources, mirror publishing and compatibility checks.
- [Add a provider of any kind](add-a-provider.md): subscription, key, model endpoint and your own GPU adapter.
- [Register your own program](register-your-program.md): agent adapters and provider sources, with the same inline registration on their setup screens.
- [Temporary environments](temporary-environments.md): create, watch the deadline, keep or export before removal.
- [Move a project between orgs or deployments](move-a-project-between-orgs.md): export an Orch package, import it, choose conflicts and people, read the report, undo, and bring in plain files.
- [The message queue](message-queue.md): edit, send now, steer, interrupt and reorder waiting messages.
- [Model call recovery](model-call-recovery.md): rules, eligible candidates, visible retries and continuations, repeat guards and summary waits.
- [Update Orch and its machines](updates.md): automatic updates, machine and node actions, local policy, environment updates and the server's host command.
- [Agent program versions](agent-program-versions.md): compatibility checks, inherited policies, exact pins, upgrades, downgrades and batch updates.
- [Write an environment template](write-an-environment-template.md): start from an example, read each configuration field, and choose a template for a place.
- [Choose where a machine keeps data](choose-machine-storage.md): named storages, shared unit settings and replacing paths in host templates.
- [Read a host’s driver features](host-driver-features.md): available operations, refusal reasons and measurements for a host or pool.
- [Storage cleanup](storage-cleanup.md): unused caches, retained data and unused releases.
- [Switch threads](switch-threads.md): reviewed provider-wide batches, target availability and the system batch setting.
- [Zero in settings](numeric-settings.md): Off, immediate actions and numeric zero values.
- [Settings and usage](settings-and-usage.md): inherited settings, personal controls, notifications, consumption and limits.
- [Finding things](finding-things.md): finder, word and semantic search, filters, result order and Show more.
- [Semantic search](semantic-search.md): ordinary embeddings providers, index states, disk limits and model changes.
- [Related items](related-items.md): documents, chats and threads by meaning, place and references, for people and agents.
- [Document links](document-links.md): what refers to a document with the line around each mention, what it refers to, the graph, and the tabs a document shows.
- [Navigation and tabs](navigation.md): Tree and Recent, the wide table, temporary bars and pinned tabs.
- [Read resource cards and choices](resource-cards.md): inspect an option, its observations and applicable limits.
- [Copy and follow references](references.md): previews, document descriptions, files and lines.
- [People and mentions](people-and-mentions.md): profiles, quick actions and explicit notices.
- [Read notices and send announcements](notices-and-announcements.md): Inbox, recipients, delivery and history.
- [Follow progress and arrange status items](follow-live-work.md): counts, copies, cancellation and personal status layout.
- [Choose and check mount copies](check-mount-copies.md): per-mount choices and dated content comparisons.
- [Give agents versioned skills](use-skills.md): import, review, publish and enable versions.
- [Plan and goals](plan-before-changes.md): Plan, Proceed and Keep planning under the current approval mode, and goals that keep an agent working until it is done.
- [DeepSeek Harness](deepseek-harness.md): what it is, providers and keys, what works and what the plugin adds, privacy defaults and limits.
- [Switch agent programs with history](switch-agent-programs.md): Carry, Summary, None and the reviewed handoff.
- [Manage your account and profile](account-and-profile.md): recovery, profile, sign-in, tokens and personal choices.
- [Reading views and details](reading-views-and-details.md): opening screens, cards, menus, long names and compact settings.

Every user article has a [Chinese edition](zh-CN/README.md) under the same file name.
The application selects the authored body for its language under the same article identity,
so the Help index lists each task once. Translated headings retain the same help destinations.

When adding a screen, name its exact router path in an article's `screens` front matter,
including detail pages and redirects. Give the article a title and search keywords, and
connect the screen's question card to the relevant heading. Keep both editions together
when behavior changes. The help checks report missing routes, translations and destinations.

- [Pause, switch and remove](pause-and-switch.md): immediate pause, waiting messages, separate switching and removal.

A translated heading can keep its original section address with front matter such as
`heading_id.索引状态: index-states`. The reader still sees the authored translated heading.
