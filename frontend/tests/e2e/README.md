# E2e validation

- `floating-layers.spec.ts`: Nineteen actual shared floating families at four corners, at 400/1440 px in both languages and themes. Long content must scroll, every surface stays within the viewport, and a tooltip leaves its trigger operable. Source-mounted fixtures use the development project; shipping screen layout uses built assets.

- `failed-environment-recovery.spec.ts`: P1 failed setup with its single first input, real Retry and the normal environment-focused reviewed switch, no input resubmission, preserved id/content and delivery from a native-shaped completion frame. Both languages, themes and widths with screenshot attachments. Uses existing fake controls and explicitly simulates completion; this is not a native file-job success proof. All unrun.

- `environment-deletion.spec.ts`: Create then confirm a managed-unit deletion, confirm an offline directory registration separately, verify kind-specific promises and absence from lists after reload, with full screenshots at 1440/400 in EN/ZH and both themes. The fake world completes removal immediately; real durable acceptance/retry/future-date/native retention behavior is covered by API hosted_environments and engine trace_retention sources. All unrun.

- `topic-agents.spec.ts`: Native Agents tab, unique usable profile links, actual default provider/agent/model, one panel read and no default writes/document-policy read. Defaults opens Settings, General remains only the section, and the topic breadcrumb returns to Overview; keyboard, geometry, accessibility and screenshots at 1440/400 in EN/ZH and both themes. Authored, unrun.

- `org-page-header.spec.ts`: Root/department/own-space headers at 1440/400 in EN/ZH and both themes: one icon star/main act/menu, ordinary aligned controls, no personal setup read/write or department action, native Settings navigation, actual department people and screenshots. First-steps browser cases retain team dismissal across windows/devices through the normal rail creation entry; uniform-places uses the native org menu for announcements and metadata. Authored, unrun.

- `place-collection-layout.spec.ts`: Project first-row visibility relative to the real scroll viewport at 1440/400, one header/New topic/filter bar, no inline provider/starter inventory, whole-page opening/idle/no-list-refetch assertions and keyboard creation. Root/department compact places follow their own bar while daily work retains its earlier priority; named provider defaults save through the existing settings row and survive reload. EN/ZH and both themes have screenshots and accessibility/first-line geometry assertions. Authored, unrun; the integrator reviews the images and request receipts.

- `resource-details.spec.ts`: All seven non-provider overviews at 1440/400, EN/ZH and light/dark: at most six directly visible tabs, one compact fact list, closed diagnostics, a two-screen height budget, initial API counts, no unopened secondary readers, full screenshots and a minute of idle traffic. One 200% CSS zoom case checks overflow and navigation; the integrator also walks real browser zoom. Authored, unrun.
- `resource-table-fit.spec.ts`: Actual Mounts/Providers lists with empty location explanations, long names/descriptions and a newly created provider unit. EN/ZH, actual light/dark choices, 1440/400 and 200% CSS zoom assert table/container bounds, an unbroken short mount name, full text on keyboard focus, readable host identity and native Open/Edit/menu reachability, with captures. Authored, unrun.

- `notification-exceptions.spec.ts`: Real bell → Settings row → Default at 1440/400, EN/ZH and both themes, including visible Off, actual state glyph/checkmark, concise preference labels, true ancestor defaults, one write/no success list read and screenshots. A real project default/thread exception workflow exercises both header entrances and inherited reset. Authored, unrun.

- `template-settings.spec.ts`: Short example-based creation, searchable shared setting rows, menu-only default/reset, source transitions and unchanged values, no search reads and Make here with Temporary in EN/ZH at 1440/400 and light/dark, with full screenshots. `template-newcomer.spec.ts`, `resources.spec.ts` and `unit-configuration.spec.ts` retain real CodeMirror, field refusal, stale-write review and literal command/program round trips through the new navigation. Authored against the fake world; integrator runs it.

- `security-sessions.spec.ts`: Screen-only bulk sign-out with cancellation, one act/answer, retained current session, keyboard and phone layouts in EN/ZH and light/dark; screenshot sources, unrun. Real-route and socket regression sources prove revocation boundaries.

- `visual-chrome.spec.ts` and `chrome-geometry.ts`: Sixteen actual routes, enrollment and finder dialogs at 1440/400 in EN/ZH × light/dark. Measure visible row/rail icons and stroke, quiet headings/contrast, flow gaps, control radii, text-field inner margins and actual icon/clear separation; retain keyboard editing and attach complete screenshots. Avatars, chart geometry, divider ink and semantic circles/pills keep their own shape. Authored, unrun.

- `recovery-codes.spec.ts`: Setup save-once sheet, lazy metadata, replacement invalidating old codes and keyboard recovery sign-in at 1440/400 in EN/ZH and both themes; screenshot sources, unrun. Native replay/concurrency guarantees have real API regression sources.

- `resource-tools.spec.ts`: Actual list Add MCP entry and real edit fields with Advanced-only JSON, a complete malformed-draft correction, actual argv definitions, retained stale-revision edits and accepted-response rendering; mount location and deletion confirmation workflows retain their existing proof.

- `topic-status.spec.ts`: Actual topic header at 1440/400 in EN/ZH and both themes, compact nonoverflowing state chip, visible keyboard focus, current radio choice, exactly one revisioned write per change and full screenshots for review. Authored, unrun.
- `table-content.spec.ts`: Actual Agents/Templates/Roles column ink, no-wrap/no-clipping and intrinsic width at 1440/1100/400 in EN/ZH × light/dark; complete image reference, keyboard scroller, no page overflow and full screenshots attached for integrator review. Authored source is not a passing visual gate.

- `people.spec.ts` and `profile.spec.ts`: Compact copy/privacy labels keep their real private/follows and activity/audience writes; native pin selection saves its exact identity, public/preview headings are distinct, unavailable private/missing/service pages share one heading and Home, and inspection guidance stays in the audience dropdown.

- `area-chrome.spec.ts`: Native area sections, retained targets and sheets below reachable chrome at 1100/400 in EN/ZH and both themes; all eight phone icons share one 44px line at 400, reveal their names on focus and keep one active view. A 360px dark case verifies the same fit without scrolling or More. Authored, unrun.
- `finder-loading.spec.ts`: At 1440/400 in EN/ZH and both themes, hold the actual bootstrap, open Find by Ctrl+K, get native Acme search results before releasing it, then retain the same focused input/query across full-frame mounting. One aggregate, no duplicated shell metadata reads and single toggles after readiness. Authored, unrun.
- `finder.spec.ts` and `craft.spec.ts`: Local commands remain usable after actual name-search refusals, a temporary failure can be retried, published guides open and a missing article address returns to the list; an English body keeps its translated catalogue identity. Authored, unrun.
- `workbench-navigation.spec.ts`: Third-pass real routes at 1440/400, EN/ZH and both themes: one current Usage section/access gate, actual enable flow, canonical Actions signs, topic own-link/breadcrumb exits and legacy Settings preference, root/department identifier settings redirects with query/hash and at most one native lookup. Positive/negative/covered DOM cases also exercise the navigation layout auditor; authored, unrun.
- `chat-layout.spec.ts`: At 1440 and 400 in EN/ZH and both themes, prove one scope/action row, pointer/keyboard place tooltips, retained scope focus and destination-specific composer placeholder; compare chat/thread controls, bounded/nonoverlapping buttons and conversation space, multiline, keyboard Send, reply/jump and compact edit/cancel. Screenshots attach to each run; authored assertions do not substitute for visual review.
- `thread.spec.ts`: Queue edits/removal while first and held, caret-on-edit and Ctrl+Enter through the real route, native delivery/Send now with explicit refused-steer fallback, retained steer/handoff controls, meaningful script previews and actual arrow icons, scoped Alt ordering alongside ordinary tree navigation, drag ordering, and a viewer with a watching card, neutral waiting language and no composer controls.
- `change.spec.ts`: Header and keyboard entry to an aside, and a real accepted-but-lost aside reply retried after navigation with the same payload, stable progress/focus, one resulting aside and newer composer words retained.

- `resources.spec.ts`: Enrollment choices produce exact commands for the three connector roles and the separate Node program; the fake arrival consumes the displayed token and the machine page shows the local roles command. Native consent/configuration is covered by the connector program test.

- `admin-marks.spec.ts`: Controlled typed projections exercise the real shared session/mirror controls, native-only environment and Everything marks, ordinary rows, authority rebase and visible owners at 400 px. Native authorization is verified separately by API tests.
- `admin-mode.spec.ts`: Private off/on/off, another session default off, retired switches, Management gate, audit stamp and keyboard focus behavior against the development world.

- `usage-performance.spec.ts`: Fake-world figures, missing values, chart measure, comparison sorting and shared filters.
- `usage-layout.spec.ts`: Four viewport widths and both themes compare ordinary Usage/Resources tab heights, visible navigation, retained dates and fitting tables; one-click place access with no audience sentence. Focused accessibility checks and screenshots remain unrun until the integrator's proof.
- `usage-requests.spec.ts`: Fake-world arrivals, pause/resume, programme filtering, turn links and retained view/span filters.

- `thread-controls.spec.ts`: The compact header and full message row at desktop and phone widths, an offered mode change and file/document references waiting for Send.

- `screen-review.spec.ts`: Screen review regressions: personal frames, contextual settings, topic wording, mobile paths and visible keyboard focus.

- `unit-configuration.spec.ts`: A template's literal commands and four program kinds survive a reload; machine programs stay read-only.

- `installation-reach.spec.ts`: Shared environment/inspector reach, phone permission loss and an explicit release lookup with a copyable command.

- `people.spec.ts`: Agent input review before an atomic visible send, no ask on edit, deliberate unread and current chat search scope.
- `bulk-share.spec.ts`: The ticked topic list opens common Share, tells a person once about both confirmed pages, and reports an unreadable page without stopping the others.
- `finder-targets.spec.ts`: Permission-row disclosure/focus return, a real person share opening Access, and setting-key routing/focus across same-screen navigation.
- `ffix-account.spec.ts`: Palette/reset in shared avatars, explicit null email proof and sign-in blockers, public-name field conflicts, 201/202 registration and returned node/guest-chat targets.

- `trace-corrections.spec.ts`: Recorded agent responsibility, request continuation and guarded mixed restores remain usable by keyboard on a phone. */

- `trace.spec.ts`: Person entries, scoped trails, filtering, keyboard details and complete downloads in the shared world. */

- `first-steps.spec.ts`: A person's first team/solo choice, scoped facts, portable landing/dismissal, normal guide and honest failure states. */
- `first-steps-projection.spec.ts`: Chosen-org projection checks against the same fake rows that ordinary screens read. */

A real browser against the built application (the development server with `ORCH_E2E_DEV=1`; the five tests tagged `@development` always run against it) with the fake API behind it: getting in (the first
administrator, a wrong and a right password, a second step, a code by mail, a locked name, signing
out), coming in for the first time (invitations, registration as the system allows it, confirming
an address, a new password), and the workbench at work (a tree that follows its org, a lost frame
made up for, views moved and kept, a small screen), the places of an org at work, and the thread
at work: a turn streamed from frames (text in pieces, a command and its output, a change and its
difference, an approval decided, the end with its usage), a message sent while the agent works
(queued, steered), Stop, a connection lost and found with a gap, a thread that is blocked and why,
somebody who may only watch, and the dialog that makes a thread from the providers, profiles,
models and environments of the one fake world. The same world has the pages of resources: a machine
added and seen connecting, a provider made on a key, an agent profile and an environment registered
on a machine, an environment made from a template and followed through its job, a template changed
while somebody else changes it, and a host closed and emptied. The same screens as a table and a page are
expected to be have a file of their own: the way to a resource in three parts and the way back to the list as it
was left, the row of kinds above every list, the act at the end of a row by pointer, by the menu button, by the
right mouse button and by the keyboard, rows ticked together with the acts they share, Edit on each kind (and a
form that says somebody changed it first), a hint beside every option that is not plain (a walk of every form
names the options that have none), and a machine enrolled again. People's chats are worked with two people in
two browsers at once: a message seen by the other as it arrives, who is typing, edits and deletes,
reactions, pins, replies, unread counts that fall as the chat is read, a direct chat and a group
made and left, a guest who sees that chat and nothing else, an admin reading what is not theirs,
a thread's item quoted into its chat, and messages picked and forwarded to a chat and a person. The finder at work: opened by its key from anywhere (and the keyboard given back), names found by a few letters and
opened by Enter, a folder with the way to it, a command run from it (and one the person may not use, listed with the
reason), the signs that keep it to one kind of answer, nothing found said, what a person may not see never listed, a
lookup that fails, the list of keys drawn from what is bound. The page of search with its filters, a result opened at the
hit and lit, more read as the reader comes to the end, and the bar that finds in a thread. Hints, the help view and its
guide, a tip closed for good. And folders, which are projects that
lie in projects: one made in a project and one in a folder, a tree three folders deep walked by
keyboard and remembered, a topic moved into a folder and out of it with its plan read first, a
folder moved into another, to a project and to an org and refused into itself, the depth refused in
words where one is made and where one is moved, what waits summed into a closed row, and who sees
"New folder". And documents: a page of every block type read and written, two people in one document and the change that came second,
comments, versions, moving, export, a reader who may not write, Markdown pasted and imported, every screen in both themes
with axe, and a page of two thousand blocks against measured budgets. The seventeen paired workflows arrive with their features.

The left bar at work: sections that fold and stay folded for the person on every device; a topic that
is done and keeps its place; a colour given to an icon; documents and chats in the tree as its reader
ticks them; what is unread from what was really read, said and never counted on a row that hides it,
with a number only for decisions that wait and for mentions; a tree narrowed to one place on one
device, its heading as the way back and its first row for a thing that lies outside it; branches
folded away by their reader, and the places that stay while the tree scrolls; the org's own rows at
the head of the bar, every org in the rail with what was shared after the last of them, and the
resources as a view of the bar that the rail brings to the front.

The settings at work: a person's own pages (the second step switched on by an authenticator app and
by a code in the mail, and asked for at the next sign-in; a token made, shown once and revoked; a
session ended; the password; the address waiting for its link; what the interface keeps), an org's at
every level (a member moved with what the dialog said first, held out and let back, a membership
ended with what is theirs listed, an invitation by the org's rules, tags, groups, service accounts),
the editor of the registry (a setting set, shown as coming from here, taken back and falling back, a
key locked above, a secret written and never shown), and channels and destinations (made from the
fields their kind declares, tried with the far side's own answer, switched off, removed), also for
somebody who may only read.

And what concerns the person themselves: a notification that arrives and the counters that move, read and unread, approvals of the three kinds decided from the list or by somebody else meanwhile, and the ways a person is told switched on.

Where the left bar meets the rest: the colour a place was given in the bar is the colour of its icon wherever it is drawn (its card, its own page, the finder, the dialog that exports it), an org has one colour in the rail, on its home and in its settings, and an org reached by its id is taken to its address with what the address said, the chat of an org having one name in the bar and in its panel.

The model scenarios cover the provider editor, groups and limits, and thread Speed, Access and Context choices, model changes, slash commands and recorded turn parameters.

Add checks with product behavior; do not assert file layout, import choices, component shape or
other implementation details.

The permission page: a person added to a line and taken off, a page that stops following its place and is put
back, a locked row, a change refused because the page moved (with the newer page shown), the full form's rows, what
an outsider is offered, and a refusal with what is missing and the form that asks for it.

Giving, with a second person in a browser of their own where it matters: a project shared with everything in it
(the person stands under People as a Viewer who can open what is inside) and alone (nothing inside is opened), a
reader who shares what they read and finds the rest disabled with the reason, a place that keeps sharing to its owners
(the field off, who can, a request asked and approved), a sharer who takes back what they gave and has no way to remove
what somebody else gave, a role's people from its chip, the documents of a topic shared with a project's agents, changed
to Manage with its sentence and taken back, a copy past the limit, a request approved with fewer permissions than
asked and too many waiting, Tell them and where it cannot, Export travelling with a choice, the form by keyboard alone,
Share in the menu of a resource, and the doors: a person named on a thread is let into its environment up to what the
choice gives, the environment's page with its five rows by area, and a door given alone.

Taking things away: a thread exported from its menu as the file it was asked for (how much, the form, the turns, what only a person's
export adds) and a chat with its attachments; a project exported with two of its three folders ticked and with documents alone, a document
unticked inside a ticked place, directly visible content/date options, a plan past the size a place allows (the button off, by how much, until less
is chosen), a place that allows none, nothing ticked, the line that says workspace files are not included and where its link goes, the
names of places read whole on a phone; a job followed in the status bar until it ends and its Download, one that fails
and says why, and a third archive at a time refused; a thread the person reads and may not export (Export… switched off in its menu
with one line and the way to ask, left out of a row's menu, greyed and counted in the dialog of its place); the list of one's exports
with a ready one, one being written, a failed one, a removed one, an expired one and one whose place is gone, the line of what each left
out, Remove, Cancel and Download again, and the list as cards on a phone; and the dialog of a place and of a thread worked by keyboard alone.

Starting work as one act: typing in a topic whose defaults are whole starts a thread with one Enter and the first answer comes into it
(nothing else is made); in a project the same Enter makes the topic, the environment it needs (named after the topic where the first name
is taken) and the thread, in that order, and the topic keeps its environment for the next; the dialog opened from a project; a default that is
missing, said and asked for one choice at a time and kept for the org; somebody who may not set it offered the request; a harness that has not
reported, waited for with nothing offered that cannot run, and the starter alive again when the report comes; the dialog with the arrangement the
server would take marked and ticked, the first row and another row sent as ticked, a way that cannot be had off with the server's reason and a
refusal that never puts another way in its place; a new environment made from the dialog and chosen, one that cannot be made, one that is
cancelled, and where none may be made; the starter and the dialog worked by keyboard alone.

- `trace-inspection.spec.ts`: Retained and purged content is inspected under its own restore guard without writes, subscriptions or live workspace work. */
- `trace-retention.spec.ts`: Kept files, actual stopped restoration jobs, permanent removal races and truthful failure/removal wording. */
- `ports.spec.ts`: Ports persona walks and current guards against the single component-double world; actual carrier proof is separate. */
- `craft.spec.ts`: The shared title bar, useful retained views, explicit act wording and canonical signs as a person uses them. */
- `craft-joined.spec.ts`: Later screens keep linked hierarchy, chosen disclosures and explicit canonical port decisions. */
- `admin.spec.ts`: Management at work: a person made and given a link, suspended and resumed, the last system admin kept, a root org's limit switched, the Overview with a part that stopped answering, a full host, and the refusal of somebody who is no system admin.
- `places-board.spec.ts`: Refused and failed status drags retain their card, expose the API's missing access and request ID through Details, and work again after recovery.
- `change.spec.ts`: Changing where and on what work runs: a fork, a switch, several switches, an environment updated, recreated and moved, what is idle recycled, side threads asked with their verified mode and kept, each with its plan, its blockers, its job and its end (`change-support.ts` holds what they share).
- `outside-order.spec.ts` — applicable Outside Orch lists at every place, delivery refusal beside the unchanged switch and its successful retry.
- `setting-own.spec.ts`: Blank own parts, repeated save/reset, linked locks and actual holders, and permission-loss draft retention.
- `project-views.spec.ts`: Personal views, explicit addresses, remote changes and view/whole-place stars.
- `topic-order.spec.ts`: One selection review and inverse, lost replies and separate tag creation/assignment outcomes.
- `place-review.spec.ts`: A phone review of work and subtree facts, partial refusal, lost-answer read, fresh review, separate move/delete fences and successful-row Undo.
- `bulk-share.spec.ts`: Selected holder-share previews, one grouped notice, skipped pages, a lost-answer read and successful-entry Undo.
- `installation-reach.spec.ts`: The two reach entrances, explicit release lookup and actual verified restore manifest, with unknown counts/copies and stale-read refusal.
- `provider-sources-native.spec.ts`: Chooses the installed Acme program on a real keeper, reads windows/balance/spend/reset/model offers and proves source secrets are not echoed. The integration runner supplies ORCH_NATIVE_URL and ORCH_PROVIDER_SOURCES_WALK (a JSON file with storageState, provider, programLabel); the keeper already reports the registered example descriptor and has a fresh example state directory. Registered in playwright.native.config.ts; no double or constructed report.

- `usage.spec.ts` and `usage-support.ts`: scoped figures, records, caps, rates and the single thread Usage view.
- `fnotify-away.spec.ts`: scoped delivery cards, pauses, current-device states and Inbox group races; a fake notifier's acceptance proves no real-device display.
- `start-help-switch.spec.ts` adds P2's unavailable-new-environment picker at 1440/400, EN/ZH and both themes: native reason, disabled profile/Create, retained first message, no creation/default write and screenshot sources. `ffix-start.spec.ts` keeps automatic environments inside the one thread action, verifies unused names and personal reuse without default changes; explicit Add environment remains its own act. Authored, unrun; native API proof owns actual transaction rollback and the new workspace's home.
