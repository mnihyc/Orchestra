---
title: Change an environment while it runs
screens: /thread/$threadId, /resources/environments/$id, /resources/templates/$id, /approvals
keywords: how do I change limits, live environment, resources, mounts, env.resources, env.mounts, env.storage, capability.policy, capability.recreate_grace_secs, mcp.enabled, MCP, restart, failed server, kvm, docker_socket, recreate, update, same image, keep files, memory
---

# Change an environment while it runs

Environment Manage controls where work runs and the environment's settings.
Read **Takes effect** beside a change: it follows the driver's actual live
abilities and this host's readiness.

## Change limits or mounts live

With Docker's declared live resources, raising memory from **2048 MB to
4096 MB**, or changing CPU/process limits, updates the running container.
Its existing process keeps running. Before lowering memory, the form shows
current use: a limit below it can make the kernel reclaim memory or stop a
process. A storage-fixed disk limit still needs a recreate.

For a mount, choose its real location and read-only/private/shared mode.
A host with safe live-mount support attaches or detaches it without restarting
the unit. Unsupported kernels or a failed safe attach leave a pending change
with its reason and Recreate now; they do not silently replace the unit.
Desired live mounts are applied again after every start.

The agent receives only the changed context line with its next accepted
input. A live change does not start a turn or send a command to stop it.
The actual context text is readable at work detail.

## Give KVM or the Docker socket

A template that allows KVM prepares its device permission and group but
leaves `/dev/kvm` absent and prevents making it before grant. Giving KVM
through the authorized capability decision exposes the node live. The
Docker socket likewise arrives as a live mount when permitted. Existing
processes and disk contents stay; the agent is told available now, nothing
stops. There is no recreate-before/after notice for this live change.

Wait for the application result after approval. A failed live change grants
nothing and may restart the same environment to restore its previous permissions.
The approval says **Approved; not granted** with the reason, and the agent
receives that failure with its next input. Ask the host's manager to resolve
the cause before requesting the capability again; the node log records the
helper's exit status and last error lines.

Taking either away removes its node/mount and restarts the **same container**
to close handles an old process may still hold. Processes stop; installations
and files outside the volumes survive this restart. The Docker socket gives
access to the host daemon, so its critical-risk decision matters.

## Restart one tool server

The environment page and a thread's Environment panel list each MCP server's
state. A failed server shows its exit and last output lines with secrets masked.
For example, **Browser tools** exits with code **1** three times and becomes
failed. An environment manager chooses **Restart** on that server. Its job
clears the failure count and starts it again; the environment, other servers
and active turns continue. Read the result: a further failure keeps its reason.
An unavailable server still needs its missing requirement or permission.

The offered tools refresh immediately when the harness supports MCP reload,
otherwise before its next turn. This operation keeps the server's enabled
choice and the machine's local policy. A stopped environment says it must be
started before this server can return.

## Restart the environment

**Restart…**, beside Stop, shows the running turns, shells, browsers, displays
and port connections that will stop. It keeps the same unit and every file,
including packages and files outside the volumes. With
`capability.recreate_grace_secs = 120`, it waits for running turns under the
ordinary recreate grace rule. Choose **Interrupt now** for an immediate
interruption instead. **Wait** lets turns finish during that grace, then
interrupts any still running; the dialog says this before confirmation.
The job shows waiting, stopping and starting, and a
failure says which step failed.

For example, a running turn has written `/opt/restart-proof.txt` outside the
volumes and another input is queued. After Restart succeeds, that file remains,
the queued input continues through its ordinary checks and the thread names
who restarted the environment. Port connections stop; openings follow their
normal stop/start behavior. A restart keeps existing restriction and credential
return checks.

## Read a recreate plan

Capabilities such as `ptrace`, GPU, network administration, unconfined,
privileged or host networking, another storage and creation-time disk limits
still need a replacement. A same-image Docker recreate keeps the stopped
unit's own filesystem as a local layer, including packages in `/usr` and
files in `/opt`. The plan states the measured size and estimated time first.
An estimate can vary by host; unknown is labelled unknown.

Another image starts clean outside workspace/home/state volumes, and the
plan says that loss before confirmation. After ten preservation layers the
driver flattens them, with the extra work in the estimate. A driver that
cannot keep same-image installations says so.

For example, install a debugger in `/opt`, then read a `ptrace` capability
card: it says processes stop and how much will be kept. After success the
installation remains. An image update's different warning should prompt you
to put a required installation into the template instead.

Kept images belong to the environment. Moves list and carry their size;
archiving retains them for restoration; failed replacements and obsolete
generations clean their own images. Deletion/purge removes them with the
owned unit and volumes. The node's page shows retained layer size.

Capability authority and request lifetime are in
[How approvals work](how-approvals-work.md). Ports are separate gateway
capabilities, described in [Let agents open ports without asking](let-agents-open-ports.md).
