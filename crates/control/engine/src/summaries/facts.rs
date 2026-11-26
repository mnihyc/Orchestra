//! Small immutable observations taken from the actual native output and canonical request ledger.
use crate::EngineError;
use orch_api_contract::summary_facts::{
    AutomaticSkipReason, SummaryAttemptFacts, SummaryAttemptOutcome, SummaryDiscardReason,
};
use orch_api_contract::{summaries::SummaryProducerView, summary_facts::GoalHelperCoverage};
use orch_api_contract::{summary_facts::SummaryResultFacts, usage::RequestTokens};
use orch_db::{Conn, DbError};
use orch_summarizer_contract::result::ConformanceFacts;
use orch_summarizer_contract::{
    descriptor::{SummarizerDescriptor, SummaryField},
    result::Conformed,
};
use orch_types::{Id, Json, Time};
use serde::{Deserialize, Serialize};

pub(super) struct ProducedFacts {
    pub attempt: Option<Id>,
    pub producer: Option<SummaryProducerView>,
    pub owner: Option<Id>,
    pub usage: Option<RequestTokens>,
    pub result: Option<SummaryResultFacts>,
    pub at: Time,
}

impl ProducedFacts {
    pub fn attempt(
        &self,
        outcome: SummaryAttemptOutcome,
        discard_reason: Option<SummaryDiscardReason>,
    ) -> SummaryAttemptFacts {
        SummaryAttemptFacts {
            usage_owner: None,
            outcome,
            discard_reason,
            producer: self.producer.clone(),
            captured_usage: self.usage.clone(),
            result_facts: self.result.clone(),
            at: self.at,
        }
    }
}

pub(super) async fn produced(
    conn: &mut Conn,
    chosen: &super::Choice,
    descriptor: &SummarizerDescriptor,
    root: Id,
    node: Id,
    conformed: Option<&Conformed>,
    now: Time,
) -> Result<Option<ProducedFacts>, EngineError> {
    let actual_id = crate::calls::read_root(
        conn,
        node,
        orch_provider_contract::calls::Purpose::Summary,
        root,
    )
    .await?
    .and_then(|row| {
        row.context["next_call"]
            .as_str()
            .and_then(|value| value.parse::<Id>().ok())
    })
    .unwrap_or(root);
    let actual = call(conn, actual_id).await?;
    if actual.is_none() && conformed.is_none() {
        return Ok(None);
    }
    let producer = if let Some(actual) = actual.as_ref() {
        let display =
            orch_db::queries::resources::thread_engine::model(conn, actual.provider, &actual.model)
                .await?
                .map(|model| model.display_name)
                .filter(|name| !name.is_empty())
                .unwrap_or_else(|| actual.model.clone());
        Some(SummaryProducerView {
            kind: "model".into(),
            title: display,
            provider: None,
            offering: Some(actual.model.clone()),
        })
    } else if chosen.kind != "model" {
        Some(SummaryProducerView {
            kind: chosen.kind.clone(),
            title: descriptor.title.clone(),
            provider: None,
            offering: None,
        })
    } else {
        None
    };
    let usage = actual.as_ref().and_then(|actual| actual.usage.clone());
    Ok(Some(ProducedFacts {
        attempt: actual.as_ref().map(|_| actual_id),
        producer,
        owner: actual.as_ref().and_then(|actual| actual.person),
        result: result(conformed.map(|value| &value.facts), usage.clone()),
        usage,
        at: actual.map_or(now, |actual| actual.at),
    }))
}

/// Metadata is committed under the caller's existing short node/thread lock order.
pub(super) async fn record_field(
    conn: &mut Conn,
    node: Id,
    field: SummaryField,
    attempt: &SummaryAttemptFacts,
    owner: Option<Id>,
    written_at: Option<Time>,
) -> Result<(), EngineError> {
    let field = match field {
        SummaryField::Title => "title",
        SummaryField::Tags => "tags",
        _ => return Ok(()),
    };
    let mut saved = serde_json::to_value(attempt)?;
    saved["usage_owner_id"] = serde_json::to_value(owner)?;
    sqlx::query("UPDATE node_summary_state SET automatic_fields=jsonb_set(COALESCE(automatic_fields,'{}'::jsonb),ARRAY[$2::text],
        COALESCE(automatic_fields->$2,'{}'::jsonb)||jsonb_build_object('last_attempt',$3::jsonb)||
        CASE WHEN $4::timestamptz IS NULL THEN '{}'::jsonb ELSE jsonb_build_object('last_success_at',$4::timestamptz) END,true) WHERE node_id=$1")
        .bind(node).bind(field).bind(saved).bind(written_at.map(|at| at.get())).execute(conn).await.map_err(DbError::from)?;
    Ok(())
}

/// Only the deciding operation may name a native skip cause; a text error is not parsed into one.
pub(crate) async fn skip(
    conn: &mut Conn,
    node: Id,
    run: Id,
    reason: AutomaticSkipReason,
) -> Result<(), EngineError> {
    let word = serde_json::to_value(reason)?;
    sqlx::query("UPDATE node_summary_state SET reason_code=$3 WHERE node_id=$1 AND run_id=$2")
        .bind(node)
        .bind(run)
        .bind(word.as_str())
        .execute(conn)
        .await
        .map_err(DbError::from)?;
    Ok(())
}

/// The source decision passed by the native goal checker, not by request JSON.
#[derive(Clone)]
pub(super) struct GoalCapture {
    pub report: Id,
    pub last_turn: Option<GoalHelperCoverage>,
}

/// Kept privately with a report; the usage owner is named through the current ordinary projection.
#[derive(Clone, Serialize, Deserialize)]
pub(crate) struct GoalRecord {
    #[serde(default)]
    pub checked: Option<bool>,
    pub attempt_id: Id,
    pub producer: Option<SummaryProducerView>,
    pub usage_owner: Option<Id>,
    pub input_allowance_tokens: Option<i64>,
    pub output_allowance_tokens: Option<i64>,
    pub last_turn: Option<GoalHelperCoverage>,
    pub usage: Option<RequestTokens>,
    pub captured_at: Time,
}

pub(super) struct GoalAdmission {
    pub node: Id,
    pub goal: Id,
    pub call: Id,
    pub owner: Id,
    pub model: String,
    pub model_display: String,
    pub input: Option<i64>,
    pub output: i64,
    pub already_issued: bool,
    pub at: Time,
}

/// Called last in the existing short admission transaction, after its source/provider locks.
pub(super) async fn capture_goal(
    conn: &mut Conn,
    selected: &GoalCapture,
    admitted: GoalAdmission,
) -> Result<(), EngineError> {
    let record = GoalRecord {
        checked: Some(false),
        attempt_id: admitted.call,
        producer: Some(SummaryProducerView {
            kind: "model".into(),
            title: admitted.model_display,
            provider: None,
            offering: Some(admitted.model),
        }),
        usage_owner: Some(admitted.owner),
        input_allowance_tokens: admitted.input,
        output_allowance_tokens: Some(admitted.output),
        // Replaying an older prepared command cannot fill its missing source facts from a new read.
        last_turn: if admitted.already_issued {
            None
        } else {
            selected.last_turn
        },
        usage: None,
        captured_at: admitted.at,
    };
    sqlx::query("UPDATE thread_goal_reports SET helper=CASE WHEN helper->>'attempt_id'=$4 THEN helper ELSE $5::jsonb || CASE WHEN helper ? 'last_turn' THEN jsonb_build_object('last_turn',helper->'last_turn') ELSE '{}'::jsonb END END
        WHERE id=$1 AND thread_id=$2 AND goal_id=$3 AND outcome IS NULL")
        .bind(selected.report).bind(admitted.node).bind(admitted.goal).bind(admitted.call.to_string())
        .bind(serde_json::to_value(record)?).execute(conn).await.map_err(DbError::from)?;
    Ok(())
}

/// The actual helper outcome is retained even when a later goal edit refuses its application.
/// No source/goal lock is taken after the report update; callers apply the goal in another transaction.
pub(crate) async fn finish_goal(
    conn: &mut Conn,
    report: Id,
    checked: bool,
) -> Result<(), EngineError> {
    let saved: Option<Json> =
        sqlx::query_scalar::<_, Option<Json>>("SELECT helper FROM thread_goal_reports WHERE id=$1")
            .bind(report)
            .fetch_optional(&mut *conn)
            .await
            .map_err(DbError::from)?
            .flatten();
    let Some(mut record) = saved
        .as_ref()
        .and_then(|value| serde_json::from_value::<GoalRecord>(value.clone()).ok())
    else {
        return Ok(());
    };
    record.checked = Some(checked);
    record.usage = call(conn, record.attempt_id)
        .await?
        .and_then(|actual| actual.usage);
    sqlx::query(
        "UPDATE thread_goal_reports SET helper=$2 WHERE id=$1 AND helper IS NOT DISTINCT FROM $3",
    )
    .bind(report)
    .bind(serde_json::to_value(record)?)
    .bind(saved)
    .execute(conn)
    .await
    .map_err(DbError::from)?;
    Ok(())
}

pub(crate) struct CallFacts {
    pub provider: Id,
    pub model: String,
    pub person: Option<Id>,
    pub usage: Option<RequestTokens>,
    pub at: Time,
}

#[derive(sqlx::FromRow)]
struct RecordedCall {
    provider_id: Id,
    model: String,
    user_id: Option<Id>,
    measurement_states: Json,
    at: chrono::DateTime<chrono::Utc>,
}

/// One primary-key lookup, never attribution guessed from a model, time or resource owner.
pub(crate) async fn call(conn: &mut Conn, id: Id) -> Result<Option<CallFacts>, EngineError> {
    let row: Option<RecordedCall> = sqlx::query_as(
        "SELECT provider_id,model,user_id,measurement_states,at FROM usage_events WHERE id=$1 AND purpose='summary'",
    ).bind(id).fetch_optional(&mut *conn).await.map_err(DbError::from)?;
    // A program's physical turn has its own ledger identity. Follow the accepted call's
    // original helper input for the model's final observation, including unreported usage.
    let row = match row {
        Some(row) => Some(row),
        None => sqlx::query_as::<_,RecordedCall>(
            "SELECT s.provider_id,t.model,w.person_id AS user_id,
                    jsonb_build_object('reported_tokens',t.usage) AS measurement_states,t.completed_at AS at
               FROM delegated_program_work w JOIN thread_inputs i ON i.id=w.initial_input_id AND i.thread_id=w.helper_id
               JOIN turns t ON t.id=i.turn_id AND t.input_id=i.id AND t.thread_id=i.thread_id
               JOIN segments s ON s.id=t.segment_id AND s.thread_id=t.thread_id
              WHERE w.source_summary_call_id=$1 AND w.attempt_id=$1 AND w.purpose='summary'
                AND s.credential_path='native' AND s.provider_id IS NOT NULL
                AND t.started_at IS NOT NULL AND t.completed_at IS NOT NULL")
            .bind(id).fetch_optional(&mut *conn).await.map_err(DbError::from)?,
    };
    Ok(row.map(|row| CallFacts {
        provider: row.provider_id,
        model: row.model,
        person: row.user_id,
        at: Time::new(row.at),
        usage: row
            .measurement_states
            .get("reported_tokens")
            .filter(|value| !value.is_null())
            .cloned()
            .and_then(|value| serde_json::from_value::<RequestTokens>(value).ok())
            .filter(|tokens| !tokens.is_empty()),
    }))
}

pub(super) fn result(
    conformance: Option<&ConformanceFacts>,
    usage: Option<RequestTokens>,
) -> Option<SummaryResultFacts> {
    if conformance.is_none() && usage.is_none() {
        return None;
    }
    Some(SummaryResultFacts {
        flattened: conformance.map(|facts| facts.flattened),
        first_line_only: conformance.map(|facts| facts.first_line_only),
        removed_characters: conformance.map(|facts| facts.removed_characters),
        unknown_tags: conformance.map(|facts| facts.unknown_tags),
        duplicate_tags: conformance.map(|facts| facts.duplicate_tags),
        over_budget_tags: conformance.map(|facts| facts.over_budget_tags),
        rejected_whole: conformance.map(|facts| facts.rejected_whole),
        captured_usage: usage,
    })
}
