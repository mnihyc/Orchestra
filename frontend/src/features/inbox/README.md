# Notification inbox

Rows about a thread lead with its current reader-safe name, then one localized event and the
nearest place. Names and redacted paths arrive in the existing HTTP page and live rows; the Inbox
does not fetch navigation trees to label them. The shared place matcher disambiguates peer names
and already cached places. A group omits its repeated place label and mobile heading; only the
individual actionable rows say that a decision waits. Authored/vendor detail stays where useful,
and names/details wrap rather than being hidden by truncation.

The actor is the current display name of a person already named in that notification or request.
An approval notice links to `/approvals?approval=<id>`, whose existing exact read opens that card even
when it lies beyond loaded list pages. Settling a request stamps its notice even if opening it already
marked it read; its original read time stays. Native approval news also carries the receiving browser
session's current summary when no notification row changed, so counters need no HTTP repair.

Settled request rows show their actual Allowed/Denied/Withdrawn/Expired outcome and current decider,
without retaining a prompt to decide. HTTP lists, place groups and the live feed enrich the same
already-authorized notice from one bounded approval batch; reading never rewrites history. A missing
or blank current thread name is Untitled thread, and a redacted name stays private. Successful turns
do not notify their own input author while that thread is visibly open. Leaving it restores normal
delivery; failures, other followers and team destinations keep their ordinary notifications.

Pause is one compact action in the Inbox's action bar. A status appears only for an active future
pause; its modal owns the one confirmed/current value while open. Resume retains its displayed
revision, and its existing act response updates the preference cache without refreshing a list.

Built: what the person was told, newest first and in their own days, with an icon and a name for each of the
25 kinds of notification, where it happened and when. A click opens what a row is about and reads it; its menu
(a right click opens the same list) reads one row, everything from one place or one chat. "Mark all read" counts
the rows the page has not loaded as well. The filters All, Unread and Mentions are kept in the address; older rows
come a page at a time, and the unread ones far down are asked for by themselves. A read mark is shown at once and
put back where the server refuses. Opening a thread reads what the person was told about it. The counters of what
is unread and what waits stand on the rail, in the left bar and in the bar of a small screen, and move as things
happen on any screen. The person's feed itself is in `src/realtime/stores/user.ts` and `src/realtime/live.ts`. The feed is
subscribed once its first load has succeeded: against a server that has no notification requests (an older build) the
counters stay hidden, the Inbox says it could not load, and the approvals list is not told when something new waits.

- `Inbox.tsx`: Day grouping, unread and mention filters.
- `NotificationRow.tsx`: Context, read state and target link.
- `kinds.ts`: What each kind of notification looks like in the Inbox: its icon, the tone of the icon and its name.
- `model.ts`: What the Inbox shows of the rows it holds: the three filters, the days the rows fall on, and the place a row is about.
- `read.ts`: Marking the rows of the Inbox read: one, those of a place or a chat, or all.
- `text.ts`: Notification action text in the reader's language, retaining authored words and legacy fallbacks.
- `Counters.tsx`: What is unread and what waits, as the rail, the left bar and the bottom bar of a small screen show it.
- `away/`: Supplied readable-place groups, stable group reads and Undo receipts, safe route notices and explicit counted catch-up; its README inventories the controls.
