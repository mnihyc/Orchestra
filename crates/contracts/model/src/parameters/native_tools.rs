//! Explicit closed bindings apply to the protocol's declared native request identities.
use super::*;
use crate::web_search::identity_matches;
use orch_provider_contract::capabilities::{Interface, protocol_interfaces};
use std::collections::BTreeMap;
use std::sync::OnceLock;

/// Selection uses the same declared containers, partial identities, root identities,
/// executable variants and forms as native-tool requests, without interpreting tool names.
fn selected(request: &Json, body: &Json) -> bool {
    let mut forms = vec![request];
    while let Some(form) = forms.pop() {
        let container = form
            .get("container")
            .and_then(Json::as_str)
            .unwrap_or("/tools");
        if body
            .pointer(container)
            .and_then(Json::as_array)
            .into_iter()
            .flatten()
            .any(|tool| {
                form.get("identities")
                    .and_then(Json::as_array)
                    .into_iter()
                    .flatten()
                    .any(|identity| identity_matches(identity, tool))
            })
            || form
                .get("root_selection_identity")
                .is_some_and(|identity| identity_matches(identity, body))
            || form
                .get("model_variant_identity")
                .and_then(Json::as_str)
                .is_some_and(|identity| {
                    body.get("model")
                        .and_then(Json::as_str)
                        .is_some_and(|model| model.split(':').skip(1).any(|part| part == identity))
                })
        {
            return true;
        }
        forms.extend(
            form.get("forms")
                .and_then(Json::as_array)
                .into_iter()
                .flatten(),
        );
    }
    false
}

pub(super) fn check_closed(
    protocol: &str,
    model: &ModelParameters,
    body: &Json,
) -> Result<(), &'static str> {
    let Some(tools) = model
        .native_tools
        .get(protocol)
        .filter(|tools| tools.values().any(Option::is_none))
    else {
        return Ok(());
    };
    static INTERFACES: OnceLock<Result<BTreeMap<String, Interface>, serde_json::Error>> =
        OnceLock::new();
    let interfaces = INTERFACES
        .get_or_init(protocol_interfaces)
        .as_ref()
        .map_err(|_| "the native protocol declarations cannot be read")?;
    let Some(interface) = interfaces.get(protocol) else {
        return Ok(());
    };
    if tools.iter().any(|(capability, binding)| {
        binding.is_none()
            && interface
                .native_tools
                .get(capability)
                .is_some_and(|declared| selected(&declared.request, body))
    }) {
        return Err("this endpoint and model do not offer the selected native tool");
    }
    Ok(())
}
