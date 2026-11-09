---
group: work
screens: [/thread/$threadId, /resources/environments, /resources/environments/$id, /resources/providers/$id]
keywords: [environment, switch, move, machine, directory, copy, mounts, temporary, plan, kept]
summary: Distinguish changing a thread’s environment, moving an environment and changing its topic.
---

# Switch a thread or move an environment

**Move to another environment…** changes where one thread runs. **Move to another machine…** relocates a supported environment. **Move to topic…** changes a thread's place in the organization. Read the named action and plan to choose the change you intend.

## Where

Open a thread's menu or its Environment card to change where it runs. Start from [navigation](/) for a thread, or [Environments](/resources/environments) for the environment's machine move. Fleet lists can plan several environment moves together. [Switch threads](switch-threads.md) covers batches of thread changes.

Choose an existing environment or create a new one. **Change directory…** selects its working directory where offered. New environments can be temporary, with the shown idle or archive lifetime. Read who manages the environment and whether the thread's readers can be let in before creating it.

## Example

Move a build thread from a laptop environment to a larger environment. Choose whether to leave files there, copy into an existing environment, or make a new one. Review every mount's Keep, Detach or Copy choice and each MCP server's kept, newly available or unavailable state. Choose whether to wait for the current turn or interrupt it, then confirm the plan.

## What to expect

The plan states which session closes, what remains on the old side, and whether any old unit is removed. Progress and the resulting timeline name the actual environments and machines. Copying does not transfer an imported MCP server's machine-local secrets.

A template-made environment can use its supported machine-move path. A move involving a person's machine is a switch with a copy where the plan says so. Current permissions, machine availability and sandbox readiness still apply. Review unavailable choices and attention notices rather than treating a listed destination as ready. See [MCP import](mcp-import-and-use.md) and [running thread views](running-thread-views.md).
