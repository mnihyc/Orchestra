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
    call: &CallContext,
    parent: &ParentLink,
) -> Result<Option<CapabilityResult>, EngineError> {
    let mut conn = engine.db().acquire().await?;
    source::check_call(&mut conn, call).await?;
    source::original_callers(
        &mut conn,
        call,
        engine.inner.clock.as_ref(),
        engine.inner.access.as_ref(),
    )
    .await?;
    let record = parent.record(&mut conn).await?;
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
        source::check_call(&mut conn, call).await?;
        drop(conn);
        thread_tool::publish(engine, call, parent, &record, &receipt).await?;
        return Ok(Some(receipt.result));
    }
    let error:Option<Json>=sqlx::query_scalar("SELECT payload FROM items WHERE thread_id=$1 AND turn_id=$2 AND kind='error' AND payload#>>'{details,capability}'=$3 ORDER BY seq_created,id LIMIT 1")
        .bind(parent.helper).bind(turn.id).bind(&record.capability).fetch_optional(&mut *conn).await.map_err(orch_db::DbError::from)?;
    if let Some(error) = error {
        drop(conn);
        let failure = CapabilityFailure::native(&record.capability, &error, record.helper_info())?;
        source::value(
            engine,
            &format!("delegated_failure:{}", record.attempt),
            failure.rule_failure(),
        )
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
        source::value(
            engine,
            &format!("delegated_failure:{}", record.attempt),
            failure.rule_failure(),
        )
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
    let mut owned = Owned {
        engine: engine.clone(),
        parent: parent.clone(),
        armed: true,
    };
    let deadline = {
        let mut conn = engine.db().acquire().await?;
        parent.record(&mut conn).await?.deadline
    };
    loop {
        match poll(engine, call, &parent).await {
            Ok(Some(result)) => {
                stop_owned(engine, &parent, true).await?;
                owned.armed = false;
                return Ok(result);
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
