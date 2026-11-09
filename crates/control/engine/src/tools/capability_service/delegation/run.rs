//! One ordinary helper per retained call-rule attempt; accepted native output is never generated twice.
use super::*;
use crate::turns::{InputAfter, TurnEnd, error_json};
use orch_db::queries::storage;

fn link(call: &CallContext) -> Result<ParentLink, EngineError> {
    let root = source::resource_id(
        "provider-service",
        crate::ids::tool_result(call.thread, &call.call_ref),
    );
    let attempt = recovery_binding::command(root)?;
    Ok(ParentLink {
        answer: source::Source::captured()?.answer(),
        helper: crate::ids::derived("capability.helper", &[attempt.as_bytes()]),
        slot: format!("{SLOT}:{attempt}"),
    })
}

/// Reducing cleanup targets only the initial turn this exact helper attempt owns.
async fn stop_owned(
    engine: &Engine,
    parent: &ParentLink,
    fulfilled: bool,
) -> Result<(), EngineError> {
    let mut tx = engine.db().begin().await?;
    let record = parent.record(&mut tx).await?;
    let Some(thread) = threads::lock(&mut tx, parent.helper).await? else {
        return Ok(());
    };
    let Some(input) = threads::input_by_client(
        &mut tx,
        parent.helper,
        Some(record.actor),
        &format!("capability-helper:{}", record.attempt),
    )
    .await?
    else {
        return Ok(());
    };
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

#[derive(sqlx::FromRow)]
struct Generated {
    native_ref: String,
    blob: String,
    media_type: String,
    revised_prompt: Option<String>,
}

async fn poll(
    engine: &Engine,
    call: &CallContext,
    parent: &ParentLink,
) -> Result<Option<Json>, EngineError> {
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
    let input = threads::input_by_client(
        &mut conn,
        parent.helper,
        Some(record.actor),
        &format!("capability-helper:{}", record.attempt),
    )
    .await?
    .ok_or(EngineError::StaleToolSource)?;
    let Some(turn) = input.turn_id else {
        let helper = threads::thread(&mut conn, parent.helper)
            .await?
            .ok_or(EngineError::StaleToolSource)?;
        if input.is_held()
            || helper.state == threads::thread_state::BLOCKED
            || input.state == threads::input_state::FAILED
            || input.state == threads::input_state::CANCELLED
        {
            return Err(EngineError::unreadable(
                "The ordinary helper could not start under its current source, provider or usage allowance.",
            ));
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
    let count = record
        .input
        .get("n")
        .and_then(Json::as_u64)
        .and_then(|n| usize::try_from(n).ok())
        .unwrap_or(1);
    let bound = i64::try_from(count.saturating_add(1)).map_err(EngineError::unreadable)?;
    let generated:Vec<Generated>=sqlx::query_as("SELECT native_ref,payload#>>'{tool,blob}' AS blob,payload#>>'{tool,media_type}' AS media_type,payload#>>'{tool,caption}' AS revised_prompt FROM items WHERE thread_id=$1 AND turn_id=$2 AND kind='tool_call' AND status='completed' AND payload#>>'{tool,kind}'='image' AND payload#>>'{tool,capability}'=$3 AND native_ref IS NOT NULL ORDER BY seq_created,id LIMIT $4")
        .bind(parent.helper).bind(turn.id).bind(&record.capability).bind(bound).fetch_all(&mut *conn).await.map_err(orch_db::DbError::from)?;
    if generated.len() >= count {
        let mut images = Vec::new();
        for image in generated {
            let blob: Hash = image.blob.parse().map_err(EngineError::unreadable)?;
            let kept = storage::blob(&mut conn, &blob)
                .await?
                .ok_or(EngineError::StaleToolSource)?;
            images.push(accepted::NativeImage {
                blob,
                size: kept.size,
                media_type: image.media_type,
                native_ref: image.native_ref,
                revised_prompt: image.revised_prompt,
            });
        }
        let command:Option<Id>=sqlx::query_scalar("SELECT id FROM commands WHERE serial_key=$1 AND machine_id=$2 AND kind IN ('turn.start','turn.start.fenced') AND state IN ('sent','accepted','succeeded','failed') AND COALESCE(body->>'turn_id',body#>>'{turn,turn_id}')=$3 AND COALESCE(body->>'thread_id',body#>>'{turn,thread_id}')=$4 ORDER BY created_at DESC,id DESC LIMIT 1")
            .bind(SerialKey::Thread(parent.helper).wire()).bind(lane.machine_id).bind(turn.id.to_string()).bind(parent.helper.to_string())
            .fetch_optional(&mut *conn).await.map_err(orch_db::DbError::from)?;
        let command = command.ok_or(EngineError::StaleToolSource)?;
        source::check_call(&mut conn, call).await?;
        let details = json!({"helper":record.helper_info(),"images":images});
        let accepted = accepted::capture_native(
            call,
            &record.capability,
            command,
            parent.clone(),
            &record.input,
            images,
            details,
        )?;
        drop(conn);
        engine
            .retain_native_capability_result(call, &accepted)
            .await?;
        return Ok(Some(Json::Null));
    }
    let error:Option<Json>=sqlx::query_scalar("SELECT payload FROM items WHERE thread_id=$1 AND turn_id=$2 AND kind='error' AND payload#>>'{details,capability}'=$3 ORDER BY seq_created,id LIMIT 1")
        .bind(parent.helper).bind(turn.id).bind(&record.capability).fetch_optional(&mut *conn).await.map_err(orch_db::DbError::from)?;
    if let Some(error) = error {
        drop(conn);
        let failed=error.get("details").and_then(|details|details.get("failure")).cloned()
            .unwrap_or_else(||json!({"code":"unavailable","message":"The native capability returned no successful result","retryable":false}));
        source::value(
            engine,
            &format!("delegated_failure:{}", record.attempt),
            failed,
        )
        .await?;
        return Err(EngineError::unreadable(
            error
                .get("message")
                .and_then(Json::as_str)
                .unwrap_or("The native capability failed."),
        ));
    }
    if !turn.is_active() {
        return Err(EngineError::unreadable(
            "The helper finished without a proven native capability result.",
        ));
    }
    Ok(None)
}

pub(crate) async fn execute(
    engine: &Engine,
    call: &CallContext,
    capability: &str,
) -> Result<Json, EngineError> {
    let input = accepted::input(call, capability)?;
    let parent = link(call)?;
    let helper = engine
        .start_delegated_capability_helper(call, capability, input)
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
