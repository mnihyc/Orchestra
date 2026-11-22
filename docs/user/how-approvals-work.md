---
group: access
title: How approvals work
screens: /thread/$threadId, /approvals, /resources/environments/$id, /topic/$topicId, /project/$projectId
keywords: how do I approve, approvals, permission, request, approval.mode, approval.allowed_modes, requests.expiry_minutes, ask, on request, auto, bypass, reviewer, capability.policy, rules
summary: Read a request’s effects and authority before approving work or changing a boundary.
---

# How approvals work

A request says what would happen, who can decide it and what each choice allows. Read those consequences before you answer. Two kinds of permission are involved: the one for work inside the environment, and the one to change its boundary.

## Work inside the environment

When an agent program asks to run a command or change a file, a person who may manage the thread decides. The approval mode of the thread applies to work inside the existing boundary of the environment. There are four modes:

- **Ask** asks before actions that the agent program does not already trust.
- **On request** asks when the agent program needs to, or when its rules for file access block an action.
- **Auto** uses the reviewer of the agent program itself. An action that is allowed runs, and one that is denied does not. The review shows its model and its outcome in the thread. Auto is never the default.
- **Bypass** lets the agent do anything in its environment without asking, and whoever chooses it answers for it. It needs another boundary that confines the agent, and a program that runs without root privileges. Questions and proposed plans still need your answer.

A new thread uses the mode of its place when the chosen program offers it. Otherwise it starts with the default of the program, such as **Ask** for DeepSeek Harness. The modes the place allows, and the boundaries of the environment, still apply.

To offer **Auto**, the selected build must pass its Auto approval check on this machine. If the check is missing or has failed, the
approval picker shows the reason. A system administrator can choose **Check** there
when the build has a check available. While a check runs, the picker shows the build host it runs on and how long it
has run; when it ends without passing, it shows why.

Claude Code uses its native Auto permission mode. Its check verifies that the
classifier lets an allowed action run, stops a denied action, and leaves the action
undone if the reviewer is unavailable. Your provider access and the place's
approval policy still apply after the check passes.

For a forwarded session, Codex reviews with the thread's model. A native sign-in
keeps its declared preferred reviewer. The thread shows the selected model and
marks a name supplied by Codex as reported. The reviewer can use provider quota.
Separately reported review usage belongs to the thread. Otherwise the totals are
those reported by the agent program. An action that needs no approval runs without
an extra review. Provider access must allow the reviewer’s model.

For example, a thread in **On request** may run its usual tests, then ask to write a protected file. **Allow once** answers that request. A question such as "Which branch should I use?" still needs your answer in every mode.

## Changes to the boundary

A request for KVM, a port or access needs the permission over the thing it would open. Managing the thread does not let you grant KVM on the host of someone else, open a port or give the provider of another person. The card shows whether you can decide it now. **Bypass**, **Auto**, a rule of a thread and **Stop** do not decide these requests.

The policy of a capability can allow, ask or deny. Under allow, the outcome records the policy that permitted it. For a port, **Allow once** permits this opening, **Grant to this environment** gives its matching port capability for later openings too, and **Deny** refuses it. The review of a link says who receives its secret. See [Let agents open ports without asking](let-agents-open-ports.md).

## Who can decide

| Request | Who may decide |
|---|---|
| Command, edit or permission of an agent | Managers of the thread |
| A proposed plan | People who may send to the thread, or managers where the place keeps plans to them |
| A held message, **Run it** | People who may send to the thread and use its current model with their own access |
| Capability | Port managers of the environment, managers of the host, managers of access of the organization or system administrators, as its scope says |
| Port | The port managers of the environment |
| Access | People who can give the permission asked for on that thing |
| Membership of a group | People who may add that person to the group |

For a request raised in a turn, the person who started it is told first, if they are eligible. Otherwise only the eligible people named directly in the Owners row of the thing get an Inbox item and see it under **Waiting for you**. When nobody there may decide, Orch goes up the Owners rows of the places above, then to the administrators of the organization, then to everyone who can decide now. Groups, roles and permissions inherited do not send notices to everybody. Other people who are eligible find the card, and **Others you may decide**, without a notice, an unread mark or a count of pending requests.

## Follow and decide a card

Open the request in the thread or in [Approvals](/approvals). Confirm its current target, its audience and its scope.

- A decision of a colleague made earlier wins. Your later click cannot overwrite it.
- A lost connection turns decisions off until the page catches up.
- A denial can include a note, and the outcome stays readable.
- One decision settles every copy of the request for its recipients and marks its Inbox item read. It leaves all lists and counts of pending requests, and who decided, the outcome and the time stay in the history. This also holds for **Proceed**, **Keep planning** and **Run it**.
- A second decision returns the first result and changes nothing a second time. If the list cannot load a decision made elsewhere, it keeps a retry for that outcome without showing the details of the old request.

Agent action requests last while the program waits. Agent boundary requests use
`requests.expiry_minutes`, initially 1,440 minutes (one day) with no fixed upper cap; the shortest
setting above the place wins. A link may lapse sooner. A person's access
request initially lasts 14 days. Repeating the same denied agent boundary
request within an hour returns its first denial and note rather than another
card; changed content makes a new request.

## Allow one routine action again

When an agent program offers **Allow for this thread**, read its exact command or the host it matches. It answers matching requests in the current binding of this thread. It is not general access to another environment or to a shared sign-in that changed. **Parameters** lists these rules, and **Take back** removes one. Questions and forms still wait. A fork or a side thread does not inherit the rule.

To understand the separate **override** of an administrator, see [Who can see and do what](who-can-see-and-do-what.md).
