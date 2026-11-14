//! Native image evidence is distinct from runtime lifetime and consumer publication.
use super::codecs::{Codec, Frame, Receipt};
use super::*;
use async_trait::async_trait;
use orch_db::queries::storage;
use orch_provider_contract::capabilities::{CapabilityItem, CapabilityResult};

pub(super) struct Image;

#[derive(sqlx::FromRow)]
struct Generated {
    native_ref: String,
    blob: String,
    media_type: String,
    revised_prompt: Option<String>,
}

#[async_trait]
impl Codec for Image {
    fn support(&self) -> ExecutionSupport {
        ExecutionSupport {
            input: "image.request".into(),
            result: "image.generated".into(),
            ..Default::default()
        }
    }

    fn prepare(
        &self,
        capability: &str,
        input: &Json,
        media: &[orch_model_contract::generation::GenerationPart],
    ) -> Result<Vec<ContentPart>, EngineError> {
        if input.get("prompt").and_then(Json::as_str).is_none() {
            return Err(EngineError::ToolCallIdentity);
        }
        let count = input.get("n").and_then(Json::as_u64).unwrap_or(1);
        let mut content = vec![ContentPart::Text {
            text: format!(
                "Use your native {capability} capability to produce {count} result(s) for this request. The prompt and admitted options follow as JSON. Do not read other conversations or change workspace files. Return the native result, without substituting a viewed or attached image.\n{}",
                serde_json::to_string(input)?
            ),
        }];
        for image in media {
            let orch_model_contract::generation::GenerationPart::ImageRef {
                blob,
                media_type,
                name,
                ..
            } = image
            else {
                return Err(EngineError::ToolCallIdentity);
            };
            content.push(serde_json::from_value(
                json!({"type":"image","blob":blob,"media_type":media_type,"name":name}),
            )?);
        }
        Ok(content)
    }

    async fn read(
        &self,
        conn: &mut Conn,
        frame: &Frame<'_>,
    ) -> Result<Option<Receipt>, EngineError> {
        let count = frame
            .input
            .get("n")
            .and_then(Json::as_u64)
            .and_then(|n| usize::try_from(n).ok())
            .unwrap_or(1);
        let bound = i64::try_from(count.saturating_add(1)).map_err(EngineError::unreadable)?;
        let generated:Vec<Generated>=sqlx::query_as("SELECT native_ref,payload#>>'{tool,blob}' AS blob,payload#>>'{tool,media_type}' AS media_type,payload#>>'{tool,caption}' AS revised_prompt FROM items WHERE thread_id=$1 AND turn_id=$2 AND kind='tool_call' AND status='completed' AND payload#>>'{tool,kind}'='image' AND payload#>>'{tool,capability}'=$3 AND native_ref IS NOT NULL ORDER BY seq_created,id LIMIT $4")
            .bind(frame.helper).bind(frame.turn).bind(frame.capability).bind(bound).fetch_all(&mut *conn).await.map_err(orch_db::DbError::from)?;
        if generated.len() < count {
            return Ok(None);
        }
        let mut items = Vec::new();
        for image in generated {
            let blob: Hash = image.blob.parse().map_err(EngineError::unreadable)?;
            let kept = storage::blob(conn, &blob)
                .await?
                .ok_or(EngineError::StaleToolSource)?;
            let image = accepted::NativeImage {
                blob,
                size: kept.size,
                media_type: image.media_type,
                native_ref: image.native_ref,
                revised_prompt: image.revised_prompt,
            };
            items.push(CapabilityItem {
                family: "image".into(),
                native_ref: Some(image.native_ref.clone()),
                payload: serde_json::to_value(image)?,
            });
        }
        let command:Option<Id>=sqlx::query_scalar("SELECT id FROM commands WHERE serial_key=$1 AND machine_id=$2 AND kind IN ('turn.start','turn.start.fenced') AND state IN ('sent','accepted','succeeded','failed') AND COALESCE(body->>'turn_id',body#>>'{turn,turn_id}')=$3 AND COALESCE(body->>'thread_id',body#>>'{turn,thread_id}')=$4 ORDER BY created_at DESC,id DESC LIMIT 1")
            .bind(SerialKey::Thread(frame.helper).wire()).bind(frame.machine).bind(frame.turn.to_string()).bind(frame.helper.to_string())
            .fetch_optional(conn).await.map_err(orch_db::DbError::from)?;
        Ok(Some(Receipt {
            command: command.ok_or(EngineError::StaleToolSource)?,
            result: CapabilityResult {
                items,
                usage: Json::Null,
            },
        }))
    }
}
