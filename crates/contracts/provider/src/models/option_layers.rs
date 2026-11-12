//! One effective form with independently scoped original validation schemas.
use orch_types::Json;
use serde_json::{Map, json};
use std::collections::BTreeSet;

/// The same root-reference walk used by the composed form, without copying literal option data.
pub(super) fn option_rule<'a>(root: &'a Json, key: &str) -> Option<&'a Json> {
    let mut fragments = Vec::new();
    form_fragments(root, root, &mut BTreeSet::new(), &mut fragments);
    fragments
        .into_iter()
        .rev()
        .find_map(|schema| schema.get("properties")?.get(key))
}

pub(super) fn compose(layers: &[&Json]) -> Json {
    if let [only] = layers {
        return (*only).clone();
    }
    let mut form = Map::new();
    let mut properties = Map::new();
    let mut required = BTreeSet::new();
    let mut definitions = Map::new();
    let mut checks = Vec::with_capacity(layers.len());
    for (index, root) in layers.iter().enumerate() {
        let name = format!("option_layer_{index}");
        let prefix = format!("#/$defs/{name}");
        definitions.insert(name, relocated(root, &prefix));
        checks.push(json!({"$ref":prefix}));
        let mut fragments = Vec::new();
        form_fragments(root, root, &mut BTreeSet::new(), &mut fragments);
        for fragment in fragments {
            let relocated = relocated(fragment, &prefix);
            let Some(object) = relocated.as_object() else {
                continue;
            };
            for (key, value) in object {
                match key.as_str() {
                    "properties" => {
                        if let Some(values) = value.as_object() {
                            properties.extend(values.clone());
                        }
                    }
                    "required" => {
                        if let Some(values) = value.as_array() {
                            required
                                .extend(values.iter().filter_map(Json::as_str).map(str::to_owned));
                        }
                    }
                    "$ref" | "$defs" | "definitions" | "allOf" => {}
                    _ => {
                        form.insert(key.clone(), value.clone());
                    }
                }
            }
        }
    }
    form.insert("properties".into(), Json::Object(properties));
    if !required.is_empty() {
        form.insert("required".into(), json!(required));
    }
    form.insert("$defs".into(), Json::Object(definitions));
    form.insert("allOf".into(), Json::Array(checks));
    // Keep original local roots for the existing protocol constraint normalizer.
    form.insert("x-orch-option-layers".into(), json!(layers));
    Json::Object(form)
}

/// A local reference may supply the root form; siblings retain their normal later form precedence.
fn form_fragments<'a>(
    root: &'a Json,
    schema: &'a Json,
    seen: &mut BTreeSet<String>,
    out: &mut Vec<&'a Json>,
) {
    let mut chain = Vec::new();
    let mut current = schema;
    loop {
        chain.push(current);
        let Some(reference) = current.get("$ref").and_then(Json::as_str) else {
            break;
        };
        let Some(pointer) = reference.strip_prefix('#') else {
            break;
        };
        if !seen.insert(reference.to_owned()) {
            break;
        }
        let Some(target) = root.pointer(pointer) else {
            break;
        };
        current = target;
    }
    out.extend(chain.into_iter().rev());
    seen.clear();
}

/// Local references keep their original schema root, including colliding definition names.
/// Literal option data in const/default/enum/examples is never traversed as a schema.
fn relocated(schema: &Json, prefix: &str) -> Json {
    let mut value = schema.clone();
    let Some(object) = value.as_object_mut() else {
        return value;
    };
    if let Some(reference) = object.get("$ref").and_then(Json::as_str)
        && let Some(pointer) = reference.strip_prefix('#')
    {
        object.insert("$ref".into(), json!(format!("{prefix}{pointer}")));
    }
    for key in [
        "properties",
        "$defs",
        "definitions",
        "patternProperties",
        "dependentSchemas",
    ] {
        if let Some(fields) = object.get_mut(key).and_then(Json::as_object_mut) {
            for schema in fields.values_mut() {
                *schema = relocated(schema, prefix);
            }
        }
    }
    for key in [
        "items",
        "additionalProperties",
        "contains",
        "propertyNames",
        "not",
        "if",
        "then",
        "else",
    ] {
        if let Some(schema) = object.get_mut(key) {
            *schema = relocated(schema, prefix);
        }
    }
    for key in ["allOf", "anyOf", "oneOf", "prefixItems"] {
        if let Some(schemas) = object.get_mut(key).and_then(Json::as_array_mut) {
            for schema in schemas {
                *schema = relocated(schema, prefix);
            }
        }
    }
    value
}
