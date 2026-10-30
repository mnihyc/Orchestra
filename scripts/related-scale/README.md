# Related-list measurements

Use the isolated native corpus prepared by [the search runner](../search-scale/README.md):
one million documents, 200,000 messages, 200,000 thread items and their distinct 256-dimensional vectors.
The corpus has independent all-content and one-percent readers, for people and agents.

Build `orch-api-core --test search_meaning --test related_scale` once through the normal build gate,
and retain its artifact JSON or suite executable manifest. The runners never build.
Prepare the search corpus on an empty, isolated, disk-backed PostgreSQL using its ordinary runner.
Then derive the whole-item vectors and add native typed references to one in ten documents:

```sh
python3 -I scripts/related-scale/measure.py --profile debug --programs programs.json \
  --database-url postgres://orch:orch@127.0.0.1:41941/orchprobe_search_related \
  --prepare --output related-debug
```

The reference target is deliberately popular, exercising bounded reverse and shared-target retrieval.
Preparation uses the same normalized-mean expression and generation-specific HNSW as the indexer.
It updates real document blocks so the ordinary live-reference projection supplies the edges.
It calls no model. For a repeat measurement omit `--prepare`.

The target checks allowed/denied source access independently for people and agents before timing.
It measures the real HTTP route and `search_shared.related_to`, each with meaning on and off,
for both readers. One-percent results must stay inside their granted topic.
After three warmups it retains thirty observations, including host load, with nearest-rank p50/p95.
No failed source becomes a fast successful observation. The raw output and every sample are saved.
Corpus counts, physical sizes, PostgreSQL version and memory settings accompany the result.

Initial cache state is unspecified. Fixture and permission reads precede the warmups.
Debug measurements on a busy shared host are diagnostic. The existing 300 ms reads bar is judged
on the optimized shipping build on a quiet release-gate machine; supply that manifest and
`--profile release` for the same run. Do not change a deadline or default to make it pass.
Remove the throwaway database after collecting the report.

## Row and query-plan bounds

Before latency acceptance, run the native SQL proof on the same disposable corpus:

```sh
python3 -I scripts/related-scale/bounds.py --container orchprobe-related-pg \
  --database orchprobe_search_related --output related-bounds
```

It creates only temporary synthetic data and rolls it back. One million distinct part identities exercise
repeated links, one million distinct targets, and a popular reverse neighborhood. Kept target counters must
match the parts after deletion and replacement. Twelve actual EXPLAIN ANALYZE plans assert result/window
bounds and indexed large-table entrances; interactive reads must not reach history or native part bodies.
Separate million-row grant and expired-membership fixtures test the bounded metadata entrances.
The accounting receipt represents 1,000 threads and ten billion tokens, including a 100-million-token thread;
it does not pretend to allocate ten billion tokens of text. The full application synthetic-world release gate
and the real HTTP/agent timings remain separate required checks.
