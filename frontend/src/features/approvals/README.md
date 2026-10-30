# All approval kinds

Built: everything that waits for the person, of the three kinds, decided from the list with the same choices the card
in a conversation has. An agent's request: a command, a change to files, a permission, questions to answer and a form an
MCP server asks for, each with the options the harness gave. A capability: its risk, the environment and the host it
would open up, why, what it would do and whose approvers decide it, with a note for whoever asked. Access: one card for
what a person asked together, a row for each thing, every permission asked for ticked (untick to give fewer), the models
and efforts of a provider, when it ends and a note, where what opens a terminal or reaches a person's machine is said in
words. The tabs are "Waiting for me", "Requested by me" and "History"; the kind and "everything I could decide" are in
the address. A request decided meanwhile, by the person or by somebody else, stays where it stood as its line and says who
answered; a decision that comes too late says who got there first. Answering waits for the live connection.
The header names Approvals once without a subtitle; its kind filter, tabs and native lists remain.
Cards have no help entrance inside them; their guides remain in the page's shared help home. Names of
the environment and host stand without internal addresses or a second sentence repeating the requester.
Timeline consumers use the live request snapshot or a confirmed decision immediately; an older pending
snapshot cannot reopen a decided request, and a later live cancellation still replaces its prior approval.
A thin, foreign or malformed cached request cannot replace the item's usable native snapshot. Its own
reader recovers an older or incomplete item; a thin answer offers an explicit Retry in that card.
Both Allow once and Send answers retain the confirmed terminal outcome while their live frame arrives.

- `Approvals.tsx`: Waiting, requested and history views.
- `ApprovalList.tsx`: The three lists of approvals, page by page, and the cards each draws: what waits for the person with its decision controls, what they asked for with its answer, and what was decided.
- `Card.tsx`: The request frame and decided summary; its shared subject scope keeps list and timeline headings while answer fields avoid a second visible copy.
- `Harness.tsx`: Command, file, permission, question and elicitation decisions; field names remain accessible when their visible subject already stands in the request heading.
- `Capability.tsx`: Risk, the scope that decides (host, org or system) and recreation consequences.
- `Access.tsx`: One card for the parts of a request: the permissions, options and expiry of each, decided one by one, with what opens a terminal or reaches a person's machine said in words.
- `Decision.tsx`: Race-aware confirmed outcome.
- `current.ts`: Live request snapshots and confirmed decisions settle a timeline card without another read.
- `tabs.ts`: The tabs of Approvals, as the address names them: read by its route before the page itself is fetched.
- `snapshot.ts`: Matching native request/option/question shapes before a cached value is rendered or offered for decision.
