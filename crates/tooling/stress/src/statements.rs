//! The database's own account of the window (optional, with a database URL): the statements that ran over a
//! threshold, from `pg_stat_statements`.
//!
//! The view is read twice, when the window opens and when it closes, and never reset (other people's measurements
//! use it too). A statement's runs in the window are the difference of its counters; its slowest run is known to be
//! in the window when its `max_exec_time` rose during it. Statements of the same database by other clients in the
//! same window are counted too: the report says so.

use anyhow::{Context, Result};
use serde::Serialize;
use sqlx::Row;
use sqlx::postgres::PgPoolOptions;
use std::collections::HashMap;

#[derive(Debug, Clone)]
struct Counters {
    calls: i64,
    total_ms: f64,
    max_ms: f64,
    query: String,
}

/// The view at one moment, by statement.
pub struct Snapshot {
    rows: HashMap<i64, Counters>,
}

/// A statement that ran over the threshold in the window.
#[derive(Debug, Clone, Serialize)]
pub struct Slow {
    pub queryid: i64,
    pub calls: i64,
    pub mean_ms: f64,
    /// Its slowest run, when that run was in the window.
    pub max_ms: Option<f64>,
    pub total_ms: f64,
    pub query: String,
}

/// Reads `pg_stat_statements` of the URL's database.
pub async fn snapshot(url: &str) -> Result<Snapshot> {
    let pool = PgPoolOptions::new()
        .max_connections(1)
        .connect(url)
        .await
        .context("connecting to the database")?;
    let rows = sqlx::query(
        "SELECT queryid, calls, total_exec_time, max_exec_time, left(regexp_replace(query, '\\s+', ' ', 'g'), 600) AS query \
         FROM pg_stat_statements WHERE dbid = (SELECT oid FROM pg_database WHERE datname = current_database()) \
         AND queryid IS NOT NULL",
    )
    .fetch_all(&pool)
    .await
    .context("reading pg_stat_statements (is the extension installed in this database?)")?;
    pool.close().await;
    let mut out = HashMap::new();
    for row in rows {
        let id: i64 = row.try_get("queryid")?;
        let c = Counters {
            calls: row.try_get("calls")?,
            total_ms: row.try_get("total_exec_time")?,
            max_ms: row.try_get("max_exec_time")?,
            query: row.try_get("query")?,
        };
        // The same statement of several users or top-levels is one statement here.
        out.entry(id)
            .and_modify(|e: &mut Counters| {
                e.calls += c.calls;
                e.total_ms += c.total_ms;
                e.max_ms = e.max_ms.max(c.max_ms);
            })
            .or_insert(c);
    }
    Ok(Snapshot { rows: out })
}

/// The statements whose mean in the window, or whose slowest run in the window, is over `threshold_ms`, slowest first.
pub fn slow(before: &Snapshot, after: &Snapshot, threshold_ms: f64) -> Vec<Slow> {
    let mut out: Vec<Slow> = after
        .rows
        .iter()
        .filter_map(|(id, a)| {
            let b = before.rows.get(id);
            let calls = a.calls - b.map_or(0, |b| b.calls);
            if calls <= 0 {
                return None;
            }
            let total = a.total_ms - b.map_or(0.0, |b| b.total_ms);
            let mean = total / calls as f64;
            let max = match b {
                Some(b) if a.max_ms <= b.max_ms => None,
                _ => Some(a.max_ms),
            };
            (mean > threshold_ms || max.is_some_and(|m| m > threshold_ms)).then(|| Slow {
                queryid: *id,
                calls,
                mean_ms: mean,
                max_ms: max,
                total_ms: total,
                query: a.query.clone(),
            })
        })
        .collect();
    out.sort_by(|x, y| {
        y.max_ms
            .unwrap_or(y.mean_ms)
            .total_cmp(&x.max_ms.unwrap_or(x.mean_ms))
    });
    out
}

/// What the database's sessions were doing during the window, sampled every few seconds from
/// `pg_stat_activity`: how many were working, waiting on a lock or held idle inside a transaction, and which
/// statements they were held after. Transactions older than the window (a leader's lease, say) are left out.
#[derive(Debug, Clone, Default, Serialize)]
pub struct Activity {
    pub samples: u64,
    /// The most sessions seen at once in each state (`active`, `waiting on a lock`, `idle in transaction`).
    pub peak: std::collections::BTreeMap<String, i64>,
    /// Sessions held idle in a transaction for over a second, by the statement they last ran.
    pub idle_in_transaction: Vec<Held>,
    /// Sessions waiting on a lock for over a second, by what they wait on and their statement.
    pub lock_waits: Vec<Held>,
}

/// A statement seen held for over a second.
#[derive(Debug, Clone, Serialize)]
pub struct Held {
    /// The wait (`Lock:tuple`) or the state.
    pub what: String,
    pub query: String,
    /// In how many samples a session was seen so.
    pub seen: u64,
    /// The longest it was held, in seconds.
    pub max_secs: f64,
}

/// Samples `pg_stat_activity` of the URL's database every `every` until `stop`.
pub async fn sample_activity(
    url: String,
    every: std::time::Duration,
    stop: tokio_util::sync::CancellationToken,
) -> Result<Activity> {
    let pool = PgPoolOptions::new()
        .max_connections(1)
        .connect(&url)
        .await
        .context("connecting to the database")?;
    let started = std::time::Instant::now();
    let mut out = Activity::default();
    let mut idle: HashMap<String, Held> = HashMap::new();
    let mut waits: HashMap<(String, String), Held> = HashMap::new();
    loop {
        let window = started.elapsed().as_secs_f64();
        let rows = sqlx::query(
            "SELECT coalesce(state, '') AS state, coalesce(wait_event_type, '') || ':' || coalesce(wait_event, '') AS wait, \
                    extract(epoch FROM clock_timestamp() - coalesce(CASE WHEN state = 'active' THEN query_start ELSE state_change END, xact_start))::float8 AS held, \
                    coalesce(extract(epoch FROM clock_timestamp() - xact_start)::float8, 0) AS xact, \
                    left(regexp_replace(query, '\\s+', ' ', 'g'), 300) AS query \
             FROM pg_stat_activity WHERE datname = current_database() AND pid <> pg_backend_pid() AND backend_type = 'client backend'",
        )
        .fetch_all(&pool)
        .await;
        if let Ok(rows) = rows {
            out.samples += 1;
            let mut counts: std::collections::BTreeMap<String, i64> =
                std::collections::BTreeMap::new();
            for row in rows {
                let state: String = row.try_get("state").unwrap_or_default();
                let wait: String = row.try_get("wait").unwrap_or_default();
                let held: f64 = row.try_get("held").unwrap_or(0.0);
                let xact: f64 = row.try_get("xact").unwrap_or(0.0);
                let query: String = row.try_get("query").unwrap_or_default();
                if xact > window + 5.0 {
                    continue;
                }
                let kind = if state == "active" && wait.starts_with("Lock:") {
                    "waiting on a lock"
                } else if state.starts_with("idle in transaction") {
                    "idle in transaction"
                } else if state == "active" {
                    "active"
                } else {
                    continue;
                };
                *counts.entry(kind.to_owned()).or_default() += 1;
                if held < 1.0 {
                    continue;
                }
                if kind == "idle in transaction" {
                    let entry = idle.entry(query.clone()).or_insert_with(|| Held {
                        what: state.clone(),
                        query,
                        seen: 0,
                        max_secs: 0.0,
                    });
                    entry.seen += 1;
                    entry.max_secs = entry.max_secs.max(held);
                } else if kind == "waiting on a lock" {
                    let entry = waits
                        .entry((wait.clone(), query.clone()))
                        .or_insert_with(|| Held {
                            what: wait.clone(),
                            query,
                            seen: 0,
                            max_secs: 0.0,
                        });
                    entry.seen += 1;
                    entry.max_secs = entry.max_secs.max(held);
                }
            }
            for (kind, n) in counts {
                let peak = out.peak.entry(kind).or_default();
                *peak = (*peak).max(n);
            }
        }
        tokio::select! {
            () = stop.cancelled() => break,
            () = tokio::time::sleep(every) => {}
        }
    }
    pool.close().await;
    let mut idle: Vec<Held> = idle.into_values().collect();
    idle.sort_by_key(|h| std::cmp::Reverse(h.seen));
    let mut waits: Vec<Held> = waits.into_values().collect();
    waits.sort_by_key(|h| std::cmp::Reverse(h.seen));
    out.idle_in_transaction = idle;
    out.lock_waits = waits;
    Ok(out)
}
