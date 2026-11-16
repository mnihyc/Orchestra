# Shared conversation screen

Built: the screen of a thread (`/thread/$threadId`, an item named by `?item=`, a sub-agent's conversation by `?agent=`) in three
parts, of which only the middle one scrolls: the head (one row that never wraps), the conversation, and the dock (the queue,
the plan, the one status line and the composer). The head holds the title (a click renames it), the compact state, the
arrows and the list of Messages, how many agents, Changes with its line counts, Find, the way of reading and a menu that offers
everything else, and what the caller may do; what does not fit leaves the row, the last first, into the menu. A present description
and the lines that tell of the thread stand at the top of the conversation; adding a description is in the menu, alongside
running commands, side threads and who is looking. Binding, model, native approval mode and
context stand beside the message; the status bar reuses the existing runtime, usage and source readers.
View holds work detail, history visibility and automatic-title actions. Side thread uses
the verified side mode; when unavailable it keeps the reason and offers Fork to beside it. A waiting
question addresses its actual decider; other readers see Waiting for an answer. Header observes older
cards' existing approval cache without making another read or inferring authority from a role.
A blocked banner's action goes below its reason. Fork, Fork to, Side thread, Switch, Star, Notifications and Share keep their original
producers in the menu; Organize, View and placement submenus keep the rest of it in order without dropping acts. A person's message
is found by Ctrl+Alt+Up and Ctrl+Alt+Down, the arrows, the list of Messages and the sticky line under the head; a sub-agent's own
conversation is entered from the list of agents or from its step, read-only, and left by the title, Escape or Main.
An incomplete cached frame is contained in this view with an explicit Retry. A missing capability
never grants a fork, move, archive or export; the next complete live frame restores the conversation.
The first load uses the native thread opening, hydrating draft, parameters, limits, resolved settings,
outline and complete node caches before publishing the conversation. Later sequence repair stays on sync.
The thread supplies its own breadcrumb path; only visible Navigation follows the org tree and chats.
Status parts observe held panel data, and recorded file copies are read only after Restore opens.
The screen also holds its conversation and
its box, the banners that say the line is lost or the page may be out of date, why a thread is blocked
or held by a job. Somebody who may only watch keeps the same box and controls disabled, with the
model and effort visible and one compact missing-access line opening the shared request form. Related
modules supply sub-agent and side-thread lanes, the inspector, files, export, finding in the conversation
and the History view.

- `Thread.tsx`: The thread as a screen of the workbench: the conversation with its composer, and its other views in the arrangement chosen.
- `Header.tsx`: The one-row head: title, compact state, arrows and Messages, agents, Changes, Find, the way of reading and the menu with the rest; exported state/time/reason readers for the status bar.
- `rowFit.ts`: A row that never wraps: what does not fit is hidden, the last first, and comes back when there is room.
- `ChangesButton.tsx`: Changes in the head, with the lines the thread added and removed.
- `messages.ts`, `messageMoves.ts`, `MessageParts.tsx`: The people's messages, the moves between them (keys and arrows) and the sticky line and the list of Messages.
- `agents.ts`, `AgentsPanel.tsx`, `agentSwitch.ts`: The sub-agents of a thread, running and finished, the panel that lists them, and the way a lane opens one in place of the main conversation.
- `sentMessages.ts`, `SentMessageBlock.tsx`: A message on its way to an idle thread is the person's message at once, dimmed; the queue keeps the rest.
- `replyRoom.ts`, `FollowButton.tsx`: After Send the sent message holds the top of the view while the reply grows; the round button goes to the end.
- `WhereChip.tsx`: Where a thread works, "<environment> · <directory>", in one chip beside the composer controls; it opens the change of the environment and the directory.
- `openWhere.ts`: The one function that opens where a thread works, called by the chip, by `/directory` and by the act that a reason about the environment offers.
- `cached.ts`: Recognizable conversation frames and reported capabilities at a cached-view boundary; no permission inference.
- `Description.tsx`: Manual or automatic progress text with override controls.
- `BindingSummary.tsx`: Provider, model and effort, the thread's environment with its owner, and the arrangement's type, what the agent works with there and isolation facts.
- `UsageChip.tsx`: Best-effort plan windows, credits and age, and the tightest usage cap.
- `UsageView.tsx`: This thread's single ledger, permitted provider observations and applicable caps view.
- `turnLink.ts`: Numbered turn links accept only a positive safe ordinal supplied by the ledger.
- `BlockedBanner.tsx`: Actionable preflight failure reasons; for a limit that is reached, the limit prompt.
- `EnvironmentStartRecovery.tsx`: One failed-setup reason with native Retry and the ordinary environment change plan; keeps the first input, allows an alternative during a slow retry and fences old refusals by thread/environment identity. The timeline omits the repeated environment failure while this banner is shown. P1 component/browser and fake-route regressions are authored, unrun; the native file-job generation fix and proof belong to w-ev.
- `LockBanner.tsx`: The switch, move or update that holds the thread: its percentage, its phase and the way back to its wizard.
- `ConnectionBanner.tsx`: Connection freshness, a slow link said as such, and new-build notice.
- `SubscriptionBell.tsx`: Inherited or explicit notification level.

Org, department, project, folder and topic headers reuse that same place bell. Its native reply
supplies the strict ancestor default and recomputed personal exceptions page; no success refetch is
needed. The compact bell names its effective All/Important/Off level, uses the actual state glyph,
and keeps the served value while a write is pending or refused. Its checked menu row and inherited
Default remain available without a tiny state dot or an extra help block.
