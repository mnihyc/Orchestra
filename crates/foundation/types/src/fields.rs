//! The one shape in which a person is asked for something, used by sign-in methods, endpoint options and imports.
//!
//! Whoever needs a value from a person describes it as a [`Field`]; one
//! form in the interface renders every such description, and one function
//! here says whether an answer fits it.

use crate::Json;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// What kind of value a field takes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum FieldType {
    /// One line of text.
    Text,
    /// One line that is never shown again.
    Secret,
    /// Several lines that are never shown again, such as a key file.
    SecretText,
    /// One of the field's choices.
    Choice,
    /// Yes or no.
    Boolean,
    /// A calendar day, `YYYY-MM-DD`.
    Date,
    /// Names with values.
    Map,
    /// Names with values that are never shown again.
    SecretMap,
}

impl FieldType {
    /// Whether what is given for such a field is kept sealed and never
    /// returned.
    #[must_use]
    pub fn is_secret(self) -> bool {
        matches!(self, Self::Secret | Self::SecretText | Self::SecretMap)
    }
}

/// One choice of a [`FieldType::Choice`] field.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct Choice {
    /// What is stored.
    pub value: String,
    /// What is shown.
    pub label: String,
}

/// One thing a person is asked for.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct Field {
    /// The member the answer is given under.
    pub name: String,
    /// What the person reads beside it.
    pub label: String,
    /// The kind of value.
    #[serde(rename = "type")]
    pub kind: FieldType,
    /// Whether an answer must be given.
    pub required: bool,
    /// A sentence of help.
    pub help: Option<String>,
    /// The choices, for a choice.
    pub choices: Option<Vec<Choice>>,
    /// What stands in the form before the person writes.
    pub default: Option<Json>,
    /// The most bytes an answer may have.
    pub max_bytes: Option<u64>,
}

/// Why an answer does not fit its fields.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("{path}: {message}")]
pub struct FieldError {
    /// The member that is wrong.
    pub path: String,
    /// What is wrong with it, as a sentence fragment after the path.
    pub message: String,
}

impl Field {
    /// A required field of `kind`, without help, choices, default or limit.
    #[must_use]
    pub fn new(name: &str, label: &str, kind: FieldType) -> Self {
        Self {
            name: name.to_owned(),
            label: label.to_owned(),
            kind,
            required: true,
            help: None,
            choices: None,
            default: None,
            max_bytes: None,
        }
    }

    fn check(&self, value: &Json) -> Result<(), String> {
        let size = |text: &str| match self.max_bytes {
            Some(most) if text.len() as u64 > most => Err(format!("must be at most {most} bytes")),
            _ => Ok(()),
        };
        match (self.kind, value) {
            (FieldType::Text | FieldType::Secret, Json::String(text)) => {
                if text.contains('\n') {
                    return Err("must be one line".into());
                }
                size(text)
            }
            (FieldType::SecretText, Json::String(text)) => size(text),
            (FieldType::Choice, Json::String(text)) => {
                let known = self
                    .choices
                    .iter()
                    .flatten()
                    .any(|choice| &choice.value == text);
                if known {
                    Ok(())
                } else {
                    Err("must be one of the choices".into())
                }
            }
            (FieldType::Boolean, Json::Bool(_)) => Ok(()),
            (FieldType::Date, Json::String(text)) => {
                chrono::NaiveDate::parse_from_str(text, "%Y-%m-%d")
                    .map(|_| ())
                    .map_err(|_| "must be a day written YYYY-MM-DD".into())
            }
            (FieldType::Map | FieldType::SecretMap, Json::Object(map)) => {
                let mut total = 0usize;
                for (name, value) in map {
                    let Json::String(text) = value else {
                        return Err(format!("{name} must be text"));
                    };
                    total += name.len() + text.len();
                }
                match self.max_bytes {
                    Some(most) if total as u64 > most => {
                        Err(format!("must be at most {most} bytes"))
                    }
                    _ => Ok(()),
                }
            }
            (FieldType::Text | FieldType::Secret | FieldType::SecretText | FieldType::Date, _) => {
                Err("must be text".into())
            }
            (FieldType::Choice, _) => Err("must be one of the choices".into()),
            (FieldType::Boolean, _) => Err("must be true or false".into()),
            (FieldType::Map | FieldType::SecretMap, _) => Err("must be names with values".into()),
        }
    }
}

/// Checks `answer`, an object, against `fields`: every required field is
/// answered, every answer fits its field, and nothing else is given. A
/// field answered with `null` counts as not answered. Every fault is
/// returned, not only the first.
pub fn validate(fields: &[Field], answer: &Json) -> Result<(), Vec<FieldError>> {
    let empty = serde_json::Map::new();
    let given = match answer {
        Json::Object(map) => map,
        Json::Null => &empty,
        _ => {
            return Err(vec![FieldError {
                path: String::new(),
                message: "must be an object".into(),
            }]);
        }
    };
    let mut faults = Vec::new();
    for field in fields {
        match given.get(&field.name) {
            None | Some(Json::Null) => {
                if field.required && field.default.is_none() {
                    faults.push(FieldError {
                        path: field.name.clone(),
                        message: "is required".into(),
                    });
                }
            }
            Some(value) => {
                if let Err(message) = field.check(value) {
                    faults.push(FieldError {
                        path: field.name.clone(),
                        message,
                    });
                }
            }
        }
    }
    for name in given.keys() {
        if !fields.iter().any(|field| &field.name == name) {
            faults.push(FieldError {
                path: name.clone(),
                message: "is not asked for".into(),
            });
        }
    }
    if faults.is_empty() {
        Ok(())
    } else {
        Err(faults)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn fields() -> Vec<Field> {
        let mut region = Field::new("region", "Region", FieldType::Choice);
        region.choices = Some(vec![Choice {
            value: "eu".into(),
            label: "Europe".into(),
        }]);
        let mut note = Field::new("note", "Note", FieldType::Text);
        note.required = false;
        note.max_bytes = Some(8);
        vec![
            Field::new("key", "API key", FieldType::Secret),
            region,
            note,
        ]
    }

    #[test]
    fn an_answer_that_fits_is_accepted() {
        assert_eq!(
            validate(&fields(), &json!({ "key": "k", "region": "eu" })),
            Ok(())
        );
    }

    #[test]
    fn every_fault_is_reported_at_once() {
        let faults = validate(
            &fields(),
            &json!({ "region": "mars", "note": "far too long", "extra": 1 }),
        )
        .unwrap_err();
        let paths: Vec<_> = faults.iter().map(|fault| fault.path.as_str()).collect();
        assert_eq!(paths, ["key", "region", "note", "extra"]);
    }

    #[test]
    fn a_field_is_written_with_the_member_type() {
        let field = Field::new("key", "API key", FieldType::SecretText);
        let written = serde_json::to_value(&field).unwrap();
        assert_eq!(written["type"], "secret_text");
        assert!(written["choices"].is_null());
        assert!(FieldType::SecretText.is_secret());
    }
}
