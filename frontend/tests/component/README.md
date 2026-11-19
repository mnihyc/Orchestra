# Component validation

- `access-error-recovery.test.tsx`: Actual preview/page errors (including 404) have Retry and keep confirmation blocked until review; a refused Copy keeps its mode and retries the frozen copy body, with native error words in EN/ZH and both themes. Authored, unrun.
- `access-view-as.test.tsx`: Same selected-person read for thirteen permission-page kinds, actual error/retry, native owner/admin/system provenance and one full resource-page entry in EN/ZH and both themes. Native membership/manager/privacy rules remain server-tested; source unrun.

- `access-history.test.tsx`: The real disclosure/query/history reader for all eight resource kinds, lazy loading, exact plural paths, pagination and read-failure recovery in EN/ZH and both themes; source only, unrun. `resource-technical-values.test.tsx` also keeps provider-unit deletion offered, confirms the human name while sending the exact unit, and preserves the optional volume choice and accepted-deletion wording.

- `change-plan-failure.test.tsx`: Distinguishes a missing thread/saved plan from a server failure, keeps the factual cause in EN/ZH, and invokes Make the plan again without describing Orch as unfinished.

- `request-errors.test.tsx`: Access, export and defaults requests retain native 404/500 errors through ErrorAlert, reason drafts, exact request bodies and retry with or without a named owner; real pending-request counts remain factual. EN/ZH sources, unrun.

- `browser-discovery.test.tsx` and `browser-discovery-fixtures.ts`: Real EnvironmentScreen/BrowsersPanel/API reader across missing support/programs, explicit Retry/Look again, cold partial feed before HTTP, retained warm metadata, independent picture failure, reachable Tools navigation and recovery in EN/ZH x themes. Twenty authored cases, unrun; the paired five `unit/browser-programs-wire` cases cover native cold/warm browser/display projections. Existing browser-view watcher/driver/socket assertions remain.

- `start-submit.test.tsx` and `start-flow.test.tsx`: P2 atomic thread/environment/project-topic admission; the exact named setup is frozen, impossible current choices stay disabled with their native reason, failed defaults are never selected or marked, the draft survives refusal and no separate environment/topic/default write is sent. EN/ZH; source only, unrun. Native rollback/initial-template/live-empty-report cases are in API `hosted_environments.rs`; `start-arrangement.test.tsx` retains the reason when its profile cannot run.

- `recent-visit-order.test.tsx`: Read-carried visits keep per-person local order, snapshot cutoffs and one local write per navigation without either explicit visit POST. Canonical thread targets behave the same; documents never mark their containing thread as opened. Assertions cover the authoritative read bookkeeping; unrun.

- `failed-environment-recovery.test.tsx` and `failed-start-fixtures.ts`: P1's actual blocked/source/queue/timeline-status surfaces with full fake-world contracts, one plain EN/ZH failure, restored native progress, exact original first-input identity/content, native Retry target, unchanged input/default path, usable elsewhere during a held retry, scoped late refusals and independent authority. EN/ZH x themes; source only, unrun. Existing start-recovery assertions retain their authority/held-message coverage with the more specific file-stage wording.

- `environment-removal.test.tsx`: EN/ZH x themes, unit versus directory confirmation from lifecycle facts even with a machine reference, zero-window temporary warning, unknown-fact refusal and stable lost-answer identity. `environment-settings-once.test.tsx` renders the actual environment entry and proves each reported duplicate key/source appears once. `template-guide-entry.test.tsx` keeps the real template-page guide entry once and opens it without losing the draft. Source only, unrun.

- `ports-discovery.test.tsx`: Actual EnvironmentScreen, PortsPanel, native feed projection and cache across missing/late discovery, retained complete answers, explicit Refresh and offline refusal; other environment tabs stay usable in EN/ZH and both themes. No mocked Ports query/converter or polling. Authored, unrun.
- `admin-user-locks.test.tsx`: Actual Users rows name all four served sign-in proofs, including recovery codes, once with the lock time and one recovery explanation in EN/ZH and both themes. Authored, unrun.

- `access-people.test.tsx`: Share focus/typing/clearing and lazy candidate reads, native owner attribution inside Everyone, primary/group owners in Roles without editable fabricated roles; EN/ZH, both themes. Source unrun.
- `access-request-words.test.tsx`: Exact translated chat/document/thread names on the real inbox/page access cards and waiting line, independent checkbox choices and unchanged permission keys in the request body; EN/ZH, both themes. Source unrun.

- `topic-agents.test.tsx`: Actual Topic tab mounts one native options reader only when shown, lists usable profiles once and their current defaults, excludes unusable/disabled/unreported placements, and separates pending/failure/retry/empty/denied/archive states. Settings uses its real frame/section heading and breadcrumb, with one Settings page identity and an explicit return to topic Overview. EN/ZH and both themes; source unrun.

- `org-page-header.test.tsx`: Native root/department/personal page menus with exactly three header controls, one real star write, guarded New project, Settings in page/row menus, no personal New department and lazy metadata/announcement/notification bodies in EN/ZH and both themes. Department people use native inherited counts; missing counts never become zero. Sources unrun.

- `place-collection-rows.test.tsx`: Full native project/department names, descriptions/tags only on request, readable owner guidance, no displayed routing address and no folder-detail opening request in EN/ZH and both themes. Authored, unrun.
- `project-page-selection.test.tsx`: Table-header page checkbox with mixed/select/clear states, one frozen status review, captured revisions, one Close in review/results and results/Undo retained after clearing the selection in EN/ZH and both themes. Source unrun.
- `project-provider-default.test.tsx`: Actual existing settings row, named usable choices, ancestor locks, exact project revisions, refusal/retry/Undo, empty-list creation, duplicate-name origins and no raw/illustrative UUID display or read in EN/ZH and both themes. Authored, unrun.

- `notification-bells.test.tsx`: Actual Off/Ring glyphs, visible Off and accessible inherited levels, full checked-row marks, refused/pending changes preserving newer thread facts, cross-bell write serialization, chat defaults and document choices in EN/ZH and both themes. Authored, unrun.
- `notification-exceptions.test.tsx`: Native reply rows, ancestor changes, Default/Undo/refusal, shortest canonical qualifiers, last-page recovery and the existing finder for an empty list, without success refetches, in EN/ZH and both themes. Authored, unrun.

- `template-settings.test.tsx`, `template-schema-form.test.tsx` and `new-template.test.tsx`: Actual shared rows, EN/ZH and both themes, localized search, truthful sources and gear-only defaults/reset, sparse/unknown-property preservation, declared complex defaults, unfinished-draft recovery and JSON round trips. Short creation keeps the complete example, places server field faults and opens the returned page without preview traffic per keystroke. `resource-setting-drafts.test.tsx` retains the competing-write revision/review assertions. Sources authored, unrun.

- `provider-chat-format.test.tsx`: The compact endpoint control names available compatible agents, changes that statement with its format, and explains Codex's Responses requirement once in both languages/themes. `provider-source-create.test.tsx` asserts the same copy in Add provider. Authored, unrun.
- `security-sessions.test.tsx`: One confirmed bulk sign-out, cancellation/refusal/retry, retained current browser and exact response projection without another list read, in both languages. Authored, unrun.

- `visual-chrome.test.tsx`: Eight EN/ZH × light/dark cases through the actual Section/TreeFilter: each name once, keyboard fold/reopen, typing/clear/Escape/arrow behavior and SVG size/stroke. Geometry is measured in the browser suite, not inferred from jsdom. Authored, unrun.

- `recovery-codes.test.tsx`: Recovery sign-in/deep-link and session verification, save-once activation cleared on close, cache redaction, fresh/expired/passwordless replacement and copy-all; both sign-in languages. Authored, unrun.

- `mcp-form.test.tsx`: Name/URL/bearer and exact command/argv/secret fields without JSON; actual lazy Add entry, intent-only Advanced mounting, malformed-draft recovery, literal multiline script arguments, write-only credentials, refusals/retry identity and imported read-only definitions in EN/ZH and both themes. Native blocked controls are focusable and cannot submit.

- `state-line.test.tsx`: Shared compact empty/error renderer, one keyboard action, no duplicate copy or raw protocol/request data, unknown-code fallback, preserved native reasons and authorized recovery focus in EN/ZH and both themes.

- `topic-status.test.tsx`: Shared topic status chip/menu in EN/ZH and both themes; keyboard/current choice, no-op selection, pending intent, refusal/retry, captured revision and Undo, read-only/archived gates and a concurrent operation.
- `table-content.test.tsx`: Full values once, responsive summaries, selection and keyboard acts through the actual Table in EN/ZH × light/dark; geometry belongs to the browser case, not jsdom measurements.
- `table-values-once.test.tsx`: Actual capability/rate tables keep display names once, omit row keys/routing slugs and repeated help, preserve the capability key in its editor and offer its existing Environment scope in EN/ZH × light/dark.

- `creation-once.test.tsx`: Single create entry in resource frames, the topic starter, each project view's header, empty messages/documents and personal orgs, with gated and fallback entries, real naming actions, on-demand metadata and compact root/department places order in both languages and themes.
- `approval-question-once.test.tsx`: One visible approval subject with accessible answer labels, distinct titles preserved, exact answer submission and elicitation fields in EN/ZH and light/dark.
- `question-item.test.tsx`: Native question items render from their payload and settle from the decision response; matching timeline subjects render once while fields remain named, with viewer/decider gates in both languages and themes.
- `audit-heading-once.test.tsx`: Management supplies the sole audit heading while a standalone scoped trail keeps its own, in both languages and themes.
- `source-observations.test.tsx`: Detailed unknown observations stay explicit; cards show only their concrete Git work/failures, with one shared stale label in EN/ZH and both themes.

- `page-structure.test.tsx`: Whole-page empty states have an h1, approval cards follow page/section heading levels, and live profile previews use unique EN/ZH landmarks beside the real editor.
- `profile-editing.test.tsx`: Accepted profile/pin answers, serialized saves and retained refused drafts, debounced current-name choices, common qualifiers, explicit lookup retry and private-safe unavailable-page headings/home actions.
- `chat-export-menu.test.tsx`: Native chat export authority, the real export/request targets and no place metadata read when opening a chat or its menu.

- `chat-scope.test.tsx`: EN/ZH scope names stay short through click and arrows; full places appear on pointer/keyboard tooltips without description rows or per-option guides.
- `input-queue.test.tsx`: First and held inputs, preserving parts and refused drafts, delivery and explicit interrupt fallback, shared write pending state, drag and arrow ordering, focused-row shortcuts against the real workbench matcher, caret-on-edit/Ctrl+Enter/IME, meaningful previews, actual arrow icons, retained steering/handoff controls and viewer limits.
- `side-ask.test.tsx`: Every verified side mode, retained unsupported/Escape drafts, frozen unconfirmed retry identity, accepted-only clearing, no closed/duplicate reads and no stale private entrance name.
- `side-attempts.test.tsx`: Unconfirmed side-act identity across unmounts, concurrent-consumer coalescing, accepted-only draft clearing after navigation, fresh server restoration without resurrecting an intentionally emptied draft or removed files, newer words/attachments retained and isolation from a later signed-in lifetime.

- `visible-navigation.test.tsx`: All page/group tabs and native links stay directly visible; pointer, Home/End/arrows, disabled choices and native Enter behavior. Both languages also verify the compact all-tabs list's names, absent descriptions/unavailable Close, selection and focus return under the current frame contract.
- `visible-options.test.tsx`: Export content/date controls and exact audit actions are visible immediately in EN/ZH; translated audit names retain the exact protocol change value and other filters.
- `workbench-targets.test.tsx`: Compact browser/display labels include live state in EN/ZH, carry the selected identity to the viewer, omit inline captions/guides and perform no target reads/subscriptions without permission. Named target lists are independent of the shared toast viewport.
- `workbench-visibility.test.tsx`: Unseen containers/sections/tabs/panels/sheets mount no reader; hidden used views drop subscriptions and retain their drafts; same-commit moves retain the subscription.
- `shell-bootstrap.test.tsx`, `navigation-data.test.tsx`: One aggregate primes all shell readers while keeping the Inbox page unloaded; avatars request no org chats, and opening an org loads its chats once.
- `screen-loading.test.tsx`, `frame-keyboard.test.tsx`: The actual root keeps frame state around a pending screen, a first skip link reaches main, and area travel excludes covered/hidden views, in both languages.
- `finder-loading.test.tsx`: The native root shell controls open/focus/toggle Find before admission and while the one bootstrap is held, in EN/ZH and both themes under StrictMode. Normal intent searches add no shell metadata read; cached custom keys, IME/session guards and listener cleanup retain their native contracts. Authored, unrun.
- `finder-errors.test.tsx`: Native name-search 404 and temporary errors keep their actual cause and local commands; explicit Retry repairs only active names and leaves background entries unread. Both languages/themes, actual LiveQueryClient, authored/unrun.
- `help-available.test.tsx`: Real router detail-screen words, readable-only article choices, article content, draft/missing-address recovery and retained loading in both languages/themes. Authored, unrun.
- `node-screen-actions.test.tsx`: Org screen acts are direct buttons/native links with retained permission refusals, in both themes/languages.
- `terminal-frame.test.tsx`, `terminal-recording.test.tsx`: One travelling creation action and scope question, exclusive empty/refused/failed states, retained read refusals/retry, expandable complete output and a terminal-code failure recovery path, in both languages/themes.
- `resource-landing.test.tsx`: All eight native counts/Add entrances, one shared summary request and only the expanded resource name page.
- `team-chat-shortcut.test.tsx`: The current page/draft remains in place and the single chat receives focus when ready, in both languages.
- `chat-links.test.tsx`: A cold chat address is revealed by the frame before its unseen body mounts, using current metadata and the same singleton path.
- `admin-exit.test.tsx`: A confirmed native Off answer or reconciled mode change leaves a private page through its nearest readable parent; ordinary View stays, the frame remains and a late answer cannot redirect another page.
- `area-navigation.test.tsx`: Every settings scope, Help and Usage show their native section links without remembered-org tree/chat readers; one current item, explicit topic Settings and plain exits, native aliases/exact-key links, retained fallback drafts and Usage scope/dates/filters in both languages/themes. Authored, unrun.
- `navigation-current.test.tsx`: Real router/search behavior marks exactly one Usage view, retains scope/dates/filters, marks Administrator access while Off and the actual section while On, and composes one real ellipsis menu across left-area titles in both languages/themes. Cache-only mode and native pointer/keyboard actions; authored, unrun.
- `topic-view.test.tsx`: Legacy saved Settings cannot trap a topic; explicit Settings leaves its content preference intact and an Overview breadcrumb wins. The actual route hook is tested without a synthetic replacement; authored, unrun.
- `workbench-chrome.test.tsx`: One native/layout title menu, a protected/current tab's available acts, and the real focused Views chooser's title, unique names, trailing groups, search and reveal in both languages/themes. Authored, unrun.
- `narrow-container-names.test.tsx`: Medium names stay visible; all eight phone icons retain accessible names, focus tooltips, one active view, arrows/Home/End/Enter and toggle behavior. A held touch reads without opening; moving, cancelling or unmounting cancels its timer, and quick taps still open. Both languages/themes; one-row geometry at 400/360 is covered by browser `area-chrome`. Authored, unrun.
- `help-sidebar-state.test.tsx`: Actual visible-slot ownership restores Help's tabs when the sidebar closes and retains the same active draft through both transitions, in both languages/themes. Authored, unrun.

- `time-zone-picker.test.tsx`: Ordered device/current/UTC choices, region groups, current local times, exact signed offsets, city/abbreviation search, Chinese names/regions, keyboard/Escape and profile writes with immediate device formatting and refused-choice recovery.

- `mount-location-status.test.tsx`: Configured/hidden/offline location reasons, readable host links, machine-scoped empty words and unknown-versus-zero in EN/ZH; common managed-machine add choices follow served capabilities, including directory machines and missing owned-data support.

- `machine-enrollment.test.tsx`: Four named roles, per-role handover, exact platform commands, separate Node enrollment, invalid/late-token hiding and EN/ZH descriptions; HTTP and home lists are doubles, native enrollment stays a separate Rust proof.

- `admin-mark.test.tsx`: Native-only resource rows, real tree shields, private page headers, ordinary focus, person/place/system owners and existing admin-act tooltips/confirmations.
- `admin-mode.test.tsx`: Available-only session control, shared title-bar/Resources mirror, Management gate, refused writes, retired self-grants, historical audit stamps and returned chats.

- `usage-performance.test.tsx`: Median units, previous-span changes, missing-measurement reasons, comparison sort and row filters.
- `usage-performance-places.test.tsx`: Twenty-request picker floor and degraded-only provider status navigation.
- `usage-performance-frame.test.tsx`: Performance figures and the accessible three-measure switch.
- `usage-layout.test.tsx`: Short shared tabs and keyboard navigation across all three views, visible-sidebar ownership and fallback, retained span, one guide, and named ordinary filter removal from one served history read with a native numbered link and distinct routing/display values.
- `usage-period.test.tsx`, `usage-scope.test.tsx`: UTC labelled once, shared custom dates, calendar-week selection and authorized scope discovery without eager org trees, in EN/ZH and both themes.
- `usage-records.test.tsx`: Short thread-first numbered links, place context once and human model display names.
- `acts.test.tsx`: Registry coverage and native answer updates to the visible detail and list without an HTTP refresh.
- `usage-visibility.test.tsx`: The redundant audience sentence/read is absent; one Access button opens the existing place permission page.
- `usage-requests.test.tsx`: Pause/resume, responsive nullable row values, named filters, numbered turn links and access closure.

- `choice.test.tsx`: Familiar names, separate accessible descriptions, radio/segment/listbox keys, checkbox labels, blocked menu reasons and guides that never choose a value; includes context menus and legacy checkbox form names.

- `provider-home.test.tsx`: Home-only choice on unit and directory runtime facts, revision-guarded writes and a reader's disabled borrowed path in EN/ZH.

- `thread-header.test.tsx`: One Star and each primary action, unsupported aside with Fork to, decider-specific waiting language and grouped View settings without duplicated menu actions.
- `composer-controls.test.tsx`: Ordered message controls, the agent's binding entrance and the viewer's read-only row with its reason.
- `composer-delivery.test.tsx`: Idle Send, running Stop with an empty draft, explicit queue/steer/interruption and unavailable delivery reasons.
- `composer-approval-mode.test.tsx`: Only offered native modes, displayed revisions, locked/viewer choices and competing writes without a silent retry.
- `composer-references.test.tsx`: Plus, permission-checked file names and document copies added to the draft without a message send.

- `auth-order.test.tsx`: Password sign-in comes before provider choices in the page and the keyboard, including a return to an invitation.

- `screen-labels.test.tsx`: Management roots, finder acts and personal frames keep the same places and destinations as their screens.

- `watching-card.test.tsx`: A viewer gets one access action, real model/manager names, no composer controls or retry for an expected refusal; granted access still refreshes the thread.
- `trace-screens.test.tsx`: Real inspection envelopes and served host facts remain readable without invented requests or acts.

- `resource-sections.test.tsx`: All section choices by keyboard, lazy technical content, paused hidden readers, retained/paged drafts, blocked invalid navigation and one native action entrance in both themes/languages.
- `resource-detail-fixtures.ts`: Typed seven-resource projections and a long extension schema, for detail composition rather than native operation proof.
- `resource-detail-pages.test.tsx`: All seven actual page compositions, at most six visible tabs, one compact fact list, lazy readers, legacy destinations, parameter links and host guarded Save/Undo/read-only/conflict behavior.
- `host-settings-entry.test.tsx`: Native header/list Edit share Host Settings; Rename is name-only. Accepted Save/Saved/Undo authority, draft receipt clearing, read-only gates and newer-cache protection in EN/ZH and both themes.
- `resource-status-agreement.test.tsx`: Actual Provider Activity observes the same native session queries as its rows, reports failures/recovery without a second read, retains model readiness and authorization, and names a unit's actual host in EN/ZH and both themes.
- `resource-setting-choices.test.tsx`: Environment/provider Template and Host settings select authorized names through native guarded rows, retain hidden/read-only references without ids, share one choice read and send automatic placement as null, in EN/ZH and both themes.
- `harness-profiles.test.tsx`: One actual named profile row across accumulated machine reports, no duplicated session counts, distinct same-kind profiles, on-demand evidence and a profile-only restart in EN/ZH and both themes.
- `resource-technical-values.test.tsx`: Short paths and program facts with exact focus/copy values, runtime fingerprints retained behind Details on both pages and plans, human runtime names and exact guarded deletion requests in EN/ZH and both themes.
- `sandbox-readiness.test.tsx`: Native boundary/authorized recovery explanations plus compact per-environment deduplication, stable dismissal and changed-failure recovery; the real Machine cross-tab/theme composition is covered in `resource-detail-pages.test.tsx`.
- `resource-template-sections.test.tsx`: Bounded schema sections/search preserve edits and unknown members; complete JSON is opt-in and invalid auxiliary drafts prevent compact navigation.
- `resource-environment-settings.test.tsx`: Actual registry rows appear once, keep revisioned saves and accepted answers, page/search long sections and load command templates only on intent or an exact link.
- `resource-names.test.tsx`: MCP/Mounts single primary add action, served inventory filters reached by keyboard, named empty/search states and concealment of retired hidden references in EN/ZH.
- Resource state coverage also extends `resource-detail-pages.test.tsx` (busy create/recreate/move versus genuine completed updates, one accepted deletion state), `host-settings-entry.test.tsx` (one admitted host observation joins both loaded list shapes without reads), and `temporary-environments.test.tsx` (idle deadline versus warning and one irreversible-removal boundary).
- `mounts-here.test.tsx`: Machine-scoped mount sections, storage/sizes, visibility, add/remove confirmation and failure/draft identity in EN/ZH.
- `pki.test.tsx`: Expiring machine names, certificate dates and links, the full count beside fifty rows, the empty state, and EN/ZH wording.

- `unit-configuration.test.tsx`: Both pools show applicable origins and timing, with owner-guarded literal commands and recoverable failures.
- `template-unit-fields.test.tsx`: Template forms preserve the recipe while editing named literal commands and all four registered program kinds.
- `runtime-configuration.test.tsx`: Runtime secrets remain write-only beside the shared configuration, and a personal provider has no unit editor.

- `setting-conflict.test.tsx`: A refused model choice names its changer, keeps the current model selected and retries only on a new choice with its revision.

- `agent-reach.test.tsx`: Permission cuts, per-thread key placements, empty and lost-read states, registry-driven updates of an open block, and compact native placement rows with one on-demand explanation in both languages/themes.
- `mcp-servers.test.tsx`: Native per-server states and guarded Restart, one on-demand panel explanation in both languages/themes, and failed-read replacement of stale names with Retry.
- `installation-records.test.tsx`: Explicit release checks, dated successful facts, verified backup completion, later failures and unknown coverage.
- `finder-tag-projects.test.tsx`: Distinguishing project labels, readable-only scope, keyboard selection of the canonical tag filter and failed-read retry.

- `people.test.tsx`: Current reader choices and caret restoration, and refused discussion-level changes.
- `chat.test.tsx`: Named keyboard access to compact input actions, a short visible placeholder with the full target retained in its accessible name, multiline drafts, meaningful draft live status without empty file lists, independent failed-send controls, alongside IME, upload, reply and edit behavior.

- `holder-picker.test.tsx`: Enter selects the uniquely resolved holder with its whole place, loading cannot select, and an unknown locator says nothing is called that.

- `first-steps.test.tsx`: Facts stay distinct from actions, solo omits inviting, roles describe current defaults, and guides are ordinary links. Unavailable setup stays distinct from a refusal, with no protocol identifier or raw route detail on the screen; round-41 source repair is unrun.

Visible behavior of what is drawn: so far a form that says what is wrong when a field is left and
when it is sent, and puts the keyboard on the field to mend; and what a person who may not do
something is shown (the act left out, switched off with its reason and still within reach, or one
quiet line) and what a refusal says is missing and whom to ask; a hint beside a term with one, two and all four of its
parts, a link inside an article that stays in the help or opens a screen, and the palette of the finder as a combobox over
options; a tree of hundreds of rows, in which a row whose thing did not change is not drawn again and opening a row draws the
rows it adds and not the rows after them; and the list a conversation is drawn in, which holds its end while rows are measured
however slowly the frames of a machine come. The box a message is written in is held to its keys (Enter sends, Shift+Enter makes
a line, an input method that is composing never sends, an `@` names the people of the chat) and a message to what it shows and
offers its reader by whose it is and what the chat allows. What a change to a page is said to reach before it is confirmed is held here too: the pages that follow the rows, the orgs where the server counts them, and the people who gain or lose with their names behind the count. Delivery and failure states of other features arrive with them.

Add checks with product behavior; do not assert file layout, import choices, component shape or
other implementation details.

A tree of ticks (a box on a row, ticked, empty or half ticked, pressed or spaced to change, one that cannot be changed), and the
small dialogs that give a thread, a chat and a document as one file: what each asks the server for, a number that is no turn
refused where it is typed, what a format could not say told once the file is saved, and what a person who may not export is told.

The step of the dialog that starts a thread where an agent is placed: the ways listed in the order the server ranked them with the one it would
take marked and ticked, another row told to the dialog when it is chosen, a way that cannot be had off with the server's sentence and out of reach,
none to choose among where none can be had, and nothing offered while the harness of the profile has not reported.

The pieces of the left bar: a section that is folded by its heading, keeps its rows out of reach once folded and says in its
heading only what waits inside (a dot, or the number of mentions in its place, and the number of decisions); the icon of a
place in the colour of its kind or the one it was given, named only where it stands alone; and the field that narrows a tree.

- `trace.test.tsx`: A person's authorized card entry and unknown retained size are readable without turning states into buttons. */
- `ports.test.tsx`: The Ports states stay distinct and selecting or refreshing a listener grants no access. */
- `ports-fixtures.ts`: Small typed Ports projections for component behavior; no request or socket success is simulated here. */

- `ports-details.test.tsx`: Ports details prove recipient choices, ready carrier facts and bounded live caller disclosure. */
- `craft-rules.test.tsx`: Act wording and signs are shared, and the title bar's visible and folded places are links. */
- `topic-board.test.tsx`: A status drag keeps a visible destination still even when more content follows the board; a failed change keeps the card in place and offers the server's access and request details. */
- `change.test.tsx`: The steps of the change wizard and the side threads' lane as a person works them, with what each says before anything is done.
- `setting-own.test.tsx`: Save, Reset and Lock submit only the local part; a refused save preserves its draft.

- `usage.test.tsx`, `usage-visibility.test.tsx`: full limit prompt and the current permission-page entry for usage visibility.
- `provider-plan.test.tsx`, `provider-reset.test.tsx`, `route-identity.test.tsx`: native unknowns/overage/privacy, guarded reset identity and fresh screens when route parameters change.
- `fnotify-away.test.tsx`, `fnotify-reads.test.tsx`: revisioned pause confirmation and retained group read/Undo receipts after uncertain replies.
- `place-review.test.tsx`: Selected archive/unarchive, move and delete plans, work and blockers, sequential partial progress, observed lost answers and confirmed-receipt Undo.
- `share-review.test.tsx`: Frozen sharing revisions, policy and before/after entries, partial/uncertain results and inverses from accepted entries rather than trial IDs.
- `backup-restore-list.test.tsx`: Actual protected manifest identity, unknown versus zero counts, recorded locations versus uninspected copies and failed-verification hiding.
- `provider-details.test.tsx`: Every member in four states for either credential kind, exact decimal and zero compact fallback, and independent model-source states.
- `provider-sources.test.tsx`: Registered choices, HTTP refusal, write-only saved authorization, explicit address-bound key consent, revisioned saves and retained refused/stale drafts.
- `provider-sources-host.test.tsx`: Actual provider-context query/write joins, keeper-only offers, learned template descriptors, local policy warnings and fresh drafts across provider routes.
- `provider-source-create.test.tsx`: Collapsed default source choices, template programs written with their runtime template and programs scoped to the selected keeping machine.
- `environment-data.test.tsx`: Independent workspace/home choices, driver refusals and the shared machine roots/uses section in English and Chinese.
