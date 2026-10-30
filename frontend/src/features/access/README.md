# Access: the permission page

Who may do what with a thing is one page, the permission page of that thing, drawn from the catalogue of
permissions the server gives (`GET /permissions`) and from the page of the object itself
(`GET` and `PATCH /{object}/{id}/permissions`). Its Share form lists each person, group and role with
its level and source, with Owners above them. Group and role people expand in place only when opened;
each person says through which holder they get that level. The full form has one cell for
each row by area, with the advanced rows and Administration. A place also has People (who holds which role in it)
and Defaults (what everything made inside it starts from) beside its own page. Share, in the menu of every node, opens
this dialog with the field that adds people in focus.

The ordinary add row is the holder search, level and Add. Options holds source-place browsing, scope,
expiry and export; the selected scope's boundary stays visible. No unavailable View-as action is offered.
People use display names rather than routing addresses. Department Members adds inherited role holders
from the same native Roles reply without calling them domain members; invitations start on Link when
mail cannot leave and continue to respect the org's allowed forms.

The entry point for other screens is `openAccess(target, options?)` in `AccessDialog.tsx`: `target` is
`{ kind, id, name }` of the thing, and `options` may say which tab to open and whether the field that adds people
has the keyboard (it has, unless told otherwise). `openAccessToMany(targets)` opens the form for several things at
once: the people named are named on each page the person may give there, and a page they may not is skipped with who can.
Share keeps its owner and access lists visible on opening: focus alone does not open suggestions or read candidates.
The list opens while the person types; clearing the field closes it again. Source-place browsing remains an explicit option.
Expanded people use the response's native `owners` identifiers to say Owner, even inside an Everyone chip;
the other people retain their level and holder attribution. Roles lists the returned Owners row separately,
without inventing editable Admin assignments. Its primary-owner mark comes from the node's native owner.
The ticked topic list opens this same form from its selection's Share action. Each page and its sharing preview are read before review;
the exact displayed body and revision are then sent once for that row. A refused or unreadable page has its own result while the other pages continue,
and Tell them names only confirmed successes. An uncertain answer can be read explicitly and supplies no invented Undo receipt.
`ShareConsequences.tsx` draws every returned holder, human effect, direct entry bound, agent reach and policy source.
`shareReview.ts` applies the frozen rows in order and builds successful-entry inverses against the returned page revision.
The agent form reads the same served preview before its ordinary share/copy request; a preview accepts no copy or input.
`<AccessHost />` draws the one dialog and is mounted once, in the shell. A screen whose content the server refused puts `NoAccess` in its place, or `NodeFailure`, which picks
between that and the ordinary error; `WhoMayUse` and `whoMayUseChange` end the dialogs that make a resource, and `NewPage` shows what a
new thread's page will be.

What a change does: every change is one `PATCH` made against the revision the person read, and one change is
made at a time, so the controls wait for the answer. A page that somebody else changed meanwhile is shown as it now
is, with what was tried, and nothing of theirs is overwritten. A change that takes something away, or that reaches
other pages, is confirmed once; the server is asked first whom it reaches. Holders are shown as names, with a count
where the server gives one, and the people behind a role, a group or a place are fetched when the chip is opened. What
a person may not do is shown off with the permission it takes and whom to ask, never hidden.

General access uses one control on every short form and in the new-thread dialog: the place's rules, everyone at
View / Take part (Edit, Write or Use in that kind's words) / Manage, or only people added. An org's audience is every
member of that org; other pages name the Viewer role at their parent. Directly named people keep their own levels.
A holder followed from the place can be changed or removed in the short form: one revisioned PATCH freezes only the
touched rows, preserves their other followed holders and makes the edit. The other rows still follow. The control
then says Custom with Back to the place's rules, which resets the whole page after one confirmation.

Giving: the form offers what its reader may give, and says why it does not offer the rest. The page says what that is
(`gives`: the three choices, the roles and the levels for agents the reader may name, and whether people who only hold
the thing may share it at all) and each row says whether the reader may give it (`can_give`). A choice or a role above
what the reader holds is shown off, with the reason in the reader's own terms; where a place keeps sharing to its owners
the field is off, names them, and offers Request access. Every entry, role and share says who gave it and when, and
whoever shares takes back only what they gave themself. The tick "They may take it away as a file" (for agents, "They
may download it") goes with a choice: on as the form opens, withheld when unticked, absent with the reason for a sharer
who may not take the thing away as a file.

A place's share says what it covers: "<Place> and everything in it" gives a role there, so what is made later is
covered too, and "Only <place> itself" names the people on the place's own page, with its chat and its documents and
nothing inside. The counts under the control come from the page (`covers`, and what is closed to them from
`covers.apart`). A thread, a document and a chat have no such control. The Agents tab is the same form on the requests
of sharing with agents: whose agents, the place's control, Can view, Can edit or Can manage, By area with the three
steps of the documents, a share that stays or a copy (its size against the limit, and when it ends), a table of what is
shared with agents, by whom and since when, each row changed or taken back after a confirmation, and a line of what the
agents of the place know. Asking for access and deciding a request, the people behind a role from its chip, the names
behind the counts of a preview, Tell them (one message to each person from the sharer in the chat the two have) and
the form for several things are built on what the server serves.

The doors: a thread's three choices name its chat, its documents and its session; its environment's files, terminal,
browsers and displays are rows of the environment's own page. Under What goes with it the form for a thread says whose
files its files are and how many people look into them, and offers Let them into the environment, on as the form
opens: readers can look at files, terminal and screens, never use or change them, even when they manage the thread.
The environment's page has Can view and Can manage, five rows by area with what each Manage
means, and the list of who reaches it.

What is not there yet, and why:

- The Approvals screen is not built: a request waiting for its approver shows as a card on the page it asks about
  ("Waiting for you"), and the decision is made there.
- The button that gives a thread an environment of its own is not built; the switch it belongs to is a placeholder.
- Where the person at a thread's page may not let people into its environment, the form asks the environment's owner for
  them, naming them as the holders of the request. The server refuses a request whose holder is somebody other than the
  person who asks (`request.invalid`), so the form says that they could not be let in; they are named on the thread all the
  same, and can ask for themselves from it.
- The tick that lets agents download what is shared is sent only when unticked, and the tick that lets people take a thing
  away as a file travels with a choice the same way (`features/access/pending.ts` types both, since the generated types do
  not name them yet).
- A copy cannot be kept yet: the server answers `capability.unsupported` and the dialog says so, offering a share that stays.
- Whose agents are looked up by name through the server's search of names; where that search is not served the dialog
  says so, and the agents of the place itself are chosen with its button.
- History reads every permission-page kind through the same paginated endpoint, only when opened.
  Its own read failure and Retry stay in the disclosure; a resource kind is never treated as a missing reader.

- `AccessDialog.tsx`: The page behind Share in its short form: the Owners line and the three lines; adding people, groups and roles, with the optional message that tells them, for one thing or for several selected at once; what goes with it; who else reaches it; and the tabs of a place.
- `ShortForm.tsx` / `AccessRoster.tsx`: Owners, compact add controls, holder levels/sources and on-demand people. Custom permissions stay named; dependency acts remain available. Advanced permission rows live once in Permissions.
- `Owners.tsx`: The Owners line of a page: who owns the thing, Add owner, Make first, taking oneself out, and the last owner who cannot.
- `AddHolders.tsx`: The field that adds people, groups and roles to a page with one of the three choices and an optional end, and says what that comes to before it is done.
- `RowCell.tsx`: One cell of the full form: a row of the page with whom it names and whom it follows from a place, the lock that decides it, and the acts on it.
- `ByArea.tsx`: The full form: one cell for each row, following its place or cut loose, locks, the advanced rows, and Administration.
- `Roles.tsx`: People: who holds which role in a place, who holds one from above, and what each role does there as things stand; a role as a name and a count, with its people on request.
- `Defaults.tsx`: What a place gives to what is made inside it: a grid for each kind, two columns where it depends on where work runs, locks, and how many pages follow.
- `Agents.tsx`: Agents that can read or write this and why; share with agents by area (conversations, chats, documents), a thing or a whole place, as a copy or live, to read or to write, where editing a place's documents is also what makes them there; take either back.
- `NewPage.tsx`: The page a thread will have, where one is created, forked or switched: one line when it follows, the page of a fork's source carried over, and what changes between a managed environment and a person's machine.
- `GeneralAccess.tsx`: The shared audience control and catalogue-derived words, with Custom/reset and the readonly environment tick.
- `Outsiders.tsx`: Somebody from outside the root org named on the page of a place, a resource or a document, by whoever holds the row for it or by an owner where the policy lets owners: a person, a role or a group; Chat only, a guest, or an invitation link; disabled, with who can, for everybody else.
- `EntryOptions.tsx`: What a holder's entry on a resource carries: models, efforts and arrangement types on a provider, the limits of a host and of a template, and expiry.
- `ChangePreview.tsx`: Who gains and who loses what, before a row is cut loose, an entry removed, a default changed or a lock set.
- `ViewAs.tsx`: What one person holds and through what, for every permission-page kind through the native effective-access endpoint; the full page has one entry, errors retain Retry, and choosing another person clears stale results. Chats retain their membership model rather than a permission page.
- `AdminMode.tsx`: One server-held session switch and title-bar state; ordinary pages remain ordinary while it is off. Management visibility uses that mode plus the server's system-admin fact.
- `AdminReachNotice.tsx`: A private answer marked by the server names the administered place without granting access or changing a page.
- `ParticipationFilter.tsx`: The name-first ordinary focus filter, independent of authority and notification subscriptions.
- `AccessRequest.tsx`: Ask for everything that is missing in one step, with a proposed end and a reason; each part goes to its owner.
- `RequestEnd.tsx`: Choose and display the proposed end of a request, preserving it across a retry.
- `GoesWith.tsx`: What a permission is no use without: Include the model for a thread, Open to the work here for a place, or a request to whoever decides.
- `Reaches.tsx`: What reaches into an environment: a thread's files, terminal, browsers and displays are its environment's, how many people look into it, what making the thread private leaves open; and, on an environment's page, who can reach it.
- `WhoMayUse.tsx`: The control that ends every dialog that makes a resource: only me, everyone in the org, chosen people, or as the org decides.
- `NoAccess.tsx`: Request access; or, for an admin of the place, the act itself with the override hint.
- `model.ts`: The page as a person reads it, and the changes it makes: who stands in which of the three lines, which rows are advanced or administration, how a grid is laid out, and each change as the request that makes it.
- `changes.ts`: Changing a page: the one request that edits it, made against the revision the person saw, with the page somebody else changed meanwhile shown and never overwritten.
- `candidates.tsx`: Whom a page can name: the people found as a name is typed, the groups of the org, and the roles, each with what is known of whether they are outside the org the page lies in.
- `holders.tsx`: A holder of a row as a chip: a person with their avatar, a role, a group or a place by its name, where it comes from, and the cross that takes it out.
- `names.ts`: What a permission is called in the reader's language: the words the product has for it everywhere, else the words of this page, else the label the server gives it, else its key.
- `target.ts`: What a thing with a page is called and where it is: the address of its screen, for a link that is passed on.
- `AgentRow.tsx`: One share with agents as a line of the Agents tab: whose agents, at what level in each area, until when, who shared it and since when, the level changed after being asked about, and the ✕ that takes it back.
- `Covers.tsx`: What sharing a place reaches: the control that says "<Place> and everything in it" or "Only <place> itself", and the line under it that counts what is reached and what stays closed.
- `KnownToAgents.tsx`: What the agents of a place know: what was shared with the agents of this place or of a place above it, each with where it comes from and who shared it.
- `RequestCard.tsx`: The card of a request for access that waits for its approver: who asks, for what and why, the permissions as ticks to give fewer, an end, and Approve or Deny with a note.
- `Several.tsx`: Share opened from a selection of several things: the same short form headed "Access to 3 threads", naming the people on each page they may give on, listing the pages it skips with who can change them, and telling each person once about all of them.
- `ShareWithAgents.tsx`: The dialog that shares a thing with agents: whose agents (threads, or places), what a place covers, the everyday choice with By area behind it, a copy or a share that stays, and a button that says what happens.
- `Tell.tsx`: Tell them: the tick under the field that sends each person named one message from the sharer in the chat the two have, with the link and a line the sharer may write.
- `agentShares.ts`: What sharing with agents is made of: the three areas a thing has, the everyday choice and what it means for each, how far a share goes, and who may change one.
- `doors.ts`: Letting people into the environment a thread runs in: what sharing a thread offers of it, what that gives and no more, and the change or the request that does it.
- `gives.ts`: What the reader of a page may give: the choices and the roles that are theirs to name, the words that say why the rest is not, and the case of a place that keeps sharing to owners.
- `hints.ts`: The hints of the access screens, by the form each stands in: the three lines of a page for each kind of thing, what sharing a place reaches, the tick that lets people take a thing away as a file, and the choices of sharing with agents.
