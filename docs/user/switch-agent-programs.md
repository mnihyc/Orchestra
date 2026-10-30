---
title: Switch agent programs with history
screens: /thread/$threadId, /resources/environments/$id
keywords: switch, fork, Carry, Summary, None, history, turns, handoff, transcript, context, agent program
---

# Switch agent programs with history

## Open the plan

Switch changes the current thread's provider or agent setup. Fork makes another
thread; Fork to opens the plan for a different destination or history. Choose
the offered provider, profile, model and environment, then read their current
requirements. A disabled choice states what is missing.

A plain directory is a normal workspace. Git is optional. A Git-only action
says it needs a repository where you choose it; preparing one is not a condition
for an ordinary turn.

The plan shows the target approval mode. It keeps the place's mode when the
program offers it; otherwise it uses the program's first ordinary mode, such
as Ask. A locked or disallowed mode blocks the plan with a reason.

## Choose history

**Carry** takes the conversation by the most native route the destination
supports: the same session, a native fork, a transplant or an import. Where
native import is unavailable, recent turns go into the handoff word for word.
Choose all turns, through one turn, or a range in the History control.

The place's carry share keeps room in the new model's window for work; it
defaults to 80 percent. The plan shows the carried amount and fidelity, plus
warnings or dropped content. Orch does not require a paid model call to carry
history.

**Summary** uses the place's configured summarizer when available and successful.
Otherwise the last three turns go briefly. Carry and Summary also keep the
full transcript and its turn index in `.orch/handoff/` for the agent to read.
**None** gives the new session no source conversation. Choose it deliberately.
The place's `thread.switch_history` supplies the default and can be locked.

## Review files and effects

History and files are separate choices. Review the directory, workspace, home,
private changes, mounts and MCP servers. A requested transfer that cannot be
done blocks the plan with a reason. Read
[Choose and check mount copies](check-mount-copies.md) before choosing a mount
fate, and [Follow progress](follow-live-work.md#read-progress) during the job.

Confirm the running work that waits or stops, required permissions and any
retained old data. The handoff notice remains visible in work detail. A later
switch back is another switch; it does not silently restore only the turns
since the previous switch.
