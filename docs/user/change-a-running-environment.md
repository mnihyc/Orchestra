---
group: resources
title: Change an environment while it runs
screens: /thread/$threadId, /resources/environments/$id, /resources/templates/$id, /approvals
keywords: how do I change limits, live environment, resources, mounts, env.resources, env.mounts, env.storage, capability.policy, capability.recreate_grace_secs, mcp.enabled, MCP, restart, failed server, kvm, docker_socket, recreate, update, same image, keep files, memory
summary: Check which environment changes apply live and which need a restart or recreate.
---

# Change an environment while it runs

This page shows which changes to an environment apply while it runs, which need a restart or a recreation, and how to restart one tool server or the whole environment.

Environment Manage controls where work runs and the environment's settings.
A change reaches running work at its own point, and **Show uses** beside it lists
the threads and environments it reaches and when: a tool server's or a skill's
change at the thread's next turn or session; a limit or a mount live where the
driver can apply it, otherwise it waits with its reason and **Recreate now**; a
template's change through the environment's Update. Switching a tool server off
refuses its calls at once, and the line beside the switch says how many threads
that is.

## Change limits or mounts live

With Docker's declared live resources, raising memory from **2048 MB to
4096 MB**, or changing CPU/process limits, updates the running container.
Its existing process keeps running. Before lowering memory, the form shows
current use: a limit below it can make the kernel reclaim memory or stop a
process. A storage-fixed disk limit still needs a recreate.

For a mount, choose its real location and read-only/private/shared mode.
A host with safe live-mount support attaches or detaches it without restarting
the container. Kernels that are not supported, or a safe attach that failed, leave a change
pending with its reason and **Recreate now**. They never replace the container quietly.
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

The tools that are offered refresh at once when the agent program supports reloading MCP servers,
otherwise before its next turn. This operation keeps the server's enabled
choice and the machine's local policy. A stopped environment says it must be
started before this server can return.

## Restart the environment

**Restart…**, beside Stop, shows the running turns, terminals, browsers, displays
and port connections that will stop. It keeps the same container and every file,
including packages and files outside the volumes. With
a grace period of 120 seconds, it waits for running turns under the
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
container's own filesystem as a local layer, including packages in `/usr` and
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
owned container and volumes. The page of the node shows retained layer size.

Who may decide a capability, and how long a request lasts, are in
[How approvals work](how-approvals-work.md). Ports are separate gateway
capabilities, described in [Let agents open ports without asking](let-agents-open-ports.md).
