# The views of a thread: workspace, runtime and people

These are views of the workbench: tabs of the right bar as shipped, and moved into any other area by
their reader. A file opens in the code view of `../files/`. Each view stands on the thread's door into its
environment (`../files/door.ts`): what the reader holds on the thread's page decides what is enabled, and
what is refused says what it takes and whom to ask.

Built: **Files** (the tree, lazy and drawn by the rows in view, kept equal to the disk; the search by name and
inside files; the menu of a row; making, renaming, copying, moving, deleting, uploading; the way the source is
told), **Changes** and **Git** (each file once with `+a −r` and its difference), **Terminal** (a person's own
shells, taken up again and kept one screen at a time), **Processes** and **Resources** (the table with a signal
sent by whoever may, the small charts with their limits), **Outline**, **Environment** (where it runs, its state,
start and stop where served, the idle control, the two switches its managers hold) and **Parameters** (the model, the
effort and approval mode read-only, changed in the message box, plus the other parameters the agent declares).
The panel omits unsupported limits, gives an empty native-parameter list one line and keeps native writes,
locks and source facts. The live feeds of directories, processes and measurements are
subscribed to only while shown. Hidden workbench bodies keep drafts while releasing their readers.
Terminal's one create action travels with its tab/section header; one scope question opens the full
consequence. Empty, refused and failed opening states are exclusive. Retained output expands in the view with Show all;
recording and terminal-code failures keep a retry path.
The compact Environment view keeps network and credential explanations behind their existing guide or
question controls. Actual mode, placement, expiry, server state and recovery actions stay in its short rows.

History reads the thread's audit lines; People reads its permission page, presence and waiting requests;
Knowledge reads its agent's shared tree without revealing redacted names; Diagnostics pages and filters raw
segment frames, with the declared agent terminal watched first and control confirmed separately. Given to the
agent shows, to whoever reads the thread, what Orch gave its agent exactly as it was sent: what each session
opened with and each message with what went before it, block by block, newest first; a person's message opens it
narrowed to its turn. Usage uses the thread's ledger view. Files and Terminal also open directly from an environment, using its own page.

- The Chat view is `../chat/ChatPanel.tsx`.
- `Inspector.tsx`: The views of a thread beside its conversation, each of which can go into any area of the workbench.
- `parts.tsx`: What every view of a thread beside its conversation shares: the door into the thread's environment with its three states, and the way a list of paths is drawn.
- `words.ts`: The words for a value the API names, and for a stand-in when a newer server names one this page has none for.
- `Outline.tsx`: Turns, human and agent messages, milestones and approvals, in the conversation's words.
- `Related.tsx`: A document inspector's bounded related list, read once per revision; reference cards load only when opened.
- `outlineRows.ts`: The outline's historical anchors and live rows merged without polling or repeating a turn.
- `Files.tsx`: The workspace of the thread's environment, opened at the thread's directory, with who else works there; the search by name and inside files; the tree's context menu; live or cached; live only for a tree too large to index.
- `Changes.tsx`: The files this thread's turns changed, newest first, each opening as a difference; or those of every thread in its environment.
- `Git.tsx`: Read-only Git status and diffs.
- `Terminal.tsx`: The caller's own shells in the thread's environment, whichever screen each was opened from: taken up again with what scrolled past, one screen at a time, how long they stay and how many, the choice of which to close for one more, Add to message, and the note on what a shell there reaches.
- `TerminalScreen.tsx`: One shell on the screen: the terminal, drawn from the bytes the shell prints and sending what is typed, with the states a shell goes through.
- `Processes.tsx`: The environment's processes, this thread's own first, and signal controls.
- `Resources.tsx`: Resource history against limits.
- `Environment.tsx`: The thread's environment as far as the viewer may see it: where it runs, its state, start and stop where served, its idle control, and the switches its managers hold.
- `IdleControl.tsx`: In-place timeout, provenance and keep awake.
- `Parameters.tsx`: The thread's resolved choices, read here and changed in the message box, and its other declared parameters.
- `History.tsx`: The thread's history, newest first, narrowed by kind and person.
- `ThreadPanel.tsx`: A thread view that works even when its thread has no environment.
- `AgentTerminal.tsx`: The harness's own terminal, watched first and controlled only after a recorded confirmation.
- `ShareToAgent.tsx`: Choose what to give before sharing it with this thread's agent.
- `Knowledge.tsx`: What this thread's agent can read and write beyond its own thread, as the tree its tools show: a pen where it may write, a plus where it may make documents.
- `People.tsx`: The thread's page in short: who views, who talks to the agent and who manages, where each comes from, presence, who lacks the model, and requests.
- `Diagnostics.tsx`: Raw protocol frames and gated agent-terminal rescue.
- `Given.tsx`: What Orch gave the thread's agent, exactly as it was sent: what each session opened with, and each message with what went before it, block by block, the newest first.
- `GivenBlock.tsx`: A recorded text block: bounded inline text and the immutable whole behind its ordinary blob read.
- `InstructionSources.tsx`: Instructions this agent reads: Orch's introduction, the guide to its tools, each place's standing instructions and the files its harness reads, in their order, with their sizes and what was cut, and a standing change that waits for the next turn.
- `givenFocus.ts`: Which turn the view of what the agent was given is narrowed to: set where a message's menu asks for what the agent received.
