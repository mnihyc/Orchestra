---
group: getting-started
title: Connect a machine
screens: /resources/machines, /resources/machines/$id, /resources/hosts, /resources/hosts/$id, /resources/environments/$id, /thread/$threadId, /admin/$section
keywords: connect, computer, install, connector, node, service, foreground, another instance, data directory, root, gateway, region, Automatic, latency, tunnel.gateway_unreachable, 连接机器, 安装, 连接器, 节点, 前台, 多实例
summary: Connect a machine with the roles, folders and gateway you intend to use.
---

# Connect a machine

Use the API, gateway and connector builds from the same update. An incompatible
framing version is reported before a session opens. Large model catalogues and
conversation history transfer automatically in parts.

## 1. Choose what runs here

Open **Resources → Machines → Add**, then choose its roles. **Environment** uses folders
you register; **Agent** runs agents; **Provider** keeps model credentials. These roles share
one connector. **Node** is separate: it makes managed environments on a Linux host.
Choose the home where you may add it. Connecting a machine does not share all its folders.
Directory environments are available on Linux. macOS and Windows connectors can still connect
for their other roles.

## Choose a gateway

The Add machine and Add host forms offer **Gateway**: **Automatic** or one of the
advertised labels. The command passes that choice to the installer. Automatic
measures from the machine being installed and chooses the lowest measured round
trip, measuring again when its primary connection reconnects. The browser does
not measure the machine's route. Automatic also rechecks while connected, and moves
when a strictly better measured gateway is available. It waits for active channels
on the primary and target attachment to finish; a selected primary never moves
on its own.

A selected gateway stays selected when it is unavailable or removed from the
catalogue. Change that choice in the machine's local console or installer when
you mean to replace it. Re-enrollment without a gateway flag retains the saved
choice. The machine or Host overview's **Connector** card shows the reported
primary and selection. Automatic also shows the gateway measurements and when
they were taken; **Not measured** is not a zero-latency result. **Refresh** reads
the latest report with its original measurement time.

## Gateway catalogue

System administrators use **Management → Gateways** to edit labels, regions and
verified endpoints. One stable ID names one gateway; several endpoint addresses
are aliases for it. A region is an operator's description. Renaming a label or
changing an address preserves selections by ID. Editing the catalogue advertises
choices; it does not install a gateway or move an already connected machine.

The editor keeps the revision opened for review. If another administrator changes
the catalogue first, the refused draft stays in the form. Cancel, refresh and
review the current catalogue before saving again. Removing a selected gateway
does not change that machine to Automatic.

## Tunnel gateway

A tool tunnel can use a different gateway from the connector's primary. The
thread shows the confirmed attached gateway when it differs; an environment's
Threads rows offer each thread's own gateway details. A shared environment does
not imply one gateway for every thread. **Connecting** names a pending attachment;
**Not reported** never means the preferred candidate has already attached. Open
**Tunnel gateway** to inspect the current report; **Refresh** reads it again.

When no common gateway can connect the machines, the screen says that the work
is kept and lists the attempted gateways with the safe refusal detail. Check the
machines' access to those gateways, then use the ordinary recovery action. A
fallback can resume a tool connection through another shared gateway while both
machines retain its original socket and bytes, within the existing one-minute
grace. It continues the same work; it does not run the tool again. Missing state,
an expired ticket, a lost port permission or an unreachable common gateway ends
that connection and uses the normal interrupted-work recovery. People's terminal,
browser and display attachments still attach afresh. Neither saved primary
choice changes; a selected primary may stay offline while a tunnel-only connection
continues elsewhere. New gateway admission still needs the API.

### Pairing measurements

New machine-to-machine tunnels use the gateway with the lowest combined measured round trip from both machines. Both sets of samples must be within the live-link freshness bound, normally 60 seconds; missing or stale measurements use the executor’s primary gateway. The **Tunnel gateway** line shows the choice, its rule, the measured sum when known, each measurement time and the sample limit. A missing measurement stays missing.

The inherited **Tunnel gateway** setting (`gateways.tunnel_strategy`) can pin new tunnels to **Executor’s gateway** for a place. It does not move a machine’s selected primary, change an existing tunnel’s frozen plan or send bytes through another gateway or the API. A common fallback still attaches both ends directly to one reachable gateway.

## 2. Choose its account and lifetime

A connector runs as **your account**, without sudo or administrator rights. Its program is
`~/.local/bin/orch-connector` on Linux/macOS, or `%LOCALAPPDATA%\Orch\orch-connector.exe`
on Windows. Data is `~/.local/share/orch-connector` on Linux (respecting `XDG_DATA_HOME`),
`~/Library/Application Support/Orch/connector` on macOS, or `%LOCALAPPDATA%\Orch\connector`.

**User service** keeps it running through systemd on Linux, launchd on macOS, or a Windows
logon task. Linux stops at logout unless lingering is enabled; macOS and Windows run while
you are logged in. The dialog's **Foreground** command installs without a service and starts
the program in this terminal; Ctrl+C stops it. Stop any existing service first.

Several connectors can run side by side. Fill **Another instance, named** for a separate
identity, data directory and service; the command adds `--data-dir` (`-DataDir` on Windows).
Keep that same path on later commands for this instance. They share the program file.

A node runs as **root**, one per machine: `/usr/local/bin/orch-node`, configuration
`/etc/orch/orch.toml`, data `/var/lib/orch-node`. **System service** uses `orch-node.service`;
**Foreground** starts it in the terminal as root instead. Share the host through **Access** to
serve other organizations; do not install a second node.

### Behind a proxy

The connector and the node reach the system, and the model services they forward to, through the
machine's proxy: the `HTTPS_PROXY`, `HTTP_PROXY`, `ALL_PROXY` and `NO_PROXY` variables (upper or
lower case), and on macOS or Windows, where none is set, the proxy settings of the system. They speak
to `http://`, `socks5://` and `socks5h://` proxies (with `socks5h` the proxy resolves names), with
`user:password@` in the address where the proxy asks for them.

A service starts with its manager's variables, not your shell's. The Linux and macOS connector
installers, the Linux node installer and `orch-connector service install` write the shell's proxy
variables into the service; the programs it starts (agent programs, `git`) inherit them. These
service files are readable only by their owner because a proxy address can contain a password.
To use another proxy than the shell's, install the connector service with
`--proxy socks5h://proxy.example:1080` (or `--proxy none` to reach every host directly) and
`--no-proxy` for the hosts reached directly; install again to change it. On Windows the logon task
starts with your account's variables: set them with `setx`. `orch-connector doctor` shows the proxy
each address of the gateway is reached through.

## 3. Connect and wait for its name

Choose the roles whose settings Orch may manage; locally kept settings always win.
Use **Full control** when the computer is reserved for Orch and can serve as its isolation boundary. Choose **Show command**, copy the
command for that computer's operating system, and run it there. Its one-use token expires;
replace the command if it has expired. When the first connection arrives, Orch confirms it,
closes the dialog and opens the machine's page automatically. A saved enrollment that is still
offline keeps waiting.

If the dialog names `public_url` or `[[gateway.endpoints]]`, ask the system administrator to
replace that setting's example address. No command is available until those addresses are set.
People and installers use the address people open. Machines use their own configured list, or
the system's list received at each connection. Environments follow their node's list unless
its configuration names another.

Browsers and installers need a certificate their device trusts. A machine also accepts Orch's
own certificate for the Orch service it names, under any address. For a company's authority,
ask the administrator to install its certificate on the device. Keep certificate verification on.

Installers allow an HTTP system address only on `127.0.0.0/8`, `[::1]` or `localhost`.
That address reaches the computer running the command; other machines need the system's HTTPS
address. A node's exception applies to its system download address; its gateway connection still uses TLS.

## Change driver settings

On a node's **Policy** tab, open **Drivers** to edit its JSON after the machine's operator has
handed that setting to Orch. For a driver that has reported a configuration schema, invalid fields
are named beside the value; fix them and save again. The editor keeps your draft. If the driver has
not reported a schema, Orch cannot check its connection fields here. A connection change takes
effect after the node restarts.

## 4. Enroll the same machine again

For another person's machine, an administrator first confirms **Override access**. This adds
their name to the machine's **Manage** row and records the override in the audit. It requires
**As admin** in a browser session; a token or a session with that mode off gains no administrative
permission. The named grant remains until removed from **Access**. The machine's locally kept
policy still applies when its roles or settings are changed.

Open the machine's page and choose **Enroll again** if its certificate was revoked, or if it
expired while the machine was away for more than the renewal grace. A certificate within
the grace renews when the machine next connects.

Run the dialog's command on that machine, using its existing data directory. It replaces the
identity in place and keeps the local settings, policy, registrations, name and page. A running
foreground instance loads the new identity; a stopped instance starts with it next time.
The dialog says when that same machine has returned. The old certificate stops working when
the new one is first used.

If its disk was lost, use the same command on its replacement with a fresh data directory.
Orch keeps the same machine page. A command from **Add machine** or **Add host** cannot replace
an existing identity: use **Enroll again** for that machine instead.

## 5. Stop or remove it

The machine's **Installation** section has its actual paths and service commands. A default
connector uses `orch-connector service stop`, `service start` or `service uninstall`;
prepend the same `--data-dir` for a named instance. On Windows invoke its executable with `&`.
For a node use `systemctl stop orch-node.service` or `systemctl start orch-node.service`.

For complete removal, revoke a connector in Orch before uninstalling its service. Inspect and
remove only its data, keeping work folders and other instances, including nested ones; remove
the shared program only after the last connector. For a node, remove its environments and
volumes, then the host, in Orch; disable and remove its service before removing its program
and data. Keep configuration used by other Orch programs. Service removal alone keeps identity
and data. See [updates](updates.md) for automatic updates and the local update command.
