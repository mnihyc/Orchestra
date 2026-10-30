---
title: Semantic search
screens: /search, /admin/$section, /org/$slug/settings/$section, /resources/providers, /resources/providers/$id, /resources/templates, /resources/templates/$id
keywords: semantic search, embeddings, Local embeddings, index, disk limit, model, rebuild, managed, 语义搜索, 嵌入, 索引
---

# Semantic search

Semantic search also finds text with a similar meaning.
It is off by default. Words continue to work while it is off, unavailable or still indexing.
[Finding things](finding-things.md#words-and-similar-meaning) explains the search list.

## Turn on semantic search

An admin opens **Management → Semantic search**.
The table shows each org's index. The settings below it are the system defaults.
An org can have its own settings and inherited locks.
Turn on **Semantic search**, then name its **Embeddings**.
The person who names the provider answers for its use and cost.
That person must keep permission to use the provider and model. Shared usage caps still apply.

## Choose embeddings

Choose the provider first, then an enabled model and its dimensions.
The provider needs an Embeddings endpoint that its runtime can reach.
Its credential, headers and secret headers are the provider's ordinary ones.
**Not set** means no embeddings provider and model have been chosen.
**Managed** means the setting uses a provider you may not view. It is not a failure.
A lock is shown by the normal settings row.

Query and document prefixes are part of this model choice.
Use the prefixes the model expects. `{title}` in a document prefix becomes the source's title, or `none`.
Changing prefixes rebuilds the index.

## Local embeddings

**Add Local embeddings** opens the ordinary **Add provider** dialog with that example selected.
It fills the Embeddings protocol, the runtime's loopback address, `embeddinggemma-2`, 256 dimensions,
and its runtime template. Choose the home, machine and runtime as for any provider, then create it.
The Embeddings editor is offered afterwards. Save the setting separately to use the provider for search.
Creating a provider does not change that setting.

The shipped model can use these prefixes. End each one with a space:

- Query: `task: search result | query:`
- Document: `title: {title} | text:`

**Template · Local embeddings** opens the template used by the provider.
It is also in **Resources → Templates**. Its Dockerfile is shown there and can be edited.
It can be removed under the same rules as any template. Adding the example after removal makes a fresh
one; it does not restore a removed template. A live template keeps the edits you made to it.
[Add a provider](add-a-provider.md#local-embeddings) explains the ordinary setup.

## Index states

| State | What to do |
| --- | --- |
| Not set up | Choose Embeddings, or use Add Local embeddings |
| Counting… | The index is discovering the text; there is no whole-index percentage or time yet |
| Indexing | The Meter shows embedded tokens; the row gives tokens or estimated time left |
| Ready | The current index is caught up |
| Paused · Provider switched off | Enable the provider to resume indexing |
| Paused · Provider removed | Choose another embeddings provider |
| Paused · Disk limit reached | Raise the limit, choose a smaller vector length, or change the indexing plan |
| Needs a choice · New model | Read the new index's estimated disk and time, then choose Rebuild or Keep current model |
| pgvector missing | Ask the operator to use a database with pgvector |

A provider, model, access, cap or machine problem is shown on the same row.
Words stay available. An unavailable source in Search keeps its reason on hover, focus and tap.
An initial index starts answering from its first finished vectors; it does not claim the whole corpus is ready.

### Provider paused

Disabling the embeddings provider cancels its current batch and pauses indexing.
The progress line says **Provider switched off**. It is a pause, with no retry or failure count.
Enabling the provider resumes indexing automatically. Removing it says **Provider removed**.
The vectors already stored stay in place.

Search can use a query vector that is still cached. It keeps meaning results and says **Index paused**.
A fresh query that needs the stopped provider uses words alone, with the reason on **Similar meaning unavailable**.
Start a new search after the provider resumes to include newly indexed content.

## Disk limit

**Disk limit** is the index's size limit as a share of the stored content it indexes.
It ships at **100 %**. Empty means **No limit**. Each index row names the estimated disk used and allowed.
Indexing pauses before its next allocation would pass the limit.
A small index may need more bytes than its text. Check the estimates before increasing the limit.

## Change models

A replacement is built while the current model answers.
New prefixes, a different chunk size, a longer vector or a different model need new content vectors.
The same model served elsewhere is checked with stored probe texts first.
A compatible shorter MRL vector is shortened and indexed again without re-embedding all content.

If the disk limit cannot hold both generations, the row asks for a choice first.
**Rebuild** removes the current index and starts the requested one; meaning covers completed pieces meanwhile.
**Keep current model** restores the provider, model, dimensions, prefixes and chunk size that made the current index.
The ordinary setting permissions and locks still apply.
An old search cursor cannot switch embedding spaces midway. Start a new search after the change.

## Indexing and ranking

**Indexing and ranking** controls chunk size, batches, candidate windows, scan budgets and the query wait.
These are defaults you can change, not permission grants.
**Word retrieval** chooses scoped reads for a smaller accessible set. Optional limits for names and titles,
descriptions, headings and files, and body inherit **Word candidates** when absent.
**Windows per request** defaults to one. **Show more** continues from that window, including when
its candidates were all hidden by current access. Broader access trees use the same bounded windows.
Search keeps the same current permission checks before returning a result or snippet.

Meaning compares a small scope's vectors exactly. Broader scopes use the graph.
**Graph scan** defaults to **Distance order**. **Graph breadth** is automatic when unset:
the candidate count multiplied by oversampling. **Exact filter candidates** at **0 (Off)** uses the graph for every scope.
The indexer starts with newer content. Progress and time estimates depend on the endpoint's measured rate.
Meaning waits only for its configured query budget; on a timeout, words remain.
Agent search returns one combined answer, or words with an unavailable reason when meaning cannot answer.
