# Routes

The file-based route tree. Every route says in its static data whom it is for and what stands
round its screen; the root route asks, once for every entry, what the system says of itself and
whether the browser holds a session, and sends the visitor on where the route is not for them
(`-access.ts`). A route whose feature is not built yet (the inbox, approvals, what is shared, usage and the
management of the system, mounts and MCP servers, a person's notifications and usage) draws an honest "not built
yet" inside the workbench until its feature is; every other route is its screen. Topic settings use the topic
tab search parameter.
Dynamic section routes share their feature modules. An org inside another has the org's own
routes, by its address. How the workbench is arranged around a screen is not in its address.
`dev/ui` exists in development builds only.

- `inspect/$kind/$id/index.tsx`: Retained metadata and stored content through the ordinary flagged readers, in a read-only page.
- `trace/$nodeId/index.tsx`: One node's shared trail, including a thread, under that node's current audit authority.
- `everything/index.tsx`: Every signed-in person's visible hierarchy, with an explicit whole-system scope for administrators.
