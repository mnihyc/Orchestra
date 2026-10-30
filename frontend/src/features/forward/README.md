# Selecting and forwarding

Forwarding copies selected messages, with their images and files, to chats,
people and agents' threads. A whole chat, thread, topic or project is shared
from `../access/`.

Messages are picked in a chat (a checkbox on each, `Shift+click` for a range, a
bar in place of the composer) and forwarded in one dialog: to chats of places,
to groups, to people and to agents' threads at once, at most ten, with a comment,
a preview and one sentence on what the choice means; if one place refuses, nothing
is sent and the dialog says which and why. A forwarded message arrives in a chat as
one message by the forwarder with a block for each source, and in a thread as an
input of the forwarder's. The picking and the dialog are used by the timeline for
the items of a thread, which go anywhere a message does, the thread's own chat
first, where they stay live quotes. The rules of the destinations and of the
picking are unit tested.

- `Selection.tsx`: Multi-select of chat messages, or of any items of a thread across its turns.
- `ForwardDialog.tsx`: Several destinations of four kinds, a comment, a preview and the stated effect; what the agent said and did in one turn arrives as one part.
- `ForwardedBlock.tsx`: Forwarded copies with their source, author and time, in a chat or a timeline.
- `destinations.ts`: Where a forward can go: the chats of the places a person can write in, their group chats, the people of the org and the agents' threads, and the sentence that says what the choice means.
