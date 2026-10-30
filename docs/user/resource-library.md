---
title: Resource library
screens: /resources/providers, /resources/agents, /resources/skills, /resources/mcp, /resources/templates, /resources/environments, /resources/mounts, /resources/hosts, /resources/machines, /everything, /project/$projectId, /topic/$topicId, /documents/$nodeId, /inbox, /approvals, /search, /usage, /trace/$nodeId, /resources/providers/$id, /resources/agents/$id, /resources/skills/$id, /resources/mcp/$id, /resources/templates/$id, /resources/environments/$id, /resources/mounts/$id, /resources/hosts/$id, /thread/$threadId, /doc/$documentId
keywords: library, resources, given, used, popular, use, mine, manage, view, sort, filters, previous state, device
---

# Resource library

## Choose a view

The **Show** menu comes before search. **Mine** shows what you own.
**I manage** includes Mine. **I can use** includes both of those.
**I can view** includes everything you may see. Environments have View and
Manage, so their menu leaves out I can use. An org's admins also have
**All in** that org, after the ordinary views.

A view changes which resources are listed. Using one still checks its current
permission, state and limits. A disabled resource can remain in a list you manage.
When a view has no rows, its button opens the next wider view.

## Read Given and Used

**Given** describes current relationships. Providers, profiles and environments
show bound threads. Skills show explicit places and delivered sessions.
MCP servers show explicit places and environments. Templates show existing
environments or provider runtimes. Mounts show attached environments; hosts show
their units. Open a Given figure to see the names you may read and **and N more**
for the others. Inherited availability is not counted as another explicit place.

**Used** counts actual reported work: turns for providers, profiles and environments;
tool calls for MCP servers; reported invocations for skills; and logical environments
made from a template or on a host. Provider templates count providers instead.
Availability and attachment do not count as use. A recreated environment keeps one
lifetime creation membership. Skill versions retain their own invocation counts.

Names, people and skill versions are shown a page at a time. **Next page** continues;
**Back to beginning** returns to the first page.

Open Used for all time, the last 30 UTC days, distinct people and the last use.
Providers also show distinct threads. **Recorded since** means earlier history is
incomplete. **Not recorded**, or a dash, means the program has not reported a reliable
figure. A mount has no Used figure. Codex skill discovery and delivery do not prove
an invocation; unreported uses are not guessed from conversation text.

**By use** sorts the whole matching list before loading its next page. Counts with
unrecorded history stay after known figures. The sort counts each resource in its
own unit, so it is most useful within one resource kind.

## Return to a list

Each list remembers its own view, sort and filters on this device. Another page
keeps its own state. Search words start empty on a plain visit. A link or Back
shows its addressed state and makes that the page's last state. A choice you
can no longer use falls back to the page's default.

These are previous page states, with no setting or inherited default. Clear them
with the device-data controls in Interface settings.
