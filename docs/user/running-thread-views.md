---
group: work
screens: [/thread/$threadId, /workbench, /settings/interface]
keywords: [thread, Focused, Full, Live, tool calls, Now, Changes, review, icons, retry, approval]
summary: Choose how much agent activity you read without changing its work.
---

# Follow a running thread

Choose how much activity you want to see without changing the work the agent runs. **Focused** keeps your inputs, the agent's answers and waiting approvals, with a summary for each turn. **Full** includes the collapsed reasoning and tool rows. **Live** follows the active work in more detail. The thread remembers your display choice.

## Where

Open a thread from [navigation](/) or the [workbench](/workbench). The header offers the detail choice and the thread's state. The Now line names the current step, elapsed time and Stop. The depth choice folds work into Steps, Summary or Everything.

A subagent row opens only its child stream; Open in panel keeps that stream beside the conversation. A side thread has its own lane, composer, queue and Stop. You can inspect either while the main agent continues; a subagent has no message box.

**Side thread** asks that thread's agent about its current work. For read-only questions about the page or your organization, open [Ask](ask.md) from the rail.

A message's action row appears on hover or keyboard focus and stays available on touch. Your message offers Edit, Copy and **Restore files to before this message…**, with further actions in its menu. An agent turn offers Copy, Retry, Fork and its further actions. A disabled action has a reason; Retry is not offered as an executable action on an unsupported earlier turn.

## Example

While the agent investigates a failing check, use Focused to read its progress summary. Switch to Live to follow a command. When the turn shows **N files changed · +a −r · Review**, open Review to inspect those files. Add a comment beside a changed line, then Send review from the composer so the next turn receives it.

## What to expect

Changes offers the selected turn, changes since the thread started, and uncommitted Git changes when a repository is available. The same file and difference views support Open file, Open difference, Copy patch, Add to message and Undo this change where permitted.

Open **Background work** from the view menu to inspect this thread’s native commands and subagents in a panel. The same reviewed list and End confirmation are used by the header control; Refresh reads a new observation.

While background work runs, another person's input can use the same session. Overlapping usage is counted to the thread's first owner. The header says this beside the wake notice and its setting; each task and woken turn keeps the author who started it. End still requires permission to interact with the thread.

The compact model control keeps the selected effort beside the model even in a narrow pane. The binding control names the current provider and opens the usual switch plan when you may change it. Add files or readable file/document references through +; choosing them does not send the draft.

The composer keeps approval mode, sandbox and Plan nearby. Waiting approvals need an explicit answer; Escape does not deny one. Thread state icons remain consistent in navigation and cards. See [restoring files](restore-files-before-a-message.md) and [approvals](how-approvals-work.md).
