# Shared conversation screen

Built: the screen of a thread (`/thread/$threadId`, an item named by `?item=`): its header (the title and
description changed in place, the compact state, the star, the bell, Share, and a menu that offers what
the caller may do). A present description takes one muted line below the title; adding one is in the menu,
alongside running commands, side threads and who is looking. Binding, model, native approval mode and
context stand beside the message; the status bar reuses the existing runtime, usage and source readers.
View holds work detail, history visibility and automatic-title/description actions. Ask on the side uses
the verified side mode; when unavailable it keeps the reason and offers Fork to beside it. A waiting
question addresses its actual decider; other readers see Waiting for an answer. Header observes older
cards' existing approval cache without making another read or inferring authority from a role.
On phones the complete title and state stand above one row of icon actions; a blocked banner's action
goes below its reason. Fork, Fork to, Ask on the side, Switch and Find keep their original producers;
Organize, View and placement submenus keep the remaining menu to ten entries without dropping acts.
An incomplete cached frame is contained in this view with an explicit Retry. A missing capability
never grants a fork, move, archive or export; the next complete live frame restores the conversation.
The first load uses the native thread opening, hydrating draft, parameters, limits, resolved settings,
outline and complete node caches before publishing the conversation. Later sequence repair stays on sync.
The thread supplies its own breadcrumb path; only visible Navigation follows the org tree and chats.
Status parts observe held panel data, and recorded file copies are read only after Restore opens.
The screen also holds its conversation and
its box, the banners that say the line is lost or the page may be out of date, why a thread is blocked
or held by a job, and, for somebody who may only watch, what they would need to work with it. Related
modules supply sub-agent and side-thread lanes, the inspector, files, export, finding in the conversation
and the History view.

- `Thread.tsx`: The thread as a screen of the workbench: the conversation with its composer, and its other views in the arrangement chosen.
- `Header.tsx`: Title and compact state, one Star, icon-only Fork/Fork to/Ask/Switch/Find in one row, and grouped remaining acts; exported state/time/reason readers for the status bar.
- `cached.ts`: Recognizable conversation frames and reported capabilities at a cached-view boundary; no permission inference.
- `Description.tsx`: Manual or automatic progress text with override controls.
- `BindingSummary.tsx`: Provider, model and effort, the thread's environment with its owner, and the arrangement's type, what the agent works with there and isolation facts.
- `UsageChip.tsx`: Best-effort plan windows, credits and age, and the tightest usage cap.
- `UsageView.tsx`: This thread's single ledger, permitted provider observations and applicable caps view.
- `turnLink.ts`: Numbered turn links accept only a positive safe ordinal supplied by the ledger.
- `BlockedBanner.tsx`: Actionable preflight failure reasons; for a limit that is reached, the limit prompt.
- `EnvironmentStartRecovery.tsx`: One failed-setup reason with native Retry and the ordinary environment change plan; keeps the first input, allows an alternative during a slow retry and fences old refusals by thread/environment identity. The timeline omits the repeated environment failure while this banner is shown. P1 component/browser and fake-route regressions are authored, unrun; the native file-job generation fix and proof belong to w-ev.
- `WatchingCard.tsx`: What the viewer lacks to work with this thread, with display names, who can grant access, one Request access action and Ask in chat; replaces the composer and its controls.
- `LockBanner.tsx`: The switch, move or update that holds the thread: its percentage, its phase and the way back to its wizard.
- `ConnectionBanner.tsx`: Connection freshness, a slow link said as such, and new-build notice.
- `SubscriptionBell.tsx`: Inherited or explicit notification level.

Org, department, project, folder and topic headers reuse that same place bell. Its native reply
supplies the strict ancestor default and recomputed personal exceptions page; no success refetch is
needed. The compact bell names its effective All/Important/Off level, uses the actual state glyph,
and keeps the served value while a write is pending or refused. Its checked menu row and inherited
Default remain available without a tiny state dot or an extra help block.
