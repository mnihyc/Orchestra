---
title: Related items
screens: /doc/$documentId, /help, /help/$article
keywords: related, documents, chats, threads, meaning, place, references, weights
---

# Related items

## Open related items

Open **Related** in a document's inspector. It lists documents, chats and threads you can read.
Each row gives a short reason: the same place, similar meaning, or a reference between the items.
If both items reference another thing, the row says **Shared references**.
An empty list adds no rows.

Hover or focus a chip for its reference card. Click or press `Enter` to keep the card open.
Use **Open**, **Open beside** or **Copy reference** there. `Escape` closes the card.

The section reads one page when opened and reuses it for the document's current revision.
**More** asks for the next page.
A change to the document asks for a new answer. Cards check their targets through the ordinary reference preview.
Changing related settings or access refreshes an open list. Reopening an unindexed list checks meaning again.

## Meaning availability

**Meaning off** means semantic search, or its ranking weight, is off.
**Meaning not indexed** means the current document has no usable whole-item vector in the answering generation.
An empty document needs content before it can have one.
**Meaning unavailable** means vector storage or its lookup could not answer.
Place and references still work when meaning cannot contribute.

Related uses the stored vectors of whole items. Opening it makes no embedding request.
It never compares vectors from different generations or models.

## Choose ranking

The document's place inherits **Related weights**, **Related places**, **Related items** and **Related candidates**.
Managers can change these in the place's settings; a lock from above still applies.

The shipped weights are meaning `0.6`, place `0.25` and references `0.15`.
They are decimal fields from zero to one. A zero weight leaves that signal out.
Available weights are normalized, so turning meaning off keeps the other signals useful.

The shipped place scores are the same folder `1.0`, topic `0.7`, project `0.4` and org `0.1`.
A folder can be a document folder or a project inside another project.
Direct references score one; shared references use the overlap of a bounded window of distinct referenced things.
Repeating a link does not increase its score.

**Related items** shows eight rows by default. Set it to **Off** to hide the list.
**Related candidates** defaults to 200 candidates per signal and reference neighborhood.
It bounds the work before exact access checks; raise it when a larger pool is useful.

## Ask an agent

An agent can call `search_shared` with `related_to` instead of `query`:

```json
{"related_to":"docs/Rollout plan.md"}
```

An `id:` reference also works. `kinds`, `path` and `names` can narrow the list.
`max_results` chooses a page size; repeat `related_to` and the filters with `next_cursor` to continue.
A changed ranking or access ends the old cursor; start the list again.
Each result has `related_by`: `meaning`, `place`, `references`, or a combination.
The agent sees only its current shared reach, which can differ from your own reading access.
