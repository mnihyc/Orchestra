# Choose and set up a host runtime

Choose how strongly the environments on a host are isolated from it, and set the host up for that choice. At the end the host offers the container runtime you chose, and Orch knows what that runtime shares with the host.

Orch's installer installs no container runtime. It detects what the host's Docker has registered, and `/dev/kvm`, and offers a runtime only on hosts that have it. People pick a runtime for an environment as [Choose where environments run](../user/choose-where-environments-run.md) describes.

## Which to choose

| Setup | Isolation | Use it when | What the host needs |
|---|---|---|---|
| [Plain Docker](#plain-docker) (runc) | Shares the host's kernel | One team on one host that trusts its agents | Nothing to install |
| [gVisor](#gvisor-without-kvm) (`runsc`) | A kernel of its own, written in user space | Agents should not share the host's kernel, and the host has no hardware virtualization (or a virtual machine without nested virtualization) | The optional `gvisor.sh` script |
| [Kata](#kata-with-kvm) | A micro virtual machine with its own kernel | Each environment should have its own kernel and agents may run Docker inside it | `/dev/kvm` (nested virtualization on a virtual machine), Docker 26 or later, and the optional `kata.sh` script |

Both scripts live in [deploy/hosts/runtimes](../../deploy/hosts/runtimes/). Each is optional, needs root, is idempotent (run again, it says "already so" for what is in place), prints what it changed, and takes `--dry-run` to say what it would do and change nothing.

## Before you start

You need a Linux host with Docker running, root access for the scripts, and a node installed on it ([Install Orch](deployment.md)). If the host is a virtual machine, check that nested virtualization is on before you choose Kata.

## 1. Keep containers running through a Docker update

Without `live-restore`, restarting or updating the Docker daemon stops every container. They are started again, but the turns that ran in them end as a lost session, and their threads need a new message to go on. Turn it on before environments run on the host:

1. In `/etc/docker/daemon.json` add:

   ```json
   { "live-restore": true }
   ```

2. Run `sudo systemctl reload docker` (a reload keeps running containers).

The containers, their agent programs and their turns then go on while the daemon is down or being updated. It does not apply to Docker Swarm.

## 2. Set up the runtime

### Plain Docker

Nothing to install: runc is Docker's own. Every environment is a container, and terminals, mounts, ports and live changes work as they always did. **Docker inside** is not offered (under a shared kernel it would be nearly the host). Virtual machines inside are fast only with the `kvm` capability, which gives the container the host's `/dev/kvm`.

### gVisor without KVM

```sh
sudo deploy/hosts/runtimes/gvisor.sh --dry-run     # what it would do
sudo deploy/hosts/runtimes/gvisor.sh
```

The script unpacks a pinned gVisor release (its SHA-256 checked), links `runsc` and its containerd shim into `/usr/local/bin`, registers the runtime `runsc` in `/etc/docker/daemon.json` and asks Docker to read it again.

- **Overlay.** By default gVisor keeps the changes a container makes to its root file system in an overlay that lives and dies with its sandbox: software installed outside the volumes (with `apt`, say) is lost whenever the environment stops. The script registers `runsc` with `--overlay2=none`, so writes go to the container's own layer as under runc and survive a stop, an idle stop, a recycle and a restart. If you register `runsc` yourself without the flag, declare `keeps_software_on_stop = false` (the default for this boundary), and Orch tells agents and people that software installed outside the volumes is lost whenever the environment stops.
- **`sudo`.** The script also sets `--allow-suid`. By default gVisor refuses a program with the set-user-ID bit (`sudo`) the right to become root even inside the sandbox, and then `sudo` fails in every environment ("effective uid is not 0"). The flag raises identity inside the sandbox's own kernel only, and a container made with no-new-privileges still cannot. If you register `runsc` yourself, add the flag.
- **Docker inside** works (tested with Docker 29). Its data is a directory of the state volume, because gVisor has no loop devices. gVisor has no firewall for Docker to configure, so the daemon runs without one (the image's `orch-dockerd` notices and does that): containers you start inside should use `--network=host` to reach the network. `-p` still serves a port, but a container on Docker's own bridge cannot reach out.

### Kata with KVM

```sh
sudo deploy/hosts/runtimes/kata.sh --dry-run
sudo deploy/hosts/runtimes/kata.sh
```

The script loads `vhost_vsock` and `vhost_net` (and keeps them loaded), unpacks a pinned Kata Containers 4 release below `/opt/kata` (its SHA-256 checked), writes the shim program that containerd starts (Kata 4 accepts only the configuration files it ships, named by `KATA_CONF_FILE`), registers the runtime `kata` and asks Docker to read it again.

What to expect:

- A Kata container starts in seconds and costs the host about 0.3 GiB beyond its limits, which Orch counts when it judges a host full.
- Mounts go through virtio-fs, so changes made on the host into a shared mount raise no file events inside (the watcher rescans about every second), and metadata-heavy work is slower. A mount or device change makes the environment again.
- Software installed outside the volumes survives a stop and a same-image replacement.
- Virtual machines inside run by software emulation.
- If the host is itself a container (a test rig, CI), Kata's guest memory is a file in `/dev/shm`: give that container a large `/dev/shm` (`--shm-size`), or the guest hangs and the agent never answers.

**Docker inside Kata.** It has its own bridge network. Choose a `bip` in the environment's `/etc/docker/daemon.json` that does not overlap the host's or the client's networks, and restart the inner daemon after changing it; an overlap can send replies to the wrong interface even when a service works from inside the environment. The Developer tools helper also accepts `--bip` through `ORCH_DOCKERD_ARGS`. This changes the inner daemon only. Orch gives Docker inside a temporary `/var/run/docker` on each boot, so shim sockets and process state from an earlier boot are never reused; images and containers stay in `/var/lib/docker` on the state volume. The Developer tools helper waits at most one minute, including its API checks, and retries once if the startup process exits. A live daemon is kept even if its API has not answered. Read `/var/log/dockerd.log` for both attempts.

## 3. Declare what the runtime is

Docker cannot tell what a runtime is, and the boundary decides which capabilities are offered under it and how risky they are. Declare every runtime you registered in the node's configuration:

```toml
[node.docker.runtimes.runsc]
boundary = "user_space_kernel"          # none, shared_kernel, user_space_kernel or own_kernel
keeps_software_on_stop = true           # optional; true when runsc has --overlay2=none
# title, memory_overhead_mb and enrol_timeout_secs are optional too

[node.docker.runtimes.kata]
boundary = "own_kernel"
```

A runtime that is not declared counts as sharing the host's kernel. Restart the node after you change its configuration.

## 4. Check that it worked

1. Docker lists the runtime: `docker info --format '{{range $name, $_ := .Runtimes}}{{$name}} {{end}}'` should include `runsc` or `kata`.
2. In **Resources → Environments → Add environment**, choose a template whose host is this one. **Container runtime** lists the runtimes that host offers, yours included.
3. Make an environment with that runtime. Its page shows a pair such as **Docker · gVisor**, the line on what it shares, and **Limits here**.

## If it does not work

| You see | What to do |
|---|---|
| The runtime is not offered on the host | The node does not see it: check that Docker lists it, that `/dev/kvm` exists for Kata, and restart the node |
| `sudo` fails in a gVisor environment | `runsc` is registered without `--allow-suid`. Add the flag or run `gvisor.sh` |
| Software is lost when a gVisor environment stops | `runsc` is registered without `--overlay2=none`. Run `gvisor.sh`, or declare `keeps_software_on_stop = false` so Orch says so |
| A Kata guest hangs | The host is a container with a small `/dev/shm`. Raise `--shm-size` |
| A service in Docker inside answers locally but not from outside | Its bridge subnet overlaps another network. Choose another `bip` |
