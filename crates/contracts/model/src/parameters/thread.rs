//! Per-thread wire choices use the provider's existing option declarations and final validator.

use orch_provider_contract::{calls, models::ModelParameters};
use orch_types::Json;
use std::collections::BTreeMap;

/// Each interface is one ordinary map member, so its choice inherits and resets independently.
pub const PREFIX: &str = "provider_options:";
/// Choices are keyed by the actual outgoing interface, including when a call is translated.
pub type Values = BTreeMap<String, Json>;

/// Freeze explicit choices only. Defaults and fixed values remain with the frozen model schema.
pub fn read(params: &Json) -> Result<Values, &'static str> {
    let object = params
        .as_object()
        .ok_or("agent parameters must be an object")?;
    let mut values = Values::new();
    for (key, value) in object {
        let Some(interface) = key.strip_prefix(PREFIX) else {
            continue;
        };
        if interface.is_empty() || interface == "native" {
            return Err("provider options must name a wire interface");
        }
        calls::completion_options(value)?;
        values.insert(interface.to_owned(), value.clone());
    }
    Ok(values)
}

/// A thread can choose only fields the provider actually opened on this interface.
/// This is no route resolver: the existing forwarder supplies its selected outgoing interface.
pub fn selected<'a>(
    interface: &str,
    model: &ModelParameters,
    values: &'a Json,
) -> Result<&'a Json, &'static str> {
    calls::completion_options(values)?;
    let object = values
        .as_object()
        .ok_or("provider options must be an object")?;
    if object.keys().any(|key| {
        model
            .option_rule(interface, key)
            .is_none_or(|rule| rule == &Json::Bool(false))
    }) {
        return Err("a thread option is not opened by this provider model");
    }
    Ok(values)
}

/// The existing schema form, restricted to explicitly opened, non-owned fields.
/// It is presentation data only; complete final validation keeps the original schemas.
#[must_use]
pub fn forms(model: &ModelParameters) -> BTreeMap<String, Json> {
    let mut forms = BTreeMap::new();
    for (interface, declared) in model
        .request_options
        .iter()
        .flat_map(|values| values.iter())
    {
        if interface == "native" {
            continue;
        }
        let mut names = std::collections::BTreeSet::new();
        let mut references = std::collections::BTreeSet::new();
        let mut at = declared;
        loop {
            if let Some(properties) = at.get("properties").and_then(Json::as_object) {
                names.extend(properties.keys().cloned());
            }
            let Some(reference) = at.get("$ref").and_then(Json::as_str) else {
                break;
            };
            if !references.insert(reference) {
                break;
            }
            let Some(next) = reference
                .strip_prefix('#')
                .and_then(|pointer| declared.pointer(pointer))
            else {
                break;
            };
            at = next;
        }
        let mut form = super::constraints(interface, model);
        let properties = names
            .into_iter()
            .filter(|key| !calls::owned_option(key))
            .filter_map(|key| {
                let rule = model.option_rule(interface, &key)?;
                if rule == &Json::Bool(false) {
                    return None;
                }
                let rule = form
                    .get("properties")
                    .and_then(|properties| properties.get(&key))
                    .unwrap_or(rule)
                    .clone();
                Some((key, rule))
            })
            .collect::<serde_json::Map<_, _>>();
        if properties.is_empty() {
            continue;
        }
        let Some(object) = form.as_object_mut() else {
            continue;
        };
        // Each field retains its local-reference root; root validators belong to outgoing_at.
        object.remove("$ref");
        object.remove("allOf");
        object.remove("x-orch-option-layers");
        object.remove("required");
        object.insert("type".into(), Json::String("object".into()));
        object.insert("properties".into(), Json::Object(properties));
        object.insert("additionalProperties".into(), Json::Bool(false));
        super::mark_provider_source(&mut form);
        forms.insert(interface.clone(), form);
    }
    forms
}
