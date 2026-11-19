# The conversation: the agent's work step by step, side threads, selection and history

Built: the conversation of a thread as a list that draws what is in view, so that five thousand items open
at their end and scroll as ten do. Every kind of item has its row; the agent's work is shown step by step or
folded to one line for each turn, by the depth the reader chose (Steps, Summary or Everything). Every
tool begins as one compact row and expands in place on click, including the most recent edit and a
failed or running command. An opened command shows its output as it prints, an edit its difference, a reasoning its
summary; a turn that is over ends in its footer; a "New" line stands before what the reader had not seen
when they opened the thread; the line under the last item says what the thread is doing. Older items are
read as the reader comes to the top, a link to an item opens the conversation around it, and a reader who
is not at the end is told how many rows came below. A sub-agent starts folded to its last reported line,
state mark and duration, then opens at its originating tool row into a bordered child lane. Its row owns
its name and clock; the lane adds its reported role and compact shared item displays. Raw and Open in
panel are icon acts on the opening row, beside its state; durations below one second are omitted.
A single-file edit names its path, kind and line totals once, with file-open on that same row; multiple
files and approvals retain their per-file headings. The running header chips
open that turn and lane; Open in panel shows the same stream in the Conversation container. Earlier child
activity uses the existing stream-filtered item endpoint, without changing the main stream's paging or
read marker. Initial sync preserves a bounded main-stream window alongside recent child activity, and
closed lanes read no history. A lane and its panel share an in-flight child-history read; closing the
last reader cancels it, while a panel that remains open keeps its read. Sub-agents have no composer.
Side threads use the same lane frame and item
displays under their recorded source turn, with their own Enter-to-send composer, queue and Stop.
Moving an aside into its panel preserves that conversation and draft. Closed asides fold to their recorded
turn count; a source outside a partial history window is named until its earlier turn is read. Folded,
hidden and retained side views mount no child reader. The open-side list consumes parent markers and one
explicit list read, and an accepted aside enters the existing thread/cache without refetching that list.
Turn footers name a model only when it changed, group recorded context links into one entry, and expose
the actual recorded usage. Native questions arrive in their item with the current reader's authority,
so an answerable question does not wait on a second request; old items retain their guarded reader.
Committed live outcomes and confirmed decision replies settle the card and its former decider without
a reload. The header, status line, turn fold and footer share the server's recorded human-wait counters:
working time freezes while a person is asked and excludes that wait after resuming or completing.
Waiting inputs and unacknowledged sends have no grey timeline row: all of them live in the queue above
the composer, including the first, Stop-held and blocked inputs.
An ended session's steps retain the native facts without a live timer or running header chip. An
unsettled historical step says it was recorded while running. A restore appears only for a recorded,
finished turn with current file read/write rights; an unavailable recorded copy puts its reason on
the actual Review changes action rather than repeating a paragraph on every turn.

- `Timeline.tsx`: Virtualized main stream, history loading and stable scroll anchors.
- `rows.ts`: The rows the conversation is drawn as: which items stand alone, which are folded into one line for their turn, and where the lines that belong to no item go.
- `titles.ts`: The words and the figure of a step's row, made from what the tool call was and not from the server's own line: so they come in the reader's language.
- `waiting.ts`: What a thread is waiting for, in words: the step of its start, the reason it is blocked, the job that holds it.
- `turnTime.ts`: A turn's working time excludes the human waits recorded by its live feed.
- `historical.ts`: The recorded before-Orch or ended-session boundary keeps historical tools from appearing live.
- `agent.ts`: Which agent said or did a thing: the agent of the segment the item belongs to, else the one the thread is bound to now.
- `context.ts`: What every item of a conversation is told about the conversation it stands in: the thread, how the reader likes the work shown, and what they may do.
- `preferences.ts`: How the reader likes a conversation: how much of the agent's work is open, how a difference is laid out, whether the thread's history is shown.
- `state.ts`: What the reader opened or closed by hand in a conversation, kept for as long as the screen is open.
- `Steps.tsx`: The agent's work as steps: the turn that runs and the last finished one step by step, older turns folded to a line; and the depth a person chose (Steps, Summary, Everything), kept for them.
- `RawCall.tsx`: A call exactly as its harness made it: its own name for the tool, its arguments and what came back, whole, searched within and copied.
- `UndoChange.tsx`: Undo this change: a recorded difference applied backwards as an ordinary save, refused where the same lines changed again.
- `PlanLine.tsx`: The plan of the running turn as one line above the composer.
- `StatusLine.tsx`: What the thread is doing now, as one line: the phase of its turn, a job that holds it, or what it waits for.
- `HistoryView.tsx`: A thread's History as a view by itself: every line of it, with what happened to its environment that the work there would notice.
- `FindInConversation.tsx`: Find in the conversation: a search of this thread alone, with its hits lit in place.
- `SelectionBar.tsx`: The bar of a selection across turns: how many items are picked, and Forward, Quote, Discuss in chat, the forks from the last one, Copy and Cancel.
- `SubagentLane.tsx`: Read-only child stream with shared renderers.
- `Lane.tsx`: One compact, indented conversation lane for a sub-agent or a person's aside.
- `SideThreadLane.tsx`: A side thread beside the conversation: its own short exchange with the same agent, named after its thread, kept by a fork or closed.
- `Catchup.tsx`: Unread summary and jump-to-latest behavior.
- `TurnFooter.tsx`: Duration, the model by its display name and effort, recorded usage, and what the turn changed.
- `ItemActions.tsx`: Copy, link, pin, quote, discuss, forward, select, and fork or ask on the side from any message.
- `ChatRefs.tsx`: Count of chat messages quoting an item, opening that chat.
- `items/UserMessage.tsx`: Human, system, forwarded and delivered announcement inputs.
- `items/AgentMessage.tsx`: Streaming agent Markdown and final actions.
- `items/Reasoning.tsx`: Available reasoning summaries.
- `items/Plan.tsx`: Live checklist.
- `items/ToolCall.tsx`: Neutral tool-kind dispatch: a display made for each kind, and the raw call under it.
- `items/Approval.tsx`: Harness request and decision card, with one visible subject shared with answer fields, the original decision/cache/decider rules and help in its shared rail home.
- `items/Announcement.tsx`: What a thread was told: an announcement, or a milestone raised to the topic, with its origin and severity; or a message another thread sent to this one, with its files.
- `items/Marker.tsx`: The lines of a thread's history between its items: who created it, changed its model, its page or its owners, switched or forked it, or let a turn past a limit; and segment and lifecycle markers.
- `items/Error.tsx`: Actionable failure and retry.
- `tools/Command.tsx`: Command, exit state, output as it prints and working directory; one left running shown as such, with Stop; what it was for, where the harness says.
- `tools/FileChange.tsx`: Paths and differences in the file's own colours with the changed words marked, in one column or two; each opening in the code view; Undo this change.
- `tools/FileRead.tsx`: A file the agent read: its path as a link, the lines, and an excerpt.
- `tools/FileSearch.tsx`: A search of files by content or by name: what was looked for, where, and the matches as links.
- `tools/Mcp.tsx`: MCP arguments and results.
- `tools/WebSearch.tsx`: Search query and result links.
- `tools/WebFetch.tsx`: A page the agent fetched: its address, its title and its status.
- `tools/Image.tsx`: An image the agent looked at or made.
- `tools/Subagent.tsx`: Child agent activity.
- `tools/Orch.tsx`: A row for each of Orch's tools: what was listed, read and searched, with the place and the depth read; a download; a document made or changed, with Show changes; a message written in a chat or sent to a thread; an announcement or a milestone; a capability asked for.
- `tools/Other.tsx`: Unknown or additional declared tool payloads.

`Timeline.tsx` also draws a retained snapshot from an isolated renderer key. It mounts no live feed or read marker, and its item/file/approval acts remain read-only; older stored output uses the explicit inspection flag.

failures.ts assigns one terminal error to a recovery chain and keeps manual actions on its latest unfinished turn. FailureAlert.tsx renders that failure once through the existing error alert, with the observed attempt count.
