//! A provider's explicit public-relay configuration, separate from discovery and thread access.

use crate::{Conn, DbError};
use orch_types::{Id, Json};

/// Read the actual configurations of an already admitted provider page in one query.
pub async fn configurations(conn: &mut Conn, providers: &[Id]) -> Result<Vec<(Id, Json)>, DbError> {
    if providers.is_empty() {
        return Ok(Vec::new());
    }
    Ok(
        sqlx::query_as("SELECT id,relay FROM providers WHERE id=ANY($1)")
            .bind(providers)
            .fetch_all(conn)
            .await?,
    )
}

/// Read the one authoritative configuration for a fresh model call.
pub async fn configuration(conn: &mut Conn, provider: Id) -> Result<Option<Json>, DbError> {
    Ok(
        sqlx::query_scalar("SELECT relay FROM providers WHERE id=$1 AND deleted_at IS NULL")
            .bind(provider)
            .fetch_optional(conn)
            .await?,
    )
}

/// Immutable operands of an admitted call; request and response bodies are absent.
#[derive(Debug, Clone, sqlx::FromRow)]
pub struct Call {
    pub id: Id,
    pub provider_id: Id,
    pub machine_id: Option<Id>,
    pub user_id: Id,
    pub entry_id: Option<Id>,
    pub home_id: Id,
    pub org_id: Id,
    pub model_name: String,
    pub effort: Option<String>,
    pub protocol: String,
    pub path: String,
    pub relay_id: Id,
    pub request_hash: String,
    pub request_bytes: i64,
    pub max_request_bytes: i64,
    pub idle_secs: i64,
    pub retry_ms: i64,
    pub model_parameters: Json,
    pub headers: Json,
    pub command_body: Json,
    pub source: Json,
    pub created_at: chrono::DateTime<chrono::Utc>,
    pub deadline: chrono::DateTime<chrono::Utc>,
    pub finished_at: Option<chrono::DateTime<chrono::Utc>>,
    pub native_done_at: Option<chrono::DateTime<chrono::Utc>>,
}

/// Retain actual call identity and attribution before its native work is queued.
pub async fn create_call(conn: &mut Conn, call: &Call) -> Result<(), DbError> {
    sqlx::query("INSERT INTO provider_relay_calls
        (id,provider_id,machine_id,user_id,entry_id,home_id,org_id,model_name,protocol,path,relay_id,
         request_hash,request_bytes,max_request_bytes,idle_secs,retry_ms,model_parameters,headers,source,created_at,deadline,effort,command_body)
        VALUES($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12,$13,$14,$15,$16,$17,$18,$19,$20,$21,$22,$23)")
        .bind(call.id).bind(call.provider_id).bind(call.machine_id).bind(call.user_id).bind(call.entry_id)
        .bind(call.home_id).bind(call.org_id).bind(&call.model_name).bind(&call.protocol).bind(&call.path)
        .bind(call.relay_id).bind(&call.request_hash).bind(call.request_bytes).bind(call.max_request_bytes)
        .bind(call.idle_secs).bind(call.retry_ms).bind(&call.model_parameters).bind(&call.headers).bind(&call.source)
        .bind(call.created_at).bind(call.deadline).bind(&call.effort).bind(&call.command_body).execute(conn).await?;
    Ok(())
}

/// Select the same retained call for native source revalidation or actual usage retention.
pub async fn call(conn: &mut Conn, id: Id) -> Result<Option<Call>, DbError> {
    Ok(
        sqlx::query_as("SELECT * FROM provider_relay_calls WHERE id=$1")
            .bind(id)
            .fetch_optional(conn)
            .await?,
    )
}

/// Hold the call's original association while current authority and its first effect are checked.
pub async fn lock_call(conn: &mut Conn, id: Id) -> Result<bool, DbError> {
    Ok(
        sqlx::query_scalar::<_, Id>("SELECT id FROM provider_relay_calls WHERE id=$1 FOR SHARE")
            .bind(id)
            .fetch_optional(conn)
            .await?
            .is_some(),
    )
}

/// Body completion and native completion are distinct; billing does not close a reader's source.
pub async fn finish_call(
    conn: &mut Conn,
    id: Id,
    at: chrono::DateTime<chrono::Utc>,
) -> Result<(), DbError> {
    sqlx::query("UPDATE provider_relay_calls SET finished_at=COALESCE(finished_at,$2) WHERE id=$1")
        .bind(id)
        .bind(at)
        .execute(conn)
        .await?;
    Ok(())
}

pub async fn native_done(
    conn: &mut Conn,
    id: Id,
    at: chrono::DateTime<chrono::Utc>,
) -> Result<(), DbError> {
    sqlx::query(
        "UPDATE provider_relay_calls SET native_done_at=COALESCE(native_done_at,$2) WHERE id=$1",
    )
    .bind(id)
    .bind(at)
    .execute(conn)
    .await?;
    Ok(())
}

/// Bind the exact native operands once after runtime preparation; a later call cannot reuse them.
pub async fn bind_command(
    conn: &mut Conn,
    id: Id,
    machine: Id,
    body: &Json,
) -> Result<bool, DbError> {
    Ok(sqlx::query("UPDATE provider_relay_calls SET machine_id=$2,command_body=$3 WHERE id=$1 AND finished_at IS NULL AND machine_id IS NULL AND command_body='{}'::jsonb")
        .bind(id).bind(machine).bind(body).execute(conn).await?.rows_affected()==1)
}

/// Retain native completion independently of an HTTP reader or a usage event.
pub async fn command_done(
    conn: &mut Conn,
    machine: Id,
    id: Id,
    at: chrono::DateTime<chrono::Utc>,
) -> Result<(), DbError> {
    sqlx::query(
        "UPDATE provider_relay_calls r SET native_done_at=COALESCE(r.native_done_at,$3)
        FROM commands c WHERE r.id=$1 AND r.machine_id=$2 AND c.id=r.id AND c.machine_id=$2
        AND c.kind='provider.relay' AND c.body=r.command_body",
    )
    .bind(id)
    .bind(machine)
    .bind(at)
    .execute(conn)
    .await?;
    Ok(())
}

/// Removing an opened model ends its already accepted calls and cannot revive them on a later enable.
pub async fn close_unopened(
    conn: &mut Conn,
    provider: Id,
    enabled: bool,
    models: &[String],
    at: chrono::DateTime<chrono::Utc>,
) -> Result<Vec<Id>, DbError> {
    Ok(sqlx::query_scalar("UPDATE provider_relay_calls SET finished_at=$4
        WHERE provider_id=$1 AND finished_at IS NULL AND (NOT $2 OR NOT model_name=ANY($3)) RETURNING id")
        .bind(provider).bind(enabled).bind(models).bind(at).fetch_all(conn).await?)
}
