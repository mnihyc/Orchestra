---
title: Register your own program
screens: /resources/machines, /resources/agents, /resources/providers
keywords: register, program, custom agent, adapter, provider source, command, GPU, describe, 注册, 程序, 代理适配器, 提供方数据源
---

# Register your own program

## 1. Choose a machine and kind

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
