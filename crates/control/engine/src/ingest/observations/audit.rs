//! Minimal runtime facts in the existing audit trail; conversation and diagnostic bodies stay in their items.
use super::*;
use orch_access_contract::audit::AuditAction;
use orch_db::audit::{self, Actor, Entry, Origin};

struct Observation {
    /// Only compared against the same already-owned native item, never used to correlate requests.
    meaning: Json,
    facts: Json,
}

fn observation(kind: &str, payload: &Json, turn: Option<Id>) -> Option<Observation> {
    if kind == "marker" && payload.get("marker").and_then(Json::as_str) == Some("harness_changed") {
        let data: orch_api_contract::threads::MarkerHarnessChanged =
            serde_json::from_value(payload.get("data")?.clone()).ok()?;
        if data.turn_id != turn {
            return None;
        }
        let mut facts = json!({"change":data.change,"subject":data.subject,"changed":data.changed,"diverged":data.diverged,"next_turn":data.next_turn,"reported_at":data.reported_at});
        if data.change == "hook" {
            // Only the normalized execution state is an audit fact; native output stays in no audit row.
            if let Some(state) = data
                .value
                .get("status")
                .and_then(Json::as_str)
                .filter(|state| {
                    matches!(
                        *state,
                        "running" | "completed" | "failed" | "stopped" | "blocked" | "unknown"
                    )
                })
            {
                facts["state"] = json!(state);
            }
        }
        return Some(Observation {
            meaning: json!({"change":data.change,"subject":data.subject,"value":data.value,"expected":data.expected,"diverged":data.diverged,"next_turn":data.next_turn}),
            facts,
        });
    }
    if kind == "marker" && payload.get("marker").and_then(Json::as_str) == Some("compacted") {
        let duration = payload
            .pointer("/data/duration_ms")
            .and_then(Json::as_i64)
            .filter(|duration| *duration >= 0);
        return Some(Observation {
            meaning: json!({"change":"context_compaction","duration_ms":duration}),
            facts: json!({"change":"context_compaction","duration_ms":duration}),
        });
    }
    if kind == "error" && payload.get("code").and_then(Json::as_str) == Some("model_rerouted") {
        let detail = payload.get("details")?;
        let declared = detail.get("turn_id")?.as_str()?.parse::<Id>().ok()?;
        if turn != Some(declared) {
            return None;
        }
        let from = detail.get("from_model").and_then(Json::as_str);
        let to = detail
            .get("to_model")?
            .as_str()
            .filter(|model| !model.trim().is_empty())?;
        return Some(Observation {
            meaning: json!({"change":"model","from_model":from,"to_model":to}),
            facts: json!({"change":"model","from_model":from,"to_model":to}),
        });
    }
    if kind == "error" && payload.get("code").and_then(Json::as_str) == Some("session_lost") {
        return Some(Observation {
            meaning: json!({"change":"session_lifecycle","state":"lost"}),
            facts: json!({"change":"session_lifecycle","state":"lost"}),
        });
    }
    if kind == "tool_call" {
        let detail = crate::approval_review::detail(payload)?;
        let outcome = detail.get("outcome")?.as_str().filter(|outcome| {
            !outcome.is_empty() && !matches!(*outcome, "inProgress" | "in_progress")
        })?;
        let model = detail.get("model").and_then(Json::as_str);
        let reported = detail.get("reported").and_then(Json::as_bool);
        let fact = json!({"change":"approval_review","outcome":outcome,"model":model,"model_reported":reported});
        return Some(Observation {
            meaning: fact.clone(),
            facts: fact,
        });
    }
    None
}

pub(super) async fn completed(
    unit: &mut Unit<'_>,
    row: &ItemRow,
    payload: &Json,
    previous: Option<&Json>,
) -> Result<(), EngineError> {
    let Some(observation) = observation(&row.kind, payload, row.turn_id) else {
        return Ok(());
    };
    if previous
        .and_then(|previous| self::observation(&row.kind, previous, row.turn_id))
        .is_some_and(|previous| previous.meaning == observation.meaning)
    {
        return Ok(());
    }
    // The opened profile supplies the program; a native sentence cannot rename its audit actor.
    let program:Option<String>=sqlx::query_scalar("SELECT COALESCE(NULLIF(a.descriptor->>'name',''),a.harness_kind) FROM segments s JOIN agents a ON a.id=s.agent_id WHERE s.id=$1 AND s.thread_id=$2")
        .bind(row.segment_id).bind(row.thread_id).fetch_optional(&mut *unit.conn).await.map_err(orch_db::DbError::from)?;
    let Some(program) = program else {
        return Ok(());
    };
    let action = orch_db::audit::action!(AuditAction::HarnessChanged);
    // An idle observation has no originating person; never borrow a newer reserved turn's author.
    let actor = row
        .turn_id
        .map_or(Actor::System, |_| Actor::Agent(row.thread_id));
    let mut entry = Entry::new(
        actor,
        action.as_str(),
        "thread",
        Some(row.thread_id),
        action.as_str(),
    );
    entry.node_id = Some(row.thread_id);
    entry.details = observation.facts;
    entry.details["program"] = json!(program);
    entry.details["thread_id"] = json!(row.thread_id);
    entry.details["segment_id"] = json!(row.segment_id);
    entry.details["turn_id"] = json!(row.turn_id);
    entry.details["item_id"] = json!(row.id);
    entry.details["stream"] = json!(row.stream);
    entry.details["native_ref"] = json!(row.native_ref);
    let now = unit.now();
    audit::append(unit.conn, &entry, &Origin::default(), now).await?;
    Ok(())
}
