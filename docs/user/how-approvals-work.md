---
group: access
title: How approvals work
screens: /thread/$threadId, /approvals, /resources/environments/$id, /topic/$topicId, /project/$projectId
keywords: how do I approve, approvals, permission, request, approval.mode, approval.allowed_modes, requests.expiry_hours, ask, on request, auto, bypass, reviewer, capability.policy, rules
summary: Read a request’s effects and authority before approving work or changing a boundary.
---

# How approvals work

A request says what would happen, who can decide, and what each choice permits.
Read those consequences before answering. Two kinds of authority are involved.

## Work inside the environment

An agent program asking to run a command or change a file needs a person who may
manage the thread. Its mode applies to work inside the environment's existing
boundary. **On request** asks when the agent program needs to or its file access
rules block an action. **Ask** asks before actions the agent program does not already trust.

A new thread uses its place's approval mode when the selected program offers it.
Otherwise it starts with the program's default, such as **Ask** for DeepSeek Harness.
The place's allowed modes and the environment's boundaries still apply.

**Auto** uses the agent program's own reviewer. The selected build must pass its
Auto approval check on this machine. If the check is missing or has failed, the
approval picker shows the reason. A system administrator can choose **Check** there
when the build has a check available. The thread shows the model and outcome of
each reported review. Auto is never the default.

Claude Code uses its native Auto permission mode. Its check verifies that the
classifier lets an allowed action run, stops a denied action, and leaves the action
undone if the reviewer is unavailable. Your provider access and the place's
approval policy still apply after the check passes.

For a forwarded session, Codex reviews with the thread's model. A native sign-in
keeps its declared preferred reviewer. The thread shows the selected model and
marks a name supplied by Codex as reported. The reviewer can use provider quota.
Separately reported review usage belongs to the thread. Otherwise the totals are
those reported by the agent program. An action that needs no approval runs without
an extra review.

**Bypass** lets the agent do anything in its environment without asking; whoever chooses it answers for it.
It requires another boundary that confines the agent and a program running without
root privileges. Questions and proposed plans still need your response.

For example, an On request thread may run its ordinary tests and then ask to
write a protected file. Allow once answers that request. A question asking
“Which branch should I use?” still needs your answer in every mode.

## Changes to the boundary

A KVM, port or access request needs the authority of the thing it would open.
Managing the thread does not let you grant KVM on someone else's host, open a
port or give another person's provider. The card shows whether you can decide it now.
Bypass, Auto, a thread rule and Stop do not decide these requests.

A capability's policy can allow, ask or deny. Under allow, the outcome records
the policy that permitted it. For a port, **Allow once** permits this opening;
**Grant to this environment** gives its matching port capability for later
openings too; **Deny** refuses it. A link's review says who receives its secret.
See [Let agents open ports without asking](let-agents-open-ports.md).

## Who can decide

| Request | Who may decide |
|---|---|
| Agent command, edit or permission | Thread managers |
| Proposed plan | People who may drive the thread, or managers where the place keeps plans to them |
| Held message, Run it | People who may drive the thread and use its current model on their own access |
| Capability | Environment port managers, host managers, org access managers or system admins, as its scope says |
| Port | The environment's port managers |
| Access | People who can give the requested permission on that thing |
| Group membership | People who may add that person to the group |

For a request raised in a turn, its eligible starter is told first. Otherwise
only eligible people directly named in the thing's Owners row receive an Inbox
item and appear in **Waiting for you**. When nobody there may decide, Orch walks
up the places' Owners rows, then the org's admins, then all people who can decide
now. Groups, roles and inherited authority do not fan notices out to everybody.
Other eligible people find the card and **Others you may decide**, without a
notice, unread mark or pending count.

## Follow and decide a card

Open the request in the thread or [Approvals](/approvals). Confirm its current
target, audience and scope. A colleague's earlier decision wins; your late
click cannot overwrite it. A lost connection disables a decision until the
page catches up. Denial can include a note, and the outcome stays readable.
One decision settles every recipient copy and marks its Inbox item read.
It leaves all pending lists and counts, with who decided, the outcome and time
kept in history. This also applies to **Proceed**, **Keep planning** and **Run it**.
A second decision returns the first result and makes no second change.
If the list cannot load a decision made elsewhere, it keeps a retry for that
outcome without showing the old request details.

A proposed plan offers **Proceed** or **Keep planning**. Proceed lets the agent
continue under the composer's approval mode. Keep planning sends your words
back so the agent can revise its plan. The Plan switch changes after the answer
is confirmed. If an answer is refused, its reason appears beside the choices;
reload the request before trying an option that is no longer offered.

Agent action requests last while the program waits. Agent boundary requests use
`requests.expiry_hours`, initially 24 hours; the shortest
setting above the place wins. A link may lapse sooner. A person's access
request initially lasts 14 days. Repeating the same denied agent boundary
request within an hour returns its first denial and note rather than another
card; changed content makes a new request.

## Allow one routine action again

When an agent program offers **Allow for this thread**, inspect its exact command or
host matcher. It answers matching requests in this thread's current binding;
it is not general access to another environment or a changed shared login.
Parameters lists these rules and **Take back** removes one. Questions and
forms still wait. A fork or side thread does not inherit the rule.

To understand an admin's separate **override**, see
[Who can see and do what](who-can-see-and-do-what.md).
