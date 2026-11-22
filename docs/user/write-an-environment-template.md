---
group: resources
title: Write an environment template
screens: /resources/templates/$id, /resources/templates
keywords: template, environment, image, limits, network, docker, configuration, new template, managed provider, example, env.template_id, runtime.template_id, 模板, 编写模板, 环境模板, 镜像, 资源限制, 网络, 配置, 示例
summary: Describe the image, the tools and the starting settings of a managed environment in a template you can reuse.
---

# Write an environment template

A template describes the image, the tools and the starting configuration of a managed environment. Saving a template does not start a machine or an environment. A template is for one of two kinds of work, and you choose which when you make it.

## Environment

Choose **Environment** when the template is for work that threads share. The threads placed in such an environment see its workspace, its home and its processes.

## Provider

Choose **Provider** when the template is for a managed provider. A provider uses the same container configuration for its home, its tools and its credential, and it has no workspace of an environment.

## Make a first template

1. Open **Resources → Templates → New template**. Choose **Environment**, a name and the organization the template belongs to.
2. Open **Start from an example** and choose **Node**. The picker explains that it is for a web project. It gives you the Docker driver and a complete configuration to edit: a Node image, four cores and 4 GiB of memory, with ordinary outgoing connections. Choosing another example replaces the configuration. The dialog asks only for the few starting choices.
3. Keep the **Image reference** for this first template, or choose another trusted image that has the tools the project needs. Orch supplies its connector. Choosing a template does not install the packages of the project.
4. Choose **Create template**. The server checks the whole example and your edits before it keeps them. A problem in a starting field appears beside the field. The new template opens at once, and it starts private to you. Its **Access** tab lets you share it when it is ready.
5. Open **Configuration** for the other settings. Search by the name, the meaning or the value of a field, then choose its section. **What it uses** holds the limits, and **Network** holds the choices about connections. Lower the limits for a small project, or raise them for larger builds. The numbers of an example are starting choices. They are not minimum requirements and not the defaults of the driver. Choose **Save template** after you review your edits.

For a program that is installed on the host, choose **Host** under **Start from an example**. It selects the Host driver, fills in the `agent` account and runs `/usr/bin/printf` after the start. Change the account in the dialog, then open **Configuration → Commands after start** on the template you created to edit its program. Choose an account and a program that are installed on your hosts. It uses no container image.

An enrolled host can be offline. The form names it and shows when it went offline, if that is known. The last form and the examples that a driver reported stay usable while its host is offline. Reconnect a matching host before you start work. If no host has reported a form yet, connect one so that the form and the examples can load. The single **Write an environment template** guide button covers the choices and fields on this screen.

For Python with PostgreSQL, the example connects to a database that is managed separately. Replace `db.example.internal` with an address of your service that can be reached. The example does not create that database. Choose its credentials through the secret names of the place instead of putting them in the variables of the template, which anyone can read.

## Read the sections

**Image** chooses one source: an image from a registry, a Dockerfile built on a host that offers builds, or an uploaded archive of an image. A tag can change; a digest keeps the same image. Dockerfiles use the classic Docker builder. A build context provides files from an uploaded archive or from a public Git repository.

The built-in **Default** and **Developer tools** templates have Dockerfiles you can edit under **Image**. Their recipes need no build context. A host that offers builds makes the image you chose the first time it is needed and keeps it for later environments, and the job shows its progress. The first build needs network access to the sources of packages and downloads of the recipe. **Developer tools** includes the **Default** recipe, so no **Default** image has to be built first.

**Confinement** selects the container runtime and the security options. Leaving the runtime unset uses the default of the host. Files of the image that are read-only still allow writes to volumes that are writable. Tool confinement controls the file access of the agent inside the container. Extra capabilities, KVM included, need suitable devices on the host and the policy or approval of the host. A host label `kvm` alone grants no device.

**What it uses** sets limits on resources and, for an environment, the ceilings to which its manager may raise them. Processor time is in thousandths of a core: 1000 is one core. Memory is in MiB and disk in GB. The host reports which limits it can enforce. **Needs** says what must be free for the placement. It reserves no capacity and sets no limit on what the container uses.

**Network** selects ordinary access or restricted outgoing connections. Restricted reaches Orch and the names on the allowed list of hosts in the settings of the place. Set that list before you start an environment, and choose a host that supports restriction. Leave custom DNS empty for restricted access. The **None** network reaches Orch alone, through the proxy of the host. A restriction that is not supported is refused. It is never changed quietly to open.

**Tools for agents** describes the tools offered to an agent that runs elsewhere and which agent programs may run here. Choose tools and agent programs that are really available in the image or from the host. What can be set up also depends on the consent of the provider. **User** chooses an account in the image, or numeric user and group IDs. Unset uses the user of the image itself. Match the permissions of paths that are writable to that user.

**Variables** are plain text that anyone can read. Use the secret-name controls for credentials. **Commands after start** lists commands with names that can be run again. They run inside the container as its user, after the attachment and before it is ready. The arguments of a command are taken as they are, its default deadline is 300 seconds, and a failure fails the start. [Run commands after a container starts](commands-after-start.md) explains the order, the commands inherited and the separate hooks of the operator of the host.

**Storage** sets the paths of the workspace and the home of the environment and any extra mounts. Use absolute paths that do not overlap each other or the directories of Orch. A named volume keeps data. Temporary storage in memory is lost when the container is removed. A bind mount shows a directory that exists on that host. A managed provider keeps its state volume and has no fields for a workspace or a home path. Its **Registered programs** section describes the programs installed in the image of the provider.

Each field is its own reference. The row of a shared setting shows its meaning, its value and its source. **This template** means a value set here. **Driver default** means the default the driver declared. **Image, host or place** means that the template and the driver leave that choice to the configuration of the container that will run. The gear of a row shows the default once and offers **Reset to default** for a value set here. Reset removes only that value and keeps its siblings and the other settings. The question mark of a field adds bounds and examples where they help. Lists, variables and other structured values open in their row. If you can only read the template, the same rows show the values and the sources under one explanation of your access. For a driver that is a plugin, the explanations and the starting examples come with that driver.

## Use JSON when useful

On the **Configuration** tab, open **Details**, then switch to **JSON** to see the same configuration in an editor with line numbers.

- Names of properties are completed from the schema of the driver. Hover a key to read what it means.
- The server checks the configuration after a short pause in typing, with the same rules as saving. A problem names its line, and selecting it goes there. A check saves nothing.
- Fix the JSON syntax before you switch back. Switching views keeps the values, including extra properties that a driver allows. The form lists settings it does not know under **Other settings (JSON)**.
- A JSON draft that is unfinished blocks switching views and saving. Fix it in its row, or use **Discard changes** to return to the saved template.

## Choose a template for a place

Open the **Settings** of the organization, project or topic and choose the environment template. The choice follows the usual inherited settings, so a folder or a topic can choose its own. The person who makes an environment must be allowed to use the template, and a suitable host must be available. Making an environment directly also offers the choice of template. A managed provider chooses the template of its runtime in its container configuration instead.

For a single environment, choose **Add environment** on the template. The dialog has this template selected. Choose the place the environment belongs to and, if you like, **Temporary** for work that is disposable. [Temporary environments](temporary-environments.md) explains the notice before removal, **Keep** and exporting files.

## Change a template already in use

Open the **Configuration** tab of the template to edit its name, its description or its fields. Saving changes the recipe for later starts and updates. It does not change the processes or the files of an environment that exists. The containers that exist keep the configuration recorded for them until an update or a recreation is reviewed and applied. Read the plan on that environment or provider before you apply it, as [Change a running environment](change-a-running-environment.md) describes. If somebody saves first, Orch reads their revision again and keeps the values you typed for you to compare. It never overwrites their work automatically.

Turning a template off stops new containers from being made from it and leaves the existing ones running. A template that containers still use cannot be deleted.
