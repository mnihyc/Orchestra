# Tune document links

Choose how many mentions, rows and graph nodes the **Links** section of a document shows, and how much work it may do to check what a reader may see. At the end the section answers fast on your installation and shows each reader only what they may read.

The **Links** section shows what refers to a document, what it refers to, what is related to it and the graph around it. People read it as described in [Document links](../user/document-links.md). This page is for whoever runs the system.

## Before you start

You need permission to change settings at the place you tune (system, organization, project or topic). Nothing here needs a migration or a model request.

## Where the references come from

The section reads one table of references. Database triggers keep it current: a block of a document, a comment on a document, a message of a chat and an item of a thread each replace their references when their content changes and remove them when they are deleted. Nothing is polled and no model request is made.

- The update that introduced comments as sources fills the references of every live comment once, as part of applying it. A large system with many comments pays for that in the update window, not later.
- A thread's superseded or cut history is never a source. Only the items its current view holds, unchanged since the view froze them, count.
- Text that was edited so that it no longer refers to a document stops being a source at once.

## 1. Set the limits

All seven settings are under **Chat and documents**, at system, organization, project and topic scope. A lower place replaces the value above it, and a place can lock it. Each is a default, not a cap.

| Key | Shipped value | Meaning |
| --- | --- | --- |
| `documents.links.shown` | all four tabs | The tabs a document's readers see: **Referenced by**, **References**, **Related**, **Graph**. A document's owner or manager can choose its own set unless a lock above sets this key. It changes only what is shown: switching a tab off changes nothing that the API or an agent's `search_shared` returns |
| `documents.links.page_size` | 20 | Rows in the first page of a list before **Show more** |
| `documents.links.occurrences` | 5 | Mentions listed under one source in **Referenced by** before **Show more** |
| `documents.links.mentions_counted` | 100 | Mentions counted in one source before its count reads "or more". The count stops there, so that a chat that cites a document a hundred thousand times costs no more than one that cites it a hundred times |
| `documents.links.line_chars` | 140 | Characters of the line shown around a mention |
| `documents.links.access_checks` | 500 | Candidate sources whose reading access is checked for one page or one graph |
| `documents.links.graph_nodes` | 150 | The most nodes a graph holds |

1. Open the settings of the place and find the key.
2. Change it, and lock it only if lower places must not change it.

## What a reader can and cannot see

A source, a target or a node appears only if the reader may read it at the moment of the request. A source the reader may not read leaves no count, name, line, identity or "and N more", and no edge is drawn through it in the graph.

The cursors of the lists are sealed under the deployment key and bound to their reader, their document and their list. A cursor held by one person opens nothing for another, and a position that fell on a hidden source tells its holder nothing. Rotating the deployment key makes every cursor already handed out invalid, and a list then starts from its first page. The agent's `referenced_by` lists page the same way, with cursors bound to the thread, the document and the filters; their key lives in the engine process, so after a restart an agent starts the list again.

## Tune the cost

A page of **Referenced by** is one range scan of the references by source, narrowed in SQL by the reader's scopes before the exact checks. When a document is referred to from very many places that readers mostly cannot read, a page may return fewer rows than asked for together with a cursor: it stopped when `documents.links.access_checks` candidates had been examined.

- Raise `documents.links.access_checks` for a place where readers are expected to see most sources. Lower it where the checks are costly.
- The graph examines at most `documents.links.access_checks` candidates in all and holds at most `documents.links.graph_nodes` nodes. When a readable node was left out for size, the answer says so, and the screen shows "Showing the first N nodes".

## If it does not work

| You see | What to do |
|---|---|
| A tab is missing for everyone | Read `documents.links.shown` at the document's place. A lock names the place that holds it. The document's own choice can be removed with `PUT /documents/{id}/links` and `null` |
| A source that should be listed is not | The reader may not read it, or its text does not hold the reference any more. Compare with the references table for the target document's identity |
| A list stops short and offers **Show more** | The access-check limit was reached: see the cost section above |
