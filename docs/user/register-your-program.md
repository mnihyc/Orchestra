---
title: Register your own program
screens: /resources/machines, /resources/environments, /resources/agents, /resources/providers, /thread
keywords: register, program, custom agent, adapter, provider source, command, GPU, describe, Codex, Claude Code, agent version, 注册, 程序, 代理适配器, 提供方数据源
---

# Register your own program

## Use an installed agent program

Open the connected machine's **Programs** section. Agent programs found on that machine show
their version and path. **Version unknown** means the program did not report a recognizable version.
Choose **Use** to confirm a found program; its row then reads **Confirmed** and offers **Remove**.
These actions need Manage on the machine and its local permission to manage agent programs.
If that permission is kept locally, use the policy command shown for that machine. **Refresh**
searches again and reads the machine's current report.

Removing a confirmation keeps the program file. It stops executors using that program, and
affected threads are checked again before they run. An unconfirmed discovery is not available
for execution.

## Allow the environment's program version

On the environment's **Tools** page, **Codex tools** shows the confirmed program's version and
path when it supplies the tools there. Its version normally must match the agent's version.
An environment manager can switch on **Any agent version**; the current approver's name appears
beside the switch. This allows the displayed path and version, including **Version unknown**.
A changed path or version needs approval again. Switching it off restores version matching.
The decision takes effect when a thread next opens a session.
The adapter's supported versions and the environment's other rules still apply.

A thread blocked by different versions links to this environment page. After a manager allows
the current program, the thread is checked again and continues if it can run. Environments made
from a template keep their own tools build.

## 1. Choose a machine and kind

To register an adapter or provider source of your own, continue below.

Open the connected machine's **Programs** section and choose **Agent adapter** or **Provider source**,
then **Add**. The program must already be available on that machine; a path on your laptop
does not name a file on a different host. Programs run as the connector's account.

An **agent adapter** wraps any agent program and translates sessions, inputs and results into
Orch's agent interface. It must declare the modes, input formats and sign-in methods it supports.
A **provider source** reports models, limits or usage for a model service; it need not run the model.
It implements the provider-source interface and declares which reports it supplies.

## 2. Give it a name and command

Enter a registration name and the executable with its arguments. Quote a path containing spaces.
The command is executed directly, without a shell: pipes and shell variables are not expanded.
Choose **Register and describe**. Orch asks the program for its declaration, saves the registration
where the machine allows it, and shows what it reports. On failure, correct the command and try again;
your entered name and command are kept.

## 3. Use the returned declaration

For an agent, open **Add agent → A program on a machine** and select this adapter to create a
profile. For a provider source, select it under **Models, limits and usage** while adding or
editing a provider. Both flows can register a program inline, using this same registry.
Capabilities come from its declaration; registration cannot give it an unsupported mode.

**Test** reads its declaration again. It does not prove a model call or agent turn works;
complete a small turn using the new profile or provider. **Remove** stops and unregisters an
Orch-managed entry but keeps the program's own files. Change profiles or providers that still use it.

## 4. If registration is kept on the machine

A local setting may keep registration under the machine operator's control. Orch shows that
reason and offers **Reload** for existing local registrations, rather than pretending Register
can override it. Ask the operator to hand registration over if you want to manage it here.
Administrator access does not override the machine's local choice.

For implementation details, read the [agent adapter contract](/help/agent-adapters) or
[provider source contract](/help/provider-readme).
