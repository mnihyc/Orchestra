//! What the worker's schedule removes or trims: spent and expired sign-in material, soft-deleted rows after their days, old raw events and metric minutes, recents beyond a person's share, and the machine certificates that are about to end.
//!
//! Deliveries that are over and old rows of people's Inboxes go too
//! ([`purge_outbox`], [`purge_inbox`]). Every function takes the moment it
//! works at and removes what is due at that moment, and nothing else. A row that something still refers to is not
//! removed: the schema's own references decide when a soft-deleted org, a
//! provider or an environment may go, which is once nothing of it is left.

use crate::{Conn, DbError};
use chrono::{DateTime, Duration, Utc};
use orch_types::{Id, Json};
use sqlx::Connection;

/// How many rows a purge removes in one statement: enough that a backlog
/// clears in a few statements, few enough that one never holds the tables for
/// long.
const BATCH: i64 = 500;

/// How many passes the purge of soft-deleted rows makes over its tables. A
/// pass frees what the one before left referenced, so a tree of nodes goes in
/// as many passes as it is deep, and this is more than any path can be.
const SOFT_DELETE_PASSES: usize = 16;

/// How many raw events a run removes from a partition that is only partly
/// old: the rest go on the next day.
const RAW_EVENT_ROWS_PER_RUN: i64 = 50_000;

/// Takes the lock a scheduled job runs under, in the current transaction and
/// until it ends: the lock is named after the job, so that of however many
/// processes try, one holds it. Says whether this one does.
pub async fn try_lock_job(conn: &mut Conn, job: &str) -> Result<bool, DbError> {
    Ok(
        sqlx::query_scalar("SELECT pg_try_advisory_xact_lock(hashtextextended($1, 0))")
            .bind(format!("orch.job.{job}"))
            .fetch_one(conn)
            .await?,
    )
}

/// The newest migration the database has applied, or none where it has applied
/// nothing yet: a service that does not apply the schema itself waits for it.
pub async fn applied_schema_version(conn: &mut Conn) -> Result<Option<i64>, DbError> {
    let recorded: bool = sqlx::query_scalar("SELECT to_regclass('_sqlx_migrations') IS NOT NULL")
        .fetch_one(&mut *conn)
        .await?;
    if !recorded {
        return Ok(None);
    }
    Ok(
        sqlx::query_scalar("SELECT max(version) FROM _sqlx_migrations WHERE success")
            .fetch_one(conn)
            .await?,
    )
}

/// What was removed of the sign-in material that has run out.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct SignInPurged {
    /// Access tokens past their end.
    pub access_tokens: u64,
    /// Sessions past their end, which no cookie can renew.
    pub sessions: u64,
    /// Mailed links past their end.
    pub email_tokens: u64,
    /// Mailed codes past their end.
    pub sign_in_codes: u64,
    /// Browsers nobody got in from for their time.
    pub known_devices: u64,
    /// Single sign-on attempts past their end.
    pub oidc_states: u64,
    /// Enrollment tokens nobody used, past their end.
    pub enrollment_tokens: u64,
}

/// Removes the tokens, codes and attempts that are past their end at `now`,
/// and the browsers nobody signed in from for `known_device_days`.
pub async fn purge_expired_sign_in(
    conn: &mut Conn,
    now: DateTime<Utc>,
    known_device_days: i64,
) -> Result<SignInPurged, DbError> {
    let access_tokens = sqlx::query("DELETE FROM access_tokens WHERE expires_at <= $1")
        .bind(now)
        .execute(&mut *conn)
        .await?
        .rows_affected();
    // The access tokens of a session that ended go with it.
    let sessions = sqlx::query("DELETE FROM sessions WHERE expires_at <= $1")
        .bind(now)
        .execute(&mut *conn)
        .await?
        .rows_affected();
    let email_tokens = sqlx::query("DELETE FROM email_tokens WHERE expires_at <= $1")
        .bind(now)
        .execute(&mut *conn)
        .await?
        .rows_affected();
    let sign_in_codes = sqlx::query("DELETE FROM sign_in_codes WHERE expires_at <= $1")
        .bind(now)
        .execute(&mut *conn)
        .await?
        .rows_affected();
    let known_devices =
        sqlx::query("DELETE FROM known_devices WHERE last_at <= $1 - make_interval(days => $2)")
            .bind(now)
            .bind(i32::try_from(known_device_days).unwrap_or(i32::MAX))
            .execute(&mut *conn)
            .await?
            .rows_affected();
    let oidc_states = sqlx::query("DELETE FROM oidc_states WHERE expires_at <= $1")
        .bind(now)
        .execute(&mut *conn)
        .await?
        .rows_affected();
    let enrollment_tokens = sqlx::query(
        "DELETE FROM enrollment_tokens WHERE expires_at <= $1 AND used_at IS NULL AND machine_id IS NULL",
    )
    .bind(now)
    .execute(&mut *conn)
    .await?
    .rows_affected();
    Ok(SignInPurged {
        access_tokens,
        sessions,
        email_tokens,
        sign_in_codes,
        known_devices,
        oidc_states,
        enrollment_tokens,
    })
}

/// Deletes at most `limit` deliveries that are over, and says how many: one
/// that was sent or given up (`cancelled`) whose last try was before
/// `settled_before`, and one that failed for good whose last try was before
/// `failed_before`, which stays longer for a person who looks at why it
/// failed or retries it. A delivery's last try is the later of when it was
/// written and when it was last due. One that waits or is on its way stays,
/// and so does one another transaction holds.
pub async fn purge_outbox(
    conn: &mut Conn,
    settled_before: DateTime<Utc>,
    failed_before: DateTime<Utc>,
    limit: i64,
) -> Result<u64, DbError> {
    Ok(sqlx::query(
        "DELETE FROM outbox WHERE id IN (
           SELECT id FROM outbox
            WHERE (state IN ('sent', 'cancelled') AND GREATEST(created_at, next_attempt_at) < $1)
               OR (state = 'failed' AND GREATEST(created_at, next_attempt_at) < $2)
            LIMIT $3
              FOR UPDATE SKIP LOCKED)",
    )
    .bind(settled_before)
    .bind(failed_before)
    .bind(limit)
    .execute(conn)
    .await?
    .rows_affected())
}

/// Deletes at most `limit` rows of people's Inboxes that were written, or
/// last renewed, before `before`, read or not, and says how many.
pub async fn purge_inbox(
    conn: &mut Conn,
    before: DateTime<Utc>,
    limit: i64,
) -> Result<u64, DbError> {
    Ok(sqlx::query(
        "DELETE FROM notifications WHERE id IN (
           SELECT id FROM notifications WHERE created_at < $1
            LIMIT $2
              FOR UPDATE SKIP LOCKED)",
    )
    .bind(before)
    .bind(limit)
    .execute(conn)
    .await?
    .rows_affected())
}

/// The tables whose rows are soft-deleted, the ones that refer to others
/// first.
const SOFT_DELETED: &[&str] = &[
    "documents",
    "document_containers",
    "mcp_servers",
    "mounts",
    "skills",
    "providers",
    "environments",
    "agents",
    "env_templates",
    "machines",
    "nodes",
];

/// What the purge of soft-deleted rows removed from one table.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SoftDeletePurged {
    /// The table.
    pub table: &'static str,
    /// How many rows went.
    pub rows: u64,
}

/// Whether a name from the catalogue can stand in a statement as it is.
fn is_plain_name(name: &str) -> bool {
    !name.is_empty() && name.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_')
}

/// The tables and columns that refer to `table` and would refuse its row to
/// go: a single column, and no cascade or nulling.
async fn referrers(conn: &mut Conn, table: &str) -> Result<Vec<(String, String)>, DbError> {
    Ok(sqlx::query_as(
        "SELECT cl.relname::text, att.attname::text
           FROM pg_constraint c
           JOIN pg_class cl ON cl.oid = c.conrelid
           JOIN pg_attribute att ON att.attrelid = c.conrelid AND att.attnum = c.conkey[1]
          WHERE c.contype = 'f' AND c.confrelid = $1::text::regclass
            AND c.confdeltype IN ('a', 'r') AND cardinality(c.conkey) = 1
          ORDER BY 1, 2",
    )
    .bind(table)
    .fetch_all(conn)
    .await?)
}

/// Removes the rows that were soft-deleted at or before `cutoff` and that
/// nothing refers to any longer. A row that is still referred to stays, and
/// goes in a later run when what referred to it has gone; an org whose
/// environments or runtimes are still on hosts therefore stays until the
/// reaper has removed them and their rows are purged in turn. Rows go by the
/// hundred; one that the database refuses to remove for a reason the
/// references did not show is left alone.
pub async fn purge_soft_deleted(
    conn: &mut Conn,
    cutoff: DateTime<Utc>,
    now: DateTime<Utc>,
) -> Result<Vec<SoftDeletePurged>, DbError> {
    let mut totals: Vec<SoftDeletePurged> = SOFT_DELETED
        .iter()
        .map(|table| SoftDeletePurged { table, rows: 0 })
        .collect();
    let mut guards: Vec<String> = Vec::new();
    for table in SOFT_DELETED {
        let mut guard = if *table == "environments" {
            " AND (t.removal_phase IS NULL OR t.removal_phase='purged')".to_owned()
        } else {
            String::new()
        };
        if *table == "document_containers" {
            // Composite parent references are deliberately deferred for whole-tree moves.
            // Collect children first, including independently deleted children, before the parent.
            guard.push_str(" AND NOT EXISTS(SELECT 1 FROM documents d WHERE d.parent_container_id=t.id)
                AND NOT EXISTS(SELECT 1 FROM document_containers c WHERE c.parent_container_id=t.id)");
        }
        if matches!(*table, "documents" | "document_containers") {
            let kind = if *table == "documents" {
                "document"
            } else {
                "document_container"
            };
            guard.push_str(&format!(
                " AND NOT EXISTS(SELECT 1 FROM audit_object_state s WHERE s.object_kind='{kind}'
                AND s.object_id=t.id AND s.purge_at>$3)"
            ));
        }
        if *table == "nodes" {
            guard.push_str(" AND NOT EXISTS(SELECT 1 FROM audit_object_state s WHERE s.purge_at>$3
                AND ((s.object_kind='document' AND EXISTS(SELECT 1 FROM documents d WHERE d.id=s.object_id AND d.node_id=t.id))
                  OR (s.object_kind='document_container' AND EXISTS(SELECT 1 FROM document_containers c WHERE c.id=s.object_id AND c.node_id=t.id))))");
        }
        for (child, column) in referrers(conn, table).await? {
            if is_plain_name(&child) && is_plain_name(&column) {
                // A row that refers to itself (a root org is its own org) does
                // not hold itself back.
                let itself = if child == *table {
                    " AND x.id <> t.id"
                } else {
                    ""
                };
                guard.push_str(&format!(
                    " AND NOT EXISTS (SELECT 1 FROM \"{child}\" x WHERE x.\"{column}\" = t.id{itself}{})",
                    crate::queries::trace::live_reference_condition(&child)
                ));
            }
        }
        guards.push(guard);
    }
    // Rows the database refused in this run: they are not asked for again,
    // so that a few that cannot go never keep the others from it.
    let mut refused: Vec<Id> = Vec::new();
    for _ in 0..SOFT_DELETE_PASSES {
        let mut removed_any = false;
        for (index, table) in SOFT_DELETED.iter().enumerate() {
            let order = if *table == "nodes" {
                "cardinality(t.path) DESC, t.id"
            } else {
                "t.deleted_at, t.id"
            };
            let find = format!(
                "SELECT t.id FROM \"{table}\" t WHERE t.deleted_at <= LEAST($1,$3::timestamptz) AND t.id <> ALL($2){} ORDER BY {order} LIMIT {BATCH}",
                guards[index]
            );
            loop {
                let ids: Vec<Id> = sqlx::query_scalar(sqlx::AssertSqlSafe(find.clone()))
                    .bind(cutoff)
                    .bind(&refused)
                    .bind(now)
                    .fetch_all(&mut *conn)
                    .await?;
                if ids.is_empty() {
                    break;
                }
                let gone = delete_by_id(conn, table, &ids, cutoff, now, &guards[index]).await?;
                refused.extend(ids.iter().filter(|id| !gone.contains(id)));
                if !gone.is_empty() {
                    removed_any = true;
                    totals[index].rows += u64::try_from(gone.len()).unwrap_or(u64::MAX);
                }
                if ids.len() < usize::try_from(BATCH).unwrap_or(usize::MAX) {
                    break;
                }
            }
        }
        if !removed_any {
            break;
        }
    }
    totals.retain(|each| each.rows > 0);
    Ok(totals)
}

/// Removes rows of `table` by identifier and says which went. Where the
/// database refuses the batch because of a reference deeper than the first
/// level, the rows are tried one at a time and the ones it refuses are left.
/// Before an org is purged, the channels it set up go the way every removed channel goes: their
/// routes are kept, off, with what the channel was, their unsent work is cancelled, and whoever had
/// a route on or work waiting is told once; their sealed secrets go. The org's removal then takes
/// the channel rows with it.
async fn before_channels_go(
    conn: &mut Conn,
    table: &str,
    ids: &[Id],
    now: DateTime<Utc>,
) -> Result<(), DbError> {
    if table != "nodes" {
        return Ok(());
    }
    let channels: Vec<Id> = sqlx::query_scalar(
        "SELECT id FROM notification_channels WHERE org_id = ANY($1) ORDER BY id FOR UPDATE",
    )
    .bind(ids)
    .fetch_all(&mut *conn)
    .await?;
    for channel in channels {
        super::people::keep_routes_and_tell(conn, channel, now).await?;
        sqlx::query(
            "DELETE FROM secrets WHERE owner_kind = 'notification_channel' AND owner_id = $1",
        )
        .bind(channel)
        .execute(&mut *conn)
        .await?;
    }
    Ok(())
}

/// Before documents are purged, or the nodes their rows go with, the files they hold let go of the
/// pins they hold on their bytes, one for each file, as taking a file out of a document does: the
/// rows of the files go with the documents, and nothing would let those pins go after, so that the
/// bytes would be kept for good.
async fn before_documents_go(
    conn: &mut Conn,
    table: &str,
    ids: &[Id],
    now: DateTime<Utc>,
) -> Result<(), DbError> {
    let documents = match table {
        "documents" => ids.to_vec(),
        "nodes" => crate::queries::documents::of_nodes(conn, ids).await?,
        _ => return Ok(()),
    };
    if !documents.is_empty() {
        crate::queries::documents::release_file_pins(conn, &documents, now).await?;
    }
    Ok(())
}

async fn delete_by_id(
    conn: &mut Conn,
    table: &str,
    ids: &[Id],
    cutoff: DateTime<Utc>,
    now: DateTime<Utc>,
    guard: &str,
) -> Result<Vec<Id>, DbError> {
    let statement = format!("DELETE FROM \"{table}\" WHERE id = ANY($1) RETURNING id");
    let mut batch = conn.begin().await?;
    let first: Result<Vec<Id>, DbError> = async {
        let ids = purge_members(&mut batch, table, ids, cutoff, now, guard).await?;
        let anchors =
            crate::queries::documents::containers::purge_anchors(&mut batch, table, &ids).await?;
        crate::queries::trace::before_purge(&mut batch, table, &ids, now).await?;
        crate::queries::trace::collect_history(&mut batch, table, &ids, now).await?;
        before_channels_go(&mut batch, table, &ids, now).await?;
        before_documents_go(&mut batch, table, &ids, now).await?;
        let gone = sqlx::query_scalar(sqlx::AssertSqlSafe(statement.clone()))
            .bind(&ids)
            .fetch_all(&mut *batch)
            .await?;
        for node in anchors {
            crate::queries::hierarchy::stamp(&mut batch, node).await?;
        }
        Ok(gone)
    }
    .await;
    match first {
        Ok(gone) => {
            batch.commit().await?;
            Ok(gone)
        }
        Err(error) => {
            batch.rollback().await?;
            if !matches!(error, DbError::Reference { .. }) {
                return Err(error);
            }
            let mut gone = Vec::new();
            for id in ids {
                let mut one = conn.begin().await?;
                let result: Result<Vec<Id>, DbError> = async {
                    let ids = purge_members(
                        &mut one,
                        table,
                        std::slice::from_ref(id),
                        cutoff,
                        now,
                        guard,
                    )
                    .await?;
                    let anchors =
                        crate::queries::documents::containers::purge_anchors(&mut one, table, &ids)
                            .await?;
                    crate::queries::trace::before_purge(&mut one, table, &ids, now).await?;
                    crate::queries::trace::collect_history(&mut one, table, &ids, now).await?;
                    before_channels_go(&mut one, table, &ids, now).await?;
                    before_documents_go(&mut one, table, &ids, now).await?;
                    let gone = sqlx::query_scalar(sqlx::AssertSqlSafe(statement.clone()))
                        .bind(&ids)
                        .fetch_all(&mut *one)
                        .await?;
                    for node in anchors {
                        crate::queries::hierarchy::stamp(&mut one, node).await?;
                    }
                    Ok(gone)
                }
                .await;
                match result {
                    Ok(removed) => {
                        one.commit().await?;
                        gone.extend(removed);
                    }
                    Err(error) => {
                        one.rollback().await?;
                        if !matches!(error, DbError::Reference { .. }) {
                            return Err(error);
                        }
                    }
                }
            }
            Ok(gone)
        }
    }
}

/// The selection can have waited for a structural writer; recheck it under the scope and row locks.
async fn purge_members(
    conn: &mut Conn,
    table: &str,
    ids: &[Id],
    cutoff: DateTime<Utc>,
    now: DateTime<Utc>,
    guard: &str,
) -> Result<Vec<Id>, DbError> {
    crate::queries::documents::containers::lock_purge_scopes(conn, table, ids).await?;
    Ok(sqlx::query_scalar(sqlx::AssertSqlSafe(format!(
        "SELECT t.id FROM \"{table}\" t WHERE t.deleted_at<=LEAST($1,$3::timestamptz) AND t.id=ANY($2){guard} ORDER BY t.id FOR UPDATE")))
        .bind(cutoff).bind(ids).bind(now).fetch_all(conn).await?)
}

/// Removes the raw events older than `cutoff` from the partitions that are
/// only partly old, up to a bounded number of rows; the partitions that are
/// wholly old are dropped apart (`migrations::drop_partitions_before`).
pub async fn trim_raw_events(conn: &mut Conn, cutoff: DateTime<Utc>) -> Result<u64, DbError> {
    Ok(sqlx::query(
        "DELETE FROM raw_events WHERE (segment_id, n, at) IN
            (SELECT segment_id, n, at FROM raw_events WHERE at < $1 LIMIT $2)",
    )
    .bind(cutoff)
    .bind(RAW_EVENT_ROWS_PER_RUN)
    .execute(conn)
    .await?
    .rows_affected())
}

/// The first day before `before` that has a minute whose hour is not folded
/// into `metric_hours` yet, if there is one: the minutes of that day, and of
/// every day after it, are kept until the hours are folded.
pub async fn first_unfolded_day(
    conn: &mut Conn,
    before: DateTime<Utc>,
) -> Result<Option<DateTime<Utc>>, DbError> {
    Ok(sqlx::query_scalar(
        "SELECT min(date_trunc('day', s.at, 'UTC')) FROM metric_samples s
          WHERE s.at < $1 AND NOT EXISTS
                (SELECT 1 FROM metric_hours h
                  WHERE h.name = s.name AND h.labels = s.labels AND h.at = date_trunc('hour', s.at, 'UTC'))",
    )
    .bind(before)
    .fetch_one(conn)
    .await?)
}

/// The start of the newest hour that has a figure in `metric_hours`, if there
/// is one: where the next fold begins, since that hour may have had a minute
/// that arrived after it was folded.
pub async fn newest_folded_hour(conn: &mut Conn) -> Result<Option<DateTime<Utc>>, DbError> {
    Ok(sqlx::query_scalar("SELECT max(at) FROM metric_hours")
        .fetch_one(conn)
        .await?)
}

/// Folds the minutes of every hour that has ended by `now` into `metric_hours`,
/// one row for each series and hour: the mean of the minute values, the
/// largest, their sum and how many minutes had a value. Only minutes from
/// `since` on are read, so that a run does not read every minute that is kept;
/// an hour that was folded already is made again from its minutes, which is the
/// same row unless a minute arrived late. Says how many hourly rows were
/// written.
pub async fn fold_metric_hours(
    conn: &mut Conn,
    since: Option<DateTime<Utc>>,
    now: DateTime<Utc>,
) -> Result<u64, DbError> {
    Ok(sqlx::query(
        "INSERT INTO metric_hours (at, name, labels, mean, peak, total, samples)
         SELECT date_trunc('hour', s.at, 'UTC'), s.name, s.labels,
                avg(s.value), max(s.value), sum(s.value), count(*)
           FROM metric_samples s
          WHERE s.at < date_trunc('hour', $2::timestamptz, 'UTC')
            AND s.at >= coalesce($1::timestamptz, '-infinity'::timestamptz)
          GROUP BY date_trunc('hour', s.at, 'UTC'), s.name, s.labels
         ON CONFLICT (name, labels, at) DO UPDATE
            SET mean = EXCLUDED.mean, peak = EXCLUDED.peak,
                total = EXCLUDED.total, samples = EXCLUDED.samples",
    )
    .bind(since)
    .bind(now)
    .execute(conn)
    .await?
    .rows_affected())
}

/// Removes the hourly figures older than `before`.
pub async fn drop_metric_hours_before(
    conn: &mut Conn,
    before: DateTime<Utc>,
) -> Result<u64, DbError> {
    Ok(sqlx::query("DELETE FROM metric_hours WHERE at < $1")
        .bind(before)
        .execute(conn)
        .await?
        .rows_affected())
}

/// How a measurement of a minute combines with one already written for the
/// same minute, which happens when a sample is taken more than once in it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Measure {
    /// A gauge: the later value replaces the earlier.
    Gauge,
    /// A counter: what grew is added to what grew before in the minute.
    Counter,
}

/// Keeps one measurement of one minute: the value of a series at a minute,
/// combined with what the minute had by the rule of its kind.
pub async fn write_sample(
    conn: &mut Conn,
    minute: DateTime<Utc>,
    name: &str,
    labels: &Json,
    value: f64,
    measure: Measure,
) -> Result<(), DbError> {
    sqlx::query(
        "INSERT INTO metric_samples (at, name, labels, value) VALUES ($1, $2, $3, $4)
         ON CONFLICT (name, labels, at) DO UPDATE
            SET value = CASE WHEN $5 THEN metric_samples.value + EXCLUDED.value ELSE EXCLUDED.value END",
    )
    .bind(minute)
    .bind(name)
    .bind(labels)
    .bind(value)
    .bind(measure == Measure::Counter)
    .execute(conn)
    .await?;
    Ok(())
}

/// Keeps the `keep` newest recent visits of each person and removes the rest.
/// Says how many went.
pub async fn trim_recents(conn: &mut Conn, keep: i64) -> Result<u64, DbError> {
    Ok(sqlx::query(
        "DELETE FROM recents r USING
            (SELECT user_id, kind, id FROM
                (SELECT user_id, kind, id,
                        row_number() OVER (PARTITION BY user_id ORDER BY visited_at DESC, kind, id) AS place
                   FROM recents) ranked
              WHERE place > $1) excess
          WHERE r.user_id = excess.user_id AND r.kind = excess.kind AND r.id = excess.id",
    )
    .bind(keep)
    .execute(conn)
    .await?
    .rows_affected())
}

/// A machine whose certificate is about to end.
#[derive(Debug, Clone, PartialEq, sqlx::FromRow)]
pub struct ExpiringCertificate {
    /// The machine.
    pub machine_id: Id,
    /// Its name.
    pub name: String,
    /// The person it belongs to.
    pub owner_id: Id,
    /// When its certificate ends.
    pub not_after: DateTime<Utc>,
}

/// The connected machines of a person whose certificate ends within `ahead` of
/// `now`, or ended less than `grace` ago and can still be renewed.
pub async fn expiring_certificates(
    conn: &mut Conn,
    now: DateTime<Utc>,
    ahead: Duration,
    grace: Duration,
) -> Result<Vec<ExpiringCertificate>, DbError> {
    Ok(sqlx::query_as(
        "SELECT id AS machine_id, name, owner_id, cert_not_after AS not_after FROM machines
          WHERE status = 'active' AND deleted_at IS NULL AND owner_id IS NOT NULL
            AND cert_not_after <= $1 + make_interval(secs => $2::bigint::float8)
            AND cert_not_after > $1 - make_interval(secs => $3::bigint::float8)
          ORDER BY cert_not_after, id",
    )
    .bind(now)
    .bind(ahead.num_seconds())
    .bind(grace.num_seconds())
    .fetch_all(conn)
    .await?)
}
