---
group: settings
title: Settings and consumption
screens: /thread/$threadId, /usage, /settings, /settings/profile, /settings/your-page, /settings/security, /settings/tokens, /settings/exports, /settings/interface, /settings/notifications, /settings/usage, /resources/providers/$id, /resources/environments/$id, /project/$projectId/settings, /org/$slug, /org/$slug/settings/$section, /admin, /admin/$section
keywords: inherited settings, locks, defaults, speed, effort, context, usage, credits, limits, tokens, notifications, variables, sub-agents, nesting, agents.subagents_at_once, agents.subagent_depth
summary: Read inherited settings and locks, manage your own choices, and see what was used and what limits it.
---

# Settings and consumption

A setting belongs to the place or resource it controls. Your screen layout and your notification routes belong to you. Usage records what ran and what its provider reported. Orch measures consumption; it does not bill your team.

## Inherited settings

The settings page shows the local choice, inherited choice, effective result
and where that result comes from. Values resolve down the place's hierarchy,
through organizations and folders to the topic and thing concerned. A resource
also has its own home and local settings.

A choice list starts with **As Acme decides** (or the source it follows), with
the inherited value beside it. It is selected until this place chooses a value.
For free values and structured editors, choose **Here** to start a local draft
from what is inherited; the inherited editor is read-only. Choosing the inherited
state removes the local contribution. Reset in the row menu does the same thing.
Shared instructions remain read-only above this place's additions, so saving an
addition never repeats the inherited text.

Object and list edits stay in a draft until **Save** or **Discard**. A hidden view,
section or page keeps its draft. Closing a view asks about that view's changes;
saving or discarding it leaves drafts in your other views alone.

Access policies, limits and required protections can only become stricter lower down, with or without a lock. The smallest limit, the highest minimum, the strongest choice and the overlap of the allowed choices take effect.

- A choice of yours that would loosen what a place above sets is still saved. The row shows the value in force and its source, with **The setting above is stricter**, and your choice can apply if that place relaxes.
- Default values are starting choices, not hidden limits.
- Parameters and preferences about behavior are replaced as their description says.

Open the question mark beside a setting for its purpose, an example, its allowed values and what it affects. A choice that is turned off says which permission, policy or capability prevents it. A missing value is never replaced by an invented one.

### Locks

A lock keeps a value for the places below it, where that setting allows locks. The page names the source and shows why a local edit cannot replace it. Managing a place does not override a lock from above. An administrator's confirmed override is recorded, and it still cannot overcome a lock above it or a machine's own policy.

## Model choices

In a place's defaults, the selected provider, agent profile and model determine the
parameter form. **Effort** offers that model's reasoning efforts. **Agent parameters**
uses the same Speed, Access and Context controls as the composer, together with the
program's declared fields. **Request options** opens the fields the provider declares
for the model. These parameter edits stay local until you save the draft. Reset a
field to follow its inherited value; choosing Standard can override an inherited
faster speed. Other saved parameters remain under **Other parameters**.

A model's provider entry says what speeds, access programs and context it may
be used with. A provider manager first enables the offers the provider actually
has. A thread's setting then chooses among them, independently of the model's
maximum. Use the inherited choice to follow the place's default or select a
local override; the page shows the one in force.
The access choice and usage reports use the same readable program names as the
provider's model editor. **None** means no access program; **Standard** is a
program when the provider offers and enables it.

An agent program may keep a parameter behind a feature, as Codex does for its
access program. **Agent features** lists each such feature by the program and the
parameter it opens, with its program-declared shipped state and the source of an override.
Each program declares which features ship on; the shared registry supplies no program-specific entries.
The access program of Codex ships on, so a program that a
manager enabled for a model is offered in a thread; a place that does not want it
offered switches the feature off there, and the others keep it.

Model and reasoning effort apply from the next turn; each parameter states when it applies. Steering a
running turn adds a message and does not change that turn's parameters. Stop,
change and continue when the new setting must take effect sooner. The provider
page may show a plan window or fast-mode cooldown; an unknown native window
stays unknown, and an explicit manager-supplied value keeps its source.

## Sub-agents

Sub-agents use the thread’s provider and environment. Two settings in **Parameters** control how many can work together.

- **How many at once** defaults to 8.
- **How deep they nest** defaults to 1. The main agent can start sub-agents. At depth 1 those sub-agents cannot start their own.

The **Parameters** panel shows each value in force, where it comes from and when the agent program takes a change. A control the program does not support says why it does not apply.

To let the threads of an organization run at most four sub-agents at once, without nesting, set both values in that organization's defaults: four at once and depth 1. The smallest value along the tree wins: a folder may reduce four to two, but a thread cannot raise it to eight. **Reset** returns to the next bound above, or to the default. When a sub-agent is refused, the thread shows it as the agent program reports it.

## Agent history

Orch keeps the conversation. The agent program also keeps its own session files in the home of its provider, so that it can resume exactly where it was. **Agent history kept** removes those files after 7 idle days by default; choose **Off** to stop removing them. The setting follows the defaults of the system, organization, project, topic and thread, and a place above can lock it.

Nothing is removed while there are running turns, waiting or held messages, pending approvals or plans, or program history that a continuation needs. Archived and recently deleted threads count as idle. A personal agent-program home is left to its owner.

The next turn starts a new session with the choice of the place: **Carry**, **Summary** or **None**.

- **Carry** uses the program's own import where it has one. Otherwise it supplies recent turns and the full transcript as a file.
- **Summary** uses the summary you set up, or brief recent turns when there is none, beside the same file.

The thread's history records the removal and how the conversation reached the new session.

## Standing instructions

Standing instructions are inherited settings. They go to the agent with its session and are refreshed for the next turn. The instruction file of the thread's directory is a file of the workspace that the agent program reads, together with the files of its parent directories. It can be empty. Changing either does not grant trust to project configuration, hooks, plugins or MCP servers; that is a separate choice for the environment.

## Variables and secrets

Environment variables belong to the environment and can be inherited from a source it may use. The edit form tells a value of its own from the inherited result. A secret is masked: it is stored and passed on to the work that needs it, and reading the page never shows it. To replace or delete a secret of your own, use the form's own action; a masked placeholder is never sent back as its value. The credential fields of a provider stay with the provider.

## Notifications

In **Settings › Notifications**, **Exceptions** lists the places where you chose another notification level. Change a row there, or choose **Default** to follow the nearest choice above that place. The picker says where that default comes from. **Undo** restores your previous choice. Places you can no longer see are left out, and the list is read a page at a time.

The bell in an org, department, project, folder, topic or thread sets that place's level. Off is
written on the bell itself; its menu marks the selected choice with a check and a filled row.
Default follows the nearest level above the place, excluding its own choice. Chat bells show their
own All, Mentions or Off choice; document bells choose all discussions or those you are involved in.
The open lists explain the levels. Quiet hours use the time zone shown beside their schedule. An unrecognized quiet-hours zone is shown as a problem and keeps outward messages waiting without spending delivery attempts. The Inbox remains available, and a corrected zone is reevaluated automatically.

The Inbox is the record inside Orch. Orch sends outside only through routes you turn on: mail, a bot, a webhook or this device's push. A push route also needs this browser's permission and subscription; another device being ready does not make this one ready. Link a bot account through its code when the route asks for it.

Choose when unread work is sent and how often messages can arrive. **Pause until** delays sending outside and keeps the Inbox record. **By place** groups unread work; **read** and **Undo** act on the group you see, with your current access. A delivery that failed or is uncertain is shown, so you can look at it or retry it. A notification excerpt is not a saved copy of private work for use offline.

An organization or system manager can configure channels and their templates in
**Settings → Email and channels** or **Management → Email and channels**. Browser
push is available by default when the installation has its push key; you still
choose whether to enable it for each browser.
A project's destination receives only the work its rules permit; private
descendants and human chat do not automatically flow outward. Weekly quiet
schedules and offline content caching are not offered in this release.

## Usage

The **Usage** of a thread shows the tokens and credits attributed to its work, and the room the provider has left where it reports it. Your own usage, the totals of a place and the detailed reports each have their own access. A thread you cannot see does not show its title, turn or text in a usage report.

Providers report different things:

- OpenRouter reports a key's limit and its budgets with the time each starts over, like the windows of a subscription, the requests of the day for free models, and the account's credits for a management key. Another upstream may report no balance. The page keeps that unknown.
- Where a provider reports no direct cost, a rate card or the catalogue estimates the credits, and the source of every figure stays visible. If the catalogue cannot be refreshed, Orch keeps the last good one; it does not estimate every model at zero.

The details of a model request keep the total the provider reported, in its own unit, beside the credits Orch counted. A price that a manager of the provider entered, or the rate card, stays in force. Where Orch would use a catalogue estimate, a reported total in the same unit replaces that estimate, using the place's catalogue scale. Search fees already included in that total are not added again. A unit Orch does not know keeps the original pricing, and the reported amount stays available for comparison.

### Plan windows and forecasts

On a provider's **Usage** tab, **Plan** sits below the figures. **This window** shows measured usage as a solid line and its forecast as a dashed line to the reset or to the limit.

- Short windows use the pace of the last hour; windows of a day or more use the last day.
- The pace never reaches back before the window began or before its latest early reset.
- A forecast needs 15 minutes of readings for a short window and 6 hours for a long one, and it goes away when the readings are more than two hours old.

**History** shows the usage added in each **Interval**, hourly when you choose it. A restart never subtracts usage. Long windows show an **Even pace** line, and warning bars mark the intervals that reached the limit. A reset marker opens its audit entry and says whether the reset was spent through Orch, spent elsewhere, or started early by the provider. It records no notification. Several resets in one interval share one counted marker, and its link opens the latest entry. **Next windows** and **Previous windows** page through providers with many windows. The summary counts the whole span you chose.

Each limit with a period shows a forecast from its average since the period began. Limits on the total, and on turns at the same time, have none. A forecast describes the pace so far; later work can change it.

### Requests

The **Turns** figure also gives the completed requests whose outcome is known. Choose **Requests** under **Over time** to chart them. A turn can hold several requests. The **Requests** tab lists the single calls and what was measured for each.

### Kinds of token

Choose **Tokens**, then **Kind of token**, to see fresh input, cache read, cache write, output and reasoning. Cache reads and writes are parts of the input, and reasoning is a part of the output. Each token is counted once. **Cache hit** is the share of the input that was read from the cache.

### What fills the context, and what a turn's tokens were

Beside the model in a thread's header, a ring shows how much of the model's context window the last request of the agent filled: green below 60 %, amber up to 85 %, red above, with a tick where the agent program shortens the conversation. Its label reads like "Context 142k of 200k (71 %); shortens at 90 %". The ring is there only where the agent program reports the size of its requests.

Click the ring to see what fills the context: a bar of its parts, each with its tokens and share, then the free space and the room the program keeps for shortening, the same numbers as a table, and when the conversation was last shortened and how much that freed. Each part says how it was counted:

- **Counted by the agent program** or **estimated by the agent program**: the program reports the parts, and the panel names the source of each figure.
- **Estimated by Orch**: the program reports a total but not its parts, so Orch estimates the instructions, tools, conversation, tool calls, files and images that it sent or saw. The model's declared estimate says how text becomes tokens. The rest belongs to the program's own context. If the estimates add up to more than the reported total, the panel says they were scaled down. A part nobody saw stays unknown.

A turn's footer carries a thin bar of what its tokens were: fresh input, input read from the cache, input written to the cache, output and its reasoning, and the share of the sub-agents. The numbers and the cache hit rate show under the pointer. Click it to open the turn's details: by model, by sub-agent, what the program's own reviewer used where the program reports it apart, what the turn was counted at where the rates are known, and how full the context was at its end.

The thread's **Usage** view shows the same over all its turns: the fill of the context as a line with the shortenings marked, the cache hit rate, the share of the sub-agents, a table of every turn and an export as CSV. Where an agent program reports nothing, the ring and the bars are absent and the view says what it does not report.

### Built-in programs

- **Codex** reports the size of the request. Orch estimates the parts it can see and labels the rest as the program's own context.
- **Claude Code** reports a breakdown of the context. Its estimate after a turn, and a count asked for between turns, are labeled by their source.
- **DeepSeek Harness** shows the figures the program reports and marks Orch's estimates. A figure that can be neither reported nor estimated stays unavailable.

## Limits

A limit bounds tokens, credits or the turns running at the same time, over its place and its period. Read what it covers, how much is used and when it resets. A narrower place still obeys the broader limits. Being an administrator does not exempt a turn.

When a limit holds work, the thread names the limit or the provider window that controls, and what can happen next. A held input stays in the queue.

Whoever holds the permission to make an exception to a limit can let one turn through after reading the current use. That act is by a named person and is counted and recorded. An agent or an automatic input cannot give itself an exception. A reset by the provider is a separate act: its managers ask for it after the confirmation names the reset that will be used. An automation key needs that reset operation selected for the provider, and the same confirmation.

## Tokens and security

Your security page manages your sign-ins and your automation keys. Each key is restricted by rules on operation, action and target, together with your current permissions. Ordinary administration needs the operation chosen on purpose and your current standing. The private-access override of the browser is not available to keys.

- **Refuse here** blocks a person at the place you are in.
- **Take out everywhere here** has the wider scope that the confirmation states.
- A suspension ends access at its scope and can be undone. It does not hand over ownership by itself.

If someone loses an authenticator or a way to sign in, the administrator of the installation can give them a one-use reset link, with the procedure in the operator guide. The tokens and renewal of a provider are managed on the provider's page, apart from your Orch account. A managed provider can be started for renewal before its sign-in lapses, five days ahead by default where the setting says five.

## Organization management

Managers of an organization control its members, roles, groups, invitations, tags and defaults, within their authority. A department has the same controls as any other organization. Moving a member changes their home and their role there, and previews the changes of access; it does not move the things they own. To create a team organization you need the invitation that the installation's policy shows.

Use **What they did here** on a person to look into their recorded work and the agents that act for them. Where your authority allows it, you can look at retained deleted content without restoring it. The audit trail does not record every read, every file content, every keystroke in a terminal or the whole content of a page.

## Layout

Layout keeps your view choices, the positions of your panes and the preferences of your screens. The order of the tree follows the organization's default until you choose your own. Which branches are folded, your recent places and your last place are personal. Your theme and your colors do not change what another person sees. A color picker offers its listed palette or a custom color as shown.

## Exports and administration

**Exports**, in your personal settings, lists the jobs and downloads your account may read. Open a job for its progress and its outcome. A job that is queued or failed has no download. **Profile** and **Your page** control what you say about yourself in public, while **Security** and **Tokens** control access to your account. See [Manage your account](account-and-profile.md).

**Management** holds the settings of the system and the inventories that you may see. Where it is required, turn on administrator access with the **As admin** control first. Each section still checks your permission. The home of a resource, its owners, the settings of a place and the administration of the system are separate authorities.

In a provider's **Use** entries, **Models** and **Services** are separate lists. Each takes exact names, family patterns and the provider's groups (written with @). A **Services** list left out includes every service that is turned on, even when **Models** is limited. A service must also pass every services policy above its thread. Combining **Use** entries is permissive, and policies of places are restrictive. A service in the list still needs to be turned on by a manager of the provider before it can run.

## System usage

System administrators can choose **System** in the scope picker of **Usage**. If administrator access is off, the screen keeps your choice and offers the switch to turn it on; it does not replace the report with your own usage. **Usage**, **Performance** and **Requests** keep the same span and filters at the system scope, including the live feed of requests and the exports. Leaving administrator mode removes the system view. The address keeps the system scope, so a reload opens the same scope under your current checks.
