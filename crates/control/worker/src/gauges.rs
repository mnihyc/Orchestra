//! The operator's gauges that the database answers: threads by state, jobs and commands that are not over,
//! runtimes that stand, people's open shells, what each host last said it uses, and what the blob store holds.
//!
//! The worker sets them at its heartbeat, beside the gauges of its own work, for the collector that reads its
//! `metrics_bind`. A count that was set once and has nothing now stands at 0, so that a collector never keeps a
//! count that is no longer true.

use std::collections::BTreeSet;
use std::sync::{Mutex, PoisonError};

use orch_db::queries::metrics as rows;
use orch_db::{Conn, DbError};

/// One gauge's identity: its metric name and owned label values.
type GaugeSeries = (&'static str, Vec<(&'static str, String)>);

/// The labelled series set so far, by name and labels, so that one with nothing now is set to 0.
static SET: Mutex<BTreeSet<GaugeSeries>> = Mutex::new(BTreeSet::new());

/// A count as a gauge's value.
fn value(count: i64) -> f64 {
    count.max(0) as f64
}

/// Database-backed heartbeat/backup ages, sampled by both API and worker so a stopped worker stays visible.
/// A missing record has present=0; its zero age is never evidence of a successful observation.
pub async fn operational(
    conn: &mut Conn,
    at: chrono::DateTime<chrono::Utc>,
) -> Result<(), DbError> {
    let beats = orch_db::heartbeat::all(conn).await?;
    for component in ["api", "gateway", "worker"] {
        let beat = beats.iter().find(|beat| beat.component == component);
        metrics::gauge!("orch_component_heartbeat_present", "component" => component)
            .set(if beat.is_some() { 1.0 } else { 0.0 });
        let age = beat.map_or(0.0, |beat| {
            (at - beat.seen_at).num_milliseconds().max(0) as f64 / 1000.0
        });
        metrics::gauge!("orch_component_heartbeat_age_seconds", "component" => component).set(age);
    }
    let publication = orch_db::queries::installation::get(conn, "database_backup")
        .await?
        .and_then(|value| serde_json::from_value::<crate::backups::Publication>(value).ok());
    let backup = publication.and_then(|publication| publication.success);
    metrics::gauge!("orch_backup_present").set(if backup.is_some() { 1.0 } else { 0.0 });
    let age = backup.map_or(0.0, |backup| {
        (at - backup.record.completed_at.get())
            .num_milliseconds()
            .max(0) as f64
            / 1000.0
    });
    metrics::gauge!("orch_backup_age_seconds").set(age);
    Ok(())
}

/// How long a transaction of the services may stay open before the worker warns: five minutes. No
/// transaction of the system's own lasts more than seconds: a job that runs longer commits page by page
/// under a lease, and nothing waits on a machine, a vendor or a file with a transaction open. One open
/// that long holds back what the database may clean up everywhere, and a connection of a pool with it.
pub const TRANSACTION_WARN_AFTER_SECS: f64 = 300.0;

/// When the worker last warned of a transaction open too long, so that one that stays open is told
/// once every [`TRANSACTION_WARN_AFTER_SECS`] and not at every heartbeat.
static WARNED: Mutex<Option<std::time::Instant>> = Mutex::new(None);

/// Sets `orch_db_oldest_transaction_seconds` to how long the oldest transaction of the services has
/// been open (0 where none is), and warns where that is past [`TRANSACTION_WARN_AFTER_SECS`]. Says
/// the age, where a transaction is open.
pub async fn transactions(conn: &mut Conn) -> Result<Option<f64>, DbError> {
    let oldest = rows::oldest_open_transaction(conn).await?;
    let age = oldest.as_ref().map_or(0.0, |(age, _)| age.max(0.0));
    metrics::gauge!("orch_db_oldest_transaction_seconds").set(age);
    if let Some((age, state)) = &oldest
        && *age > TRANSACTION_WARN_AFTER_SECS
    {
        let mut warned = WARNED.lock().unwrap_or_else(PoisonError::into_inner);
        let every = std::time::Duration::from_secs_f64(TRANSACTION_WARN_AFTER_SECS);
        if warned.is_none_or(|at| at.elapsed() >= every) {
            *warned = Some(std::time::Instant::now());
            tracing::warn!(age_seconds = age.round(), %state,
                "a transaction of the system has been open for longer than any of its own should be");
        }
    }
    Ok(oldest.map(|(age, _)| age))
}

/// Available bytes on the filesystem that will hold the store. Both logical classes share
/// that filesystem; these are two views of its free room, never two quantities to add.
#[must_use]
pub fn free_bytes(root: &std::path::Path) -> Option<f64> {
    #[cfg(unix)]
    {
        let existing = root.ancestors().find(|path| path.exists())?;
        let stat = rustix::fs::statvfs(existing).ok()?;
        Some(stat.f_bavail as f64 * stat.f_frsize as f64)
    }
    #[cfg(not(unix))]
    {
        let _ = root;
        None
    }
}

/// A failed filesystem reading leaves the previous reading in place, rather than inventing zero free bytes.
pub fn storage_free(root: &std::path::Path) {
    if let Some(free) = free_bytes(root) {
        for class in ["pinned", "cache"] {
            metrics::gauge!("orch_blob_store_free_bytes","class"=>class).set(free);
        }
    }
}

/// Sets one series and remembers it.
fn set(
    seen: &mut BTreeSet<GaugeSeries>,
    name: &'static str,
    labels: Vec<(&'static str, String)>,
    count: i64,
) {
    let pairs: Vec<metrics::Label> = labels
        .iter()
        .map(|(key, value)| metrics::Label::new(*key, value.clone()))
        .collect();
    metrics::gauge!(name, pairs).set(value(count));
    seen.insert((name, labels));
}

/// Reads the counts and sets the gauges; those set before and absent now are set to 0.
pub async fn sample(conn: &mut Conn, at: chrono::DateTime<chrono::Utc>) -> Result<(), DbError> {
    let mut now: BTreeSet<GaugeSeries> = BTreeSet::new();
    for (state, count) in rows::threads_by_state(conn).await? {
        set(&mut now, "orch_threads", vec![("state", state)], count);
    }
    for (harness, status, count) in rows::harness_processes(conn).await? {
        set(
            &mut now,
            "orch_harness_processes",
            vec![("harness", harness), ("status", status)],
            count,
        );
    }
    let (cap, lock) = rows::turns_waiting(conn).await?;
    for (reason, count) in [("cap", cap), ("lock", lock)] {
        set(
            &mut now,
            "orch_turns_waiting",
            vec![("reason", reason.to_owned())],
            count,
        );
    }
    let mut tokens = std::collections::BTreeMap::<String, (i64, i64)>::new();
    let mut rated = std::collections::BTreeMap::<(String, String), f64>::new();
    for purpose in ["turn", "summary", "review"] {
        tokens.insert(purpose.to_owned(), (0, 0));
        for source in ["provider", "rate_card", "catalogue"] {
            rated.insert((purpose.to_owned(), source.to_owned()), 0.0);
        }
    }
    // One bounded set of purpose/source totals per minute. The append-only ledger is the
    // authority and survives a worker restart; no provider or pricing logic is duplicated here.
    for (purpose, source, total, credits, unrated) in rows::counted_usage(conn).await? {
        let counts = tokens.entry(purpose.clone()).or_default();
        counts.0 = counts.0.saturating_add(total);
        counts.1 = counts.1.saturating_add(unrated);
        if let Some(source) = source {
            rated.insert((purpose, source), credits);
        }
    }
    for ((purpose, source), credits) in rated {
        metrics::gauge!("orch_usage_credits_total","purpose"=>purpose,"source"=>source)
            .set(credits);
    }
    for (purpose, (total, unrated)) in tokens {
        metrics::counter!("orch_usage_tokens_total","purpose"=>purpose.clone())
            .absolute(u64::try_from(total).unwrap_or(0));
        metrics::counter!("orch_usage_unrated_tokens_total","purpose"=>purpose)
            .absolute(u64::try_from(unrated).unwrap_or(0));
    }
    if let Some(catalogue) = orch_db::usage::ledger::catalogue_in_force(conn).await? {
        for origin in ["shipped", "uploaded", "fetched"] {
            let age = if catalogue.origin == origin {
                (at.date_naive() - catalogue.as_of).num_days().max(0) as f64
            } else {
                0.0
            };
            metrics::gauge!("orch_rate_catalogue_age_days","origin"=>origin).set(age);
        }
    }
    for (kind, state, count) in rows::jobs_open(conn).await? {
        set(
            &mut now,
            "orch_jobs",
            vec![("kind", kind), ("state", state)],
            count,
        );
    }
    for (kind, driver, state, count) in rows::runtimes_standing(conn).await? {
        set(
            &mut now,
            "orch_runtimes",
            vec![("kind", kind), ("driver", driver), ("state", state)],
            count,
        );
    }
    for (kind, count) in rows::commands_open(conn).await? {
        set(&mut now, "orch_commands_open", vec![("kind", kind)], count);
    }
    for (state, count) in rows::terminal_sessions_open(conn).await? {
        set(
            &mut now,
            "orch_terminal_sessions",
            vec![("state", state)],
            count,
        );
    }
    // What a host said it uses is a reading, not a count: a host that no longer reports keeps its last one
    // rather than a 0 that would read as a full disk.
    for (host, cpu, memory, disk_used, disk_free) in rows::host_loads(conn).await? {
        for (name, reading) in [
            ("orch_host_cpu_percent", cpu),
            ("orch_host_memory_used_mb", memory),
            ("orch_host_disk_used_gb", disk_used),
            ("orch_host_disk_free_gb", disk_free),
        ] {
            if let Some(reading) = reading {
                metrics::gauge!(name, "host_id" => host.to_string()).set(reading);
            }
        }
    }
    let blobs = rows::blob_totals(conn).await?;
    set(
        &mut now,
        "orch_blob_store_bytes",
        vec![("class", "pinned".to_owned())],
        blobs.pinned_bytes,
    );
    set(
        &mut now,
        "orch_blob_store_bytes",
        vec![("class", "cache".to_owned())],
        blobs.cache_bytes,
    );
    let mut before = SET.lock().unwrap_or_else(PoisonError::into_inner);
    for (name, labels) in before.difference(&now) {
        let pairs: Vec<metrics::Label> = labels
            .iter()
            .map(|(key, value)| metrics::Label::new(*key, value.clone()))
            .collect();
        metrics::gauge!(*name, pairs).set(0.0);
    }
    before.extend(now);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn byte_counts_over_four_gibibytes_keep_their_size_and_negative_counts_are_zero() {
        assert!((value(7) - 7.0).abs() < f64::EPSILON);
        assert_eq!(
            value(5_i64 * 1024 * 1024 * 1024),
            5.0 * 1024.0 * 1024.0 * 1024.0
        );
        assert_eq!(value(-1), 0.0);
    }
}
