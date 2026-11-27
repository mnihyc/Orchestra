---
group: resources
title: Choose where environments run
screens: /resources/templates, /resources/templates/$id, /resources/environments, /resources/environments/$id, /resources/hosts/$id
keywords: runtime, runc, gVisor, Kata, microVM, isolation, boundary, container, kernel, docker inside, developer tools, qemu, kvm, runs on, limits here, env.runtime, environments.runtimes, 运行时, 隔离
summary: Choose the host and the container runtime that run an environment and set how strongly it is isolated.
---

# Choose where environments run

Leave **Container runtime** unset unless you need a stronger boundary. Then the host's default applies, as long as it meets the rules of the template and of the place. This page tells you what the runtimes are, how to choose one when you make an environment, and what changes with each.

Every environment has two names: what it **runs on** (Docker or a machine) and its container runtime, the program that holds it in. They show together, for example **Docker · gVisor**. The runtime is named as it is, with one line on what it shares with the host:

- **runc**: shares the kernel of the host.
- **gVisor**: its own kernel, written in user space.
- **Kata**: a micro virtual machine with its own kernel.

A runtime that the host registers in addition (crun, Sysbox) shows under its own name. Docker cannot tell what a runtime is, so the operator of the host declares it in the configuration of the node. A runtime that nobody declared counts as sharing the kernel of the host.

## Choose a runtime when you make an environment

1. Open **Resources → Environments → Add environment**.
2. Choose the template. It gives the runtime the environment starts from.
3. Open **Container runtime**. It lists the runtimes the chosen host offers. A runtime that the template does not allow, or that the place forbids, is shown with its reason. If the place requires one runtime, it is the only one you can pick.
4. If you are an expert, **Other container runtime name…** takes a name that the host registered.

A template names its default runtime under **Confinement → Container runtime** and can limit the choice. The policy of a place can allow, forbid or require runtimes, and it can be locked like any policy. Orch checks the policy again before it makes a container, so a rule that changed can stop a creation or a replacement that is in progress. Registered aliases of the same runtime follow the same rule.

The page of the environment shows the pair, the line on what it shares, and **Limits here**: what works differently under its runtime, as what to do and why. The same lines are told to the agent at the start of each session, and again after the environment moves or its runtime changes.

If the host runs out of space while it pulls or builds an image, free space on that host and try again. A full disk never makes Orch put an older cached image in its place.

## Which pairs exist

| Runs on ＼ Runtime | none | runc | gVisor | Kata |
|---|---|---|---|---|
| **Machine** | the only one | — | — | — |
| **Docker** | — | the default | when `runsc` is registered | when Kata is registered and `/dev/kvm` is present (nested virtualization on a virtual machine) |

## What each pair supports

| | Machine | runc | gVisor | Kata |
|---|---|---|---|---|
| Boundary | none | shared kernel | kernel in user space | own kernel |
| Workspace, home, state | directories | volumes | volumes | volumes (virtio-fs) |
| Terminals, files, MCP servers, executors, browsers, ports | ✓ | ✓ | ✓ | ✓ |
| Shared and read-only mounts | ✓ in place | ✓ | ✓ | ✓ (virtio-fs, slower metadata) |
| File events for changes made outside | ✓ | ✓ | rescans (about 1 s) | rescans (about 1 s) |
| Change of a mount or a device without a new container | ✓ | ✓ | the environment is made again | the environment is made again |
| Restricted network | — | ✓ | ✓ when it was shown to work on the host | ✓ when it was shown to work on the host |
| Host network | ✓ | with a capability | ✗ | ✗ |
| Docker inside | ✓ its own | ✗ (only through the critical capabilities for a privileged container and the Docker socket) | ✓ without a firewall for the network of Docker itself: run the containers you start inside with `--network=host` to reach the network (`-p` still serves a port) | ✓ |
| `sudo` and other programs that run as their owner | ✓ | ✓ | only when `runsc` is registered with `--allow-suid` (the optional script `gvisor.sh` does it) | ✓ |
| Virtual machines inside (QEMU) | ✓ KVM | ✓ KVM with the `kvm` capability | by software emulation only | by software emulation only |
| GPU | ✓ | with a capability | ✗ | ✗ |
| Software installed outside the volumes survives a stop, an idle stop, a recycle and a restart | ✓ | ✓ | only with `--overlay2=none` (set by the optional `gvisor.sh`) | ✓ |
| A replacement with the same image keeps that software as a kept layer | ✓ | ✓ | only with `--overlay2=none` | ✓ |
| Memory the host spends beyond the limit of the container | none | none | tens of MiB | about 0.3 GiB |
| Setup of the host | none | none | install and register `runsc` | install and register Kata; KVM |

A capability that a pair cannot have is shown where it would be offered, with the reason. For example, **Host network** is not offered under Kata, because the guest has a network of its own.

## When software you installed is lost

Software that you install with `apt`, or by hand, outside the volumes lives in the file system of the container. A **stop**, an **idle stop**, a **recycle** and a **restart** keep it wherever the table says so. A container is **replaced** by **Recreate**, by an update, by a change of the template, by a change of configuration or capability that cannot be applied live (every change of a mount or a device under gVisor or Kata, and **Docker inside** itself), by a failed container being replaced, and by a move.

A replacement with the same image keeps the software as a kept layer where the runtime allows it. Another image starts fresh. The plans and the grant dialogs say which applies: "Software installed since the environment was made, outside its volumes (for example with apt), is removed. The image's own and the volumes stay." The volumes, `/var/lib/docker` included, always stay.

Move and recovery plans show this warning when they cannot carry a kept image from the old hosted container. A plan that carries the image keeps the software, even if its measured size is zero. Moving from a directory of a machine does not remove the software installed on that machine.

Under the default overlay of gVisor, the software is lost whenever the environment stops: "Software you install outside the volumes is lost whenever the environment stops; keep it in the image or the volumes." The optional script `deploy/hosts/runtimes/gvisor.sh` registers `runsc` with `--overlay2=none`, which keeps it as runc does. See the operator page "Recommended host setups".

## Docker inside, and the Developer tools template

**Docker inside** is a capability. It is offered only under a runtime with a kernel of its own (Kata, gVisor), where its risk is moderate: it gives every kernel capability and takes the filters of the container off, inside the guest only. Docker keeps its data in `/var/lib/docker`. Orch makes that a mount of the state volume of the environment every time the environment starts: a file system of its own in a file of the volume under Kata, and a directory of it under gVisor. Under a kernel shared with the host, Docker is possible only through the socket of the host or in privileged mode. Both are critical and denied by default.

Ask for Docker inside first, then install Docker. Granting it makes the environment again.

- The **Developer tools** template is made from an image that has Docker Engine (with buildx and compose), QEMU for x86_64 and aarch64 and the usual build tools built in. Start Docker there with `orch-dockerd`.
- In a **Default** environment an agent can install Docker itself with `sudo apt-get install docker.io` and start `dockerd`.
- An image without Docker says so where Docker inside is asked for: "Docker is not installed in this image: install it with sudo (it lasts until the environment is recreated), or make the environment from Developer tools, which has it built in."

If a service inside answers locally but its published port cannot be reached from another machine, check whether the bridge subnet of the inner Docker overlaps the network of the host or of the client. Choose a `bip` that does not overlap in `/etc/docker/daemon.json` inside the environment, and restart the inner daemon. If startup fails, read `/var/log/dockerd.log`. The helper waits at most a minute, and it retries once only if the startup process exits.

Orch looks for the Docker client, the Docker daemon and both QEMU system programs in the image that is available on the host, whether it was pulled, built, imported or already cached. This checks which programs are present and does not start them. If the image cannot be checked, availability stays unknown; a check that fails does not mean a program is missing.

Virtual machines inside a Kata or gVisor environment run by software emulation, which works but slowly. On a plain container, the `kvm` capability makes them fast, with the weaker isolation of a container that holds the `/dev/kvm` of the host.
