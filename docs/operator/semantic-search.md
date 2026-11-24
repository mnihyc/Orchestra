# Set up semantic search

Turn on search by meaning for an organization, give it an embeddings provider, and follow the index until it is ready. At the end people can find text with a similar meaning as well as the words they typed. Word search works throughout, with or without a semantic index.

People read about the controls and the index states in [Semantic search](../user/semantic-search.md). This page is for the operator who sets it up.

## Before you start

You need:

- **The shipped PostgreSQL image.** It includes pgvector 0.8.7 and the extension for Unicode substring search. Keep the existing data volume when you build or replace the image.
- **An embeddings model** that an API provider can serve: a model server your hosts can reach, or the Local embeddings preset below.
- **A person who answers for the cost.** The person who saves the Embeddings setting is the one whose permissions and limits are checked when work needs the model, and whose name the embedding usage carries.

A database without pgvector still starts. The semantic page then reports that the extension is missing or cannot be installed. Install it as your database policy allows, or ask the database operator to. Word search stays available meanwhile.

## 1. Add an embeddings provider

1. In **Resources → Providers**, add an API provider with an `openai-embeddings` endpoint. Its runtime must reach the base address; the API server needs no route to the model.
2. Give it a credential with the usual controls: a bearer key, a named header, no credential, headers, or a sealed secret header. Keep secrets in those controls, never in a URL or a log.
3. Turn the model on in the provider.

**Or use Local embeddings.** In **Add provider**, choose **Local embeddings**. When no readable template exists, Orch creates one from `images/orch-embeddings/Dockerfile`; a template you removed is never brought back, and the template is listed, editable and removable like any other. Its recipe pins and checks the llama.cpp build and the EmbeddingGemma 2 Q8_0 model; the build downloads the model file, and it is not kept in Git. The preset uses `http://127.0.0.1:8090/v1` inside the runtime, the model `embeddinggemma-2` and 256 dimensions. The template starts with four CPUs and 2 GiB; change them in the template if you need to. The build host must be able to fetch the recipe's public files. How fast the runtime starts and embeds decides when it is ready and how fast indexing goes.

## 2. Turn semantic search on

1. In **Management → Semantic search** (or on a root organization's settings), turn on `search.semantic.enabled`.
2. Save `search.semantic.embeddings` with the provider, the model and its dimensions. For EmbeddingGemma 2 use the query prefix `task: search result | query: ` and the document prefix `title: {title} | text: `, each with a trailing space. Other models need other prefixes, or none.

Creating the provider does not change search: only this setting does. The index belongs to a root organization and covers its tree, so set the keys on that root organization or at the system.

**Check that it works.** The table in **Management → Semantic search** shows the index of each organization. Expect **Counting…** first, then **Indexing** with a token count and the endpoint's measured rate, and finally **Ready**. Then search for a phrase in other words: a result marked **Similar meaning** shows that it works.

## 3. Watch the index

Counting finds the whole inventory of sources before any content is embedded, so it shows no percentage and no time left. Indexing reports tokens embedded and the measured rate. Empty and short content and source metadata still cost something, and the index can reach its disk share before every source has a vector.

The disk figure is an estimate of content, cuts, source rows and vectors across the generations that are kept. It is not a filesystem quota. Raising it is your choice. Turning search off stops its work but does not promise to give back the rows already written.

## If it does not work

Read the reason on the index row first. It names the cause: disk, provider or model, access, a limit, an offline machine, or a missing extension.

| You see | What to do |
|---|---|
| **Paused · Disk limit reached** | Raise `search.semantic.max_disk_share` (`null` removes the limit), choose a shorter vector, or change the indexing plan |
| **Provider switched off** or **Provider removed** | Turn the provider on again, or choose another embeddings provider. These are deliberate pauses: they add no model failure and no retry, and use already reported stays in the usage ledger |
| Paused for access, a cap or an offline machine | Repair the provider's runtime, key, offered model or the writer's permissions. Do not add a second endpoint or a second secret for search |
| pgvector missing | Use a database that has it |
| Search is slow | Measure words, meaning lookup and access checks separately; a cold index and cold metadata read more slowly than a warm one. Check query plans and cache pressure before you raise the query wait. `ORCH_LOG=orch::search::timing=debug` prints phase times and counts, never the query or the content |

When the provider is switched off, indexing pauses at once and cancels commands already admitted, even ones not yet issued, so turning it on again cannot revive that unfinished request. Stored vectors stay. A query vector still in the cache can use them; a new query uses words alone while the provider is off. Search labels the paused index and keeps the current filters and permissions. Turning the provider on resumes indexing by itself. Start a new search afterwards to include new work.

To attribute paid work, look at the Embeddings records in **Usage**.

## Change the model

1. Save the new `search.semantic.embeddings`. The old generation keeps answering while the new one builds.
2. If both generations do not fit the disk share, the row waits for an administrator's choice: **Rebuild** discards the current generation, and semantic results then cover what has been finished; **Keep current model** restores every part of the earlier choice at once, including prefixes and chunk size, within the usual locks.

New prefixes, another chunk size, a longer vector or a different model need new content embeddings. The same model served from another place is first checked against saved probes, and matching probes avoid embedding it all again. A compatible shortening (MRL) slices and renormalizes the content and the cached query vectors and then rebuilds the index. An old search cursor ends when its embedding space changes.

The index tables are derived data. Back them up and restore them with the rest of PostgreSQL. A rebuild may need one model call for each batch of content, so it takes time and billed use.

## Settings

All of these are ordinary settings; locks and current permissions apply.

| Key | Shipped value | Meaning |
| --- | --- | --- |
| `search.semantic.enabled` | false | Also retrieve results by meaning |
| `search.semantic.embeddings` | unset | Provider, model, dimensions and prefixes; the person who saves it answers for the use |
| `search.semantic.max_disk_share` | 100 | Estimated index size as a percent of the stored content it indexes; `null` removes the limit |
| `search.semantic.tuning` | registry object | Chunk and batch sizes, nearest candidates, scan limits, fusion and query wait |
| `search.page_size` | 20 | Results shown at first |
| `search.reorder_window_secs` | 5 | How long the list may settle while the reader has not started reading |
| `search.meaning_pause_ms` | 800 | Typing pause before meaning retrieval starts |
| `search.vector_cache_secs` | 300 | How long a query vector is reused, per organization and model generation |
| `search.word_candidates` | 50 | Candidates per field before the exact access check and ranking |
| `search.word_recent_candidates` | 200 | Recent sample before a text-index probe |
| `search.word_tuning` | scope rows 30,000; no tier overrides | Estimated scoped reads, and separate limits for title, description, heading and body |

**Word search.** It uses the planner's estimates from existing statistics and does not count rows per request, so keep automatic analyze on after content changes. The scope threshold chooses an entrance; it never grants access or limits the total results. A missing tier override inherits `search.word_candidates`.

**Meaning search.** `exact_filter_candidates` is 20,000, `max_scan_tuples` 20,000 and the scan memory 2. `iterative_scan` is `strict_order` by default; `relaxed_order` sorts the chosen candidates before returning them; `off` uses only the initial graph breadth. An omitted `ef_search` keeps candidates times oversampling (200 with the shipped values), including its growth on continued pages. An `exact_filter_candidates` of 0 uses the graph for every scope. The query wait is three seconds; words and meaning run in parallel over one response, and aborting it cancels both and any pending embedding command. Agents get one merged answer, or the words with a reason when meaning misses its budget. A phrase, a regular expression, a title-only search or **Newest** uses words alone. Writer, creator, last editor, owner, date, mention, file, sharing and place filters apply to both.

## Keep when you maintain the database

- Keep the text and recency indexes for title, description, heading and body, and the covering indexes by place or source that serve the recent candidates. Do not drop them while you maintain the semantic indexes.
- Keep the triggers and indexes that maintain the file-presence counters and media projections. The migrations fill them for existing content.
- Keep the whole-item vectors (`search_item_vectors`) and the reference index (`related_references`): the first holds each source's mean vector, with its own index, and the second holds typed links from documents, chat messages and thread items. Neither keeps names or permissions; current access is checked before anything is shown.

## Related items

The document inspector and agents' `search_shared.related_to` rank related items the same way. `related.weights`, `related.place_scores`, `related.count` and `related.candidates` inherit at the system, organization, project and topic, and can be locked. A count of 0 switches the list off. The defaults are described in [Related items](../user/related-items.md).

After you upgrade an existing index, the indexer fills the whole-item vectors in its usual batches, without new model calls. They are counted in the disk estimate before they are written, and a model change shortens and reindexes them with the rest. A change of generation never mixes old and new vectors. Opening **Related** makes no provider request. Its optional meaning lookup uses `search.semantic.tuning.query_wait_secs`; if it misses, the place and reference results stay.

To measure search on a large corpus, see the [corpus runner](../../scripts/search-scale/README.md).
