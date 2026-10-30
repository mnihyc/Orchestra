# Document links

The Links section of a document shows what refers to it, what it refers to, what is related to it and the graph around it.
The [screen guide](../user/document-links.md) explains the tabs. This page is for whoever runs the system.

## Where the references come from

The section reads one table of edges, `related_references`. Database triggers keep it current: a block of a document, a
comment on a document, a message of a chat and an item of a thread each replace their edges whenever their content
changes, and remove them when they are deleted. Nothing is polled and no model request is made.

- Migration `0038_document_links.sql` lets comments be sources and refills the edges of every live comment once, in the
  statement that applies it. A large system with many comments pays for that in the update window, not later.
- A thread's superseded or cut history is never a source. Only the items its current view holds, unchanged since the
  view froze them, count.
- Text that was edited so that it no longer refers to a document stops being a source at once.

## Settings

All seven are under Chat and documents, at system, org, project and topic scope. A lower place replaces the value above it,
and a place can lock it. Each is a default, not a cap.

| Key | Shipped value | Meaning |
| --- | --- | --- |
| `documents.links.shown` | all four tabs | The tabs a document's readers see: Referenced by, References, Related, Graph. A document's owner or manager can choose its own set unless a lock above sets this key. Display only: switching a tab off changes nothing that the API or an agent's `search_shared` returns. |
| `documents.links.page_size` | 20 | Rows in the first page of a list before Show more. |
| `documents.links.occurrences` | 5 | Mentions listed under one source in Referenced by before Show more. |
| `documents.links.mentions_counted` | 100 | Mentions counted in one source before its count reads "or more". The count stops there, so that a chat that cites a document a hundred thousand times costs no more than one that cites it a hundred times. |
| `documents.links.line_chars` | 140 | Characters of the line shown around a mention. |
| `documents.links.access_checks` | 500 | Candidate sources whose reading access is checked for one page or one graph. |
| `documents.links.graph_nodes` | 150 | The most nodes a graph holds. |

## What a reader can and cannot see

A source, a target or a node appears only if the reader may read it at the moment of the request. A source the reader
may not read leaves no count, name, line, identity or "and N more", and no edge is drawn through it in the graph.
The cursors of the lists are sealed under the deployment key and bound to their reader, their document and their list,
so a cursor held by one person opens nothing for another and a position that fell on a hidden source tells its holder
nothing. Rotating the deployment key makes every cursor already handed out invalid; a list then starts from its first page.

The agent's `referenced_by` lists also page with sealed cursors, bound to the thread, document and filters. Their
transient key lives in the engine process; after a restart an agent starts the list again. No cursor registry grows
with the number of searches. The tab override's change and reset are document history lines, committed with the choice.

## Cost and tuning

A page of Referenced by is one range scan of the edge table by source identity, narrowed in SQL by the reader's scopes
before the exact checks run. When a document is referred to from very many places the readers can mostly not read,
a page may return fewer rows than asked for together with a cursor: it stopped when `documents.links.access_checks`
candidates had been examined. Raise the setting for a place where readers are expected to see most sources; lower it
where the checks are costly.
The graph examines at most `documents.links.access_checks` candidates in all and holds at most
`documents.links.graph_nodes` nodes; when a readable node was left out for size the answer says so and the screen shows
"Showing the first N nodes".

Outgoing pages select target keys from the per-source counters before reading their first occurrence.
Migration `0059_document_reference_storage.sql` backfills the internal-marker flag and occurrence timestamps in
batches of 1,000 rows under a transaction lock. Existing item/history values and target counts stay intact.
The flag replaces payload serialization during conversation-source checks; an ordered occurrence index keeps
a heavily cited target from making its first anchor expensive. Payload and source-time changes maintain both projections.

## Diagnosis

- A tab is missing for everyone: read `documents.links.shown` at the document's place; a lock names the place that holds it.
  The document's own choice is the row of `document_links_display`, and `PUT /documents/{id}/links` with `null` removes it.
- A source that should be listed is not: the reader may not read it, or its text does not hold the reference any more.
  Compare with `related_references` for the target document's identity.
- A list that stops short with Show more: see the access checks above.
