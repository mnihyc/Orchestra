---
title: DeepSeek Harness
screens: /resources/agents, /resources/agents/$id, /thread/$threadId, /admin/$section
keywords: DeepSeek Harness, dsh, DeepSeek, OpenRouter, Agent Client Protocol, privacy, telemetry, streaming, plugin, program, key, provider
---

# DeepSeek Harness

DeepSeek Harness (`dsh`) is DeepSeek's open-source agent program. Orch runs it like Codex and Claude Code: it works in
the thread's environment, uses Orch's tools, and asks before it changes things when the thread's approvals say so.

## What it is

Orch starts one `dsh` for each thread and talks to it over the Agent Client Protocol. The build comes from PyPI, checked
by its SHA-256, and is kept once in the machine's tool cache; it needs no Python or Node.js. Add it like any agent: open
**Resources → Agents**, choose **DeepSeek Harness**, and pick an environment that lets it run.

## Models and providers

Every model request goes through Orch's proxy, which holds the key. Give the environment a key for DeepSeek,
OpenRouter, or an OpenAI Chat, OpenAI Responses or Anthropic endpoint; the thread then offers the models of that
provider. Without a key for one of these, the thread says so and what to do: give the environment a key for a provider
the program supports.

DeepSeek account sign-in is not offered yet. Use a key.

A provider that offers only Responses or Anthropic works without Chat Completions. When a provider offers both
OpenAI protocols, dsh uses Chat Completions. If the program refuses a session, check its provider and settings,
then retry; the thread keeps the program's own reason.

## What works

- Finished messages, reasoning, tool calls with their output, the plan list and how full the context is.
- Approvals: where a command asks to leave dsh's own limits, you see the command and its reason and answer once or for good.
- Stop: the running turn ends as stopped; a program that does not stop is ended and the thread says the session was lost.
- Continue: a thread opens the program again with its earlier conversation. The first turn after that takes a few
  seconds longer.
- If the program ends while nobody waits for it (it crashes, is stopped, or ends after standing idle), the thread says
  how it ended, with its exit code or the signal. Your next message starts it again with the earlier conversation.
- Orch's tools and the environment's own servers are given to it by address, with the thread's token.
- Images, where the model takes them, and the thread's choice of model and effort.

## What the plugin adds

Orch ships a plugin that runs inside dsh and adds what the protocol does not carry. The program says which of
these work in the build you have; the **Harnesses** page lists what is off and why.

- Live text and reasoning as the model writes it.
- Adding to a running turn.
- Questions to you (`ask_user_question`).
- Plan mode that changes nothing until you press Proceed, and Keep planning with your words.
- Asking before every change under Ask approvals.
- Token counts for each model call.
- Compaction phases.
- Skills from the thread's skills.
- Carrying a thread's history into dsh.
- Commands and files in the thread's environment, when it is separated from the provider.

Goals use Orch's goal controls and tool. dsh's own goal tools and automatic continuations are disabled so every turn
passes the thread's approvals, limits and pause controls.

Without the plugin, or with an extra off, the rest keeps working: that part is plain protocol.

When switching into dsh, History carries the whole conversation by default; a selected turn carries only the conversation through that turn. If import fails, retry with Summary or None, or ask an operator to check the program’s history support.

## Privacy defaults

- Log upload is off: dsh does not send the log of the session to its vendor.
- The plugin list is off: it does not send the list of its plugins.
- Telemetry is off.
- Traffic goes through Orch's proxy: the only requests that leave an environment are model requests, and the headers
  that name the client are removed from them. A request that carries the log or the plugin list is refused whole.

## Limits

- DeepSeek account sign-in is not offered yet.
- It has no modes or commands list of its own; Orch's plan mode and approvals come from the plugin.
- Fork at a turn is not offered yet.
- A thread's model list is the provider's; a model the program does not offer is refused before the turn starts.
- The build is about 400 MB once unpacked, kept once for each version on a machine.
