# Semantic search

Semantic search is off until an operator enables it and a person names Embeddings.
Words keep working without a semantic index or the vector extension.
The [screen guide](../user/semantic-search.md) explains controls and index states.

## Database

Use the shipped PostgreSQL image. It includes pgvector 0.8.7 and the native Unicode substring index extension.
Keep the existing PostgreSQL data volume when building or replacing that image.
A database without pgvector still starts; the semantic page reports that it is missing or cannot be installed.
Grant extension setup only according to your database policy, or have the database operator install it.
Ordinary word search remains available.

The semantic index belongs to a root org and covers its tree. Set its keys on that root org or at system scope.
The index uses half-precision chunks and a generation-specific cosine HNSW index.
Word title, description, heading and body candidates use their text and recency indexes.
Recent candidates also have covering indexes by place or source, so their text can be read after
the candidate window is bounded. Retain those indexes when maintaining search storage.
Do not remove those indexes when maintaining the semantic ones.

File-presence counters and media projections are maintained with ordinary content writes.
Retain their triggers and indexes during maintenance; canonical migrations populate them for existing content.

## Provider and accountable use

Add an ordinary API provider with an `openai-embeddings` endpoint. Its runtime must reach the base address;
the API server does not need a route to the model. Use the ordinary bearer key, named header, no-credential,
headers and sealed secret-header options. Keep secrets in those controls rather than a source URL or logs.
Enable the model in its provider, then save `search.semantic.embeddings` as the person who answers for its use.
Provider Use, offered/enabled models, allowed-model rules, subscription rules, current suspension and shared
caps are checked when work needs the model. Embedding usage has purpose `embeddings` and names that writer.
A private provider can be locked as infrastructure. Readers who cannot view it see Managed in setting summaries.

## Local embeddings

In Add provider choose Local embeddings. The ordinary template is created from
`images/orch-embeddings/Dockerfile` when no live readable template is available; a removed template is never
restored. The template is listed, editable and removable with the others. Its recipe pins and checks the
llama.cpp build and EmbeddingGemma 2 Q8_0 model; the model file is downloaded by the build, not stored in Git.
The preset uses the runtime's own `http://127.0.0.1:8090/v1`, `embeddinggemma-2` and 256 dimensions.
Its ordinary template defaults are four CPUs and 2 GiB. Change them in the template when needed.
Allow the build host to fetch the recipe's public artifacts. Its measured startup and embedding rate determine
when that runtime is ready and how fast indexing proceeds.

Creating the provider does not configure search. Save the provider/model in the ordinary Embeddings setting.
For this model use query prefix `task: search result | query: ` and document prefix `title: {title} | text: `.
Both include a trailing space. Other models can require different prefixes or no prefix.

## Settings and progress

| Key | Shipped value | Meaning |
| --- | --- | --- |
| `search.semantic.enabled` | false | Also retrieve meaning results |
| `search.semantic.embeddings` | unset | Provider, model, dimensions and prefixes; the writer answers for use |
| `search.semantic.max_disk_share` | 100 | Estimated index size as percent of indexed stored content; null removes the limit |
| `search.semantic.tuning` | registry object | Chunk/batch sizes, nearest candidates, scan limits, fusion and query wait |
| `search.page_size` | 20 | Initial visible results |
| `search.reorder_window_secs` | 5 | Settling window while the reader has not started reading |
| `search.meaning_pause_ms` | 800 | Typing pause before meaning retrieval |
| `search.vector_cache_secs` | 300 | Query-vector reuse per org and model generation |
| `search.word_candidates` | 50 | Bounded candidates per field before exact access/rank |
| `search.word_recent_candidates` | 200 | Recent density sample before a text-index probe |
| `search.word_tuning` | scope rows 30,000; no tier overrides | Planner-estimated scoped reads and independent limits for title, description, heading and body streams |

All are ordinary configurable settings. Locks and current permissions apply.
Word retrieval uses planner estimates from existing statistics; it does not count corpus rows per request.
Keep automatic analyze enabled after content changes. The scope threshold chooses an entrance and never
grants access or limits the total results. Missing tier overrides inherit `search.word_candidates`.
Semantic tuning retains `exact_filter_candidates`20,000, `max_scan_tuples`20,000 and scan memory2.
`iterative_scan` defaults to `strict_order`; `relaxed_order` sorts the selected candidates before return,
and `off` uses only the initial graph breadth. Omitted `ef_search` keeps candidates×oversample,
200 with shipped values, including its existing growth on continued pages. All graph values use SET LOCAL.
An exact-filter allowance of0 uses the graph for every scope. None changes the query wait or current access.
The query wait ships at three seconds. Words and meaning run in parallel over one NDJSON response.
Aborting it cancels both retrievals and a pending embedding command.
Agents use the same sources and return one merged answer; on a missed meaning budget they report its reason.
A phrase, regular expression, title-only selection or Newest uses words alone.
Writer, creator, last editor, owner, date, mention, file, sharing and location filters apply to both.

Counting discovers the full source inventory before content embedding. It has no whole-index percentage or ETA.
Indexing reports fresh-vector tokens and the endpoint's measured rate. Empty/short content and source metadata
still have costs; the index can reach its default disk share before every source has a vector.
Disk is an estimate of content, cuts, source rows and vectors across retained generations, not a filesystem quota.
Raising it is an operator choice. Turning search off stops its work; it does not promise to reclaim existing rows.

## Provider pauses

Disable cancels admitted embedding commands and pauses indexing with Provider switched off.
Removal uses Provider removed. These intentional stops add no model failure or retry.
Already reported use remains in the ordinary usage ledger. Enable resumes indexing automatically.

An admitted stop is retained even if a command has not been issued yet, so Enable cannot revive that
unfinished request. Stored content vectors are retained. A query vector still in the cache can use them;
a fresh query must use words while its embeddings provider is off. Search labels the paused index and
keeps current filters and permissions. Start a new search after indexing resumes to include new work.

## Model changes and disk choices

Keep the old generation while a replacement builds. Prefix/chunking changes, a longer length or a new model
need fresh content embeddings. A changed endpoint/name is checked with saved probes; matching probes avoid
re-embedding the same model served elsewhere. A compatible MRL shortening slices and renormalizes content and
cached query vectors, then rebuilds its index. The old search cursor is ended when its embedding space changes.

If both generations cannot fit the disk share, wait for the admin's Rebuild or Keep current model choice.
Rebuild discards the current generation; semantic results then cover what has completed. Keep restores all
parts of the current model choice atomically, including prefixes and chunk size, subject to ordinary locks.
Back up and restore these tables with the rest of PostgreSQL. They are derived data; rebuilding can require
another model call for each content batch and therefore more time and billed use.

## Diagnose a pause or slow search

Read the row's reason first: disk, provider/model, access, cap, offline machine or missing extension.
Repair the provider's ordinary runtime, key, offered model or writer's permissions rather than adding a second
endpoint or secret setting for search. Inspect Embeddings records in Usage to attribute paid work.
For latency, measure words, meaning lookup and payload/access work separately. Cold index and metadata reads
can be slower than warm reads. Check query plans and cache pressure before increasing a query wait.
`ORCH_LOG=orch::search::timing=debug` enables phase durations/counts; it does not print the query or payload.
Do not claim a query-vector cache removes storage reads or permission checks. Query cursors retain positions
and seen identities, never payload permissions, and each continued batch is authorized again.

## Related items

The document inspector and agents' `search_shared.related_to` use the same weighted ranking.
`related.weights`, `related.place_scores`, `related.count` and `related.candidates` inherit at system,
org, project and topic scopes and support locks. The [Related guide](../user/related-items.md) describes
their defaults. Count zero switches the list off; candidate count is a configurable work bound.

Each completed source keeps the unit-normalized mean of its normalized chunk vectors in
`search_item_vectors`, with the same generation and content marker. After upgrading an existing index,
the ordinary indexer fills these derived item vectors in its configured batches without new model calls.
They have their own generation-specific HNSW and are included in the semantic disk estimate, reserved
with their source before embedding. Shortening also shortens and reindexes the item vectors.
A generation switch never mixes old and new vectors. Opening Related makes no provider request.
Its optional lookup uses `search.semantic.tuning.query_wait_secs`; a missed lookup leaves place and reference
results available. Narrow readable sets reuse `exact_filter_candidates` before choosing the generation graph.
Replacement estimates include whole-item vectors at the replacement model's dimensions.

`related_references` indexes typed target identities from live document blocks, chat messages and thread
items. Native edits remove and replace their edges atomically. Related candidates do not retain names or
permissions in that index; ordinary current access checks precede delivery.

The [corpus runner](../../scripts/search-scale/README.md) prepares isolated native storage and measures
an already compiled debug or release target. It records source availability alongside latency.
Its cold-container option restarts only a verified bind-mounted fixture between samples; it keeps
the host page cache and the product's query deadlines unchanged.
