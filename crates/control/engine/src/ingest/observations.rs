//! Native observations enrich their exact originating turn without changing the person's next-turn choices.
use super::*;
mod audit;

pub(super) async fn rerouted(
    unit: &mut Unit<'_>,
    thread: Id,
    segment: Id,
    turn: Option<Id>,
    payload: &Json,
) -> Result<Option<Json>, EngineError> {
    if payload.get("code").and_then(Json::as_str) != Some("model_rerouted") {
        return Ok(None);
    }
    let Some(details) = payload.get("details") else {
        return Ok(None);
    };
    let Some(to) = details
        .get("to_model")
        .and_then(Json::as_str)
        .filter(|model| !model.trim().is_empty())
    else {
        return Ok(None);
    };
    let Some(turn) = turn.filter(|turn| {
        details
            .get("turn_id")
            .and_then(Json::as_str)
            .and_then(|id| id.parse::<Id>().ok())
            == Some(*turn)
    }) else {
        return Ok(None);
    };
    let Some(row) = threads::turn(unit.conn, turn)
        .await?
        .filter(|row| row.thread_id == thread && row.segment_id == segment)
    else {
        return Ok(None);
    };
    let from = details
        .get("from_model")
        .and_then(Json::as_str)
        .filter(|model| !model.is_empty())
        .unwrap_or(&row.model);
    let names:Vec<(String,String)>=sqlx::query_as("SELECT model_name,display_name FROM provider_models WHERE provider_id=(SELECT provider_id FROM segments WHERE id=$1) AND model_name=ANY($2)")
        .bind(segment).bind(vec![from.to_owned(),to.to_owned()]).fetch_all(&mut *unit.conn).await.map_err(orch_db::DbError::from)?;
    let name = |model: &str| {
        names
            .iter()
            .find(|(key, label)| key == model && !label.is_empty())
            .map(|(_, label)| label.clone())
            .unwrap_or_else(|| model.to_owned())
    };
    let mut enriched = payload.clone();
    enriched["details"]["from_display"] = json!(name(from));
    enriched["details"]["to_display"] = json!(name(to));
    let mut detail: orch_api_contract::context::TurnUsageDetail =
        serde_json::from_value(row.usage_detail.clone().unwrap_or_default()).unwrap_or_default();
    let changed = detail.effective_model.as_deref() != Some(to);
    detail.effective_model = Some(to.to_owned());
    detail.effective_model_display = Some(name(to));
    if changed {
        detail.context_window = None;
        detail.context_used = None;
        detail.compact_at = None;
        let seq = unit.stamp(thread).await?;
        sqlx::query("UPDATE turns SET usage_detail=$2,context=NULL,seq_updated=$3 WHERE id=$1")
            .bind(turn)
            .bind(crate::context_fill::with_uncarried(
                serde_json::to_value(detail)?,
                row.usage_detail
                    .as_ref()
                    .and_then(|kept| kept.get(crate::context_fill::UNCARRIED)),
            ))
            .bind(seq)
            .execute(&mut *unit.conn)
            .await
            .map_err(orch_db::DbError::from)?;
        if orch_db::queries::thread_history::selected_turn(unit.conn, thread, turn).await? {
            let current = threads::lock(unit.conn, thread).await?;
            if let Some(mut usage) = current
                .and_then(|thread| thread.usage)
                .filter(Json::is_object)
            {
                usage["context_model"] = json!(to);
                usage["context_model_display"] = json!(name(to));
                // Keep token consumption, but await a window actually observed for this model.
                usage["context_window"] = Json::Null;
                usage["context_used"] = Json::Null;
                usage["compact_at"] = Json::Null;
                threads::set_usage(unit.conn, thread, &usage, seq).await?;
            }
        }
    }
    Ok(Some(enriched))
}

/// A main conversation compacted even when its phase notification was absent.
fn context_compaction(payload: &Json) -> bool {
    payload.get("marker").and_then(Json::as_str) == Some("compacted")
        || (payload.get("marker").and_then(Json::as_str) == Some("harness_changed")
            && payload.pointer("/data/change").and_then(Json::as_str) == Some("context_compaction"))
}

pub(super) async fn completed_marker(
    unit: &mut Unit<'_>,
    segment: Id,
    stream: &str,
    payload: &Json,
) -> Result<(), EngineError> {
    if stream == "main" && context_compaction(payload) {
        threads::set_context_sent(unit.conn, segment, &json!({"resend":true})).await?;
    }
    Ok(())
}

/// One committed native item supplies both context repair and the factual audit.
pub(super) async fn completed_item(
    unit: &mut Unit<'_>,
    row: &ItemRow,
    payload: &Json,
    previous: Option<&Json>,
) -> Result<(), EngineError> {
    if row.kind == "marker"
        && (row.status == "in_progress"
            || previous.is_none_or(|previous| !context_compaction(previous)))
    {
        completed_marker(unit, row.segment_id, &row.stream, payload).await?;
    }
    audit::completed(unit, row, payload, previous).await
}

/// Every committed native observation uses the same audit producer, including a live or failed marker.
pub(super) async fn observed_item(
    unit: &mut Unit<'_>,
    row: &ItemRow,
    payload: &Json,
    previous: Option<&Json>,
) -> Result<(), EngineError> {
    audit::completed(unit, row, payload, previous).await
}
