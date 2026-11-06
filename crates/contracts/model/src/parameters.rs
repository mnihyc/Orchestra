//! Existing model defaults and explicit endpoint-format options for model requests.

use crate::protocol::{
    ANTHROPIC_MESSAGES, OPENAI_CHAT, OPENAI_EMBEDDINGS, OPENAI_RESPONSES, parameter_format,
};
use orch_provider_contract::{calls, models::ModelParameters};
use orch_types::Json;
use serde_json::{Map, json};

/// Read the actual wire field, keeping Anthropic's capacity tier separate from inference speed.
#[must_use]
pub fn requested_service_tier(protocol: &str, params: &Json) -> Option<String> {
    let (field, _) = parameter_format(protocol).tier?;
    params.get(field).and_then(Json::as_str).map(str::to_owned)
}

/// These text formats declare a response tier/speed field the observer can read.
/// This is decoder support, not a claim that any model offers a paid speed.
#[must_use]
pub fn reports_service_tier(protocol: &str) -> bool {
    parameter_format(protocol).tier.is_some()
}

/// Documented aliases in the OpenAI text formats, only for resolving an absent rate multiplier.
/// The actual reported value remains separate and is never replaced by this alias.
#[must_use]
pub fn service_tier_price_alias(protocol: &str, reported: &str) -> Option<&'static str> {
    parameter_format(protocol)
        .tier_aliases
        .iter()
        .find_map(|(tier, alias)| {
            if *tier == reported {
                Some(*alias)
            } else if *alias == reported {
                Some(*tier)
            } else {
                None
            }
        })
}

/// The requested processing class used by existing tier caps, not an assertion about the answer.
#[must_use]
pub fn requested_tier_for_caps(protocol: &str, params: &Json) -> Option<String> {
    requested_service_tier(protocol, params).and_then(|tier| match tier.as_str() {
        "default" | "standard" | "auto" => None,
        _ => Some(
            parameter_format(protocol)
                .tier_aliases
                .iter()
                .find_map(|(reported, alias)| (*alias == tier).then_some(*reported))
                .map_or(tier, str::to_owned),
        ),
    })
}

/// Endpoint defaults are least specific, then the model's defaults, then this use's options.
/// Normalize effort aliases at each layer so an explicit value can replace a default alias.
pub fn resolve(
    protocol: &str,
    model: &ModelParameters,
    endpoint: &Json,
    selected: &Json,
) -> Result<Json, &'static str> {
    let format = parameter_format(protocol);
    let mut model_defaults = json!({});
    if let Some(effort) = &model.default_effort {
        model_defaults["effort"] = json!(effort);
    }
    if let Some(tier) = model.service_tier_start() {
        let (field, _) = format
            .tier
            .ok_or("this protocol has not declared model service tiers")?;
        model_defaults[field] = json!(tier);
    }
    if let Some(program) = model.access_program_start() {
        let (field, member, _) = format
            .program
            .ok_or("this protocol has not declared model access programs")?;
        model_defaults[field] = json!({(member):program});
    }
    if let Some(field) = format.web_search
        && model.default_web_search.is_some()
    {
        model_defaults[field] = json!(model.web_search_start());
    }
    let mut body = apply(protocol, b"{}", endpoint, false)?;
    body = apply(protocol, &body, &model_defaults, false)?;
    body = apply(protocol, &body, selected, false)?;
    let params: Json = serde_json::from_slice(&body).map_err(|_| "model options cannot be read")?;
    constrain(protocol, model, &params)
}

/// Fill only absent properties. Explicit null is a value; a default never overwrites a choice.
/// A const also supplies an absent fixed value, so omission cannot defer it to an upstream default.
fn schema_defaults(schema: &Json, value: &mut Json) {
    fill_schema_defaults(
        schema,
        schema,
        value,
        &mut std::collections::BTreeSet::new(),
    );
}

fn schema_start<'a>(
    root: &'a Json,
    rule: &'a Json,
    seen: &mut std::collections::BTreeSet<String>,
) -> Option<&'a Json> {
    if let Some(start) = rule.get("const").or_else(|| rule.get("default")) {
        return Some(start);
    }
    let reference = rule.get("$ref")?.as_str()?;
    if !seen.insert(reference.to_owned()) {
        return None;
    }
    let result = reference
        .strip_prefix('#')
        .and_then(|pointer| root.pointer(pointer))
        .and_then(|rule| schema_start(root, rule, seen));
    seen.remove(reference);
    result
}

fn fill_schema_defaults(
    root: &Json,
    schema: &Json,
    value: &mut Json,
    seen: &mut std::collections::BTreeSet<String>,
) {
    if let Some(Json::Object(start)) = schema.get("const").or_else(|| schema.get("default"))
        && let Some(object) = value.as_object_mut()
    {
        overlay(object, start.clone(), true);
    }
    if let Some(reference) = schema.get("$ref").and_then(Json::as_str)
        && let Some(rule) = reference
            .strip_prefix('#')
            .and_then(|pointer| root.pointer(pointer))
        && seen.insert(reference.to_owned())
    {
        fill_schema_defaults(root, rule, value, seen);
        seen.remove(reference);
    }
    if let Some(items) = schema.get("items")
        && let Some(values) = value.as_array_mut()
    {
        for value in values {
            fill_schema_defaults(root, items, value, seen);
        }
    }
    let Some(properties) = schema.get("properties").and_then(Json::as_object) else {
        return;
    };
    let Some(object) = value.as_object_mut() else {
        return;
    };
    for (key, rule) in properties {
        if !object.contains_key(key) {
            if let Some(start) = schema_start(root, rule, &mut std::collections::BTreeSet::new()) {
                object.insert(key.clone(), start.clone());
            } else if rule.get("properties").is_some() || rule.get("$ref").is_some() {
                let mut child = json!({});
                fill_schema_defaults(root, rule, &mut child, seen);
                if child.as_object().is_some_and(|child| !child.is_empty()) {
                    object.insert(key.clone(), child);
                }
            }
        }
        if let Some(value) = object.get_mut(key) {
            fill_schema_defaults(root, rule, value, seen);
        }
    }
}

/// Validate actual resolved values, never attempt to prove that one schema implies another.
/// The same schema is projected to downstream forms. Protocol-owned members are not option values.
pub fn constrain(
    protocol: &str,
    model: &ModelParameters,
    params: &Json,
) -> Result<Json, &'static str> {
    let mut params = params.clone();
    if let Some(field) = parameter_format(protocol).web_search
        && let Some(search) = params.get_mut(field)
    {
        *search = Json::Object(orch_provider_contract::search::options(search)?);
    }
    if let Some(schema) = model.option_schema(protocol) {
        schema_defaults(schema, &mut params);
    }
    // A schema cannot reopen a disabled known tier/program.
    let params = with_model_defaults(protocol, model, &params)?;
    let params: Json = serde_json::from_slice(&apply(protocol, b"{}", &params, false)?)
        .map_err(|_| "model options cannot be read")?;
    let schema = constraints(protocol, model);
    orch_types::schema::validate(&schema, &params)
        .map_err(|_| "model options exceed the provider's allowed values")?;
    Ok(params)
}

/// Effective wire options for dispatch and forms: declared lists plus the manager's schema.
#[must_use]
pub fn constraints(protocol: &str, model: &ModelParameters) -> Json {
    let mut schema = model
        .option_schema(protocol)
        .cloned()
        .unwrap_or_else(|| json!({"type":"object"}));
    let declared = schema.clone();
    let Some(object) = schema.as_object_mut() else {
        return json!(false);
    };
    let properties = object.entry("properties").or_insert_with(|| json!({}));
    let Some(properties) = properties.as_object_mut() else {
        return json!(false);
    };
    let format = parameter_format(protocol);
    if let Some(field) = format.web_search
        && let Some(search) = crate::web_search::declared(protocol)
    {
        let mut intrinsic = search.options_schema.clone();
        intrinsic["properties"]["mode"] = json!({"type":"string"});
        intrinsic["default"] = json!({"mode":model.web_search_start()});
        let rule = properties.entry(field).or_insert_with(|| json!({}));
        if rule.as_bool() == Some(true) {
            *rule = json!({});
        }
        if let (Some(rule), Some(intrinsic)) = (rule.as_object_mut(), intrinsic.as_object()) {
            overlay(rule, intrinsic.clone(), true);
            if let Some(mode) = rule
                .get_mut("properties")
                .and_then(Json::as_object_mut)
                .map(|properties| properties.entry("mode").or_insert_with(|| json!({})))
            {
                let mut values = vec![json!("disabled")];
                values.extend(
                    model
                        .web_search_modes()
                        .iter()
                        .filter(|mode| search.modes.contains_key(*mode))
                        .map(|mode| json!(mode)),
                );
                narrow_choices(
                    mode,
                    &values,
                    json!(model.web_search_start()),
                    &declared,
                    &format!("/properties/{field}/properties/mode"),
                );
            }
            if model.web_search_modes().is_empty()
                && declared
                    .get("properties")
                    .and_then(|properties| properties.get(field))
                    .is_none()
            {
                rule.insert("x-orch-hidden".to_owned(), json!(true));
            }
        }
    }
    if let Some((field, ordinary)) = format.tier {
        let mut values = vec![json!(ordinary)];
        for tier in model.service_tiers() {
            if tier.id != "auto" && !values.contains(&json!(tier.id)) {
                values.push(json!(tier.id));
            }
            for (reported, alias) in format.tier_aliases {
                let alternate = if tier.id == *reported {
                    Some(*alias)
                } else if tier.id == *alias {
                    Some(*reported)
                } else {
                    None
                };
                if let Some(alternate) = alternate
                    && !values.contains(&json!(alternate))
                {
                    values.push(json!(alternate));
                }
            }
        }
        let rule = properties.entry(field).or_insert_with(|| json!({}));
        narrow_choices(
            rule,
            &values,
            json!(model.service_tier_start().unwrap_or(ordinary)),
            &declared,
            &format!("/properties/{field}"),
        );
        if model.service_tiers().is_empty()
            && declared
                .get("properties")
                .and_then(|properties| properties.get(field))
                .is_none()
        {
            rule["x-orch-hidden"] = json!(true);
        }
    }
    if let Some((field, member, ordinary)) = format.program {
        let programs = properties
            .entry(field)
            .or_insert_with(|| json!({"type":"object"}));
        if model.access_programs().is_empty()
            && declared
                .get("properties")
                .and_then(|properties| properties.get(field))
                .is_none()
        {
            programs["x-orch-hidden"] = json!(true);
        }
        if let Some(programs) = programs.as_object_mut() {
            let nested = programs.entry("properties").or_insert_with(|| json!({}));
            if let Some(nested) = nested.as_object_mut() {
                let mut values = vec![json!(ordinary)];
                values.extend(model.access_programs().iter().map(|program| json!(program)));
                narrow_choices(
                    nested.entry(member).or_insert_with(|| json!({})),
                    &values,
                    json!(model.access_program_start().unwrap_or(ordinary)),
                    &declared,
                    &format!("/properties/{field}/properties/{member}"),
                );
            }
        }
    }
    if let Some((field, member)) = format.effort {
        let mut values = vec![Json::Null];
        values.extend(model.efforts().iter().map(|effort| json!(effort)));
        let (parent, field) = member.map_or((None, field), |member| (Some(field), member));
        let pointer = parent.map_or_else(
            || format!("/properties/{field}"),
            |parent| format!("/properties/{parent}/properties/{field}"),
        );
        let fields = if let Some(parent) = parent {
            let nested = properties
                .entry(parent)
                .or_insert_with(|| json!({"type":"object"}));
            if model.efforts().is_empty()
                && declared
                    .get("properties")
                    .and_then(|properties| properties.get(parent))
                    .is_none()
            {
                nested["x-orch-hidden"] = json!(true);
            }
            nested.as_object_mut().and_then(|object| {
                object
                    .entry("properties")
                    .or_insert_with(|| json!({}))
                    .as_object_mut()
            })
        } else {
            Some(properties)
        };
        if let Some(fields) = fields {
            let rule = fields.entry(field).or_insert_with(|| json!({}));
            narrow_choices(
                rule,
                &values,
                json!(model.default_effort),
                &declared,
                &pointer,
            );
            if model.efforts().is_empty() && declared.pointer(&pointer).is_none() {
                rule["x-orch-hidden"] = json!(true);
            }
        }
    }
    mark_provider_source(&mut schema);
    schema
}

fn mark_provider_source(schema: &mut Json) {
    if let Some(object) = schema.as_object_mut() {
        object.insert("x-orch-source".to_owned(), json!("provider"));
        for name in ["properties", "$defs"] {
            if let Some(properties) = object.get_mut(name).and_then(Json::as_object_mut) {
                for rule in properties.values_mut() {
                    mark_provider_source(rule);
                }
            }
        }
        if let Some(items) = object.get_mut("items") {
            mark_provider_source(items);
        }
    }
}

/// Project the supported text/vector protocol constraints and any configured extension schemas.
#[must_use]
pub fn constraint_catalogue(model: &ModelParameters) -> std::collections::BTreeMap<String, Json> {
    let mut protocols = vec![
        OPENAI_CHAT,
        OPENAI_RESPONSES,
        ANTHROPIC_MESSAGES,
        OPENAI_EMBEDDINGS,
    ];
    if let Some(schemas) = &model.request_options {
        protocols.extend(
            schemas
                .keys()
                .map(String::as_str)
                .filter(|name| *name != "native"),
        );
    }
    let mut catalogue: std::collections::BTreeMap<_, _> = protocols
        .into_iter()
        .map(|protocol| (protocol.to_owned(), constraints(protocol, model)))
        .collect();
    if let Some(native) = model.option_schema("native") {
        let mut native = native.clone();
        mark_provider_source(&mut native);
        catalogue.insert("native".to_owned(), native);
    }
    catalogue
}

fn narrow_choices(rule: &mut Json, values: &[Json], start: Json, declared: &Json, pointer: &str) {
    if rule.as_bool() == Some(true) {
        *rule = json!({});
    }
    if !rule.is_object() {
        return;
    }
    let choices: Vec<_> = values
        .iter()
        .filter(|value| {
            declared.pointer(pointer).is_none()
                || orch_types::schema::allows_at(declared, pointer, value)
        })
        .cloned()
        .collect();
    rule["enum"] = json!(choices);
    if rule.get("default").is_none() {
        rule["default"] = start;
    }
    rule["x-orch-source"] = json!("provider");
}

/// Apply and check a forwarded body after translation and endpoint defaults, preserving owned fields.
pub fn outgoing(
    protocol: &str,
    model: &ModelParameters,
    body: &[u8],
) -> Result<Vec<u8>, &'static str> {
    let payload: Json =
        serde_json::from_slice(body).map_err(|_| "model request must be an object")?;
    let members = payload
        .as_object()
        .ok_or("model request must be an object")?;
    let mut params = Json::Object(
        members
            .iter()
            .filter(|(key, _)| !calls::owned_option(key))
            .map(|(key, value)| (key.clone(), value.clone()))
            .collect(),
    );
    let search = crate::web_search::declared(protocol);
    if let Some(field) = parameter_format(protocol).web_search
        && let Some(search) = &search
    {
        let selection = search.selection(&payload)?;
        if model.option_rule(protocol, field) != Some(&Json::Bool(false))
            || search.selected(&payload)
        {
            params[field] = selection;
        }
    }
    let params = constrain(protocol, model, &params)?;
    let owned = Json::Object(
        members
            .iter()
            .filter(|(key, _)| calls::owned_option(key))
            .map(|(key, value)| (key.clone(), value.clone()))
            .collect(),
    );
    let owned = serde_json::to_vec(&owned).map_err(|_| "model request cannot be encoded")?;
    let merged = apply(protocol, &owned, &params, false)?;
    let mut payload: Json =
        serde_json::from_slice(&merged).map_err(|_| "model request must be an object")?;
    if let Some(field) = parameter_format(protocol).web_search
        && let Some(search) = search
        && let Some(selected) = payload
            .as_object_mut()
            .and_then(|object| object.remove(field))
    {
        search.apply_constrained(&mut payload, &selected)?;
    }
    serde_json::to_vec(&payload).map_err(|_| "model request cannot be encoded")
}

/// Side endpoints retain their documented option applicability. Compaction supports a service tier,
/// while Cyber programs are generation-only. Token counting has no inference speed selection.
pub fn outgoing_at(
    protocol: &str,
    path: &str,
    model: &ModelParameters,
    body: &[u8],
) -> Result<Vec<u8>, &'static str> {
    let mut applicable = std::borrow::Cow::Borrowed(model);
    if let Some(field) = parameter_format(protocol).web_search
        && let Some(search) = crate::web_search::declared(protocol)
        && !search.accepts(path)
    {
        let original: Json =
            serde_json::from_slice(body).map_err(|_| "model request must be an object")?;
        if search.selected(&original)
            || original.get(field).is_some_and(|selected| {
                orch_provider_contract::search::mode(selected) != Ok("disabled")
            })
        {
            return Err("web search does not apply to this request");
        }
        let applicable = applicable.to_mut();
        applicable.default_web_search = None;
        if let Some(schema) = applicable
            .request_options
            .as_mut()
            .and_then(|schemas| schemas.get_mut(protocol))
        {
            if let Some(properties) = schema.get_mut("properties").and_then(Json::as_object_mut) {
                properties.remove(field);
            }
            if let Some(required) = schema.get_mut("required").and_then(Json::as_array_mut) {
                required.retain(|name| name.as_str() != Some(field));
            }
            for key in ["default", "const"] {
                if let Some(value) = schema.get_mut(key).and_then(Json::as_object_mut) {
                    value.remove(field);
                }
            }
        }
    }
    let model = applicable.as_ref();
    let format = parameter_format(protocol);
    let compact = path.ends_with("/responses/compact") && format.program.is_some();
    let count_tokens = path.ends_with("/messages/count_tokens") && format.tier.is_some();
    if compact || count_tokens {
        let mut parameters = model.clone();
        parameters.default_effort = None;
        parameters.default_access_program = None;
        let mut result: Json = serde_json::from_slice(&outgoing(protocol, &parameters, body)?)
            .map_err(|_| "model request must be an object")?;
        let original: Json =
            serde_json::from_slice(body).map_err(|_| "model request must be an object")?;
        if compact {
            if let Some((field, _, _)) = format.program {
                if original.get(field).is_some()
                    || model.access_program_start().is_some()
                    || model
                        .option_schema(protocol)
                        .and_then(|schema| schema.get("properties"))
                        .and_then(|properties| properties.get(field))
                        .is_some()
                {
                    return Err("this endpoint has not declared access program selection");
                }
                result
                    .as_object_mut()
                    .ok_or("model request must be an object")?
                    .remove(field);
            }
        } else if let Some((field, _)) = format.tier {
            if original.get(field).is_some()
                || model
                    .option_schema(protocol)
                    .and_then(|schema| schema.get("properties"))
                    .and_then(|properties| properties.get(field))
                    .is_some()
            {
                return Err("token counting has not declared an inference speed");
            }
            result
                .as_object_mut()
                .ok_or("model request must be an object")?
                .remove(field);
        }
        return serde_json::to_vec(&result).map_err(|_| "model request cannot be encoded");
    }
    outgoing(protocol, model, body)
}

fn has_effort(params: &Json) -> bool {
    [
        "/effort",
        "/reasoning_effort",
        "/reasoning/effort",
        "/output_config/effort",
    ]
    .iter()
    .any(|path| params.pointer(path).is_some())
}

/// Resolve once before issuing a completion, against its actual current model and wire format.
/// Unknown vendor options remain JSON; being configurable does not declare vendor support.
pub fn with_model_defaults(
    protocol: &str,
    model: &ModelParameters,
    params: &Json,
) -> Result<Json, &'static str> {
    calls::completion_options(params)?;
    let mut options = params
        .as_object()
        .cloned()
        .ok_or("model options must be an object")?;
    if !has_effort(params)
        && let Some(effort) = &model.default_effort
    {
        options.insert("effort".to_owned(), json!(effort));
    }
    let format = parameter_format(protocol);
    if let Some((field, ordinary)) = format.tier {
        if !options.contains_key(field) || options.get(field).and_then(Json::as_str) == Some("auto")
        {
            // Resolve automatic selection locally so account defaults cannot exceed the provider ceiling.
            options.insert(
                field.to_owned(),
                json!(model.service_tier_start().unwrap_or(ordinary)),
            );
        }
        if let Some(selected) = options.get_mut(field) {
            if selected.is_null() {
                *selected = json!(ordinary);
            }
            let value = selected
                .as_str()
                .filter(|value| !value.is_empty())
                .ok_or("service tier must be text or null")?;
            if value != ordinary
                && !model.service_tiers().iter().any(|tier| {
                    tier.id == value
                        || format.tier_aliases.iter().any(|(reported, alias)| {
                            (value == *alias && tier.id == *reported)
                                || (value == *reported && tier.id == *alias)
                        })
                })
            {
                return Err("the selected model has not enabled the requested service tier");
            }
        }
    } else if model.service_tier_start().is_some() {
        return Err("this protocol has not declared model service tiers");
    }
    if let Some((field, member, ordinary)) = format.program {
        if !options.contains_key(field) {
            options.insert(field.to_owned(), json!({}));
        }
        let programs = options
            .get_mut(field)
            .and_then(Json::as_object_mut)
            .ok_or("access programs must be an object")?;
        if !programs.contains_key(member) {
            programs.insert(
                member.to_owned(),
                json!(model.access_program_start().unwrap_or(ordinary)),
            );
        }
        if let Some(selected) = programs.get_mut(member) {
            if selected.is_null() {
                *selected = json!(ordinary);
            }
            let value = selected
                .as_str()
                .filter(|value| !value.is_empty())
                .ok_or("access program must be text or null")?;
            if value != ordinary
                && !model
                    .access_programs()
                    .iter()
                    .any(|program| program == value)
            {
                return Err("the selected model has not enabled the requested access program");
            }
        }
    } else if model.access_program_start().is_some() {
        return Err("this protocol has not declared model access programs");
    }
    if let Some(field) = format.web_search {
        if let Some(selected) = options.get(field) {
            let mode = orch_provider_contract::search::mode(selected)?;
            if mode != "disabled"
                && !model
                    .web_search_modes()
                    .iter()
                    .any(|enabled| enabled == mode)
            {
                return Err("the selected model has not enabled the requested web search mode");
            }
        } else if model.default_web_search.is_some() {
            options.insert(field.to_owned(), json!(model.web_search_start()));
        }
    }
    let options = Json::Object(options);
    if let Some(effort) = calls::requested_effort(&options)?
        && !model.efforts().iter().any(|enabled| enabled == effort)
    {
        return Err("the selected model has not enabled the requested reasoning effort");
    }
    Ok(options)
}

pub(crate) fn overlay(target: &mut Map<String, Json>, source: Map<String, Json>, defaults: bool) {
    for (key, value) in source {
        match (target.get_mut(&key), value) {
            (Some(Json::Object(held)), Json::Object(extra)) => overlay(held, extra, defaults),
            (Some(_), _) if defaults => {}
            (_, value) => {
                target.insert(key, value);
            }
        }
    }
}

/// Apply generation options after the protocol created its owned request fields.
/// Endpoint defaults fill absent members; explicit request values take precedence.
pub fn apply(
    protocol: &str,
    body: &[u8],
    params: &Json,
    defaults: bool,
) -> Result<Vec<u8>, &'static str> {
    calls::completion_options(params)?;
    let mut extra = params
        .as_object()
        .cloned()
        .ok_or("model options must be an object")?;
    if extra.is_empty() {
        return Ok(body.to_vec());
    }
    if has_effort(params)
        && let Some((field, member)) = parameter_format(protocol).effort
    {
        let effort = calls::requested_effort(params)?;
        extra.remove("effort");
        extra.remove("reasoning_effort");
        for key in ["reasoning", "output_config"] {
            if let Some(Json::Object(nested)) = extra.get_mut(key) {
                nested.remove("effort");
                if nested.is_empty() {
                    extra.remove(key);
                }
            }
        }
        if let Some(member) = member {
            let nested = extra.entry(field.to_owned()).or_insert_with(|| json!({}));
            nested
                .as_object_mut()
                .ok_or("reasoning options must be an object")?
                .insert(member.to_owned(), json!(effort));
        } else {
            extra.insert(field.to_owned(), json!(effort));
        }
    }
    if let Some(field) = parameter_format(protocol).web_search
        && let Some(search) = extra.get_mut(field)
    {
        *search = Json::Object(orch_provider_contract::search::options(search)?);
    }
    if extra.is_empty() {
        return Ok(body.to_vec());
    }
    let mut payload: Json =
        serde_json::from_slice(body).map_err(|_| "model request must be an object")?;
    let members = payload
        .as_object_mut()
        .ok_or("model request must be an object")?;
    overlay(members, extra, defaults);
    serde_json::to_vec(&payload).map_err(|_| "model request cannot be encoded")
}

/// Internal vectors use the protocol's float decoder. Raw relay requests keep their own encoding.
pub fn apply_embedding(
    protocol: &str,
    body: &[u8],
    params: &Json,
) -> Result<Vec<u8>, &'static str> {
    if params
        .get("encoding_format")
        .is_some_and(|format| format.as_str() != Some("float"))
    {
        return Err("embeddings require float encoding for the vector decoder");
    }
    apply(protocol, body, params, false)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn model() -> ModelParameters {
        serde_json::from_value(json!({
            "efforts":["low","high"],"default_effort":"low",
            "service_tiers":[{"id":"priority","label":"Fast"}],
            "default_service_tier":"priority"
        }))
        .expect("model parameters")
    }

    #[test]
    fn endpoint_model_and_explicit_options_have_one_preserving_precedence() {
        let resolved = resolve(
            OPENAI_RESPONSES,
            &model(),
            &json!({
                "service_tier":"default", "effort":"high",
                "extra_body":{"vendor":{"keep":[true,null,3],"switch":false}}
            }),
            &json!({
                "service_tier":null, "reasoning":{"effort":"high","summary":"concise"},
                "extra_body":{"vendor":{"switch":true}}
            }),
        )
        .expect("request options");
        assert_eq!(resolved["service_tier"], "default");
        assert_eq!(
            resolved["reasoning"],
            json!({"effort":"high","summary":"concise"})
        );
        assert_eq!(
            resolved["extra_body"]["vendor"],
            json!({"keep":[true,null,3],"switch":true})
        );
        assert!(resolved.get("effort").is_none());
    }

    #[test]
    fn documented_responses_alias_does_not_become_another_protocols_capability() {
        let options =
            with_model_defaults(OPENAI_RESPONSES, &model(), &json!({"service_tier":"fast"}))
                .expect("documented alias of enabled priority");
        assert_eq!(
            options["service_tier"], "fast",
            "requested spelling is retained"
        );
        assert_eq!(
            requested_tier_for_caps(OPENAI_RESPONSES, &options).as_deref(),
            Some("priority")
        );
        assert!(
            with_model_defaults(ANTHROPIC_MESSAGES, &model(), &json!({"speed":"fast"})).is_err()
        );
        assert_eq!(
            service_tier_price_alias(ANTHROPIC_MESSAGES, "priority"),
            None
        );
        assert!(!reports_service_tier("unlisted-format"));
        assert_eq!(
            requested_service_tier("unlisted-format", &json!({"service_tier":"fast"})),
            None
        );
    }

    #[test]
    fn endpoint_defaults_cannot_change_an_explicit_request_or_owned_fields() {
        let body =
            br#"{"model":"chosen","input":"material","temperature":0.7,"future":{"n":null}}"#;
        let merged = apply(
            OPENAI_RESPONSES,
            body,
            &json!({
                "temperature":0.2,"future":{"n":3,"enabled":true}
            }),
            true,
        )
        .expect("body defaults");
        let merged: Json = serde_json::from_slice(&merged).expect("JSON");
        assert_eq!(merged["model"], "chosen");
        assert_eq!(merged["input"], "material");
        assert_eq!(merged["temperature"], json!(0.7));
        assert_eq!(merged["future"], json!({"n":null,"enabled":true}));
        for protected in [
            "model",
            "input",
            "messages",
            "system",
            "tools",
            "max_output_tokens",
            "store",
            "headers",
        ] {
            let replacement = Json::Object(
                [(protected.to_owned(), json!("replacement"))]
                    .into_iter()
                    .collect(),
            );
            assert!(
                apply(OPENAI_RESPONSES, body, &replacement, false).is_err(),
                "{protected}"
            );
        }
    }
}

/// Apply generation options using the selected protocol's declared tool mapping.
/// Non-generation requests cannot acquire a hosted search tool.
pub fn apply_request(
    protocol: &dyn crate::protocol::WireProtocol,
    path: &str,
    body: &[u8],
    params: &Json,
    defaults: bool,
) -> Result<Vec<u8>, &'static str> {
    let mut options = params
        .as_object()
        .cloned()
        .ok_or("model options must be an object")?;
    let field = parameter_format(protocol.name()).web_search;
    let search = field.and_then(|field| options.remove(field));
    let merged = apply(protocol.name(), body, &Json::Object(options), defaults)?;
    let Some(search) = search else {
        return Ok(merged);
    };
    let declaration = match crate::web_search::declared(protocol.name()) {
        Some(declaration) => declaration,
        None if orch_provider_contract::search::mode(&search)? == "disabled" => return Ok(merged),
        None => return Err("this protocol has not declared web search"),
    };
    let mut payload: Json =
        serde_json::from_slice(&merged).map_err(|_| "model request must be an object")?;
    if defaults {
        declaration.apply_defaults(path, &mut payload, &search)?;
    } else {
        declaration.apply(path, &mut payload, &search)?;
    }
    serde_json::to_vec(&payload).map_err(|_| "model request cannot be encoded")
}
