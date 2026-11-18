---
group: getting-started
title: Register your own program
screens: /resources/machines, /resources/environments, /resources/agents, /resources/providers, /thread
keywords: register, program, custom agent, adapter, provider source, command, GPU, describe, Codex, Claude Code, agent version, 注册, 程序, 代理适配器, 提供方数据源, 智能体适配器
summary: Confirm an agent program that is installed on a machine, or register your own agent adapter or provider source.
---

# Register your own program

Orch can use an agent program that is installed on a connected machine, and it can run one that you write. This page shows how to confirm an installed program, how to allow a version of it in an environment, and how to register an agent adapter or a provider source of your own.

## Use an installed agent program

1. Open the **Programs** section of the connected machine. The agent programs found on that machine show their version and their path. **Version unknown** means that the program did not report a version that can be recognized.
2. Choose **Use** to confirm a program that was found. Its row then reads **Confirmed** and offers **Remove**.

These actions need **Manage** on the machine, and the local permission of the machine to manage agent programs. If that permission is kept locally, use the policy command that is shown for that machine. **Refresh** searches again and reads the current report of the machine.

Removing a confirmation keeps the program file. It stops the executors that use that program, and the threads affected are checked again before they run. A program that was discovered and not confirmed is not available for running.

## Allow the environment's program version

On the **Tools** page of the environment, **Codex tools** shows the version and the path of the confirmed program when it supplies the tools there. Its version normally has to match the version of the agent.

An environment manager can switch on **Any agent version**. The name of the current approver appears beside the switch. This allows the path and the version that are shown, **Version unknown** included. A path or a version that changes needs approval again. Switching it off restores matching of versions. The decision takes effect when a thread next opens a session. The versions that the adapter supports and the other rules of the environment still apply.

A thread that is blocked by different versions links to this page of the environment. After a manager allows the current program, the thread is checked again and continues if it can run. Environments made from a template keep the build of tools of their own.

## 1. Choose a machine and kind

To register an adapter or a provider source of your own, continue below.

Open the **Programs** section of the connected machine, choose **Agent adapter** or **Provider source**, then **Add**. The program must already be available on that machine. A path on your laptop does not name a file on a different host. Programs run as the account of the connector.

- An **agent adapter** wraps any agent program and translates sessions, inputs and results into the interface of Orch for agents. It must declare the modes, the formats of input and the ways of signing in that it supports.
- A **provider source** reports models, limits or usage for a model service. It need not run the model. It implements the interface of a provider source and declares which reports it supplies.

## 2. Give it a name and command

Enter a name for the registration and the executable with its arguments. Quote a path that contains spaces. The command is run directly, without a shell, so pipes and shell variables are not expanded.

Choose **Register and read details**. Orch asks the program for its declaration, saves the registration where the machine allows it, and shows what the program reports. If it fails, correct the command and try again. The name and the command you entered are kept.

## 3. Use the returned declaration

- For an agent, open **Add agent → A program on a machine** and select this adapter to create a profile.
- For a provider source, select it under **Models, limits and usage** while you add or edit a provider.
- Both flows can register a program inline, in this same registry.

The capabilities come from its declaration, and registering cannot give a program a mode it does not support. **Test** reads its declaration again. It does not prove that a model call or a turn of an agent works, so complete a small turn with the new profile or provider. **Remove** stops an entry that Orch manages and unregisters it, and keeps the files of the program itself. Change the profiles or providers that still use it.

## 4. If registration is kept on the machine

A local setting can keep registration under the control of the machine's operator. Orch shows that reason and offers **Reload** for registrations that exist locally, instead of pretending that **Register** can override it. Ask the operator to hand registration over if you want to manage it here. Administrator access does not override the choice that the machine keeps locally.

For the details of building one, read the [agent adapter contract](/help/agent-adapters) or the [provider source contract](/help/provider-readme).

## Choose an arrangement

The heading **Where the agent runs** in the setup of a thread opens this guide from its question. Read where the agent and its tools run, and whether a credential leaves the provider, before you choose. The ways on offer come from the declarations of the program, the provider and the environment, and a way that is unavailable keeps its reason. Choosing an arrangement gives no **Use** and does not change the limits of a provider. The first usable choice by rank may be suggested, and your own choice stays yours.

The [settings reference](settings-reference.md) lists every setting and structured field that comes with Orch. The [option field reference](option-reference.md) lists the options of the interfaces that come with Orch. The [state reference](state-reference.md) explains the states that are reported, and the [error reference](error-reference.md) explains the meaning of a refusal and how to recover. In **Help → Settings**, each full entry of a setting also gives its default, an example and the scopes it can be set at. Opening it from a control can show its real current source.

## Detected profile details

In **Add agent → A program on a machine**, the first program that was confirmed fills the field of the program. With **An available agent program**, a single choice in the catalogue is filled in, and several choices stay for you to pick. **From machine**, **From catalogue** and **Your choice** say where a value comes from. A refresh keeps the program, the name and the home you chose. If the program disappears, its name stays with **This program is no longer available**. Choose another before you create the profile. Changing the mode or the machine starts a new choice of program.

- **Version** is the version of the agent program installed, as that machine reports it, also for the declaration of a custom adapter. It is never the build number of the adapter itself or a build assumed from the catalogue. **Version unknown** means that no usable version was reported, and **Choose a machine to detect** means there is no observation of a machine yet. The version is an observation and not a new pin of a version.
- **Accounts** lists the providers of subscriptions that are registered for that program on that machine, with the state each reports. Sign-ins that exist are found in the usual way of discovery of the connector. A listed account does not prove a fresh sign-in and gives no **Use**. **No account reported** leaves that fact unknown, and it does not say that the machine has no sign-in. Open the card of an account to look at it.
- **Program supports** comes from a declaration of the machine that is ready where there is one, and otherwise from the catalogue. **Machine allows** shows apart the policy for arrangements that the machine reported. **None** is a list that is empty on purpose, and **Not reported** means that no list was supplied.
- **Arrangement → Automatic · From thread defaults** keeps the selector that is shared. At the setup of a thread it considers the real provider, environment, machine and access, puts the preferences configured first, and takes the first arrangement that is available and permitted. These details of the profile do not choose a route for a capability, copy a credential or turn on a service. An arrangement that you choose there stays your choice.
