//! Configuration schema and supported fields.
//!
//! A summarizer says what it is: a name, a title, the schema of its configuration, and which of the fields of
//! a node it can write ([`SummaryField`]). `auto.summarizer.kind` names the implementation; the rest of that
//! setting is validated against the implementation's `config_schema` and passed as `config`
//! ([`split_setting`]). A summarizer that needs no model (rules, templates, an internal service) is as valid
//! as one that calls one.

use orch_plugin_contract::Extensions;
use orch_types::Json;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// A field of a node that automation can write.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize, JsonSchema,
)]
#[serde(rename_all = "snake_case")]
pub enum SummaryField {
    /// The node's title.
    Title,
    /// Its description: the live progress of the work.
    Description,
    /// Its tags, among those defined for its project and org.
    Tags,
    /// A topic's status made from its threads' statuses.
    Rollup,
    /// The bounded summary for another agent to continue the work.
    Handoff,
}

impl SummaryField {
    /// Every field.
    pub const ALL: [Self; 5] = [
        Self::Title,
        Self::Description,
        Self::Tags,
        Self::Rollup,
        Self::Handoff,
    ];
}

/// What a summarizer declares about itself.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct SummarizerDescriptor {
    /// Its name, which `auto.summarizer.kind` names.
    pub name: String,
    /// What it is called.
    pub title: String,
    /// The JSON Schema of the rest of the setting, which is passed to it as `config`.
    pub config_schema: Json,
    /// Which fields it can write.
    pub fields: Vec<SummaryField>,
    /// Members that belong to the implementation, kept and never interpreted.
    #[serde(flatten)]
    pub extensions: Extensions,
}

/// What is wrong with a summarizer's declaration.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum DescriptorFault {
    /// It has no name.
    #[error("a summarizer has a name")]
    NoName,
    /// It can write no field.
    #[error("a summarizer writes at least one field")]
    NoField,
    /// It names a field twice.
    #[error("the field {0:?} is listed twice")]
    DuplicateField(SummaryField),
    /// The schema of its configuration is not an object.
    #[error("the schema of the configuration is a JSON Schema object")]
    SchemaNotAnObject,
}

impl SummarizerDescriptor {
    /// Whether it can write `field`.
    #[must_use]
    pub fn writes(&self, field: SummaryField) -> bool {
        self.fields.contains(&field)
    }

    /// Of the fields that settings ask automation to write, those it cannot.
    #[must_use]
    pub fn cannot_write(&self, wanted: &[SummaryField]) -> Vec<SummaryField> {
        wanted
            .iter()
            .copied()
            .filter(|field| !self.writes(*field))
            .collect()
    }

    /// Checks that the declaration is consistent with itself. Every fault is returned.
    pub fn check(&self) -> Result<(), Vec<DescriptorFault>> {
        let mut faults = Vec::new();
        if self.name.is_empty() {
            faults.push(DescriptorFault::NoName);
        }
        if self.fields.is_empty() {
            faults.push(DescriptorFault::NoField);
        }
        for (position, field) in self.fields.iter().enumerate() {
            if self.fields[..position].contains(field) {
                faults.push(DescriptorFault::DuplicateField(*field));
            }
        }
        if !self.config_schema.is_object() {
            faults.push(DescriptorFault::SchemaNotAnObject);
        }
        if faults.is_empty() {
            Ok(())
        } else {
            Err(faults)
        }
    }
}

/// Why `auto.summarizer` is not a setting a summarizer can be chosen by.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum SettingError {
    /// It is not an object.
    #[error("auto.summarizer is an object")]
    NotAnObject,
    /// It names no kind.
    #[error("auto.summarizer names the kind of summarizer in its member kind")]
    NoKind,
}

/// Splits the value of `auto.summarizer` into the kind it names and the rest, which is the configuration
/// that kind's schema validates and that is passed to it.
pub fn split_setting(setting: &Json) -> Result<(String, Json), SettingError> {
    let Json::Object(members) = setting else {
        return Err(SettingError::NotAnObject);
    };
    let kind = members
        .get("kind")
        .and_then(Json::as_str)
        .filter(|kind| !kind.is_empty())
        .ok_or(SettingError::NoKind)?
        .to_owned();
    let mut config = members.clone();
    config.remove("kind");
    Ok((kind, Json::Object(config)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn model() -> SummarizerDescriptor {
        serde_json::from_value(json!({
            "name": "model", "title": "A small model",
            "config_schema": { "type": "object", "properties": { "provider_id": { "type": "string" }, "model_name": { "type": "string" } } },
            "fields": ["title", "description", "tags"]
        }))
        .unwrap()
    }

    #[test]
    fn a_summarizer_declares_its_name_its_configuration_and_the_fields_it_writes() {
        let descriptor = model();
        assert_eq!(descriptor.check(), Ok(()));
        assert!(descriptor.writes(SummaryField::Tags));
        assert_eq!(
            serde_json::to_value(&descriptor).unwrap()["fields"],
            json!(["title", "description", "tags"])
        );
    }

    #[test]
    fn a_summarizer_with_rules_and_no_model_that_writes_descriptions_alone_is_as_valid() {
        let rules: SummarizerDescriptor = serde_json::from_value(json!({
            "name": "rules", "title": "Rule-based", "config_schema": {}, "fields": ["description"], "x_rules": 12
        }))
        .unwrap();
        assert_eq!(rules.check(), Ok(()));
        assert_eq!(
            rules.cannot_write(&SummaryField::ALL),
            [
                SummaryField::Title,
                SummaryField::Tags,
                SummaryField::Rollup,
                SummaryField::Handoff
            ]
        );
        assert_eq!(rules.extensions.get("x_rules"), Some(&json!(12)));
        assert!(rules.cannot_write(&[SummaryField::Description]).is_empty());
    }

    #[test]
    fn a_declaration_that_cannot_hold_together_is_refused() {
        let mut broken = model();
        broken.name.clear();
        broken.fields = vec![SummaryField::Title, SummaryField::Title];
        broken.config_schema = json!(null);
        let faults = broken.check().unwrap_err();
        for expected in [
            DescriptorFault::NoName,
            DescriptorFault::DuplicateField(SummaryField::Title),
            DescriptorFault::SchemaNotAnObject,
        ] {
            assert!(faults.contains(&expected), "{expected:?} in {faults:?}");
        }
        broken.fields.clear();
        assert!(
            broken
                .check()
                .unwrap_err()
                .contains(&DescriptorFault::NoField)
        );
    }

    #[test]
    fn the_setting_names_the_kind_and_the_rest_is_the_configuration() {
        let setting = json!({
            "kind": "model", "provider_id": "p1", "model_name": "small", "max_output_tokens": 120,
            "input_items": 50, "input_chars": 500, "prompts": { "description": null, "title": null, "tags": null }
        });
        let (kind, config) = split_setting(&setting).unwrap();
        assert_eq!(kind, "model");
        assert!(config.get("kind").is_none());
        assert_eq!(config["input_items"], 50);
        assert_eq!(
            split_setting(&json!({ "model_name": "x" })),
            Err(SettingError::NoKind)
        );
        assert_eq!(
            split_setting(&json!({ "kind": "" })),
            Err(SettingError::NoKind)
        );
        assert_eq!(
            split_setting(&json!("model")),
            Err(SettingError::NotAnObject)
        );
    }
}
