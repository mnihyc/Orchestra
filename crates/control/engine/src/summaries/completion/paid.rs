//! Immutable native command journal; shared paid completion is the only ledger writer.
use super::*;
use crate::commands::CommandAuthority;
use crate::tools::capability_service::PaidAttribution;
use orch_types::Time;

pub(super) async fn read(engine: &Engine, id: Id) -> Result<Option<Outgoing>, EngineError> {
    let mut conn = engine.db().acquire().await?;
    let stored: Option<Json> = sqlx::query_scalar::<_, Option<Json>>(
        "SELECT prepared_command FROM summary_calls WHERE id=$1 AND purpose='summary'",
    )
    .bind(id)
    .fetch_optional(&mut *conn)
    .await
    .map_err(DbError::from)?
    .flatten();
    stored
        .map(serde_json::from_value)
        .transpose()
        .map_err(EngineError::from)
}

pub(super) async fn attribution(
    conn: &mut Conn,
    call: &Call,
) -> Result<PaidAttribution, EngineError> {
    let (org, project) = orch_db::usage::ledger::summary_place(conn, call.topic_id)
        .await?
        .ok_or_else(unavailable)?;
    Ok(PaidAttribution {
        place: call.node_id,
        org,
        project,
        topic: call.topic_id,
        thread: call.thread_id,
        conversation: None,
        person: call.issued_by,
        purpose: "summary".into(),
        harness_kind: None,
    })
}

pub(super) async fn journal(engine: &Engine, outgoing: Outgoing) -> Result<Outgoing, EngineError> {
    let authority = outgoing.authority.as_ref().ok_or_else(unavailable)?;
    let mut tx = engine.db().begin().await?;
    orch_db::queries::transaction_guard::lock_action(&mut tx).await?;
    let key = outgoing.key.wire();
    let proposed = orch_db::queries::link::NewCommand {
        id: outgoing.id,
        machine_id: outgoing.machine,
        kind: outgoing.kind.as_str(),
        body: &outgoing.body,
        serial_key: key.as_deref(),
        deadline_at: outgoing.deadline.ok_or_else(unavailable)?.get(),
    };
    authority.lock_native_source(&mut tx).await?;
    Box::pin(authority.current_issue_callers(
        &mut tx,
        &proposed,
        engine.inner.clock.as_ref(),
        engine.inner.access.as_ref(),
    ))
    .await?
    .ok_or_else(unavailable)?;
    let raw = serde_json::to_value(&outgoing)?;
    let stored: Option<Json> = sqlx::query_scalar("UPDATE summary_calls SET prepared_command=COALESCE(prepared_command,$2) WHERE id=$1 AND purpose='summary' AND cancel_reason IS NULL RETURNING prepared_command")
        .bind(outgoing.id).bind(&raw).fetch_optional(&mut *tx).await.map_err(DbError::from)?;
    if stored.as_ref() != Some(&raw) {
        return Err(unavailable());
    }
    tx.commit().await.map_err(DbError::from)?;
    Ok(outgoing)
}

pub(super) async fn prepare(
    host: &Host,
    outgoing: Outgoing,
    attribution: PaidAttribution,
) -> Result<Outgoing, EngineError> {
    let prepared = host
        .engine
        .prepare_paid_service_command(outgoing, attribution)
        .await?;
    journal(&host.engine, prepared).await
}

pub(super) async fn retire(conn: &mut Conn, id: Id) -> Result<(), EngineError> {
    sqlx::query("DELETE FROM summary_calls WHERE id=$1 AND purpose='summary'")
        .bind(id)
        .execute(conn)
        .await
        .map_err(DbError::from)?;
    Ok(())
}

pub(super) fn deadline(call: &Call) -> Time {
    Time::new(
        call.created_at
            + chrono::Duration::seconds(i64::from(
                orch_link_contract::commands::CommandKind::ProviderComplete.deadline_secs(),
            )),
    )
}

pub(super) async fn retire_unprepared(conn: &mut Conn, id: Id) -> Result<(), EngineError> {
    sqlx::query("DELETE FROM summary_calls s WHERE s.id=$1 AND s.purpose='summary' AND s.prepared_command IS NULL AND NOT EXISTS(SELECT 1 FROM commands c WHERE c.id=s.id)")
        .bind(id).execute(conn).await.map_err(DbError::from)?;
    Ok(())
}

pub(super) fn matches(
    outgoing: &Outgoing,
    node: Id,
    call: Id,
    work: Option<&super::super::resource::Work>,
) -> bool {
    let Some(authority) = outgoing.authority.as_ref() else {
        return false;
    };
    let native = authority.paid_service_native().unwrap_or(authority);
    matches!(native, CommandAuthority::ResourceSummary(source) if source.belongs_to(node, call, work))
}

fn unavailable() -> EngineError {
    EngineError::BeforeEffect(Box::new(crate::operation_scope::continuing::Error::Refused))
}
