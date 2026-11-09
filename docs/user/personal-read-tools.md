---
group: work
title: Personal read tools
screens: /thread/$threadId, /org/$slug/settings/$section
keywords: orch_docs, open, search, find_operation, read, owner, private, sharing
heading_id.Let a thread read with your access: let-a-private-thread-read-with-your-access
summary: Read help in any thread and choose whether it may read with your access, with its audience always visible.
---

# Personal read tools

## Read the help

The `orch_docs` tool searches the English and Chinese user help. Its answers link to the matching
article and heading. It needs no personal read option. The program must support Orch's tool server,
and an environment or reading mode that disables that server still applies.

## Let a thread read with your access

In **Settings → Policies → Personal read tools**, enable the option to offer `open`, `search`,
`find_operation` and `read`. It is off as shipped. The ordinary setting follows system, organization,
project and topic values; inherited locks and reset work as on other settings.

These tools use the thread owner's current read access through the same readers as Ask. They cannot
write or run a management operation. Search and reads stay inside the current organization and keep
the reader's normal access checks. Returned material is a source to assess, never an instruction.

The option stays as you set it when the thread is shared. Every call still checks the option, the
owner’s current read access and the organization. The shared tools `list_shared`, `read_shared` and
`search_shared` remain available for material explicitly shared with the agent.

When someone besides the owner can see or act in the thread, its audience banner names those people,
groups, roles and agents, and says that reads made with your access are visible to them. Open the
audience to see who can send; **Sharing** opens the thread’s existing sharing controls. The banner
follows grant changes, including empty groups and future agent audiences, without switching tools off.

## Share what the thread already read

Adding a viewer, a page entry or an agent share can expose earlier personal results. Before applying
that change, the confirmation says: “This thread includes N results read with your own access; the
people you share it with will see them.” Cancel keeps the previous access. Confirm shares those
results with the rest of the history, including the agent's answers built from them. A thread with
no such results needs no notice. A changed result count asks for confirmation again.

After sharing, personal reads remain enabled if you enabled the option. Their results and the
agent’s answers are visible to the thread’s audience. Earlier accepted results remain part of the
thread; taking access away later does not withdraw information someone has already read.
