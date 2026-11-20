# Search measurements

Scripts that measure how Orch's search behaves on a large corpus: a million English and Chinese documents, 200,000 chat messages, 200,000 thread items and their 256-dimension vectors, read by two readers (everything, and one percent). They report latency and check that results stay inside the configured windows. They use an isolated database, never the shared test database, and never call a real embeddings provider.

## Run it

You need PostgreSQL in Orch's database image on an isolated, disk-backed server (pgvector and the image's substring indexes included), a compiled test target, and Python 3.

1. **Prepare a database.** Create an empty database named `orchprobe_search_<run>` and bind-mount its data directory on the storage device reserved for the corpus: an anonymous Docker volume can fill the system disk. Then:

   ```sh
   python3 scripts/search-scale/measure.py --profile debug --prepare \
     --database-url postgres://orch:orch@127.0.0.1:41639/orchprobe_search_run \
     --output search-corpus-preparation
   ```

   Preparation refuses a database that already has public tables, applies the application's migrations, and fills the corpus and the same generation index the indexer builds. It uses `psql` and needs no Rust build. It uses 2 GiB of maintenance memory for the index (the 1.4-million-vector graph needs about 1.6 GiB; a smaller build can spill to storage and take much longer). If it finished all the content and vector phases but its final index was interrupted, run it again with `--finish-index` instead of `--prepare`. `--documents 1000` is a quick check of the setup, not a scale result.
2. **Measure.** Build the target through your normal build, save Cargo's JSON artifact output or an executable manifest (this runner never invokes Cargo), and run:

   ```sh
   ORCH_SEARCH_SCALE_URL=postgres://orch:orch@127.0.0.1:41639/orch \
   python3 scripts/search-scale/measure.py --profile debug \
     --programs search-programs.json --output search-debug-run
   ```

   Use `--profile release` and the optimized target's manifest for a measurement you will judge. Run it on a quiet machine. The target records its compiled profile, and the runner refuses to label another profile as the one you asked for.
3. **Compare** two runs of the same unchanged corpus and generation:

   ```sh
   python3 scripts/search-scale/compare.py before/measurements.json after/measurements.json
   ```

   The comparison refuses missing cases and unavailable baselines, and reports changes in the final order or the agent's hits. It ignores timing and the order in which sources arrived.
4. **Clean up.** Remove the throwaway database and its storage.

## What you will see

The output directory holds `measurements.json`, `environment.json` and the complete `output.log`. They record the corpus counts and disk sizes, the load, the available memory, the cache state, three kept warmups and thirty observations. Percentiles use the nearest rank. The first search is not a cold-storage claim: use `--cache-state` to say what the state was. The runner neither flushes nor warms the cache.

- `words_ms` covers successful word retrieval. `fused_ms` covers only complete merged answers whose meaning source was available, or the deliberate words-only phrase case. `segment_ms` also records the duration of fallbacks and failed sources.
- The state, reason and count of every source, and the agent's outcome, are kept for every sample. An unavailable source never becomes a fast merged success. A missing percentile is `null`, with a valid-sample count of zero, and a partial case establishes nothing.
- Every sample checks that the payloads and ordering entries kept stay inside the lexical window and the nearest-candidate budgets, and the report states those bounds and the actual count.

## Options

- `--cold-container orchprobe-search-measure` restarts the named database container before every API and agent sample, for cold PostgreSQL buffers (the host page cache is unchanged). The container must publish the database's exact loopback port and bind-mount its data directory, and the target must support the coordination protocol. Use it only on the disposable database.
- `--plan-markers` adds fixed labels to the database logs around each timed operation, never the search text. Use it for statement diagnosis, apart from the uninstrumented latency run.
- `--reader all|small` and `--query` run one case; `--trace` adds phase logging and records that overhead.
- `ORCH_SEARCH_SCALE_OWNER_EDITED=1` measures documents with the fixture owner's id and an edited-time window from January 3 to 10 (the fixture spans about twelve days, so the seven-day cut is not empty).

## Plan checks

`filter-plans.py --database-url <isolated URL> --output <new directory>` checks the production file-selection SQL on temporary tables of 100,000 or 1,000,000 rows, with 1,000 file-bearing documents and nearly all files on one document. It asserts one selected file per requested document, at most two examined file rows per request, and indexed lookups; it keeps the plans and index sizes, and checks the kept file count through add, move, rename and delete. `media-plans.py` takes the same options and checks rare, common and any-type message files, frozen-revision payloads, tool images and text with unrelated media metadata; its temporary tables and DDL are rolled back. Both are checks of structure at scale, not claims about latency.
