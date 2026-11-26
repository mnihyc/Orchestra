---
group: work
title: Thread states
screens: /thread/$threadId, /topic/$topicId, /project/$projectId, /org/$slug, /everything, /search, /inbox, /resources/providers/$id
keywords: thread, icon, state, queued, held, finished, blocked, approval, unread
summary: Read consistent thread states, unread results and supported background-work controls.
---

# Thread states

A thread's state shows as an icon that is the same everywhere the thread appears. This page explains the icons, how new results show, and the controls for work that runs in the background.

## Read the icons

A thread has the same state icon in its tree row, tab, header, lists and references. Hover or focus
the icon for its reason, waiting-message count or completion time. Live views update together;
a search result or reference keeps the state from its last read.
Tree and Recent share these icons, including the Navigation sheet and Everything's narrow cards.

| Icon | State | Meaning |
|---|---|---|
| Shield with a question | Waiting for approval | An approval, question, plan or held message needs a person |
| Slashed circle | Blocked | The thread cannot continue; its reason says what needs to change |
| Alert octagon | Failed | The last turn failed and no later message was accepted |
| Spinner | Running | A turn is starting or working |
| Clock | Messages waiting | Messages wait behind work, a limit or a schedule |
| Pause circle | Held | The queue stays stopped until someone chooses Resume |
| Check circle | Finished | A turn completed since you last saw it |
| Speech bubble | Idle | No work or unseen result needs attention |
| Archive | Archived | The thread or its place is archived |

An outstanding decision takes priority over a blocker, then failed work, running work, waiting
messages, a held queue and a new result. An archived thread keeps its archive icon.
Only Running spins. Reduced motion keeps it still. Shapes carry the meaning in either theme.

An automatic retry uses the clock, with its time and attempt: “Retrying in 55 s · attempt 2 of 3”.
Nothing needs your decision while that retry is scheduled. An active recovery shows Running.
When automatic recovery stops, the failure icon explains why, such as “Provider busy · 4 times in 5 min”.
Requests waiting for people and changes holding the thread keep their priority.

## See new results

Finished replaces the thread's unread dot. Seeing its latest turn clears the check for you;
other readers keep theirs until they see that turn. Mention counts stay visible.

A place keeps its own icon and shows its most urgent thread state at the row's end.
A check there means new results inside. A private reference shows no state to someone who
cannot read the thread.

For queue controls, see [The message queue](message-queue.md).

## Background work

An agent that is idle can still have a command or a sub-agent running. Beside Idle, the header names each known task and its state. Chips of sub-agents open their conversations.

**Will wake when these finish** means a task that succeeds or fails will request a continuation under the person who started it. The adjacent **Background wake: On** shows the value accepted by the session. Managers can click it to open that one setting; inherited values, locks and reset keep their normal behavior. A changed value is shown as **Next session: On/Off** until it applies. Session changes wait while work runs.

With background wake off, **Results arrive with the next input**. Background work of Codex can still run, and its results arrive with the next turn. Claude Code does not offer background tasks in this mode, and its commands run in the foreground. Changes apply to the next session, and each completion also checks the current setting and the original person's access.

A continuation starts with **Woken by: Build web finished**. Several causes are separated by commas; a failure says **Review auth failed**. These labels come from the recorded tasks. A continuation waiting for access or model use shows the ordinary held-input reason with its cause. **N finished tasks waiting** means Stop is holding those wakes until the next accepted human input. It does not resume the ordinary Stop-held queue.

**Stop** ends the foreground turn and holds its pending work. **End** stops one exact background task without Orch prompting the model. The agent program may tell the model that the task ended. **End** needs Interact access, and the agent program has to support ending that kind of task. Where it does not, the task says **Cannot be ended from here**. **End** opens a fresh review by the agent program before you confirm, so that later work cannot be ended by the button of an older task. Orch starts no continuation for an ended task; an unconfirmed End remains unknown until current facts confirm it.

**Lost with the session** records work lost when the session that owned it disappeared. A disconnection that is temporary is not a known empty list and does not prove a loss. Lost work is not restored from a transcript.
