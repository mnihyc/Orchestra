# orch-blob-local

Local directory implementation of content-addressed storage.

Implemented: the blob contract's store over a directory. A blob is a read-only file at
`<root>/blobs/<hex[0:2]>/<hex[2:4]>/<hex>`; a write goes to a temporary file below `<root>/tmp`, is checked against the hash and
the size it claims while it passes, and is linked into place in one step, so that two writers of the same content both succeed with
one file and a write that is cut off leaves nothing. Reads are streams, whole (checked against the name as they pass) or by a range.
Tested against real directories: refused hashes and sizes, every range against a read of the whole, sixteen concurrent identical
writes, abandoned and failed writes, damaged files, and the clearing of what a crash left.

| Module | Planned responsibility |
|--------|------------------------|
| [store](src/store.rs) | Immutable hash-addressed files. |
| [ranges](src/ranges.rs) | Streaming range reads. |
| [writes](src/writes.rs) | Verified atomic content insertion and deletion. |
