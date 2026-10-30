---
title: The message queue
screens: /thread/$threadId
keywords: queue, pending, held, edit, send now, steer, interrupt, reorder, drag, Alt+Up, Alt+Down, 消息队列, 排队, 编辑, 立即发送, 引导, 中断, 重排
---

# The message queue

## 1. Find messages waiting to run

Waiting inputs appear in one queue above the composer, including the first input and messages
held by Stop, a limit or a blocked thread. Open its header if it is folded. A row shows the
first meaningful line; hover or focus it to read the full preview.

## 2. Correct or remove a message

Use **Edit** on a pending message you wrote. Save in place with the button or Ctrl+Enter
(Command+Enter on macOS); Cancel keeps the previous text. Attachments stay with the message.
Someone who may drive the thread can remove a pending message. Held messages remain editable
and removable; the thread's reason for waiting is not a reason to lose your draft.
Once ordinary delivery has begun, the row says **Sending** and its pending edit controls stop accepting changes.
An in-flight steer has the separate **Steering…** state described below.

## 3. Decide when it reaches the agent

During a running turn, Enter or Send queues by default. Ctrl+Enter (Command+Enter on
macOS) requests steering. The place's **Default delivery** setting can change its default.

- **Queued** waits for the running turn to finish.
- **Steer** adds the input to the current turn when the agent supports it, without stopping that turn.
- **Interrupt** stops the running turn and puts this input first.

**Send now** moves the message first; during a turn it steers where supported. If the action
reads **Interrupt now**, it will stop that turn instead. Unsupported steering has a reason;
choose interruption deliberately. Neither action bypasses a limit, access check or blocked setup.
After Stop holds the queue, use its resume action when you want waiting messages to run again.
If Stop interrupts a turn while it is still starting, its undelivered input waits in the held queue too.
If the session is lost before the turn begins, the thread shows the failure and the input returns to the queue.
After repeated start failures the input stays failed. Otherwise, send another message when you are ready to proceed;
the waiting input goes first.
While the original message is still waiting, the turn's footer points to the queue above the composer
instead of offering a second copy. If the queue is folded, open its header to find the message.

## 4. Put messages in order

Drag a pending row by its handle, use its up/down arrows, or focus the row and press Alt+Up
or Alt+Down. These keys reorder the message rather than moving to another conversation.
Controls are offered according to your rights; only the author edits their text.
If a change fails, read the row's error and retry without composing a duplicate message.

## 5. Put an untaken steer back

A steer stands in the tray as **Steering…**. Use **Back to queue** before Orch begins
handing it to the program. It returns to its place among the waiting messages, keeping
its words and author. When **The agent is taking this message** appears, it cannot be
returned. After the program takes it, the conversation says **Steered**.

If the turn ends while a steer is being handed over, Orch waits for the result before starting the next turn.
A confirmed steer belongs to the turn that received it; a refused steer returns to the front of the queue.

Returning a message does not undo work the agent already did. Send a correction as
another message when the earlier one has been taken.

See [Paste and upload](paste-and-upload.md#pasted-text) for folded text and private attachments.

## Messages waiting for model or environment access

Reading or writing a thread does not by itself allow use of its model, nor of
the environment it runs in: a message to the agent has it act there for the
sender, so sending one needs Interact on the thread, Use of its provider with
the model and effort, and Use of its environment (**Use Environment**). With
the shipped Held for approval policy, your input keeps its queue position and
says **Waiting for someone who may use <model> and <environment>** (or only the
model, for a thread in no environment). Other eligible inputs can go ahead. You
can still edit or remove your held input under the ordinary rules.

A steer checks the provider, model and effort of the turn running now. A changed model setting applies to the next turn.

Someone who may drive the thread and use its current provider, model and effort
and its environment can choose **Run it**. The turn runs on that person's current access and usage,
with your authorship retained and their approval recorded. The action checks
their rights again; it cannot borrow permission from the original sender.
One decision settles every request copy, records who decided and when, and removes
it from pending and unread counts. Another attempt shows that recorded outcome.
The [approval rules](how-approvals-work.md#who-can-decide) determine who is told
and who may decide.

A place can instead choose Refused, which explains the missing use before
sending, or Allowed, which checks the thread's first owner. These choices
belong to `thread.inputs_without_use` and can be locked. Granting provider or
environment access and approving one held input are separate actions.
