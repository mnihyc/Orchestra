//! Apply a protocol's declared native search tool without replacing client tools.

use std::collections::BTreeMap;

use orch_types::Json;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::json;

/// A wire protocol owns this declaration. Shared request code knows no vendor tool names.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct WebSearchDeclaration {
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

impl WebSearchDeclaration {
    /// The wire protocol validates the full path before this generation-only applicability check.
    #[must_use]
    pub fn accepts(&self, path: &str) -> bool {
        let path = path.split('?').next().unwrap_or(path);
        self.paths.iter().any(|allowed| allowed == path)
            || self
                .path_suffixes
                .iter()
                .any(|suffix| path.ends_with(suffix))
    }

    /// Endpoint defaults never replace an existing tool version, mode or explicit tool option.
    pub fn apply_defaults(
        &self,
        path: &str,
        body: &mut Json,
        defaults: &Json,
    ) -> Result<(), &'static str> {
        if !self.accepts(path) {
            return Ok(());
        }
        let mut options = orch_provider_contract::search::options(defaults)?;
        options.remove("mode");
        orch_types::schema::validate(&self.options_schema, &Json::Object(options.clone()))
            .map_err(|_| "web search options do not fit this protocol")?;
        self.selected_mode(body)?;
        if let Some(tool) = body
            .get_mut("tools")
            .and_then(Json::as_array_mut)
            .into_iter()
            .flatten()
            .find(|tool| self.matches_tool(tool))
        {
            let existing = tool
                .pointer_mut(&self.options_pointer)
                .and_then(Json::as_object_mut)
                .ok_or("the search tool has no options object")?;
            crate::parameters::overlay(existing, options, true);
        }
        Ok(())
    }

    /// Null in an identity means that the member exists, for tools identified by an object key.
    #[must_use]
    pub fn matches_tool(&self, tool: &Json) -> bool {
        self.identities.iter().any(|identity| {
            identity.as_object().is_some_and(|identity| {
                identity.iter().all(|(key, value)| {
                    if value.is_null() {
                        tool.get(key).is_some()
                    } else {
                        tool.get(key) == Some(value)
                    }
                })
            })
        })
    }

    /// The actual native mode, suitable for the shared final provider-policy check.
    pub fn selected_mode(&self, body: &Json) -> Result<String, &'static str> {
        let tools: Vec<_> = body
            .get("tools")
            .and_then(Json::as_array)
            .into_iter()
            .flatten()
            .filter(|tool| self.matches_tool(tool))
            .collect();
        if tools.len() > 1 {
            return Err("a request must select at most one hosted search tool");
        }
        let Some(tool) = tools.first() else {
            return Ok("disabled".to_owned());
        };
        if let Some(mode) = self
            .options_schema
            .get("x-orch-mode-overrides")
            .and_then(Json::as_array)
            .into_iter()
            .flatten()
            .find_map(|rule| {
                let identity = rule.get("identity")?.as_object()?;
                identity
                    .iter()
                    .all(|(key, value)| tool.get(key) == Some(value))
                    .then(|| rule.get("mode").and_then(Json::as_str))
                    .flatten()
            })
        {
            return Ok(mode.to_owned());
        }
        let options = tool
            .pointer(&self.options_pointer)
            .ok_or("the search tool has no options object")?;
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
            .map_or_else(|| "live".to_owned(), |(mode, _)| mode.clone()))
    }

    /// Lift only declared mutable search options out of the protocol-owned tools array.
    /// Tool identity, versions and other builder-owned metadata remain in the original tool.
    pub fn selection(&self, body: &Json) -> Result<Json, &'static str> {
        let mode = self.selected_mode(body)?;
        let mut selected = serde_json::Map::new();
        selected.insert("mode".to_owned(), json!(mode));
        if let Some(tool) = body
            .get("tools")
            .and_then(Json::as_array)
            .into_iter()
            .flatten()
            .find(|tool| self.matches_tool(tool))
        {
            let options = tool
                .pointer(&self.options_pointer)
                .and_then(Json::as_object)
                .ok_or("the search tool has no options object")?;
            for name in self
                .options_schema
                .get("properties")
                .and_then(Json::as_object)
                .into_iter()
                .flat_map(|properties| properties.keys())
            {
                if let Some(value) = options.get(name) {
                    selected.insert(name.clone(), value.clone());
                }
            }
        }
        Ok(Json::Object(selected))
    }

    /// Apply already-constrained options without replacing a native tool's identity or version.
    pub(crate) fn apply_constrained(
        &self,
        body: &mut Json,
        selected: &Json,
    ) -> Result<(), &'static str> {
        let mode = orch_provider_contract::search::mode(selected)?;
        if mode == "disabled" {
            if let Some(tools) = body.get_mut("tools").and_then(Json::as_array_mut) {
                tools.retain(|tool| !self.matches_tool(tool));
            }
            return Ok(());
        }
        let mut options = orch_provider_contract::search::options(selected)?;
        options.remove("mode");
        options.extend(
            self.modes
                .get(mode)
                .and_then(Json::as_object)
                .ok_or("this protocol does not support the selected web search mode")?
                .clone(),
        );
        let tool = body
            .get_mut("tools")
            .and_then(Json::as_array_mut)
            .into_iter()
            .flatten()
            .find(|tool| self.matches_tool(tool))
            .ok_or("the request did not select the constrained search tool")?;
        tool.pointer_mut(&self.options_pointer)
            .and_then(Json::as_object_mut)
            .ok_or("the search tool has no options object")?
            .extend(options);
        Ok(())
    }

    /// Whether the actual request selected a hosted search tool.
    #[must_use]
    pub fn selected(&self, body: &Json) -> bool {
        body.get("tools")
            .and_then(Json::as_array)
            .into_iter()
            .flatten()
            .any(|tool| self.matches_tool(tool))
    }

    /// Update only the declared hosted search tool. Other tools retain their order and contents.
    pub fn apply(&self, path: &str, body: &mut Json, selected: &Json) -> Result<(), &'static str> {
        let mode = orch_provider_contract::search::mode(selected)?;
        if !self.accepts(path) {
            return if mode == "disabled" {
                Ok(())
            } else {
                Err("web search does not apply to this request")
            };
        }
        let mut options = orch_provider_contract::search::options(selected)?;
        options.remove("mode");
        orch_types::schema::validate(&self.options_schema, &Json::Object(options.clone()))
            .map_err(|_| "web search options do not fit this protocol")?;
        let members = body
            .as_object_mut()
            .ok_or("model request must be an object")?;
        if members.get("tools").is_some_and(|tools| !tools.is_array()) {
            return Err("request tools must be an array");
        }
        if let Some(tools) = members.get_mut("tools").and_then(Json::as_array_mut) {
            tools.retain(|tool| !self.matches_tool(tool));
        }
        if mode == "disabled" {
            return Ok(());
        }
        let mode_options = self
            .modes
            .get(mode)
            .ok_or("this protocol does not support the selected web search mode")?;
        options.extend(
            mode_options
                .as_object()
                .ok_or("invalid web search mode declaration")?
                .clone(),
        );
        let mut tool = self.tool.clone();
        tool.pointer_mut(&self.options_pointer)
            .and_then(Json::as_object_mut)
            .ok_or("invalid web search options pointer")?
            .extend(options);
        members
            .entry("tools".to_owned())
            .or_insert_with(|| json!([]))
            .as_array_mut()
            .ok_or("request tools must be an array")?
            .push(tool);
        Ok(())
    }
}

/// Read a protocol's data declaration, without selecting a vendor in request code.
#[must_use]
pub fn declared(protocol: &str) -> Option<WebSearchDeclaration> {
    crate::protocol::parameter_format(protocol).web_search?;
    serde_json::from_value(orch_provider_contract::search::declaration(protocol)?).ok()
}
