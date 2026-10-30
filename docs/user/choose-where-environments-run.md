---
title: Choose where environments run
screens: /resources/templates, /resources/templates/$id, /resources/environments, /resources/environments/$id, /resources/hosts/$id
keywords: runtime, runc, gVisor, Kata, microVM, isolation, boundary, container, kernel, docker inside, developer tools, qemu, kvm, runs on, limits here, env.runtime, environments.runtimes, 运行时, 隔离
---

# Choose where environments run

Every environment has two names: what it **runs on** (Docker or a machine) and its **runtime**, the
program that holds it in. They are shown together, for example **Docker · gVisor**. The runtime is named as it is, with
one line on what it shares with the host:

- **runc**: shares the host's kernel.
- **gVisor**: its own kernel written in user space.
- **Kata**: a microVM with its own kernel.

Another runtime that the host registers (crun, Sysbox) is shown under its own name. Docker cannot tell what a runtime
is, so the operator declares it in the node's configuration; a runtime nobody declared counts as sharing the host's
kernel.

## Choose a runtime when you make an environment

Open **Resources → Environments → Add environment**. The template gives the runtime it starts from; the **Runtime**
choice lists those the chosen host offers. A runtime the template does not permit, or that the place forbids, is shown
with its reason. If the place requires one runtime, it is the only one you can pick. Under **Other runtime name…** an
expert can type a name the host registered.

A template names its default runtime in **Confinement → Runtime**, and may limit the choice. A place's policy
(`environments.runtimes`) may allow, forbid or require runtimes, and can be locked like any policy.

Leaving **Runtime** unset uses the host's default, which must meet the template and place's rules. Registered aliases
of the same runtime follow the same rule. Orch checks the policy again before making a unit; a changed rule can stop
an in-progress creation or replacement.

The environment's page shows its pair, the line on what it shares, and **Limits here**: what works differently under
its runtime, as what to do and why. The same lines are told to the agent at the start of each session, and after the
environment moves or its runtime changes.

If the host runs out of space while pulling or building an image, free space on that host and retry. A full disk does
not make Orch substitute an older cached image.

## Which pairs exist

| Runs on ＼ Runtime | none | runc | gVisor | Kata |
|---|---|---|---|---|
| **Machine** | the only one | — | — | — |
| **Docker** | — | the default | when `runsc` is registered | when Kata is registered and `/dev/kvm` is present (nested virtualisation on a virtual machine) |

## What each pair supports

| | Machine | runc | gVisor | Kata |
|---|---|---|---|---|
| Boundary | none | shared kernel | user-space kernel | own kernel |
| Workspace, home, state | directories | volumes | volumes | volumes (virtio-fs) |
| Terminals, files, MCP, executors, browsers, ports | ✓ | ✓ | ✓ | ✓ |
| Shared and read-only mounts | ✓ in place | ✓ | ✓ | ✓ (virtio-fs, slower metadata) |
| File events for changes made outside | ✓ | ✓ | rescans (about 1 s) | rescans (about 1 s) |
| Mount or device change without a new unit | ✓ | ✓ | the environment is made again | the environment is made again |
| Restricted network | — | ✓ | ✓ when proved on the host | ✓ when proved on the host |
| Host network | ✓ | with a capability | ✗ | ✗ |
| Docker inside (`docker_inside`) | ✓ native | ✗ (only the critical `privileged` and `docker_socket`) | ✓ without a firewall for Docker's own network: run the containers you start inside with `--network=host` to reach the network (`-p` still serves a port) | ✓ |
| `sudo` and other set-user-ID programs | ✓ | ✓ | only when `runsc` is registered with `--allow-suid` (set by `gvisor.sh`) | ✓ |
| Virtual machines inside (QEMU) | ✓ KVM | ✓ KVM with the `kvm` capability | by software emulation only | by software emulation only |
| GPU | ✓ | with a capability | ✗ | ✗ |
| Software installed outside the volumes survives a stop, an idle stop, a recycle and a restart | ✓ | ✓ | only with `--overlay2=none` (set by the optional `gvisor.sh`) | ✓ |
| Same-image replacement keeps that software as a kept layer | ✓ | ✓ | only with `--overlay2=none` | ✓ |
| Memory the host spends beyond the unit's limit | none | none | tens of MiB | about 0.3 GiB |
| Host setup | none | none | install and register `runsc` | install and register Kata; KVM |

A capability a pair cannot have is shown where it would be offered, with the reason: for example **Host network** is not
offered under Kata, because the guest has a network of its own.

## When software you installed is lost

Software installed with `apt` (or by hand) outside the volumes lives in the unit's own file system. A **stop**, an **idle
stop**, a **recycle** and a **restart** keep it wherever the table says so. A unit is **replaced** by Recreate, an Update, a
template change, a configuration or capability change that cannot be applied live (every mount or device change under
gVisor or Kata, and **Docker inside** itself), a failed unit being replaced, and a move. A same-image replacement keeps it
as a kept layer where the runtime allows; another image starts fresh. The plans and the grant dialogs say which applies:
"Software installed since the environment was made, outside its volumes (for example with apt), is removed. The image's
own and the volumes stay." The volumes, `/var/lib/docker` included, always stay.

Move and recovery plans show this warning when they cannot carry a kept image from the old hosted unit. A plan that
carries the image keeps that software, even if its measured size is zero. Moving from a machine directory does not
remove the software installed on that machine.

Under gVisor's default overlay the software is lost whenever the environment stops: "Software you install outside the
volumes is lost whenever the environment stops; keep it in the image or the volumes." The optional script
`deploy/hosts/runtimes/gvisor.sh` registers runsc with `--overlay2=none`, which keeps it as runc does; see
the operator page "Recommended host setups".

## Docker inside, and the Developer tools template

**Docker inside** is a capability, offered only under a runtime with a kernel of its own (Kata, gVisor), where it is
moderate in risk: it gives every kernel capability and takes the container's filters off, inside the guest only. Docker
keeps its data in `/var/lib/docker`, which Orch makes a mount of the environment's state volume every time the
environment starts: a file system of its own in a file of the volume under Kata, a directory of it under gVisor. Under a kernel shared with the host, Docker is possible only through the
host's socket or privileged mode, both critical and denied by default.

Ask for Docker inside first, then install Docker: granting it makes the environment again. The **Developer tools**
template is made from an image that has Docker Engine (with buildx and compose), QEMU for x86_64 and aarch64 and the
usual build tools built in; start Docker there with `orch-dockerd`. In a Default environment an agent can install Docker
itself with `sudo apt-get install docker.io` and start `dockerd`. An image without Docker says so where Docker inside is
asked for: "Docker is not installed in this image: install it with sudo (it lasts until the environment is recreated),
or make the environment from Developer tools, which has it built in."

If a service inside answers locally but its published port cannot be reached from another machine, check whether
inner Docker's bridge subnet overlaps the host or client network. Choose a non-overlapping `bip` in
`/etc/docker/daemon.json` inside the environment and restart the inner daemon.

If startup fails, read `/var/log/dockerd.log`: the helper waits at most a minute and retries once only if the startup
process exits.

Orch checks for the Docker client and daemon and both QEMU system programs in the image available on the host, whether
it was pulled, built, imported or already cached. This checks which programs are present, without starting them. If the
image cannot be checked, availability stays unknown; an unsuccessful check does not mean a program is missing.

Virtual machines inside a Kata or gVisor environment run by software emulation: they work, slowly. On a plain container,
the `kvm` capability makes them fast, with the weaker isolation of a container that holds the host's `/dev/kvm`.
