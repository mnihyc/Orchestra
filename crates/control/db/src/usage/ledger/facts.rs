//! Reconcile two observations only when an engine-verified lease proves their upstream identity.
use super::*;
use orch_api_contract::usage::{RequestTiming, UsageMeasurement};

#[derive(Default)]
pub(super) struct Merged {
    pub handled: bool,
    pub adapter_ref: Option<String>,
    pub tool_refs: Option<Json>,
}

fn combine(primary: &RequestTiming, secondary: &RequestTiming) -> RequestTiming {
    let mut result = primary.clone();
    result.error_reason = result
        .error_reason
        .or_else(|| secondary.error_reason.clone());
    result.upstream_status = result.upstream_status.or(secondary.upstream_status);
    result.provider_request_id = result
        .provider_request_id
        .or_else(|| secondary.provider_request_id.clone());
    result.attempt = primary.attempt.into_iter().chain(secondary.attempt).max();
    result.asked = result.asked.or(secondary.asked);
    result.first_output = result.first_output.or(secondary.first_output);
    result.end = result.end.or(secondary.end);
    result.reported_tokens = result
        .reported_tokens
        .or_else(|| secondary.reported_tokens.clone());
    result.vendor_words = result
        .vendor_words
        .or_else(|| secondary.vendor_words.clone());
    result.error_message = result
        .error_message
        .or_else(|| secondary.error_message.clone());
    for (held, other) in [
        (&mut result.first_token_ms, &secondary.first_token_ms),
        (&mut result.request_ms, &secondary.request_ms),
        (&mut result.generation_ms, &secondary.generation_ms),
        (&mut result.speed_milli, &secondary.speed_milli),
        (&mut result.streamed_tokens, &secondary.streamed_tokens),
        (&mut result.web_searches, &secondary.web_searches),
    ] {
        if matches!(
            held,
            UsageMeasurement::Unsupported | UsageMeasurement::Unread
        ) {
            *held = other.clone();
        }
    }
    if !matches!(result.outcome, UsageMeasurement::Told { .. }) {
        result.outcome = secondary.outcome.clone();
    }
    result
}

pub(super) async fn reconcile(
    conn: &mut Conn,
    event: &mut NewEvent<'_>,
    timing: &mut RequestTiming,
    lease: Option<Id>,
) -> Result<Merged, DbError> {
    let adapter_ref = (event.origin == origin::HARNESS)
        .then(|| event.request_ref.map(str::to_owned))
        .flatten();
    let Some(lease) = lease else {
        return Ok(Merged {
            adapter_ref,
            ..Default::default()
        });
    };
    let Some(identity) = timing
        .provider_request_id
        .as_deref()
        .filter(|id| !id.is_empty())
    else {
        return Ok(Merged {
            adapter_ref,
            ..Default::default()
        });
    };
    // Adapters and the forwarder can publish concurrently. No timing/token/model similarity is identity.
    sqlx::query("SELECT pg_advisory_xact_lock(hashtextextended($1,0))")
        .bind(format!("usage_request:{lease}:{identity}"))
        .execute(&mut *conn)
        .await?;
    let sql = format!(
        "SELECT false AS running,{EVENT_COLUMNS} FROM usage_events WHERE turn_id=$1 AND provider_id=$2 AND measurement_states->>'_lease_id'=$3 AND provider_request_id=$4 AND scope='request' ORDER BY counted DESC,id LIMIT 1 FOR UPDATE"
    );
    let existing: Option<EventRow> = sqlx::query_as(sqlx::AssertSqlSafe(sql))
        .bind(event.attribution.turn_id)
        .bind(event.attribution.provider_id)
        .bind(lease.to_string())
        .bind(identity)
        .fetch_optional(&mut *conn)
        .await?;
    let Some(existing) = existing else {
        let prior: Option<(Id,Option<Json>,Option<String>)> = sqlx::query_as("SELECT id,measurement_states->'_tool_refs',measurement_states->>'_adapter_request_ref' FROM usage_running WHERE turn_id=$1 AND provider_id=$2 AND measurement_states->>'_lease_id'=$3 AND provider_request_id=$4 ORDER BY id LIMIT 1 FOR UPDATE")
            .bind(event.attribution.turn_id).bind(event.attribution.provider_id).bind(lease.to_string()).bind(identity)
            .fetch_optional(&mut *conn).await?;
        let mut retained_adapter = adapter_ref;
        let tool_refs = prior.and_then(|(id, refs, adapter)| {
            event.id = id;
            retained_adapter = adapter.or(retained_adapter.take());
            refs
        });
        return Ok(Merged {
            adapter_ref: retained_adapter,
            tool_refs,
            ..Default::default()
        });
    };
    if existing.origin == event.origin {
        return Ok(Merged {
            handled: true,
            adapter_ref,
            ..Default::default()
        });
    }
    if event.origin == origin::FORWARDER && !existing.counted {
        *timing = combine(timing, &existing.timing());
        event.id = existing.id;
        rollup::unfold_event(conn, existing.id).await?;
        sqlx::query("DELETE FROM usage_events WHERE id=$1 AND NOT counted")
            .bind(existing.id)
            .execute(&mut *conn)
            .await?;
        return Ok(Merged {
            handled: false,
            adapter_ref: existing.request_ref,
            tool_refs: existing.measurement_states.get("_tool_refs").cloned(),
        });
    }
    if event.origin != origin::HARNESS || existing.origin != origin::FORWARDER {
        return Ok(Merged {
            adapter_ref,
            ..Default::default()
        });
    }
    // The counted source owns billing and upstream classification; the harness supplies missing timings.
    let joined = combine(&existing.timing(), timing);
    let mut states = existing.measurement_states.clone();
    if let Some(states) = states.as_object_mut() {
        if let Some(joined) = serde_json::to_value(&joined)
            .ok()
            .and_then(|value| value.as_object().cloned())
        {
            states.extend(joined);
        }
        if let Some(reference) = &adapter_ref {
            states.insert(
                "_adapter_request_ref".into(),
                Json::String(reference.clone()),
            );
        }
    }
    if states != existing.measurement_states {
        rollup::unfold_event(conn, existing.id).await?;
        sqlx::query("UPDATE usage_events SET measurement_states=$2,error_reason=$3,upstream_status=$4,attempt=$5,first_token_ms=$6,generation_ms=$7,request_ms=$8,streamed_output_tokens=$9,speed_milli=$10 WHERE id=$1")
            .bind(existing.id).bind(states).bind(joined.error_reason.as_deref()).bind(joined.upstream_status)
            .bind(joined.attempt.unwrap_or(1)).bind(joined.first_token_ms.value().copied())
            .bind(joined.generation_ms.value().copied()).bind(joined.request_ms.value().copied())
            .bind(joined.streamed_tokens.value().copied()).bind(joined.speed_milli.value().copied())
            .execute(&mut *conn).await?;
        rollup::fold_event(conn, existing.id).await?;
    }
    sqlx::query("DELETE FROM usage_running WHERE id=$1")
        .bind(event.id)
        .execute(&mut *conn)
        .await?;
    Ok(Merged {
        handled: true,
        adapter_ref,
        ..Default::default()
    })
}
