# Configuration

The services read `config/orch.toml` in a deployment. An environment variable named
`ORCH_<SECTION>_<KEY>` overrides that key; for example, `ORCH_DATABASE_SLOW_STATEMENT_MS`.
Unknown keys and values of the wrong type stop startup. Restart the affected service after changing
its configuration. The complete defaults and field descriptions are in the
[server configuration](../../crates/platform/config/src/server.rs) and the deployment's
[example file](../../deploy/config/orch.toml).

## Database watch

```toml
[database]
slow_statement_ms = 1000
slow_acquire_ms = 1000
```

The API and worker warn when a statement reaches `slow_statement_ms`, or when a successful pooled
connection acquisition takes more than `slow_acquire_ms`. Both values are milliseconds; zero watches
every operation. SQLx logs the statement text with its parameter placeholders, elapsed time and threshold;
it does not log bound values. Do not put secrets into SQL text or enable logging of credentials yourself.

The two [monitoring counters](monitoring.md) count these events even if `ORCH_LOG` suppresses their
printed warnings. Acquiring a connection still times out after ten seconds; these thresholds do not
change that timeout, a pool's size, or statement execution limits. A failed/timed-out acquisition is an
error, not a successful slow acquisition in that counter. Investigate the statement/plan and connection
ownership before changing pool sizes. Never hold a connection while awaiting another machine.

## Logs and measurements

`[log] format` is `text` or `json`; `ORCH_LOG` selects the tracing output filter (`info` by default).
`[api] metrics_bind`, `[gateway] metrics_bind` and `[worker] metrics_bind` select the private metrics
listeners. The [monitoring guide](monitoring.md) explains their addresses, scrape setup and alerts.

## Recorded files and restore

Setup already provides a local content store: `[storage] root` defaults to `/var/lib/orch`, mounted
from `runtime/data` by the shipped deployment. It holds manifests and the plans transferred between
the server and a connector. No external blob service or extra setting is needed to restore a turn's files.
The API opens this store before starting thread work and refuses startup if it cannot open it.

The connector automatically keeps the immutable file objects outside the project, below its own
data directory (`file-states/`) or the environment's persistent home (`.orch/file-states/`). Preserve
both the server data and these machine-side roots; copying only the database cannot restore file bytes.
The defaults record included files for seven days, up to one GiB per state. Project ignore rules and
the environment's recording settings still apply. New thread directories prepare their indexed coverage
before the first recording answers; a live tree or genuinely incomplete capture stays unavailable.

## Blob collection and backups

`[retention] blob_grace_hours` defaults to 24. Complete each dump's separate blob copy after that dump and
within this grace measured from the dump's start; retain the pair together. A larger grace leaves more
time to make a copy and retains more unpinned data. See [Backup and restore](backup-and-restore.md).
