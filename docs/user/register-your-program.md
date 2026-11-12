---
group: getting-started
title: Register your own program
screens: /resources/machines, /resources/environments, /resources/agents, /resources/providers, /thread
keywords: register, program, custom agent, adapter, provider source, command, GPU, describe, Codex, Claude Code, agent version, 注册, 程序, 代理适配器, 提供方数据源
summary: Confirm an installed program or register an agent adapter or provider source.
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


## Choose an arrangement

The **Where the agent runs** heading in thread setup opens this guide from its existing question. Read where the agent and tools run and whether a credential leaves the provider before choosing. The offered ways come from the program, provider and environment declarations; an unavailable way keeps its reason. A selected arrangement does not grant Use or change a provider's limits. The first ranked usable choice may be suggested, and your explicit choice stays yours.

Use the [settings reference](settings-reference.md) for every built-in setting and structured field, the [option field reference](option-reference.md) for shipped interface options, the [state reference](state-reference.md) for reported states, and the [error reference](error-reference.md) for a refusal's meaning and recovery. In Help → Settings, each full setting entry also gives its default, example and allowed scopes. Opening it from a control can show its actual current source.

## Detected profile details

In **Add agent → A program on a machine**, the first confirmed program fills the program field. With **An available adapter**, a sole catalogue choice is filled; several choices stay yours to pick. **From machine**, **From catalogue** and **Your choice** identify the source. A refresh keeps the chosen program, name and home. If the program disappears, its name stays with **This program is no longer available**; choose another before creating the profile. Changing the mode or machine starts a new program choice.

**Version** is the installed harness version reported by that machine, including a custom adapter's harness declaration. It is never the adapter's own build number or an assumed catalogue build. **Version unknown** means no usable version was reported; **Choose a machine to detect** means there is no machine observation yet. The version is an observation, not a new version pin.

**Accounts** lists already registered subscription providers for that program on that machine, with each provider's reported state. Existing sign-ins are discovered through the connector's normal discovery path. A listed account does not prove a fresh login or grant Use. **No account reported** leaves that fact unknown; it does not say the machine has no sign-in. Open the account's card to inspect it.

**Program supports** comes from a ready machine declaration where available, otherwise from the catalogue; **Machine allows** separately shows the machine's reported arrangement policy. **None** is an explicitly empty list, and **Not reported** means no list was supplied. **Arrangement → Automatic · From thread defaults** keeps the existing shared selector: at thread setup it considers the actual provider, environment, machine and access, puts the configured preferences first, and takes the first available and permitted arrangement. These profile facts do not select a capability route, copy a credential or enable a service. An explicit arrangement selected there remains the person's choice.
