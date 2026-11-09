//! Native MCP image parts use the ordinary image item and retained-output cache.

use base64::Engine as _;
use orch_agent_contract::events::{
    BlobRef, ErrorPayload, ItemCompleted, ItemContent, ItemStarted, ItemStatus, OutputRef, RawCall,
    SessionEvent, ToolCallPayload, ToolPayload,
};
use orch_types::{Id, Json, Time};
use serde_json::json;

use super::{Context, opt_text, text};

/// Text and structured MCP data stay in their original result; binary image data has its own item.
pub(super) fn result_without_image_data(result: &Json) -> Json {
    let mut result = result.clone();
    if let Some(parts) = result.get_mut("content").and_then(Json::as_array_mut) {
        for part in parts {
            if text(part, "type") == "image" {
                *part = image_metadata(part);
            }
        }
    }
    result
}

fn image_metadata(part: &Json) -> Json {
    json!({"type":"image", "mimeType":part.get("mimeType"), "name":part.get("name")})
}

/// The complete original result remains available on demand through its ordinary raw output blob.
pub(super) fn raw_result(
    result: &Json,
    ctx: &Context<'_>,
    blob: &mut Option<BlobRef>,
) -> Option<OutputRef> {
    if !result
        .get("content")
        .and_then(Json::as_array)
        .is_some_and(|parts| parts.iter().any(|part| text(part, "type") == "image"))
    {
        return None;
    }
    let raw = result.to_string();
    let retained = ctx.outputs.put(raw.as_bytes()).ok()?;
    let hash = retained.hash;
    *blob = Some(retained);
    Some(OutputRef {
        inline: String::new(),
        total_len: i64::try_from(raw.len()).unwrap_or(i64::MAX),
        truncated: true,
        blob: Some(hash),
    })
}

pub(super) fn returned_images(
    item: &Json,
    turn_id: Id,
    stream: &str,
    started_at: Time,
    ctx: &Context<'_>,
) -> Vec<SessionEvent> {
    let mut events = Vec::new();
    let parent = text(item, "id");
    for (index, part) in item
        .pointer("/result/content")
        .and_then(Json::as_array)
        .into_iter()
        .flatten()
        .enumerate()
        .filter(|(_, part)| text(part, "type") == "image")
    {
        // Native item ids and content positions survive replay; equal bytes still represent two parts.
        let native_ref = format!("image-result:{parent}:{index}");
        let retained = retain(part, ctx);
        let (content, payload, status) = match retained {
            Ok(tool) => {
                let call = ToolCallPayload {
                    title: "Image".to_owned(),
                    started_at,
                    duration_ms: None,
                    tool,
                    raw: RawCall {
                        name: "image_result".to_owned(),
                        input: OutputRef::of(&image_metadata(part).to_string()),
                        output: None,
                        is_error: false,
                    },
                };
                let payload = serde_json::to_value(&call).unwrap_or(Json::Null);
                (ItemContent::ToolCall(call), payload, ItemStatus::Completed)
            }
            Err(reason) => {
                let error = ErrorPayload {
                    code: "image_result_unavailable".to_owned(),
                    message: "The returned image could not be retained.".to_owned(),
                    retryable: false,
                    details: Some(json!({"reason":reason})),
                    window: None,
                    resets_at: None,
                };
                let payload = serde_json::to_value(&error).unwrap_or(Json::Null);
                (ItemContent::Error(error), payload, ItemStatus::Failed)
            }
        };
        events.push(SessionEvent::ItemStarted(ItemStarted {
            native_ref: native_ref.clone(),
            turn_id,
            stream: stream.to_owned(),
            parent_native_ref: Some(parent.to_owned()),
            content,
        }));
        events.push(SessionEvent::ItemCompleted(ItemCompleted {
            native_ref,
            status,
            payload,
            output_blob: None,
        }));
    }
    events
}

fn retain(part: &Json, ctx: &Context<'_>) -> Result<ToolPayload, &'static str> {
    retain_inline_image(
        text(part, "mimeType"),
        part.get("data").and_then(Json::as_str),
        opt_text(part, "name"),
        ctx.outputs,
    )
}

pub(super) fn retain_inline_image(
    media_type: &str,
    encoded: Option<&str>,
    name: Option<String>,
    outputs: &super::OutputStore,
) -> Result<ToolPayload, &'static str> {
    if !media_type.strip_prefix("image/").is_some_and(|subtype| {
        !subtype.is_empty()
            && subtype
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || b"!#$&^_.+-".contains(&byte))
    }) {
        return Err("invalid_media_type");
    }
    let encoded = encoded.ok_or("missing_inline_data")?;
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(encoded)
        .map_err(|_| "invalid_base64")?;
    if bytes.is_empty() {
        return Err("empty_image");
    }
    let blob = outputs.put(&bytes).map_err(|_| "output_unavailable")?;
    Ok(ToolPayload::Image {
        blob: blob.hash,
        media_type: media_type.to_owned(),
        name,
        caption: None,
    })
}
