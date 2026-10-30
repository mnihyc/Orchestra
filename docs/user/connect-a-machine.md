---
title: Connect a machine
screens: /resources/machines, /resources/machines/$id, /resources/hosts
keywords: connect, computer, install, connector, node, service, foreground, another instance, data directory, root, 连接机器, 安装, 连接器, 节点, 前台, 多实例
---

# Connect a machine

## 1. Choose what runs here

Open **Resources → Machines → Add**, then choose its roles. **Environment** uses folders
you register; **Agent** runs agents; **Provider** keeps model credentials. These roles share
one connector. **Node** is separate: it makes managed environments on a Linux host.
Choose the home where you may add it. Connecting a machine does not share all its folders.

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

A service starts with its manager's variables, not your shell's: `orch-connector service install`
writes the proxy variables of the shell you run it from into the service, and the programs it
starts (agent programs, `git`) inherit them. To use another proxy than the shell's, install with
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
