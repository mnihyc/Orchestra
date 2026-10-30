# Operator guide

How to install, check, back up, update and recover an Orch system on one Docker host. The pages say what was
run, with the commands that were typed; the scripts that run them again are in
[scripts/smoke](../../scripts/smoke/README.md).

- [Deployment](deployment.md): from the files of a release to a running system and its first administrator
- [Running without internet access](offline.md): gather, carry in, install, update and use internal sources
- [Configuration](configuration.md): service configuration, database warning thresholds, logs and blob grace
- [File previews and agent delivery](file-previews.md): preview/viewer settings, disposable caching, formats and native-image policy
- [Uploads and private drafts](uploads.md): size and context settings, attachment retention and upload outcomes
- [Import and export](import-and-export.md): package imports, limits, permissions and undo retention
- [Import and export limits](import-and-export-limits.md): the archive limits, how an import is bounded and checked, and where its records live
- [Person notifications](person-notifications.md): explicit notice limits, access checks and audit records
- [Monitoring](monitoring.md): private metrics listeners, scrape configuration, readiness and alerts
- [HTTPS and WebSocket edge](edge.md): what the front you run must do, an apache2 example, and `orchctl doctor --url`
- [Backup and restore](backup-and-restore.md): what to keep, and putting a system back
- [Agent programs](agent-programs.md): manifests, where builds come from and how each is verified and checked, and adding a program
- [Programs of the Agent Client Protocol](acp-programs.md): DeepSeek Harness, how a session of such a program runs, its privacy rules, and adding another
- [Upgrades and recovery](upgrades.md): releases, agent program versions, going back, and finishing what an interruption left
- [Storage cleanup](storage-cleanup.md): cache retention, restore dependencies, unused releases and dumps
- [Incident diagnosis](incidents.md): readiness, link/provider failures, host recovery and scoped cleanup
- [Model call recovery](model-call-recovery.md): inherited rules, candidate eligibility, repeat guards, deadlines and authority
- [Lost machines](lost-machines.md): declaring permanent loss, recovering environments and taking restored volumes back
- [Node hooks and unit networks](node-hooks.md): operator programs at start/stop, their documents and the worked VPN recipe
- [Agent build checks](harness-build-checks.md): native history support, disabled features and rechecking a build
- [Codex history and carries](codex-history.md) ([简体中文](zh-CN/codex-history.md)): homes, native fork checks and bounded carries
- [Driver policy and host scope](driver-policy.md): shared image ceilings, driver-owned options and pool measurements
- [Recommended host setups](recommended-host-setups.md): plain Docker, gVisor without KVM, Kata with KVM, and the optional scripts that set a host up
- [Skills and live work](skills-and-live-work.md): versioned executable content, stuck refreshes, progress, cancellation and database-update observations

What the files of [deploy/](../../deploy/README.md) are, and how the updater works, is on its own page.

- [Semantic search](semantic-search.md): embeddings providers, accountable use, disk limits, model changes and diagnosis.
- [Document links](document-links.md): where references come from, the seven settings, what a reader can and cannot see, and tuning the checks
