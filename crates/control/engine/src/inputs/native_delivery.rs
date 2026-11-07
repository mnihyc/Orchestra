//! Current input authority at the first native publication of its actual retained command.
//! Native Accepted/result replay is handled by the dispatcher before this new-work verifier.

use super::{InputError, check_sender_for_turn, input_author_caller, input_caller};
use crate::{EngineError, access::ResourceAccess, unit::Unit};
use orch_authz::Caller;
use orch_db::{Conn, queries::link::CommandRow, queries::threads};
use orch_link_contract::{
    commands::{SerialKey, TurnStart, TurnSteer},
    continuations::TurnStartFenced,
};
use orch_types::{Clock, Id, Time};

/// The existing command body identifies the thread to fence before locking the command row.
/// It is a factual lock operand, not an admission or a new source of caller authority.
pub fn thread_of(command: &CommandRow) -> Result<Option<Id>, EngineError> {
    Ok(match command.kind.as_str() {
        "turn.start" => Some(serde_json::from_value::<TurnStart>(command.body.clone())?.thread_id),
        "turn.start.fenced" => {
            Some(serde_json::from_value::<TurnStartFenced>(command.body.clone())?.thread_id)
        }
        "turn.steer" => Some(serde_json::from_value::<TurnSteer>(command.body.clone())?.thread_id),
        _ => None,
    })
}

struct Delivery {
    thread: Id,
    turn: Id,
    input: Id,
    segment: Option<Id>,
    native_action: Option<orch_types::Json>,
    start: Option<TurnStart>,
}

/// Unrelated maintenance has no input authority to replace. A new Steer supplies
/// only its immutable input reference through the existing private command channel.
async fn delivery(
    conn: &mut Conn,
    command: &CommandRow,
    steer_input: Option<Id>,
) -> Result<Option<Delivery>, EngineError> {
    let (thread, turn, segment, native_action, start, input) = match command.kind.as_str() {
        "turn.start" | "turn.start.fenced" => {
            let (body, segment) = if command.kind == "turn.start.fenced" {
                let fenced: TurnStartFenced = serde_json::from_value(command.body.clone())?;
                if fenced.thread_id != fenced.turn.thread_id {
                    return Err(EngineError::unreadable(
                        "a fenced input command names another thread",
                    ));
                }
                (*fenced.turn, Some(fenced.segment_id))
            } else {
                (
                    serde_json::from_value::<TurnStart>(command.body.clone())?,
                    None,
                )
            };
            let Some(turn) = threads::turn(conn, body.turn_id).await? else {
                // Standalone ordinary native commands never acquire an input's rights.
                return Ok(None);
            };
            if !(0..crate::turns::SESSION_ATTEMPTS)
                .any(|attempt| crate::ids::turn_start(turn.id, attempt) == command.id)
            {
                return Ok(None);
            }
            let native = body
                .native_action
                .as_ref()
                .map(serde_json::to_value)
                .transpose()?;
            (
                body.thread_id,
                body.turn_id,
                segment,
                native,
                Some(body),
                turn.input_id,
            )
        }
        "turn.steer" => {
            let Some(input) = steer_input else {
                // Canonical pre-upgrade None is old work. Never adopt a current input
                // into that row or cancel unrelated legacy/system queued commands.
                return Ok(None);
            };
            let body: TurnSteer = serde_json::from_value(command.body.clone())?;
            let native = body
                .native_action
                .as_ref()
                .map(serde_json::to_value)
                .transpose()?;
            (body.thread_id, body.turn_id, None, native, None, input)
        }
        _ => return Ok(None),
    };
    Ok(Some(Delivery {
        thread,
        turn,
        input,
        segment,
        native_action,
        start,
    }))
}

fn ordinary(result: Result<(), InputError>) -> Result<bool, EngineError> {
    match result {
        Ok(()) => Ok(true),
        Err(InputError::Internal(EngineError::Authz(
            orch_authz::Error::NoSuchUser(_)
            | orch_authz::Error::NotFound(_)
            | orch_authz::Error::Suspended { .. }
            | orch_authz::Error::Archived { .. },
        ))) => Ok(false),
        Err(InputError::Internal(error)) => Err(error),
        Err(_) => Ok(false),
    }
}

/// Reuse every real accepted author/actor operation and the ordinary input checks.
/// The caller holds the existing thread fence before the canonical command row lock;
/// these DB-only checks retain all resulting callers to the final gateway queue boundary.
pub async fn current_callers(
    conn: &mut Conn,
    command: &CommandRow,
    steer_input: Option<Id>,
    clock: &dyn Clock,
    access: &dyn ResourceAccess,
) -> Result<Option<Vec<(Option<Caller>, Option<Time>)>>, EngineError> {
    if steer_input.is_some() && command.kind != "turn.steer" {
        // Present typed evidence must never fall through to unrelated maintenance.
        return Ok(None);
    }
    let Some(delivery) = delivery(conn, command, steer_input).await? else {
        return Ok(Some(Vec::new()));
    };
    let Some(thread) = threads::thread(conn, delivery.thread).await? else {
        return Ok(None);
    };
    let Some(turn) = threads::turn(conn, delivery.turn).await? else {
        return Ok(None);
    };
    let Some(input) = threads::input(conn, delivery.input).await? else {
        return Ok(None);
    };
    let Some(segment) = threads::segment(conn, turn.segment_id).await? else {
        return Ok(None);
    };
    if turn.thread_id != thread.node_id
        || input.thread_id != thread.node_id
        || segment.thread_id != thread.node_id
        || thread.current_turn_id != Some(turn.id)
        || thread.current_segment_id != Some(segment.id)
        || thread.active_continuation_id != input.continuation_id
        || segment.continuation_id != input.continuation_id
        || thread.deleted_at.is_some()
        || thread.archived_at.is_some()
        || segment.ended_at.is_some()
        || turn.interrupted_by.is_some()
        || input.state != threads::input_state::DELIVERING
        || input.turn_id != Some(turn.id)
        || delivery.segment.is_some_and(|id| id != segment.id)
        || command.serial_key() != SerialKey::Thread(thread.node_id).wire().as_deref()
        || delivery.native_action != input.native_action
    {
        return Ok(None);
    }
    if let Some(start) = &delivery.start {
        if turn.state != threads::turn_state::STARTING
            || turn.input_id != input.id
            || start.model != turn.model
            || start.effort != turn.effort
            || start.plan != turn.plan
        {
            return Ok(None);
        }
    } else if turn.state != threads::turn_state::RUNNING
        || input.delivery != "steer"
        || input.state_detail.as_deref() != Some("steering")
        || crate::ids::turn_steer(input.id) != command.id
    {
        return Ok(None);
    }
    let Some(provider) =
        orch_db::queries::resources::thread_engine::provider(conn, segment.provider_id).await?
    else {
        return Ok(None);
    };
    let Some(environment) =
        orch_db::queries::environments::thread_engine::environment(conn, segment.environment_id)
            .await?
    else {
        return Ok(None);
    };
    if thread.provider_id != Some(segment.provider_id)
        || thread.agent_id != Some(segment.agent_id)
        || thread.environment_id != Some(segment.environment_id)
        || environment.generation != segment.env_generation
        || crate::harnesses::machine_of(
            Some(&segment.arrangement),
            &provider,
            environment.machine_id,
        ) != Some(command.machine_id)
    {
        return Ok(None);
    }
    // The input association is frozen while delivering/steering; the canonical row's
    // immutable id/key/body was independently checked by the dispatcher. No input edit
    // can lend the retained native command a later author's or model approver's origin.
    let known_author = super::saved_authority(input.submission_authority.as_ref())?
        .is_some_and(|saved| saved.has_actor_origins());
    let known_actor = if input.run_as.is_some() {
        input.run_as_authority.is_some()
    } else {
        known_author
    };
    if !known_author && !known_actor {
        // Pre-provenance/imported or authorless system work retains its old lifetime.
        // Absence is not converted to a Session/operation, and cannot hide a separately
        // present execution approver's original credential boundary.
        return Ok(Some(Vec::new()));
    }
    let reference = if delivery.start.is_some() {
        turn.id
    } else {
        input.id
    };
    let associated: bool = sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM model_inputs WHERE node_id=$1 AND segment_id=$2
         AND turn_id=$3 AND input_id=$4 AND purpose='input' AND reference=$5)",
    )
    .bind(thread.node_id)
    .bind(segment.id)
    .bind(turn.id)
    .bind(input.id)
    .bind(reference.to_string())
    .fetch_one(&mut *conn)
    .await
    .map_err(orch_db::DbError::from)?;
    if !associated {
        return Ok(None);
    }
    let mut unit = Unit::new(conn, clock.now());
    let mut callers = Vec::new();
    let author = if known_author {
        let Some(author) = input_author_caller(&mut unit, thread.node_id, &input, clock).await?
        else {
            return Ok(None);
        };
        for caller in author.callers() {
            if !ordinary(
                check_sender_for_turn(&mut unit, caller, &thread, false, Some(&turn), access).await,
            )? {
                return Ok(None);
            }
            callers.push((Some(caller.clone()), None));
        }
        if let Some(person) = input.author_id
            && crate::preflight::second_step_missing(unit.conn, person, thread.node_id).await?
        {
            return Ok(None);
        }
        Some(author)
    } else {
        None
    };
    if known_actor {
        let Some(actor) = input_caller(&mut unit, &thread, &input, clock).await? else {
            return Ok(None);
        };
        for caller in actor.callers() {
            if !ordinary(
                check_sender_for_turn(&mut unit, caller, &thread, true, Some(&turn), access).await,
            )? || crate::preflight::second_step_missing(unit.conn, caller.user(), thread.node_id)
                .await?
            {
                return Ok(None);
            }
            callers.push((Some(caller.clone()), None));
        }
    }
    if let (Some(raw), Some(author)) = (&input.native_action, &author) {
        let selected = serde_json::from_value(raw.clone())?;
        let delivery = match input.delivery.as_str() {
            "steer" => orch_api_contract::threads::Delivery::Steer,
            "interrupt" => orch_api_contract::threads::Delivery::Interrupt,
            _ => orch_api_contract::threads::Delivery::Queue,
        };
        match crate::native_actions::validate_for_input(
            &mut unit,
            author,
            thread.node_id,
            &selected,
            delivery,
        )
        .await
        {
            Ok(_) => {}
            Err(crate::native_actions::NativeError::Internal(error)) => return Err(error),
            Err(_) => return Ok(None),
        }
    }
    if callers.iter().any(|(caller, _)| {
        caller
            .as_ref()
            .is_some_and(|caller| !caller.key_deadline_allows(clock))
    }) {
        return Ok(None);
    }
    Ok(Some(callers))
}
