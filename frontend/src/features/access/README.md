# Access: the permission page

Who may do what with a thing is one page: the permission page of that thing. It is drawn from the catalogue of permissions that the server gives (`GET /permissions`) and from the page of the object itself (`GET` and `PATCH /{object}/{id}/permissions`). **Share**, in the menu of every node, opens it with the field that adds people in focus.

- **Share** (the short form) lists each person, group and role with its level and source, with the Owners above them. A group or role expands to its people only when opened, and each person says through which holder they get their level. The ordinary add row is the holder search, a level and **Add**. **Options** holds source-place browsing, scope, expiry and export, and the selected scope's boundary stays visible.
- **By area** (the full form) has one cell for each row of the page, with the advanced rows and Administration.
- A place also has **People** (who holds which role in it) and **Defaults** (what everything made inside it starts from).
- People are shown by display name, not by routing address. Roles lists the returned Owners row apart and does not invent editable Admin assignments.

Other screens open the dialog with `openAccess(target, options?)` in [AccessDialog.tsx](AccessDialog.tsx). `target` is `{ kind, id, name }` of the thing, and `options` can name the tab to open and whether the field that adds people has the keyboard (it does, unless told otherwise). `openAccessToMany(targets)` opens the form for several things at once: the people named are added on each page the person may give on, and a page they may not give on is skipped, with who can. Each page and its sharing preview are read before review, then the exact displayed body and revision are sent once for that row. A refused or unreadable page has its own result while the other pages continue.

## Files

| File | What it does |
|------|--------------|
| [AccessDialog.tsx](AccessDialog.tsx) | The page behind Share in its short form: the Owners line and the three lines; adding people, groups and roles, with the optional message that tells them, for one thing or for several selected at once; what goes with it; who else reaches it; and the tabs of…. |
| [AccessRequest.tsx](AccessRequest.tsx) | Ask for everything that is missing in one step, with a reason; each part goes to its owner. |
| [AccessRoster.tsx](AccessRoster.tsx) | The Share list: identities, their level and its source; a group opens its actual people with the same attribution. |
| [AddHolders.tsx](AddHolders.tsx) | The field that adds people, groups and roles to a page with one of the three choices and an optional end, and says what that comes to before it is done. |
| [AdminMode.tsx](AdminMode.tsx) | The one session-wide switch, with a persistent named title-bar indicator while the reach is on. |
| [AdminReachNotice.tsx](AdminReachNotice.tsx) | A server-marked private opening stays explicit, without asking the administrator to grant themselves access. |
| [AgentAccessChoice.tsx](AgentAccessChoice.tsx) | The same exact Share areas, confirmed as a creation choice or edited as its inherited preset. |
| [AgentAdvancedRows.tsx](AgentAdvancedRows.tsx) | Independent thread acts use the same Advanced disclosure as the ordinary permission page. |
| [AgentContribution.tsx](AgentContribution.tsx) | Requested ceilings and an exact contribution's assessed area rights are separate facts. |
| [AgentRow.tsx](AgentRow.tsx) | One share with agents as a line of the Agents tab: whose agents, at what level in each area, until when, who shared it and since when, the level changed after being asked about, and the ✕ that takes it back. |
| [Agents.tsx](Agents.tsx) | Agents that can read or write this and why; share with agents by area (conversations, chats, documents), a thing or a whole place, as a copy or live, to read or to write, where editing a place's documents is also what makes them there; take either back. |
| [agentShares.ts](agentShares.ts) | What sharing with agents is made of: the three areas a thing has, the everyday choice and what it means for each, how far a share goes, and who may change one. |
| [agentShareWords.ts](agentShareWords.ts) | One set of area names and nested level words for requested and currently assessed contributions. |
| [ByArea.tsx](ByArea.tsx) | The full form: one cell for each row, following its place or cut loose, locks, the advanced rows, and Administration. |
| [candidates.tsx](candidates.tsx) | One visible holder picker: current people, groups, roles and members with their source places. |
| [CardSharing.tsx](CardSharing.tsx) | A managed thing's sharing line opens the existing short access form. |
| [cardSummary.ts](cardSummary.ts) | Cards describe the same audience and current management authority as the short access form. |
| [ChangePreview.tsx](ChangePreview.tsx) | Who gains and who loses what, before a row is cut loose, an entry removed, a default changed or a lock set. |
| [changes.ts](changes.ts) | Changing a page: the one request that edits it, made against the revision the person saw, with the page somebody else changed meanwhile shown and never overwritten. |
| [Covers.tsx](Covers.tsx) | What sharing a place reaches: the control that says "<Place> and everything in it" or "Only <place> itself", and the line under it that counts what is reached and what stays closed. |
| [Defaults.tsx](Defaults.tsx) | What a place gives to what is made inside it: a grid for each kind, two columns where it depends on where work runs, locks, and how many pages follow. |
| [doors.ts](doors.ts) | Letting people into the environment a thread runs in: what sharing a thread offers of it, what that gives and no more, and the change or the request that does it. |
| [EntryOptions.tsx](EntryOptions.tsx) | What a holder's entry on a resource carries: models, efforts and arrangement types on a provider, the limits of a host and of a template, and expiry. |
| [GeneralAccess.tsx](GeneralAccess.tsx) | One general audience choice, using the same catalogue and rows as the three-line form. |
| [gives.ts](gives.ts) | What the reader of a page may give: the choices and the roles that are theirs to name, the words that say why the rest is not, and the case of a place that keeps sharing to owners. |
| [GoesWith.tsx](GoesWith.tsx) | What a permission is no use without: Include the model for a thread, Open to the work here for a place, or a request to whoever decides. |
| [GroupAccess.tsx](GroupAccess.tsx) | A group's Share surface is its native roster and visibility, inside the existing Access dialog. |
| [hints.ts](hints.ts) | The hints of the access screens, by the form each stands in: the three lines of a page for each kind of thing, what sharing a place reaches, the tick that lets people take a thing away as a file, and the choices of sharing with agents. |
| [holders.tsx](holders.tsx) | A holder of a row as a chip: a person with their avatar, a role, a group or a place by its name, where it comes from, and the cross that takes it out. |
| [KnownToAgents.tsx](KnownToAgents.tsx) | What the agents of a place know: what was shared with the agents of this place or of a place above it, each with where it comes from and who shared it. |
| [model.ts](model.ts) | The page as a person reads it, and the changes it makes: who stands in which of the three lines, which rows are advanced or administration, how a grid is laid out, and each change as the request that makes it. |
| [names.ts](names.ts) | What a permission is called in the reader's language: shared words, the words of this page, or a readable label the server gives it. |
| [NewPage.tsx](NewPage.tsx) | The page a thread will have, where one is created, forked or switched: one line when it follows, the page of a fork's source carried over, and what changes between a managed environment and a person's machine. |
| [NoAccess.tsx](NoAccess.tsx) | A real refusal names what is missing and offers the ordinary access request when it is served. |
| [opening.ts](opening.ts) | Cached private views still report an opening, without making another grant or content read. |
| [Outsiders.tsx](Outsiders.tsx) | Somebody from outside the root org named on the page of a place, a resource or a document, by whoever holds the row for it or by an owner where the policy lets owners: a person, a role or a group; Chat only, a guest, or an invitation link; disabled, with…. |
| [OwnerReadConfirmation.tsx](OwnerReadConfirmation.tsx) | The server's informed-sharing notice, with the same explicit confirmation on every share path. |
| [Owners.tsx](Owners.tsx) | The Owners line of a page: who owns the thing, Add owner, Make first, taking oneself out, and the last owner who cannot. |
| [participation.ts](participation.ts) | Focus uses the Frame's explicit assignment/follow fact, independently of readability or admin reach. |
| [ParticipationFilter.tsx](ParticipationFilter.tsx) | An ordinary focus filter narrows the displayed work, without changing anyone's authority or subscriptions. |
| [Reaches.tsx](Reaches.tsx) | What reaches into an environment: a thread's files, terminal, browsers and displays are its environment's, how many people look into it, what making the thread private leaves open; and, on an environment's page, who can reach it. |
| [RequestCard.tsx](RequestCard.tsx) | Access requests on a permission page use the same timed decisions and settled status as the Inbox and reference card. |
| [RequestEnd.tsx](RequestEnd.tsx) | The proposed end of an access request and its shared presentation. |
| [Roles.tsx](Roles.tsx) | People: who holds which role in a place, who holds one from above, and what each role does there as things stand; a role as a name and a count, with its people on request. |
| [RowCell.tsx](RowCell.tsx) | One cell of the full form: a row of the page with whom it names and whom it follows from a place, the lock that decides it, and the acts on it. |
| [Several.tsx](Several.tsx) | Share opened from a selection of several things: the same short form headed "Access to 3 threads", naming the people on each page they may give on, listing the pages it skips with who can change them, and telling each person once about all of them. |
| [ShareConsequences.tsx](ShareConsequences.tsx) | Every holder, human effect, agent reach and policy source in an ordinary share preview. |
| [shareReview.ts](shareReview.ts) | Frozen holder-share reviews and revision-bound inverses of confirmed additions and updates. |
| [ShareWithAgents.tsx](ShareWithAgents.tsx) | The dialog that shares a thing with agents: whose agents (threads, or places), what a place covers, the everyday choice with By area behind it, a copy or a share that stays, and a button that says what happens. |
| [SharingReadState.tsx](SharingReadState.tsx) | Sharing reads keep a nameless current state without lending a cached audience. |
| [ShortForm.tsx](ShortForm.tsx) | Share: owners, a compact add field and the people, groups and roles with their level and source. |
| [target.ts](target.ts) | Permission-page targets use the same public addresses as their menus, including chat and document folders. |
| [Tell.tsx](Tell.tsx) | Tell them: the tick under the field that sends each person named one message from the sharer in the chat the two have, with the link and a line the sharer may write. |
| [ViewAs.tsx](ViewAs.tsx) | What one person holds here, and through what. |
| [WhereField.tsx](WhereField.tsx) | The place an entry lets work run in: the environments, hosts and machines it names, as one compact control, and the names of them wherever a refusal says where. |
