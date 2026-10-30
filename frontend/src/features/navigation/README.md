# Organization and work navigation

Projects switches between Tree and a paged Recent listing, ordered by the reader's own visits or by
thread/document activity. The shared listing applies current access, beginning, kind, archive and done
filters before paging. Calendar groups and the same switches also serve Everything. Each reader and
org keeps its previous arrangement on this device. Show in tree opens the path and locates a row,
including documents beyond their first page, without navigating Main.

`recent.ts` supplies calendar grouping and listing predicates; `ArrangementSwitch.tsx` supplies the
shared controls, `RecentList.tsx` renders tree rows/menus and `showInTree.ts` carries an explicit reveal
between a wide page and Navigation. `useRecentUpdates.ts` follows the existing visit/reconnect events
for visible exact listing queries. Existing org selection and ordinary tree actions remain native.

Recent reuses `RowDetails` for the nearest parent's name and the full authorized path on hover.
`RelativeTime compact` follows each calendar group's short form without truncating the time.
Desktop menus replace the whole time on hover, focus or selection; phones keep both. The flat tree
has no expander gutter. Recent's narrowing field uses a short placeholder beside its order control.
Thread rows read their server display through the same state renderer as Tree. Other places retain
their kind and colour with the shared contained-work priority; no row acquires a thread feed.

Built, as two views of the left bar and the path above a screen. The Navigation view, from the outside
in: the org in view with its own menu; the org's own rows, its team's chat and its Messages; what its
reader starred; what they opened last; and the tree of the org. The Resources view: the kinds of
resource, each with how many its reader may see, opening to the things of that kind. The rail leads to
both: an org's stop to its tree, the Resources stop to the resources. Inbox and Approvals are a
person's own and stand in the rail alone; on a small screen, which has no rail, the head of the bar is
the way to another org, to what was shared and to the resources.

Resources reads children only for kinds explicitly expanded by this person in a shown view. Visiting a
detail page marks its kind without expanding it. The explicit expansion key ignores the older device
key, which mixed visits with chosen expansions; counts remain the one shared native summary.

Settings, Management, Help and Usage keep their section links inside their own page frame. Global
Navigation retains its chosen organization tree and its saved visibility. A place's settings reuse
the page's canonical native identity read. Help section links have addresses, and Usage links preserve
the chosen scope, dates and filters. Opening these pages never retitles or replaces Navigation.

Navigation's lazy native place menu accepts the view's layout choices through the same trigger;
those appended choices have their own ids, leaving row and screen actions and permissions intact.

Starred, Recent and Projects each fold by a marker at their heading, and how each stands is kept with
the person on every device, as are the kinds of thing the tree shows and whether topics that are done
are hidden. Recent is folded as shipped and lists the last eight things opened in the org. Every
heading has the height of a row, every section the same room around it, and every row begins its icon
on the same line: one rhythm from the top of the bar to its foot. What a row carries stands in one
column at its end; its "+" and "⋯" lie over that column where the pointer rests or the keyboard is,
and take no room of their own.

The device visit store also carries this tab's bounded visit times for org home. Visits newer than
its held names answer appear first without another read; times stay in memory and use the same
client clock as that answer. The authorized page read records the server visit; the local recorder
sends no additional POST, including for a canonical personal target. Opening a document or chat
does not count as opening its containing thread. `recent-visit-order.test.tsx` retains person/snapshot
isolation, exact local storage ordering and one local write per navigation, with zero visit mutations.

The tree is kept current by the org's live feed: orgs inside the org, projects, the folders inside a
project to any depth, topics, and, as its reader ticks them in the tree's own menu, threads with what
they are doing, the documents written beside each place (read when the place is opened) and the chat
of each place somebody wrote in, with the org's group chats. A topic that is done stays where it is,
with a check in its icon and a quieter name; nothing moves when one is marked done or opened again. An
org, a project, a folder and a topic show the glyph of their kind in the colour of the kind, or in the
one of the twelve that somebody who manages the place gave it ("Colour…": the twelve by name, "As its
kind" and "Random"); the same icon stands in the path above a screen.

One rule for every row that hides something, and for the heading of a folded section. It is bold with
a dot while anything in it that its reader follows is unread, threads and chats alike, worked out from
the tree, from the reader's read positions and levels and from their standing in chats, which their
own feed keeps current on every device. It carries a number only for what waits for the reader in
person: the decisions that wait, and the times they are mentioned. It never carries a sum of unread
things. A chat its reader does not follow is a quiet row, says nothing on the rows above it, and still
carries a mention. A place's chat is a small glyph on the place's row while the chat has no row of its
own on screen. Nothing of a thing the reader may not see is said, since it is not in their tree.
"Mark as read" in a row's menu reads every thread and chat of the row and of what lies in it.

A reader narrows the tree to one place: "Show only this" in a place's menu, "Show only where I am" in
the tree's own. The heading is then the way to that place, each place above it a step back, and "Show
everything" is in the tree's menu. The tree stays where it was put wherever its reader goes; while
they look at something outside it, its first row says where that lies and shows it there. Where the
tree begins, like which of its rows are open, belongs to the work under way on one device: it is kept
there for each org, and is in no address. A place that is gone narrows nothing. Folding away is the
reader's act as well: "Collapse the others" in a row's menu, "Open to where I am" and "Collapse all" in
the tree's own. Going somewhere opens the way to its row once, and nothing folds by itself. While the
tree scrolls, the places above its first row stay at its top, for no more than a third of its height.

Every row has its right-click menu, its "⋯" and, on an org, a project and a folder, its "+": one list
of acts, made only for the row that is asked, in which what the caller may not do is left out. A
document's row has a document's menu. The dialogs those acts open are drawn once for the whole
application. `Alt+↑` and `Alt+↓` go to the thread before and after the one in view, with `Shift` to
the next thing that is unread, a chat among them where chats have rows. With the keyboard in the
tree, `Alt+Shift+→` narrows the tree to the row the keyboard is on and `Alt+Shift+←` steps one place
up; where either cannot, it says why. A field at the top of the tree narrows it to the places and
threads with the typed letters in their name, each with the way down to it; `Ctrl+Alt+F` gives it the
keyboard from the tree, and Escape empties it. Those three keys are the tree's own: they are bound,
and stand in the list of keys, while the keyboard is in the bar.

The path above a screen is drawn from the tree where the tree holds the whole way, and from the node's
own way where it does not: a place the reader may not see is named by its kind alone ("Private
project"), is no link, and the way goes on past it. The number on Messages, and the dot on an org of
the rail, are counted from the person's own chats and their standing in each; the numbers on Inbox and
Approvals in the rail are the Inbox part's own (`features/inbox/Counters`), read from the person's feed. Two
orgs of the rail whose names begin alike show a second letter, and each its own colour.

Covered by unit tests of the tree, the menus, what is unread, where the tree begins and what the bar
keeps, and by end-to-end, accessibility and performance tests against the fake API. A tree of two
thousand rows is laid out again at every change of a thread without sorting again (what stands under
what is kept while no node moves), a row whose item did not change is not drawn again, and the frame is
not drawn again at every change either (the screens of places watch the stores).

- `Sidebar.tsx`: The orgs inside the selected org, projects, topics, threads, starred and unread state, chat signals and Messages.
- `currentOrg.ts`: Cache-only destination selection; no remembered-org tree/chat detour before the addressed place is known.
- `area.ts`, `AreaSections.tsx`: Route-only area names and Help/Usage section links; personal/system links reuse the settings frame's navigation source.
- `TeamChat.tsx`: A native button reveals the existing team chat and focuses its composer, keeping the current page and draft.
- `PlaceHeaderPanels.tsx`: The named counterparts of a place page's header icons: entries of its "…" menu that open its description, announcements or notifications in a dialog.
- `Section.tsx`: A section of the left bar that folds: one quiet sentence-case heading, its marker, what needs a look while folded, its actions and rows. Icons share the 16px row context and the application's one stroke.
- `OrgSwitcher.tsx`: Personal and team organization selection.
- `NodeTree.tsx`: Visible hierarchy and private ancestor references.
- `treeRows.ts`: The tree of the left bar as rows in order: what stands where, what each row carries of what it hides, and what is left out, worked out from the tree and from what its reader chose and has read.
- `TreeFilter.tsx`: The field that narrows a tree as its reader types: shared field styling, a 12px inset before its 16px search icon, an 8px text gap and room for the clear action; matching names keep their way to them.
- `beginning.ts`: Where the tree of the left bar begins and which of its rows are open: what one device keeps of a tree, the place the tree is narrowed to, and the branches that lead to a row.
- `Resources.tsx`: Resource kinds share the landing's authorized count read; expanding a kind reads only its bounded name pages, never the other kinds or their runtime details.
- `NodeMenu.tsx`: Permission-aware row/context and screen actions. Compact header icons keep named equivalents in the menu; primary creation remains in the page toolbar, with secondary containers also in the menu.
- `DocMenu.tsx`: A document's menu where it stands in the tree: open it, rename it, move it, share it, take it away as a file, copy its link, delete it.
- `RecentlyDeleted.tsx`: What was deleted under a node, by whom and until when it can be restored.
- `Breadcrumbs.tsx`: Full, wrapping ancestor paths with private places redacted; below 600 pixels the nearest parent's full name is the back link.
- `places.tsx`: What a place is to the left bar and to every menu that acts on one: its kind, its name and where it lies, its address and its icon.
- `favorites.tsx`: Starring a place, which is a person's own and easily undone: it shows at once and is put right if the server refuses.
- `refresh.ts`: Making what the screens of places read follow what happened: after an act of the person's own, and when the tree says that something changed.
- `visits.ts`: The device's bounded recent places and newer-than-snapshot ordering; successful authorized page reads carry server visits without another mutation.
- `archived.ts`: Whether the left bar shows what is archived: one choice for each org on this device, shared by every menu that offers it.
- `preferences.ts`: What the left bar keeps for a person on every device: which of its sections are folded, which kinds of thing its tree shows, and whether topics that are done are hidden.
- `attention.ts`: What is unread for the person: how far they have read each thread and which places they follow, kept current by their own feed, and the rule that makes a thread unread from the two.
- `counts.tsx`: How many of the person's chats in an org are unread: the number on Messages in the left bar and the dot on the org in the rail.
- `treeDocs.ts`: The documents written beside the places that are open in the tree: read when a place is opened, a page at a time, and read again when the place says it holds another number of them.
- `NodeDialogs.tsx`: The dialogs that make a place or change one: a new org, department, project and topic, rename, archive, delete, move, what was deleted, and joining by a link.
- `PlaceDetails.tsx`: Descriptions and tags on demand for the compact headers of orgs, departments and projects. Header and named row disclosures keep the full value, existing guarded edit and native settings destination reachable; closed disclosures mount no tag reader. Collection headers opt into the compact place menu, keeping only secondary container creation there and leaving the one primary action visible.

Org headers use the same metadata body in their page-menu dialog. Every org menu includes the served
native Settings link; personal spaces omit department creation across row, screen and command contexts.
Org screen menus remain menus by default rather than silently expanding into a toolbar. The header's
star and primary action do not repeat in that screen menu; row/context-menu stars remain available.
- `DocDialogs.tsx`: The dialogs of a document that stands in the tree: a new title, another place, and deleting it with the way back.
- `Colour.tsx`: The colour of a place's icon: one of the twelve, the colour of its kind again, or one picked at random, given by whoever manages the place and the same for everybody.
- `depth.ts`: What a refusal for the depth of folders says: the sentence of its code and the number the system gave.
- `PlaceChain.tsx`: The project and the folders a topic lies in, as links on one line: the place, with the folders on the way where there is room.

Recently deleted also reuses the guarded stored document and kept-environment rows, with their actual retention jobs, rather than assuming restoration from the place role.

Failure Details can open the server's access and request information through the application-wide dialog host, including after the person leaves the screen where the failed act began.
