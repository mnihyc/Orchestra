# Operator guide

How to install, secure, update, back up and keep an Orch system running on Docker hosts. The pages say what you need, give the steps, show how to check that they worked, and say what to do when they did not. The scripts that run the same checks again are in [scripts/smoke](../../scripts/smoke/README.md).

## Start here

1. [Install Orch](deployment.md): from the files of a release to a running system and its first administrator.
2. [Run the HTTPS front](edge.md): put HTTPS in front of the web edge and check it.
3. [Secure an installation](security.md): what you do, and leave alone, for the security model to hold.
4. [Back up and restore Orch](backup-and-restore.md): a daily backup and a practised restore.

To report a vulnerability, see the [security policy](../../SECURITY.md).

## Keep it running

- [Monitor an installation](monitoring.md): check the stack, scrape its measurements, and set the first alerts.
- [Diagnose a problem](incidents.md): a table of symptoms, with the repair for each.
- [Update and roll back](upgrades.md): update a release, go back, and finish what an interruption left.
- [Clean up storage and uninstall](storage-cleanup.md): caches, old releases and dumps, and removing an instance or the stack.
- [Recover after a host is lost](lost-machines.md): restore volumes and re-create environments on a new host.
- [Run Orch without internet access](offline.md): gather, carry in, install and update on a disconnected network.

## Configure the system

- [Human check](human-check.md) ([简体中文](zh-CN/human-check.md)).

- [Configure Orch](configuration.md): where each kind of setting lives, the addresses, the stack's `.env` and `orch.toml`.
- [Uploads and private drafts](uploads.md): size and context settings, attachment retention and upload results.
- [Set file preview limits](file-previews.md): preview and viewer limits, and how images reach agents.
- [Set import and export limits](import-and-export-limits.md): how large an import or an export may be.
- [Set up semantic search](semantic-search.md): an embeddings provider, the index and its disk share.
- [Tune document links](document-links.md): how much the **Links** section shows and checks.
- [Set how access requests and timed access work](access-requests.md): grant durations, pending limits and expiry.
- [Read the resource library's counts and set paste delivery](resource-library.md): what the counts mean, and the paste setting.
- [Use the API with a key](api-with-a-key.md): call the API from a script, and what stays with the browser.
- [Person notifications](person-notifications.md): limits on explicit notices, and their access checks and audit records.

## Hosts, machines and gateways

- [Choose and set up a host runtime](recommended-host-setups.md): plain Docker, gVisor or Kata, and the optional scripts.
- [Restrict what a host's drivers may do](driver-policy.md): image and account ceilings and driver options.
- [Run a program when a container starts or stops](node-hooks.md): host routing, firewall or monitoring hooks, with a VPN recipe.
- [Run regional gateways](regional-gateways.md): a gateway near each group of machines.

## Agent programs, providers and models

- [Choose where agent program builds come from](agent-programs.md): build policies, channels, mirrors and checks.
- [Run DeepSeek Harness](acp-programs.md): the program of the Agent Client Protocol that ships with Orch.
- [Read why a DeepSeek Harness extra is off](deepseek-harness-plugin.md): the extras and their reasons.
- [Codex history and carries](codex-history.md) ([简体中文](zh-CN/codex-history.md)): homes, checks of the program's own fork, and bounded carries.
- [Managed agent history](agent-history.md): how long the program's own session files are kept.
- [Set the rules for failed model calls](model-call-recovery.md): wait, continue, fail over or ask a person.
- [Limit a provider's request options](provider-options.md): tiers, access programs and request schemas.
- [Operate skills and live work](skills-and-live-work.md): versioned executable content, stuck refreshes and progress.

The files of [deploy/](../../deploy/README.md) and how the updater works are on their own page.
