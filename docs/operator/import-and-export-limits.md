# Set import and export limits

Choose how large an import or an export may be, and how much memory and disk you give them. At the end the limits fit your hosts, and you know what an import refuses and what it keeps.

People import and export from the screens: see [Import and export](../user/import-and-export.md) and [Move a project between orgs or deployments](../user/move-a-project-between-orgs.md). This page is for the operator.

An import reads a zip, a tar (plain or packed with gzip, bzip2, xz or zstd), or loose files that a person uploaded, checks them, and creates documents, places, chats and threads as that person. An export writes an Orch package that an import can read back. The limits are settings, so you can change them without a release.

## Before you start

You need to be a system administrator, or hold **Manage** on the organization or project whose limits you set. Every number below is a default that you may change.

## 1. Choose the limits

| Setting | Where it can be set | Default | What it bounds |
|---|---|---|---|
| `import.max_bytes` | system, organization, project | none | The bytes of the source that one import into that place may hold once expanded |
| `import.max_items` | system, organization, project | none | The places, folders, documents, chats and threads that one import into that place may hold |
| `export.max_bytes` | system, organization, project | 2 GiB | The largest archive one export may come to |
| `export.max_items` | system, organization, project | 10,000 | How many threads, chats and documents one export may hold |
| `[limits] imports_per_user` | `config/orch.toml` | 2 | Imports of one person that run at once; a third gets 429 `rate_limited` |

1. In the settings of the system, organization or project, set the limits you want. Lock them only when needed.
2. Check the result in the review of an import: it shows the limits in force and what blocks.

A limit of `none` for an import follows the export limit of the same place, so whatever an export may hold, an import of that archive can bring back. Raising `export.max_bytes` raises the import limit with it unless you set `import.max_bytes` on purpose. The limits are read where the import lands, not where the archive was made, so a person with few permissions over a place sees that place's limits.

An import that would pass a limit never begins. The dry run says so with `import.too_large` or `import.too_many` and shows the limit and the size asked for.

## 2. Size the memory and the disk

- **The plan.** What the dry run shows and the job works through is held in memory while the import is planned, about 3 KB for each thing. The default of 10,000 things is about 30 MiB. Allowing a million things allows about 3 GiB for the plan.
- **Threads and chats.** The record of a thread or chat is copied to a scratch file, checked against the hash in the manifest, and read from there a page at a time (200 items or 4 MiB, whichever comes first). Memory does not grow with the history an import brings, and no single record has its own limit: `import.max_bytes` bounds the whole.
- **A tar.** A tar has no index, so it is unpacked once into a scratch file of at most `import.max_bytes` (a tar that unpacks to more is refused), and its files are read from there by position. The copy is removed when the import lets go of the source. Unpacking an xz can use up to 256 MiB of memory, whatever the file's size.
- **Zips inside a zip.** A zip that holds only other zips (a Notion export does) has each taken out into a scratch file and opened, together at most `import.max_bytes`. Their entries count against the same limit as one archive's. Several zips chosen together are read the same way without the copy.
- **Scratch space.** Records being read and files being stored are copied to the `tmp/` directory of the storage root (the system's temporary directory where the engine has none). Each copy is removed when its object is made or the job ends.
- **Undo and reports.** Undo counts what an import made with a few statements and takes it back in batches of 500 rows, each in its own transaction. Reports are read, shown and downloaded a page of rows at a time, so a report of a million rows costs one page of memory. A browser without direct file saving cannot download a report from the screen, but the report stays readable in pages.

## What an import protects

- **Expansion.** The dry run reads the archive's directory and counts bytes before it opens an entry, and the job streams entries instead of unpacking to disk. An archive that claims more than `import.max_bytes` once expanded, or has more entries than `import.max_items` allows, is refused. An absolute path, a `..` segment or a repeated entry name is refused (`import.invalid`).
- **Checksums.** A package's manifest lists the SHA-256 of every part. A part that does not match is reported as damaged and the thing it belongs to is not made; the rest of the package comes.
- **Permissions.** Every object is made as the person who started the import, with the permissions that person holds at the time of each batch. A system administrator's permissions are not borrowed. Restoring a package's sharing needs access management at the destination and follows its locks.
- **Repeats.** The ids of what an import makes derive from the importer, the destination, the package and the source id, and each is recorded in `import_origins`. Running the same import again finds what the first one made.

## What is kept

- A failed or cancelled import keeps its partial results and report. The report lasts as long as its row in `imports`, and a person's list of reports is **Your imports**.
- **Undo import** deletes what the import made in the usual way, by soft deletion first, so the retention settings of deleted things apply. The plan shows when the affected work can be brought back. Undo is unavailable when the affected work or its access cannot be checked.
- `import_origins` rows follow the things they name and are removed with them when they are purged after the retention window.
- An uploaded source is a normal upload and follows [Uploads and private drafts](uploads.md).
- Import jobs appear in the system's job lists as kind `import`. If a worker stops in the middle of an import, another worker adopts the job and resumes from the last batch committed. The things of a finished batch are never made twice.

## Limits of an agent's document import

An agent that runs `edit_document import` reads a workspace directory or zip as a background job, planning included.

- The first call waits at most 20 seconds and then returns progress. The agent polls with the same path and file. `cancel: true` stops at a file boundary and keeps the documents already committed; `resume: true` checks access again and reuses their identities.
- The job lives in one process. After a restart the agent repeats the original call, which plans again and skips documents that exist.
- Two agent imports run at once, admitted before any source is read, so the 256 MiB converter sandboxes and archive buffers do not multiply with the number of agents. Further starts get a limit refusal and are not queued.
- Receipts: at most 64, kept for ten minutes, each with at most 200 results and 1 MiB of diagnostics. Polling lists the latest 60 and reports how many results were left out. A completed receipt can be dropped earlier when the pool is full.
- A plan has at most 100,000 paths and 16 MiB of path text. Split a larger directory into subdirectories.
- An archive's declared and actual expanded bytes are both checked against `sharing.snapshot_max_bytes`. The document and file size settings still apply to each conversion and stored file. One file is converted at a time in a job.
- An edit to one large document is all or nothing: a stale block or a limit refusal rolls back the complete edit, earlier chunks included.

## If it does not work

| You see | What to do |
|---|---|
| `import.too_large` or `import.too_many` in the dry run | Raise `import.max_bytes` or `import.max_items` (or `export.max_bytes` if the import follows it) at the destination, or import a smaller part |
| `import.invalid` | The archive has an absolute path, a `..` segment or a repeated name. Make the archive again |
| 429 `rate_limited` | The person already has `imports_per_user` imports running. Wait for one, or raise the limit in `config/orch.toml` |
| A part is reported as damaged | The part does not match its manifest. Export again; the rest of the package still comes |
| Undo is unavailable | The affected work, or the access to check it, is gone. Read the reason on the report |
