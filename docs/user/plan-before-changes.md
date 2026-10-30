---
title: Plan and goals
screens: /thread/$threadId, /approvals, /org/$slug/settings/$section, /project/$projectId/settings, /admin/$section
keywords: Plan, Proceed, Keep planning, plan mode, approval, read only, thread.plan_first, thread.plan_answered_by, goal, /goal, Done when, keep working, thread.goal.max_turns, thread.goal.max_minutes, thread.goal.max_credits, summary.handoff_input_share
---

# Plan and goals

## Plan a turn

Turn on **Plan** beside the composer's approval mode, then send the work.
The agent reads and proposes a plan before making changes. The control is
off by default; a place can choose and lock `thread.plan_first`. It is offered
only by agent programs that support planning.

Plan and approval mode are independent. Planning does not select Ask, Auto or
Bypass. Proceed continues under the approval mode already shown. Read
[How approvals work](how-approvals-work.md) for that mode's boundary.

## Decide the plan

Read the plan card. **Proceed** lets the agent implement it under the approval
mode shown in the composer. The agent program determines how it accepts the
decision. Orch records who answered.

**Keep planning** sends your words back for a revised plan. It does not approve
the implementation. A plan checklist can also update while ordinary work runs;
read the card's state before treating a checklist as an unanswered proposal.

## Who can answer

By default, someone who may drive the thread can answer. A place can restrict
answers to its owner and managers. A view-only reader can read the plan but
cannot proceed. Current access and model use still apply; planning grants no
permission to spend another person's provider or change an environment.

## Built-in programs

### Codex

Proceed adds the visible input **Implement the plan.** under the person who
answered.

### Claude Code

Proceed answers the pending plan request and restores the thread's approval
mode for implementation.

### DeepSeek Harness

Planning is offered when the program and its checked build declare support.
The plan card and the approval mode remain separate.
## Goals

A goal keeps an agent working on one piece of work until it is done. Orch runs
it, the same for every agent program; the program's own goal mode stays off,
because it would start turns Orch did not check.

## Set a goal

Type `/goal` and what must be true when the work is done, such as
`/goal npm test exits 0`, or choose **Set a goal…** in the thread's ⋯ menu to
write the **Objective** and **Done when** apart. **Limits** (turns, minutes and
credits) are none by default; a place may set them and lock them. **Notify me
when it ends** stays off unless you tick it.

If a goal is working, paused or stopped, `/goal` opens its saved fields. The
thread's ⋯ menu offers **Edit goal…**. Reloading keeps its line and actions;
raise any reached limit before choosing **Resume**, or use **Clear goal**.

## How the goal goes on

After each turn Orch counts what it used and starts the next one in your name,
so it passes every check a message of yours passes: usage caps, held queues,
approvals and recovery. It waits while a question, an approval or a plan asks
somebody, while the queue is held and while the thread is blocked. The line
above the composer says **Working**, **Waiting for you**, **Paused**, **Met** or
**Stopped**, with the turns, the time and the tokens used.

The conversation outline names each goal turn by its **Done when** condition,
so you can return to that part of the work after reloading.

## Met, and the evidence

The agent reports with its goal tool: working, met (with its evidence) or
blocked. Where the place has a summary model, it checks the evidence against
**Done when**; a "not yet" starts the next turn with its reason. Where there is
none, the agent's report decides, and the conversation says so. A blocked agent
pauses the goal and says what it needs.

## Limits and stopping

A limit that is set stops the goal and says which one. Raise it in **Edit
goal…**, then **Resume**. **Pause** holds the goal, stopping a running turn
pauses it, and taking its queued turn out of the queue pauses it. **Clear goal**
removes it; the conversation keeps what it did.

A program that cannot take Orch's tools has no way to report on a goal, so its goal goes
on until a limit is reached or you pause it; the line's chip says so on hover.

## Who can set a goal

Whoever may drive the thread sets, pauses, edits and clears its goal. Setting
one also needs your own use of the thread's model, since each turn runs on your
access. Each act is named in the conversation and in the audit trail.

## Plan and goal

With **Plan** on, the first turn of the goal plans; **Proceed** starts the work.
