//! Private immutable model details for one forwarded native process.
use orch_agent_contract::adapter::AdapterError;
use orch_agent_contract::session::{ProviderAccess, SessionSpec};
use orch_types::{Hash, Id, Json};
use serde_json::json;
use std::io::Write as _;
use std::sync::Arc;

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub(super) struct Identity {
    pub provider: Id,
    pub fingerprint: Hash,
}

pub(super) struct Catalogue {
    pub identity: Identity,
    file: tempfile::NamedTempFile,
}
impl Catalogue {
    pub fn path(&self) -> &std::path::Path {
        self.file.path()
    }
    pub fn write(
        provider: Id,
        document: &Json,
        directory: &std::path::Path,
    ) -> Result<Arc<Self>, AdapterError> {
        std::fs::create_dir_all(directory)
            .map_err(|error| AdapterError::HarnessUnavailable(error.to_string()))?;
        let bytes = serde_json::to_vec(document)
            .map_err(|error| AdapterError::Internal(error.to_string()))?;
        let mut file = tempfile::Builder::new()
            .prefix("orch-model-catalogue-")
            .suffix(".json")
            .tempfile_in(directory)
            .map_err(|error| AdapterError::HarnessUnavailable(error.to_string()))?;
        file.write_all(&bytes)
            .and_then(|()| file.flush())
            .map_err(|error| AdapterError::HarnessUnavailable(error.to_string()))?;
        Ok(Arc::new(Self {
            identity: Identity {
                provider,
                fingerprint: Hash::of(&bytes),
            },
            file,
        }))
    }
}

pub(super) fn document(spec: &SessionSpec) -> Option<Json> {
    if !matches!(spec.provider, ProviderAccess::Forwarded { .. }) {
        return None;
    }
    let native = spec
        .settings
        .native_model
        .as_deref()
        .filter(|model| model.model == spec.settings.model)?;
    let parameters = &native.parameters;
    let switches = crate::sessions::resolved_native_config(&spec.settings);
    let image = parameters.takes_images();
    let window = parameters
        .context_window
        .filter(|window| *window > 0)
        .or(parameters.context_open());
    let maximum = parameters.context_open().or(window);
    let tiers: Vec<_> = parameters
        .service_tiers
        .as_ref()
        .into_iter()
        .flatten()
        .map(|tier| json!({"id":tier.id,"name":tier.label,"description":""}))
        .collect();
    let efforts: Vec<_> = parameters
        .efforts
        .as_ref()
        .into_iter()
        .flatten()
        .map(|effort| json!({"effort":effort,"description":""}))
        .collect();
    let source = include_str!("model-prompt.md");
    let instructions = source
        .strip_prefix("<!--")
        .and_then(|source| source.split_once("-->\n"))
        .map_or(source, |(_, instructions)| instructions);
    let default_effort = parameters.default_effort.as_deref().filter(|effort| {
        parameters
            .efforts
            .as_ref()
            .is_some_and(|enabled| enabled.iter().any(|value| value == effort))
    });
    // The native fallback's tool-output default; explicit native profile overrides still apply.
    // This is independent of the admitted model context and never limits a document or attachment.
    Some(json!({"models":[{
        "slug":native.model,"display_name":native.model,"description":null,
        "supported_reasoning_levels":efforts,"default_reasoning_level":default_effort,
        "shell_type":"unified_exec","visibility":"list","supported_in_api":true,"priority":0,
        "additional_speed_tiers":[],"service_tiers":tiers,"default_service_tier":null,
        "availability_nux":null,"upgrade":null,"model_messages":{"instructions_template":instructions},
        "include_apps_usage_instructions":false,"supports_reasoning_summary_parameter":true,
        "support_verbosity":false,"default_verbosity":null,"apply_patch_tool_type":null,
        "truncation_policy":{"mode":"bytes","limit":10_000},
        "context_window":window,"max_context_window":maximum,
        "auto_review_model_override":null,
        "effective_context_window_percent":100,"experimental_supported_tools":[],
        "input_modalities":if image {vec!["text","image"]}else{vec!["text"]},
        "use_responses_lite":switches.get("model.use_responses_lite").and_then(Json::as_bool).unwrap_or(false)
    }]}))
}

pub(super) fn fingerprint(document: &Json) -> Result<Hash, AdapterError> {
    serde_json::to_vec(document)
        .map(|bytes| Hash::of(&bytes))
        .map_err(|error| AdapterError::Internal(error.to_string()))
}
