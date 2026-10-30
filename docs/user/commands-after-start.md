---
title: Run commands after a unit starts
screens: /resources/environments, /resources/providers, /resources/templates
keywords: how do I run a command after start, Commands after start, after_up, env.after_up, runtime.after_up, runtime.storage, runtime.needs, env.variables, runtime.variables, argv, timeout, read-only root
---

# Run commands after a unit starts

Managed environments and provider runtimes share their unit configuration
form. **Commands after start** runs inside the unit after its connector
has enrolled/attached and before it is ready. The host operator's hooks
run separately, on that host, before these unit commands.

The template's named commands appear read-only first. Below them are
the resolved `env.after_up` or `runtime.after_up` values and where they
come from. The template commands run first in name order, then the
resolved setting commands in name order. A null setting entry removes
an inherited setting command; it does not erase a template command.

## Choose a command and its rights

Enter a name, a program and each argument separately, then its time limit.
The default is 300 seconds; the allowed range is 1–3600. There is no
implicit shell: `$HOME`, `&&` or a pipe in an argument remains ordinary
text. To use shell syntax, choose a trusted image that actually has a
shell and name that shell explicitly in argv. The command runs as the
unit's configured user with its configured variables, not as the host
operator. Environment commands start in its workspace; provider commands
start in its home.

For an environment with the ordinary file tools, an idempotent command
can make its cache directory:

```json
{"cache":{"command":["/usr/bin/mkdir","-p","/workspace/.orch/cache"],"timeout_secs":30}}
```

On a provider, only its owners can edit these commands at the resource:
they run beside its credential with its harness's rights. The seeded
provider image has a read-only root, a non-root user and no shell or
package manager. Choose an appropriate trusted template before adding a
command; a setting adds neither root nor missing tools.

For example, a trusted template containing `/usr/bin/install` and an
image-carried static `/opt/tools/acme-cli` can put it in the writable
provider home on state:

```json
{"install-cli":{"command":["/usr/bin/install","-D","-m","755","/opt/tools/acme-cli","/home/agent/bin/acme-cli"],"timeout_secs":30}}
```

This writes the provider's own home and leaves the read-only image root
intact. The exact home/user must match your template. A scope's settings
manager may set defaults; the provider's own commands remain owners-only.
Secrets are selected by name through the ordinary unit secret controls,
never pasted into plain command arguments or printed for diagnosis.

## Starts, retries and failure

Commands run on each new start, restart, recreate, update, move and
renewal start of a managed unit. They must be safe to repeat across
those runs. Replaying one start job returns the journalled result and
does not run the same command twice. A renewal that reuses an already
running unit does not run them again.

Nonzero exit, failed launch or timeout fails the start and stops the
unit; there is no Ignore failure switch. A timeout kills the command's
process group. The job/resource page says which command failed, its
exit and its masked first lines, with the setting link. History keeps
the name, result and duration, never output or a secret. Fix the actual
command or image, then start again under a new reviewed run.

## A person's own machine

There is no managed unit, so Commands after start and driver storage/
limits say why they do not apply there. Environment variables apply
when its connector attaches; provider harness variables belong to the
agent profile. Orch does not run these commands on the person's host.

For managed providers, `runtime.storage` chooses the state volume's
storage when it is newly made; an existing volume stays on its recorded
storage. `runtime.needs` declares what it needs for placement without
reserving memory or disk. Work mounts, workspace storage and doors
remain environment configuration. See [resources](resources.md) and
[provider details](provider-details.md).
