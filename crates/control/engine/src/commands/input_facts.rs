//! Image delivery observations travel privately to the same transaction that admits their command.
use crate::EngineError;
use crate::unit::Unit;
use orch_api_contract::threads::NativeImageReceipt;
use orch_types::Id;

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct InputCommandFacts {
    pub thread: Id,
    pub input: Id,
    pub native_image: NativeImageReceipt,
}

/// Called only after a new native input command has passed its ordinary admission checks.
/// Replays and later settings never replace the first accepted observation.
pub async fn retain_input_facts(
    conn: &mut orch_db::Conn,
    facts: &InputCommandFacts,
    body: &orch_types::Json,
    now: chrono::DateTime<chrono::Utc>,
) -> Result<(), EngineError> {
    let body_thread = body
        .get("thread_id")
        .and_then(|value| value.as_str())
        .and_then(|value| value.parse::<Id>().ok());
    let turn = body
        .get("turn_id")
        .or_else(|| body.get("turn").and_then(|value| value.get("turn_id")))
        .and_then(|value| value.as_str())
        .and_then(|value| value.parse::<Id>().ok());
    let input = orch_db::queries::threads::input(conn, facts.input)
        .await?
        .ok_or_else(|| EngineError::unreadable("the admitted image input is absent"))?;
    if body_thread != Some(facts.thread)
        || input.thread_id != facts.thread
        || input.turn_id != turn
        || turn.is_none()
    {
        return Err(EngineError::unreadable(
            "the image observation belongs to another native input",
        ));
    }
    if input.native_image.is_some() {
        return Ok(());
    }
    let mut unit = Unit::new(conn, now);
    let seq = unit.stamp(facts.thread).await?;
    sqlx::query("UPDATE thread_inputs SET native_image=$2,seq_updated=$3 WHERE id=$1 AND native_image IS NULL")
        .bind(facts.input).bind(serde_json::to_value(&facts.native_image)?).bind(seq)
        .execute(&mut *unit.conn).await.map_err(orch_db::DbError::from)?;
    unit.finish().await?;
    Ok(())
}
