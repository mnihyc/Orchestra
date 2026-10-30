# Search measurements

`filter-plans.py --database-url <isolated URL> --output <new directory>` checks the production file-selection
SQL against temporary native-shaped document and file tables, with 1,000 file-bearing documents and nearly
all files on one document. Both tables have the selected 100,000 or 1,000,000 rows; setup verifies their counts.
It asserts one selected file per requested document, at most two examined file rows per request, and indexed
exact/family lookups. It preserves analyzed plans and index sizes. The temporary table disappears on disconnect;
the persistent corpus is unchanged. It also checks the kept file count through add, move, rename and delete.
This is a structural scale test, not a latency claim.
`media-plans.py` accepts the same URL, output and row-count options. It checks rare, common and Any
message file types, frozen-revision payloads, tool images and text carrying unrelated media metadata.
Its temporary tables and function/index DDL are rolled back. Both scripts retain plans and row bounds.

Set `ORCH_SEARCH_SCALE_OWNER_EDITED=1` when invoking `measure.py` to measure document results with the
fixture owner's ID and an edited-time window from January 3 through January 10. The fixture spans about
twelve days, so this exercises a nonempty seven-day cut; the report names the selection. Agent timings
continue to measure their ordinary search separately.

The ignored `search_meaning` test target measures an isolated synthetic corpus.
It does not assert a latency limit. Ordinary integration tests retain their search, access,
model-switch and cancellation assertions.
Every sample also asserts that retained payloads and ordering entries stay inside the configured lexical
window plus nearest-candidate budgets; reports include those bounds and the actual retained count.

Use a database prepared with the search corpus: one million English and Chinese documents,
200,000 chat messages, 200,000 thread items and their distinct 256-dimensional vectors.
Its `search_probe_id` and `corpus_vector_prototypes` fixtures name two readers: all content
and one percent. The target checks those readers before measuring. It never uses the shared
test database or calls a real embeddings provider.

To reproduce it, create an empty database named `orchprobe_search_<run>` on an isolated,
disk-backed PostgreSQL using Orch's database image. It needs pgvector and the image's substring
indexes. Supply its loopback URL and the compiled target to the same runner with `--prepare`:

Bind-mount the database data directory on the storage device reserved for the corpus.
An anonymous Docker volume can fill the system disk during preparation.

```sh
python3 scripts/search-scale/measure.py --profile debug --programs search-programs.json \
  --database-url postgres://orch:orch@127.0.0.1:41639/orchprobe_search_run \
  --prepare --output search-corpus-preparation
```

Preparation refuses existing public tables before applying the application's ordinary migrations.
Without `--program` or `--programs`, preparation uses `psql` with the same migration files,
fixture SQL, document batch commits and generation index expression. It requires no Rust build.
Its preparation profile is `not_applicable`; every measurement still requires a compiled
target and verifies the actual debug or release profile. For example:

```sh
python3 scripts/search-scale/measure.py --profile debug --prepare \
  --database-url postgres://orch:orch@127.0.0.1:41639/orchprobe_search_run \
  --output search-corpus-preparation
```

Preparation uses 2 GiB maintenance memory for the generation index, with parallel maintenance
disabled. The measured 1.4-million-vector graph needs about 1.6 GiB; a smaller build can spill
to storage and take much longer. This is preparation memory, separate from query limits.
If preparation completed all content/vector phases but its final index was interrupted, use
`--finish-index` in place of `--prepare`, without a compiled target. It first verifies every
native corpus count, dimensions and prototypes, and refuses an incomplete or already indexed
fixture. Ordinary preparation still refuses every populated database.

It keeps constraints enabled, commits document batches to bound trigger memory, and builds native
search tables and the same generation HNSW as the indexer. Its deterministic vector neighborhoods
are independent of the hundred access scopes. The two agents have inherited shares as well as the
two people's roles; both kinds of reader are checked on an allowed and a denied document before timing.
`--documents 1000` is a setup smoke check, not a scale result. Remove the throwaway database and its
storage after measuring. Preparation is outside measurement samples and never invokes a build.

Build the target through the build environment's normal gate. Save Cargo's JSON artifact
output or use a saved executable manifest. This runner never invokes Cargo or builds release.
Run the same command with the corresponding compiled profile:

```sh
ORCH_SEARCH_SCALE_URL=postgres://orch:orch@127.0.0.1:41639/orch \
python3 scripts/search-scale/measure.py --profile debug \
  --programs search-programs.json --output search-debug-run
```

For the release gate, change `--profile debug` to `--profile release` and supply the optimized
target's artifact manifest. Run on the quiet gate machine. The target records its compiled
profile, and the runner refuses to label a different profile as the requested one.

The output contains `measurements.json`, `environment.json` and the complete `output.log`.
It records corpus counts and disk sizes, load, available host memory, cache-state disclosure,
three retained warmups and thirty observations. Setup reads precede the warmups; the first
search is not automatically a cold-storage claim. Use `--cache-state` to disclose a known
initial state. It does not flush or prewarm storage. Percentiles use nearest rank.

For controlled cold PostgreSQL samples, add `--cold-container orchprobe-search-measure`.
The named fixture must publish the database's exact loopback port and bind-mount PGDATA.
The runner restarts it before every API and agent sample, waits for readiness outside the request
clock, and verifies every restart in `environment.json`. PostgreSQL shared buffers are cold;
the host page cache is unchanged. The compiled target must support this coordination protocol;
an older target is refused rather than silently labelled cold. Preparation and cold sampling are
separate operations. Run this only on the disposable fixture.

`--plan-markers` adds fixed reader/case/sample labels to database logs around each timed API
or agent operation. It never inserts the search text. Use it only for statement diagnostics,
separately from the uninstrumented latency run.

`words_ms` covers successful words retrieval. `fused_ms` includes only complete merged
answers with an available meaning source, or the deliberate words-only phrase case.
`segment_ms` also records fallback and failed-source durations. Source state, reason, count
and agent outcome are retained for every sample. An unavailable source never becomes a fast
fused success. Missing percentiles are `null`, with a zero valid-sample count.
The gate checks valid-sample counts and availability alongside latency. A partial case does not establish the bar.

Reports also retain final ordered part identities, source membership, and authorized payloads.
On the same unchanged corpus and generation, compare a baseline and a later run with:

```sh
python3 scripts/search-scale/compare.py before/measurements.json after/measurements.json
```

The comparison refuses missing cases, unavailable baselines and changes to the final order or
agent hits. It ignores source arrival order and timing. Keep the generation's graph and corpus
unchanged between runs so a storage rebuild does not introduce a second variable.

Use `--reader all|small` and `--query` to diagnose one case without repeating completed cases.
`--trace` adds phase logging and records that diagnostic overhead. The shipping latency bar
is assessed from the optimized gate run, not from debug measurements on a busy host.
