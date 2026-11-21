---
group: settings
title: Semantic search
screens: /search, /admin/$section, /org/$slug/settings/$section, /resources/providers, /resources/providers/$id, /resources/templates, /resources/templates/$id
keywords: semantic search, embeddings, Local embeddings, index, disk limit, model, rebuild, managed, 语义搜索, 嵌入, 索引
summary: Choose embeddings, follow whether the index is ready, and keep word search available while you change them.
---

# Semantic search

Semantic search also finds text that has a similar meaning. It is off by default. Word search keeps working while semantic search is off, unavailable or still indexing. [Finding things](finding-things.md#words-and-similar-meaning) explains the list of search results.

## Turn on semantic search

1. An administrator opens **Management → Semantic search**. The table shows the index of each organization. The settings below it are the defaults of the system. An organization can have its own settings and inherited locks.
2. Turn on **Semantic search**, then name its **Embeddings**.

The person who names the provider answers for its use and its cost. That person must keep the permission to use the provider and the model, and the shared limits on usage still apply.

## Choose embeddings

Choose the provider first, then a model that is turned on and its dimensions. The provider needs an Embeddings endpoint that its container can reach. Its credential, its headers and its secret headers are the ordinary ones of the provider.

- **Not set** means no provider and model for embeddings has been chosen.
- **Managed** means the setting uses a provider you may not see. It is not a failure.
- A lock shows in the usual row of the setting.

The prefixes for queries and for documents are part of the choice of model. Use the prefixes the model expects. In a prefix for documents, `{title}` becomes the title of the source, or `none`. Changing the prefixes rebuilds the index.

## Local embeddings

**Add Local embeddings** opens the usual **Add provider** dialog with that example selected. It fills in the Embeddings protocol, the loopback address of the container, `embeddinggemma-2`, 256 dimensions and the template of its runtime. Choose the home, the machine and the runtime as for any provider, then create it. The Embeddings editor is offered afterwards. Save the setting on its own to use the provider for search. Creating a provider does not change that setting.

The model that comes with Orch can use these prefixes. End each one with a space:

- Query: `task: search result | query:`
- Document: `title: {title} | text:`

**Template · Local embeddings** opens the template of the provider. It is also in **Resources → Templates**, where its Dockerfile is shown and can be edited. It can be removed under the same rules as any template. Adding the example after a removal makes a new one and does not bring back the template that was removed. A template that exists keeps the edits you made to it. [Add a provider](add-a-provider.md#local-embeddings) explains the usual setup.

## Index states

| State | What to do |
|---|---|
| Not set up | Choose Embeddings, or use **Add Local embeddings** |
| Counting… | The index is finding the text. There is no percentage for the whole index and no time yet |
| Indexing | The meter shows the embedded tokens, and the row gives the tokens or the estimated time left |
| Ready | The current index has caught up |
| Paused · Provider switched off | Turn the provider on to resume indexing |
| Paused · Provider removed | Choose another provider of embeddings |
| Paused · Disk limit reached | Raise the limit, choose a shorter vector, or change the plan for indexing |
| Needs a choice · New model | Read the estimated disk and time of the new index, then choose **Rebuild** or **Keep current model** |
| pgvector missing | Ask the operator to use a database that has pgvector |

A problem with a provider, a model, access, a limit or a machine shows on the same row. Word search stays available. A source in **Search** that is not available keeps its reason on hover, focus and tap. A first index starts to answer from its first finished vectors, and it never claims that the whole corpus is ready.

### Provider paused

Turning the provider of embeddings off cancels its current batch and pauses indexing. The progress line says **Provider switched off**. It is a pause, with no retry and no count of failures. Turning the provider on resumes indexing by itself. Removing it says **Provider removed**. The vectors already stored stay where they are.

Search can use a query vector that is still cached. It keeps results by meaning and says **Index paused**. A new query that needs the stopped provider uses words alone, with the reason on **Similar meaning unavailable**. Start a new search after the provider resumes to include the content that was indexed since.

## Disk limit

**Disk limit** is the size limit of the index, as a share of the stored content that it indexes. It starts at **100 %**. Empty means **No limit**. Each row of an index names the disk used and allowed, as an estimate. Indexing pauses before its next allocation would pass the limit. A small index can need more bytes than its text. Check the estimates before you raise the limit.

## Change models

A replacement is built while the current model keeps answering. New prefixes, another size of chunk, a longer vector or a different model need new vectors of the content. The same model served from another place is checked first with texts of probes that were stored. A shorter vector that is compatible (MRL) is shortened and indexed again without embedding all the content again.

If the disk limit cannot hold both generations, the row asks you to choose first.

- **Rebuild** removes the current index and starts the one you asked for. Meanwhile, search by meaning covers the pieces that are finished.
- **Keep current model** restores the provider, the model, the dimensions, the prefixes and the size of chunk that made the current index.

The usual permissions and locks of the setting still apply. An old cursor of a search cannot switch between embedding spaces halfway. Start a new search after the change.

## Indexing and ranking

**Indexing and ranking** controls the size of chunks, batches, windows of candidates, budgets for scanning and the wait for a query. These are defaults you can change, and they grant no permission.

- **Word retrieval** chooses reads limited to the scope, for a smaller set that can be accessed. The optional limits for names and titles, descriptions, headings and files, and body take **Word candidates** when they are left out. **Windows per request** is one by default. **Show more** continues from that window, also when all its candidates were hidden by your current access. Larger trees of access use the same bounded windows. Search keeps the same checks of current permissions before it returns a result or a snippet.
- **Meaning** compares the vectors of a small scope exactly. Broader scopes use the graph. **Graph scan** is **Distance order** by default. **Graph breadth** is automatic when it is not set: the number of candidates times the oversampling. **Exact filter candidates** at **0 (Off)** uses the graph for every scope.
- The indexer starts with the newer content. The progress and the estimates of time depend on the rate measured for the endpoint.
- Meaning waits only for its budget for queries, and on a timeout the words remain. Search by an agent returns one answer that combines both, or the words with a reason, when meaning cannot answer.
