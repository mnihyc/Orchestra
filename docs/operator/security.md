# Secure an installation

What the operator of an Orch installation has to do, and has to leave alone, for the guarantees of the [security model](../security/model.md) to hold. To report a vulnerability, see the [security policy](../../SECURITY.md).

## Checklist

1. **Put an HTTPS front before the web edge.** Orch's own edge speaks plain HTTP on the host. People must reach it over HTTPS: see [The HTTPS front](edge.md) for what the front must do and how to check it.
2. **Name the front's network.** Orch trusts no proxy unless the deployment names it. Until you list the addresses of your front, every caller appears to come from the front, and sign-in limits and the audit origin lose their meaning. [Which proxies may describe a caller](edge.md#5-name-the-proxies-that-may-describe-a-caller) shows how.
3. **Keep the secret files private.** `.env` and everything in `config/secrets/` are readable by their owner alone. `update.sh` corrects the mode of `.env`, and `orchctl doctor --only secrets` names any file that others can read. It never prints their contents.
4. **Keep a private copy of the master key and the database password.** Losing `orch_master_key` or `orch_ca_key.pem` cannot be undone. Keep the copy where the host's own failure cannot take it too: [Back up and restore](backup-and-restore.md).
5. **Run one API process.** Terminal, browser and display tickets and their live tunnels belong to the process that made them. The shipped services run one API process, and running more than one is not supported.
6. **Keep the Docker socket to the node.** Only the node's container mounts the host's Docker socket, so that it can make managed environments. Whoever controls that socket controls the host. Do not mount it into anything else, and do not add a service to the host's Docker group that you would not trust with the host.
7. **Pin the stock images.** The release's Compose file selects the stock images by digest, including the image index that serves every supported processor. Update the pins only through the [release image script](../../scripts/release/README.md), and review what it prints.
8. **Decide what the Default environment image may do.** In an environment made from the shipped image, the agent's account can become root with `sudo` and no password, so that whoever works there installs what the work needs. That is root inside the container and nowhere else. A template that sets `no_new_privileges` takes it away. [Configuration](configuration.md#build-environment-images) says where the image comes from.
9. **Restrict networks where it matters.** An environment can run on a restricted network that its driver enforces. If the driver cannot enforce it, creating the environment is refused; it never falls back to an open network. See [Driver policy and host scope](driver-policy.md) and [Node hooks and unit networks](node-hooks.md).
10. **Decide which private addresses notifications may reach.** Notification connections refuse private, loopback and other special-purpose addresses. An exception belongs to the system's outbound policy, which a system administrator sets, so grant one only for a host you mean to reach.
11. **Treat a plugin as code on the host.** Plugins run with their host account's authority, or with their own service account when attached. Orch does not sandbox them. [Plugins](../development/extensions/plugins.md) describes their local transport and checks.

## What the stack does by itself

The stack's own services (the API, the gateway, the worker and the node) run as the numeric user of the one who ran `first-start`, with a read-only root file system, no capabilities, `no-new-privileges` and a small temporary file system. The web edge starts as root only to bind its port, and keeps the capabilities that needs.

The Compose files publish only two ports on the host: the web edge (`127.0.0.1:5180` unless you change `ORCH_FRONTEND_BIND`) and the gateway's direct listener (9443 unless you change `ORCH_GATEWAY_PUBLISH`). The metrics listeners are not published. [Monitoring](monitoring.md) says how to reach them.

## What Orch connects to

Nothing connects out of an Orch system unless somebody configured what it connects to. [Running without internet access](offline.md#what-orch-contacts-and-what-to-use-instead) lists every outgoing connection, what it is for and what replaces it on an isolated network.

## Platforms

Environments that run directly on a person's machine are available on Linux. The macOS and Windows paths are not tested and are not offered, although macOS and Windows machines can still connect for their other roles.
