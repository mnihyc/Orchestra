//! Program-declared mappings from native profile configuration to the existing parameter validator.

use crate::parameters::{ModelParameters, ParameterDef, Resolution};
use orch_types::Json;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// The retained extension key; native mappings cannot be changed by descriptor overrides.
pub const EXTENSION: &str = "x_profile_parameters";

/// A program's native configuration paths and their managed parameter aliases.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProfileParameters {
    /// The first version of this additive mapping contract.
    pub version: u32,
    /// Native key paths and their existing neutral value constraints.
    #[serde(default)]
    pub bindings: Vec<Binding>,
}

/// A native option is translated by its program, never by a vendor branch in shared code.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Binding {
    /// JSON pointer within the profile's configuration.
    pub pointer: String,
    /// The same declared parameter definition used for model choices and native-schema validation.
    pub parameter: ParameterDef,
    /// Exact native string aliases; an absent alias keeps the reported JSON value unchanged.
    #[serde(default)]
    pub values: BTreeMap<String, Json>,
}

impl ProfileParameters {
    /// Validates only declared native parameters; unrelated configuration is not rewritten or refused.
    pub fn validate(
        &self,
        config: &Json,
        model: &ModelParameters,
        features: &BTreeMap<String, bool>,
    ) -> Result<BTreeMap<String, Json>, &'static str> {
        if self.version != 1 {
            return Err("the program's native profile parameter mapping is unsupported");
        }
        let mut values = BTreeMap::new();
        for binding in &self.bindings {
            // Native configuration writers omit JSON null; it is not an explicit startup value.
            let Some(raw) = config
                .pointer(&binding.pointer)
                .filter(|value| !value.is_null())
            else {
                continue;
            };
            let value = raw
                .as_str()
                .and_then(|key| binding.values.get(key))
                .unwrap_or(raw);
            if !binding.parameter.is_enabled(features) {
                return Err("the agent profile sets a native parameter that is not enabled");
            }
            match binding.parameter.resolve_value(model, Some(value)) {
                Resolution::Send(valid) if valid == *value => {}
                Resolution::SendNothing if value.is_null() => {}
                _ => {
                    return Err(
                        "the agent profile's native parameters exceed the provider's allowed values",
                    );
                }
            }
            values.insert(binding.parameter.key.clone(), value.clone());
        }
        Ok(values)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn native_aliases_use_the_same_model_range_and_schema_without_reading_unrelated_config()
    -> Result<(), Box<dyn std::error::Error>> {
        let binding: Binding = serde_json::from_value(
            json!({"pointer":"/native_window", "parameter":{
            "key":"context", "title":"Context", "description":"Window", "group":"model", "type":"integer",
            "range":{"min":1, "max":{"from_model":"context_max"}}, "default":null,
            "scopes":[], "apply":"new_session"}}),
        )?;
        let declared = ProfileParameters {
            version: 1,
            bindings: vec![binding],
        };
        let model: ModelParameters = serde_json::from_value(json!({"context_max":128000,
            "request_options":{"native":{"type":"object", "properties":{"context":{"type":"integer", "maximum":64000}}}}}))?;
        let source = json!({"native_window":32000, "unknown_feature":{"retained":"unchanged"}});
        let before = source.clone();
        assert_eq!(
            declared
                .validate(&source, &model, &BTreeMap::new())?
                .get("context"),
            Some(&json!(32000))
        );
        assert_eq!(source, before);
        assert!(
            declared
                .validate(&json!({"native_window":64001}), &model, &BTreeMap::new())
                .is_err()
        );
        assert!(
            declared
                .validate(&json!({"native_window":"32000"}), &model, &BTreeMap::new())
                .is_err()
        );
        assert!(
            declared
                .validate(
                    &json!({"native_window":null, "unknown_feature":true}),
                    &model,
                    &BTreeMap::new()
                )?
                .is_empty()
        );
        Ok(())
    }
}
