# orch-realtime-contract

Browser WebSocket contracts, including terminal sockets.

Implemented. Every frame of the browser's WebSocket and of a terminal's socket is a type with the exact form it has on the wire, in both directions. The rules that need no socket are plain functions and values: how a client follows a durable feed by sequence (apply, drop or resync), how a growing text is followed by offsets and merged for a reader that is behind, how feeds are named, and the limits, close codes and reconnection of a socket. The unit tests state those rules, run simulations of a lossy link and of a slow reader, and read and write a literal example of every frame; an integration test checks the schemas that the browser's types are generated from.

Machine view feeds reuse the HTTP row types: processes and resource samples, browser/profile lists,
personal terminal pages, the port view and Git status with its actual environment/directory. A
directory's first event resets its entries; later events name only changes/removals and retain the
native truncation flag. Git can be named by thread (current binding) or environment/directory.
`env:<id>:procs` and `env:<id>:dir:<base64url path>` are also used beside a thread; its current
environment supplies the identity. The browser and Rust parse the same examples in the workspace-feed
fixture, so a permissive fake cannot hide an unsupported thread alias.
Job and fork-batch feeds carry the matching authorized read as `{job}` and `{batch}`. These current
ephemeral views seed subscribers, and durable tree/thread/chat/inbox feeds keep their sequence repair.

| Module | Planned responsibility |
|--------|------------------------|
| [frames](src/frames.rs) | Authentication, subscription, sync, delta and event frames. |
| [feeds](src/feeds.rs) | Durable and ephemeral feed identifiers, an environment's own feed and a document's among them. |
| [foreign](src/foreign.rs) | Row types that belong to the HTTP contract, carried as JSON until it defines them. |
| [sync](src/sync.rs) | Sequence and reload contracts. |
| [delta](src/delta.rs) | Text and output offsets. |
| [documents](src/documents.rs) | A document's feed: its reference, the blocks that changed by sequence, and its discussions as they are opened, answered and resolved. |
| [presence](src/presence.rs) | Visible readers. |
| [typing](src/typing.rs) | Typing notices in chats, and who is writing in which block of a document. |
| [pacing](src/pacing.rs) | How often a side may speak: reports that are merged, and notices that are dropped. |
| [terminal](src/terminal.rs) | Raw terminal data and control frames; a shell taken up on another screen, and the replay it is taken up with. |
| [version](src/version.rs) | Protocol versions, limits and close codes. |
| [browser](src/browser.rs) | Browser socket frames, pictures and the acknowledgement that keeps only the newest image. |
