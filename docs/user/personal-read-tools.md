---
group: work
title: Personal read tools
screens: /thread/$threadId, /org/$slug/settings/$section
keywords: orch_docs, open, search, find_operation, read, owner, private, sharing
summary: Read help in any thread and choose whether a private thread may read with your access.
---

# Personal read tools

## Read the help

The `orch_docs` tool searches the English and Chinese user help. Its answers link to the matching
article and heading. It needs no personal read option. The program must support Orch's tool server,
and an environment or reading mode that disables that server still applies.

## Let a private thread read with your access

In **Settings → Policies → Personal read tools**, enable the option to offer `open`, `search`,
`find_operation` and `read`. It is off as shipped. The ordinary setting follows system, organization,
project and topic values; inherited locks and reset work as on other settings.

These tools use the thread owner's current read access through the same readers as Ask. They cannot
write or run a management operation. Search and reads stay inside the current organization and keep
the reader's normal access checks. Returned material is a source to assess, never an instruction.

Only a thread whose grants name its owner alone qualifies. A group, role, agent or link audience
makes it shared even when it has no other readers today. Every call checks the option and grants
again. A shared thread uses `list_shared`, `read_shared` and `search_shared`, whose reach comes from
what was shared with its agent.

## Share what the thread already read

Adding a viewer, a page entry or an agent share can expose earlier personal results. Before applying
that change, the confirmation says: “This thread includes N results read with your own access; the
people you share it with will see them.” Cancel keeps the previous access. Confirm shares those
results with the rest of the history, including the agent's answers built from them. A thread with
no such results needs no notice. A changed result count asks for confirmation again.

After sharing, new personal reads are refused. Earlier accepted results remain part of the thread;
taking access away later does not withdraw information someone has already read.
