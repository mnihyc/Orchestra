# Recommended host setups

Orch's installer installs no container runtime. It detects what a host's Docker has registered, and `/dev/kvm`, and
offers a runtime only on the hosts that have it. The setups below say which suits which use; the two scripts under
[deploy/hosts/runtimes](../../deploy/hosts/runtimes/) set up a host for an administrator who wants one. Each script is
optional, needs root, is idempotent (run again, it says "already so" for what is in place), prints what it changed, and
takes `--dry-run` to say what it would do and change nothing.

Whatever runtime a host has, **declare what it is** in the node's configuration: Docker cannot tell, and the boundary
decides which capabilities are offered under the runtime and how risky they are.

```toml
[node.docker.runtimes.runsc]
boundary = "user_space_kernel"          # none, shared_kernel, user_space_kernel or own_kernel
keeps_software_on_stop = true           # optional: see "gVisor's overlay" below
# title, memory_overhead_mb and enrol_timeout_secs are optional too
```

A runtime that is not declared counts as sharing the host's kernel.

## Keep units running through a Docker update

Without `live-restore`, restarting or updating the Docker daemon stops every container: the units are kept
and started again, but the turns that ran in them end ("The harness lost the session"), and their threads
need a new message to go on. Turn it on in `/etc/docker/daemon.json` before environments run on the host:

```json
{ "live-restore": true }
```

then `sudo systemctl reload docker` (a reload keeps running containers). The units, their agent programs and
their turns then go on while the daemon is down or updated. It does not apply to Docker Swarm.

## Plain Docker

For one team on one host that trusts its agents with a shared kernel. Nothing to install: runc is Docker's own. Every
environment is a container; terminals, mounts, ports and live changes work as they always did. **Docker inside** is not
offered (under a shared kernel it would be nearly the host), and virtual machines inside are fast only with the `kvm`
capability, which hands the container the host's `/dev/kvm`.

## gVisor without KVM

For a host with no hardware virtualisation (or a virtual machine without nested virtualisation) where agents should not
share the host's kernel. gVisor is its own kernel written in user space; it asks nothing special of the host.

```sh
sudo deploy/hosts/runtimes/gvisor.sh --dry-run     # what it would do
sudo deploy/hosts/runtimes/gvisor.sh
```

What the script does: unpacks a pinned gVisor release (its SHA-256 checked), links `runsc` and its containerd shim into
`/usr/local/bin`, registers the runtime `runsc` in `/etc/docker/daemon.json` and asks Docker to read it again. Then add
the declaration shown above to the node.

**gVisor's overlay.** By default gVisor keeps the changes a unit makes to its root file system in an overlay that lives
and dies with its sandbox: software installed outside the volumes (with `apt`, say) is lost whenever the environment
stops. The script registers runsc with `--overlay2=none`, so that writes go to the container's own layer as they do under
runc and survive a stop, an idle stop, a recycle and a restart. A 300 MB sequential write to the root file system took no longer than under runc on the
proof host (about 100 MB/s against 80 MB/s, a virtual machine on a virtual machine); small-file work was not measured. If you register runsc yourself without the flag, declare
`keeps_software_on_stop = false` (the default for this boundary), and Orch tells agents and people that software installed
outside the volumes is lost whenever the environment stops.

The script also sets `--allow-suid`. gVisor by default refuses a program with the set-user-ID bit (`sudo`) the right to
become root even inside the sandbox, and then `sudo` fails in every environment ("effective uid is not 0"). The flag
raises identity inside the sandbox's own kernel only, and a container made with no-new-privileges still cannot. If you
register runsc yourself, add the flag.

Docker inside gVisor works (measured with Docker 29): its data is a directory of the state volume, because gVisor has no
loop devices, and an overlay runs on it as it is. gVisor has no firewall for Docker to configure, so the daemon runs
without one (the image's `orch-dockerd` notices and does that): containers you start inside should use
`--network=host` to reach the network; `-p` still serves a port, but a container on Docker's own bridge cannot reach out.

## Kata with KVM

For a host with `/dev/kvm` (on a virtual machine, nested virtualisation switched on) where each environment should have a
kernel of its own, and where agents may run Docker inside it. Kata Containers 4 with its Rust runtime and QEMU.

```sh
sudo deploy/hosts/runtimes/kata.sh --dry-run
sudo deploy/hosts/runtimes/kata.sh
```

What the script does: loads `vhost_vsock` and `vhost_net` (and keeps them loaded), unpacks a pinned Kata release below
`/opt/kata` (its SHA-256 checked), writes the shim program containerd starts (Kata 4 accepts only the configuration
files it ships, named by `KATA_CONF_FILE`), registers the runtime `kata` and asks Docker to read it again. Docker 26 or
later is needed.

What to expect: a Kata unit starts in seconds (about seven on a host that is itself a virtual machine); each costs the
host about 0.3 GiB beyond its limits, which Orch counts when it judges a host full; mounts go through virtio-fs, so
changes made on the host into a shared mount raise no file events inside (the watcher rescans about every second) and
metadata-heavy work is slower; a mount or device change makes the environment again. Software installed outside the
volumes survives a stop and a same-image replacement. Virtual machines inside run by software emulation.

If the host is itself a container (a test rig, CI), Kata's guest memory is a file in `/dev/shm`: give that container a
large `/dev/shm` (`--shm-size`), or the guest hangs and the agent never answers.

Docker inside has its own bridge network. Choose a `bip` in the environment's `/etc/docker/daemon.json` that does
not overlap the host or client networks, and restart the inner daemon after changing it. An overlap can send replies
to the wrong interface even when a service works from inside the environment. The Developer tools helper also accepts
`--bip` through `ORCH_DOCKERD_ARGS`. This changes the inner daemon only.

Orch gives Docker inside a temporary `/var/run/docker` on each boot. Shim sockets and process state from a previous
boot must not be reused; images and containers stay in `/var/lib/docker` on the state volume. The Developer tools
helper waits at most one minute, including its API checks, and retries once if the startup process exits. A live
daemon is retained even if its API has not answered. Read `/var/log/dockerd.log` for both attempts.
