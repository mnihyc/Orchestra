# Topic: threads, environments and collaboration

Built: a topic's screen (`/topic/$topicId`) with its header (name, status, tags, star, bell, menu)
and its tabs: the overview and the threads, from what the org's live tree says of each thread; the
environments the topic names; its Documents, the list of the documents written beside it (`../documents/`);
its usable Agents and current thread defaults; and its Activity. Settings is the editor of the topic's settings and where it sends what happens (`../settings/`). "New thread" opens the dialog that makes one, from the defaults of the place; the
overview says where the topic lies, project or folder, with the folders on the way. The data of a thread
(its agent, its model, its last message) belongs to the thread screen.

- `Topic.tsx`: Topic header and tab selection.
- `Status.tsx`: One compact shared state chip opens the Open/Done menu; authorizes through the existing status mutation, keeps pending intent/refusals and the returned-revision Undo. Read-only and archived topics keep a plain chip.
- `Overview.tsx`: Live thread cards and environment/usage details, with one eligible inline starter or a single gated creation entry.
- `ThreadCard.tsx`: Live conversation summary and approvals; shows concrete observed work and failures without repeating a generic missing environment observation.
- `Threads.tsx`: All threads of the topic, with provider, model and effort, and environment.
- `Activity.tsx`: A topic's Activity: what its threads did, and the lines of its own history and of the management of its threads.
- `Agents.tsx`: One lazy native binding-options read lists usable profiles once across providers, marks the configured usable agent/provider defaults, and shows the default model/effort in readable words. Denied, archived, loading, failed, empty and incomplete-default states stay distinct. Defaults links to the existing scoped editor; viewing this panel neither reads document policy nor writes any default.
- `Environments.tsx`: The environments this topic's threads use: owner, place, threads bound, mounts, the default for new threads, and adding one.
- `Settings.tsx`: The Settings tab of a topic: its general settings, what new threads and new environments made for it start with, and where what happens in it is sent.
- `Documents.tsx`: The Documents tab of a topic: the documents written beside it, and with a switch those beside the places below it.

- `TagPicker.tsx`: Creates a tag only at a permitted defining place, applies it separately and preserves failed names or assignments.

- `statusUndo.ts`: Uses the returned topic revision for Mark done/Reopen inverses, with current rights and no work restarted.

- `TopicMenu.tsx`: One ordinary menu for a topic on its page, list and board; list/board status acts keep their captured revision and Undo, while the page changes status only at its chip.

Settings names the workbench page and tab; its heading names the current section. The authorized
breadcrumb ends at the topic once, and its link explicitly opens Overview rather than resuming the
Settings tab that was just left. Section navigation and persistence remain the shared settings frame's.
