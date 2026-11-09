//! The most recent hundred authenticated calls of a key, retained for at most seven days.

use crate::{Conn, DbError};
use chrono::{DateTime, Duration, Utc};
use orch_types::{Id, Json};
use sqlx::Connection;

/// Existing accepted storage bound, shared by writer and recent reader.
pub const KEPT_CALLS: i64 = 100;
/// Existing accepted age bound; the worker also removes inactive keys' old rows.
pub const KEPT_DAYS: i64 = 7;

/// Safe metadata supplied by the shared operation context, never a request or response payload.
pub struct NewCall<'a> {
    /// The record identity.
    pub id: Id,
    /// The authenticated credential.
    pub token: Id,
    /// Its authenticated account; the writer checks this identity pair.
    pub account: Id,
    /// Server call time.
    pub at: DateTime<Utc>,
    /// Stable shared operation ID.
    pub operation: &'a str,
    /// Selected canonical variant, or none when validation never resolved one.
    pub selection: Option<&'a str>,
    /// Typed safe facet snapshots, with no names, queries, bodies or raw inputs.
    pub facets: &'a Json,
    /// Typed org ancestry from those same resolved facets, for the historical org slice.
    pub orgs: &'a [Id],
    /// Declared protocol method, not used to infer authority.
    pub method: &'a str,
    /// Declared route template or safe protocol label, never a raw URL.
    pub route: &'a str,
    /// Exact final response or protocol status.
    pub status: u16,
    /// Stable error code only, never the error's message or details.
    pub error: Option<&'a str>,
    /// Explicitly marked by a real idempotency producer.
    pub replayed: bool,
    /// Monotonic duration before outcome completion.
    pub duration_ms: i64,
    /// Existing request identity.
    pub request_id: &'a str,
    /// Already resolved through the server's trusted-proxy boundary.
    pub ip: Option<&'a str>,
}

/// A historical record. Facet names and other resource content are not stored here.
#[derive(Debug, Clone, sqlx::FromRow)]
pub struct Row {
    /// Record identity.
    pub id: Id,
    /// Server call time.
    pub at: DateTime<Utc>,
    /// Stable operation.
    pub operation: String,
    /// Canonical selected variant.
    pub selection: Option<String>,
    /// Safe facet snapshots; org-only readers project only their own slice.
    pub facets: Json,
    /// Declared method.
    pub method: String,
    /// Declared template/protocol.
    pub route: String,
    /// Exact status.
    pub status: i32,
    /// Stable error code.
    pub error_code: Option<String>,
    /// True only for a real replay.
    pub replayed: bool,
    /// Monotonic duration.
    pub duration_ms: i64,
    /// Existing request identity.
    pub request_id: String,
    /// Source IP under the reader's separate existing privacy projection.
    pub ip: Option<String>,
}

/// Appends once and trims this key atomically. Appends serialize independently of credential
/// policy changes; a shared key-lifetime fence preserves the actual owner/foreign-key relation.
pub async fn append(
    conn: &mut Conn,
    call: &NewCall<'_>,
    now: DateTime<Utc>,
) -> Result<bool, DbError> {
    let mut tx = conn.begin().await?;
    sqlx::query("SELECT pg_advisory_xact_lock(hashtextextended($1,0))")
        .bind(format!("api_token_activity:{}", call.token))
        .execute(&mut *tx)
        .await?;
    let key: Option<Id> =
        sqlx::query_scalar("SELECT id FROM api_tokens WHERE id=$1 AND user_id=$2 FOR KEY SHARE")
            .bind(call.token)
            .bind(call.account)
            .fetch_optional(&mut *tx)
            .await?;
    if key.is_none() {
        tx.rollback().await?;
        return Ok(false);
    }
    sqlx::query("INSERT INTO api_token_activity(id,token_id,at,operation,selection,facets,org_ids,method,route,status,error_code,replayed,duration_ms,request_id,ip) VALUES($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12,$13,$14,$15)")
        .bind(call.id).bind(call.token).bind(call.at).bind(call.operation).bind(call.selection)
        .bind(call.facets).bind(call.orgs).bind(call.method).bind(call.route).bind(i32::from(call.status))
        .bind(call.error).bind(call.replayed).bind(call.duration_ms).bind(call.request_id).bind(call.ip)
        .execute(&mut *tx).await?;
    sqlx::query("DELETE FROM api_token_activity WHERE token_id=$1 AND (at<$2 OR id IN(SELECT id FROM api_token_activity WHERE token_id=$1 ORDER BY at DESC,id DESC OFFSET $3))")
        .bind(call.token).bind(now-Duration::days(KEPT_DAYS)).bind(KEPT_CALLS).execute(&mut *tx).await?;
    tx.commit().await?;
    Ok(true)
}

/// Whether the existing key belongs to this account, including retained revoked keys.
pub async fn owned(conn: &mut Conn, token: Id, account: Id) -> Result<bool, DbError> {
    Ok(
        sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM api_tokens WHERE id=$1 AND user_id=$2)")
            .bind(token)
            .bind(account)
            .fetch_one(conn)
            .await?,
    )
}

/// Captures the current per-key read watermark. Appends serialize on the key before
/// allocating a sequence, so a later completion cannot enter an earlier read snapshot.
pub async fn snapshot(conn: &mut Conn, token: Id) -> Result<i64, DbError> {
    Ok(sqlx::query_scalar(
        "SELECT COALESCE(max(sequence),0) FROM api_token_activity WHERE token_id=$1",
    )
    .bind(token)
    .fetch_one(conn)
    .await?)
}

/// A bounded recent page, restricted to the first page's private sequence watermark and,
/// for an org reader, its recorded typed boundary. Age is rechecked on every page.
/// New self-read events cannot extend a traversal; normal retention can remove old rows.
pub async fn page(
    conn: &mut Conn,
    token: Id,
    org: Option<Id>,
    now: DateTime<Utc>,
    snapshot: i64,
    after: Option<(DateTime<Utc>, Id)>,
    limit: i64,
) -> Result<Vec<Row>, DbError> {
    let (after_at, after_id) = after.unzip();
    Ok(sqlx::query_as("SELECT id,at,operation,selection,facets,method,route,status,error_code,replayed,duration_ms,request_id,ip FROM api_token_activity WHERE token_id=$1 AND at>=$2 AND ($3::uuid IS NULL OR org_ids @> ARRAY[$3]::uuid[]) AND ($4::timestamptz IS NULL OR (at,id)<($4,$5::uuid)) AND sequence<=$6 ORDER BY at DESC,id DESC LIMIT $7")
        .bind(token).bind(now-Duration::days(KEPT_DAYS)).bind(org).bind(after_at).bind(after_id)
        .bind(snapshot).bind(limit.clamp(1,KEPT_CALLS+1)).fetch_all(conn).await?)
}

/// Removes old rows even for keys that have become inactive. The existing upkeep cadence
/// supplies its bounded batch size; readers always apply the exact age cutoff independently.
pub async fn purge(conn: &mut Conn, now: DateTime<Utc>, limit: i64) -> Result<u64, DbError> {
    Ok(sqlx::query("DELETE FROM api_token_activity WHERE id IN(SELECT id FROM api_token_activity WHERE at<$1 ORDER BY at,id LIMIT $2)")
        .bind(now-Duration::days(KEPT_DAYS)).bind(limit.max(1)).execute(conn).await?.rows_affected())
}
