//! Stored metric samples: written by each service, folded into hours that are kept, read as series over any span, and the minutes dropped a day at a time.
//!
//! The writers are the services' (`metric_samples` gets one value of a measurement for each
//! minute, `metric_hours` one row for each hour that has ended). What is read here is what
//! Management shows: a series over a span from the minutes or from the hours, at a step of its
//! own; and, from the other tables of the system, the figures that say how it stands (what waits in
//! its queues, what its blob store holds).

use crate::{Conn, DbError};
use chrono::{DateTime, Utc};
use orch_types::{Id, Json};

// ---- Management over HTTP: series over a span, and what the system holds ----

/// Which stored values a series is read from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Resolution {
    /// The value of each minute.
    Minutes,
    /// The row of each hour.
    Hours,
}

/// The suffix by which the catalogue of measurements names a counter: the figures that grow.
/// Every other measurement is a gauge, a figure that stands at a value.
pub const COUNTER_SUFFIX: &str = "_total";

/// Stores the aggregates observed inside one service process. Process labels keep simultaneous gauges distinct.
pub async fn write_process_samples(
    conn: &mut Conn,
    at: DateTime<Utc>,
    component: &str,
    process: Id,
    samples: &[orch_types::measurements::MetricSample],
) -> Result<usize, DbError> {
    use crate::queries::notifications::upkeep::{Measure, write_sample};
    use chrono::Timelike;
    let minute = at
        .with_second(0)
        .and_then(|at| at.with_nanosecond(0))
        .unwrap_or(at);
    let mut written = 0;
    for sample in samples {
        let Some(name) = sample.name.strip_prefix("orch_") else {
            continue;
        };
        if !sample.value.is_finite()
            || name.len() > 128
            || sample.labels.len() > 16
            || sample
                .labels
                .iter()
                .any(|(key, value)| key.len() > 64 || value.len() > 256)
        {
            continue;
        }
        let mut labels = serde_json::to_value(&sample.labels)
            .map_err(|error| DbError::from(sqlx::Error::Decode(Box::new(error))))?;
        if let Some(labels) = labels.as_object_mut() {
            labels.insert("component".to_owned(), Json::String(component.to_owned()));
            labels.insert("process".to_owned(), Json::String(process.to_string()));
        }
        write_sample(
            conn,
            minute,
            name,
            &labels,
            sample.value,
            if sample.counter {
                Measure::Counter
            } else {
                Measure::Gauge
            },
        )
        .await?;
        written += 1;
    }
    Ok(written)
}

/// Whether the measurement `name` is a counter, whose points are the sum of what it grew by; a
/// gauge's are the mean of what it stood at.
#[must_use]
pub fn is_counter(name: &str) -> bool {
    name.ends_with(COUNTER_SUFFIX)
}

/// One point of a series: the figure of one step.
#[derive(Debug, Clone, PartialEq, sqlx::FromRow)]
pub struct StepRow {
    /// The measurement.
    pub name: String,
    /// Its labels.
    pub labels: Json,
    /// The start of the step, aligned to the step from the first of January 1970 (UTC).
    pub at: DateTime<Utc>,
    /// A gauge's mean or a counter's sum over the step.
    pub value: f64,
    /// From hours: the largest minute value in the step; none from minutes.
    pub peak: Option<f64>,
}

/// The names of the measurements that have a value between `from` and `to`, both included, in
/// the stored values `resolution` names.
pub async fn measured_names(
    conn: &mut Conn,
    resolution: Resolution,
    from: DateTime<Utc>,
    to: DateTime<Utc>,
) -> Result<Vec<String>, DbError> {
    let statement = match resolution {
        Resolution::Minutes => {
            "SELECT DISTINCT name FROM metric_samples WHERE at >= $1 AND at <= $2 ORDER BY name"
        }
        Resolution::Hours => {
            "SELECT DISTINCT name FROM metric_hours WHERE at >= $1 AND at <= $2 ORDER BY name"
        }
    };
    Ok(sqlx::query_scalar(statement)
        .bind(from)
        .bind(to)
        .fetch_all(conn)
        .await?)
}

/// The points of the measurements `names` between `from` and `to`, both included, one for each
/// step of `step_minutes` that has a value, by measurement, labels and time. A gauge's point is
/// the mean of its values over the step (of the hours, weighted by how many minutes each had) and
/// a counter's the sum of what it grew by; from hours each point also has the largest minute
/// value of the step.
pub async fn steps(
    conn: &mut Conn,
    resolution: Resolution,
    names: &[String],
    from: DateTime<Utc>,
    to: DateTime<Utc>,
    step_minutes: i32,
) -> Result<Vec<StepRow>, DbError> {
    let statement = match resolution {
        Resolution::Minutes => {
            "SELECT s.name, s.labels,
                    date_bin(make_interval(mins => $4), s.at, TIMESTAMPTZ '1970-01-01 00:00:00+00') AS at,
                    (CASE WHEN right(s.name, length($5)) = $5 THEN sum(s.value) ELSE avg(s.value) END)::float8 AS value,
                    NULL::float8 AS peak
               FROM metric_samples s
              WHERE s.name = ANY($1) AND s.at >= $2 AND s.at <= $3
              GROUP BY s.name, s.labels, 3
              ORDER BY s.name, s.labels::text, 3"
        }
        Resolution::Hours => {
            "SELECT h.name, h.labels,
                    date_bin(make_interval(mins => $4), h.at, TIMESTAMPTZ '1970-01-01 00:00:00+00') AS at,
                    (CASE WHEN right(h.name, length($5)) = $5 THEN sum(h.total)
                          ELSE COALESCE(sum(h.mean * h.samples) / NULLIF(sum(h.samples), 0), 0) END)::float8 AS value,
                    max(h.peak)::float8 AS peak
               FROM metric_hours h
              WHERE h.name = ANY($1) AND h.at >= $2 AND h.at <= $3
              GROUP BY h.name, h.labels, 3
              ORDER BY h.name, h.labels::text, 3"
        }
    };
    Ok(sqlx::query_as(statement)
        .bind(names)
        .bind(from)
        .bind(to)
        .bind(step_minutes)
        .bind(COUNTER_SUFFIX)
        .fetch_all(conn)
        .await?)
}

/// What the blob store holds, from its index.
#[derive(Debug, Clone, Copy, PartialEq, Eq, sqlx::FromRow)]
pub struct BlobTotals {
    /// How many blobs.
    pub count: i64,
    /// Their size in bytes.
    pub bytes: i64,
    /// The bytes of blobs something refers to, which are kept.
    pub pinned_bytes: i64,
    /// The bytes of blobs nothing refers to but the cache of files, which may be evicted.
    pub cache_bytes: i64,
}

/// Threads by the state they are in, those deleted left out.
pub async fn threads_by_state(conn: &mut Conn) -> Result<Vec<(String, i64)>, DbError> {
    Ok(sqlx::query_as(
        "SELECT t.state, count(*)::bigint FROM threads t JOIN nodes n ON n.id = t.node_id
          WHERE n.deleted_at IS NULL GROUP BY t.state ORDER BY t.state",
    )
    .fetch_all(conn)
    .await?)
}

/// Counted usage by its recorded purpose and rate source; trace-only duplicates never enter these totals.
/// Credits are converted to float only for observations, never for billing or admission.
pub async fn counted_usage(
    conn: &mut Conn,
) -> Result<Vec<(String, Option<String>, i64, f64, i64)>, DbError> {
    Ok(sqlx::query_as(
        "SELECT purpose, rate_source, COALESCE(sum(total_tokens),0)::bigint,
        COALESCE(sum(credits),0)::float8,
        COALESCE(sum(total_tokens) FILTER (WHERE credits IS NULL),0)::bigint
        FROM usage_events WHERE counted GROUP BY purpose,rate_source ORDER BY purpose,rate_source",
    )
    .fetch_all(conn)
    .await?)
}

/// The harness groups the server currently knows, by kind and process state.
pub async fn harness_processes(conn: &mut Conn) -> Result<Vec<(String, String, i64)>, DbError> {
    Ok(sqlx::query_as(
        "SELECT a.harness_kind,h.status,count(*)::bigint FROM harnesses h
        JOIN agents a ON a.id=h.agent_id GROUP BY a.harness_kind,h.status ORDER BY 1,2",
    )
    .fetch_all(conn)
    .await?)
}

/// Distinct conversations waiting for cap room, or held by a switch/runtime-change lock.
pub async fn turns_waiting(conn: &mut Conn) -> Result<(i64, i64), DbError> {
    Ok(sqlx::query_as("SELECT
        count(*) FILTER (WHERE t.blocked_reason='quota_exceeded' OR EXISTS
            (SELECT 1 FROM thread_inputs i WHERE i.thread_id=t.node_id AND i.state='pending' AND i.state_detail='cap_reached'))::bigint,
        count(*) FILTER (WHERE t.blocked_reason='switching')::bigint
        FROM threads t JOIN nodes n ON n.id=t.node_id WHERE n.deleted_at IS NULL
        AND EXISTS (SELECT 1 FROM thread_inputs i WHERE i.thread_id=t.node_id AND i.state='pending')")
        .fetch_one(conn).await?)
}

/// Jobs that are not over, by kind and state.
pub async fn jobs_open(conn: &mut Conn) -> Result<Vec<(String, String, i64)>, DbError> {
    Ok(sqlx::query_as(
        "SELECT kind, state, count(*)::bigint FROM jobs WHERE state IN ('queued', 'running')
          GROUP BY kind, state ORDER BY kind, state",
    )
    .fetch_all(conn)
    .await?)
}

/// Runtimes that stand, by kind, driver and state.
pub async fn runtimes_standing(
    conn: &mut Conn,
) -> Result<Vec<(String, String, String, i64)>, DbError> {
    Ok(sqlx::query_as(
        "SELECT kind, driver, state, count(*)::bigint FROM runtimes
          WHERE successor_of IS NULL AND state NOT IN ('retired', 'removed')
          GROUP BY kind, driver, state ORDER BY kind, driver, state",
    )
    .fetch_all(conn)
    .await?)
}

/// Commands to machines that are not over, by kind.
pub async fn commands_open(conn: &mut Conn) -> Result<Vec<(String, i64)>, DbError> {
    Ok(sqlx::query_as(
        "SELECT kind, count(*)::bigint FROM commands WHERE state IN ('queued', 'sent', 'accepted')
          GROUP BY kind ORDER BY kind",
    )
    .fetch_all(conn)
    .await?)
}

/// People's shells that are open, by whether anybody has them on a screen: `attached` or `detached`.
pub async fn terminal_sessions_open(conn: &mut Conn) -> Result<Vec<(String, i64)>, DbError> {
    Ok(sqlx::query_as(
        "SELECT CASE WHEN attached THEN 'attached' ELSE 'detached' END AS state, count(*)::bigint
           FROM terminal_sessions WHERE state <> 'closed' GROUP BY 1 ORDER BY 1",
    )
    .fetch_all(conn)
    .await?)
}

/// What each host last reported it uses, for those that reported: processor percent, memory used in MiB, and
/// disk used and free in GB, each where the report had it.
pub async fn host_loads(
    conn: &mut Conn,
) -> Result<Vec<(Id, Option<f64>, Option<f64>, Option<f64>, Option<f64>)>, DbError> {
    Ok(sqlx::query_as(
        "SELECT id, (load->>'cpu_percent')::float8, (load->>'memory_used_mb')::float8,
                (load->>'disk_used_gb')::float8, (load->>'disk_free_gb')::float8
           FROM hosts WHERE load_at IS NOT NULL ORDER BY id",
    )
    .fetch_all(conn)
    .await?)
}

/// What the blob store holds.
pub async fn blob_totals(conn: &mut Conn) -> Result<BlobTotals, DbError> {
    Ok(sqlx::query_as(
        "SELECT count(*)::bigint AS count,
                COALESCE(sum(size), 0)::bigint AS bytes,
                COALESCE(sum(size) FILTER (WHERE pin_count > 0), 0)::bigint AS pinned_bytes,
                COALESCE(sum(size) FILTER (WHERE pin_count = 0), 0)::bigint AS cache_bytes
           FROM blobs",
    )
    .fetch_one(conn)
    .await?)
}

/// What the cache of files keeps of one environment.
#[derive(Debug, Clone, Copy, PartialEq, Eq, sqlx::FromRow)]
pub struct EnvironmentBytes {
    /// The environment.
    pub environment_id: Id,
    /// The bytes of its cached blobs that nothing else refers to.
    pub cache_bytes: i64,
    /// The bytes of its cached blobs that something refers to.
    pub pinned_bytes: i64,
}

/// The environments whose files take the most room in the blob store, the heaviest first, at most
/// `limit`. A blob that several files of one environment share counts once for it.
pub async fn heaviest_environments(
    conn: &mut Conn,
    limit: i64,
) -> Result<Vec<EnvironmentBytes>, DbError> {
    Ok(sqlx::query_as(
        "SELECT f.environment_id,
                COALESCE(sum(b.size) FILTER (WHERE b.pin_count = 0), 0)::bigint AS cache_bytes,
                COALESCE(sum(b.size) FILTER (WHERE b.pin_count > 0), 0)::bigint AS pinned_bytes
           FROM (SELECT DISTINCT environment_id, hash FROM file_cache) f
           JOIN blobs b ON b.hash = f.hash
          GROUP BY f.environment_id
          ORDER BY sum(b.size) DESC, f.environment_id
          LIMIT $1",
    )
    .bind(limit)
    .fetch_all(conn)
    .await?)
}

/// How many deliveries wait in the outbox.
pub async fn outbox_pending(conn: &mut Conn) -> Result<i64, DbError> {
    Ok(
        sqlx::query_scalar("SELECT count(*) FROM outbox WHERE state = 'pending'")
            .fetch_one(conn)
            .await?,
    )
}

/// How long, in seconds, the oldest transaction that another session of the system's database holds
/// open has been open, and what that session does now (`active`, or `idle in transaction` between its
/// statements); none where no transaction is open. Only the services' own sessions count: the
/// database's own workers and an operator's `pg_dump`, whose one transaction lasts the dump, do not.
pub async fn oldest_open_transaction(conn: &mut Conn) -> Result<Option<(f64, String)>, DbError> {
    Ok(sqlx::query_as(
        "SELECT EXTRACT(EPOCH FROM clock_timestamp() - xact_start)::float8, COALESCE(state, '')
           FROM pg_stat_activity
          WHERE datname = current_database() AND pid <> pg_backend_pid()
            AND backend_type = 'client backend' AND xact_start IS NOT NULL
            AND COALESCE(application_name, '') <> 'pg_dump'
          ORDER BY xact_start LIMIT 1",
    )
    .fetch_optional(conn)
    .await?)
}

/// How many threads are running a turn, those that were not deleted.
pub async fn running_threads(conn: &mut Conn) -> Result<i64, DbError> {
    Ok(sqlx::query_scalar(
        "SELECT count(*) FROM threads t JOIN nodes n ON n.id = t.node_id
          WHERE t.state = 'running' AND n.deleted_at IS NULL",
    )
    .fetch_one(conn)
    .await?)
}

/// How many machines are connected now, by the last that is known of each.
pub async fn machines_online(conn: &mut Conn) -> Result<i64, DbError> {
    Ok(sqlx::query_scalar(
        "SELECT count(*) FROM machines WHERE online AND status = 'active' AND deleted_at IS NULL",
    )
    .fetch_one(conn)
    .await?)
}

// Host maintenance: read only series carrying this host's label.

/// Stored host measurements grouped into bounded time steps.
pub async fn host_steps(
    conn: &mut Conn,
    host: Id,
    resolution: Resolution,
    from: DateTime<Utc>,
    to: DateTime<Utc>,
    step_minutes: i32,
) -> Result<Vec<StepRow>, DbError> {
    let statement=match resolution {
        Resolution::Minutes=>"SELECT name, labels, date_bin(make_interval(mins => $4),at,TIMESTAMPTZ '1970-01-01 00:00:00+00') AS at, avg(value)::float8 AS value, NULL::float8 AS peak
          FROM metric_samples WHERE labels ->> 'host_id' = $1::text AND at >= $2 AND at <= $3 GROUP BY name,labels,3 ORDER BY name,labels::text,3",
        Resolution::Hours=>"SELECT name, labels, date_bin(make_interval(mins => $4),at,TIMESTAMPTZ '1970-01-01 00:00:00+00') AS at, (sum(mean * samples)/NULLIF(sum(samples),0))::float8 AS value, max(peak)::float8 AS peak
          FROM metric_hours WHERE labels ->> 'host_id' = $1::text AND at >= $2 AND at <= $3 GROUP BY name,labels,3 ORDER BY name,labels::text,3",
    };
    Ok(sqlx::query_as(statement)
        .bind(host)
        .bind(from)
        .bind(to)
        .bind(step_minutes)
        .fetch_all(conn)
        .await?)
}
