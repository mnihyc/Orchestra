//! Conversations with no tools use only the supplied material and no ambient project context.
use orch_agent_contract::{adapter::AdapterError, descriptor::SideReading, session::SessionSpec};
use orch_types::Json;
use serde_json::json;

pub(crate) fn no_tools(spec: &SessionSpec) -> bool {
    spec.settings.side_reading == Some(SideReading::NoTools)
}

pub(crate) fn validate(spec: &SessionSpec) -> Result<(), AdapterError> {
    if let Some(reading) = spec.settings.side_reading {
        if reading != SideReading::NoTools
            || !crate::descriptor::manifest().proves("side_thread_modes.no_tools")
        {
            return Err(AdapterError::Unsupported(format!(
                "The requested {reading:?} reading mode has no verified confinement."
            )));
        }
        if !spec.settings.orch_tools.is_empty() || !spec.mcp.is_empty() {
            return Err(AdapterError::Unsupported(
                "A tool-free conversation cannot offer a model-callable server.".into(),
            ));
        }
    }
    Ok(())
}

/// These pins override profile and home preferences for this private conversation only.
pub(crate) fn settings() -> serde_json::Map<String, Json> {
    let mut settings = crate::sessions::managed_settings(false);
    settings.insert("autoMemoryEnabled".into(), json!(false));
    settings.insert("claudeMdExcludes".into(), json!(["**"]));
    settings.insert("enabledPlugins".into(), json!({}));
    settings
}
