//! A scheduled new thread's file preparation retains its actual accepted input authority.
//! The input row is the single immutable origin bundle; a file job stores only its identity.

use super::{ScheduledInputDestination, ScheduledInputOrigin, saved_authority};
use crate::EngineError;
use crate::operation_scope::{Prepared, compatible_selection, facets, targets};
use orch_access_contract::decisions::{Decision, ObjectRef};
use orch_access_contract::key_scope::{KeyAction, KeyIdentityKind, KeyObjectRef, KeyTargetKind};
use orch_access_contract::permissions::{ObjectKind, Permission};
use orch_api_contract::operations::{FacetUsage, TargetResolver};
use orch_authz::{CapturedOperation, Policy, Principal};
use orch_db::{Conn, queries::input_schedules, queries::threads};
use orch_types::{Clock, Id, Time};

const SCHEDULE: KeyTargetKind = KeyTargetKind::Identity(KeyIdentityKind::Schedule);
const THREAD: KeyTargetKind = KeyTargetKind::Object(ObjectKind::Thread);
const ENVIRONMENT: KeyTargetKind = KeyTargetKind::Object(ObjectKind::Environment);

fn invalid() -> EngineError {
    EngineError::unreadable("the scheduled source has no matching accepted occurrence")
}

/// Attach the real accepted input before the due transaction commits. A validation-only
/// trial has no accepted input/run and cannot manufacture this reference.
pub async fn retain_scheduled_source_origin(
    conn: &mut Conn,
    actor: Id,
    thread: Id,
    input: Id,
    reference: ScheduledInputOrigin,
) -> Result<(), EngineError> {
    if reference.destination != ScheduledInputDestination::NewThread {
        return Err(invalid());
    }
    let input_row = threads::input(conn, input).await?.ok_or_else(invalid)?;
    let authority = saved_authority(input_row.submission_authority.as_ref())?
        .filter(|authority| authority.scheduled() == Some(reference))
        .ok_or_else(invalid)?;
    if input_row.thread_id != thread
        || input_row.author_id != Some(actor)
        || authority.operation_origins().is_empty()
        || authority
            .operation_origins()
            .iter()
            .any(|origin| origin.user() != actor)
    {
        return Err(invalid());
    }
    let schedule = input_schedules::schedule(conn, reference.schedule_id, false)
        .await?
        .filter(|row| row.author_id == actor)
        .ok_or_else(invalid)?;
    let run = input_schedules::run_by_id(conn, reference.run_id)
        .await?
        .filter(|run| {
            run.schedule_id == schedule.id
                && run.thread_id.is_none_or(|actual| actual == thread)
                && run.input_id.is_none_or(|actual| actual == input)
        })
        .ok_or_else(invalid)?;
    if thread
        != crate::ids::derived(
            "input-schedule.thread",
            &[schedule.id.as_bytes(), run.occurrence.as_bytes()],
        )
    {
        return Err(invalid());
    }
    let row = threads::lock(conn, thread).await?.ok_or_else(invalid)?;
    if row.topic_id != schedule.topic_id {
        return Err(invalid());
    }
    let mut source = row.source.ok_or_else(invalid)?;
    if source.get("source").is_none()
        || source
            .get("scheduled_input")
            .is_some_and(|known| known != &serde_json::json!(input))
        || source
            .get("native_plan")
            .filter(|plan| !plan.is_null())
            .is_some_and(|plan| plan.get("scheduled_input") != Some(&serde_json::json!(input)))
    {
        return Err(invalid());
    }
    let environment = row.environment_id.ok_or_else(invalid)?;
    let generation: i32 =
        sqlx::query_scalar("SELECT generation FROM environments WHERE id=$1 FOR SHARE")
            .bind(environment)
            .fetch_one(&mut *conn)
            .await
            .map_err(orch_db::DbError::from)?;
    if source
        .get("scheduled_environment_generation")
        .is_some_and(|known| known.as_i64() != Some(i64::from(generation)))
    {
        return Err(invalid());
    }
    source["scheduled_input"] = serde_json::json!(input);
    source["scheduled_environment_generation"] = serde_json::json!(generation);
    // This is private provenance in the existing creation transaction, not a visible source change.
    sqlx::query("UPDATE threads SET source=$2 WHERE node_id=$1")
        .bind(thread)
        .bind(source)
        .execute(conn)
        .await
        .map_err(orch_db::DbError::from)?;
    Ok(())
}

/// Reauthorize all accepted definition origins for this residual file effect. State controller
/// actions and later schedule edits do not replace the accepted input's immutable bundle.
#[allow(
    clippy::too_many_arguments,
    reason = "an actual source effect keeps its accepted input, current target and recognition ceiling separate"
)]
pub async fn scheduled_source_authorized(
    conn: &mut Conn,
    input: Id,
    thread: Id,
    environment: Id,
    worktree: bool,
    now: Time,
    actor: Id,
    recognition: Option<&CapturedOperation>,
) -> Result<bool, EngineError> {
    let Some(input_row) = threads::input(conn, input).await? else {
        return Ok(false);
    };
    if input_row.thread_id != thread || input_row.author_id != Some(actor) {
        return Ok(false);
    }
    let Some(authority) = saved_authority(input_row.submission_authority.as_ref())? else {
        return Ok(false);
    };
    let Some(reference) = authority
        .scheduled()
        .filter(|reference| reference.destination == ScheduledInputDestination::NewThread)
    else {
        return Ok(false);
    };
    let Some(schedule) = input_schedules::schedule(conn, reference.schedule_id, false)
        .await?
        .filter(|row| row.author_id == actor)
    else {
        return Ok(false);
    };
    let Some(run) = input_schedules::run_by_id(conn, reference.run_id)
        .await?
        .filter(|run| {
            run.schedule_id == schedule.id
                && run.thread_id == Some(thread)
                && run.input_id == Some(input)
        })
    else {
        return Ok(false);
    };
    if thread
        != crate::ids::derived(
            "input-schedule.thread",
            &[schedule.id.as_bytes(), run.occurrence.as_bytes()],
        )
    {
        return Ok(false);
    }
    let Some(row) = threads::thread(conn, thread)
        .await?
        .filter(|row| row.topic_id == schedule.topic_id && row.environment_id == Some(environment))
    else {
        return Ok(false);
    };
    let Some(source) = row.source.as_ref() else {
        return Ok(false);
    };
    if source.get("prepare").and_then(serde_json::Value::as_bool) != Some(true)
        || !matches!(
            source
                .pointer("/info/state")
                .and_then(serde_json::Value::as_str),
            Some("pending" | "preparing")
        )
        || source
            .get("worktree")
            .is_some_and(|choice| !choice.is_null())
            != worktree
        || source.get("scheduled_input") != Some(&serde_json::json!(input))
    {
        return Ok(false);
    }
    let accepted_generation = source
        .get("scheduled_environment_generation")
        .and_then(serde_json::Value::as_i64);
    let generation: Option<i32> =
        sqlx::query_scalar("SELECT generation FROM environments WHERE id=$1")
            .bind(environment)
            .fetch_optional(&mut *conn)
            .await
            .map_err(orch_db::DbError::from)?;
    if accepted_generation.is_none() || accepted_generation != generation.map(i64::from) {
        return Ok(false);
    }
    let Some(restored) = authority.restore(conn, actor, now).await? else {
        return Ok(false);
    };
    if restored.origins.is_empty()
        || restored.origins.iter().any(|origin| {
            !matches!(
                origin.operation.as_deref(),
                Some("TopicInputScheduleCreate" | "InputScheduleUpdate.definition")
            )
        })
    {
        return Ok(false);
    }
    let mut callers = restored
        .origins
        .into_iter()
        .map(|origin| (origin.operation.unwrap_or_default(), origin.caller))
        .collect::<Vec<_>>();
    if let Some(recognition) = recognition {
        if recognition.user() != actor
            || !authority
                .operation_origins()
                .iter()
                .any(|origin| origin.operation() == recognition.operation())
        {
            return Ok(false);
        }
        let Some(caller) = recognition.restore(conn, now).await? else {
            return Ok(false);
        };
        // A due checkpoint can have stricter current ceilings than its original HTTP origin.
        // Check it in addition to every origin; never use it as their representative.
        callers.push((recognition.operation().to_owned(), caller));
    }
    authorize_callers(
        conn,
        schedule.id,
        row.topic_id,
        environment,
        worktree,
        now,
        callers,
    )
    .await
}

/// Identity/time export ONLY after the existing scheduled-source ordinary/canonical
/// validator succeeded in the same action transaction. Every definition/recognition
/// origin is retained; no representative actor or substitute operation is constructed.
pub(crate) async fn scheduled_source_current_callers(
    conn: &mut Conn,
    input: Id,
    thread: Id,
    actor: Id,
    recognition: Option<&CapturedOperation>,
    clock: &dyn Clock,
) -> Result<Option<Vec<(Option<orch_authz::Caller>, Option<Time>)>>, EngineError> {
    let Some(row) = threads::input(conn, input).await? else {
        return Ok(None);
    };
    if row.thread_id != thread || row.author_id != Some(actor) {
        return Ok(None);
    }
    let Some(authority) = saved_authority(row.submission_authority.as_ref())? else {
        return Ok(None);
    };
    let Some(restored) = authority.restore_current(conn, actor, clock).await? else {
        return Ok(None);
    };
    let mut callers = restored
        .origins
        .into_iter()
        .map(|origin| (Some(origin.caller), None))
        .collect::<Vec<_>>();
    if let Some(recognition) = recognition {
        let Some(caller) = recognition.restore_current(conn, clock).await? else {
            return Ok(None);
        };
        callers.push((Some(caller), None));
    }
    Ok(Some(callers))
}

async fn authorize_callers(
    conn: &mut Conn,
    schedule: Id,
    topic: Id,
    environment: Id,
    worktree: bool,
    now: Time,
    callers: Vec<(String, orch_authz::Caller)>,
) -> Result<bool, EngineError> {
    let topic = targets::node(conn, topic)
        .await
        .map_err(EngineError::unreadable)?;
    let env = targets::page_object(
        conn,
        ObjectRef {
            kind: ObjectKind::Environment,
            id: environment,
        },
    )
    .await
    .map_err(EngineError::unreadable)?;
    for (selection, caller) in callers {
        let plan = Prepared::for_selection(&selection).map_err(EngineError::unreadable)?;
        if !compatible_selection(&caller, plan.variant()) {
            return Ok(false);
        }
        let mut subject = topic.clone();
        if selection == "InputScheduleUpdate.definition" {
            subject.object = KeyObjectRef {
                kind: SCHEDULE,
                id: Some(schedule),
            };
        }
        let mut required = Vec::new();
        for (resolver, action, kind, target) in [
            (
                TargetResolver::Path("id"),
                KeyAction::Manage,
                SCHEDULE,
                &subject,
            ),
            (
                TargetResolver::Resolved("scheduled_destinations"),
                KeyAction::Write,
                THREAD,
                &topic,
            ),
            (
                TargetResolver::ResolvedCollection("creator_file_environments"),
                KeyAction::Manage,
                ENVIRONMENT,
                &env,
            ),
        ]
        .into_iter()
        .chain(worktree.then_some((
            TargetResolver::ResolvedCollection("creator_file_sources"),
            KeyAction::Read,
            ENVIRONMENT,
            &env,
        ))) {
            let mut matching = plan.variant().facets.iter().filter(|facet| {
                facet.target == resolver
                    && facet.action == action
                    && facet.usage == FacetUsage::Required
                    && facet.resources == [kind]
            });
            let metadata = matching.next().ok_or_else(|| {
                EngineError::unreadable("the scheduled file effect is not declared")
            })?;
            if matching.next().is_some() {
                return Err(EngineError::unreadable(
                    "the scheduled file effect is ambiguous",
                ));
            }
            required.extend(
                facets(&selection, metadata, kind, std::slice::from_ref(target))
                    .map_err(EngineError::unreadable)?,
            );
        }
        let Ok(admitted) = caller.admit_facets(&required) else {
            return Ok(false);
        };
        let principal =
            match Principal::load(conn, admitted.into_caller().with_admin_mode(false), now).await {
                Ok(principal) => principal,
                Err(
                    orch_authz::Error::NotFound(_)
                    | orch_authz::Error::NoSuchUser(_)
                    | orch_authz::Error::Suspended { .. }
                    | orch_authz::Error::Archived { .. },
                ) => return Ok(false),
                Err(error) => return Err(error.into()),
            };
        match orch_authz::resources::check(
            conn,
            &principal,
            ObjectRef {
                kind: ObjectKind::Environment,
                id: environment,
            },
            Permission::EnvironmentManage,
            Policy::default(),
        )
        .await
        {
            Ok(Decision::Allowed { .. }) => {}
            Ok(_)
            | Err(
                orch_authz::Error::NotFound(_)
                | orch_authz::Error::NoSuchUser(_)
                | orch_authz::Error::Suspended { .. }
                | orch_authz::Error::Archived { .. },
            ) => return Ok(false),
            Err(error) => return Err(error.into()),
        }
    }
    Ok(true)
}

/// Check the source actually frozen by the ordinary new-thread writer before its input
/// is accepted. All writes are still private to the due transaction and roll back on refusal.
#[allow(
    clippy::too_many_arguments,
    reason = "a real occurrence checks all original boundaries against its materialized source"
)]
pub async fn scheduled_source_before_acceptance(
    conn: &mut Conn,
    actor: Id,
    origins: &[CapturedOperation],
    reference: ScheduledInputOrigin,
    thread: Id,
    clock: &dyn Clock,
) -> Result<bool, EngineError> {
    if reference.destination != ScheduledInputDestination::NewThread {
        return Err(invalid());
    }
    let authority = super::InputAuthority::for_schedule(actor, origins, reference)?;
    let Some(restored) = authority.restore_current(conn, actor, clock).await? else {
        return Ok(false);
    };
    let Some(schedule) = input_schedules::schedule(conn, reference.schedule_id, false)
        .await?
        .filter(|row| row.author_id == actor)
    else {
        return Ok(false);
    };
    let Some(run) = input_schedules::run_by_id(conn, reference.run_id)
        .await?
        .filter(|run| {
            run.schedule_id == schedule.id && run.thread_id.is_none_or(|actual| actual == thread)
        })
    else {
        return Ok(false);
    };
    if thread
        != crate::ids::derived(
            "input-schedule.thread",
            &[schedule.id.as_bytes(), run.occurrence.as_bytes()],
        )
    {
        return Ok(false);
    }
    let Some(row) = threads::thread(conn, thread)
        .await?
        .filter(|row| row.topic_id == schedule.topic_id)
    else {
        return Ok(false);
    };
    let Some(environment) = row.environment_id else {
        return Ok(false);
    };
    let source = row.source.as_ref().ok_or_else(invalid)?;
    let state = source
        .pointer("/info/state")
        .and_then(serde_json::Value::as_str)
        .ok_or_else(invalid)?;
    if state == "ready" {
        return Ok(restored
            .origins
            .iter()
            .all(|origin| origin.caller.key_deadline_allows(clock)));
    }
    if state != "pending"
        || source.get("prepare").and_then(serde_json::Value::as_bool) != Some(true)
    {
        return Err(invalid());
    }
    let worktree = source
        .get("worktree")
        .is_some_and(|choice| !choice.is_null());
    let callers = restored
        .origins
        .into_iter()
        .map(|origin| {
            let selection = origin.operation.ok_or_else(invalid)?;
            if !matches!(
                selection.as_str(),
                "TopicInputScheduleCreate" | "InputScheduleUpdate.definition"
            ) {
                return Err(invalid());
            }
            Ok((selection, origin.caller))
        })
        .collect::<Result<Vec<_>, EngineError>>()?;
    if callers.is_empty() {
        return Err(invalid());
    }
    // Retain every actual origin through the later target/ordinary-permission waits.
    // This is only a pre-acceptance source check; the input writer rechecks its own
    // retained callers at new input insertion and first-wire checks remain separate.
    let deadlines: Vec<_> = callers.iter().map(|(_, caller)| caller.clone()).collect();
    let allowed = authorize_callers(
        conn,
        schedule.id,
        row.topic_id,
        environment,
        worktree,
        clock.time(),
        callers,
    )
    .await?;
    Ok(allowed
        && deadlines
            .iter()
            .all(|caller| caller.key_deadline_allows(clock)))
}
