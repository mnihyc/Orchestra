# Import and export limits

An import is the reverse of an export: it reads a zip, a tar (packed with gzip, bzip2, xz or zstd, or not), or loose files that a person
uploaded, checks them, and creates documents, places, chats and threads as that person. Its
limits are settings so an operator can raise or lower them without a release. Every number
here is a default that an operator may change.

## What bounds an import

| Setting | Where it can be set | Default | What it bounds |
|---|---|---|---|
| `import.max_bytes` | system, org, project | none | the bytes of the source one import into that place may hold once expanded |
| `import.max_items` | system, org, project | none | the places, folders, documents, chats and threads one import into that place may hold |
| `export.max_bytes` | system, org, project | 2 GiB | the largest archive one export may come to |
| `export.max_items` | system, org, project | 10000 | how many conversations, chats and documents one export may hold |
| `[limits] imports_per_user` | `config/orch.toml` | 2 | imports of one person that run at once; a third answers 429 `rate_limited` |

A `none` import limit follows the export limit of the same place, so that whatever an export
may hold, an import of that archive may bring back. Raising `export.max_bytes` therefore raises
the import limit with it, unless `import.max_bytes` was set on purpose. An import that would
pass a limit is not begun; the dry run says so (`import.too_large`, `import.too_many`) with the
limit and the size asked for.

The limits are read at the destination, where the import lands, not where the archive was
made. A person with only a little rights over a place sees the limits of that place.

## What an import protects

- **Expansion.** The dry run reads the archive's directory and counts bytes before opening any
  entry, and the job streams entries rather than unpacking to disk. An archive that claims more
  than `import.max_bytes` once expanded, or that has more entries than `import.max_items` allows,
  is refused with `import.too_large` or `import.too_many`. Entry names are checked: an absolute
  path, a `..` segment, or a name the archive repeats is refused (`import.invalid`).
- **Checksums.** A package's manifest lists the SHA-256 of every part; a part that does not match is
  reported as damaged and the thing it belongs to is not made. The rest of the package comes.
- **Rights.** Every object is made as the person who started the import, with the rights that person
  holds at the time of each batch. A system administrator's rights are not borrowed.
- **Idempotence.** Ids of what an import makes derive from the importer, the destination, the package
  and the source id, and each is recorded in `import_origins`. Running the same import again
  finds what the first made.

## Memory and work

- **A conversation or a chat of any size.** The record of a thread or of a chat is copied to a scratch file
  (checked against the hash the manifest gives it) and read from there a page at a time: two hundred items, or four
  mebibytes of the record, whichever comes first. The memory of an import does not grow with the history it brings,
  and no single record has a limit of its own; `import.max_bytes` bounds the whole.
- **The plan.** What the dry run shows and the job works through is held in memory while an import is planned: about
  three kilobytes for each thing. The default of 10,000 things is about 30 MiB; raising `import.max_items` raises it in
  proportion, so an operator who allows a million things allows about 3 GiB for the plan.
- **Undo import.** What an import made is counted by a few statements and taken back in batches of five hundred rows,
  each batch in a transaction of its own; the chats and threads an import added to are taken one at a time, under what
  the person may do in each at that moment. A job that stops goes on from what is still there.
- **The report.** It is read, shown and downloaded a page of rows at a time; a report of a million rows costs a page of
  memory.
- **A tar.** A tar has no index to find a file by, so one (packed with gzip, bzip2, xz or zstd, or not) is unpacked once into a
  scratch file, at most `import.max_bytes` of it (a tar that unpacks to more is refused), and its files are read from there
  by position; the copy is removed when the import lets go of the source. The unpacking of an xz may use up to 256 MiB,
  whatever the size of the file.
- **Zips inside a zip.** A zip that holds nothing but other zips (a Notion export does) has each of them taken out into a scratch
  file and opened, together at most `import.max_bytes`, and the entries of all of them count against the same claim as one archive's;
  several zips chosen together are read the same way without the copy. They are removed when the import lets go of the source.
- **Scratch space.** A record being read and a file being stored are copied to the `tmp/` directory of the storage root
  (the system's directory for temporary files where the engine has none); each copy is removed when its object is made or the
  job ends.

## Storage and cleanup

The report of an import is kept as long as its row in `imports`; a person's list is **Your imports**.
`import_origins` rows follow the things they name and are removed with them when they are purged
after the retention window. **Undo import** deletes what the import made the usual way (soft deletion
first), so the retention settings of deleted things apply to it. An uploaded source is a normal
upload and follows [uploads](uploads.md).

Import jobs show in the job lists of the system as kind `import`. A worker that stops mid-import
leaves the job to be adopted by another worker, which resumes from the batch it had last committed;
the created things of a finished batch are never made twice.


## Agent document imports

`edit_document import` reads a workspace directory or zip as a background job, including its planning phase.
The first call waits at most 20 seconds, then returns planning or import progress. Poll with the same path and file;
`cancel: true` stops at a file boundary and keeps committed documents, and `resume: true` rechecks access and reuses
those identities. The job is process-local; after a restart, repeat the original call to replan and skip existing documents.

The engine admits two agent imports at once, before any source read, so the 256 MiB converter sandboxes and archive
buffers cannot multiply with the number of agents. Further starts return a limit refusal without queuing. It retains
at most 64 receipts for ten minutes, with at most 200 results and 1 MiB of diagnostics each; polling lists the latest
60 and reports all omitted results. Completed receipts may be evicted earlier when the receipt pool fills.

A plan has at most 100,000 paths and 16 MiB of path text; larger directories must be split into subdirectories.
Archives check both declared and actual expanded bytes against `sharing.snapshot_max_bytes`. The existing document
and file size settings still apply to each conversion and stored file. One file is converted at a time per job;
its neighbour map is borrowed, and existing folders are found by an indexed name lookup.

Orch Markdown layout comments use the same reader and a closed JSON schema. Each comment is bounded to
256 KiB, four times the existing block allowance for caption markup, comment escaping and cell coordinates.
Pairing is linear and permits at most 48 annotation/native-container layers for sixteen document levels.
Invalid layout leaves ordinary content visible; restoration refuses to hide text inserted into a covered table cell.
No block, file, author or history identities are stored in layout metadata.

A large document edit keeps one live block skeleton for its transaction. Each later chunk reads only its named
blocks and content-changing rows for exact byte totals. A stale block or a limit refusal rolls back the complete
edit, including earlier chunks; splitting the validation never turns a partial edit into an accepted one.
