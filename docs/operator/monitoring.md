# Monitoring

Scrape each service separately with a Prometheus-compatible collector on the deployment's management
network. The listeners answer `GET /metrics` as Prometheus text; they have no application sign-in.
Measurements stay in this system unless you configure a collector to send them elsewhere.

| Service | Default listener / URL inside Compose | Configuration |
| --- | --- | --- |
| API | `api:9100/metrics` | `[api] metrics_bind` |
| Gateway | `gateway:9101/metrics` | `[gateway] metrics_bind` |
| Worker | `worker:9102/metrics` | `[worker] metrics_bind` |

They bind `0.0.0.0` inside their containers. Compose does not publish these ports on the host. Put the
collector on the same Compose network, or add an explicit loopback/private-network port mapping in an
operator override. The public web edge does not proxy these listeners. Example scrape configuration:

```yaml
scrape_configs:
  - job_name: orch-api
    scrape_interval: 15s
    static_configs:
      - targets: ['api:9100']
  - job_name: orch-gateway
    scrape_interval: 15s
    static_configs:
      - targets: ['gateway:9101']
  - job_name: orch-worker
    scrape_interval: 15s
    static_configs:
      - targets: ['worker:9102']
```

Replace service names when the collector's network differs. Prometheus supplies `up`, `job` and
`instance`; they are collector labels, not application metrics. Fifteen seconds matches service beats.

## Readiness and service age

`GET /health` and `GET /api/v1/health` both return 200/`ok` only when the database answers and the worker
has a recorded beat no older than **45 seconds**, the named `orch_db::heartbeat::WORKER_HEARTBEAT_MAX_AGE`
(three fifteen-second beats). Missing/stale worker or database failure returns 503. A beat exactly at
the limit remains current. This is readiness, not proof that every host, model or external channel works.
Use `/api/v1/admin/system` as an authorized administrator for component/queue details.

On first startup, the worker waits for the schema the API applies. Compose starts it after PostgreSQL
is healthy and the API has started; it deliberately does not wait for API readiness, which needs that
worker's first heartbeat. Preserve that dependency when overriding the deployment.

Both API and worker sample the database-backed heartbeat and verified-backup records every fifteen
seconds. The API keeps their age visible when the worker has stopped. A missing record has `present=0`;
its zero age must never be read as a fresh success. A failed query leaves prior readings unchanged, so
also alert on readiness and scrape failure. Backup ages describe the database dump/manifest pair only,
not completion of the [separate blob/volume copies](backup-and-restore.md).

## Metric meanings

The stored Management catalogue also includes `provider_plan_used_percent`, a
gauge labelled by `provider` and `window`. Each stored provider reading updates
that minute in the same transaction. Minute retention follows `metrics_fine_days`;
older plan charts use hourly peaks under `metrics_days`. It is available through
the stored metrics API, without a separate retention setting or scrape producer.
Plan charts read one bounded page of windows. Peaks, bucket usage and reset
counts are aggregated in PostgreSQL; forecasts read only hour/day pace summaries.
The partial `audit_provider_plan_window_idx` index serves reset receipts without
scanning unrelated audit history.


Counters accumulate until their exporting process restarts; use `rate`/`increase`, not raw totals for
an interval. Gauges show current/latest state. Duration distributions use seconds and the exporter's
histogram/summary form (`_count`, `_sum` and buckets/quantiles where exposed). Series appear when their
producer runs; zero traffic alone does not prove a failure. Gateway aggregates are also carried to the
API; choose one exporter for them, rather than summing the same observation twice.

| Metric | Type, labels and meaning |
| --- | --- |
| `orch_http_requests_total` | Counter, `route,status`: completed requests by declared route/status, without raw URLs or query values |
| `orch_http_request_seconds` | Distribution, `route,status`: server request duration |
| `orch_ws_connections` | Gauge, `feed_kind`: connected browser sockets (`all`) |
| `orch_ws_subscriptions` | Gauge, `feed_kind`: active feed subscriptions |
| `orch_ws_merged_deltas_total` | Counter: deltas merged for a reader that was behind |
| `orch_ws_slow_closes_total` | Counter: browser sockets closed with 4408 for not taking data |
| `orch_threads` | Gauge, `state`: live thread counts by state |
| `orch_turns_total` | Counter, `status,harness`: completed turns by outcome and harness |
| `orch_turn_seconds` | Distribution, `status,harness`: turn duration |
| `orch_turns_waiting` | Gauge, `reason=cap|lock`: inputs held by a cap or switching lock, excluding a person's queue Hold |
| `orch_link_sessions` | Gauge, `placement,trust`: current machine sessions |
| `orch_link_events_total` | Counter, `stream_kind`: committed link events |
| `orch_link_event_lag_seconds` | Distribution, `stream_kind`: age on admission of delivered link events |
| `orch_link_round_trip_seconds` | Distribution, `placement`: measured machine-link round trip |
| `orch_commands_open` | Gauge, `kind`: machine commands not yet over |
| `orch_tunnels_open` | Gauge, `kind`: current tunnel routes, including those waiting within their resume grace |
| `orch_tunnel_bytes_total` | Counter, `kind`: accepted forwarded traffic bytes; excludes refused traffic |
| `orch_blob_store_bytes` | Gauge, `class=pinned|cache`: stored pinned bytes or unpinned cached bytes |
| `orch_blob_store_free_bytes` | Gauge, `class=pinned|cache`: physical filesystem availability. Both classes see the same free room: never add them. A failed filesystem reading retains the prior value. |
| `orch_outbox_pending` | Gauge, `channel`: notification deliveries waiting for their channel |
| `orch_route_messages_total` | Counter, `channel,form,result`: route deliveries, single/digest and sent/failed/gone |
| `orch_routes` | Gauge, `channel,state=on|off|unlinked`: people's notification routes |
| `orch_jobs` | Gauge, `kind,state`: jobs that are not over |
| `orch_runtimes` | Gauge, `kind,driver,state`: managed runtime inventory |
| `orch_harness_processes` | Gauge, `harness,status`: reported harness process states |
| `orch_summaries_total` | Counter, `field,result`: summary generation outcomes |
| `orch_summary_attempts_total` | Counter, `result`: completed, failed or skipped summary attempts |
| `orch_plugins` | Gauge, `kind,state`: reported external plugin states |
| `orch_usage_tokens_total` | Counter, `purpose`: authoritative counted tokens, excluding trace-only duplicates |
| `orch_usage_credits_total` | Monotone counter, `purpose,source`: counted rated credits; `source=provider|rate_card|catalogue`, with catalogue amounts estimates. Fractional credits are retained. |
| `orch_usage_unrated_tokens_total` | Counter, `purpose`: tokens with no credit rate; not included in rated-credit totals |
| `orch_usage_caps` | Gauge, `meter,state=ok|warning|reached`: current enabled cap evaluations |
| `orch_model_requests_total` | Counter, `model,harness,outcome`: terminal model requests, including an unmeasured turn fallback; `done` is success and `forced` is a cancellation, excluded from error numerators |
| `orch_model_first_token_seconds` | Histogram, `model,harness`: input/tool result to first output, including hidden thinking when present; absent when not measured |
| `orch_model_output_tokens_per_second` | Histogram, `model,harness`: eligible streamed output speed, excluding hidden reasoning where separable; requires at least 500 ms and 32 streamed tokens |
| `orch_limit_bypasses_total` | Counter, `meter`: explicitly bypassed token/credit/running/provider limits |
| `orch_terminal_sessions` | Gauge, `state=attached|detached`: people's open shells |
| `orch_host_cpu_percent` | Gauge, `host_id`: host's last reported CPU percentage |
| `orch_host_memory_used_mb` | Gauge, `host_id`: last reported used memory, MiB |
| `orch_host_disk_used_gb`, `orch_host_disk_free_gb` | Gauges, `host_id`: last reported disk use/free room, GiB |
| `orch_chat_messages_total` | Counter, `kind=node|direct|group`: created chat messages |
| `orch_shared_calls_total` | Counter, `tool,result`: shared-tree agent tool outcomes, including refusal/truncation/rate limit |
| `orch_document_exports_total` | Counter, `format`: document exports; browser PDF preparation counts as Typst |
| `orch_document_ops_total` | Counter, `result=applied|conflict|too_large`: document change batches |
| `orch_rate_catalogue_age_days` | Gauge, `origin`: age of the catalogue in force, not an upload timestamp |
| `orch_db_pool_in_use` | Gauge, `component`: checked-out database pool connections, sampled periodically |
| `orch_db_slow_statements_total` | Counter: SQLx statements reaching `[database] slow_statement_ms` (1000 ms by default); emitted separately by API/worker |
| `orch_db_slow_acquires_total` | Counter: successful SQLx acquisitions exceeding `[database] slow_acquire_ms` (1000 ms); excludes acquisitions that failed/timed out |
| `orch_db_pool_max_connections` | Gauge: the largest size of the service's database pool (`[api] db_pool`, `[worker] db_pool`) |
| `orch_db_pool_acquire_seconds` | Histogram: how long a caller of the API or the worker waited for a pooled connection |
| `orch_db_pool_acquire_over_100ms_total` | Counter: waits for a pooled connection longer than 100 ms, the release bar |
| `orch_db_transactions_held_over_200ms_total` | Counter: transactions of the API or the worker open longer than 200 ms (a debug build also logs each with its request) |
| `orch_component_heartbeat_present` | Gauge, `component=api|gateway|worker`: 1 when a heartbeat record exists, else 0 |
| `orch_component_heartbeat_age_seconds` | Gauge, `component`: age of that last beat, clamped at zero for a future timestamp; consult presence too |
| `orch_backup_present` | Gauge: 1 when the reconciled verified database backup has a successful record, else 0 |
| `orch_backup_age_seconds` | Gauge: age of the last successful database pair; later failed attempts do not reset it |
| `orch_db_oldest_transaction_seconds` | Gauge, worker: how long the oldest transaction of the services' database sessions has been open, 0 when none is; sampled at each worker heartbeat. An operator's `pg_dump` is not counted |

Stored Management minute samples contain counter growth, current gauges and interval distribution means.

Model-performance histograms use the same fixed logarithmic edges as Usage: eight steps per doubling
from 2^-10 to 2^20 (the first edge is below the smallest positive reported speed), a real-zero bucket and overflow. First-output edges are converted from milliseconds
to seconds. Usage estimates percentiles at each selected bucket’s geometric middle (about 4.5 percent
relative bucket error). Prometheus applies its chosen histogram estimator. Missing measurements are omitted, never exported as zero. Short bursts, unseparated
hidden thinking and untimed sub-agent replies still contribute requests and outcomes.

For a provider error-rate alert, count only `provider_exhausted`, `provider_unauthorized`,
`provider_busy`, `provider_unreachable` and `model_unavailable` in the numerator, and include every terminal
request in the denominator. A useful starting point matches the web status part: at least ten requests
and twenty percent errors over fifteen minutes. The web also compares median first output with the
provider's own seven-day median, requiring ten measured first outputs in each window. Watch the sample
count alongside latency and speed percentiles.
They do not preserve individual histogram observations. Host facts remain dated observations, not a claim
that a disconnected host still has those resources. Metric names/labels contain no credentials or SQL.

## Alerts to start with

Tune traffic-dependent thresholds to your installation. These are useful starting conditions:

| Signal | Example condition and response |
| --- | --- |
| Service missing | `up == 0` for two scrapes; check the process/container and listener |
| Worker stopped | API's `orch_component_heartbeat_present{component="worker"} == 0` or age >45 seconds; allow one sampling interval before paging, and check readiness/worker logs |
| Outbox blocked | `orch_outbox_pending` grows or remains nonzero beyond the expected route delay; inspect channel failures/credentials, including deliberate delay/digest settings |
| Link loss | `orch_link_sessions` drops below the number of machines expected online; check gateway reachability and certificates. Zero is normal on an empty/off-hours installation. |
| Blob filesystem filling | `orch_blob_store_free_bytes{class="pinned"}` approaches the capacity needed for uploads/restore; choose an absolute reserve for your largest operation and alert before it is exhausted |
| Transaction left open | `orch_db_oldest_transaction_seconds > 300`; the worker also logs a warning. No transaction of the services should last more than seconds, and one left open holds back the database's cleanup and a pooled connection. Report it with the worker's log |
| Backup missing/old | `orch_backup_present == 0` or age >172800 seconds when daily backups are expected. Two days matches shipped `backup.warn_days=2`; adapt to your policy and independently verify the paired blob/volume copies. |
| Slow SQL / pool contention | `increase(orch_db_slow_statements_total[5m]) > 0` or `increase(orch_db_slow_acquires_total[5m]) > 0`; inspect warning statements/plans and connection ownership before increasing a pool |
| Pool near its size, or waited for | `max_over_time(orch_db_pool_in_use[5m]) / orch_db_pool_max_connections > 0.8`, or `increase(orch_db_pool_acquire_over_100ms_total[5m]) > 0`. A request holds its connection for its database work alone (the API's sessions are ended by PostgreSQL after 10 s idle in a transaction, its statements after 15 s); look for long transactions (`orch_db_transactions_held_over_200ms_total`) before raising `db_pool` |

Slow warnings contain SQL text/placeholders and elapsed/threshold fields, never the bound values. Counters
keep counting even if `ORCH_LOG` hides printed warnings. The watch changes neither pool sizes nor the
ten-second acquire timeout. See [configuration](configuration.md).
