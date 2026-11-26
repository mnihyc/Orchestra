---
group: operations
title: Switch threads
screens: /thread/$threadId, /resources/providers/$id, /resources/agents/$id, /resources/environments/$id, /admin
keywords: switch, batch, provider, profile, model, environment, history, carry, queued messages, switch.batch_threads
summary: Review a batch of threads that use one provider, and switch each thread that is eligible to the target you chose.
---

# Switch threads

To move many threads from one provider, profile, model or environment to another, you review one batch and switch each thread that can be switched.

## Switch a batch

1. Choose a provider, a model and, if you like, an agent profile.
2. Read the plan of each thread before you confirm. A thread that is blocked stays where it is, and the other threads can switch.

On a provider, the whole selection uses the threads that are bound to it now. Orch includes only the threads you may switch, and shows the rest as a count. Untick a thread to make an explicit selection. A thread you cannot see never gives you permission to change it.

Agent profiles and the models of a provider also have **Switch its threads…**. The act of a model selects that exact model at its provider. Switching away is available while the source is off. **Disable** keeps the threads bound, and **Delete** links to this separate act.

Typing searches the choices on offer. Committing a target reads its details. A provider or an environment that is switched off cannot be a new target. Choose another target, or turn it on first.

The plan shows how the history travels and which files move. Running work follows the option you chose, to wait or to interrupt. The progress and the results belong to the batch, and each thread has its own result. **Cancel** stops work before its step that cannot be undone.

## Batch size

A system administrator sets **Threads per batch**, which starts at 200. Raise it to plan a larger batch that you review. **Batch size** separately sets how many jobs run at once. A larger selection is refused with the name of the current setting. Orch never drops threads from the selection quietly.
