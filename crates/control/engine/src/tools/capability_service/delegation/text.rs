//! Native text is factual output of a successful owned turn, not evidence of another result family.
use super::codecs::{Codec, Frame, Receipt};
use super::*;
use async_trait::async_trait;
use orch_provider_contract::capabilities::{CapabilityItem, CapabilityResult};

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct CompletedText {
    pub text: String,
    pub native_ref: String,
    pub command: Id,
}

/// The source-bound runtime supplies ownership and current authority before reading these facts.
/// A message completing alone cannot turn a failed or unfinished native turn into an answer.
pub(crate) async fn completed(
    conn: &mut Conn,
    helper: Id,
    turn: Id,
    machine: Option<Id>,
) -> Result<Option<CompletedText>, EngineError> {
    let Some(machine) = machine else {
        return Ok(None);
    };
    let successful: bool = sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM turns t JOIN segments s ON s.id=t.segment_id AND s.thread_id=t.thread_id
         WHERE t.id=$1 AND t.thread_id=$2 AND t.state='completed' AND t.completed_at IS NOT NULL
           AND t.error IS NULL AND s.credential_path='native' AND s.machine_id=$3)",
    ).bind(turn).bind(helper).bind(machine).fetch_one(&mut *conn).await.map_err(orch_db::DbError::from)?;
    if !successful {
        return Ok(None);
    }
    let message: Option<(String, String)> = sqlx::query_as(
        "WITH messages AS (
           SELECT native_ref,payload->>'text' AS text,payload->>'phase' AS phase,seq_created,id
           FROM items WHERE thread_id=$1 AND turn_id=$2 AND kind='agent_message' AND status='completed'
             AND stream='main' AND parent_item_id IS NULL AND native_ref IS NOT NULL
             AND jsonb_typeof(payload->'text')='string'
         ) SELECT native_ref,text FROM messages
           WHERE phase='final' OR (phase IS NULL AND NOT EXISTS(SELECT 1 FROM messages WHERE phase IS NOT NULL))
           ORDER BY seq_created DESC,id DESC LIMIT 1",
    ).bind(helper).bind(turn).fetch_optional(&mut *conn).await.map_err(orch_db::DbError::from)?;
    let Some((native_ref, text)) =
        message.filter(|(reference, text)| !reference.is_empty() && !text.trim().is_empty())
    else {
        return Ok(None);
    };
    Ok(Some(CompletedText {
        text,
        native_ref,
        command: super::codecs::command(conn, helper, turn, Some(machine)).await?,
    }))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Request {
    system: String,
    input: String,
    max_output_tokens: i64,
}

pub(super) struct Text;

#[async_trait]
impl Codec for Text {
    fn support(&self) -> ExecutionSupport {
        // Confined routes stay unavailable until their source-bound native context enforces it.
        ExecutionSupport {
            input: "summary.request".into(),
            result: "summary.result".into(),
            ..Default::default()
        }
    }
    fn prepare(
        &self,
        capability: &str,
        input: &Json,
        media: &[orch_model_contract::generation::GenerationPart],
    ) -> Result<Vec<ContentPart>, EngineError> {
        let request: Request = serde_json::from_value(input.clone())?;
        if capability != "summary" || !media.is_empty() || request.max_output_tokens <= 0 {
            return Err(EngineError::ToolCallIdentity);
        }
        Ok(vec![ContentPart::Text {
            text: format!("{}\n\n{}", request.system, request.input),
        }])
    }
    async fn read(
        &self,
        conn: &mut Conn,
        frame: &Frame<'_>,
    ) -> Result<Option<Receipt>, EngineError> {
        if frame.capability != "summary" {
            return Err(EngineError::ToolCallIdentity);
        }
        Ok(completed(conn, frame.helper, frame.turn, frame.machine)
            .await?
            .map(|answer| Receipt {
                command: answer.command,
                result: CapabilityResult {
                    items: vec![CapabilityItem {
                        family: "summary".into(),
                        native_ref: Some(answer.native_ref),
                        payload: json!({"text":answer.text}),
                    }],
                    usage: Json::Null,
                },
            }))
    }
}
