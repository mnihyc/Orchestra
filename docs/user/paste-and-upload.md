---
title: Paste and upload
screens: /thread/$threadId, /chat/$chatId, /place-chat/$nodeId, /topic/$topicId, /project/$projectId, /org/$slug, /doc/$documentId, /workbench, /resources/environments/$id
keywords: paste, drop, upload, attachments, pasted text, files, folder, draft, context, cancel, conflict, 粘贴, 拖放, 上传, 附件, 草稿
---

# Paste and upload

The place under the pointer is the destination. A drop line names that place and,
when the browser supplies a count, the number of files. File names appear after
the drop. Drop an item already in Orch to insert a reference rather than another copy.

## Message files

Drop files on a message composer, paste them, or use its attachment button.
They stay in your private draft until **Send**. Removing or cancelling an attachment
stops its upload; it gives nothing to the environment or agent.

A thread's chips show their destination under `.orch/attachments/`, followed by
**when sent**. An occupied name gets a numbered suffix. The agent receives the actual
path; supported images also reach the model as images. Chat attachments stay in the
chat's files. Ready attachments survive a reload and are available with the same draft
on another device.

## Pasted text

A paste that would make the composer scroll becomes a **Pasted text** chip. There is
no fixed character threshold. Open the chip to edit its words in place.
**Paste as text** puts the latest words back into the composer and removes the chip.
Wait for a pending edit to finish saving before sending; Retry keeps the local words.

Each chip has **In the message** and **As a file**. The choice belongs to that paste,
and stays with its draft and edits. **Pasted text in the message** supplies the default:
25% of the model's remaining context. Unknown context defaults to a file.
Choose **Always as a file** (0) or **Always in the message** (100) at either end.
The setting inherits down to a thread and can be locked; the sender still chooses
the delivery of each paste.

The token count is an estimate, marked **≈**, using the selected model's data.
When that data is unavailable, the chip omits the estimate.

Choosing the message allows the agent program to compact its own context. Orch does
not turn that default into a size limit. If one message exceeds the model's whole
window, the turn reports the provider's refusal and offers **Send as a file**.
A file chip shows its destination; the agent receives its path and size.

Chat keeps pasted text in its files. A copy sent to an agent follows that receiving
thread's context. In a sent message, the chip opens read-only. Use the message's
existing **Edit** action to make a new version; its pasted text opens editable.
Saving that edit keeps the original version and gives the changed file a private copy.

## Workspace uploads

Use **Files** to put files into the environment now. Drop onto a folder row or the
empty part of that folder. Pasting files uses the selected folder. **Upload…** opens
the file chooser. Folder uploads retain their relative directories here.

Each row shows its name, size and progress. The batch shows completed files and bytes.
Transfers resume from received bytes on Retry. The content store's size setting applies;
there is no separate 8 MiB limit in Files. Uploading needs **Manage files**, a machine
that is online and remote file writes enabled. An unavailable action shows its reason.

A conflict asks once for that batch: **Keep both** chooses numbered names; **Replace**
replaces conflicting names. The final write checks the destination again.
After the browser upload, the same row follows the copy to the machine. Cancel stops
unfinished work. The row stays **Stopping…** until the machine confirms that its write
has ended. If the machine is offline, the stop waits for its return. History records
**Cancelled** after that confirmation. Files already saved remain; an unfinished
replacement leaves the original file in place.

The environment's **History** records the person, file chips, folder, measured size,
time and outcome. Open **and N more** for the remaining names. Replacements are counted;
cancelled and failed batches keep what was confirmed. An unknown amount reads **–**.
The upload sends no message or notice to other threads or agents in that environment.

## Document files

Drop files at the document block under the pointer, paste files at the cursor, or use
the attachment button. They become document files and blocks at that position.
Moving the cursor while an upload runs does not move its insertion point.
Pasted text becomes ordinary document blocks; it does not fold into a Pasted text chip.
Folder imports add their contained files. Empty folders do not create file blocks.
Markdown, HTML and Word are read as described in [Write documents as files](write-documents-as-files.md).

## Viewers and references

File viewers and code views do not accept file uploads. Open **Files** to upload into
the workspace. Ordinary text paste still edits an editable code file as typing does.
Dragging an Orch file or attachment into a composer or document inserts its reference.

## Queue and steer

While a turn runs, Send queues by default. The queue's **Send now** and the composer's
Ctrl/Command+Enter request immediate delivery. Read [the message queue](message-queue.md#5-put-an-untaken-steer-back)
for steering, interruption and returning an untaken steer to its place.
