---
title: Document links
screens: /doc/$documentId, /help, /help/$article
keywords: links, backlinks, referenced by, references, graph, mentions, related, tabs, cited by
---

# Document links

## Read links

Open **Links** in a document's inspector. It has four tabs.

- **Referenced by** lists the documents, chats, threads and comments that mention this document. Each row is one source with its place and how many mentions it holds. Open a row to read the line around each mention, with the reference in bold, who wrote it and when.
- **References** lists what this document points to, in a stable order by kind and identity. Something you may not open stays a locked chip with the words its author wrote; open its card to ask for access.
- **Related** is the list of items related by meaning, place and references. See [Related items](related-items.md).
- **Graph** draws the readable things that refer to each other around the document.

A tab shows the rows read so far, and a plus where more follow. A source that mentions the document more often than a count goes on reads, for example, "100 or more mentions". Nothing counts, names or hints at a source you may not read: no number includes it, and there is no "and more you cannot see".

On a narrow bar the first two tabs read **Cited by** and **Cites**. The full words are in their tooltips.

## Jump to a mention

Press a line to open its source at that block, comment, message or turn. A source with more mentions than a page holds ends its list with **Show more**.

Every list in Links grows the same way. **Show more** is always at its foot, and the next page also loads when the foot scrolls into view. The button stays for the keyboard and when a page failed.

## Open the graph

The Graph tab draws the document and up to four of its nearest linked items, one step out, so their names fit in the small view. Press **Open graph** for the full view in the main area, including the remaining links.

- **Depth** goes from one to three steps.
- **Kinds** limits the graph to documents, chats or threads.
- **Related by meaning** adds dashed lines to what is alike in meaning. It needs semantic search.
- A click on a node makes it the centre. `Enter` opens it.
- Names use the free space beside nodes. Point at a node for its full name; the Nodes list keeps every name even where the picture has no room for it.
- The **Nodes** list holds the same nodes for the keyboard and for screen readers: the arrow keys move, `Space` re-centres and `Enter` opens.

A line goes between two readable things only. If a thing in between is hidden from you, the things on either side are not joined through it. A graph holds at most 150 nodes by default and says when it left one out.

## Switch tabs off

Whoever manages a document can choose which tabs it shows: open the document's menu and choose **Links tabs…**. A tab that is off disappears for every reader, and the whole section is not offered when all four are off. This changes what is displayed only. It does not change what the lists, the graph or an agent's tools return.

A place can set the default for the documents in it: **Links tabs** under Chat and documents in its settings. A lock on that setting decides for every document below, and the dialog then shows its boxes switched off.

Changing or resetting a document's choice is recorded in its history with who changed it and when.

## Ask an agent

An agent can call `search_shared` with `referenced_by` instead of `query`:

```json
{"referenced_by":"docs/Rollout plan.md"}
```

An `id:` reference also works, and `kinds`, `path`, `names` and `max_results` narrow the list. Each result is a document, chat or conversation the agent may read, with its path, its mentions and the lines around the first of them, the reference in square brackets. A document's comments count as part of it. The agent sees only what its own reach allows, which can differ from yours.

Continue with `next_cursor` and the same reference and filters until there is no cursor. A page may be empty when its access-check budget ended. Each call checks the agent's current shares again. After an engine restart, start the list again if its cursor is refused.

## Choose the numbers

Each number is a default of a setting that a place can change, and none is a limit of the product.

- **Links page size**: rows before **Show more**. 20 by default.
- **Mentions per source**: mentions listed under one source before **Show more**. 5 by default.
- **Mentions counted**: mentions counted in one source before its count reads "or more". 100 by default.
- **Mention line length**: characters of the line around a mention. 140 by default.
- **Links access checks**: sources whose reading access is checked for one page. A very popular document goes on with a cursor. 500 by default.
- **Graph nodes**: the most nodes a graph draws. 150 by default.
