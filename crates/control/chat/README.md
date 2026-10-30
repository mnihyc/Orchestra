# orch-chat

People's chats: messages, following, inspection and forwarding.

Implemented. Node, direct and group chats support messages, replies, edits, deletion, reactions, pins, mentions, private drafts, forwarding, following and inspection; their rows and notifications share the caller's transaction. File references require an uploader's authority or a currently readable saved source, and drafts pin their files until replaced or discarded. Real PostgreSQL, HTTP, blob collection and realtime tests exercise these behaviors, with unit tests for the rules that need no database.

Chats are conversations among people. What is said in a chat reaches an
agent only when a person forwards messages to that agent's thread or
shares the chat with that agent, and an agent writes in a chat only where
somebody shared it for writing.

| Module | Planned responsibility |
|--------|------------------------|
| [chats](src/chats.rs) | One chat per node; direct and group chats within a root org, with guests from outside it. |
| [messages](src/messages.rs) | Posting by people, and by agents where a chat was shared with them for writing; editing, deleting, replies, reactions, pins and masking. |
| [mentions](src/mentions.rs) | Mention resolution for people who can read a chat. |
| [audiences](src/audiences.rs) | Current readers and complete role and group audiences for a deliberate chat mention. |
| [audiences](src/audiences.rs) | Current readers and complete role and group audiences for a deliberate chat mention. |
| [quotes](src/quotes.rs) | Timeline items quoted into a thread's chat, and their counts. |
| [written](src/written.rs) | A chat written out: its messages by number with what each carries, in brief or whole, for an agent that reads it, a copy and a download. |
| [following](src/following.rs) | Engagement, chosen levels, unread state and mark-read. |
| [notifications](src/notifications.rs) | Mention and per-chat notifications written with the message. |
| [error](src/error.rs) | What can go wrong with a chat, in the words of the part that answers the request. |
| [redaction](src/redaction.rs) | Masking of what must not be stored in a chat: the values of the secrets a node resolves, and the patterns an org asks to be masked. |
| [views](src/views.rs) | Rows as the types of the API: a chat as one person reads it, and a message as it is shown. |
| [attachments](src/attachments.rs) | Images and files of messages as pinned blobs. |
| [inspection](src/inspection.rs) | Read-only access to the direct and group chats among an org's people, for whoever holds that row on the org: audited, and announced to nobody. |
| [forward](src/forward.rs) | Copies of selected messages, or of selected items of a thread with the agent's work of a turn as one part, with their images and files, for chats and people; thread destinations become inputs. |
