# Uploads and private drafts

Message attachments stay in Orch until the message is sent. A thread receives its files
under `.orch/attachments/` when delivery runs. Files-panel uploads write into the selected
environment folder immediately. Both use resumable browser upload sessions.

## Size and context settings

`storage.max_blob_bytes` is the content store's upload limit. The Files panel uses upload
sessions for large files rather than its former raw-request limit. A request's smaller
body or chunk limit does not set the maximum complete file size.

`composer.paste_inline_context_share` is an inheritable parameter from the system to a
thread. It is 25% by default; 0 is Off and sends every folded paste as a file. The share
uses the model's remaining context. Unknown context uses a file, and estimated counts
are marked approximately. Folding itself follows the composer's displayed height.

## Retention and recovery

Apply the release migrations before starting the updated API and worker. Saved thread
drafts own one blob pin per distinct hash. Pending private filename reservations hold
their own bytes until a matching draft adopts them, they are removed, or they expire.
Editing a saved attachment holds the replacement until the draft acknowledges its hash.
Deleting a draft releases its pins; another message, draft or copied source keeps its own.

Keep the database and content store together in backups. Removing an attachment releases
its unused reference; physical collection still follows the configured blob grace.
Closed upload sessions retain only name, size and measured progress receipts until expiry.
A receipt is not retained content and does not grant access to a blob.

## Workspace permissions and cancellation

Workspace uploads require current `files.manage`, remote writes enabled and an online
machine. Its final copy is a durable delivery job, with the machine's progress in the
upload row. Cancellation stops the browser transfer and requests the machine's stop.
The row stays Stopping until the authenticated command result confirms that its handler
has ended. A cancelled or expired command row alone is not that confirmation. An offline
machine keeps the stop pending and receives it when it reconnects. Files already written
are kept; an incomplete replacement preserves the original.

The delivery job pins its uploaded content until completion or confirmed stopping,
including across API restarts. Its owned checkpoint releases that pin once and keeps a
verified completed file in the audit. Missing parent directories are created by the same
native write, so cancellation also owns that work. Keep the API and machine programs on
the same release when updating this behavior.

The environment's History groups the confirmed writes and partial receipts by uploader
and batch. It records names, folder, sizes, replacement count and outcome without sending
notices to other threads or agents. History can record retained facts after a machine
goes offline; recording those facts performs no filesystem write.

See [Paste and upload](../user/paste-and-upload.md) for the visible controls and destinations.
