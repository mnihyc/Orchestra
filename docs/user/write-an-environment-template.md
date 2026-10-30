---
title: Write an environment template
screens: /resources/templates/$id, /resources/templates
keywords: template, environment, image, limits, network, docker, configuration, new template, provider runtime, example, env.template_id, runtime.template_id, 模板, 编写模板, 环境模板, 镜像, 资源限制, 网络, 配置, 示例
---

# Write an environment template

A template describes the image, tools and starting configuration of a managed
environment. It does not start a machine or an environment when you save it.
## Environment

An environment is shared by the threads placed in it: they see its workspace,
home and processes. Choose **Environment** when writing a template for that work.

## Provider

A provider runtime uses the same unit configuration for its home, tools and
credential, with no environment workspace. Choose **Provider** when
writing a template for that runtime.

## Make a first template

1. Open **Resources → Templates → New template**. Choose **Environment**,
   a name and the organization it belongs to.
2. Open **Start from an example**, then choose **Node**. The open picker explains
   that it is for a web project. This gives
   you the Docker driver and a complete configuration to edit: a Node image, four cores and 4 GiB of
   memory, with ordinary outgoing connections. Choosing another example replaces
   the configuration. The dialog asks only for the few starting choices.
3. Keep the **Image reference** for this first template, or choose
   another trusted image that contains the tools the project needs. Orch supplies
   its connector; choosing a template does not install the project's packages.
4. Choose **Create template**. The server checks the whole example and your edits
   before keeping them. A problem in a starting field appears beside it. The
   resulting template opens immediately; it starts private to you. Its **Access**
   tab lets you share it when it is ready.
5. Open **Configuration** for the remaining settings. Search by a field's name,
   meaning or value, then choose its section. **What it uses** holds limits;
   **Network** holds connection choices. Reduce limits for a small project or
   raise them for larger builds. An example's numbers are starting choices,
   not minimum requirements or the driver's defaults. Choose **Save template**
   after reviewing your edits.

For a program installed on the host, choose **Host** under **Start from an
example**. It selects the Host driver, fills in the `agent` account and runs
`/usr/bin/printf` after start. Change the account in the dialog, then open
**Configuration → Commands after start** on the created template to edit its
program. Choose an account and program installed on your hosts. It uses no
container image.

An enrolled host can be offline. The form names it and shows its recorded
disconnection time, when known. A driver's last reported form and examples
remain usable while its host is offline; reconnect a matching host before
starting work. If no host has reported a form yet, connect one so the form and
examples can be loaded. The single **Write an environment template** guide
button covers the choices and fields on this screen.

For Python with PostgreSQL, the example connects to a database managed
separately. Replace `db.example.internal` with your service's reachable address.
The example does not create that database. Select its credentials through the
place's secret names rather than putting them in the template's visible variables.

## Read the sections

**Image** chooses one source: an image from a registry, a Dockerfile built on a
host that offers builds, or an uploaded image archive. A tag can change; a digest
keeps the same image. Dockerfiles use the classic Docker builder. A build context
provides files from an uploaded archive or a public Git repository.

**Confinement** selects the container runtime and security options. Leaving the
runtime unset uses the host's default. Read-only image files still allow writes
to writable volumes. Tool confinement controls the agent's file access inside
the container. Additional capabilities, including KVM, need suitable host devices
and the host's policy or approval; a `kvm` host label alone grants no device.

**What it uses** sets resource limits and, for an environment, the ceilings to
which its manager may raise them. Processor time uses thousandths of a core:
1000 is one core. Memory uses MiB; disk uses GB. The host reports which limits it
can enforce. **Needs** says what must be free for placement; it does not reserve
capacity and does not set the unit's usage limits.

**Network** selects ordinary access or restricted outgoing connections.
Restricted reaches Orch and the names in the place's `network.allowed_hosts`
allow-list. Set that list in the place's settings before starting an environment,
and choose a host that supports restriction. Leave custom DNS empty for restricted
access. The **None** network connection reaches Orch alone through the host proxy.
An unsupported restriction is refused; it is never silently changed to open.

**Tools for agents** describes tools offered to an agent running elsewhere and which
agents may run here. Choose tools and agent applications actually available in the
image or from the host. The available setup also depends on the provider's
consent. **User** chooses an account in the image or numeric user and group IDs;
unset uses the image's own user. Match writable paths to that user's permissions.

**Variables** are visible plain text. Use the ordinary secret-name controls for
credentials. **Commands after start** lists named, repeatable commands run inside
the unit as its user, after attachment and before readiness. A command's arguments
are literal, its default deadline is 300 seconds, and failure fails the start.
Read [Run commands after a unit starts](commands-after-start.md) for ordering,
inherited commands and the host operator's separate hooks.

**Storage** sets the environment's workspace and home paths and any extra mounts.
Use absolute paths that do not overlap one another or Orch's directories. A named
volume keeps data; temporary memory storage is lost when the container is removed;
a bind mount exposes an existing directory on that host. A provider runtime keeps
its state volume and has no environment workspace or home-path fields. Its
**Registered programs** section describes programs installed in the provider image.

Each field is its own reference: a shared setting row shows its meaning, value
and source. **This template** means an explicit value; **Driver default** means
the driver's declared default. **Image, host or place** means that the template
and driver leave that choice to the eventual unit's configuration. A row's gear
shows the default once and offers **Reset to default** for an explicit value.
Reset removes only that value and keeps its siblings and other settings.
The field question adds bounds and examples where they help. Lists, variables
and other structured values open in their row. If you can only read the template,
the same rows show values and sources under one explanation of your access.
For a plugin driver, the explanations and starting examples ship with that driver.

## Use JSON when useful

On the **Configuration** tab, open **Details**, then switch to **JSON** to see
the same configuration in an editor with line numbers.
Property-name completion comes from the driver's schema. Hover a key to read its
meaning. The server checks the configuration after a short typing pause, using
the same rules as saving; a problem names its line, and selecting it goes there.
Nothing is saved by a check. Fix JSON syntax before switching back. Switching
views preserves values, including extra properties a driver permits; the form
lists unrecognised settings under **Other settings (JSON)**.
An unfinished JSON draft blocks switching views or saving. Fix it in its row,
or use **Discard changes** to return to the saved template.

## Choose a template for a place

Open the organization, project or topic's **Settings** and choose the environment
template (`env.template_id`). The choice follows the usual inherited settings, so
a folder or topic can choose its own. The maker must have permission to use the
template, and a suitable host must be available. Making an environment directly
also offers its template choice. A managed provider instead chooses its runtime
template (`runtime.template_id`) in its unit configuration.

For a single environment, choose **Add environment** on the template.
The dialog already selects this template; choose the place it belongs to and
optionally **Temporary** for disposable work. Read
[Temporary environments](temporary-environments.md) for its removal notice,
**Keep** and file export.

## Change a template already in use

Open the template's **Configuration** tab to edit its name, description or fields.
Saving changes the recipe for future starts and updates; it does not alter an
existing environment's running processes or files immediately. Existing units
keep their recorded configuration until an appropriate update or recreation is
reviewed and applied. Read the plan on that environment or provider runtime before
applying it, as described in [Change a running environment](change-a-running-environment.md).
If somebody saves first, Orch reads their revision again and keeps your typed
values for you to compare; it does not overwrite their work automatically.

Switching a template off stops new units being made from it and leaves existing
units running. A template still used by units cannot be deleted.
