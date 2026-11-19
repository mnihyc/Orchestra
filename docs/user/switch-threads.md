---
title: Switch threads
screens: /thread/$threadId, /resources/providers/$id, /resources/agents/$id, /resources/environments/$id, /admin
keywords: switch, batch, provider, profile, model, environment, history, carry, queued messages, switch.batch_threads
---

# Switch threads

## Switch a batch

Choose a provider, model and optional profile. Read each thread's plan before
confirming. A blocked thread stays where it is. The other threads can switch.

On a provider, the whole selection uses its current bound threads. Orch includes
only threads you may switch. The rest appear as a count. Untick a thread to make
an explicit selection. A hidden thread never grants permission to change it.

Profiles and provider models also have **Switch its threads…**. A model's act
selects that exact model at its provider. Switching away is available while the
source is off. Disable keeps the threads bound; Delete links this separate act.

Local typing searches the offered choices. Committing a target reads
its facts. A provider or environment that is switched off cannot be a new target.
Choose another target, or enable it first.

The plan shows how history travels and what files move. Running work follows
the selected wait or interrupt option. Progress and results belong to the batch;
each thread has its own result. Cancel stops work before its irreversible step.

## Batch size

A system administrator sets **Threads per batch** (`switch.batch_threads`).
It starts at 200. Raise it to plan a larger reviewed batch. **Batch size**
(`switch.batch_parallel`) separately sets how many jobs run at once.

A larger selection is refused with the current setting's name. Orch never
silently drops threads from the selection.
