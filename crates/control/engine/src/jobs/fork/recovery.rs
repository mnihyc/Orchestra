//! Reducing cleanup of an admitted full-history preparation. No new fork,
//! model turn, file restore or editor authorization is part of this path.

use super::*;
use crate::commands::Outcome;
use orch_db::queries::{link, thread_history_cuts as cuts};
use orch_link_contract::{
    commands::{CommandKind, MachineProbe},
    continuations::{SessionCloseFenced, SessionContinuationResume, SessionOpenFenced},
};

const ORIGINAL: &str = "history_original";
const INTENT: &str = "history_native_changed";

#[derive(Clone, Serialize, Deserialize)]
struct Original {
    machine: Id,
    environment_machine: Option<Id>,
    body: SessionOpen,
    special_resume: bool,
}

/// Resolve the original's real configuration before its first possible close.
/// The ordinary session snapshot's stand-ins allocate no input/turn/lease.
pub(super) async fn retain_original(job: &Job, fork: &Forking) -> Result<(), Stop> {
    if !fork.replace() {
        return Ok(());
    }
    let plan = job.plan().await?;
    if plan.get(ORIGINAL).is_some() {
        return Ok(());
    }
    let old_machine = member::<Id>(&plan, made::OLD_MACHINE);
    let target_machine = plan
        .get("history_target")
        .and_then(|target| target["machine"].as_str())
        .and_then(|id| id.parse::<Id>().ok());
    if old_machine.is_none() || old_machine != target_machine {
        return Ok(());
    }
    let source = source::retained(&plan)?;
    let (Some(segment), Some(native)) = (source.segment, source.native_ref.as_ref()) else {
        return Ok(());
    };
    let (mut snapshot, environment_machine) = {
        let mut conn = job.engine().db().acquire().await?;
        source::authorize(&mut conn, job, &fork.recipe, &source, None).await?;
        let thread = threads::thread(&mut conn, source.thread)
            .await?
            .ok_or_else(|| {
                failed(
                    "history_source_changed",
                    "The original thread is unavailable.",
                )
            })?;
        let old = threads::segment(&mut conn, segment).await?.ok_or_else(|| {
            failed(
                "history_source_changed",
                "The original segment is unavailable.",
            )
        })?;
        let environment = environment_rows::environment(&mut conn, old.environment_id)
            .await?
            .ok_or_else(|| {
                failed(
                    "history_source_changed",
                    "The original environment is unavailable.",
                )
            })?;
        let agent = resource_rows::agent(&mut conn, old.agent_id)
            .await?
            .ok_or_else(|| {
                failed(
                    "history_source_changed",
                    "The original profile is unavailable.",
                )
            })?;
        let provider = resource_rows::provider(&mut conn, old.provider_id)
            .await?
            .ok_or_else(|| {
                failed(
                    "history_source_changed",
                    "The original provider is unavailable.",
                )
            })?;
        let mut recipe = fork.recipe.clone();
        recipe.new_thread = None;
        recipe.new_segment = segment;
        recipe.forkplan.to.agent = recipe.forkplan.from.agent.clone();
        recipe.forkplan.to.provider = recipe.forkplan.from.provider.clone();
        recipe.forkplan.to.arrangement =
            serde_json::from_value(json!(old.arrangement)).map_err(EngineError::from)?;
        recipe.forkplan.to.environment = ForkPlanTarget::Existing(
            recipe.forkplan.from.environment.clone().ok_or_else(|| {
                failed(
                    "history_source_changed",
                    "The original environment reference is unavailable.",
                )
            })?,
        );
        recipe.forkplan.to.directory = thread.workspace_dir;
        let settings = crate::resolved::load(&mut conn, source.thread).await?;
        recipe.forkplan.to.model = settings.model.ok_or_else(|| {
            failed(
                "history_source_changed",
                "The original model is unavailable.",
            )
        })?;
        recipe.forkplan.to.effort = settings.effort;
        let mut snapshot = job
            .engine()
            .session_snapshot(
                &mut conn,
                &Forking { recipe },
                source.thread,
                &environment,
                &agent,
                &provider,
            )
            .await?;
        snapshot.segment = old;
        (snapshot, environment.machine_id)
    };
    snapshot.segment.native_session_ref = Some(native.clone());
    let mut body = crate::sessions::session_open_body(&snapshot, SessionMode::Resume)?;
    body.mcp = job
        .engine()
        .mcp_for_session(snapshot.environment.id, source.thread)
        .await?;
    let original = Original {
        machine: snapshot.key.machine,
        environment_machine,
        body,
        special_resume: snapshot
            .descriptor
            .capabilities
            .native_input_prefix
            .is_some(),
    };
    let mut tx = job.engine().db().begin().await?;
    let owned = job.lock_owned(&mut tx).await?.ok_or(Stop::Lost)?;
    source::authorize(&mut tx, job, &fork.recipe, &source, None).await?;
    if let Some(old) = owned.plan.get(ORIGINAL) {
        if old != &to_json(&original)? {
            return Err(failed(
                "history_source_changed",
                "Another original recovery configuration is retained.",
            ));
        }
    } else {
        orch_db::queries::jobs::merge_plan(
            &mut tx,
            job.id(),
            &json!({ORIGINAL:original}),
            job.now(),
        )
        .await?;
    }
    tx.commit().await.map_err(orch_db::DbError::from)?;
    Ok(())
}

/// Same admission transaction as the changing wire. A cancelled caller cannot
/// lose the obligation to reduce/recover a lane it already admitted.
pub(super) async fn intent(
    conn: &mut orch_db::Conn,
    job: &Job,
    recipe: &Recipe,
    plan: &Json,
    command: &Outgoing,
) -> Result<(), Stop> {
    if recipe.result == ForkResult::Replace
        && matches!(
            command.kind,
            CommandKind::SessionCloseFenced
                | CommandKind::SessionOpenFenced
                | CommandKind::SessionContinuationResume
        )
    {
        let source = source::retained(plan)?;
        let displaces = member::<Id>(plan, made::OLD_MACHINE) == Some(command.machine);
        if displaces && source.native_ref.is_some() && plan.get(ORIGINAL).is_none() {
            return Err(failed(
                "history_original_unconfirmed",
                "Native preparation has no retained original recovery target.",
            ));
        }
        if displaces {
            orch_db::queries::jobs::merge_plan(conn, job.id(), &json!({INTENT:true}), job.now())
                .await?;
        }
    }
    Ok(())
}

fn session(command: &Outgoing) -> Result<Option<SessionOpen>, Stop> {
    Ok(match command.kind {
        CommandKind::SessionOpenFenced => Some(
            *serde_json::from_value::<SessionOpenFenced>(command.body.clone())
                .map_err(EngineError::from)?
                .session,
        ),
        CommandKind::SessionContinuationResume => Some(
            *serde_json::from_value::<SessionContinuationResume>(command.body.clone())
                .map_err(EngineError::from)?
                .session,
        ),
        _ => None,
    })
}

async fn validate(conn: &mut orch_db::Conn, command: &Outgoing) -> Result<(), Stop> {
    let row = link::command(conn, command.id).await?.ok_or_else(|| {
        failed(
            "history_native_unconfirmed",
            "The original admitted native receipt is unavailable.",
        )
    })?;
    if row.machine_id != command.machine
        || row.kind != command.kind.as_str()
        || row.body != command.body
        || row.serial_key() != command.key.wire().as_deref()
        || command.deadline != Some(Time::new(row.deadline_at))
    {
        return Err(failed(
            "history_native_identity",
            "The retained native command changed identity or original budget.",
        ));
    }
    Ok(())
}

/// The actual command supplier, rather than equal JSON or a manufactured Stop,
/// supplies the outcome. A cancelled controller row keeps its original ending;
/// an independently retained success still identifies the one actual child.
async fn outcome(job: &Job, command: &Outgoing) -> Result<Option<Outcome>, Stop> {
    let outcome = job
        .engine()
        .inner
        .commands
        .status(command.id)
        .await
        .map_err(EngineError::from)?
        .and_then(|info| info.outcome);
    let mut tx = job.engine().db().begin().await?;
    job.lock_owned(&mut tx).await?.ok_or(Stop::Lost)?;
    validate(&mut tx, command).await?;
    if let Some(outcome) = &outcome {
        let error;
        let ending = match outcome {
            Outcome::Succeeded(value) => link::Ending::Succeeded(value),
            Outcome::Failed(value) => {
                error = serde_json::to_value(value).map_err(EngineError::from)?;
                link::Ending::Failed(&error)
            }
            Outcome::Cancelled => link::Ending::Cancelled,
            Outcome::Expired => link::Ending::Expired,
        };
        link::finish_command(&mut tx, command.id, &ending, job.now()).await?;
        if matches!(outcome, Outcome::Succeeded(_)) {
            cuts::settle_command(&mut tx, job.id(), command.id).await?;
        }
        if let (Some(open), Outcome::Succeeded(value)) = (session(command)?, outcome) {
            let opened: SessionOpenResult =
                serde_json::from_value(value.clone()).map_err(EngineError::from)?;
            if opened.native_session_ref.is_empty()
                || open.mode == SessionMode::Resume
                    && open.native_session_ref.as_deref()
                        != Some(opened.native_session_ref.as_str())
                || open.mode == SessionMode::Fork
                    && open.native_session_ref.as_deref()
                        == Some(opened.native_session_ref.as_str())
            {
                return Err(failed(
                    "history_native_identity",
                    "The original native result has another source or child identity.",
                ));
            }
            if open.mode != SessionMode::Resume {
                let cut = cuts::cut(&mut tx, job.id()).await?.ok_or_else(|| {
                    failed(
                        "history_source_unconfirmed",
                        "The original cut is unavailable.",
                    )
                })?;
                let name = format!("native_result_{}", command.id);
                if !cuts::keep_artifact(
                    &mut tx,
                    cut.id,
                    &name,
                    &json!({"command":command,"result":opened}),
                    &[],
                )
                .await?
                {
                    return Err(failed(
                        "history_native_identity",
                        "This native command already retained another result.",
                    ));
                }
            }
        }
    }
    tx.commit().await.map_err(orch_db::DbError::from)?;
    Ok(outcome)
}

/// A reducing command owns its own original catalogue deadline. Retrying a
/// genuinely terminal barrier never extends or reissues the changing command.
async fn reducing(
    job: &Job,
    slot: &str,
    template: Outgoing,
    original: Option<Id>,
    previous: Option<Id>,
) -> Result<Outgoing, Stop> {
    let mut tx = job.engine().db().begin().await?;
    let owned = job.lock_owned(&mut tx).await?.ok_or(Stop::Lost)?;
    if owned.plan.get("commit_done").is_some() {
        return Err(Stop::Lost);
    }
    if matches!(
        template.kind,
        CommandKind::SessionOpenFenced | CommandKind::SessionContinuationResume
    ) {
        let source = source::retained(&owned.plan)?;
        let original: Original =
            serde_json::from_value(owned.plan.get(ORIGINAL).cloned().ok_or_else(|| {
                failed(
                    "history_original_unconfirmed",
                    "The original recovery target is unavailable.",
                )
            })?)
            .map_err(EngineError::from)?;
        if !eligible(&mut tx, &source, &original).await? {
            return Err(failed(
                "history_original_withdrawn",
                "The original recovery target was withdrawn or another lane is selected.",
            ));
        }
    }
    let mut all = owned
        .plan
        .get("history_recovery")
        .cloned()
        .unwrap_or_else(|| json!({}));
    let mut command = if let Some(value) = all.get(slot) {
        let old: Outgoing = serde_json::from_value(value.clone()).map_err(EngineError::from)?;
        if old.machine != template.machine
            || old.key != template.key
            || old.kind != template.kind
            || old.body != template.body
        {
            return Err(failed(
                "history_native_identity",
                "The reducing receipt names another native lane or queue.",
            ));
        }
        if previous == Some(old.id) {
            template
        } else if previous.is_some() && old.id != template.id {
            return Err(Stop::Lost);
        } else {
            old
        }
    } else {
        template
    };
    if command.deadline.is_none() {
        command.deadline = Some(Time::new(
            job.now() + chrono::Duration::seconds(i64::from(command.kind.deadline_secs())),
        ));
    }
    let deadline = command.deadline.ok_or_else(|| {
        failed(
            "history_native_unconfirmed",
            "The recovery budget is unavailable.",
        )
    })?;
    let key = command.key.wire();
    link::insert_command(
        &mut tx,
        &link::NewCommand {
            id: command.id,
            machine_id: command.machine,
            kind: command.kind.as_str(),
            body: &command.body,
            serial_key: key.as_deref(),
            deadline_at: deadline.get(),
        },
        job.now(),
    )
    .await?;
    validate(&mut tx, &command).await?;
    if let Some(original) = original {
        sqlx::query(
            "UPDATE thread_history_cut_commands SET fence_id=$3 WHERE cut_id=$1 AND command_id=$2",
        )
        .bind(job.id())
        .bind(original)
        .bind(command.id)
        .execute(&mut *tx)
        .await
        .map_err(orch_db::DbError::from)?;
    } else {
        cuts::hold_command(&mut tx, job.id(), command.id, command.machine).await?;
        if let Some(previous) = previous {
            // The replacement wire and removal of the genuinely drained old
            // recovery hold publish atomically; zero-grace GC cannot erase an
            // old receipt while the plan still depends on it for advancement.
            let removed=sqlx::query("DELETE FROM thread_history_cut_commands WHERE cut_id=$1 AND command_id=$2 AND settled")
                .bind(job.id()).bind(previous).execute(&mut *tx).await.map_err(orch_db::DbError::from)?.rows_affected();
            if removed != 1 {
                return Err(failed(
                    "history_native_unconfirmed",
                    "The previous recovery attempt has no verified completion barrier.",
                ));
            }
        }
    }
    all[slot] = to_json(&command)?;
    orch_db::queries::jobs::merge_plan(
        &mut tx,
        job.id(),
        &json!({"history_recovery":all}),
        job.now(),
    )
    .await?;
    tx.commit().await.map_err(orch_db::DbError::from)?;
    Ok(command)
}

async fn retired(job: &Job, command: Id) -> Result<bool, Stop> {
    let mut conn = job.engine().db().acquire().await?;
    Ok(sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM thread_history_cut_commands h JOIN commands r
      ON r.machine_id=h.retirement_machine_id AND r.kind='node.runtime.remove' AND r.state='succeeded'
        AND r.body->>'runtime_id'=h.runtime_id::text WHERE h.command_id=$1 AND h.cut_id=$2 AND h.runtime_id IS NOT NULL)")
        .bind(command).bind(job.id()).fetch_one(&mut *conn).await.map_err(orch_db::DbError::from)?)
}

/// Fence one original stage. Same-key completion/actual physical retirement is
/// proof; changed generation, age, desired state and cancellation are not.
async fn drain(job: &Job, original: &Outgoing, close: bool) -> Result<(), Stop> {
    let actual = outcome(job, original).await?;
    if !close && matches!(actual, Some(Outcome::Succeeded(_))) {
        return Ok(());
    }
    if matches!(&actual,Some(Outcome::Failed(error)) if error.code=="unsupported")
        && session(original)?.is_some()
    {
        let mut conn = job.engine().db().acquire().await?;
        cuts::settle_command(&mut conn, job.id(), original.id).await?;
        return Ok(());
    }
    if retired(job, original.id).await? {
        let mut conn = job.engine().db().acquire().await?;
        cuts::settle_command(&mut conn, job.id(), original.id).await?;
        return Ok(());
    }
    // Withdraw a not-yet-adopted output before placing its barrier. The final
    // dispatcher admission check prevents a prepared issuer arriving afterward.
    {
        let mut tx = job.engine().db().begin().await?;
        job.lock_owned(&mut tx).await?.ok_or(Stop::Lost)?;
        validate(&mut tx, original).await?;
        link::finish_command(&mut tx, original.id, &link::Ending::Cancelled, job.now()).await?;
        tx.commit().await.map_err(orch_db::DbError::from)?;
    }
    job.engine()
        .inner
        .commands
        .cancel(original.id)
        .await
        .map_err(EngineError::from)?;
    let plan = job.plan().await?;
    let restore = plan
        .get("history_recovery")
        .and_then(|all| all.get("restore_original"))
        .and_then(|value| value["id"].as_str())
        .is_some_and(|id| id == original.id.to_string());
    let slot = if restore {
        "restore_reduce".to_owned()
    } else {
        format!("reduce_{}", original.id)
    };
    let retained = plan
        .get("history_recovery")
        .and_then(|all| all.get(&slot))
        .cloned()
        .map(serde_json::from_value::<Outgoing>)
        .transpose()
        .map_err(EngineError::from)?;
    let mut previous = None;
    let id = if let Some(command) = &retained {
        let associated = {
            let mut conn = job.engine().db().acquire().await?;
            sqlx::query_scalar::<_,Option<Id>>("SELECT fence_id FROM thread_history_cut_commands WHERE cut_id=$1 AND command_id=$2")
                .bind(job.id()).bind(original.id).fetch_optional(&mut *conn).await.map_err(orch_db::DbError::from)?.flatten()==Some(command.id)
        };
        if !associated {
            previous = Some(command.id);
            ids::derived("fork.history-reduce", &[original.id.as_bytes()])
        } else {
            let known = outcome(job, command).await?;
            if matches!(&known, Some(Outcome::Succeeded(_)))
                || close
                    && matches!(&known,
            Some(Outcome::Failed(error)) if error.code=="session.segment_changed")
            {
                let mut conn = job.engine().db().acquire().await?;
                cuts::settle_command(&mut conn, job.id(), original.id).await?;
                return Ok(());
            }
            if known.is_some() {
                if command
                    .deadline
                    .is_some_and(|deadline| job.now() < deadline.get())
                {
                    return Err(failed(
                        "history_recovery_wait",
                        "The reducing attempt is terminal; its retained retry point is kept.",
                    ));
                }
                previous = Some(command.id);
                crate::commands::busy_retry_id(command.id)
            } else {
                command.id
            }
        }
    } else {
        ids::derived("fork.history-reduce", &[original.id.as_bytes()])
    };
    let template = if close {
        let opened = session(original)?.ok_or_else(|| {
            failed(
                "history_native_identity",
                "The native creation has no captured segment.",
            )
        })?;
        Outgoing::new(
            id,
            original.machine,
            original.key.clone(),
            &SessionCloseFenced {
                thread_id: opened.thread_id,
                segment_id: opened.segment_id,
            },
        )
    } else {
        Outgoing::new(id, original.machine, original.key.clone(), &MachineProbe {})
    }
    .map_err(EngineError::from)?;
    let command = reducing(job, &slot, template, Some(original.id), previous).await?;
    let answer = job.command_to_undo(command.clone()).await;
    outcome(job, &command).await?;
    if answer.is_ok()
        || close
            && matches!(&answer,Err(Stop::Failed(error)) if error.code=="session.segment_changed")
    {
        let mut conn = job.engine().db().acquire().await?;
        cuts::settle_command(&mut conn, job.id(), original.id).await?;
        return Ok(());
    }
    Err(answer.err().unwrap_or_else(|| {
        failed(
            "history_recovery_wait",
            "The native queue still has an unknown outcome.",
        )
    }))
}

/// First reduce every owned child/unknown output. Only then may original-lane
/// recovery or destruction of temporary files/targets proceed.
pub(super) async fn undo(job: &Job, fork: &Forking) -> Result<(), Stop> {
    let plan = job.plan().await?;
    if plan.get("commit_done").is_some() {
        return Ok(());
    }
    let wires: Vec<Outgoing> = plan
        .get("history_wire")
        .and_then(Json::as_object)
        .into_iter()
        .flat_map(|wire| wire.values())
        .map(|value| serde_json::from_value(value.clone()).map_err(EngineError::from))
        .collect::<Result<_, _>>()?;
    // Reverse admission order: any last successful child is closed before the
    // original gets resumed. The identities come from actual saved bodies.
    for command in wires.iter().rev() {
        let opening = session(command)?.is_some();
        drain(job, command, opening).await?;
    }
    if !fork.replace() || plan.get(INTENT) != Some(&Json::Bool(true)) {
        return Ok(());
    }
    let Some(value) = plan.get(ORIGINAL) else {
        return Ok(());
    };
    let original: Original = serde_json::from_value(value.clone()).map_err(EngineError::from)?;
    restore(job, fork, &original).await
}

async fn restore(job: &Job, fork: &Forking, original: &Original) -> Result<(), Stop> {
    let plan = job.plan().await?;
    let source = source::retained(&plan)?;
    if plan.get("history_original_restored").is_some() {
        return Ok(());
    }
    let allowed = {
        let mut tx = job.engine().db().begin().await?;
        job.lock_owned(&mut tx).await?.ok_or(Stop::Lost)?;
        let allowed = eligible(&mut tx, &source, original).await?;
        tx.commit().await.map_err(orch_db::DbError::from)?;
        allowed
    };
    // An authorized ending/new lane is never resurrected by old housekeeping.
    // This is not a claim that a withdrawn original was resumed.
    if !allowed {
        return Ok(());
    }
    let saved = plan
        .get("history_recovery")
        .and_then(|all| all.get("restore_original"))
        .cloned()
        .map(serde_json::from_value::<Outgoing>)
        .transpose()
        .map_err(EngineError::from)?;
    let mut previous = None;
    let id = if let Some(saved) = &saved {
        let actual = outcome(job, saved).await?;
        if let Some(Outcome::Succeeded(value)) = &actual {
            let opened: SessionOpenResult =
                serde_json::from_value(value.clone()).map_err(EngineError::from)?;
            if original.body.native_session_ref.as_deref()
                != Some(opened.native_session_ref.as_str())
            {
                return Err(failed(
                    "history_native_identity",
                    "The retained original resume names another conversation.",
                ));
            }
            return Ok(());
        }
        if actual.is_some() {
            if saved
                .deadline
                .is_some_and(|deadline| job.now() < deadline.get())
            {
                return Err(failed(
                    "history_recovery_wait",
                    "The original resume retains its recovery retry point.",
                ));
            }
            drain(job, saved, true).await?;
            previous = Some(saved.id);
            crate::commands::busy_retry_id(saved.id)
        } else {
            saved.id
        }
    } else {
        ids::derived("fork.history-restore", &[job.id().as_bytes()])
    };
    let native = source.native_ref.ok_or_else(|| {
        failed(
            "history_original_unconfirmed",
            "The original native identity is unavailable.",
        )
    })?;
    if !original.special_resume {
        let close = Outgoing::new(
            ids::derived("fork.history-original-close", &[job.id().as_bytes()]),
            original.machine,
            SerialKey::Thread(source.thread),
            &SessionCloseFenced {
                thread_id: source.thread,
                segment_id: original.body.segment_id,
            },
        )
        .map_err(EngineError::from)?;
        let command = reducing(job, "restore_close_original", close, None, None).await?;
        let answer = job.command_to_undo(command.clone()).await;
        outcome(job, &command).await?;
        if answer.is_err()
            && !matches!(&answer,Err(Stop::Failed(error)) if error.code=="session.segment_changed")
        {
            return Err(answer.err().ok_or(Stop::Lost)?);
        }
        let mut conn = job.engine().db().acquire().await?;
        cuts::settle_command(&mut conn, job.id(), command.id).await?;
    }
    let template = if original.special_resume {
        Outgoing::new(
            id,
            original.machine,
            SerialKey::Thread(source.thread),
            &SessionContinuationResume {
                thread_id: source.thread,
                session: Box::new(original.body.clone()),
                source: orch_agent_contract::history::NativePrefixSource {
                    thread_id: source.thread,
                    segment_id: original.body.segment_id,
                    native_session_ref: native.clone(),
                },
                replace_segment_id: Some(fork.recipe.new_segment),
                replace_retained_source: None,
            },
        )
    } else {
        Outgoing::new(
            id,
            original.machine,
            SerialKey::Thread(source.thread),
            &SessionOpenFenced {
                thread_id: source.thread,
                session: Box::new(original.body.clone()),
                predecessor: None,
            },
        )
    }
    .map_err(EngineError::from)?;
    let command = match reducing(job, "restore_original", template, None, previous).await {
        Err(Stop::Failed(error)) if error.code == "history_original_withdrawn" => return Ok(()),
        other => other?,
    };
    let answer = job.command_to_undo(command.clone()).await;
    outcome(job, &command).await?;
    let answer = match answer {
        Ok(value) => value,
        Err(error) => return Err(error),
    };
    let opened: SessionOpenResult = serde_json::from_value(answer).map_err(EngineError::from)?;
    if opened.native_session_ref != native {
        return Err(failed(
            "history_native_identity",
            "Recovery resumed another native conversation.",
        ));
    }
    let mut tx = job.engine().db().begin().await?;
    job.lock_owned(&mut tx).await?.ok_or(Stop::Lost)?;
    cuts::settle_command(&mut tx, job.id(), command.id).await?;
    orch_db::queries::jobs::merge_plan(&mut tx,job.id(),&json!({"history_original_restored":{
        "command_id":command.id,"segment_id":original.body.segment_id,"native_session_ref":native}}),job.now()).await?;
    tx.commit().await.map_err(orch_db::DbError::from)?;
    job.engine().forget_open_sessions(source.thread);
    Ok(())
}

/// The authoritative runner/real original target gate is repeated in the same
/// transaction as a restore admission. It never consults the failed candidate
/// binding or demands a revoked editor grant for no-new-work recovery.
async fn eligible(
    conn: &mut orch_db::Conn,
    source: &source::Source,
    original: &Original,
) -> Result<bool, Stop> {
    let thread = threads::lock(conn, source.thread).await?;
    let segment = threads::segment(conn, original.body.segment_id).await?;
    let environment =
        environment_rows::environment(conn, original.body.exec.environment_id).await?;
    Ok(thread
        .zip(segment)
        .zip(environment)
        .is_some_and(|((thread, segment), environment)| {
            thread.deleted_at.is_none()
                && thread.archived_at.is_none()
                && thread.active_continuation_id == source.view
                && thread.execution_revision == source.execution_revision
                && thread.current_segment_id == source.segment
                && thread.current_turn_id.is_none()
                && thread.agent_id == Some(segment.agent_id)
                && thread.provider_id == Some(segment.provider_id)
                && thread.environment_id == Some(segment.environment_id)
                && thread.arrangement.as_deref() == Some(segment.arrangement.as_str())
                && segment.native_session_ref == source.native_ref
                && environment.generation == segment.env_generation
                && environment.machine_id == original.environment_machine
                && environment.deleted_at.is_none()
                && environment.remove_at.is_none()
                && !matches!(environment.status.as_str(), "disabled" | "removed")
        }))
}
