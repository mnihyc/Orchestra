//! One ordinary helper per retained call-rule attempt; accepted native output is never generated twice.
use super::*;
use crate::turns::{InputAfter, TurnEnd, error_json};
use orch_provider_contract::capabilities::CapabilityResult;

fn link(call: &CallContext) -> Result<ParentLink, EngineError> {
    let root = source::resource_id(
        "provider-service",
        crate::ids::tool_result(call.thread, &call.call_ref),
    );
    let attempt = recovery_binding::command(root)?;
    Ok(ParentLink {
        work: crate::ids::derived("capability.work", &[attempt.as_bytes()]),
        helper: crate::ids::derived("capability.helper", &[attempt.as_bytes()]),
    })
}

/// Reducing cleanup targets only the initial turn this exact helper attempt owns.
async fn stop_owned(
    engine: &Engine,
    parent: &ParentLink,
    fulfilled: bool,
) -> Result<(), EngineError> {
    stop_owned_turn(engine, parent, fulfilled).await?;
    close_unit_session(engine, parent).await
}

async fn close_unit_session(engine: &Engine, parent: &ParentLink) -> Result<(), EngineError> {
    let mut tx = engine.db().begin().await?;
    let admitted:bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM delegated_program_work WHERE id=$1 AND helper_id=$2 AND unit_admission IS NOT NULL)")
        .bind(parent.work).bind(parent.helper).fetch_one(&mut *tx).await.map_err(orch_db::DbError::from)?;
    if !admitted {
        return Ok(());
    }
    let Some(thread) = threads::lock(&mut tx, parent.helper).await? else {
        return Ok(());
    };
    let Some(work) = capability_work::program(&mut tx, parent.work).await? else {
        return Ok(());
    };
    let Some(input) = work.input else {
        return Ok(());
    };
    let Some(input) = threads::input(&mut tx, input).await? else {
        return Ok(());
    };
    let Some(turn) = input.turn_id else {
        return Ok(());
    };
    let Some(turn) = threads::turn(&mut tx, turn).await? else {
        return Ok(());
    };
    if turn.thread_id != parent.helper
        || turn.input_id != input.id
        || thread.current_segment_id != Some(turn.segment_id)
        || thread
            .current_turn_id
            .is_some_and(|current| current != turn.id)
    {
        return Ok(());
    }
    let Some(segment) = threads::segment(&mut tx, turn.segment_id).await? else {
        return Ok(());
    };
    let Some(machine) = segment.machine_id else {
        return Ok(());
    };
    let command = crate::ids::derived(
        "capability.unit.session.close",
        &[parent.work.as_bytes(), turn.segment_id.as_bytes()],
    );
    let outgoing = crate::commands::session_close(
        &mut tx,
        command,
        machine,
        parent.helper,
        Some(turn.segment_id),
    )
    .await?;
    crate::archive::persist(&mut tx, engine.inner.clock.now(), outgoing).await?;
    capability_work::retain_idle_close(&mut tx, parent.work, command).await?;
    tx.commit().await.map_err(orch_db::DbError::from)?;
    engine.inner.signal.notify_one();
    Ok(())
}

async fn stop_owned_turn(
    engine: &Engine,
    parent: &ParentLink,
    fulfilled: bool,
) -> Result<(), EngineError> {
    let mut tx = engine.db().begin().await?;
    let Some(work) = capability_work::program(&mut tx, parent.work).await? else {
        return Ok(());
    };
    let Some(thread) = threads::lock(&mut tx, parent.helper).await? else {
        return Ok(());
    };
    let Some(input_id) = work.input else {
        return Ok(());
    };
    let Some(input) = threads::input(&mut tx, input_id).await? else {
        return Ok(());
    };
    if input.thread_id != parent.helper || input.author_id != Some(work.actor) {
        return Ok(());
    }
    let Some(turn) = input.turn_id else {
        return Ok(());
    };
    let Some(turn) = threads::turn(&mut tx, turn).await? else {
        return Ok(());
    };
    if turn.input_id != input.id
        || turn.thread_id != parent.helper
        || thread.current_turn_id != Some(turn.id)
        || !turn.is_active()
    {
        return Ok(());
    }
    let mut unit = crate::unit::Unit::new(&mut tx, engine.inner.clock.now());
    let message = if fulfilled {
        "The delegated capability returned its requested result."
    } else {
        "The requesting capability call ended; its owned helper was stopped."
    };
    engine
        .end_turn(
            &mut unit,
            &thread,
            &turn,
            TurnEnd {
                state: if fulfilled {
                    threads::turn_state::COMPLETED
                } else {
                    threads::turn_state::INTERRUPTED
                },
                error: (!fulfilled).then(|| error_json("capability_source_ended", message, false)),
                input: InputAfter::Keep,
                then_block: None,
            },
        )
        .await?;
    let machine = engine.harness_machine_of(unit.conn, parent.helper).await?;
    let finished = unit.finish().await?;
    engine.commit(tx, finished).await?;
    if let Some(machine) = machine {
        engine
            .send_interrupt_best_effort(machine, parent.helper, turn.id)
            .await;
    }
    Ok(())
}

struct Owned {
    engine: Engine,
    parent: ParentLink,
    armed: bool,
}
impl Drop for Owned {
    fn drop(&mut self) {
        if self.armed
            && !self.engine.is_stopped()
            && let Ok(runtime) = tokio::runtime::Handle::try_current()
        {
            let engine = self.engine.clone();
            let parent = self.parent.clone();
            runtime.spawn(async move {
                if let Err(error) = stop_owned(&engine, &parent, false).await {
                    tracing::debug!(%error,"The owned capability helper could not yet be stopped.");
                }
            });
        }
    }
}

async fn poll(
    engine: &Engine,
    parent: &ParentLink,
) -> Result<Option<(Record, codecs::Receipt)>, EngineError> {
    let mut conn = engine.db().acquire().await?;
    let record = parent.record(&mut conn).await?;
    record.origin.before_read(engine, &mut conn).await?;
    if !record.placement.current(&mut conn).await? {
        return Err(EngineError::StaleToolSource);
    }
    let input = threads::input_by_client(
        &mut conn,
        parent.helper,
        Some(record.actor),
        &format!("capability-helper:{}", record.attempt),
    )
    .await?
    .ok_or(EngineError::StaleToolSource)?;
    if !parent.owns_input(&mut conn, input.id).await? {
        return Err(EngineError::StaleToolSource);
    }
    let Some(turn) = input.turn_id else {
        let helper = threads::thread(&mut conn, parent.helper)
            .await?
            .ok_or(EngineError::StaleToolSource)?;
        if input.is_held()
            || helper.state == threads::thread_state::BLOCKED
            || input.state == threads::input_state::FAILED
            || input.state == threads::input_state::CANCELLED
        {
            return Err(CapabilityFailure::unavailable(
                &record.capability,
                "helper_start_refused",
                "The helper could not start under its current source, sign-in or usage allowance.",
            )
            .with_helper(record.helper_info())
            .into());
        }
        return Ok(None);
    };
    let turn = threads::turn(&mut conn, turn)
        .await?
        .ok_or(EngineError::StaleToolSource)?;
    let lane = threads::segment(&mut conn, turn.segment_id)
        .await?
        .ok_or(EngineError::StaleToolSource)?;
    let foreign: bool = sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM thread_inputs WHERE turn_id=$1 AND id<>$2)",
    )
    .bind(turn.id)
    .bind(input.id)
    .fetch_one(&mut *conn)
    .await
    .map_err(orch_db::DbError::from)?;
    if foreign
        || turn.thread_id != parent.helper
        || turn.input_id != input.id
        || turn.model != record.model
        || turn.effort != record.effort
        || lane.provider_id != record.provider
        || lane.agent_id != record.agent
        || lane.credential_path != "native"
    {
        return Err(EngineError::StaleToolSource);
    }
    let codec = codecs::installed()
        .map_err(EngineError::unreadable)?
        .get(&record.execution)?;
    let frame = codecs::Frame {
        helper: parent.helper,
        turn: turn.id,
        machine: lane.machine_id,
        capability: &record.capability,
        input: &record.input,
    };
    if let Some(receipt) = codec.read(&mut conn, &frame).await? {
        record.origin.before_read(engine, &mut conn).await?;
        return Ok(Some((record, receipt)));
    }
    let error:Option<Json>=sqlx::query_scalar("SELECT payload FROM items WHERE thread_id=$1 AND turn_id=$2 AND kind='error' AND payload#>>'{details,capability}'=$3 ORDER BY seq_created,id LIMIT 1")
        .bind(parent.helper).bind(turn.id).bind(&record.capability).fetch_optional(&mut *conn).await.map_err(orch_db::DbError::from)?;
    if let Some(error) = error {
        drop(conn);
        let failure = CapabilityFailure::native(&record.capability, &error, record.helper_info())?;
        record
            .origin
            .retain_failure(engine, record.attempt, failure.rule_failure())
            .await?;
        return Err(failure.into());
    }
    if !turn.is_active() {
        let failure = if let Some(error) = &turn.error {
            CapabilityFailure::native(&record.capability, error, record.helper_info())?
        } else {
            CapabilityFailure::unavailable(
                &record.capability,
                "native_result_unproven",
                "The helper finished without a proven native capability result.",
            )
            .with_helper(record.helper_info())
        };
        record
            .origin
            .retain_failure(engine, record.attempt, failure.rule_failure())
            .await?;
        return Err(failure.into());
    }
    Ok(None)
}

pub(crate) async fn execute(
    engine: &Engine,
    call: &CallContext,
    capability: &str,
    selected: selection::Selection,
    required: &ExecutionSupport,
) -> Result<CapabilityResult, EngineError> {
    let input = accepted::input(call, capability)?;
    let parent = link(call)?;
    let helper = engine
        .start_delegated_capability_helper(call, capability, input, selected, required)
        .await?;
    if helper != parent.helper {
        return Err(EngineError::ToolCallIdentity);
    }
    await_result(engine, &parent).await
}

pub(super) async fn await_result(
    engine: &Engine,
    parent: &ParentLink,
) -> Result<CapabilityResult, EngineError> {
    let mut owned = Owned {
        engine: engine.clone(),
        parent: parent.clone(),
        armed: true,
    };
    #[cfg(test)]
    source::tests::pause_program_wait(parent.helper).await;
    let deadline = {
        let mut conn = engine.db().acquire().await?;
        parent.record(&mut conn).await?.deadline
    };
    loop {
        if engine.is_stopped() {
            owned.armed = false;
            return Err(crate::commands::CommandsError::Stopping.into());
        }
        match poll(engine, parent).await {
            Ok(Some((record, receipt))) => {
                record
                    .origin
                    .publish(engine, parent, &record, &receipt)
                    .await?;
                stop_owned(engine, parent, true).await?;
                owned.armed = false;
                return Ok(receipt.result);
            }
            Ok(None) => {}
            Err(error) => return Err(error),
        }
        if engine.inner.clock.time() >= deadline {
            return Err(EngineError::StaleToolSource);
        }
        let remaining = deadline
            .get()
            .signed_duration_since(engine.inner.clock.now())
            .to_std()
            .map_err(EngineError::unreadable)?;
        tokio::select! {
            ()=engine.inner.signal.notified()=>{},
            ()=tokio::time::sleep(remaining.min(engine.inner.config.sweep_every))=>{},
        }
    }
}

/// Re-entry can deliver an already paid answer without admitting a new model effect.
pub(super) async fn completed_result(
    engine: &Engine,
    parent: &ParentLink,
) -> Result<Option<CapabilityResult>, EngineError> {
    let Some((record, receipt)) = poll(engine, parent).await? else {
        return Ok(None);
    };
    record
        .origin
        .publish(engine, parent, &record, &receipt)
        .await?;
    stop_owned(engine, parent, true).await?;
    Ok(Some(receipt.result))
}

/// This reads actual completed native output only. It neither starts work nor checks a fresh paid allowance.
pub(super) async fn reconcile(
    engine: &Engine,
    call: &CallContext,
    capability: &str,
) -> Result<Option<accepted::AcceptedCapabilityResult>, EngineError> {
    let works = {
        let mut conn = engine.db().acquire().await?;
        source::check_call(&mut conn, call).await?;
        capability_work::programs_for_tool(&mut conn, source::Source::captured()?.answer()).await?
    };
    for work in works {
        if work.purpose != capability {
            return Err(EngineError::ToolCallIdentity);
        }
        let parent = ParentLink {
            work: work.id,
            helper: work.helper,
        };
        {
            let mut conn = engine.db().acquire().await?;
            let record = parent.record(&mut conn).await?;
            if !record.origin.matches_call(call)? {
                return Err(EngineError::ToolCallIdentity);
            }
        }
        let (record, receipt) = match poll(engine, &parent).await {
            Ok(Some(ready)) => ready,
            Ok(None) | Err(EngineError::Capability(_)) => continue,
            Err(error) => return Err(error),
        };
        let result = accepted::reconcile_native_result(
            engine,
            call,
            capability,
            record.input.clone(),
            record.material.clone(),
            record.origin.publish(engine, &parent, &record, &receipt),
        )
        .await?;
        stop_owned(engine, &parent, true).await?;
        return Ok(Some(result));
    }
    Ok(None)
}

/// The same call-rule recovery row consumes the actual native failure and reset, without a fake command.
pub(crate) async fn failure(
    conn: &mut Conn,
    call: &CallContext,
    attempt: Id,
) -> Result<Option<Json>, EngineError> {
    source::check_call(conn, call).await?;
    Ok(sqlx::query_scalar::<_, Option<Json>>(
        "SELECT plan->$2 FROM thread_tool_calls WHERE answer_id=$1",
    )
    .bind(crate::ids::tool_result(call.thread, &call.call_ref))
    .bind(format!("delegated_failure:{attempt}"))
    .fetch_optional(conn)
    .await
    .map_err(orch_db::DbError::from)?
    .flatten())
}
