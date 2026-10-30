# Chats among people

A chat is a place of its own beside an agent's thread: its own history,
composer, draft and read line. Nothing typed in a chat goes to an agent
unless a person forwards it or shares the chat, and an agent writes here
only where a chat was shared with it for writing.

The chat is built. One component draws every chat: messages oldest first with
day dividers, the line where the new ones begin and the header shared by a
person's messages within five minutes; replies, edits, deletes, reactions with
an emoji picker, pins, `@` mentions, who is typing and who is here, the bell
with its levels, a draft kept on the server and followed to other devices, and
a message that is sent again while the line is bad and stays with "Not sent"
if it cannot go. The chat of a place stands in the right bar as the `chat` view
(a thread's, its topic's and its project's by a segmented control), and the
direct and group chats of a root org are the screen at `/org/$slug/messages`,
with a switch for those who may read the chats among an org's people. A guest of
a group sees that chat and nothing of the org. The rules that need no browser
(rows, text, Markdown, mentions, the reducers of the live state) are covered by
unit tests; the screens by end-to-end tests with two people in two browsers
against the fake API, and by axe in both themes.

Chat scope and icons share one ordinary-height header row; edit actions wrap within their panel and
icon controls keep the shared compact height. People looking stay one avatar button with all names in
its tooltip and list, rather than expanding the row.
The single-line composer keeps its attachments, emoji and Send beside the input, even as text grows.
Empty file collections and idle draft/typing indicators take no conversation space; their live regions
stay mounted for announcements. The browser layout proofs compare chats against the thread at 1440
and 400 pixels, in both languages and themes, including multiline input and editing.

The chat scope selector uses short kind names alone. The full place is in its pointer/keyboard tooltip
and in the composer's accessible name; its visible placeholder stays short at narrow widths.
The input measures its full scroll height once, including padding. Focus stays in the switch when changing chats replaces the live
conversation. It uses no separate caption, card or option guide. Appearance and visibility controls follow
the same compact convention; descriptions of dropdown choices stay in the open list.

Images and files use the common resumable uploader and are kept in drafts. Failed pieces can be
retried, and Send waits until files are attached or removed. Not here yet:
sharing a chat with agents and what
agents write (a message by an agent is drawn as it will be), and searching in a
chat. Forwarding to an agent's thread is in `../forward/`, where it says the same.

- `ChatView.tsx`: Header, message list and composer of one chat.
- `ChatPanel.tsx`: The Chat view, with the nearest thread, topic and project chats. Node and ancestor discovery follows its visible slot; a hidden right bar reads nothing.
- `register.ts`: Puts the real Chat view into the workbench, in place of the note that said it was not built.
- `shown.ts`: Whether a view of the workbench is in front of its reader: its tab is the one showing in its group, and its area is open.
- `panel.ts`: The chats a node's panel offers: the node's own and those of the places above it that talk about the same work.
- `MessageList.tsx`: Windowed history with day dividers, the line of the new messages, and a jump to the latest.
- `rows.ts`: The rows of a conversation: its messages with day dividers, the line where the new ones begin, and which messages share the header of the one before.
- `Message.tsx`: Grouped message with replies, mentions, edits and deletions.
- `AgentMessage.tsx`: A message posted by an agent in a chat that was shared with it for writing, with the thread it came from.
- `Body.tsx`: The text of a message as Markdown, with the people it names and the places it links to drawn as chips.
- `markup.ts`: The Markdown a chat message may hold, read into blocks and runs of text: paragraphs, quotes, lists, code, emphasis, links and mentions, and never raw HTML.
- `Attachments.tsx`: The images and files a message carries: thumbnails that open larger, and chips with a name and a size that download.
- `QuotedItem.tsx`: A timeline item quoted into a chat: a card with the item's kind, author, time and first lines, which leads to the item in the timeline.
- `Reactions.tsx`: Emoji reactions and who reacted.
- `Composer.tsx`: IME-aware chat input with mentions, emoji, replies, quotes and a draft; attaching waits for the server.
- `text.ts`: What a person types and what is sent: the word being mentioned under the caret, a mention put in its place, and the text of a message as parts.
- `draft.ts`: The draft of a message: what is typed, the message it answers and the items it quotes, kept on the server a second after typing stops and followed to the person's other devices. Shown copies share the existing chat draft query key; hidden composers neither read nor subscribe. Native draft events/write answers update that cache, local content is kept per person, and a late old-session writer cannot apply or replay into the new reader.
- `Typing.tsx`: Who is typing.
- `Bell.tsx`: Follow level: chosen, or the default once the person takes part.
- `Pinned.tsx`: Pinned messages.
- `acts.ts`: What a person does in a chat: sending, with the message shown at once and sent again while the line is bad, editing, deleting, reacting, pinning and reading.
- `context.tsx`: What the parts of an open chat share: the chat, who is reading it, the people known in it, and what can be done to a message.
- `open.ts`: Goes to an address inside the application through the router, so that the page is not loaded again.
- `discuss.ts`: "Discuss in chat": opens the thread's own chat with items of its conversation quoted in the composer.
- `focus.ts`: A navigation shortcut focuses the single live composer when it is ready, without changing the page.
- `links.ts`: The frame reveals a chat address before mounting its unseen body, through the same singleton open path.
- `Messages.tsx`: Direct and group chats of a root org with unread state.
- `ChatRow.tsx`: One row of a list of chats: avatars, name, the first line of the last message, its time, the unread dot and the mentions, and the row's menu.
- `NewChat.tsx`: Member picker for a direct or group chat.
- `GroupMenu.tsx`: Add people, guests included, rename and leave.
- `AllChats.tsx`: The switch, for whoever holds that row on an org, to the chats among its people and every node's chat, read-only.
