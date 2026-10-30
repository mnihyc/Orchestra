# Organization home, the same at every level

Built: one home for an org at every level (`/org/$slug`, a root org by its short name and an org
inside one by its address, or by its id), with its overview (this person's waiting and recent threads,
then a shared filter immediately followed by the orgs inside it and its projects as compact rows,
with setup/readme and other context later), its members
(the list of `../settings/org/Members.tsx`, which is also the Members section of the org's settings: listed by
name and a page at a time, invited in either of the two forms, with the invitations still good, a role changed,
a member moved, held out or let go) and its Activity; a person's own org begins empty and says what it
is; moving a project, a folder, a topic or a department with what it changes said first: one
dialog for all of them, whose list of places is the orgs and the projects and folders of the tree
that can hold what is moved, with the one it is in and itself and what lies in it greyed and why,
and whose plan says who gains and loses what, which settings resolve differently and what stops it
(the depth of folders among it) before anything moves. A project's card counts its own open topics
and its folders, and shows what needs a look in all of it. Controls follow what the API says the
caller holds on the org. Documents and the usage board belong to later parts.

Daily work leads the Overview before First steps, readme, filters, org summaries and usage, even when
the children query is empty, pending or refused. Waiting includes the reader's outstanding decisions,
their participating work and threads they recently opened. Other people's admin-only work is not
promoted. A waiting thread appears once, ahead of recents; recents use recorded visit order, rather
than the busiest threads' activity. Every row carries its current readable title, one localized reason
(including approval versus answer, and a classified sandbox refusal), and only visible ancestors.
Names and paths wrap, so phone widths and keyboard users get the whole value without ellipses. State
icons align with the first title line. A native link opens the thread, including middle-click; beyond
six rows, one compact keyboard-operable control reveals the rest in place, without another request.

One thread-only names lookup reuses the existing authorized recents source and its maximum of fifty
retained visits. It adds no detail, approval-list or per-thread read, and no poller. The held tree is
the authority for titles/state/visibility; a stale names hit never restores a revoked or deleted row.
Direct shares remain visible at the root even when a private parent is absent. A department needs its
branch or the authorized visit's path to prove scope; it never borrows cached ancestor names. A failed
visit read can fall back to this person's local visits, cut to the same tree, and still says the read
failed. Known waits remain usable while visits load or fail. Hidden Overview tabs mount none of these
readers, and the Members tab is the one home of the directory preview, avoiding a duplicate request.
Visits made in this tab after the held names snapshot go first immediately, so a fast return to home
does not wait for that query to go stale. These short-lived times use the query client's own clock,
stay inside the existing 24-place device bound and are not persisted as server facts. A node addressed
through the generic visit route is remembered with the same single POST; a document or chat visit
does not pretend its containing thread was opened.

The existing HTTP and live tree carry the same compact `waiting` summary. The API uses approval
listing's own current `can_decide`, including a capability's scoped authority, never participation,
the thread's owner or an approval count. Only that flag says “your approval/answer”; a viewer sees
neutral wording. The classified `blocked_cause` exposes no diagnostic text or internal setting key.
First steps remains limited to current owners of an active team root with observed missing provider/member
setup; Home has no second setup query. Its standalone regression sources cover the full eligibility.

The header has its name, one icon star, one New project and one secondary menu. Description/tags,
announcements, notifications, Settings and other secondary acts live in that lazy native menu;
none becomes another header button or an expanded action wall. Settings is a real link in both
page and navigation-row menus. Descriptions and tags mount only on request. Rows wrap complete names,
align their icon and controls with the first line, and show only differing counts/attention;
department routing addresses remain in native link destinations. Empty department sections are omitted;
the secondary menu offers New department only for team orgs with its current authority. A personal
space has neither that action nor First steps, even with missing provider facts. Children retain pagination,
including an empty readable page with a next cursor. The root and department use the same collection
order. Daily work's earlier priority and First steps' owner/root/missing-setup guard remain intact.

The compact header omits counts. Department rows use the native `people_count`: active humans from
branch membership and Viewer-or-higher roles here, including inherited/group roles, deduplicated.
`member_count` retains its separate domain meaning. Both holders expand in one existing batched
org-facts statement at the caller's clock, using the same suspension/expiry/deletion rules as the
native holder engine. Cards start no directory read; an older missing count is omitted, never guessed
as zero. Names wrap without displaying routing addresses; the count's meaning is available on focus.

- `Home.tsx`: One home for an org at every level: the orgs inside it, its projects, running work, attention, documents and the usage board; for a person's own org, what it is and the ways to begin.
- `HeaderMenu.tsx`: Secondary org-page acts share the existing lazy place menu; only the star and primary act stay outside.
- `Work.tsx`: Daily work first on an org home: named personal waiting threads and recent visits with one readable reason each.
- `workModel.ts`: The person's recent and waiting threads in one org, joined only to its current readable tree.
- `ProjectCards.tsx`: Project and department summaries as compact named rows.
- `Activity.tsx`: An org's Activity: the lines of its own history, the management of the orgs and projects in it, and members joining, moved, leaving, suspended and resumed.
- `Move.tsx`: Move to…: an org that lies in another taken elsewhere in its tree, with who gains and who loses what, what resolves differently and how many addresses change said first.
- `Invite.tsx`: Invite…: one dialog that brings somebody into an org at any level, by a link for one address or a link to share, with the role, the time it is good for and the rules the org sets.

Regression sources: `tests/unit/org-work.test.ts`, `tests/component/org-home-work.test.tsx`,
`tests/component/recent-visit-order.test.tsx`, `tests/e2e/org-home-work.spec.ts`; native API
`org_home_work` and `org_home_live`. They cover EN/ZH,
light/dark, 1440/400 widths, first-line alignment, real links/keyboard, live answers/renames/revokes,
scoping, slow/error/empty states and no follow-up reads. Written only: the integrator runs the round's
Rust/contracts/frontend gates and captures fresh screenshots/request counts; no writer ran them.
