# Monitor an installation

Check that a running Orch stack is healthy, scrape its measurements with a Prometheus-compatible collector, and set the first alerts. At the end you can see each service's state by hand and from your collector, and you are told when a service, the worker, notifications, machine links, disk or backups go wrong.

## Before you start

You need a running stack ([Install Orch](deployment.md)) and, for the scrape, a Prometheus-compatible collector on the deployment's management network. Measurements stay in this system unless you set a collector to send them elsewhere.

## 1. Check the stack by hand

`orchctl doctor` tries, in turn, the permissions of the local secret files, the configuration, the database, the API on its own listener, and the heartbeats of the gateway, the worker and the node. Run it inside the API's container, where the configuration, the database address and the secrets are:

```sh
docker compose exec api /opt/orchctl/orchctl doctor
```

It prints one line per check, saying what it tried and what came back, and exits with 1 when a line says `FAIL`.

- `--only` and `--skip` choose lines. Use `--skip node` for a stack without the bundled node.
- Stop a service and its line fails and names it:

  ```
  FAIL  worker           last heard 58 s ago, and a live one reports every few seconds: it reports through the database
  ```

- With a loopback public address, such as a trial's `http://127.0.0.1:5180`, `edge-page` and `edge-api` say `skip`: inside the container that address is the container itself. `./update.sh doctor` tries them from the host.
- The same program checks the HTTPS front from wherever you run it: [Check the front](edge.md#6-check-the-front).

## 2. Scrape the services

Each service answers `GET /metrics` as Prometheus text on its own listener. The listeners have no application sign-in.

| Service | Default listener inside Compose | Setting |
| --- | --- | --- |
| API | `api:9100/metrics` | `[api] metrics_bind` |
| Gateway | `gateway:9101/metrics` | `[gateway] metrics_bind` |
| Worker | `worker:9102/metrics` | `[worker] metrics_bind` |

The listeners bind `0.0.0.0` inside their containers. Compose does not publish these ports on the host, and the public web edge does not proxy them. Either put the collector on the same Compose network, or add a loopback or private-network port mapping in an operator override.

1. Add the three jobs to the collector (replace the service names if its network differs):

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

2. Reload the collector and check that `up` is 1 for each job. `up`, `job` and `instance` come from the collector, not from Orch. Fifteen seconds matches how often the services record their own beats.

## 3. Set the first alerts

Tune the thresholds that depend on traffic to your installation. These are useful starting conditions:

| Signal | Condition and response |
| --- | --- |
| Service missing | `up == 0` for two scrapes. Check the process or container and its listener |
| Worker stopped | The API's `orch_component_heartbeat_present{component="worker"} == 0`, or its age above 45 seconds. Allow one sampling interval before paging, then check readiness and the worker's log |
| Notifications blocked | `orch_outbox_pending` grows, or stays above zero longer than the delay you expect. Look at channel failures and credentials, including delay and digest settings you chose |
| Machine links lost | `orch_link_sessions` drops below the number of machines expected online. Check that the gateway is reachable and the certificates are valid. Zero is normal on an empty or idle installation |
| Blob filesystem filling | `orch_blob_store_free_bytes{class="pinned"}` approaches the room your largest upload or restore needs. Choose an absolute reserve and alert before it runs out |
| Transaction left open | `orch_db_oldest_transaction_seconds > 300` (the worker also logs a warning). No transaction of the services should last more than seconds; one left open holds back the database's cleanup and a pooled connection. Report it with the worker's log |
| Backup missing or old | `orch_backup_present == 0`, or `orch_backup_age_seconds` above 172800 when you expect daily backups. Two days matches the shipped `backup.warn_days = 2`. Check the paired blob and volume copies yourself |
| Slow SQL or pool contention | `increase(orch_db_slow_statements_total[5m]) > 0` or `increase(orch_db_slow_acquires_total[5m]) > 0`. Look at the warning statements and plans, and at who holds connections, before you raise a pool |
| Pool near its size, or waited for | `max_over_time(orch_db_pool_in_use[5m]) / orch_db_pool_max_connections > 0.8`, or `increase(orch_db_pool_acquire_over_100ms_total[5m]) > 0`. A request holds its connection only for its database work (PostgreSQL ends the API's sessions after 10 s idle in a transaction and its statements after 15 s). Look for long transactions with `orch_db_transactions_held_over_200ms_total` before you raise `db_pool` |
| Provider errors | A ratio of errors to all terminal requests of a model, as described under [Model performance](#model-performance). A starting point: at least ten requests and twenty percent errors over fifteen minutes |

Slow-statement warnings contain the SQL text with placeholders and the elapsed and threshold times, never the bound values. The counters keep counting even when `ORCH_LOG` hides the printed warnings. These alerts change neither pool sizes nor the ten-second acquire timeout ([Configure Orch](configuration.md)).

When pages show live changes late, `ORCH_LOG=info,orch_realtime::timing=debug` on the API prints where the time went: for every read of a feed for its open pages, how long its announcement waited, its longest wait for a page's subscription and its longest read; and for every frame, how long the socket's writer took from taking it to sending it. The lines name feeds, connections and subscriptions by their identifiers, never their content.

## What readiness means

`GET /health` and `GET /api/v1/health` return 200 `ok` only when the database answers and the worker has recorded a beat no older than **45 seconds** (three fifteen-second beats). A missing or stale worker beat, or a database failure, returns 503. A beat exactly at the limit counts as current. Readiness does not prove that every host, model or outside channel works. For component and queue detail, use `/api/v1/admin/system` in an authorized browser session, or `/openapi/v1/admin/system` with an account key whose permissions and key rules allow it.

On first start the worker waits for the schema the API applies. Compose starts it after PostgreSQL is healthy and the API has started. It does not wait for the API to be ready, because readiness needs the worker's first beat. Keep that dependency if you override the deployment.

The API and the worker both sample the database's heartbeat and verified-backup records every fifteen seconds, so the API shows their age even when the worker has stopped. A missing record has `present=0`, and its zero age is never a fresh success. A failed query leaves earlier readings in place, so alert on readiness and on scrape failure too. Backup ages describe the database dump and its manifest only, not the [blob and volume copies](backup-and-restore.md).

## Read the metrics

Counters accumulate until their exporting process restarts, so use `rate` or `increase` for an interval, not raw totals. Gauges show the latest state. Duration distributions are in seconds, in the exporter's histogram or summary form (`_count`, `_sum`, and buckets or quantiles where shown). A series appears when the thing that produces it runs, so zero traffic alone does not mean a failure. The gateway's aggregates also reach the API: choose one exporter for them so you do not sum an observation twice.

| Metric | Type, labels and meaning |
| --- | --- |
| `orch_http_requests_total` | Counter, `route,status`: completed requests by declared route and status, without raw URLs or query values |
| `orch_http_request_seconds` | Distribution, `route,status`: server request duration |
| `orch_ws_connections` | Gauge, `feed_kind`: connected browser sockets (`all`) |
| `orch_ws_subscriptions` | Gauge, `feed_kind`: active feed subscriptions |
| `orch_ws_merged_deltas_total` | Counter: deltas merged for a reader that was behind |
| `orch_ws_slow_closes_total` | Counter: browser sockets closed with 4408 for not taking data |
| `orch_threads` | Gauge, `state`: live thread counts by state |
| `orch_turns_total` | Counter, `status,harness`: completed turns by outcome and agent program |
| `orch_turn_seconds` | Distribution, `status,harness`: turn duration |
| `orch_turns_waiting` | Gauge, `reason=cap|lock`: inputs held by a cap or a switching lock, not counting a person's queue hold |
| `orch_link_sessions` | Gauge, `placement,trust`: current machine sessions |
| `orch_link_events_total` | Counter, `stream_kind`: committed link events |
| `orch_link_stream_resets_total` | Counter, `reason=spool_lost|too_large`: times a machine said that events of a stream are gone; `too_large` is an event larger than a message of the session may be |
| `orch_link_diagnostics_dropped_total` | Counter: waiting diagnostic frames discarded by their byte or age allowance; capture sequence gaps stay explicit, while authoritative events remain until acknowledgement |
| `orch_link_event_lag_seconds` | Distribution, `stream_kind`: age on admission of delivered link events |
| `orch_link_round_trip_seconds` | Distribution, `placement`: measured machine-link round trip |
| `orch_commands_open` | Gauge, `kind`: machine commands not yet over |
| `orch_tunnels_open` | Gauge, `kind`: current tunnel routes, including those waiting within their resume grace |
| `orch_tunnel_bytes_total` | Counter, `kind`: accepted forwarded traffic in bytes; refused traffic is excluded |
| `orch_blob_store_bytes` | Gauge, `class=pinned|cache`: stored pinned bytes, or unpinned cached bytes |
| `orch_blob_store_free_bytes` | Gauge, `class=pinned|cache`: free room on the physical filesystem. Both classes see the same room: never add them. A failed filesystem reading keeps the earlier value |
| `orch_outbox_pending` | Gauge, `channel`: notification deliveries waiting for their channel |
| `orch_route_messages_total` | Counter, `channel,form,result`: route deliveries, single or digest, sent, failed or gone |
| `orch_routes` | Gauge, `channel,state=on|off|unlinked`: people's notification routes |
| `orch_jobs` | Gauge, `kind,state`: jobs that are not over |
| `orch_runtimes` | Gauge, `kind,driver,state`: managed runtime inventory |
| `orch_harness_processes` | Gauge, `harness,status`: reported agent program process states |
| `orch_summaries_total` | Counter, `field,result`: summary generation outcomes |
| `orch_summary_attempts_total` | Counter, `result`: completed, failed or skipped summary attempts |
| `orch_plugins` | Gauge, `kind,state`: reported states of external plugins |
| `orch_usage_tokens_total` | Counter, `purpose`: authoritative counted tokens, without duplicates that exist only in traces |
| `orch_usage_credits_total` | Monotone counter, `purpose,source`: counted rated credits; `source=provider|rate_card|catalogue`, catalogue amounts being estimates. Fractional credits are kept |
| `orch_usage_unrated_tokens_total` | Counter, `purpose`: tokens with no credit rate; not in the rated-credit totals |
| `orch_usage_caps` | Gauge, `meter,state=ok|warning|reached`: current evaluations of enabled caps |
| `orch_limit_bypasses_total` | Counter, `meter`: token, credit, running or provider limits that were explicitly bypassed |
| `orch_terminal_sessions` | Gauge, `state=attached|detached`: people's open terminals |
| `orch_host_cpu_percent` | Gauge, `host_id`: the host's last reported CPU percentage |
| `orch_host_memory_used_mb` | Gauge, `host_id`: last reported used memory, MiB |
| `orch_host_disk_used_gb`, `orch_host_disk_free_gb` | Gauges, `host_id`: last reported disk use and free room, GiB |
| `orch_chat_messages_total` | Counter, `kind=node|direct|group`: created chat messages |
| `orch_shared_calls_total` | Counter, `tool,result`: outcomes of the agent tools for the shared tree, including refusal, truncation and rate limit |
| `orch_document_exports_total` | Counter, `format`: document exports; browser PDF preparation counts as Typst |
| `orch_document_ops_total` | Counter, `result=applied|conflict|too_large`: document change batches |
| `orch_rate_catalogue_age_days` | Gauge, `origin`: age of the catalogue in force, not an upload time |
| `orch_db_pool_in_use` | Gauge, `component`: checked-out database pool connections, sampled periodically |
| `orch_db_slow_statements_total` | Counter: statements reaching `[database] slow_statement_ms` (1000 ms by default); the API and the worker emit it separately |
| `orch_db_slow_acquires_total` | Counter: successful acquisitions slower than `[database] slow_acquire_ms` (1000 ms); failed or timed-out acquisitions are excluded |
| `orch_db_pool_max_connections` | Gauge: the service's largest database pool size (`[api] db_pool`, `[worker] db_pool`) |
| `orch_db_pool_acquire_seconds` | Histogram: how long a caller of the API or the worker waited for a pooled connection |
| `orch_db_pool_acquire_over_100ms_total` | Counter: waits for a pooled connection longer than 100 ms |
| `orch_db_transactions_held_over_200ms_total` | Counter: transactions of the API or the worker open longer than 200 ms (a debug build also logs each one with its request) |
| `orch_component_heartbeat_present` | Gauge, `component=api|gateway|worker`: 1 when a heartbeat record exists, else 0 |
| `orch_component_heartbeat_age_seconds` | Gauge, `component`: age of the last beat, clamped at zero for a future timestamp; read it with presence |
| `orch_backup_present` | Gauge: 1 when the reconciled verified database backup has a successful record, else 0 |
| `orch_backup_age_seconds` | Gauge: age of the last successful database pair; later failed attempts do not reset it |
| `orch_db_oldest_transaction_seconds` | Gauge, worker: how long the oldest transaction among the services' database sessions has been open, 0 when none is; sampled at each worker heartbeat. An operator's `pg_dump` is not counted |
| `provider_plan_used_percent` | Gauge, `provider,window`: use of a provider's plan window. It is stored with Management's own charts, one reading per minute, kept for `metrics_fine_days` and then as hourly peaks for `metrics_days`; it is not a scrape series and has no separate retention setting |

Stored Management minute samples hold counter growth, current gauges and the means of interval distributions. They do not keep individual histogram observations.

### Model performance

| Metric | Type, labels and meaning |
| --- | --- |
| `orch_model_requests_total` | Counter, `model,harness,outcome`: terminal model requests, including an unmeasured turn fallback. `done` is success and `forced` is a cancellation; neither counts as an error |
| `orch_model_first_token_seconds` | Histogram, `model,harness`: time from input or tool result to first output, including hidden thinking when present. Absent when not measured |
| `orch_model_output_tokens_per_second` | Histogram, `model,harness`: streamed output speed, excluding hidden reasoning where it can be separated. It needs at least 500 ms and 32 streamed tokens |

The histograms use the same fixed logarithmic edges as **Usage**: eight steps per doubling from 2^-10 to 2^20, a bucket for a real zero, and an overflow bucket. First-output edges are converted from milliseconds to seconds. **Usage** estimates percentiles at each bucket's geometric middle, about 4.5 percent relative error; Prometheus applies the estimator you choose. A missing measurement is left out and never exported as zero. Short bursts, unseparated hidden thinking and sub-agent replies without timing still count in requests and outcomes.

For a provider error-rate alert, count only `provider_exhausted`, `provider_unauthorized`, `provider_busy`, `provider_unreachable` and `model_unavailable` in the numerator, and every terminal request in the denominator. The web status uses at least ten requests and twenty percent errors over fifteen minutes. It also compares the median first output with the provider's own seven-day median, with ten measured first outputs in each window. Watch the sample count beside latency and speed percentiles.

Host figures are dated observations: they do not say that a disconnected host still has those resources. Metric names and labels contain no credentials or SQL.
