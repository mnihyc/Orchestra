//! Local JSON Schema validation with field paths and refusals that omit supplied values.

use jsonschema::{PatternOptions, Retrieve, Uri, Validator};
use orch_types::Json;

/// Keeps a malformed large form's refusal bounded while refusing the entire configuration.
pub const MAX_FAULTS: usize = 64;

/// One failing rule, with no instance value, enum value or credential in its message.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Fault {
    /// The instance's JSON pointer, relative to the configuration being checked.
    pub path: String,
    /// The schema's JSON pointer to the failing rule.
    pub schema_path: String,
    /// A sentence describing the rule without quoting the instance.
    pub message: String,
}

/// A declaration or configuration that cannot be accepted whole.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum Error {
    /// A malformed schema, unsupported pattern or reference outside its own document.
    #[error("the schema cannot be used at {path}")]
    Schema {
        /// Where the declaration broke its schema's rules.
        path: String,
    },
    /// The supplied configuration fails one or more rules.
    #[error("the configuration does not meet its schema")]
    Value {
        /// At most MAX_FAULTS field failures, never a partially accepted configuration.
        faults: Vec<Fault>,
    },
}

struct LocalOnly;

impl Retrieve for LocalOnly {
    fn retrieve(
        &self,
        _uri: &Uri<String>,
    ) -> Result<Json, Box<dyn std::error::Error + Send + Sync>> {
        Err("a schema reference must resolve inside its own document".into())
    }
}

fn compile(schema: &Json) -> Result<Validator, Error> {
    jsonschema::options()
        .with_retriever(LocalOnly)
        // Linear matching gives a supplied pattern no unbounded backtracking work.
        .with_pattern_options(PatternOptions::regex())
        .should_validate_formats(true)
        .build(schema)
        .map_err(|error| Error::Schema {
            path: error.instance_path.to_string(),
        })
}

/// Checks a declaration before accepting an extension or saving a configuration that uses it.
pub fn check(schema: &Json) -> Result<(), Error> {
    compile(schema).map(|_| ())
}

/// A missing required member belongs at the member's own JSON pointer.
fn instance_path(error: &jsonschema::ValidationError<'_>) -> String {
    let mut path = error.instance_path.to_string();
    if let jsonschema::error::ValidationErrorKind::Required { property } = &error.kind
        && let Some(name) = property.as_str()
    {
        path.push('/');
        path.push_str(&name.replace('~', "~0").replace('/', "~1"));
    }
    path
}

/// Refuses the whole value on any nested rule failure. Nothing is applied or coerced.
pub fn validate(schema: &Json, value: &Json) -> Result<(), Error> {
    let validator = compile(schema)?;
    let faults: Vec<_> = validator
        .iter_errors(value)
        .take(MAX_FAULTS)
        .map(|error| {
            let schema_path = error.schema_path.to_string();
            let rule = schema_path.rsplit('/').next().unwrap_or_default();
            let message = match rule {
                "type" => "the value has the wrong type",
                "required" => "a required member is missing",
                "additionalProperties" | "unevaluatedProperties" => {
                    "the object contains a member this configuration does not accept"
                }
                "enum" | "const" => "the value is not an allowed choice",
                "minimum" | "exclusiveMinimum" | "maximum" | "exclusiveMaximum" => {
                    "the number is outside the allowed range"
                }
                "minLength" | "maxLength" => "the text has a length outside the allowed range",
                "pattern" | "format" => "the text does not have the required format",
                "minItems" | "maxItems" | "uniqueItems" => {
                    "the list does not meet the required length or uniqueness"
                }
                _ => "the value does not meet this configuration rule",
            };
            Fault {
                path: instance_path(&error),
                schema_path,
                message: message.to_owned(),
            }
        })
        .collect();
    if faults.is_empty() {
        Ok(())
    } else {
        Err(Error::Value { faults })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn nested_fields_unknown_members_and_local_references_are_validated_without_values_in_faults() {
        let schema = json!({"$defs":{"key":{"type":"string","pattern":"^[a-z]+$"}},"type":"object","additionalProperties":false,
            "properties":{"connection":{"type":"object","additionalProperties":false,"required":["key","tries"],"properties":{"key":{"$ref":"#/$defs/key"},"tries":{"type":"integer","minimum":1}}}},"required":["connection"]});
        assert!(validate(&schema, &json!({"connection":{"key":"okay","tries":2}})).is_ok());
        let error = validate(
            &schema,
            &json!({"connection":{"key":"credential-do-not-echo!","tries":0,"unexpected":true}}),
        )
        .unwrap_err();
        let Error::Value { faults } = &error else {
            panic!("the schema is valid");
        };
        assert!(faults.iter().any(|fault| fault.path == "/connection/key"));
        assert!(faults.iter().any(|fault| fault.path == "/connection/tries"));
        assert!(
            faults
                .iter()
                .any(|fault| fault.schema_path.ends_with("additionalProperties"))
        );
        assert!(!format!("{error:?}").contains("credential-do-not-echo"));
    }

    #[test]
    fn missing_required_members_point_to_the_escaped_member() {
        let schema = json!({"type":"object", "properties": {"group": {
            "type":"object", "required":["namespace", "a~/b"]}}, "required":["group"]});
        let Error::Value { faults } = validate(&schema, &json!({"group":{}})).unwrap_err() else {
            panic!("the schema is valid");
        };
        assert!(faults.iter().any(|fault| fault.path == "/group/namespace"));
        assert!(faults.iter().any(|fault| fault.path == "/group/a~0~1b"));
    }

    #[test]
    fn malformed_declarations_and_remote_or_file_references_are_refused_locally() {
        assert!(check(&json!({"type":"not-a-type"})).is_err());
        for reference in [
            "https://example.invalid/private.json",
            "file:///private/config.json",
        ] {
            let error = check(&json!({"$ref":reference})).unwrap_err();
            assert!(!error.to_string().contains(reference));
        }
        assert!(validate(&json!(true), &json!({"any":"value"})).is_ok());
        assert!(validate(&json!(false), &json!({})).is_err());
    }
}
