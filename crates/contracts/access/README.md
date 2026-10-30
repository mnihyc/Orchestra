# orch-access-contract

Permission catalogue, roles and permission-page contracts, without access decisions.

Implemented. The catalogues are here as data (every permission with what it
is on, its area and level, what it goes with and the options it takes; the
four roles; every audit action with whether it is a line of a thing's
history), with the types of a page and of a decision, and the rules that
need no storage: what a row resolves to through the pages above it, when a
lock decides, and whether a change to a page is valid. The three Export rows
(`thread.export`, `chat.export`, `doc.export`: taking a conversation, a chat or
a document away as a file, shipped to Viewers) are the one kind of row that
travels with the three everyday choices, as far as the giver holds them
(`NameChange.export`, `CheckedRow.optional`). Unit tests state
each rule.

| Module | Planned responsibility |
|--------|------------------------|
| [permissions](src/permissions.rs) | Closed permission catalogue: areas and levels on nodes, on documents and on resources, an environment's two levels among them, the advanced rows for one act each (owning a thing, naming somebody from outside, letting a turn past a limit among them), the options a permission takes, and the three everyday choices. |
| [roles](src/roles.rs) | The four roles, as names held in a place, with Admin inherited downward from the system through orgs and the orgs inside them. |
| [pages](src/pages.rs) | Permission pages: the Owners row, rows that follow or are cut loose, entries with their holders, options, expiry and source; defaults pages with their two profiles and locks. |
| [decisions](src/decisions.rs) | What a person holds and through what, and refusals with everything that is missing: the nearest people who can give it and how many more could, what an admin of the place may force, and what nobody can give. |
| [audit](src/audit.rs) | Audit action catalogue, with the actions that are lines of a thing's history and the mark of a bypass. |
