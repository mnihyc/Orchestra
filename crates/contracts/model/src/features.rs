//! Common request declarations are projected by the wire format's data bindings.
use orch_provider_contract::models::ModelParameters;
use orch_types::Json;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::{Map, json};
use std::collections::BTreeSet;

pub const COMMON: &str = "common";
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct FeatureField {
    pub common: String,
    /// Relative native JSON pointer; absent for a factual fixed member.
    pub path: Option<String>,
    pub default: Option<Json>,
    pub choices: Option<Vec<Json>>,
    pub schema: Json,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct FeatureBinding {
    pub feature: String,
    /// The first non-wildcard path can carry an authored default when no instance exists.
    pub paths: Vec<String>,
    pub fields: Vec<FeatureField>,
    /// Native members identifying this feature's active form.
    pub discriminator: Json,
    /// Native members required when a new instance is made.
    pub native_defaults: Json,
}
#[derive(Debug)]
pub struct FeatureError(String);
impl std::fmt::Display for FeatureError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}
impl std::error::Error for FeatureError {}
fn fail(message: impl Into<String>) -> FeatureError {
    FeatureError(message.into())
}
fn field(
    common: &str,
    path: &str,
    default: Option<Json>,
    choices: Option<Vec<Json>>,
) -> FeatureField {
    FeatureField {
        common: common.into(),
        path: Some(path.into()),
        default,
        choices,
        schema: json!({"type":if common=="schema"{"object"}else if common=="strict"{"boolean"}else{"string"}}),
    }
}
/// Built-ins name interfaces, independently of the vendor hosting an endpoint.
pub fn builtin(protocol: &str) -> Vec<FeatureBinding> {
    use crate::protocol::*;
    let (cache, output) = match protocol {
        OPENAI_RESPONSES | OPENAI_CHAT => (
            FeatureBinding {
                feature: "cache".into(),
                paths: vec![String::new()],
                fields: vec![
                    field("key", "/prompt_cache_key", None, None),
                    field(
                        "retention",
                        "/prompt_cache_retention",
                        None,
                        Some(vec![json!("in_memory"), json!("24h")]),
                    ),
                ],
                discriminator: json!({}),
                native_defaults: json!({}),
            },
            FeatureBinding {
                feature: "structured_output".into(),
                paths: vec![
                    if protocol == OPENAI_RESPONSES {
                        "/text/format"
                    } else {
                        "/response_format"
                    }
                    .into(),
                ],
                fields: if protocol == OPENAI_RESPONSES {
                    vec![
                        field("schema", "/schema", None, None),
                        field("name", "/name", Some(json!("output")), None),
                        field("strict", "/strict", Some(json!(false)), None),
                    ]
                } else {
                    vec![
                        field("schema", "/json_schema/schema", None, None),
                        field("name", "/json_schema/name", Some(json!("output")), None),
                        field("strict", "/json_schema/strict", Some(json!(false)), None),
                    ]
                },
                discriminator: json!({"type":"json_schema"}),
                native_defaults: json!({"type":"json_schema"}),
            },
        ),
        ANTHROPIC_MESSAGES => (
            FeatureBinding {
                feature: "cache".into(),
                paths: vec![
                    "/cache_control".into(),
                    "/system/*/cache_control".into(),
                    "/tools/*/cache_control".into(),
                    "/messages/*/content/*/cache_control".into(),
                    "/messages/*/content/*/content/*/cache_control".into(),
                ],
                fields: vec![field(
                    "retention",
                    "/ttl",
                    Some(json!("5m")),
                    Some(vec![json!("5m"), json!("1h")]),
                )],
                discriminator: json!({"type":"ephemeral"}),
                native_defaults: json!({"type":"ephemeral"}),
            },
            FeatureBinding {
                feature: "structured_output".into(),
                paths: vec!["/output_config/format".into()],
                fields: vec![
                    field("schema", "/schema", None, None),
                    FeatureField {
                        common: "name".into(),
                        path: None,
                        default: None,
                        choices: None,
                        schema: json!({"type":"string"}),
                    },
                    FeatureField {
                        common: "strict".into(),
                        path: None,
                        default: Some(json!(true)),
                        choices: Some(vec![json!(true)]),
                        schema: json!({"type":"boolean"}),
                    },
                ],
                discriminator: json!({"type":"json_schema"}),
                native_defaults: json!({"type":"json_schema"}),
            },
        ),
        _ => return Vec::new(),
    };
    vec![cache, output]
}
fn segments(pointer: &str) -> Vec<String> {
    pointer
        .trim_start_matches('/')
        .split('/')
        .filter(|part| !part.is_empty())
        .map(|part| part.replace("~1", "/").replace("~0", "~"))
        .collect()
}
fn pointer(parts: &[String]) -> String {
    parts
        .iter()
        .map(|part| format!("/{}", part.replace('~', "~0").replace('/', "~1")))
        .collect()
}
fn locations(value: &Json, parts: &[String], prefix: &mut Vec<String>, out: &mut Vec<Vec<String>>) {
    let Some((first, rest)) = parts.split_first() else {
        out.push(prefix.clone());
        return;
    };
    if first == "*" {
        if let Some(values) = value.as_array() {
            for (index, value) in values.iter().enumerate() {
                prefix.push(index.to_string());
                locations(value, rest, prefix, out);
                prefix.pop();
            }
        }
    } else if let Some(value) = value.get(first).or_else(|| {
        first
            .parse::<usize>()
            .ok()
            .and_then(|index| value.get(index))
    }) {
        prefix.push(first.clone());
        locations(value, rest, prefix, out);
        prefix.pop();
    }
}
fn put(value: &mut Json, parts: &[String], new: Option<Json>) -> Result<(), FeatureError> {
    let Some((first, rest)) = parts.split_first() else {
        if let Some(new) = new {
            *value = new;
        }
        return Ok(());
    };
    if let Some(array) = value.as_array_mut() {
        let index = first
            .parse::<usize>()
            .map_err(|_| fail("a feature path has an invalid array position"))?;
        let item = array
            .get_mut(index)
            .ok_or_else(|| fail("a feature path leaves the existing input array"))?;
        return put(item, rest, new);
    }
    if value.is_null() {
        *value = json!({});
    }
    let object = value
        .as_object_mut()
        .ok_or_else(|| fail("a feature path crosses a non-object value"))?;
    if rest.is_empty() {
        if let Some(new) = new {
            object.insert(first.clone(), new);
        } else {
            object.remove(first);
        }
        return Ok(());
    }
    if new.is_none() && !object.contains_key(first) {
        return Ok(());
    }
    put(
        object.entry(first.clone()).or_insert_with(|| json!({})),
        rest,
        new,
    )
}
fn projection(binding: &FeatureBinding, native: &Json) -> Option<Json> {
    if native.is_null() {
        return Some(Json::Null);
    }
    let discriminator = binding.discriminator.as_object();
    if discriminator.is_some_and(|fields| {
        fields
            .iter()
            .any(|(key, wanted)| native.get(key) != Some(wanted))
    }) {
        return Some(Json::Null);
    }
    let mut value = Map::new();
    for field in &binding.fields {
        if let Some(path) = &field.path {
            if let Some(found) = native.pointer(path) {
                value.insert(field.common.clone(), found.clone());
            }
        } else if let Some(fixed) = &field.default {
            value.insert(field.common.clone(), fixed.clone());
        }
    }
    if value.is_empty() && discriminator.is_none_or(Map::is_empty) {
        None
    } else {
        Some(Json::Object(value))
    }
}
fn has_default(root: &Json, rule: &Json, feature: &str, seen: &mut BTreeSet<String>) -> bool {
    if ["default", "const"].into_iter().any(|key| {
        rule.get(key)
            .is_some_and(|value| value.get(feature).is_some())
    }) {
        return true;
    }
    if rule
        .get("properties")
        .and_then(|properties| properties.get(feature))
        .is_some_and(|rule| rule.get("default").is_some() || rule.get("const").is_some())
    {
        return true;
    }
    if let Some(reference) = rule.get("$ref").and_then(Json::as_str)
        && seen.insert(reference.into())
        && let Some(rule) = reference
            .strip_prefix('#')
            .and_then(|path| root.pointer(path))
        && has_default(root, rule, feature, seen)
    {
        return true;
    }
    rule.get("allOf")
        .and_then(Json::as_array)
        .is_some_and(|rules| {
            rules
                .iter()
                .any(|rule| has_default(root, rule, feature, &mut seen.clone()))
        })
}
fn finish_value(binding: &FeatureBinding, value: &mut Json) -> Result<(), FeatureError> {
    if value.is_null() {
        return Ok(());
    }
    let object = value
        .as_object_mut()
        .ok_or_else(|| fail(format!("{} needs an object or null", binding.feature)))?;
    for key in object.keys() {
        if !binding.fields.iter().any(|field| &field.common == key) {
            return Err(fail(format!(
                "this interface cannot represent {}.{key}",
                binding.feature
            )));
        }
    }
    for field in &binding.fields {
        if !object.contains_key(&field.common)
            && let Some(default) = &field.default
        {
            object.insert(field.common.clone(), default.clone());
        }
        if let Some(value) = object.get(&field.common) {
            orch_types::schema::validate(&field.schema, value)
                .map_err(|error| fail(format!("{}.{}: {error}", binding.feature, field.common)))?;
            if field
                .choices
                .as_ref()
                .is_some_and(|choices| !choices.contains(value))
            {
                return Err(fail(format!(
                    "this interface cannot represent that {}.{} value",
                    binding.feature, field.common
                )));
            }
            if field.path.is_none()
                && field.default.is_some()
                && field.default.as_ref() != Some(value)
            {
                return Err(fail(format!(
                    "this interface has a fixed {}.{} value",
                    binding.feature, field.common
                )));
            }
        }
    }
    if binding.feature == "structured_output" && !object.get("schema").is_some_and(Json::is_object)
    {
        return Err(fail("structured_output.schema needs its JSON schema"));
    }
    Ok(())
}
fn write(
    binding: &FeatureBinding,
    body: &mut Json,
    path: &[String],
    value: &Json,
) -> Result<(), FeatureError> {
    if value.is_null() {
        if !path.is_empty() {
            return put(body, path, None);
        }
        for field in &binding.fields {
            if let Some(native) = &field.path {
                put(body, &segments(native), None)?;
            }
        }
        return Ok(());
    }
    let mut native = body
        .pointer(&pointer(path))
        .cloned()
        .unwrap_or_else(|| json!({}));
    if !native.is_object() {
        native = json!({});
    }
    if let Some(defaults) = binding.native_defaults.as_object() {
        for (key, value) in defaults {
            native[key] = value.clone();
        }
    }
    for field in &binding.fields {
        if let Some(path) = &field.path
            && let Some(value) = value.get(&field.common)
        {
            put(&mut native, &segments(path), Some(value.clone()))?;
        }
    }
    put(body, path, Some(native))
}
/// The same common declaration governs native SDK fields and explicit per-thread common choices.
pub fn apply(
    bindings: &[FeatureBinding],
    model: &ModelParameters,
    bytes: &[u8],
    choices: Option<&Json>,
) -> Result<Vec<u8>, FeatureError> {
    let Some(schema) = model.option_schema(COMMON) else {
        if choices.is_some_and(|choices| {
            choices
                .as_object()
                .is_none_or(|choices| !choices.is_empty())
        }) {
            return Err(fail(
                "common request choices are not declared by this model",
            ));
        }
        return Ok(bytes.to_vec());
    };
    let mut body: Json = serde_json::from_slice(bytes).map_err(|error| fail(error.to_string()))?;
    let mut actual = Vec::new();
    let mut common = json!({});
    for binding in bindings {
        for path in &binding.paths {
            let mut found = Vec::new();
            locations(&body, &segments(path), &mut Vec::new(), &mut found);
            for path in found {
                if let Some(value) = body
                    .pointer(&pointer(&path))
                    .and_then(|native| projection(binding, native))
                {
                    common
                        .as_object_mut()
                        .ok_or_else(|| fail("common request values need an object"))?
                        .entry(binding.feature.clone())
                        .or_insert_with(|| value.clone());
                    actual.push((binding, path, value));
                }
            }
        }
    }
    let original = common.clone();
    if let Some(choices) = choices {
        for (key, value) in choices
            .as_object()
            .ok_or_else(|| fail("common request choices need an object"))?
        {
            common[key] = value.clone();
        }
    }
    crate::parameters::schema_defaults(schema, &mut common);
    // A default for a nested name cannot enable a previously absent output mode.
    for binding in bindings {
        if original.get(&binding.feature).is_none()
            && choices
                .and_then(|choices| choices.get(&binding.feature))
                .is_none()
            && !has_default(schema, schema, &binding.feature, &mut BTreeSet::new())
        {
            common
                .as_object_mut()
                .ok_or_else(|| fail("common request values need an object"))?
                .remove(&binding.feature);
        }
    }
    for binding in bindings {
        if original.get(&binding.feature).is_some_and(Json::is_null)
            && choices
                .and_then(|choices| choices.get(&binding.feature))
                .is_none()
        {
            common
                .as_object_mut()
                .ok_or_else(|| fail("common request values need an object"))?
                .remove(&binding.feature);
        }
    }
    for key in common
        .as_object()
        .ok_or_else(|| fail("common request values need an object"))?
        .keys()
    {
        if !bindings.iter().any(|binding| &binding.feature == key) {
            return Err(fail(format!(
                "this interface cannot represent common feature {key}"
            )));
        }
    }
    for binding in bindings {
        if let Some(value) = common.get_mut(&binding.feature) {
            finish_value(binding, value)?;
        }
    }
    orch_types::schema::validate(schema, &common)
        .map_err(|error| fail(format!("common request choices: {error}")))?;
    let mut one = common.clone();
    for (binding, path, before) in actual.iter() {
        if before.is_null() && common.get(&binding.feature).is_none() {
            if body.pointer(&pointer(path)).is_some_and(Json::is_null) && !path.is_empty() {
                put(&mut body, path, None)?;
            }
            continue;
        }
        let mut local = before.clone();
        if let Some(choice) = choices.and_then(|choices| choices.get(&binding.feature)) {
            local = choice.clone();
        }
        if let Some(start) = schema
            .get("const")
            .or_else(|| schema.get("default"))
            .and_then(|root| root.get(&binding.feature))
        {
            crate::parameters::fill_schema_defaults(
                schema,
                &json!({"default":start}),
                &mut local,
                &mut BTreeSet::new(),
            );
        }
        if let Some(rule) = model.option_rule(COMMON, &binding.feature) {
            if let Some(replacement) =
                crate::parameters::schema_replacement(schema, rule, &mut BTreeSet::new())
            {
                local = replacement.clone();
            }
            crate::parameters::fill_schema_defaults(schema, rule, &mut local, &mut BTreeSet::new());
        }
        one[&binding.feature] = local;
        let value = one
            .get_mut(&binding.feature)
            .ok_or_else(|| fail("the common feature disappeared"))?;
        finish_value(binding, value)?;
        orch_types::schema::validate(schema, &one)
            .map_err(|error| fail(format!("common {}: {error}", binding.feature)))?;
        let value = &one[&binding.feature];
        if value != before {
            write(binding, &mut body, path, value)?;
        }
    }
    for binding in bindings {
        if actual
            .iter()
            .any(|(actual, _, _)| actual.feature == binding.feature)
        {
            continue;
        }
        if let Some(value) = common.get(&binding.feature) {
            let path = binding
                .paths
                .iter()
                .find(|path| !segments(path).iter().any(|part| part == "*"))
                .ok_or_else(|| {
                    fail(format!(
                        "this interface has no place for a default {}",
                        binding.feature
                    ))
                })?;
            write(binding, &mut body, &segments(path), value)?;
        }
    }
    serde_json::to_vec(&body).map_err(|error| fail(error.to_string()))
}

/// Ephemeral native validation operands after the common layer was checked exactly once.
pub struct Prepared<'a> {
    pub model: std::borrow::Cow<'a, ModelParameters>,
    pub body: std::borrow::Cow<'a, [u8]>,
}
fn augment_at(
    root: &Json,
    rule: &mut Json,
    parts: &[String],
    leaf: &Json,
    seen: &mut BTreeSet<String>,
) {
    if rule.is_boolean() {
        return;
    }
    if let Some(reference) = rule.get("$ref").and_then(Json::as_str).map(str::to_owned)
        && seen.insert(reference.clone())
        && let Some(referenced) = reference
            .strip_prefix('#')
            .and_then(|path| root.pointer(path))
    {
        let mut referenced = referenced.clone();
        augment_at(root, &mut referenced, parts, leaf, seen);
        if let Some(own) = rule.as_object_mut() {
            own.remove("$ref");
        }
        augment_at(root, rule, parts, leaf, seen);
        if let Some(own) = rule.as_object_mut() {
            let all = own.entry("allOf").or_insert_with(|| json!([]));
            if let Some(all) = all.as_array_mut() {
                all.push(referenced);
            }
        }
        seen.remove(&reference);
        return;
    }
    if let Some(rules) = rule.get_mut("allOf").and_then(Json::as_array_mut) {
        for rule in rules {
            augment_at(root, rule, parts, leaf, &mut seen.clone());
        }
    }
    let Some((first, rest)) = parts.split_first() else {
        return;
    };
    if first == "*" {
        if let Some(items) = rule.get_mut("items") {
            augment_at(root, items, rest, leaf, seen);
        }
        return;
    }
    let closed = rule.get("additionalProperties") == Some(&Json::Bool(false));
    let Some(rule) = rule.as_object_mut() else {
        return;
    };
    let properties = rule.entry("properties").or_insert_with(|| json!({}));
    let Some(properties) = properties.as_object_mut() else {
        return;
    };
    if rest.is_empty() {
        properties
            .entry(first.clone())
            .or_insert_with(|| leaf.clone());
        return;
    }
    let child = properties.entry(first.clone()).or_insert_with(|| {
        if closed {
            json!({"type":"object","properties":{},"additionalProperties":false})
        } else {
            json!({"type":"object","properties":{}})
        }
    });
    augment_at(root, child, rest, leaf, seen);
}
/// Extend the existing native schemas only with features actually declared in the common layer.
/// Original explicit blocks and every native value constraint remain in force.
pub fn prepare<'a>(
    protocol: &str,
    bindings: &[FeatureBinding],
    model: &'a ModelParameters,
    body: &'a [u8],
    choices: Option<&Json>,
) -> Result<Prepared<'a>, FeatureError> {
    use std::borrow::Cow;
    let Some(common) = model.option_schema(COMMON) else {
        if choices.is_some_and(|choices| {
            choices
                .as_object()
                .is_none_or(|choices| !choices.is_empty())
        }) {
            return Err(fail(
                "common request choices are not declared by this model",
            ));
        }
        return Ok(Prepared {
            model: Cow::Borrowed(model),
            body: Cow::Borrowed(body),
        });
    };
    let bytes = apply(bindings, model, body, choices)?;
    let mut native = crate::parameters::constraints(protocol, model);
    for binding in bindings {
        if model.option_rule(COMMON, &binding.feature).is_none()
            && !has_default(common, common, &binding.feature, &mut BTreeSet::new())
        {
            continue;
        }
        for path in &binding.paths {
            for field in &binding.fields {
                if let Some(field_path) = &field.path {
                    let mut target = segments(path);
                    target.extend(segments(field_path));
                    let root = native.clone();
                    augment_at(
                        &root,
                        &mut native,
                        &target,
                        &field.schema,
                        &mut BTreeSet::new(),
                    );
                }
            }
            for (key, value) in binding.discriminator.as_object().into_iter().flatten() {
                let mut target = segments(path);
                target.push(key.clone());
                let kind = if value.is_string() {
                    "string"
                } else if value.is_boolean() {
                    "boolean"
                } else {
                    "object"
                };
                let root = native.clone();
                augment_at(
                    &root,
                    &mut native,
                    &target,
                    &json!({"type":kind}),
                    &mut BTreeSet::new(),
                );
            }
        }
    }
    let mut model = model.clone();
    let options = model.request_options.get_or_insert_with(Default::default);
    options.remove(COMMON);
    options.insert(protocol.to_owned(), native);
    Ok(Prepared {
        model: Cow::Owned(model),
        body: Cow::Owned(bytes),
    })
}
