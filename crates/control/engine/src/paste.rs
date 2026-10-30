//! Folded text is verified from its private blob and sent inline only within the remaining context budget.
//!
//! The share chooses the default. An explicit message choice, including the always-inline setting,
//! leaves context compaction to the agent program and does not impose an Orch message-size cap.

use crate::context_fill::{TextKind, estimate_text};
use crate::{Engine, EngineError};
use orch_api_contract::threads::ContentPart;
use orch_blob_contract::store::read_all;
use orch_db::Conn;
use orch_db::queries::threads::ThreadRow;
use orch_types::Id;
use std::collections::BTreeSet;

/// The shipped share leaves most remaining context for work; the registry makes it changeable.
pub const DEFAULT_INLINE_SHARE_PERCENT: i64 = 25;
/// Four-byte Unicode at 1.32 characters per token can use 5.28 bytes; rounding up bounds every eligible read.
const MOST_BYTES_PER_ESTIMATED_TOKEN: i64 = 6;
/// Percent values are stored as whole numbers by the settings registry.
const PERCENT: i64 = 100;

/// Public copies keep the folded upload metadata and load its text only when somebody opens it.
#[must_use]
pub fn metadata(content: &orch_types::Json) -> orch_types::Json {
    let mut content = content.clone();
    if let Some(parts) = content.as_array_mut() {
        for part in parts {
            if part.get("type").and_then(orch_types::Json::as_str) == Some("pasted_text")
                && let Some(part) = part.as_object_mut()
            {
                part.remove("inline_text");
            }
        }
    }
    content
}

/// The paste allowance, from the free part of the current context rather than the whole window.
#[must_use]
pub fn inline_allowance(window: Option<i64>, used: Option<i64>, share: i64) -> i64 {
    match (window, used) {
        (Some(window), Some(used)) => {
            window
                .saturating_sub(used)
                .max(0)
                .saturating_mul(share.clamp(0, PERCENT))
                / PERCENT
        }
        _ => 0,
    }
}

/// The first available numbered filename, preserving the extension and treating a dotfile as one name.
#[must_use]
pub fn available_name(name: &str, used: &BTreeSet<String>) -> String {
    if !used.contains(name) {
        return name.to_owned();
    }
    let split = name.rfind('.').filter(|at| *at > 0).unwrap_or(name.len());
    let mut number = 2_u64;
    loop {
        let suffix = format!(" ({number})");
        let mut stem = name[..split].to_owned();
        let mut extension = name[split..].to_owned();
        // Keep the existing character bound and never grow an already-long Unicode component in bytes.
        let bytes = name.len().max(crate::delivery::NAME_MAX_CHARS);
        while stem.chars().count() + suffix.chars().count() + extension.chars().count()
            > crate::delivery::NAME_MAX_CHARS
            || stem.len() + suffix.len() + extension.len() > bytes
        {
            if stem.chars().count() > 1 {
                let _ = stem.pop();
            } else if extension.pop().is_none() {
                break;
            }
        }
        let candidate = format!("{stem}{suffix}{extension}");
        if !used.contains(&candidate) {
            return candidate;
        }
        number = number.saturating_add(1);
    }
}

/// Names already promised by an input or a saved draft; allocation is serialized by the thread lock.
pub async fn attachment_names(
    conn: &mut Conn,
    thread: Id,
) -> Result<BTreeSet<String>, EngineError> {
    let names: Vec<String> = sqlx::query_scalar("SELECT DISTINCT part->>'name' FROM (
        SELECT content FROM thread_inputs WHERE thread_id=$1
        UNION ALL SELECT content FROM drafts WHERE thread_id=$1
        UNION ALL SELECT jsonb_build_array(jsonb_build_object('type','file','name',name)) FROM thread_attachment_uploads WHERE thread_id=$1
      ) sources CROSS JOIN LATERAL jsonb_array_elements(content) part
      WHERE part->>'type' IN ('image','file','pasted_text') AND part->>'name' IS NOT NULL")
        .bind(thread).fetch_all(conn).await.map_err(orch_db::DbError::from)?;
    Ok(names.into_iter().collect())
}

impl Engine {
    /// Gives new attachment paths distinct names while preserving intentional references to identical bytes.
    pub(crate) async fn prepare_attachment_names(
        &self,
        conn: &mut Conn,
        thread: Id,
        content: &mut [ContentPart],
    ) -> Result<(), EngineError> {
        if !content.iter().any(|part| matches!(part, ContentPart::Image { .. } | ContentPart::PastedText(_)) || matches!(part, ContentPart::File { path, blob: Some(_), .. } if path.starts_with(".orch/attachments/"))) { return Ok(()); }
        let values: Vec<orch_types::Json> = sqlx::query_scalar("SELECT part FROM thread_inputs CROSS JOIN LATERAL jsonb_array_elements(content) part WHERE thread_id=$1 AND part->>'type' IN ('image','file','pasted_text')")
            .bind(thread).fetch_all(&mut *conn).await.map_err(orch_db::DbError::from)?;
        let mut held = std::collections::BTreeMap::new();
        for value in values {
            if let Ok(part) = serde_json::from_value::<ContentPart>(value) {
                match part {
                    ContentPart::Image { name, blob, .. } => {
                        held.insert(format!(".orch/attachments/{name}"), blob);
                    }
                    ContentPart::File {
                        path,
                        blob: Some(blob),
                        ..
                    } => {
                        held.insert(path, blob);
                    }
                    ContentPart::PastedText(paste) => {
                        held.insert(paste.path, paste.blob);
                    }
                    _ => {}
                }
            }
        }
        let mut used: BTreeSet<String> = held
            .keys()
            .filter_map(|path| path.strip_prefix(".orch/attachments/").map(str::to_owned))
            .collect();
        for part in content {
            let (name, blob, path) = match part {
                ContentPart::Image {
                    name, blob, path, ..
                } => (name, *blob, Some(path.get_or_insert_with(String::new))),
                ContentPart::File {
                    name,
                    blob: Some(blob),
                    path,
                    ..
                } if path.starts_with(".orch/attachments/") => (name, *blob, Some(path)),
                ContentPart::PastedText(paste) => {
                    (&mut paste.name, paste.blob, Some(&mut paste.path))
                }
                _ => continue,
            };
            *name = crate::delivery::safe_name(name);
            let original = format!(".orch/attachments/{name}");
            if held.get(&original) != Some(&blob) {
                *name = available_name(name, &used);
            }
            let assigned = format!(".orch/attachments/{name}");
            used.insert(name.clone());
            held.insert(assigned.clone(), blob);
            if let Some(path) = path {
                *path = assigned;
            }
        }
        Ok(())
    }

    /// Verifies folded uploads at sending and records the delivery choice the person previews.
    pub(crate) async fn prepare_pastes(
        &self,
        conn: &mut Conn,
        thread: &ThreadRow,
        original: &[ContentPart],
    ) -> Result<Vec<ContentPart>, EngineError> {
        if !original
            .iter()
            .any(|part| matches!(part, ContentPart::PastedText(_)))
        {
            return Ok(original.to_vec());
        }
        let settings = crate::resolved::load(conn, thread.node_id).await?;
        let model = settings.model.as_deref().unwrap_or_default();
        let family = crate::token_density::of(model);
        let share = orch_settings::get::<i64>(
            conn,
            orch_settings::Place::Node(thread.node_id),
            "composer.paste_inline_context_share",
        )
        .await?
        .unwrap_or(DEFAULT_INLINE_SHARE_PERCENT);
        let usage = thread.usage.as_ref();
        let scope = orch_db::queries::thread_continuations::projection(conn, thread.node_id, None)
            .await?
            .ok_or_else(|| EngineError::unreadable("the selected thread history is unavailable"))?;
        let context = orch_db::queries::thread_history::context_history(
            conn,
            thread.node_id,
            scope.viewed_continuation_id,
        )
        .await?;
        let fill = context.context.and_then(|value| {
            serde_json::from_value::<orch_api_contract::context::ContextFill>(value).ok()
        });
        let model_parameters = if let Some(provider) = thread.provider_id {
            orch_db::queries::resources::thread_engine::model(conn, provider, model)
                .await?
                .and_then(|row| {
                    serde_json::from_value::<orch_provider_contract::models::ModelParameters>(
                        row.parameters,
                    )
                    .ok()
                })
        } else {
            None
        };
        let model_window = model_parameters
            .as_ref()
            .and_then(orch_provider_contract::models::ModelParameters::context_start);
        let mut configured = None;
        if let Some(agent) = thread.agent_id
            && let Some(agent) =
                orch_db::queries::resources::thread_engine::agent(conn, agent).await?
        {
            let definitions = agent
                .descriptor
                .get("parameters")
                .cloned()
                .and_then(|value| {
                    serde_json::from_value::<Vec<orch_agent_contract::parameters::ParameterDef>>(
                        value,
                    )
                    .ok()
                })
                .unwrap_or_default();
            configured = definitions
                .iter()
                .find(|def| {
                    def.kind_of_thread()
                        == Some(orch_agent_contract::parameters::ThreadKind::Context)
                })
                .and_then(|def| settings.params.get(&def.key))
                .and_then(orch_types::Json::as_i64)
                .filter(|value| {
                    *value > 0
                        && model_parameters
                            .as_ref()
                            .and_then(|model| model.context_max.or(model.context_window))
                            .is_none_or(|maximum| *value <= maximum)
                });
        }
        let reported_window = fill.as_ref().and_then(|value| value.window).or_else(|| {
            usage
                .and_then(|value| value.get("context_window"))
                .and_then(orch_types::Json::as_i64)
        });
        let window = match (reported_window, configured.or(model_window)) {
            (Some(reported), Some(effective)) => Some(reported.min(effective)),
            (reported, effective) => reported.or(effective),
        };
        let reported = fill.as_ref().map(|value| value.used).or_else(|| {
            usage
                .and_then(|value| value.get("context_used"))
                .and_then(orch_types::Json::as_i64)
        });
        let used = reported;
        let mut budget = inline_allowance(window, used, share);
        let store = self
            .blobs()
            .ok_or_else(|| EngineError::unreadable("the upload store is unavailable"))?;
        let mut content = original.to_vec();
        for part in &mut content {
            let ContentPart::PastedText(paste) = part else {
                continue;
            };
            paste.inline_text = None;
            let forced = paste.delivery == Some(orch_api_contract::threads::PasteDelivery::Inline)
                || paste.delivery.is_none() && share >= PERCENT;
            let stored = orch_db::queries::storage::blob(conn, &paste.blob)
                .await?
                .ok_or_else(|| EngineError::unreadable("the pasted text is no longer stored"))?;
            paste.size = stored.size;
            if paste.characters < 0
                || paste.characters > paste.size
                || paste.ascii_characters < 0
                || paste.ascii_characters > paste.characters
            {
                return Err(EngineError::unreadable(
                    "the pasted text character count is invalid",
                ));
            }
            if paste.delivery == Some(orch_api_contract::threads::PasteDelivery::File)
                || !forced
                    && (share <= 0
                        || window.is_none()
                        || used.is_none()
                        || paste.size > budget.saturating_mul(MOST_BYTES_PER_ESTIMATED_TOKEN))
            {
                continue;
            }
            let read = store
                .get(&paste.blob, None)
                .await
                .map_err(EngineError::unreadable)?;
            let bytes = read_all(read.body).await.map_err(EngineError::unreadable)?;
            let text = String::from_utf8(bytes.to_vec()).map_err(EngineError::unreadable)?;
            paste.characters = i64::try_from(text.chars().count()).unwrap_or(i64::MAX);
            paste.ascii_characters =
                i64::try_from(text.bytes().filter(u8::is_ascii).count()).unwrap_or(i64::MAX);
            let tokens = estimate_text(&text, family, TextKind::Prose);
            if forced || tokens <= budget {
                budget = budget.saturating_sub(tokens);
                paste.inline_text = Some(text);
            }
        }
        Ok(content)
    }
}
