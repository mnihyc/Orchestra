//! A lifecycle ending reduces only this operation's native lanes; it never resumes work.

use super::*;
use crate::commands::{Outcome, Outgoing};
use orch_db::queries::threads::ThreadRow;
use orch_link_contract::commands::{CommandBody, CommandKind};
use orch_link_contract::continuations::SessionCloseFenced;
use orch_types::Time;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

const RETIREMENT: &str = "continuation_retirement";
const CLOSES: &str = "continuation_retirement_closes";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum Reason {
    Archived,
    EnvironmentRemoved,
    Disabled,
    Closed,
}

/// Captured in the already-authorized lifecycle transaction, before its cancellation commits.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(super) struct Retirement {
    thread: Id,
    selected: Id,
    execution_revision: i64,
    reason: Reason,
    at: Time,
}

impl Retirement {
    fn validates(&self, recipe: &Recipe) -> bool {
        self.thread == recipe.thread_id
            && self.selected == recipe.request.expected.active_continuation_id
            && self.execution_revision == recipe.request.expected.execution_revision
    }
}

pub(super) fn retirement(plan: &Json, recipe: &Recipe) -> Result<Option<Retirement>, Stop> {
    let value = plan
        .get(RETIREMENT)
        .cloned()
        .map(serde_json::from_value::<Retirement>)
        .transpose()
        .map_err(internal)?;
    if value.as_ref().is_some_and(|value| !value.validates(recipe)) {
        return Err(failed(
            "continuation.identity",
            "The lifecycle ending names another execution selection.",
        ));
    }
    Ok(value)
}

/// This is housekeeping of an already authorized ending, not a new editor/provider grant.
/// Use the existing cancellation write's lock order and never overwrite the first ending.
pub(crate) async fn supersede_in(
    conn: &mut orch_db::Conn,
    job: Id,
    thread: &ThreadRow,
    reason: &str,
    by: Option<Id>,
    at: chrono::DateTime<chrono::Utc>,
) -> Result<bool, EngineError> {
    let reason = match reason {
        "archived" => Reason::Archived,
        "environment_removed" => Reason::EnvironmentRemoved,
        "disabled" => Reason::Disabled,
        "closed" => Reason::Closed,
        _ => {
            return Err(EngineError::unreadable(
                "The continuation ending has no declared lifecycle reason.",
            ));
        }
    };
    let marker = serde_json::to_value(Retirement {
        thread: thread.node_id,
        selected: thread.active_continuation_id,
        execution_revision: thread.execution_revision,
        reason,
        at: Time::new(at),
    })?;
    Ok(sqlx::query("UPDATE jobs SET plan=jsonb_set(
        CASE WHEN plan ? 'cancel' THEN plan ELSE jsonb_set(plan,'{cancel}',
          jsonb_build_object('by',$3::text,'at',to_jsonb($4::timestamptz))) END,
        '{continuation_retirement}',COALESCE(plan->'continuation_retirement',$5::jsonb)),updated_at=$4
        WHERE id=$1 AND node_id=$2 AND kind='thread_continuation' AND state IN ('queued','running')
          AND NOT (plan ? 'committed')")
        .bind(job).bind(thread.node_id).bind(by).bind(at).bind(marker)
        .execute(conn).await.map_err(orch_db::DbError::from)?.rows_affected()==1)
}

/// At most the candidate, temporary source and original lane have a reducing receipt.
/// A terminal refusal may admit a new close attempt; an uncertain one keeps its exact body/budget.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
struct CloseReceipt {
    command: Outgoing,
    attempt: u64,
    finished: bool,
    previous_terminal: Option<Id>,
}

fn close_id(recipe: &Recipe, segment: Id, attempt: u64) -> Id {
    crate::ids::derived(
        "continuation.lifecycle-close",
        &[
            recipe.operation_id.as_bytes(),
            segment.as_bytes(),
            &attempt.to_be_bytes(),
        ],
    )
}

fn new_close(
    job: &Job,
    recipe: &Recipe,
    segment: Id,
    machine: Id,
    attempt: u64,
    previous_terminal: Option<Id>,
) -> Result<CloseReceipt, Stop> {
    let deadline = job.now()
        + chrono::Duration::seconds(i64::from(CommandKind::SessionCloseFenced.deadline_secs()));
    let command = Outgoing::new(
        close_id(recipe, segment, attempt),
        machine,
        SerialKey::Thread(recipe.thread_id),
        &SessionCloseFenced {
            thread_id: recipe.thread_id,
            segment_id: segment,
        },
    )
    .map_err(internal)?
    .by(Time::new(deadline));
    Ok(CloseReceipt {
        command,
        attempt,
        finished: false,
        previous_terminal,
    })
}

/// Freezing a body and its durable queue row is one transaction. The dispatcher
/// may adopt it later, but may never recreate an admitted terminal command.
pub(super) async fn retain_command(
    conn: &mut orch_db::Conn,
    mut command: Outgoing,
    at: chrono::DateTime<chrono::Utc>,
    fresh: bool,
) -> Result<Outgoing, Stop> {
    let recorded = orch_db::queries::link::command(conn, command.id).await?;
    let deadline = match (command.deadline, recorded.as_ref()) {
        (Some(deadline), _) => deadline,
        (None, Some(row)) => Time::new(row.deadline_at),
        (None, None) if fresh => {
            Time::new(at + chrono::Duration::seconds(i64::from(command.kind.deadline_secs())))
        }
        (None, None) => {
            return Err(failed(
                "continuation.native_unconfirmed",
                "The older saved command has no retained original deadline; another admission is not guessed.",
            ));
        }
    };
    command.deadline = Some(deadline);
    let key = command.key.wire();
    orch_db::queries::link::insert_command(
        conn,
        &orch_db::queries::link::NewCommand {
            id: command.id,
            machine_id: command.machine,
            kind: command.kind.as_str(),
            body: &command.body,
            serial_key: key.as_deref(),
            deadline_at: deadline.get(),
        },
        at,
    )
    .await?;
    let row = orch_db::queries::link::command(conn, command.id)
        .await?
        .ok_or_else(|| {
            failed(
                "continuation.native_unconfirmed",
                "The frozen native command is not retained.",
            )
        })?;
    if row.machine_id != command.machine
        || row.kind != command.kind.as_str()
        || row.body != command.body
        || row.serial_key() != key.as_deref()
        || Time::new(row.deadline_at) != deadline
    {
        return Err(failed(
            "continuation.identity",
            "The durable native command differs from its admitted body or budget.",
        ));
    }
    Ok(command)
}

/// A command supplier's genuine terminal outcome is factual even if this owner
/// was cancelled. The API mirror does the same monotonic write; narrow test
/// suppliers and a lost metadata checkpoint must not leave an admitted row queued.
pub(super) async fn mirror_command(job: &Job, command: &Outgoing) -> Result<(), Stop> {
    let Some(outcome) = job
        .engine()
        .inner
        .commands
        .status(command.id)
        .await
        .map_err(internal)?
        .and_then(|status| status.outcome)
    else {
        return Ok(());
    };
    let mut conn = job
        .engine()
        .db()
        .acquire()
        .await
        .map_err(EngineError::from)?;
    let row = orch_db::queries::link::command(&mut conn, command.id)
        .await?
        .ok_or_else(|| {
            failed(
                "continuation.native_unconfirmed",
                "The admitted command's durable receipt is unavailable.",
            )
        })?;
    let key = command.key.wire();
    if row.machine_id != command.machine
        || row.kind != command.kind.as_str()
        || row.body != command.body
        || row.serial_key() != key.as_deref()
        || command
            .deadline
            .is_some_and(|deadline| Time::new(row.deadline_at) != deadline)
    {
        return Err(failed(
            "continuation.identity",
            "The native outcome names another admitted command.",
        ));
    }
    let error;
    let ending = match &outcome {
        Outcome::Succeeded(value) => orch_db::queries::link::Ending::Succeeded(value),
        Outcome::Failed(value) => {
            error = serde_json::to_value(value).map_err(internal)?;
            orch_db::queries::link::Ending::Failed(&error)
        }
        Outcome::Cancelled => orch_db::queries::link::Ending::Cancelled,
        Outcome::Expired => orch_db::queries::link::Ending::Expired,
    };
    orch_db::queries::link::finish_command(&mut conn, command.id, &ending, job.now()).await?;
    Ok(())
}

async fn close_receipt(
    job: &Job,
    recipe: &Recipe,
    marker: &Retirement,
    segment: Id,
    machine: Id,
) -> Result<CloseReceipt, Stop> {
    let mut tx = job.engine().db().begin().await?;
    let owned = job.lock_owned(&mut tx).await?.ok_or(Stop::Lost)?;
    if retirement(&owned.plan, recipe)?.as_ref() != Some(marker) {
        return Err(Stop::Lost);
    }
    let mut receipts: BTreeMap<Id, CloseReceipt> = owned
        .plan
        .get(CLOSES)
        .cloned()
        .map(serde_json::from_value)
        .transpose()
        .map_err(internal)?
        .unwrap_or_default();
    let mut receipt = match receipts.get(&segment) {
        Some(receipt) => receipt.clone(),
        None => new_close(job, recipe, segment, machine, 0, None)?,
    };
    let body = serde_json::to_value(SessionCloseFenced {
        thread_id: recipe.thread_id,
        segment_id: segment,
    })
    .map_err(internal)?;
    if receipt.command.id != close_id(recipe, segment, receipt.attempt)
        || receipt.command.machine != machine
        || receipt.command.key != SerialKey::Thread(recipe.thread_id)
        || receipt.command.kind != SessionCloseFenced::KIND
        || receipt.command.body != body
        || receipt.command.deadline.is_none()
    {
        return Err(failed(
            "continuation.identity",
            "The reducing receipt names another native lane.",
        ));
    }
    receipt.command = retain_command(&mut tx, receipt.command, job.now(), false).await?;
    receipts.insert(segment, receipt.clone());
    if receipts.len() > 3 {
        return Err(failed(
            "continuation.identity",
            "The lifecycle ending contains unrelated native lanes.",
        ));
    }
    jobs::merge_plan(
        &mut tx,
        job.id(),
        &json!({"continuation_retirement_closes":receipts}),
        job.now(),
    )
    .await?;
    tx.commit().await.map_err(orch_db::DbError::from)?;
    Ok(receipt)
}

async fn checkpoint(
    job: &Job,
    recipe: &Recipe,
    marker: &Retirement,
    segment: Id,
    receipt: &CloseReceipt,
    finished: bool,
) -> Result<(), Stop> {
    let mut tx = job.engine().db().begin().await?;
    let owned = job.lock_owned(&mut tx).await?.ok_or(Stop::Lost)?;
    if retirement(&owned.plan, recipe)?.as_ref() != Some(marker) {
        return Err(Stop::Lost);
    }
    let mut receipts: BTreeMap<Id, CloseReceipt> =
        serde_json::from_value(owned.plan[CLOSES].clone()).map_err(internal)?;
    if receipts.get(&segment) != Some(receipt) {
        return Err(Stop::Lost);
    }
    let next = if finished {
        CloseReceipt {
            finished: true,
            ..receipt.clone()
        }
    } else {
        let attempt = receipt.attempt.checked_add(1).ok_or_else(|| {
            failed(
                "continuation.recovery_limit",
                "The reducing receipt counter is exhausted.",
            )
        })?;
        new_close(
            job,
            recipe,
            segment,
            receipt.command.machine,
            attempt,
            Some(receipt.command.id),
        )?
    };
    receipts.insert(segment, next);
    jobs::merge_plan(
        &mut tx,
        job.id(),
        &json!({"continuation_retirement_closes":receipts}),
        job.now(),
    )
    .await?;
    tx.commit().await.map_err(orch_db::DbError::from)?;
    Ok(())
}

impl Continuing {
    pub(super) async fn retire(
        job: &Job,
        recipe: &Recipe,
        marker: &Retirement,
        plan: &Json,
    ) -> Result<(), Stop> {
        if !marker.validates(recipe) {
            return Err(Stop::Lost);
        }
        if plan.get("native_changed").and_then(Json::as_bool) != Some(true) {
            // No replacing native command can precede this durable intent. The ordinary
            // lifecycle transaction already owns the current lane's reducing control.
            return Ok(());
        }
        let mut lanes = vec![
            (recipe.segment_id, recipe.source_lane.machine_id),
            (
                recipe.source_lane.source.segment_id,
                recipe.source_lane.machine_id,
            ),
        ];
        if let Some(original) = &recipe.original_lane {
            lanes.push((original.source.segment_id, original.machine_id));
        }
        let mut seen = BTreeMap::new();
        for (segment, machine) in lanes {
            if let Some(previous) = seen.insert(segment, machine) {
                if previous != machine {
                    return Err(failed(
                        "continuation.identity",
                        "One captured native segment names different machines.",
                    ));
                }
                continue;
            }
            let receipt = close_receipt(job, recipe, marker, segment, machine).await?;
            if receipt.finished {
                continue;
            }
            let outcome = Self::command(job, receipt.command.clone(), true).await;
            // An exact-segment mismatch is an actual native queue result: this owned
            // segment is no longer the live slot. Never close the unrelated replacement.
            let actual = job
                .engine()
                .inner
                .commands
                .status(receipt.command.id)
                .await
                .map_err(internal)?
                .and_then(|status| status.outcome);
            let gone = matches!(&actual, Some(Outcome::Succeeded(_)))
                || matches!(&actual,Some(Outcome::Failed(error)) if error.code=="session.segment_changed");
            if gone {
                checkpoint(job, recipe, marker, segment, &receipt, true).await?;
                crate::engine::lock(&job.engine().inner.sessions).closed(segment);
            } else {
                if actual.is_some() {
                    checkpoint(job, recipe, marker, segment, &receipt, false).await?;
                }
                return Err(outcome.err().unwrap_or_else(||failed("continuation.native_unconfirmed",
                    "The reducing native close has no retained terminal outcome; the exact operation is kept.")));
            }
        }
        Ok(())
    }

    pub(super) async fn fail_retained(job: &Job) -> Result<(), Stop> {
        let mut tx = job.engine().db().begin().await?;
        let owned = job.lock_owned(&mut tx).await?.ok_or(Stop::Lost)?;
        rows::mark_failed_version(&mut tx, job.id()).await?;
        rows::fail(
            &mut tx,
            job.id(),
            matches!(
                super::super::runner::abort_reason(&owned)?,
                Some(Stop::Cancelled)
            ),
        )
        .await?;
        tx.commit().await.map_err(orch_db::DbError::from)?;
        Ok(())
    }
}
