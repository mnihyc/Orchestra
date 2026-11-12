//! Apply a protocol's declared native search tool without replacing client tools.

use std::collections::BTreeMap;

use orch_types::Json;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::json;

/// A wire protocol owns this declaration. Shared request code knows no vendor tool names.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct WebSearchDeclaration {
    /// Root-encoded modes on a declared native side request, using the same neutral search setting.
    #[serde(default)]
    pub root_mode_pointer: Option<String>,
    /// Equivalent wire values by mode.
    #[serde(default)]
    pub root_mode_values: BTreeMap<String, Vec<Json>>,
    /// A side request must also match the captured session's public native feature and mode.
    #[serde(default)]
    pub native_session: Option<NativeSessionConstraint>,
    /// A root option object on a side request; all its members must fit the same search ceiling.
    #[serde(default)]
    pub root_options_pointer: Option<String>,
    /// JSON pointer of the native array; ordinary protocols use tools.
    #[serde(default = "tools_container")]
    pub container: String,
    /// Other declared wire forms for the same capability, recognized without changing its setting.
    #[serde(default)]
    pub forms: Vec<WebSearchDeclaration>,
    /// Neutral option names mapped to their actual wire names.
    #[serde(default)]
    pub option_names: BTreeMap<String, String>,
    /// Neutral options carried at explicit root JSON pointers instead of inside the native entry.
    #[serde(default)]
    pub root_option_names: BTreeMap<String, String>,
    /// A declared executable colon-delimited model variant; disabling never rewrites its model name.
    #[serde(default)]
    pub model_variant_identity: Option<String>,
    /// Partial root identity; null members mean presence.
    #[serde(default)]
    pub root_selection_identity: Json,
    /// This native format permits omitting its otherwise nested configurable object.
    #[serde(default)]
    pub optional_options_object: bool,
    /// Known wire mode members omitted by this endpoint after their actual mode has been checked.
    #[serde(default)]
    pub strip_options: Vec<String>,
    /// Other protocol-known configurable fields remain visible to a narrower endpoint's final ceiling.
    #[serde(default)]
    pub recognized_options: Vec<String>,
    /// JSON pointer inside the tool for configurable members; empty means the tool object itself.
    #[serde(default)]
    pub options_pointer: String,
    /// Generation paths that accept the tool, below the endpoint base.
    pub paths: Vec<String>,
    /// Generation suffixes for protocols whose validated path contains a model name.
    #[serde(default)]
    pub path_suffixes: Vec<String>,
    /// The protocol-owned tool identity and required members.
    pub tool: Json,
    /// Any of these partial objects identifies an existing hosted search tool to replace.
    pub identities: Vec<Json>,
    /// Native tool members set by each supported neutral mode.
    pub modes: BTreeMap<String, Json>,
    /// Schema of configurable tool members, excluding mode and protocol-owned identity fields.
    pub options_schema: Json,
}

/// Setting names are extension data; credentials and route selection never enter this grammar.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct NativeSessionConstraint {
    /// Public boolean switch emitted by the existing capability resolution.
    pub enabled_setting: String,
    /// Protocol input members retained separately from configurable model options.
    #[serde(default)]
    pub owned_fields: Vec<String>,
    /// Options applicable to this side request after the ordinary provider constraint check.
    #[serde(default)]
    pub parameter_fields: Vec<String>,
}

fn tools_container() -> String {
    "/tools".into()
}
pub(crate) fn identity_matches(identity: &Json, value: &Json) -> bool {
    identity.as_object().is_some_and(|members| {
        members.iter().all(|(key, expected)| {
            if expected.is_null() {
                value.get(key).is_some()
            } else {
                value.get(key) == Some(expected)
            }
        })
    })
}
fn write_pointer(root: &mut Json, pointer: &str, value: Json) -> Result<(), &'static str> {
    let mut parts = pointer
        .strip_prefix('/')
        .ok_or("a native option pointer must be absolute")?
        .split('/')
        .peekable();
    let mut at = root;
    while let Some(part) = parts.next() {
        let key = part.replace("~1", "/").replace("~0", "~");
        let members = at
            .as_object_mut()
            .ok_or("a native option pointer crosses a non-object")?;
        if parts.peek().is_none() {
            members.insert(key, value);
            return Ok(());
        }
        at = members.entry(key).or_insert_with(|| json!({}));
    }
    Err("a native option pointer needs a member")
}
fn remove_pointer(root: &mut Json, pointer: &str) {
    if let Some((parent, name)) = pointer.rsplit_once('/') {
        let key = name.replace("~1", "/").replace("~0", "~");
        if let Some(members) = root.pointer_mut(parent).and_then(Json::as_object_mut) {
            members.remove(&key);
        }
    }
}

impl WebSearchDeclaration {
    fn all_forms(&self) -> Vec<&Self> {
        let mut todo = vec![self];
        let mut found = Vec::new();
        while let Some(form) = todo.pop() {
            found.push(form);
            todo.extend(form.forms.iter().rev());
        }
        found
    }
    fn accepts_form(&self, path: &str) -> bool {
        let path = path.split('?').next().unwrap_or(path);
        self.paths.iter().any(|allowed| allowed == path)
            || self
                .path_suffixes
                .iter()
                .any(|suffix| path.ends_with(suffix))
    }
    /// The ordinary wire protocol still validates the complete generation path.
    #[must_use]
    pub fn accepts(&self, path: &str) -> bool {
        self.all_forms().iter().any(|form| form.accepts_form(path))
    }

    /// The selected form must fit the actual path, rather than another form's path.
    pub fn accepts_selected(&self, path: &str, body: &Json) -> Result<bool, &'static str> {
        Ok(self
            .active(body)?
            .is_none_or(|form| form.accepts_form(path)))
    }

    /// Only the form actually selected by this payload can supply side-request option applicability.
    pub(crate) fn native_session_in(
        &self,
        body: &Json,
    ) -> Result<Option<&NativeSessionConstraint>, &'static str> {
        Ok(self
            .active(body)?
            .and_then(|form| form.native_session.as_ref()))
    }

    /// Read the existing resolution's mode and native flags without resolving another route.
    pub fn admit_native_request(
        &self,
        path: &str,
        body: &Json,
        configuration: &BTreeMap<String, Json>,
        mode: &str,
    ) -> Result<(), &'static str> {
        let form = self
            .active(body)?
            .ok_or("the request lacks a declared native search form")?;
        let constraint = form
            .native_session
            .as_ref()
            .filter(|_| form.accepts_form(path))
            .ok_or("the endpoint has not declared this native side request")?;
        if configuration.get(&constraint.enabled_setting) != Some(&Json::Bool(true)) {
            return Err("the admitted session does not enable this native side request");
        }
        if form.mode_in(body)? != mode {
            return Err("the native request changes the admitted web search mode");
        }
        Ok(())
    }
    /// A mode is available if at least one declared form can carry it.
    #[must_use]
    pub fn supports_mode(&self, mode: &str) -> bool {
        self.all_forms()
            .iter()
            .any(|form| form.modes.contains_key(mode))
    }
    /// Each form retains its own closed schema. The model's common ceiling can describe their union.
    #[must_use]
    pub fn constraint_schema(&self) -> Json {
        if self.forms.is_empty() {
            return self.options_schema.clone();
        }
        let branches: Vec<_> = self
            .all_forms()
            .into_iter()
            .map(|form| {
                let mut schema = form.options_schema.clone();
                schema["properties"]["mode"] = json!({"type":"string"});
                schema
            })
            .collect();
        json!({"type":"object","anyOf":branches,"properties":{"mode":{"type":"string"}}})
    }
    fn tool_in<'a>(&self, body: &'a Json) -> Option<&'a Json> {
        body.pointer(&self.container)
            .and_then(Json::as_array)?
            .iter()
            .find(|tool| self.matches_tool(tool))
    }
    fn implicit_variant(&self, body: &Json) -> bool {
        self.model_variant_identity
            .as_deref()
            .is_some_and(|identity| {
                body.get("model")
                    .and_then(Json::as_str)
                    .is_some_and(|model| model.split(':').skip(1).any(|part| part == identity))
            })
    }
    fn implicit(&self, body: &Json) -> bool {
        self.implicit_variant(body)
            || (!self.root_selection_identity.is_null()
                && identity_matches(&self.root_selection_identity, body))
    }
    fn active<'a>(&'a self, body: &Json) -> Result<Option<&'a Self>, &'static str> {
        let mut active = None;
        for form in self.all_forms() {
            if body
                .pointer(&form.container)
                .is_some_and(|container| !container.is_array())
            {
                return Err("a native search container must be an array");
            }
            for pointer in form.root_option_names.values() {
                if let Some((parent, _)) = pointer.rsplit_once('/')
                    && !parent.is_empty()
                    && body.pointer(parent).is_some_and(|value| !value.is_object())
                {
                    return Err("a native root option parent must be an object");
                }
            }
            let count = body
                .pointer(&form.container)
                .and_then(Json::as_array)
                .into_iter()
                .flatten()
                .filter(|tool| form.matches_tool(tool))
                .count();
            if count > 1 {
                return Err("a request must select at most one hosted search tool");
            }
            if count == 1 || form.implicit(body) {
                if active.is_some() {
                    return Err("a request must select one native search form");
                }
                active = Some(form);
            }
        }
        Ok(active)
    }
    /// Null identity members mean present, not a guessed native tool name.
    #[must_use]
    pub fn matches_tool(&self, tool: &Json) -> bool {
        self.identities
            .iter()
            .any(|identity| identity_matches(identity, tool))
    }
    fn mode_in(&self, body: &Json) -> Result<String, &'static str> {
        if let Some(pointer) = &self.root_mode_pointer {
            let value = body
                .pointer(pointer)
                .ok_or("the native request lacks its search mode")?;
            return self
                .root_mode_values
                .iter()
                .find(|(_, values)| values.contains(value))
                .map(|(mode, _)| mode.clone())
                .ok_or("the native request has an undeclared search mode");
        }
        let Some(tool) = self.tool_in(body) else {
            return Ok("live".into());
        };
        if let Some(mode) = self
            .options_schema
            .get("x-orch-mode-overrides")
            .and_then(Json::as_array)
            .into_iter()
            .flatten()
            .find_map(|rule| {
                identity_matches(rule.get("identity")?, tool)
                    .then(|| rule.get("mode").and_then(Json::as_str))
                    .flatten()
            })
        {
            return Ok(mode.into());
        }
        let options = match tool.pointer(&self.options_pointer) {
            Some(options) => options,
            None if self.optional_options_object => return Ok("live".into()),
            None => return Err("the search tool has no options object"),
        };
        if !options.is_object() {
            return Err("the search tool has no options object");
        }
        Ok(self
            .modes
            .iter()
            .find(|(_, members)| {
                members.as_object().is_some_and(|members| {
                    !members.is_empty()
                        && members
                            .iter()
                            .all(|(key, value)| options.get(key) == Some(value))
                })
            })
            .map_or_else(|| "live".into(), |(mode, _)| mode.clone()))
    }
    /// Actual mode includes declared native root fields and executable model variants.
    pub fn selected_mode(&self, body: &Json) -> Result<String, &'static str> {
        self.active(body)?
            .map_or_else(|| Ok("disabled".into()), |form| form.mode_in(body))
    }
    fn known_options(&self) -> std::collections::BTreeSet<String> {
        self.all_forms()
            .into_iter()
            .flat_map(|form| {
                form.options_schema
                    .get("properties")
                    .and_then(Json::as_object)
                    .into_iter()
                    .flat_map(|properties| properties.keys().cloned())
                    .chain(form.recognized_options.iter().cloned())
            })
            .collect()
    }
    /// Lift neutral names from their actual declared wire locations; native engine mode stays distinct.
    pub fn selection(&self, body: &Json) -> Result<Json, &'static str> {
        let Some(form) = self.active(body)? else {
            return Ok(json!({"mode":"disabled"}));
        };
        let mut selected =
            serde_json::Map::from_iter([("mode".into(), json!(form.mode_in(body)?))]);
        if let Some(pointer) = &form.root_options_pointer {
            let options = body
                .pointer(pointer)
                .and_then(Json::as_object)
                .ok_or("the native search settings must be an object")?;
            let mode_fields: std::collections::BTreeSet<&str> = form
                .modes
                .values()
                .filter_map(Json::as_object)
                .flat_map(|value| value.keys().map(String::as_str))
                .collect();
            selected.extend(
                options
                    .iter()
                    .filter(|(name, _)| !mode_fields.contains(name.as_str()))
                    .map(|(name, value)| (name.clone(), value.clone())),
            );
        }
        let options = form
            .tool_in(body)
            .and_then(|tool| tool.pointer(&form.options_pointer))
            .and_then(Json::as_object);
        for name in self
            .known_options()
            .into_iter()
            .filter(|name| name != "mode")
        {
            let value = if let Some(pointer) = form.root_option_names.get(&name) {
                body.pointer(pointer)
            } else {
                let wire = form.option_names.get(&name).unwrap_or(&name);
                options.and_then(|options| options.get(wire))
            };
            if let Some(value) = value {
                selected.insert(name, value.clone());
            }
        }
        Ok(Json::Object(selected))
    }
    fn prepared(
        &self,
        selected: &Json,
        include_mode: bool,
    ) -> Result<serde_json::Map<String, Json>, &'static str> {
        let mode = orch_provider_contract::search::mode(selected)?;
        let mut options = orch_provider_contract::search::options(selected)?;
        options.remove("mode");
        orch_types::schema::validate(&self.options_schema, &Json::Object(options.clone()))
            .map_err(|_| "web search options do not fit the selected native form")?;
        if include_mode {
            options.extend(
                self.modes
                    .get(mode)
                    .and_then(Json::as_object)
                    .ok_or("this native form does not support the selected web search mode")?
                    .clone(),
            );
        }
        Ok(options)
    }
    fn write_options(
        &self,
        body: &mut Json,
        options: serde_json::Map<String, Json>,
        defaults: bool,
    ) -> Result<(), &'static str> {
        if self.container == "/model"
            || self.container.starts_with("/model/")
            || self
                .root_option_names
                .values()
                .any(|pointer| pointer == "/model" || pointer.starts_with("/model/"))
        {
            return Err("native search options cannot change the selected model");
        }
        let mut roots = Vec::new();
        let mut wire = serde_json::Map::new();
        for (name, value) in options {
            if let Some(pointer) = self.root_option_names.get(&name) {
                roots.push((pointer.clone(), value));
            } else {
                wire.insert(self.option_names.get(&name).cloned().unwrap_or(name), value);
            }
        }
        let present = self.tool_in(body).is_some();
        // Preserve an implicit native variant when there is no explicit wire option to add.
        if present || !self.implicit(body) || !wire.is_empty() {
            if !present {
                if body.pointer(&self.container).is_none() {
                    write_pointer(body, &self.container, json!([]))?;
                }
                body.pointer_mut(&self.container)
                    .and_then(Json::as_array_mut)
                    .ok_or("a native search container must be an array")?
                    .push(self.tool.clone());
            }
            let tool = body
                .pointer_mut(&self.container)
                .and_then(Json::as_array_mut)
                .into_iter()
                .flatten()
                .find(|tool| self.matches_tool(tool))
                .ok_or("the request did not select the constrained search tool")?;
            if tool.pointer(&self.options_pointer).is_none() && self.optional_options_object {
                write_pointer(tool, &self.options_pointer, json!({}))?;
            }
            let target = tool
                .pointer_mut(&self.options_pointer)
                .and_then(Json::as_object_mut)
                .ok_or("the search tool has no options object")?;
            if defaults {
                crate::parameters::overlay(target, wire, true);
            } else {
                for field in &self.strip_options {
                    target.remove(field);
                }
                target.extend(wire);
            }
        }
        for (pointer, value) in roots {
            if !defaults || body.pointer(&pointer).is_none() {
                write_pointer(body, &pointer, value)?;
            }
        }
        Ok(())
    }
    fn disable(&self, body: &mut Json) -> Result<(), &'static str> {
        if self.all_forms().iter().any(|form| {
            form.implicit_variant(body)
                || (form.root_selection_identity.get("model").is_some()
                    && identity_matches(&form.root_selection_identity, body))
        }) {
            return Err("the configured model variant requires native search");
        }
        for form in self.all_forms() {
            if let Some(tools) = body
                .pointer_mut(&form.container)
                .and_then(Json::as_array_mut)
            {
                tools.retain(|tool| !form.matches_tool(tool));
            }
            for pointer in form.root_option_names.values() {
                remove_pointer(body, pointer);
            }
            if let Some(identity) = form.root_selection_identity.as_object()
                && let Some(members) = body.as_object_mut()
            {
                for key in identity.keys() {
                    members.remove(key);
                }
            }
        }
        Ok(())
    }
    /// Endpoint defaults retain the exact selected form/version and explicit wire values.
    pub fn apply_defaults(
        &self,
        path: &str,
        body: &mut Json,
        defaults: &Json,
    ) -> Result<(), &'static str> {
        let Some(form) = self.active(body)? else {
            return Ok(());
        };
        if !form.accepts_form(path) {
            return Ok(());
        }
        let options = form.prepared(defaults, false)?;
        form.write_options(body, options, true)
    }
    /// Final model constraints retain the selected form and never invent a native tool from a default.
    pub(crate) fn apply_constrained(
        &self,
        body: &mut Json,
        selected: &Json,
    ) -> Result<(), &'static str> {
        if orch_provider_contract::search::mode(selected)? == "disabled" {
            return self.disable(body);
        }
        let form = self
            .active(body)?
            .ok_or("the request did not select the constrained search tool")?;
        let options = form.prepared(selected, true)?;
        form.write_options(body, options, false)
    }
    /// Whether any declared native form, root option or model variant selects search.
    #[must_use]
    pub fn selected(&self, body: &Json) -> bool {
        self.all_forms()
            .iter()
            .any(|form| form.tool_in(body).is_some() || form.implicit(body))
    }
    /// The same declared builder for auxiliary and native requests, preserving client tools/plugins.
    pub fn apply(&self, path: &str, body: &mut Json, selected: &Json) -> Result<(), &'static str> {
        let mode = orch_provider_contract::search::mode(selected)?;
        let form = self.active(body)?.unwrap_or(self);
        if !form.accepts_form(path) {
            return if mode == "disabled" {
                Ok(())
            } else {
                Err("web search does not apply to this request")
            };
        }
        if mode == "disabled" {
            return self.disable(body);
        }
        let options = form.prepared(selected, true)?;
        form.write_options(body, options, false)
    }
}

/// Read a protocol's data declaration, without selecting a vendor in request code.
#[must_use]
pub fn declared(protocol: &str) -> Option<WebSearchDeclaration> {
    crate::protocol::parameter_format(protocol).web_search?;
    serde_json::from_value(orch_provider_contract::search::declaration(protocol)?).ok()
}

/// The same native-tool grammar, with an actual endpoint/model declaration overriding a compatible default.
/// An explicit null is unsupported; future members remain in the original stored Binding JSON.
#[must_use]
pub fn for_model(
    protocol: &str,
    model: &orch_provider_contract::models::ModelParameters,
) -> Option<WebSearchDeclaration> {
    match model
        .native_tools
        .get(protocol)
        .and_then(|tools| tools.get("web_search"))
    {
        Some(None) => None,
        Some(Some(binding)) => {
            let mut declaration: WebSearchDeclaration =
                serde_json::from_value(binding.request.clone()).ok()?;
            if let Some(base) = declared(protocol) {
                declaration.recognized_options.extend(
                    base.options_schema
                        .get("properties")
                        .and_then(Json::as_object)
                        .into_iter()
                        .flat_map(|properties| properties.keys().cloned()),
                );
                declaration.recognized_options.sort();
                declaration.recognized_options.dedup();
            }
            Some(declaration)
        }
        None => declared(protocol),
    }
}
