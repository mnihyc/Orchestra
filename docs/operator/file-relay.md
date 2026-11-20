# File relay

File content that passes through Orch is carried, not kept. A file a person uploads into an environment, a file an
environment's machine sends to another machine in a move, a fork, a new environment or a re-creation, and a file a person
opens or downloads: the server is where the two ends meet, and its copy goes when the transfer ends. This page says what
that means for the server's disk, what a person can no longer do, how to keep the old behaviour where it is wanted, and
what changes at the update.

## The setting

**File relay** (`storage.relay_transfers`) is a setting of the Workspace group. It is on by default and shown as
**Relayed** or **Cached**. It follows the ordinary rule of settings: the system, an org and a project each may choose
for themselves, and the nearest place that chose wins. An operator who wants the guarantee for the whole installation
sets it at the system and locks it; a locked system value cannot be overridden below. An operator who wants the
file cache back sets **Cached** at the system, or lets one org or project choose it for itself.

It is read for the environment's place, at the moment a transfer ends and by the eviction job. A switch is felt by the next
transfer, not by one in flight.

## What the server keeps, and what it does not

| Kept | Not kept |
|---|---|
| The list of an environment's tree (paths, sizes, times, hashes), so that the files view and moves can list while a machine is away | A copy of a file that was opened, downloaded, previewed or saved |
| Snapshots, which are kept on purpose and are the server's only copy of a file | A copy of a file that a move, a fork, a new environment or a re-creation carried between machines |
| Attachments, documents, exports and everything else that belongs to a chat, a document or a person | The copy of a file a person uploaded into an environment, once it is written |

A transfer between machines still passes through the server's store, because two machines are never connected to each other.
The store holds what the transfer brought for as long as the transfer runs, and removes it, row and file, when the transfer
ends, whether it succeeded, failed or was cancelled. Content that something else holds stays: a snapshot, an attachment, a
document or a model input keeps the file it names, and so does another transfer that is still under way: the content its
machine uploaded, or was told the system holds, is held for its job from that moment until the job ends, so that the end of
one transfer does not take from another what it carries in common.

Browser reads, downloads and saves stream between the browser and the environment through the server’s memory. The server can describe a file and serve ranges without keeping its bytes on disk. A private upload that a person explicitly retained can supply another write.

## What a person loses

* A file of an offline environment that was opened before can no longer be opened; the listing remains, and the file opens when
  the environment is back.
* Opening a file a second time moves it a second time.
* **Recovery of a lost environment can restore only the files the server holds.** The server's copy of opened files was its only
  backup of files nobody had put in a snapshot. Take snapshots of what must survive the loss of a machine. The recovery plan
  states, before anyone confirms, how many files are not on the server and will be listed as missing, and each thread's
  directory receives the full list in `.orch/handoff/missing-files.txt`.

## At the update

The first run of the cache eviction job after the update (it starts with the server and then runs every ten minutes) forgets
the cached versions of every environment whose place relays, and removes the content nothing else holds. It does this in
batches, so a large cache can take several runs. `storage.env_cache_quota_bytes` sets the file cache size for places that choose **Cached**. Switching a place from **Cached** to **Relayed** later has the same effect.

## Disk and memory

Allow disk space for machine-to-machine transfers that are running at the same time. Temporary content is eligible for removal once its transfer ends and no independent owner holds it. Browser file relays use the system’s **File relay concurrency** setting to bound the memory used across simultaneous streams.
