//! Exact native prefix/resume preparation precedes one claimed continuation-selection commit.

use super::runner::{Done, Failure, Job, Phase, Phased, Stop, Undone};
use crate::{
    commands::Outgoing,
    continuations::{
        admission,
        recipe::{Kind, Lane, NativeCreation, PreparedNative as Prepared, Recipe, RunningAction},
    },
    error::EngineError,
    unit::Unit,
};
use async_trait::async_trait;
use orch_authz::{Caller, TokenGrant};
use orch_db::queries::{jobs, thread_continuations as rows, threads};
use orch_link_contract::{
    commands::{CommandBody, SerialKey, SessionMode, SessionOpen, SessionOpenResult},
    continuations::{
        SessionContinuationClose, SessionContinuationResume, SessionPrefixInspect,
        SessionPrefixOpen,
    },
};
use orch_types::{Id, Json, Time};
use serde_json::json;
use std::time::Duration;

pub const KIND: &str = rows::JOB_KIND;

mod lifecycle;
#[cfg(test)]
mod resume_trace;
pub(crate) use lifecycle::supersede_in;
pub const PHASES: &[Phase] = &[
    Phase {
        name: "quiesce",
        weight: 10,
    },
    Phase {
        name: "prepare",
        weight: 70,
    },
    Phase {
        name: "activate",
        weight: 20,
    },
];
// The same quarter-second observation cadence as the existing fork/switch job.
const LOOK_EVERY: Duration = Duration::from_millis(250);

pub struct Continuing;

fn failed(code: &str, detail: impl Into<String>) -> Stop {
    Failure::new(code, detail).into()
}
fn internal(error: impl std::fmt::Display) -> Stop {
    failed("internal", error.to_string())
}

fn sender_failed(error: crate::inputs::InputError) -> Stop {
    match error {
        crate::inputs::InputError::Internal(error) => error.into(),
        crate::inputs::InputError::Denied(refusal) => failed(
            "authz.denied",
            if refusal.required == orch_access_contract::permissions::Permission::ProviderUse {
                "The accepted credential does not include provider.use."
            } else {
                "The accepted credential no longer permits interaction or this provider, model and effort."
            },
        ),
        other => failed("continuation.input_refused", other.to_string()),
    }
}

impl Continuing {
    pub fn of(row: &jobs::JobRow) -> Result<Self, EngineError> {
        let recipe: Recipe = serde_json::from_value(row.plan["recipe"].clone())?;
        if recipe.operation_id != row.id || Some(recipe.thread_id) != row.node_id {
            return Err(EngineError::unreadable(
                "The continuation job has another operation or thread identity.",
            ));
        }
        Ok(Self)
    }

    async fn recipe(job: &Job) -> Result<Recipe, Stop> {
        serde_json::from_value(job.plan().await?["recipe"].clone()).map_err(internal)
    }

    async fn command(job: &Job, command: Outgoing, undo: bool) -> Result<Json, Stop> {
        #[cfg(test)]
        resume_trace::note(job.id(), &command);
        let outcome = if undo {
            job.command_to_undo(command.clone()).await
        } else {
            job.command(command.clone()).await
        };
        lifecycle::mirror_command(job, &command).await?;
        outcome
    }

    async fn current(
        job: &Job,
        conn: &mut orch_db::Conn,
        recipe: &Recipe,
        undo: bool,
        waiting: bool,
    ) -> Result<threads::ThreadRow, Stop> {
        Self::current_in_lane(job, conn, recipe, undo, waiting, None).await
    }

    async fn current_in_lane(
        job: &Job,
        conn: &mut orch_db::Conn,
        recipe: &Recipe,
        undo: bool,
        waiting: bool,
        reducing_original: Option<&Lane>,
    ) -> Result<threads::ThreadRow, Stop> {
        let owned = job.lock_owned(conn).await?.ok_or(Stop::Lost)?;
        if undo && lifecycle::retirement(&owned.plan, recipe)?.is_some() {
            return Err(failed(
                "continuation.retiring",
                "A committed lifecycle ending now owns native cleanup; the original lane is not resumed.",
            ));
        }
        if !undo {
            if let Some(abort) = super::runner::abort_reason(&owned)? {
                return Err(abort);
            }
            if owned.cancel_requested() {
                return Err(Stop::Cancelled);
            }
            if job.now() >= recipe.deadline.get() {
                return Err(failed(
                    "continuation.timeout",
                    "The accepted native preparation budget expired; retained work is kept.",
                ));
            }
        }
        let thread = threads::lock(conn, recipe.thread_id)
            .await?
            .ok_or_else(|| failed("not_found", "The thread was removed."))?;
        if thread.active_continuation_id != recipe.request.expected.active_continuation_id
            || thread.execution_revision != recipe.request.expected.execution_revision
        {
            return Err(failed(
                "continuation.changed",
                "Another execution selection now owns this thread.",
            ));
        }
        if !waiting && thread.current_turn_id.is_some() {
            return Err(failed(
                "continuation.busy",
                "The native turn must finish before its session can be replaced.",
            ));
        }
        // Recovery validates the lane it restores. A rejected inactive candidate must
        // never prevent restoring an otherwise valid original native conversation.
        let lane = reducing_original.unwrap_or_else(|| {
            if undo {
                recipe.original_lane.as_ref().unwrap_or(&recipe.source_lane)
            } else {
                &recipe.source_lane
            }
        });
        if thread.agent_id != Some(lane.agent_id)
            || thread.provider_id != Some(lane.provider_id)
            || thread.environment_id != Some(lane.environment_id)
        {
            return Err(failed(
                "continuation.binding_changed",
                "The thread now belongs to another binding; this operation cannot replace its native lane.",
            ));
        }
        if !admission::generation_current(conn, lane).await? {
            return Err(failed(
                "continuation.source_changed",
                "The environment generation changed; the retained native conversation is kept.",
            ));
        }
        if !undo {
            let principal = super::source::load_principal(
                conn,
                job,
                recipe.actor_id,
                recipe.authorization_token,
            )
            .await?;
            admission::require_view(conn, &principal, recipe.thread_id)
                .await
                .map_err(internal)?;
            if recipe.request.kind == Kind::Edit {
                let input = threads::input(
                    conn,
                    recipe.request.before_input_id.ok_or_else(|| {
                        failed(
                            "continuation.identity",
                            "The edited input identity is missing.",
                        )
                    })?,
                )
                .await?
                .filter(|input| {
                    input.thread_id == recipe.thread_id
                        && input.source == "user"
                        && input.author_id == Some(recipe.actor_id)
                })
                .ok_or_else(|| {
                    failed(
                        "authz.denied",
                        "Only the original author can prepare these revised words.",
                    )
                })?;
                if input.state != threads::input_state::DELIVERED {
                    return Err(failed(
                        "continuation.input_changed",
                        "The original input is not a retained delivered prompt.",
                    ));
                }
            }
            if recipe.needs_prefix {
                let agent = orch_db::queries::resources::thread_engine::agent(conn, lane.agent_id)
                    .await?
                    .ok_or_else(|| failed("not_found", "The source agent profile was removed."))?;
                if crate::harnesses::descriptor_of(&agent)?
                    .and_then(|descriptor| descriptor.capabilities.native_input_prefix)
                    != Some(orch_agent_contract::descriptor::NativeInputPrefixSupport::TurnStart)
                {
                    return Err(failed(
                        "continuation.native_unsupported",
                        "The current agent program does not declare exact turn-start preparation.",
                    ));
                }
            }
            let mut callers = vec![principal.caller().clone()];
            if let Some(scope) = &recipe.authorization_scope {
                callers.push(Caller::token(
                    recipe.actor_id,
                    TokenGrant::from_stored(&scope.scopes, scope.subtree).map_err(internal)?,
                ));
            }
            let mut unit = Unit::new(conn, job.now());
            for caller in callers {
                job.engine()
                    .check_sender(&mut unit, &caller, &thread, true)
                    .await
                    .map_err(sender_failed)?;
                if let Some(content) = &recipe.request.content {
                    job.engine()
                        .check_content(&mut unit, &caller, content)
                        .await
                        .map_err(internal)?;
                }
            }
        }
        Ok(thread)
    }

    /// Freeze the entire outgoing body before admission; replay never rebuilds it from newer settings.
    async fn saved<B: CommandBody>(
        job: &Job,
        slot: &str,
        id: Id,
        machine: Id,
        body: &B,
        deadline: Option<Time>,
    ) -> Result<Outgoing, Stop> {
        Self::saved_with_gate(
            job,
            slot,
            id,
            machine,
            body,
            deadline,
            slot == "close_active",
        )
        .await
    }

    async fn saved_with_gate<B: CommandBody>(
        job: &Job,
        slot: &str,
        id: Id,
        machine: Id,
        body: &B,
        deadline: Option<Time>,
        reducing_original: bool,
    ) -> Result<Outgoing, Stop> {
        let mut tx = job.engine().db().begin().await?;
        let row = job.lock_owned(&mut tx).await?.ok_or(Stop::Lost)?;
        let recipe: Recipe =
            serde_json::from_value(row.plan["recipe"].clone()).map_err(internal)?;
        // Closing the captured original is a reducing operation, not permission
        // to author in an unavailable candidate. Its exact lane/command/body is
        // required even for an older first-close recovery checkpoint. Fresh
        // preparation additionally rechecks the candidate in this transaction.
        let original = if slot == "close_active" {
            let original = recipe.original_lane.as_ref().ok_or_else(|| {
                failed(
                    "continuation.identity",
                    "The captured original lane is absent.",
                )
            })?;
            let expected = SessionContinuationClose {
                thread_id: recipe.thread_id,
                provider_id: original.provider_id,
                agent_id: original.agent_id,
                source: original.source.clone(),
            };
            if id != recipe.commands.close_active
                || machine != original.machine_id
                || B::KIND != orch_link_contract::commands::CommandKind::SessionContinuationClose
                || serde_json::to_value(body).map_err(internal)?
                    != serde_json::to_value(expected).map_err(internal)?
            {
                return Err(failed(
                    "continuation.identity",
                    "The reducing close does not name the exact captured original lane.",
                ));
            }
            Some(original)
        } else {
            None
        };
        if reducing_original && original.is_none() {
            return Err(failed(
                "continuation.identity",
                "This command cannot use original-lane reduction.",
            ));
        }
        // Commit actual queue admission under the same source/lifecycle lock as its
        // frozen body. A later archive can then withdraw it before a delayed issue.
        Self::current_in_lane(
            job,
            &mut tx,
            &recipe,
            slot == "restore_active",
            false,
            if reducing_original { original } else { None },
        )
        .await?;
        if slot == "open_prefix" {
            if B::KIND != orch_link_contract::commands::CommandKind::SessionPrefixOpen
                || id != recipe.commands.open_prefix
                || machine != recipe.source_lane.machine_id
            {
                return Err(failed(
                    "continuation.identity",
                    "The native creation command has another retained identity.",
                ));
            }
            let creation = serde_json::to_value(NativeCreation {
                operation_id: job.id(),
                command_id: id,
                machine_id: machine,
                segment_id: recipe.segment_id,
            })
            .map_err(internal)?;
            let changed = sqlx::query(
                "UPDATE thread_continuations SET native_creation=COALESCE(native_creation,$3)
                WHERE thread_id=$1 AND id=$2 AND (native_creation IS NULL OR native_creation=$3)",
            )
            .bind(recipe.thread_id)
            .bind(recipe.target_continuation_id)
            .bind(creation)
            .execute(&mut *tx)
            .await
            .map_err(orch_db::DbError::from)?
            .rows_affected();
            if changed != 1 {
                return Err(failed(
                    "continuation.creation_unknown",
                    "This version already owns a different or unconfirmed native creation; another child is not created.",
                ));
            }
        }
        if let Some(saved) = row.plan.get("wire").and_then(|wire| wire.get(slot)) {
            let saved: Outgoing = serde_json::from_value(saved.clone()).map_err(internal)?;
            let terminal_restore = slot == "restore_active"
                && row
                    .plan
                    .get("restore_terminal")
                    .and_then(Json::as_str)
                    .and_then(|id| id.parse::<Id>().ok())
                    == Some(saved.id);
            if saved.machine != machine
                || saved.kind != B::KIND
                || saved.key != SerialKey::Thread(body_thread(&row)?)
                || saved.id != id && !terminal_restore
                || original.is_some()
                    && saved.body != serde_json::to_value(body).map_err(internal)?
            {
                return Err(failed(
                    "continuation.identity",
                    "A retained native command names another operation lane.",
                ));
            }
            if saved.id == id {
                let saved = lifecycle::retain_command(&mut tx, saved, job.now(), false).await?;
                tx.commit().await.map_err(orch_db::DbError::from)?;
                return Ok(saved);
            }
        }
        let mut outgoing = Outgoing::new(id, machine, SerialKey::Thread(body_thread(&row)?), body)
            .map_err(internal)?;
        outgoing.deadline = deadline;
        let outgoing = lifecycle::retain_command(&mut tx, outgoing, job.now(), true).await?;
        let mut wire = row.plan.get("wire").cloned().unwrap_or_else(|| json!({}));
        wire[slot] = serde_json::to_value(&outgoing).map_err(internal)?;
        jobs::merge_plan(&mut tx, job.id(), &json!({"wire":wire}), job.now()).await?;
        tx.commit().await.map_err(orch_db::DbError::from)?;
        Ok(outgoing)
    }

    /// Preserve an already admitted native outcome under its runner claim, independently of
    /// cancellation/actor rights. This records history; selection still uses current(false).
    async fn retain_prepared(job: &Job, recipe: &Recipe, prepared: &Prepared) -> Result<(), Stop> {
        if let Prepared::Prefix(opened) = prepared {
            recipe
                .verify_opened(opened)
                .map_err(|detail| failed("continuation.identity", detail))?;
        }
        let mut tx = job.engine().db().begin().await?;
        let owned = job.lock_owned(&mut tx).await?.ok_or(Stop::Lost)?;
        let actual: Recipe =
            serde_json::from_value(owned.plan["recipe"].clone()).map_err(internal)?;
        if actual.operation_id != recipe.operation_id
            || actual.target_continuation_id != recipe.target_continuation_id
            || actual.segment_id != recipe.segment_id
            || actual.commands != recipe.commands
        {
            return Err(Stop::Lost);
        }
        if owned.plan.get("prepared") == Some(&serde_json::to_value(prepared).map_err(internal)?) {
            tx.commit().await.map_err(orch_db::DbError::from)?;
            return Ok(());
        }
        let mut unit = Unit::new(&mut tx, job.now());
        let seq = unit.stamp(recipe.thread_id).await?;
        let changed=sqlx::query("UPDATE segments SET native_session_ref=$4,seq_updated=$5
            WHERE id=$1 AND thread_id=$2 AND continuation_id=$3 AND (native_session_ref IS NULL OR native_session_ref=$4)")
            .bind(recipe.segment_id).bind(recipe.thread_id).bind(recipe.target_continuation_id).bind(prepared.native()).bind(seq)
            .execute(&mut *unit.conn).await.map_err(orch_db::DbError::from)?.rows_affected();
        if changed != 1 {
            return Err(failed(
                "continuation.identity",
                "The successful native result does not match its owned prepared segment.",
            ));
        }
        if let Prepared::Prefix(opened) = prepared {
            let creation = serde_json::to_value(NativeCreation {
                operation_id: job.id(),
                command_id: recipe.commands.open_prefix,
                machine_id: recipe.source_lane.machine_id,
                segment_id: recipe.segment_id,
            })
            .map_err(internal)?;
            let opened = serde_json::to_value(opened).map_err(internal)?;
            let changed=sqlx::query("UPDATE thread_continuations SET native_creation_result=COALESCE(native_creation_result,$4)
                WHERE thread_id=$1 AND id=$2 AND native_creation=$3
                AND (native_creation_result IS NULL OR native_creation_result=$4)")
                .bind(recipe.thread_id).bind(recipe.target_continuation_id).bind(creation).bind(opened)
                .execute(&mut *unit.conn).await.map_err(orch_db::DbError::from)?.rows_affected();
            if changed != 1 {
                return Err(failed(
                    "continuation.identity",
                    "The native result belongs to another creation receipt.",
                ));
            }
        }
        sqlx::query("UPDATE thread_continuations SET native_source=COALESCE(native_source,$3),
            native_prefix=COALESCE(native_prefix,$4),open_result=$5,current_segment_id=$6 WHERE thread_id=$1 AND id=$2")
            .bind(recipe.thread_id).bind(recipe.target_continuation_id)
            .bind(serde_json::to_value(&recipe.source_lane.source).map_err(internal)?)
            .bind(recipe.prefix.as_ref().map(serde_json::to_value).transpose().map_err(internal)?)
            .bind(serde_json::to_value(prepared).map_err(internal)?).bind(recipe.segment_id)
            .execute(&mut *unit.conn).await.map_err(orch_db::DbError::from)?;
        jobs::merge_plan(
            unit.conn,
            job.id(),
            &json!({"recipe":recipe,"prepared":prepared}),
            job.now(),
        )
        .await?;
        unit.finish().await?;
        tx.commit().await.map_err(orch_db::DbError::from)?;
        Ok(())
    }

    /// The existing durable command journal is authoritative even if activation was cancelled
    /// or its metadata checkpoint was lost. No machine/second connection is awaited inside a Tx.
    async fn reconcile_creation(job: &Job, recipe: &Recipe) -> Result<Option<Prepared>, Stop> {
        if !recipe.needs_prefix {
            return Ok(None);
        }
        let continuation = {
            let mut conn = job
                .engine()
                .db()
                .acquire()
                .await
                .map_err(EngineError::from)?;
            rows::continuation(&mut conn, recipe.thread_id, recipe.target_continuation_id).await?
        }
        .ok_or_else(|| failed("not_found", "The retained continuation was removed."))?;
        let Some(creation) = continuation.native_creation else {
            return Ok(None);
        };
        let creation: NativeCreation = serde_json::from_value(creation).map_err(internal)?;
        if creation.operation_id != recipe.operation_id
            || creation.command_id != recipe.commands.open_prefix
            || creation.segment_id != recipe.segment_id
            || creation.machine_id != recipe.source_lane.machine_id
        {
            return Err(failed(
                "continuation.creation_unknown",
                "The original creation receipt belongs to another operation; another child is not created.",
            ));
        }
        if let Some(command) = job
            .plan()
            .await?
            .get("wire")
            .and_then(|wire| wire.get("open_prefix"))
        {
            let command: Outgoing = serde_json::from_value(command.clone()).map_err(internal)?;
            lifecycle::mirror_command(job, &command).await?;
        }
        let invalid_result = |_| {
            failed(
                "continuation.native_result_invalid",
                "The retained native program result does not describe the inspected prefix.",
            )
        };
        let opened = if let Some(opened) = continuation.native_creation_result {
            Some(serde_json::from_value(opened).map_err(invalid_result)?)
        } else {
            match job
                .engine()
                .inner
                .commands
                .status(creation.command_id)
                .await
                .map_err(internal)?
                .and_then(|info| info.outcome)
            {
                Some(crate::commands::Outcome::Succeeded(value)) => {
                    Some(serde_json::from_value(value).map_err(invalid_result)?)
                }
                _ => None,
            }
        };
        let Some(opened) = opened else {
            return Ok(None);
        };
        let prepared = Prepared::Prefix(opened);
        Self::retain_prepared(job, recipe, &prepared).await?;
        Ok(Some(prepared))
    }

    async fn body(
        job: &Job,
        recipe: &Recipe,
        lane: &Lane,
        segment: Id,
        mode: SessionMode,
        undo: bool,
    ) -> Result<SessionOpen, Stop> {
        let mut tx = job.engine().db().begin().await?;
        Self::current(job, &mut tx, recipe, undo, false).await?;
        let mut segment = threads::segment(&mut tx, segment)
            .await?
            .ok_or_else(|| failed("not_found", "The retained segment was removed."))?;
        segment.native_session_ref = Some(lane.source.native_session_ref.clone());
        let snapshot = job
            .engine()
            .continuation_snapshot(&mut tx, recipe.thread_id, recipe.source_turn_id, segment)
            .await?;
        if snapshot.key.machine != lane.machine_id
            || snapshot.environment.generation != lane.environment_generation
        {
            return Err(failed(
                "continuation.source_changed",
                "The retained native conversation belongs to another machine or environment generation.",
            ));
        }
        let mut body = crate::sessions::session_open_body(&snapshot, mode)?;
        body.raw_after =
            orch_db::queries::raw_events::last_number(&mut tx, snapshot.segment.id).await?;
        tx.commit().await.map_err(orch_db::DbError::from)?;
        // These helpers can wait on another service; no metadata transaction remains open.
        let start = crate::ids::derived(
            "continuation.harness",
            &[recipe.operation_id.as_bytes(), lane.machine_id.as_bytes()],
        );
        if matches!(
            job.engine()
                .ensure_harness(snapshot.key, &snapshot.agent, start)
                .await?,
            crate::harnesses::Started::Failed(_)
        ) {
            return Err(failed(
                "agent_offline",
                "The retained conversation's agent program could not be started.",
            ));
        }
        body.mcp = job
            .engine()
            .mcp_for_session(lane.environment_id, recipe.thread_id)
            .await?;
        Ok(body)
    }

    async fn quiesce(job: &Job, recipe: &Recipe) -> Result<Done, Stop> {
        loop {
            job.check()?;
            crate::tools::source::reap(job.engine(), Some(recipe.thread_id)).await?;
            let (active, busy) = {
                let mut conn = job.engine().db().begin().await?;
                Self::current(job, &mut conn, recipe, false, true).await?;
                let active = threads::active_turn(&mut conn, recipe.thread_id).await?;
                let mut busy = threads::turn_end_work_pending(&mut conn, recipe.thread_id).await?
                    || crate::background::native_busy(&mut conn, recipe.thread_id).await?;
                busy |= sqlx::query_scalar::<_,bool>("SELECT EXISTS(SELECT 1 FROM thread_tool_commands WHERE thread_id=$1 AND NOT settled)")
                    .bind(recipe.thread_id).fetch_one(&mut *conn).await.map_err(orch_db::DbError::from)?;
                busy|=sqlx::query_scalar::<_,bool>("SELECT EXISTS(SELECT 1 FROM jobs WHERE node_id=$1 AND id<>$2
                    AND kind IN ('snapshot','delivery','workspace_prepare','workspace_restore','git_commit','git_push')
                    AND state IN ('queued','running')) OR EXISTS(SELECT 1 FROM commands WHERE state IN ('queued','sent','accepted')
                    AND (body->>'thread_id'=$1::text OR body#>>'{session,thread_id}'=$1::text))")
                    .bind(recipe.thread_id).bind(job.id()).fetch_one(&mut *conn).await.map_err(orch_db::DbError::from)?;
                conn.commit().await.map_err(orch_db::DbError::from)?;
                (active, busy)
            };
            if let Some(turn) = active {
                let policy = recipe
                    .request
                    .active_turn
                    .filter(|policy| policy.turn_id == turn.id)
                    .ok_or_else(|| {
                        failed(
                            "continuation.turn_changed",
                            "The running turn differs from the accepted wait or interrupt choice.",
                        )
                    })?;
                if policy.action == RunningAction::Interrupt {
                    job.engine()
                        .interrupt_for_change(recipe.thread_id, &turn, recipe.actor_id)
                        .await?;
                }
            } else if !busy {
                return Ok(Done::Finished);
            }
            if job.now()
                >= recipe.accepted_at.get() + chrono::Duration::minutes(recipe.wait_minutes)
            {
                return Err(failed(
                    "continuation.wait_timeout",
                    "The original native work has not finished within the thread's wait limit; its continuation and pending inputs are kept.",
                ));
            }
            job.progress(
                0.0,
                Some("Waiting for the original native turn, files and accounting to finish."),
            )
            .await?;
            job.pause(LOOK_EVERY).await?;
        }
    }

    async fn close(
        job: &Job,
        recipe: &Recipe,
        lane: &Lane,
        slot: &str,
        id: Id,
    ) -> Result<(), Stop> {
        Self::close_with_gate(job, recipe, lane, slot, id, slot == "close_active").await
    }

    async fn close_with_gate(
        job: &Job,
        recipe: &Recipe,
        lane: &Lane,
        slot: &str,
        id: Id,
        reducing_original: bool,
    ) -> Result<(), Stop> {
        let body = SessionContinuationClose {
            thread_id: recipe.thread_id,
            provider_id: lane.provider_id,
            agent_id: lane.agent_id,
            source: lane.source.clone(),
        };
        let command = Self::saved_with_gate(
            job,
            slot,
            id,
            lane.machine_id,
            &body,
            Some(recipe.deadline),
            reducing_original,
        )
        .await?;
        Self::command(job, command, false).await?;
        crate::engine::lock(&job.engine().inner.sessions).closed(lane.source.segment_id);
        Ok(())
    }

    async fn prepare(job: &Job, mut recipe: Recipe) -> Result<Done, Stop> {
        if job.plan().await?.get("prepared").is_some() {
            return Ok(Done::Finished);
        }
        if Self::reconcile_creation(job, &recipe).await?.is_some() {
            return Ok(Done::Finished);
        }
        let plan = job.plan().await?;
        if let Some(wire) = plan.get("wire").and_then(|wire| wire.get("open_prefix")) {
            let command: Outgoing = serde_json::from_value(wire.clone()).map_err(internal)?;
            if command.id != recipe.commands.open_prefix
                || command.machine != recipe.source_lane.machine_id
                || command.kind != orch_link_contract::commands::CommandKind::SessionPrefixOpen
            {
                return Err(failed(
                    "continuation.identity",
                    "The retained creation command changed identity.",
                ));
            }
            let body: SessionPrefixOpen =
                serde_json::from_value(command.body.clone()).map_err(internal)?;
            let command = Self::saved(
                job,
                "open_prefix",
                command.id,
                command.machine,
                &body,
                command.deadline,
            )
            .await?;
            let opened = serde_json::from_value(Self::command(job, command, false).await?)
                .map_err(internal)?;
            #[cfg(test)]
            tests::after_creation().await;
            Self::retain_prepared(job, &recipe, &Prepared::Prefix(opened)).await?;
            return Ok(Done::Finished);
        }
        // This checks candidate generation, binding and actual current actor rights
        // before either the selected or inactive source lane may be changed.
        let source_body = Self::body(
            job,
            &recipe,
            &recipe.source_lane,
            recipe.source_lane.source.segment_id,
            SessionMode::Resume,
            false,
        )
        .await?;
        // Verify the ordinary active source before closing it. Unsupported inputs/goals
        // leave its native runtime untouched; only an explicitly unobserved cold lane hydrates first.
        if recipe.needs_prefix
            && recipe
                .original_lane
                .as_ref()
                .is_some_and(|original| original == &recipe.source_lane)
        {
            let inspect = SessionPrefixInspect {
                thread_id: recipe.thread_id,
                provider_id: recipe.source_lane.provider_id,
                agent_id: recipe.source_lane.agent_id,
                source: recipe.source_lane.source.clone(),
                before_input_ref: recipe.source_turn_id.to_string(),
            };
            let command = Self::saved(
                job,
                "preinspect",
                recipe.commands.preinspect,
                recipe.source_lane.machine_id,
                &inspect,
                Some(recipe.deadline),
            )
            .await?;
            match Self::command(job, command, false).await {
                Ok(value) => {
                    let prefix = serde_json::from_value(value).map_err(internal)?;
                    if recipe.prefix.as_ref().is_some_and(|old| old != &prefix) {
                        return Err(failed(
                            "continuation.source_changed",
                            "The retained native prefix changed.",
                        ));
                    }
                    recipe.prefix = Some(prefix);
                }
                Err(Stop::Failed(failure)) if failure.code == "session.source_unobserved" => {}
                Err(error) => return Err(error),
            }
        }
        // The rollback obligation exists before the first native lane can be changed.
        let mut intent = job.engine().db().begin().await?;
        Self::current(job, &mut intent, &recipe, false, false).await?;
        jobs::merge_plan(
            &mut intent,
            job.id(),
            &json!({"native_changed":true}),
            job.now(),
        )
        .await?;
        intent.commit().await.map_err(orch_db::DbError::from)?;
        if let Some(original) = &recipe.original_lane {
            Self::close_with_gate(
                job,
                &recipe,
                original,
                "close_active",
                recipe.commands.close_active,
                false,
            )
            .await?;
        }
        let resume = SessionContinuationResume {
            thread_id: recipe.thread_id,
            session: Box::new(source_body),
            source: recipe.source_lane.source.clone(),
            replace_segment_id: None,
            replace_retained_source: None,
        };
        let command = Self::saved(
            job,
            "resume_source",
            recipe.commands.resume_source,
            recipe.source_lane.machine_id,
            &resume,
            Some(recipe.deadline),
        )
        .await?;
        let resumed: SessionOpenResult =
            serde_json::from_value(Self::command(job, command, false).await?).map_err(internal)?;
        if resumed.native_session_ref != recipe.source_lane.source.native_session_ref {
            return Err(failed(
                "continuation.identity",
                "The native program resumed another source conversation.",
            ));
        }
        let prepared = if recipe.needs_prefix {
            let inspect = SessionPrefixInspect {
                thread_id: recipe.thread_id,
                provider_id: recipe.source_lane.provider_id,
                agent_id: recipe.source_lane.agent_id,
                source: recipe.source_lane.source.clone(),
                before_input_ref: recipe.source_turn_id.to_string(),
            };
            let command = Self::saved(
                job,
                "inspect",
                recipe.commands.inspect,
                recipe.source_lane.machine_id,
                &inspect,
                Some(recipe.deadline),
            )
            .await?;
            let prefix: orch_agent_contract::history::NativeInputPrefix =
                serde_json::from_value(Self::command(job, command, false).await?)
                    .map_err(internal)?;
            if recipe.prefix.as_ref().is_some_and(|saved| saved != &prefix) {
                return Err(failed(
                    "continuation.source_changed",
                    "The previously inspected native prefix changed; its retained version is kept.",
                ));
            }
            recipe.prefix = Some(prefix);
            let mut tx = job.engine().db().begin().await?;
            Self::current(job, &mut tx, &recipe, false, false).await?;
            let prefix = serde_json::to_value(&recipe.prefix).map_err(internal)?;
            let saved = sqlx::query(
                "UPDATE thread_continuations SET native_prefix=$2,native_source=$3 WHERE id=$1
                AND (native_prefix IS NULL OR native_prefix=$2)",
            )
            .bind(recipe.target_continuation_id)
            .bind(&prefix)
            .bind(serde_json::to_value(&recipe.source_lane.source).map_err(internal)?)
            .execute(&mut *tx)
            .await
            .map_err(orch_db::DbError::from)?
            .rows_affected();
            if saved != 1 {
                return Err(failed(
                    "continuation.source_changed",
                    "The retained native cut has another fingerprint.",
                ));
            }
            jobs::merge_plan(&mut tx, job.id(), &json!({"recipe":recipe}), job.now()).await?;
            tx.commit().await.map_err(orch_db::DbError::from)?;
            Self::close(
                job,
                &recipe,
                &recipe.source_lane,
                "close_source",
                recipe.commands.close_source,
            )
            .await?;
            let mut body = Self::body(
                job,
                &recipe,
                &recipe.source_lane,
                recipe.segment_id,
                SessionMode::Fork,
                false,
            )
            .await?;
            body.native_session_ref = Some(recipe.source_lane.source.native_session_ref.clone());
            let open = SessionPrefixOpen {
                thread_id: recipe.thread_id,
                session: Box::new(body),
                prefix: recipe.prefix.clone().ok_or_else(|| {
                    failed(
                        "continuation.identity",
                        "The exact prefix receipt is missing.",
                    )
                })?,
            };
            let command = Self::saved(
                job,
                "open_prefix",
                recipe.commands.open_prefix,
                recipe.source_lane.machine_id,
                &open,
                Some(recipe.deadline),
            )
            .await?;
            let opened = serde_json::from_value(Self::command(job, command, false).await?)
                .map_err(internal)?;
            recipe
                .verify_opened(&opened)
                .map_err(|detail| failed("continuation.identity", detail))?;
            recipe.opened = Some(opened.clone());
            #[cfg(test)]
            tests::after_creation().await;
            Prepared::Prefix(opened)
        } else {
            // Install a new stream segment for this retained native identity, with no input submitted.
            Self::close(
                job,
                &recipe,
                &recipe.source_lane,
                "close_source",
                recipe.commands.close_source,
            )
            .await?;
            let body = Self::body(
                job,
                &recipe,
                &recipe.source_lane,
                recipe.segment_id,
                SessionMode::Resume,
                false,
            )
            .await?;
            let resume = SessionContinuationResume {
                thread_id: recipe.thread_id,
                session: Box::new(body),
                source: recipe.source_lane.source.clone(),
                replace_segment_id: None,
                replace_retained_source: None,
            };
            let command = Self::saved(
                job,
                "resume_target",
                recipe.commands.resume_target,
                recipe.source_lane.machine_id,
                &resume,
                Some(recipe.deadline),
            )
            .await?;
            let opened: SessionOpenResult =
                serde_json::from_value(Self::command(job, command, false).await?)
                    .map_err(internal)?;
            if opened.native_session_ref != recipe.source_lane.source.native_session_ref {
                return Err(failed(
                    "continuation.identity",
                    "The native program resumed another retained continuation.",
                ));
            }
            Prepared::Resumed(opened)
        };
        Self::retain_prepared(job, &recipe, &prepared).await?;
        Ok(Done::Finished)
    }

    async fn activate(job: &Job, recipe: &Recipe) -> Result<Done, Stop> {
        let mut tx = job.engine().db().begin().await?;
        let owned = job.lock_owned(&mut tx).await?.ok_or(Stop::Lost)?;
        let operation = rows::operation(&mut tx, job.id())
            .await?
            .ok_or_else(|| failed("not_found", "The continuation receipt was removed."))?;
        if operation.status == "committed" {
            tx.commit().await.map_err(orch_db::DbError::from)?;
            return Ok(Done::Finished);
        }
        let _thread = Self::current(job, &mut tx, recipe, false, false).await?;
        let prepared: Prepared =
            serde_json::from_value(owned.plan["prepared"].clone()).map_err(internal)?;
        if let Prepared::Prefix(opened) = &prepared {
            recipe
                .verify_opened(opened)
                .map_err(|detail| failed("continuation.identity", detail))?;
        }
        if !jobs::pass_commit(&mut tx, job.id(), job.now()).await? {
            return Err(Stop::Cancelled);
        }
        let mut unit = Unit::new(&mut tx, job.now());
        let seq = unit.stamp(recipe.thread_id).await?;
        threads::set_native_session_ref(unit.conn, recipe.segment_id, prepared.native(), seq)
            .await?;
        if recipe.request.kind == Kind::Activate {
            // Preserve a prior person's Stop; an inactive queue never starts implicitly on selection.
            sqlx::query("UPDATE thread_continuations c SET queue_held=true,
                queue_held_by=CASE WHEN c.queue_held THEN c.queue_held_by ELSE $2 END,
                queue_held_at=CASE WHEN c.queue_held THEN c.queue_held_at ELSE $3 END,queue_revision=queue_revision+1
                WHERE c.id=$1 AND (c.queue_held OR EXISTS(SELECT 1 FROM thread_inputs i WHERE i.continuation_id=c.id
                  AND i.state='pending' AND ($4::uuid IS NULL OR i.id<>$4)))")
                .bind(recipe.target_continuation_id).bind(recipe.actor_id).bind(job.now()).bind(recipe.input_id)
                .execute(&mut *unit.conn).await.map_err(orch_db::DbError::from)?;
            sqlx::query("UPDATE thread_inputs i SET queue_held=true,seq_updated=$2 FROM thread_continuations c
                WHERE c.id=$1 AND c.queue_held AND i.continuation_id=c.id AND i.state='pending'")
                .bind(recipe.target_continuation_id).bind(seq).execute(&mut *unit.conn).await.map_err(orch_db::DbError::from)?;
        }
        if !rows::activate(unit.conn, &operation, recipe.segment_id, seq, job.now()).await? {
            return Err(failed(
                "continuation.changed",
                "The selected execution or queue changed during native preparation; the original continuation is kept.",
            ));
        }
        if let Some(input) = recipe.input_id
            && let Some(input) = threads::input(unit.conn, input).await?
        {
            crate::notifications::follow_thread(
                unit.conn,
                recipe.actor_id,
                recipe.thread_id,
                job.now(),
            )
            .await?;
            let thread = threads::thread(unit.conn, recipe.thread_id)
                .await?
                .ok_or_else(|| failed("not_found", "The thread was removed."))?;
            crate::notifications::thread_mentions(unit.conn, &thread, &input, job.now()).await?;
        }
        unit.wake(recipe.thread_id);
        let finished = unit.finish().await?;
        job.engine().commit(tx, finished).await?;
        Ok(Done::Finished)
    }

    async fn restore(job: &Job, recipe: &Recipe) -> Result<(), Stop> {
        let plan = job.plan().await?;
        if plan.get("committed").and_then(Json::as_bool) == Some(true) {
            return Ok(());
        }
        let reconciliation = Self::reconcile_creation(job, recipe).await;
        if matches!(&reconciliation, Err(Stop::Lost)) {
            return Err(Stop::Lost);
        }
        if let Some(retirement) = lifecycle::retirement(&plan, recipe)? {
            Self::retire(job, recipe, &retirement, &plan).await?;
            if let Err(error) = reconciliation {
                let invalid = matches!(&error,Stop::Failed(failure) if matches!(failure.code.as_str(),
                    "continuation.native_result_invalid"|"continuation.identity"|"continuation.creation_unknown"));
                if !invalid {
                    return Err(error);
                }
            }
            return Self::fail_retained(job).await;
        }
        if plan.get("native_changed").and_then(Json::as_bool) == Some(true)
            && let Some(original) = &recipe.original_lane
        {
            let body = Self::body(
                job,
                recipe,
                original,
                original.source.segment_id,
                SessionMode::Resume,
                true,
            )
            .await?;
            let resume = SessionContinuationResume {
                thread_id: recipe.thread_id,
                session: Box::new(body),
                source: original.source.clone(),
                replace_segment_id: Some(recipe.segment_id),
                replace_retained_source: Some(recipe.source_lane.source.clone()),
            };
            let attempt = plan
                .get("restore_attempt")
                .and_then(Json::as_u64)
                .unwrap_or(0);
            let id = if attempt == 0 {
                recipe.commands.restore_active
            } else {
                crate::ids::derived(
                    "continuation.restore-retry",
                    &[
                        recipe.commands.restore_active.as_bytes(),
                        &attempt.to_be_bytes(),
                    ],
                )
            };
            // One current resume body is retained; terminal attempts live in the existing command journal.
            let command = Self::saved(
                job,
                "restore_active",
                id,
                original.machine_id,
                &resume,
                None,
            )
            .await?;
            let command_id = command.id;
            let outcome = Self::command(job, command, true).await;
            let terminal = if outcome.is_err() {
                job.engine()
                    .inner
                    .commands
                    .status(command_id)
                    .await
                    .map_err(internal)?
                    .and_then(|info| info.outcome)
                    .is_some_and(|outcome| {
                        matches!(
                            outcome,
                            crate::commands::Outcome::Failed(_)
                                | crate::commands::Outcome::Expired
                                | crate::commands::Outcome::Cancelled
                        )
                    })
            } else {
                false
            };
            if terminal {
                // Only a terminal resume failure admits another attempt. It always names the
                // same original native identity; prefix creation never uses this retry path.
                let next = attempt.checked_add(1).ok_or_else(|| {
                    failed(
                        "continuation.recovery_limit",
                        "The native recovery attempt counter is exhausted.",
                    )
                })?;
                job.remember(&json!({"restore_attempt":next,"restore_terminal":command_id}))
                    .await?;
            }
            let restored: SessionOpenResult = serde_json::from_value(outcome?).map_err(internal)?;
            if restored.native_session_ref != original.source.native_session_ref {
                return Err(failed(
                    "continuation.identity",
                    "The original native continuation could not be verified during recovery.",
                ));
            }
            // The next actual authored turn refreshes lease tickets/configuration through
            // normal SessionOpen; a preparation observer is not its paid-session setup.
            crate::engine::lock(&job.engine().inner.sessions).closed(original.source.segment_id);
        }
        if let Err(error) = reconciliation {
            // A damaged candidate cannot make valid original recovery unreachable. Its
            // durable creation identity remains unknown and therefore cannot fork again.
            // A storage/claim failure still retains this owned abort until its receipt flushes.
            let invalid = matches!(&error,Stop::Failed(failure) if matches!(failure.code.as_str(),
                "continuation.native_result_invalid"|"continuation.identity"|"continuation.creation_unknown"));
            if !invalid {
                return Err(error);
            }
        }
        Self::fail_retained(job).await
    }
}

fn body_thread(row: &jobs::JobRow) -> Result<Id, Stop> {
    row.node_id
        .ok_or_else(|| failed("not_found", "The continuation job has no thread."))
}

#[async_trait]
impl Phased for Continuing {
    async fn run(&self, job: &Job, phase: &str) -> Result<Done, Stop> {
        let recipe = Self::recipe(job).await?;
        match phase {
            "quiesce" => Self::quiesce(job, &recipe).await,
            "prepare" => Self::prepare(job, recipe).await,
            "activate" => Self::activate(job, &recipe).await,
            _ => Err(failed(
                "internal",
                "The continuation job has an unknown phase.",
            )),
        }
    }
    async fn undo(&self, job: &Job, _why: &Stop) -> Undone {
        match Self::recipe(job).await {
            Ok(recipe)=>match Self::restore(job,&recipe).await {Ok(())=>Undone::Everything,
                Err(Stop::Failed(failure))=>Undone::Deferred(format!("The original native conversation could not be confirmed ({}): {} Its pending inputs stay held while recovery retries that same session.",failure.code,failure.message)),
                Err(_)=>Undone::Deferred("Restoring the original native conversation; its pending inputs remain held until that exact session is confirmed.".into())},
            Err(_)=>Undone::Deferred("The continuation's retained recovery recipe cannot be read; the original execution remains held.".into()),
        }
    }
    async fn result(&self, job: &Job) -> Option<Json> {
        let recipe = Self::recipe(job).await.ok()?;
        Some(json!({"continuation_id":recipe.target_continuation_id,"input_id":recipe.input_id}))
    }
}

#[cfg(test)]
mod tests;
