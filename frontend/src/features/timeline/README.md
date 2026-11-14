# The conversation: the agent's work step by step, side threads, selection and history

Built: the conversation of a thread as a list that draws what is in view, so that five thousand items open
at their end and scroll as ten do. Every kind of item has its row. A turn reads as the person's message (a tinted block
at the right; the sender's name above it only where more than one person wrote), the work of the agent as one group,
the answer as plain text, and what stands under it. The group holds everything between the message and the answer:
steps, thought, decided approvals and the sentences the agent says between them. A turn that is over shows it as one thin
rule with its label ("Worked for 1m 31s · 29 commands · 3 files edited"), which opens to the steps in place; while the
turn runs the group is open as a preview of the latest steps in a box of bounded height that follows its own end and
folds to the line when the turn is over. The line says failed or stopped only when the turn did, and names the agent only
where it is not the one of the turn before. A question or an approval that waits for a person, an error and a message
steered into the turn stand outside the group, where they came. The reader reads in one of three ways, kept for each
thread: Normal (this), Verbose (every step a row, a running tool's output open as it prints) and Diagnostics (Verbose, and
the program's own state). Every
tool begins as one compact row and expands in place on click, including the most recent edit and a
failed or running command. An opened command shows its output as it prints, an edit its difference, a reasoning its
summary; a turn that is over ends in its footer; a "New" line stands before what the reader had not seen
when they opened the thread; the one line of the dock, under the conversation, says what the thread is doing. Older items are
read as the reader comes to the top, a link to an item opens the conversation around it, and a reader who
is not at the end is told, by the round button above the composer, how many rows came below, and goes there with it.
A read that was cancelled or overtaken is no error: the conversation stays drawn, and one that failed leaves a small line
with Try again, tried again by itself when the reader comes to the top once more. A sub-agent starts folded to its last reported line,
state mark and duration, then opens at its originating tool row into a bordered child lane. Its row owns
its name and clock; the lane adds its reported role and compact shared item displays. Raw and Open in
panel are icon acts on the opening row, beside its state; durations below one second are omitted.
A single-file edit names its path, kind and line totals once, with file-open on that same row; multiple
files and approvals retain their per-file headings. The header's list of agents opens a sub-agent's own conversation in place of the main one (the address carries it), and
Open in panel shows the same stream in the Conversation container. Earlier child
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
Under a turn that is over stand, as needed, how it ended where no work line says so, a model or effort that changed, and one
card for the files it changed ("3 files changed +10 −3", opening the list, with Review). The duration is on the work line;
the toolbar of the answer (Copy, Retry, Fork, Restore, the time and a menu) shows on hover and focus, and the time opens
what the turn used by input, cached input and output, by model; no sum of the requests is drawn as one figure. A turn with
no answer carries the toolbar under its work. Native questions arrive in their item with the current reader's authority,
so an answerable question does not wait on a second request; old items retain their guarded reader.
Committed live outcomes and confirmed decision replies settle the card and its former decider without
a reload. The header, status line, work line and footer share the server's recorded human-wait counters:
working time freezes while a person is asked and excludes that wait after resuming or completing.
A message sent to a thread that takes it into a turn of its own is the person's message in the conversation, dimmed until
the agent takes it; the queue above the composer holds what waits behind a turn that runs, and the Stop-held, blocked and
failed ones.
An ended session's steps retain the native facts without a live timer or running header chip. An
unsettled historical step says it was recorded while running. A restore appears only for a recorded,
finished turn with current file read/write rights; an unavailable recorded copy puts its reason on
the actual Review changes action rather than repeating a paragraph on every turn.

- `Timeline.tsx`: Virtualized main stream, history loading and stable scroll anchors.
- `rows.ts`: The rows the conversation is drawn as: which items stand alone, which are drawn together as the work of their turn, which message is a turn's answer, and where the lines that belong to no item go.
- `rowKinds.ts`: What each item is to the reader: the conversation, the agent's work, or the program's own state (diagnostics), which is kept and drawn only in Diagnostics reading.
- `ItemRow.tsx`: One item drawn by its kind, for the conversation and for the steps inside a group.
- `AnswerFrame.tsx`: A turn's answer with the toolbar of the turn.
- `silence.ts`: How long nothing may come from a running agent, with no tool running, before the status line says so: the thread's `thread.silent_after_secs`.
- `thinking.ts`: The first line of the summary of the thought being written now, which the status line names.
- `titles.ts`: The words and the figure of a step's row, made from what the tool call was and not from the server's own line: so they come in the reader's language.
- `waiting.ts`: What a thread is waiting for, in words: the step of its start, the reason it is blocked, the job that holds it.
- `turnTime.ts`: A turn's working time excludes the human waits recorded by its live feed.
- `historical.ts`: The recorded before-Orch or ended-session boundary keeps historical tools from appearing live.
- `agent.ts`: Which agent said or did a thing: the agent of the segment the item belongs to, else the one the thread is bound to now.
- `context.ts`: What every item of a conversation is told about the conversation it stands in: the thread, how the reader likes the work shown, and what they may do.
- `preferences.ts`: How the reader likes a conversation: how it is read (Normal, Verbose, Diagnostics), how a difference is laid out, whether the thread's history is shown.
- `state.ts`: What the reader opened or closed by hand in a conversation, kept for as long as the screen is open: a step, a group of work, the raw call under a step.
- `Steps.tsx`: The agent's work in a turn as one group: its line, and its steps and sentences under it.
- `RawCall.tsx`: A call exactly as its harness made it: its own name for the tool, its arguments and what came back, whole, searched within and copied.
- `UndoChange.tsx`: Undo this change: a recorded difference applied backwards as an ordinary save, refused where the same lines changed again.
- `PlanLine.tsx`: The plan of the running turn as one line above the composer.
- `StatusLine.tsx`: What the thread is doing now, as the one line of the dock: a spinner, what is being done, the turn's one clock (for a thread that starts, the time since its step began, where the thread says it), and Stop only where the composer, which has it, is not drawn; "Needs your input" while an approval waits; how long nothing came, after the seconds the thread's silence notice names (two minutes unless set otherwise) with no tool running.
- `HistoryView.tsx`: A thread's History as a view by itself: every line of it, with what happened to its environment that the work there would notice.
- `FindInConversation.tsx`: Find in the conversation: a search of this thread alone, with its hits lit in place; a hit in the folded work of a turn opens that group.
- `SelectionBar.tsx`: The bar of a selection across turns: how many items are picked, and Forward, Quote, Discuss in chat, the forks from the last one, Copy and Cancel.
- `SubagentLane.tsx`: Read-only child stream with shared renderers: under its step, in the side panel, or in place of the main conversation.
- `Lane.tsx`: One compact, indented conversation lane for a sub-agent or a person's aside.
- `SideThreadLane.tsx`: A side thread beside the conversation: its own short exchange with the same agent, named after its thread, kept by a fork or closed.
- `Catchup.tsx`: The "New" line and the card of what came while the reader was away.
- `TurnFooter.tsx`: What stands under a turn that is over: how it ended, a model or effort that changed, the card of the files it changed, and its acts where it has no answer.
- `TurnFiles.tsx`: The one card of the files a turn changed.
- `TurnActions.tsx`: The acts on a turn: try it again, fork from it, ask aside, pick it, copy it or its address.
- `TurnFigures.tsx`: The time of an answer, and under the pointer what its turn used.
- `ItemActions.tsx`: Copy, link, pin, quote, discuss, forward, select, and fork or start a side thread from any message.
- `ChatRefs.tsx`: Count of chat messages quoting an item, opening that chat.
- `items/UserMessage.tsx`: Human, system, forwarded and delivered announcement inputs: a person's words in a tinted block at the right.
- `items/AgentMessage.tsx`: Streaming agent Markdown as plain text; the answer carries the turn's toolbar.
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
