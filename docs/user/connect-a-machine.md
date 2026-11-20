---
group: getting-started
title: Connect a machine
screens: /resources/machines, /resources/machines/$id, /resources/hosts, /resources/hosts/$id, /resources/environments/$id, /thread/$threadId, /admin/$section
keywords: connect, computer, install, connector, node, service, foreground, another instance, data directory, root, gateway, region, Automatic, latency, tunnel.gateway_unreachable, 连接机器, 安装, 连接器, 节点, 前台, 多实例
summary: Connect a machine with the roles, folders and gateway you intend to use, and enroll it again or remove it later.
---

# Connect a machine

To connect a machine, you choose what it does, run one command on it, and wait until it shows up in Orch. This page goes through those steps, then explains the gateway choices, enrolling a machine again, and removing it.

Use the API, gateway and connector builds from the same update. An incompatible
framing version is reported before a session opens. Large model catalogues and
conversation history transfer automatically in parts.

## 1. Choose what runs here

Open **Resources → Machines → Add**, then choose its roles.

- **Environment** uses the folders you register.
- **Agent** runs agents.
- **Provider** keeps model credentials.

These three roles share one connector. **Node** is separate: it makes managed environments on a Linux host, and it is its own program, `orch-node`. To have both on one computer, add it as a host and as a machine. The connector refuses the **Node** role.

Choose the home where you may add it. It defaults to your own organization, and the machine starts private to you: being in the same organization does not give another member access. Connecting a machine does not share all its folders.

On a fresh Linux connector, **Made by Orch** creates workspace and home directories in the connector’s private data storage. **Existing directory** offers the roots the machine owner has explicitly shared. Named storage or shared roots keep their configured locations.

Linux directory environments work with the machine's ordinary policy. On macOS and Windows, go to **Machine → Policy → Native system review** to confirm the running connector build. Replacing the executable requires a fresh review. Revocation closes directory attachments and keeps their data. A registered agent program can run independently. On a reviewed Windows connector, shared-directory file operations and an existing loopback VNC server are available. Native process trees, terminals, mounts and listener forwarding have separate platform requirements. On macOS executors report the shell, installed program and sandbox modes they can actually use.

The host installed with the server is marked **Server host** in the host list and overview.
Its containers may share the server's kernel. Giving Host Use or handing over its ownership
requires the server-host warning. `orchctl doctor --config <file> --only node` reports whether
non-administrator accounts hold Host Use, including through higher permissions; an accepted
sharing choice does not make the node's health check fail.

## Choose a gateway

The forms **Add machine** and **Add host** offer **Gateway**: **Automatic**, or one of the labels that are advertised. The command passes your choice to the installer.

- **Automatic** measures from the machine being installed and picks the lowest measured round trip. It measures again when the primary connection reconnects, and also while connected, and it moves when a gateway is strictly better. It waits for the active channels on the primary and on the target attachment to end first. The browser does not measure the route of the machine.
A selected gateway stays selected when it is unavailable or removed from the
catalogue. On a connected machine or Host overview, managers can use **Change gateway**
in the **Connector** card, choose **Automatic** or an advertised gateway and **Save**.
The enrolled identity stays the same; no new token or installer run is needed.
The native answer is saved before reconnecting. The local console remains available,
and a later local choice supersedes an older remote completion.
Re-enrollment without a gateway flag retains the saved choice. The card shows the reported
primary and selection. Automatic also shows the gateway measurements and when
they were taken; **Not measured** is not a zero-latency result. **Refresh** reads
the latest report with its original measurement time.

## Gateway catalogue

System administrators use **Management → Gateways** to edit labels, regions and the endpoints that were verified. One stable ID names one gateway, and several endpoint addresses can be aliases of it. A region is the description of the operator. Renaming a label or changing an address keeps the selections by ID. Editing the catalogue advertises choices. It does not install a gateway and does not move a machine that is already connected.

The editor keeps the revision you opened. If another administrator changes the catalogue first, the draft that was refused stays in the form. **Cancel**, refresh and review the current catalogue before you save again.

## Tunnel gateway

The tunnel of a tool can use a different gateway from the primary one of the connector. The thread shows the gateway attached and confirmed when it differs, and the **Threads** rows of an environment offer the gateway details of each thread. A shared environment does not mean one gateway for every thread. **Connecting** names an attachment that is pending. **Not reported** never means the preferred candidate has already attached. Open **Tunnel gateway** to read the current report, and **Refresh** to read it again.

When no gateway that both machines share can connect them, the screen says the work is kept, and lists the gateways it tried with the refusal in safe terms. Check that the machines can reach those gateways, then use the usual recovery action.

A fallback can resume the connection of a tool through another shared gateway while both machines keep its original socket and bytes, within the existing one-minute grace. It continues the same work and does not run the tool again. A state that is missing, a ticket that expired, a permission for a port that was lost, or no shared gateway that can be reached, ends that connection and uses the usual recovery of interrupted work. The terminals, browsers and displays of people attach afresh. Neither saved primary choice changes: a selected primary may stay offline while a tunnel continues elsewhere. Admitting a new gateway still needs the API.

### Pairing measurements

A new tunnel between two machines uses the gateway with the lowest total of the round trips measured from both. Both sets of samples must be fresh enough for a live link, normally within 60 seconds. When a measurement is missing or stale, the primary gateway of the executor is used. The **Tunnel gateway** line shows the choice, its rule, the measured total when it is known, the time of each measurement and the limit on samples. A missing measurement stays missing.

The inherited **Tunnel gateway** setting can pin new tunnels of a place to the **Executor’s gateway**. It does not move the primary gateway you selected for a machine, change the plan that an existing tunnel has frozen, or send bytes through another gateway or through the API. A fallback that is shared still attaches both ends directly to one gateway that can be reached.

## 2. Choose its account and lifetime

A connector runs as **your account**, without sudo and without administrator rights. It works in the folders you register, whatever the current folder of the terminal is.

| | Program | Data |
|---|---|---|
| Linux | `~/.local/bin/orch-connector` | `~/.local/share/orch-connector` (or under `XDG_DATA_HOME`) |
| macOS | `~/.local/bin/orch-connector` | `~/Library/Application Support/Orch/connector` |
| Windows | `%LOCALAPPDATA%\Orch\orch-connector.exe` | `%LOCALAPPDATA%\Orch\connector` |

- **User service** keeps it running through systemd on Linux, launchd on macOS, or a logon task on Windows. On Linux it stops at logout unless an administrator allows user services to keep running after logout. On macOS and Windows it runs while you are signed in. Stop or start it with `orch-connector service stop` and `orch-connector service start`. On Windows, run `& "$env:LOCALAPPDATA\Orch\orch-connector.exe"` before `service stop` or `service start`.
- **Foreground** adds `--no-service` (`-NoService` on Windows). The complete command of the dialog installs the program without a service and starts it in the terminal, and Ctrl+C stops it. If you run the installer by hand, run the command the installer prints at its end, which ends in `run`. Stop any existing service first. Choosing **Foreground** does not remove one.
- **Another instance, named.** Several connectors can run side by side. Fill **Another instance, named** for a separate identity, data directory and service. The command adds `--data-dir` (`-DataDir` on Windows). Use the same path on the later commands for that instance, for example `service stop` or `service uninstall`, and leave it out for an installation made without it. The instances share the program file, so the others use an update after they restart. A node can run beside a connector, but it cannot be multiplied this way.

A node runs as **root**, one per machine: the program is `/usr/local/bin/orch-node`, the configuration `/etc/orch/orch.toml` and the data `/var/lib/orch-node`. **System service** uses `orch-node.service`. **Foreground** adds `--no-service` and starts the node in the terminal, as root, with `/usr/local/bin/orch-node --config /etc/orch/orch.toml run`; Ctrl+C stops it. The complete command of the dialog includes that start. Stop an existing service first. To serve other organizations, share the host through **Access**. Do not install a second node.

### Behind a proxy

The connector and the node reach the system, and the model services they forward to, through the proxy of the machine. They read the variables `HTTPS_PROXY`, `HTTP_PROXY`, `ALL_PROXY` and `NO_PROXY` (in upper or lower case) and, on macOS or Windows where none is set, the proxy settings of the system. They speak to proxies of the kinds `http://`, `socks5://` and `socks5h://` (with `socks5h` the proxy resolves names), with `user:password@` in the address where the proxy asks for them.

A service starts with the variables of its manager, not those of your shell. The installers of the connector for Linux and macOS, the installer of the node for Linux and `orch-connector service install` write the proxy variables of your shell into the service. The programs it starts (agent programs, `git`) inherit them. These service files can be read only by their owner, because the address of a proxy can hold a password.

- To use another proxy than the one of your shell, install the connector service with `--proxy socks5h://proxy.example:1080`, or with `--proxy none` to reach every host directly. Add `--no-proxy` for the hosts reached directly. Install again to change it.
- On Windows the logon task starts with the variables of your account. Set them with `setx`.
- `orch-connector doctor` shows through which proxy each address of the gateway is reached.

## 3. Connect and wait for its name

1. Choose the roles whose settings Orch may manage. Settings kept locally always win. For each role, **Hand over its settings to Orch** starts off, and turning it on hands the policy of that role over. The separate **Dedicated** switch (**Full control**) declares a virtual machine or a computer that holds nothing else and can serve as the isolation boundary. That is local consent, never a permission that Orch can turn on later.
2. Choose **Show command**, copy the command for the operating system of that computer, and run it there. The command changes with every choice, so use the complete command. Its token can be used once, is bound to your choices, and expires. Replace the command if it has expired. Choosing another home also makes a new command: use the one that is shown now.
3. When the first connection arrives, Orch confirms it, closes the dialog and opens the page of the machine. An enrollment that was saved and is still offline keeps waiting.

On Linux and macOS the command can hold `--roles environment,agent --delegate environment,agent --dedicated`. Windows uses `-Roles`, `-Delegate` and `-Dedicated`. `--delegate-policy` (`-DelegatePolicy`) hands every key of the policy over instead. The connector saves the roles and their sections of settings only after the enrollment is trusted. An older token without a choice of roles keeps the earlier default.

To change the roles later, the page of the machine shows `orch-connector roles set environment,agent,provider`. Edit the list, run it on that computer and restart the connector. Its reconnect reports the roles chosen. The page has no action to edit roles remotely. To add **Node**, enroll the host as a separate step.

If the dialog names a setting called `public_url` or `[[gateway.endpoints]]`, ask the system administrator to replace the example address in that setting. No command is available until those addresses are set. People and installers use the address people open. Machines use their own configured list, or the list of the system that they receive at each connection. Environments follow the list of their node unless its configuration names another.

Browsers and installers need a certificate that their device trusts. A machine also accepts the certificate of Orch for the Orch service that it names, under any address. For the authority of a company, ask the administrator to install its certificate on the device. Keep the checking of certificates on.

An installer allows an HTTP address of the system only for `127.0.0.0/8`, `[::1]` and `localhost`. That address reaches the computer that runs the command, so other machines need the HTTPS address of the system. The exception for a node applies to the download address of the system. Its connection to the gateway still uses TLS.

## Change driver settings

On the **Policy** tab of a node, open **Drivers** to edit its JSON once the operator of the machine has handed that setting over to Orch. For a driver that has reported a schema for its configuration, fields that are not valid are named beside the value; fix them and save again. The editor keeps your draft. If the driver has not reported a schema, Orch cannot check its connection fields here. A change to a connection takes effect after the node restarts.

## 4. Enroll the same machine again

For the machine of another person, an administrator first confirms **Override access**. This adds their name to the **Manage** row of the machine and records the override in the audit trail. It needs **As admin** in a browser session; a token, or a session with that mode off, gains no administrator permission. The grant by name remains until it is removed from **Access**. The policy that the machine keeps locally still applies when its roles or settings change.

Open the page of the machine and choose **Enroll again** if its certificate was revoked, or if it expired while the machine was away for longer than the grace for renewal. A certificate still within the grace renews when the machine next connects.

Run the command of the dialog on that machine, with its existing data directory. It replaces the identity in place and keeps the settings, the policy, the registrations, the name and the page. A foreground instance that is running loads the new identity. One that is stopped starts with it next time. The dialog says when the same machine has returned. The old certificate stops working when the new one is first used.

If its disk was lost, run the same command on the replacement with a fresh data directory. Orch keeps the same page for the machine. A command from **Add machine** or **Add host** cannot replace an identity that exists; use **Enroll again** for that machine.

## 5. Stop or remove it

The **Installation** section of the page of the machine has its real paths and service commands.

- **A connector.** A default connector uses `orch-connector service stop`, `service start` or `service uninstall`. For an instance that has a name, put the same `--data-dir` first. On Windows run its executable with `&`.
- **A node.** Use `systemctl stop orch-node.service` or `systemctl start orch-node.service`.

To remove a **connector** completely, revoke it in Orch before you uninstall its service with `orch-connector service uninstall` (with the same `--data-dir` for an instance that has a name). That stops and removes the service and keeps the data and the identity. Look at its data before you delete it. Keep the work folders and the other instances, including nested `instances/` directories. Remove the shared program only after the last connector is gone.

To remove a **node**, first remove its environments and volumes, then the host, in Orch. Then, as root, run `systemctl disable --now orch-node.service`, remove `/etc/systemd/system/orch-node.service` and run `systemctl daemon-reload`. Look at the program and the data of this node, and remove them. Keep `/etc/orch/orch.toml` if another Orch program uses it. An installation that runs in the foreground has no service to remove. Removing only a service keeps the enrollment and the stored data, and it does not remove any work. See [updates](updates.md) for automatic updates and the local command to update.

To use a program already signed in on this computer, choose **Use an installed
sign-in** in Add machine before generating the command. The existing role
controls show Agent, Provider and Environment, with Agent program control on;
you can review those choices before running the command. After it connects,
choose the detected program and its home, then Register. Orch confirms its
reported path and registers the profile and provider together. It preserves
the program's existing sign-in. If provider registration fails after the profile
was added, Try again reuses that profile. The machine's Registered here section
offers the same flow. The provider page shows any remaining sign-in or model
selection before you start a thread.
