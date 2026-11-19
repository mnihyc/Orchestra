//! Neutral native search choices. Wire protocols declare their own tool fields.

use orch_types::Json;
use serde_json::{Map, json};

/// The selected search mode. A configured object defaults to live search; null disables it.
pub fn mode(value: &Json) -> Result<&str, &'static str> {
    let selected = if value.is_null() {
        "disabled"
    } else if let Some(mode) = value.as_str() {
        mode
    } else if let Some(object) = value.as_object() {
        object.get("mode").map_or(Ok("live"), |mode| {
            mode.as_str().ok_or("web search mode must be text")
        })?
    } else {
        return Err("web search must be a mode or a configuration object");
    };
    match selected {
        "disabled" | "cached" | "live" => Ok(selected),
        _ => Err("web search mode must be disabled, cached or live"),
    }
}

/// Canonical object form lets a per-use mode retain configured tool options during a merge.
pub fn options(value: &Json) -> Result<Map<String, Json>, &'static str> {
    let mode = mode(value)?;
    let mut options = value.as_object().cloned().unwrap_or_default();
    options.insert("mode".to_owned(), json!(mode));
    Ok(options)
}

/// Built-in option declarations shared with the provider forms. New protocols add data here;
/// external protocol implementations can supply their own declaration instead.
#[must_use]
pub fn declaration(protocol: &str) -> Option<Json> {
    static DECLARATIONS: std::sync::OnceLock<Json> = std::sync::OnceLock::new();
    DECLARATIONS
        .get_or_init(|| {
            serde_json::from_str(include_str!("../../../../shared/protocol-options.json"))
                .unwrap_or(Json::Null)
        })
        .get("protocols")?
        .get(protocol)?
        .get("web_search")
        .cloned()
}
