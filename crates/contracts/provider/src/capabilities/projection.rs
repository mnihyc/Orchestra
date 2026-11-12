//! Trusted declaration templates map actual input and replies; they execute no code and carry no credentials.
use super::*;
use crate::{SourceError, SourceErrorCode};
use orch_types::Json;
use serde_json::json;
use std::collections::BTreeMap;
use url::Url;

/// Native/service facts for one actual endpoint/model, independent of manager-enabled storage.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct EndpointProjection {
    /// Immutable program-specific model dialect metadata selected by the same endpoint rules.
    pub model_metadata: BTreeMap<String, Json>,
    /// Provider-owned program feature refinements for this exact endpoint and model.
    pub native_switches: BTreeMap<String, BTreeMap<String, NativeSwitch>>,
    /// Exact native binding or explicit unsupported override.
    pub native_tools: BTreeMap<String, Option<Binding>>,
    /// Additional endpoints under the existing account credential roles.
    pub endpoints: BTreeMap<String, Endpoint>,
    /// Offered, still manager-disabled service entries.
    pub offerings: BTreeMap<String, Offering>,
}
/// URL parser facts distinguish an exact vendor endpoint from user-info, query, port or look-alike hosts.
#[must_use]
pub fn endpoint_facts(protocol: &str, base: &Url, model: &str) -> Json {
    json!({"protocol":protocol,"scheme":base.scheme(),"host":base.host_str(),"port":base.port(),"path":base.path(),
        "has_credentials":!base.username().is_empty() || base.password().is_some(),"has_query":base.query().is_some(),"has_fragment":base.fragment().is_some(),"model":model})
}
/// Extensions overlay their matching rules in declared order. Nothing here enables a manager's model/service.
#[must_use]
pub fn project_endpoint(
    declarations: &Declarations,
    protocol: &str,
    base: &Url,
    model: &str,
) -> EndpointProjection {
    let facts = endpoint_facts(protocol, base, model);
    let mut result = EndpointProjection::default();
    for rule in &declarations.endpoint_rules {
        if !rule.when.is_null() && orch_types::schema::validate(&rule.when, &facts).is_err() {
            continue;
        }
        result.model_metadata.extend(rule.model_metadata.clone());
        result.native_tools.extend(rule.native_tools.clone());
        for (program, switches) in &rule.native_switches {
            result
                .native_switches
                .entry(program.clone())
                .or_default()
                .extend(switches.clone());
        }
        result.endpoints.extend(rule.endpoints.clone());
        result
            .offerings
            .extend(rule.offerings.iter().map(|(id, offering)| {
                let mut offering = offering.clone();
                offering.enabled = false;
                (id.clone(), offering)
            }));
    }
    result
}

fn malformed(message: &str) -> SourceError {
    SourceError::new(SourceErrorCode::Malformed, message)
}
/// A template reference is interpreted only in the trusted template, never recursively in substituted input.
fn render(
    template: &Json,
    input: &Json,
    result: &Json,
    response: &Json,
) -> Result<Json, SourceError> {
    if let Some(object) = template.as_object() {
        for (name, source) in [
            ("from_input", input),
            ("from_result", result),
            ("from_response", response),
        ] {
            if let Some(pointer) = object.get(name) {
                let pointers: Vec<&str> = match pointer {
                    Json::String(pointer) => vec![pointer],
                    Json::Array(pointers) => pointers.iter().filter_map(Json::as_str).collect(),
                    _ => return Err(malformed("a declared template reference is a JSON pointer")),
                };
                if let Some(value) = pointers
                    .into_iter()
                    .find_map(|pointer| source.pointer(pointer).filter(|value| !value.is_null()))
                {
                    return Ok(value.clone());
                }
                if object.get("optional") == Some(&json!(true)) {
                    return Ok(Json::Null);
                }
                if let Some(default) = object.get("default") {
                    return Ok(default.clone());
                }
                return Err(malformed(
                    "the capability reply or input lacks a required declared member",
                ));
            }
        }
        return object
            .iter()
            .map(|(key, value)| {
                render(value, input, result, response).map(|value| (key.clone(), value))
            })
            .collect::<Result<serde_json::Map<_, _>, _>>()
            .map(Json::Object);
    }
    if let Some(values) = template.as_array() {
        return values
            .iter()
            .map(|value| render(value, input, result, response))
            .collect::<Result<Vec<_>, _>>()
            .map(Json::Array);
    }
    Ok(template.clone())
}
impl Binding {
    /// Whether an installed binding declares a native wire identity.
    /// This is declaration data, not permission or model support.
    #[must_use]
    pub fn has_native_request_selection(&self) -> bool {
        fn declared(request: &Json) -> bool {
            ["tool", "root_selection_identity"].iter().any(|key| {
                request
                    .get(key)
                    .and_then(Json::as_object)
                    .is_some_and(|value| !value.is_empty())
            }) || request
                .get("identities")
                .and_then(Json::as_array)
                .is_some_and(|values| !values.is_empty())
                || request
                    .get("model_variant_identity")
                    .and_then(Json::as_str)
                    .is_some_and(|value| !value.is_empty())
                || request
                    .get("forms")
                    .and_then(Json::as_array)
                    .is_some_and(|values| values.iter().any(declared))
        }
        declared(&self.request)
    }

    /// Test only the installed declaration's native wire identity.
    #[must_use]
    pub fn native_request_selected(&self, body: &Json) -> bool {
        fn matches(identity: &Json, value: &Json) -> bool {
            identity.as_object().is_some_and(|members| {
                !members.is_empty()
                    && members.iter().all(|(key, expected)| {
                        if expected.is_null() {
                            value.get(key).is_some()
                        } else {
                            value.get(key) == Some(expected)
                        }
                    })
            })
        }
        fn selected(request: &Json, body: &Json) -> bool {
            if request
                .get("forms")
                .and_then(Json::as_array)
                .is_some_and(|forms| forms.iter().any(|form| selected(form, body)))
            {
                return true;
            }
            let root = request
                .get("root_selection_identity")
                .and_then(Json::as_object);
            if root.is_some_and(|members| {
                !members.is_empty()
                    && members.iter().all(|(key, value)| {
                        if value.is_null() {
                            body.get(key).is_some()
                        } else {
                            body.get(key) == Some(value)
                        }
                    })
            }) {
                return true;
            }
            if request
                .get("model_variant_identity")
                .and_then(Json::as_str)
                .is_some_and(|identity| {
                    body.get("model")
                        .and_then(Json::as_str)
                        .is_some_and(|model| model.split(':').any(|part| part == identity))
                })
            {
                return true;
            }
            let container = request
                .get("container")
                .and_then(Json::as_str)
                .unwrap_or("/tools");
            body.pointer(container)
                .and_then(Json::as_array)
                .is_some_and(|tools| {
                    tools.iter().any(|tool| {
                        request
                            .get("identities")
                            .and_then(Json::as_array)
                            .is_some_and(|identities| {
                                identities.iter().any(|identity| matches(identity, tool))
                            })
                            || request
                                .get("tool")
                                .is_some_and(|identity| matches(identity, tool))
                    })
                })
        }
        selected(&self.request, body)
    }
}

/// Validate the actual call against its binding and build the provider extension's fixed request fragment.
fn validate_input(binding: &Binding, input: &Json) -> Result<(), SourceError> {
    if !binding.input_schema.is_null()
        && orch_types::schema::validate(&binding.input_schema, input).is_err()
    {
        return Err(SourceError::new(
            SourceErrorCode::Refused,
            "the capability input does not fit the selected backend",
        ));
    }
    Ok(())
}
/// Validate the actual call and render its provider-owned API request template.
pub fn render_request(binding: &Binding, input: &Json) -> Result<Json, SourceError> {
    validate_input(binding, input)?;
    let mut selected = None;
    if let Some(actions) = binding
        .extensions
        .get("x_actions")
        .and_then(Json::as_object)
    {
        for action in actions.values() {
            let Some(schema) = action
                .get("input_schema")
                .filter(|schema| !schema.is_null())
            else {
                continue;
            };
            if orch_types::schema::validate(schema, input).is_ok() {
                if selected.is_some() {
                    return Err(SourceError::new(
                        SourceErrorCode::Malformed,
                        "the capability input selects ambiguous declared actions",
                    ));
                }
                selected = action.get("request");
            }
        }
    }
    render(
        selected.unwrap_or(&binding.request),
        input,
        &Json::Null,
        &Json::Null,
    )
}
/// Auxiliary model prompts/options are declared data; the existing model builder and ceiling still own the wire request.
pub fn render_auxiliary(binding: &Binding, input: &Json) -> Result<Json, SourceError> {
    validate_input(binding, input)?;
    let template = binding.extensions.get("auxiliary").ok_or_else(|| {
        SourceError::new(
            SourceErrorCode::Unsupported,
            "this native binding does not declare an auxiliary request",
        )
    })?;
    render(template, input, &Json::Null, &Json::Null)
}
/// Select a declared JSON pointer with array wildcards. An existing empty array remains a valid empty result.
fn select<'a>(root: &'a Json, pointer: &str) -> Option<Vec<&'a Json>> {
    if pointer.is_empty() {
        return Some(vec![root]);
    }
    let mut values = vec![root];
    for segment in pointer.strip_prefix('/')?.split('/') {
        if values.is_empty() {
            return Some(values);
        }
        let segment = segment.replace("~1", "/").replace("~0", "~");
        let mut next = Vec::new();
        let mut present = false;
        for value in values {
            if segment == "*" {
                if let Some(array) = value.as_array() {
                    present = true;
                    next.extend(array);
                }
            } else if let Some(value) = value
                .as_object()
                .and_then(|object| object.get(&segment))
                .or_else(|| {
                    value.as_array().and_then(|array| {
                        segment
                            .parse::<usize>()
                            .ok()
                            .and_then(|index| array.get(index))
                    })
                })
            {
                present = true;
                next.push(value);
            }
        }
        if !present {
            return None;
        }
        values = next;
    }
    Some(values)
}
/// Project actual structured replies through the binding's generic result mapping.
/// Collection output count follows the caller's requested limit, never a monetary or usage counter.
pub fn project_result(
    binding: &Binding,
    input: &Json,
    response: &Json,
    limit: usize,
) -> Result<Json, SourceError> {
    if let Some(schema) = binding.result.get("schema")
        && orch_types::schema::validate(schema, response).is_err()
    {
        return Err(malformed(
            "the capability reply does not fit its declared result schema",
        ));
    }
    if let Some(errors) = binding.result.get("errors").and_then(Json::as_array) {
        for error in errors {
            let pointer = error
                .get("pointer")
                .and_then(Json::as_str)
                .ok_or_else(|| malformed("a declared error selector needs a pointer"))?;
            let when = error.get("when").unwrap_or(&Json::Null);
            if select(response, pointer)
                .into_iter()
                .flatten()
                .any(|value| when.is_null() || orch_types::schema::validate(when, value).is_ok())
            {
                return Err(SourceError::new(
                    SourceErrorCode::Refused,
                    "the backend's native capability tool refused the request",
                ));
            }
        }
    }
    if let Some(page) = binding.result.get("page") {
        if let Some(pointer) = page.get("pointer").and_then(Json::as_str) {
            let when = page.get("when").unwrap_or(&Json::Null);
            let mut pages = select(response, pointer)
                .into_iter()
                .flatten()
                .filter(|value| {
                    when.is_null() || orch_types::schema::validate(when, value).is_ok()
                });
            let value = pages
                .next()
                .ok_or_else(|| malformed("the capability reply lacks its declared page content"))?;
            if pages.next().is_some() {
                return Err(malformed(
                    "the capability reply contains more than one declared page",
                ));
            }
            let item = page
                .get("item")
                .ok_or_else(|| malformed("a declared page selector needs an item template"))?;
            return render(item, input, value, response);
        }
        return render(page, input, response, response);
    }
    let collections = binding
        .result
        .get("collections")
        .and_then(Json::as_array)
        .ok_or_else(|| malformed("the capability declares a result projection"))?;
    let mut rows = Vec::new();
    let mut seen = std::collections::BTreeSet::new();
    if limit == 0 {
        return Ok(Json::Array(rows));
    }
    for collection in collections {
        let pointer = collection
            .get("pointer")
            .and_then(Json::as_str)
            .ok_or_else(|| malformed("a declared collection needs a pointer"))?;
        let Some(values) = select(response, pointer) else {
            if collection.get("optional") == Some(&json!(true)) {
                continue;
            }
            return Err(malformed(
                "the capability reply lacks its result collection",
            ));
        };
        let template = collection
            .get("item")
            .ok_or_else(|| malformed("a declared collection needs an item template"))?;
        for value in values {
            if let Some(when) = collection.get("when")
                && orch_types::schema::validate(when, value).is_err()
            {
                continue;
            }
            let row = render(template, input, value, response)?;
            let identity = binding
                .result
                .get("unique_by")
                .and_then(Json::as_str)
                .and_then(|pointer| row.pointer(pointer))
                .unwrap_or(&row)
                .to_string();
            if seen.insert(identity) {
                rows.push(row);
            }
            if rows.len() >= limit {
                return Ok(Json::Array(rows));
            }
        }
    }
    Ok(Json::Array(rows))
}

#[cfg(test)]
#[allow(clippy::unwrap_used, reason = "a test fails by panicking")]
mod tests {
    use super::*;
    #[test]
    fn native_selection_uses_declared_containers_and_never_confuses_a_caller_function() {
        let mut binding = Binding {
            request: json!({"identities":[{"type":"native_example"}]}),
            ..Default::default()
        };
        assert!(binding.has_native_request_selection());
        assert!(binding.native_request_selected(&json!({"tools":[{"type":"native_example"}]})));
        assert!(!binding.native_request_selected(
            &json!({"tools":[{"type":"function","name":"native_example"}]})
        ));
        assert!(!binding.native_request_selected(&json!({"tools":[]})));
        binding.request = json!({"container":"/plugins","identities":[{"id":"native_example"}]});
        assert!(binding.native_request_selected(&json!({"plugins":[{"id":"native_example"}]})));
        assert!(!binding.native_request_selected(&json!({"tools":[{"id":"native_example"}]})));
        binding.request = json!({"body":{"from_input":""}});
        assert!(!binding.has_native_request_selection());
    }

    #[test]
    fn templates_keep_input_as_data_and_reply_projection_uses_actual_records_without_inventing_usage()
     {
        let binding:Binding=serde_json::from_value(json!({"input_schema":{"type":"object","required":["query"],"properties":{"query":{"type":"string"}},"additionalProperties":false},
            "request":{"method":"POST","path":"/lookup","body":{"query":{"from_input":"/query"}}},
            "result":{"schema":{"type":"object","required":["results"],"properties":{"results":{"type":"array"}}},"collections":[{"pointer":"/results/*","item":{"title":{"from_result":["/title","/url"]},"url":{"from_result":"/url"}}}]}})).unwrap();
        let input = json!({"query":"{\"from_input\":\"/credential\"}"});
        assert_eq!(
            render_request(&binding, &input).unwrap()["body"]["query"],
            input["query"]
        );
        assert!(render_request(&binding, &json!({"query":"q","provider_id":"injected"})).is_err());
        let reply = json!({"results":[{"title":"Actual","url":"https://page.example.invalid"},{"title":"Actual","url":"https://page.example.invalid"}]});
        let projected = project_result(&binding, &input, &reply, 10).unwrap();
        assert_eq!(projected.as_array().unwrap().len(), 1);
        assert!(projected[0].get("usage").is_none());
        assert_eq!(
            project_result(&binding, &input, &json!({"results":[]}), 10).unwrap(),
            json!([])
        );
        assert!(project_result(&binding, &input, &json!({}), 10).is_err());
    }
    #[test]
    fn an_explicit_success_counter_keeps_free_empty_and_unknown_counts_distinct() {
        let count:Counter=serde_json::from_value(json!({"constant":1,"only_when":{"required":["results"],"properties":{"results":{"type":"array","minItems":1}}},"otherwise":0})).unwrap();
        assert_eq!(count.count(&json!({"results":[{}]})), Some(1));
        assert_eq!(count.count(&json!({"results":[]})), Some(0));
        let count = Counter {
            pointer: "/usage/successes".into(),
            ..Counter::default()
        };
        assert_eq!(count.count(&json!({"usage":{"successes":0}})), Some(0));
        assert_eq!(count.count(&json!({"usage":{"successes":-1}})), None);
        assert_eq!(count.count(&json!({})), None);
    }
}
