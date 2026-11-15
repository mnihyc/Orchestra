//! Preflight publishes a checked queue observation; a reader can retire it, never create it.

use crate::engine::Engine;
use crate::error::EngineError;
use crate::preflight::Outcome;
use crate::unit::Unit;
use crate::views::People;
use orch_api_contract::threads::QueueEligibility;
use orch_db::queries::threads::{self, ThreadRow, eligibility as rows};
use orch_types::{Id, Time};
use serde_json::json;

#[derive(Debug, Default)]
pub(crate) enum Choice {
    #[default]
    Unreported,
    None,
    Selected {
        input: Id,
        author: Option<Id>,
        authority: Option<Id>,
    },
}

impl Engine {
    pub(crate) async fn keep_queue_eligibility(
        &self,
        unit: &mut Unit<'_>,
        thread: Id,
        before: Option<String>,
        choice: Choice,
        outcome: &Outcome,
    ) -> Result<(), EngineError> {
        let now = unit.now();
        let Some(row) = threads::thread(unit.conn, thread).await? else {
            return Ok(());
        };
        let old = rows::observation(unit.conn, thread).await?;
        if matches!(choice, Choice::Unreported) && old.is_none() {
            return Ok(());
        }
        let after = if matches!(choice, Choice::Unreported) {
            None
        } else {
            rows::basis(unit.conn, thread, now).await?
        };
        let valid = before.is_some()
            && before == after
            && !matches!(
                outcome,
                Outcome::Started(_) | Outcome::Busy | Outcome::Gone | Outcome::Dormant
            );
        let mut result = if valid {
            match choice {
                Choice::None => json!({"state":"none"}),
                Choice::Selected {
                    input,
                    author,
                    authority,
                } => json!({
                    "state":"selected","input_ref":input,"author_id":author,"authority_id":authority,
                    "blocking_reason":match outcome { Outcome::Blocked(why) => Some(why.reason), _ => None },
                }),
                Choice::Unreported => json!({"state":"unreported"}),
            }
        } else {
            json!({"state":"unreported"})
        };
        result["queue_revision"] = json!(row.queue_revision);
        let basis = after.unwrap_or_default();
        if let Some(old) = &old {
            let mut previous = old.result.clone();
            if let Some(object) = previous.as_object_mut() {
                object.remove("evaluated_at");
            }
            if old.basis == basis
                && old.queue_revision == row.queue_revision
                && old.state_seq == row.seq_state
                && previous == result
            {
                return Ok(());
            }
        }
        if result["state"] != "unreported" {
            result["evaluated_at"] = json!(Time::new(now));
        }
        let seq = unit.stamp(thread).await?;
        threads::set_seq_state(unit.conn, thread, seq).await?;
        rows::keep(
            unit.conn,
            thread,
            &rows::Observation {
                basis,
                queue_revision: row.queue_revision,
                state_seq: seq,
                result,
            },
        )
        .await?;
        Ok(())
    }
}

pub(crate) async fn view(
    conn: &mut orch_db::Conn,
    row: &ThreadRow,
    now: Time,
) -> Result<QueueEligibility, EngineError> {
    let unknown = QueueEligibility::Unreported;
    let Some(observed) = rows::observation(conn, row.node_id).await? else {
        return Ok(unknown);
    };
    // The common running/changed-queue path never evaluates the wider metadata guard.
    if observed.result["state"] == "unreported"
        || observed.queue_revision != row.queue_revision
        || observed.state_seq != row.seq_state
    {
        return Ok(unknown);
    }
    if rows::basis(conn, row.node_id, now.get()).await?.as_ref() != Some(&observed.basis) {
        return Ok(unknown);
    }
    let mut result = observed.result;
    if result["state"] == "selected" {
        let author: Option<Id> = serde_json::from_value(result["author_id"].clone())?;
        let authority: Option<Id> = serde_json::from_value(result["authority_id"].clone())?;
        let people = People::load(conn, [author, authority].into_iter().flatten()).await?;
        result["safe_author"] = people.named_or_null(author);
        result["safe_authority"] = people.named_or_null(authority);
    }
    serde_json::from_value(result)
        .map_err(|error| EngineError::unreadable(format!("a checked queue observation: {error}")))
}
