---
title: Settings and consumption
screens: /thread/$threadId, /usage, /settings, /settings/profile, /settings/your-page, /settings/security, /settings/tokens, /settings/exports, /settings/interface, /settings/notifications, /settings/usage, /resources/providers/$id, /resources/environments/$id, /project/$projectId/settings, /org/$slug, /org/$slug/settings/$section, /admin, /admin/$section
keywords: inherited settings, locks, defaults, speed, effort, context, usage, credits, limits, tokens, notifications, variables, sub-agents, nesting, agents.subagents_at_once, agents.subagent_depth
---

# Settings and consumption

Settings belong to the place or resource they control. Your screen layout and
notification routes belong to you. Usage records what ran and what its provider
reported; Orch measures consumption and does not bill your team.

## Inherited settings

The settings page shows the local choice, inherited choice, effective result
and where that result comes from. Values resolve down the place's hierarchy,
through organizations and folders to the topic and thing concerned. A resource
also has its own home and local settings. Reset removes your local override,
so it follows the inherited value again.

Open the question mark beside a key for its purpose, example, allowed values
and effects. A descriptor supplies help for an extension's options. A disabled
choice explains the permission, policy or capability that prevents it; a
missing value is not silently replaced with an invented fact.

### Locks

A lock holds a value for the places below it where that setting permits locks.
The page names the source and shows why a local edit cannot replace it. Managing
one place does not override a lock from above. An administrator's confirmed
override is recorded and still cannot overcome a higher lock or local machine
policy.

## Model choices

A model's provider entry says what speeds, access programs and context it may
be used with. A provider manager first enables the offers the provider actually
has. A thread's setting then chooses among them, independently of the model's
maximum. Use the inherited choice to follow the place's default or select a
local override; the page shows the one in force.

Model, reasoning effort and parameters apply from the next turn. Steering a
running turn adds a message and does not change that turn's parameters. Stop,
change and continue when the new setting must take effect sooner. The provider
page may show a plan window or fast-mode cooldown; an unknown native window
stays unknown, and an explicit manager-supplied value keeps its source.

## Sub-agents

The harness runs its sub-agents inside the thread's provider use and environment
limits. `agents.subagents_at_once` accepts 1 to 64 and ships as **8**;
`agents.subagent_depth` accepts 1 to 5 and ships as **1**. Depth 1 lets the main
agent start sub-agents, while those sub-agents start none of their own. The
Parameters panel shows each effective value, where it comes from and when the
harness takes a change; an unsupported control explains why it does not apply.

For an org whose threads should run at most four sub-agents at once without
nesting, set `agents.subagents_at_once = 4` and `agents.subagent_depth = 1` in
that org's defaults. The smallest value set along a thread's hierarchy wins.
A folder may reduce four to two; setting eight in a thread cannot relax the
org's four. Reset returns to the remaining inherited bound or the shipped
value. A refused spawn is shown as the harness reports it in the thread.

## Agent history

Orch keeps the conversation. The agent program also keeps native session files
in its provider's home so it can resume exactly. **Agent history kept**
(`retention.agent_history_idle_days`) removes that cache after **7 idle days**
by default. Choose **Off** (`0`) to stop automatic removal.
The setting follows system, organization, project, topic and thread defaults;
an ancestor may lock it.

Running turns, waiting or held messages, pending approvals or plans, and native
history needed by a continuation prevent removal. Archived and recently deleted
threads still count as idle. Personal agent-program homes are left to their owner.

The next turn starts a new session using the place's Carry, Summary or None
choice. Carry uses the program's native import where available; otherwise it
supplies recent turns and the full transcript file. Summary uses the configured
summary, or brief recent turns when it is unavailable, beside the same file.
The timeline records removal and how the conversation reached the new session.

## Standing instructions

Standing instructions are inherited settings supplied to the agent with its
session and refreshed for the next turn. The directory's instruction file is
a workspace file read by the harness, including its parent-file inheritance.
It can be empty. Changing either does not grant trust to project configuration,
hooks, plugins or MCP servers; that is a separate environment choice.

## Variables and secrets

Environment variables belong to the environment and can be inherited from its
permitted source. The edit form distinguishes a local value from the inherited
result. A masked secret is stored and passed for the work that needs it; reading
the page does not reveal it. Replacing or deleting a local secret follows the
form's explicit action, rather than resubmitting a masked placeholder as its
value. Provider credential fields remain with the provider's runtime.

## Notifications

In **Settings › Notifications**, **Exceptions** lists the places where you chose a different notification
level. Change a row here, or choose **Default** to follow the nearest choice
above that place. The picker says where that default comes from. Undo restores
your previous choice. Places you can no longer view are left out, and the list
is read a page at a time.

The bell in an org, department, project, folder, topic or thread sets that place's level. Off is
written on the bell itself; its menu marks the selected choice with a check and a filled row.
Default follows the nearest level above the place, excluding its own choice. Chat bells show their
own All, Mentions or Off choice; document bells choose all discussions or those you are involved in.
The open lists explain the levels. Quiet hours use the time zone shown beside their schedule.

Inbox is the record inside Orch. Outside Orch sends only through routes you
switch on: mail, a bot, webhook or this device's push, as offered. A push route
also needs this browser's permission and subscription; another device's ready
state does not make this device ready. Link a bot account through its code when
the route asks for it.

Choose when unread work should be sent and how often messages can arrive.
Pause until delays outward delivery and keeps the Inbox record. By place groups
unread work; read and Undo act on the shown group with its current access.
A failed or uncertain external delivery is shown so you can inspect or retry
it. A notification excerpt is not a cached copy of private work for offline use.
The Inbox keeps a notification for 90 days, read or not, unless a system admin
chose another number of days ("Inbox kept"); a delivery that failed stays a
month for you to retry, and one that went stays a week.

An organization or system manager can configure channels and their templates.
A project's destination receives only the work its rules permit; private
descendants and human chat do not automatically flow outward. Weekly quiet
schedules and offline content caching are not offered in this release.

## Usage

The thread's Usage shows tokens and credits attributed to its work, and the
provider's available room where reported. Your own usage, a place's totals and
its detailed reports have separate access. A hidden thread does not disclose
its title, turn id or text in a usage report.

Providers report different facts. OpenRouter reports a key's limit and its
budgets with the time each starts over, like a subscription's windows, the free
models' requests of the day, and the account's credits for a management key;
another upstream may provide no balance. The page keeps that unknown. Rate
cards and a catalogue estimate credits where direct cost is absent, and the
source of a figure stays visible. A failed catalogue refresh keeps the last
valid catalogue rather than estimating every model at zero.

### Plan windows and forecasts

A provider's Usage tab puts **Plan** below the figures. **This window** shows
measured usage as a solid line and its forecast as a dashed continuation to the
reset or the limit. Short windows use the last hour's pace; windows of a day or
longer use the last day. The pace never reaches before the window began or its
latest early reset. Forecasts need 15 minutes or 6 hours of readings, respectively,
and disappear when readings are more than two hours old.

**History** shows usage added in each **Interval** bucket, including hourly bars when
chosen. Starting over never subtracts usage. Long windows show an **Even pace**
line; warning bars mark buckets that reached the limit. Reset markers open the
audit entry and say whether a reset was spent through Orch, elsewhere, or the
provider started the window over early. This records no notification.
Several resets in one bucket share a counted marker; its link opens the latest
entry. **Next windows** and **Previous windows** page through providers with
many windows. The history summary counts the whole selected span.

Each cap with a period shows a forecast from its average since that period
began. Total and concurrent-turn limits have none. A forecast describes the
observed pace; future work can change it.

### Requests

The Turns figure also gives completed requests with a known outcome. Choose
**Requests** in **Over time** to chart them. A turn can contain several requests.
The Requests tab lists the individual calls and their measured results.

### Kinds of token

Choose **Tokens**, then **Kind of token** to see fresh input, cache read,
cache write, output and reasoning. Cache reads and writes are parts of input;
reasoning is part of output. Each token is counted once. **Cache hit** is the
share of input read from cache.

### What fills the context, and what a turn's tokens were

Beside the model in a thread's header, a ring shows how much of the model's
context window the agent's last request filled: green below 60 %, amber up to
85 %, red above, with a tick where the harness shortens the conversation. Its
label reads like "Context 142k of 200k (71 %); shortens at 90 %". The ring
is there only where the harness reports the size of its requests.

One click on the ring opens what fills the context: a bar of its parts, each
with its tokens and share, then the free space and the room the harness keeps
for shortening, with the same numbers written as a table, and when the
conversation was last shortened and how much that freed. Each part says how it
was counted:

- **Counted by the agent program** or **estimated by the agent program**:
  the program reports the parts. The panel names the source of each figure.
- **Estimated by Orch**: when the program reports a total but not its parts,
  Orch estimates the instructions, tools, conversation, tool calls, files and
  images it sent or observed. The model's declared estimate describes how text
  is converted to tokens. The remainder belongs to the program's own context.
  When the estimates exceed the reported total, the panel says they were scaled
  down. A missing observation remains unknown.

A turn's footer carries a thin bar of what its tokens were: fresh input, input
read from the cache, input written to the cache, output and its reasoning, and
the sub-agents' share, with the numbers and the cache hit rate under the pointer.
A click opens the turn's details: by model, by sub-agent, what the harness's
permission reviewer used where the harness reports it apart, what the turn was
counted at where its rates are known, and how full the context was at its end.
The thread's Usage view shows the same over all its turns, with the context's
fill as a line, its shortenings marked, the cache hit rate, the sub-agents'
share, a table of every turn and an export as CSV. Where a harness reports
nothing, the ring and the bars are absent and the view says what it does not
report.

### Built-in programs

#### Codex

Codex reports the request size. Orch estimates the parts it can observe and
labels the remaining program context.

#### Claude Code

Claude Code reports a context breakdown. Its estimate after a turn and a
count requested between turns are labelled by their source.

#### DeepSeek Harness

The context view uses the figures the program reports and identifies Orch's
estimates. A figure that cannot be reported or estimated stays unavailable.

## Limits

A cap bounds tokens, credits or concurrently running turns over its scope and
period. Read its coverage, used amount and reset; narrower scopes still obey
broader caps. Being an administrator does not exempt a turn. When a limit holds
work, the thread names the controlling cap or provider window and what can happen
next; a held input remains in the queue.

Whoever holds permission to make a limit exception may confirm one turn after reading its
current use. That named human act is counted and recorded. An agent or an
automatic input cannot give itself a limit exception. A provider reset is a separate
vendor action: only its managers can request it, in a browser session, after
the confirmation names the available reset being consumed.

## Tokens and security

Your security page manages sign-in and API tokens. Token scopes and place
boundaries restrict requests; a token does not carry browser-only system-admin
authority. Refuse here blocks it at the current place. Take out everywhere here
has the broader scope stated by the confirmation. Suspensions end access at
their scope and are reversible; they do not transfer ownership by themselves.

If someone loses an authenticator or sign-in method, the installation's
administrator can give them a one-use reset link using the operator procedure.
Provider tokens and renewal are managed on the provider page, independently of
your Orch account. A managed runtime can be started for renewal before the
known lapse, five days ahead by default where that setting resolves to five.

## Organization management

Organization managers control members, roles, groups, invitations, tags and
defaults within their authority. A department has the same controls as another
organization. Moving a member changes their domain and role and previews the
access changes; it does not move the things they own. Creating a team organization
requires the invitation shown by the installation's policy.

Use a person's What they did here to investigate their recorded work and the
agents acting for them. Inspect retained deleted content without restoring it
when your authority permits it. Ordinary reads, file contents, shell typing
and entire page contents are not a record of every action in the audit trail.

## Layout

Layout keeps your view choices, pane positions and screen preferences. Tree
order follows the organization's default until you choose your own. Branch
folding, recent places and your last place are personal state. Your theme and
color choices do not change another person's view, and a picker offers its
listed palette or a custom color as shown.

## Exports and administration

Personal Exports lists the jobs and downloads your account may read. Open a
job for its progress and outcome; a queued or failed job has no ready download.
Profile and Your page control your public words, while Security and Tokens
control account access. Read [Manage your account](account-and-profile.md).

Management holds system settings and the permitted system inventories.
Administrator access must be enabled through the existing As admin control
where required. A section's permission checks still apply. A resource's home,
its owners, a place's settings and system administration are separate authority.
