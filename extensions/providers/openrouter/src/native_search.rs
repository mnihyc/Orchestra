//! OpenRouter's explicit native-search wire forms, citations and original-unit fee quotes.
//! Shared registration, model admission, item writing and billing remain with the common consumer.
use orch_provider_contract::models::{ModelOffered, ModelSuggestion};
use orch_types::Json;
use serde_json::json;
use std::collections::BTreeMap;
use url::Url;

/// Native server-tool options and reported request counts.
pub const TOOL_DOCUMENTATION: &str =
    "https://openrouter.ai/docs/guides/features/server-tools/web-search";
/// Plugin, implicit model variant and citation format.
pub const PLUGIN_DOCUMENTATION: &str =
    "https://openrouter.ai/docs/guides/features/plugins/web-search";
/// Per-engine native quote table, separate from model token prices.
pub const PRICING_DOCUMENTATION: &str = "https://openrouter.ai/tool/web-search";

/// The documented API formats at their actual base paths.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Endpoint {
    /// OpenAI Chat Completions.
    Chat,
    /// OpenAI Responses.
    Responses,
    /// Anthropic Messages.
    Messages,
}
/// Exact protocol/base matching; a compatible proxy or arbitrary URL proves no provider ability.
#[must_use]
pub fn endpoint(protocol: &str, base: &Url) -> Option<Endpoint> {
    crate::known_vendor(base)?;
    match (protocol, base.path()) {
        ("openai-chat", "/api/v1" | "/api/v1/") => Some(Endpoint::Chat),
        ("openai-responses", "/api/v1" | "/api/v1/") => Some(Endpoint::Responses),
        ("anthropic-messages", "/api" | "/api/") => Some(Endpoint::Messages),
        _ => None,
    }
}
/// Both forms are available to generation models; the server form additionally requires tool calling.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Form {
    /// Preferred model-invoked server tool.
    ServerTool,
    /// Once-per-request web plugin, also selected by the online model variant.
    Plugin,
}
/// The plugin supplies live search independently of a model's own native provider engine.
/// An offer is not an enabled/default choice, and this mapping offers no page-fetch backend.
#[must_use]
pub fn offered(protocol: &str, base: &Url, model: &str) -> Option<ModelOffered> {
    endpoint(protocol, base)?;
    if model.is_empty() {
        return None;
    }
    Some(ModelOffered {
        web_search: Some(vec!["live".into()]),
        ..Default::default()
    })
}
/// Preserve manager choices and every unrelated observation.
pub fn enrich(protocol: &str, base: &Url, model: &mut ModelSuggestion) {
    if model
        .parameters
        .offered
        .as_ref()
        .and_then(|offered| offered.web_search.as_ref())
        .is_none()
        && let Some(offered) = offered(protocol, base, &model.model_name)
    {
        model
            .parameters
            .offered
            .get_or_insert_with(ModelOffered::default)
            .web_search = offered.web_search;
        model
            .origins
            .insert("/parameters/offered/web_search".into(), "catalogue".into());
    }
}
/// One source-owned engine choice. Auto is a routing choice, not an observed Exa/native price.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Engine {
    /// Vendor chooses an available native provider, otherwise its fallback.
    Auto,
    /// Pass through the selected model provider's native search.
    Native,
    /// Exa search.
    Exa,
    /// Firecrawl search and actual returned-result fees.
    Firecrawl,
    /// Parallel search.
    Parallel,
    /// Perplexity search.
    Perplexity,
}
impl Engine {
    /// A documented value, never substring inference from a model name.
    #[must_use]
    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "auto" => Some(Self::Auto),
            "native" => Some(Self::Native),
            "exa" => Some(Self::Exa),
            "firecrawl" => Some(Self::Firecrawl),
            "parallel" => Some(Self::Parallel),
            "perplexity" => Some(Self::Perplexity),
            _ => None,
        }
    }
    /// The wire word.
    #[must_use]
    pub const fn word(self) -> &'static str {
        match self {
            Self::Auto => "auto",
            Self::Native => "native",
            Self::Exa => "exa",
            Self::Firecrawl => "firecrawl",
            Self::Parallel => "parallel",
            Self::Perplexity => "perplexity",
        }
    }
}
/// Original USD fee terms for a known selected engine/mode. No result count is estimated here.
/// None for native/auto means provider-dependent, not zero and not a guessed fixed search rate.
#[must_use]
pub fn price(engine: Engine, mode: Option<&str>) -> Option<Json> {
    let (base, included, extra) = match engine {
        Engine::Auto | Engine::Native => return None,
        Engine::Exa => (
            match mode.unwrap_or("auto") {
                "instant" | "fast" | "auto" => "0.007",
                "deep-lite" | "deep" => "0.012",
                "deep-reasoning" => "0.015",
                _ => return None,
            },
            10,
            "0.001",
        ),
        Engine::Parallel => (
            match mode.unwrap_or("basic") {
                "turbo" | "fast" => "0.001",
                "basic" | "advanced" => "0.005",
                _ => return None,
            },
            10,
            "0.001",
        ),
        Engine::Perplexity => ("0.005", 0, "0"),
        Engine::Firecrawl => ("0.005", 0, "0.0125"),
    };
    Some(
        json!({"unit":"USD","per_call":base,"included_results_per_call":included,"per_additional_result":extra}),
    )
}
/// Source-owned wire grammar; engine_mode stays distinct from Orch's disabled/live/service mode.
/// Defaults describe vendor behavior and never constrain an explicit value.
#[must_use]
pub fn options_schema(form: Form, engine: Engine) -> Json {
    let mut properties = serde_json::Map::from_iter([
        (
            "engine".into(),
            json!({"type":"string","enum":["auto","native","exa","firecrawl","parallel","perplexity"]}),
        ),
        (
            "max_results".into(),
            json!({"type":"integer","minimum":1,"maximum":if engine==Engine::Perplexity{20}else{25},"default":5}),
        ),
        (
            "search_context_size".into(),
            json!({"type":"string","enum":["low","medium","high"]}),
        ),
        (
            "allowed_domains".into(),
            json!({"type":"array","items":{"type":"string"}}),
        ),
        (
            "excluded_domains".into(),
            json!({"type":"array","items":{"type":"string"}}),
        ),
    ]);
    properties.insert("engine_mode".into(),match engine {
        Engine::Exa=>json!({"type":"string","enum":["instant","fast","auto","deep-lite","deep","deep-reasoning"]}),
        Engine::Parallel=>json!({"type":"string","enum":["turbo","fast","basic","advanced"]}),
        _=>json!({"type":"string"}),
    });
    match form {
        Form::ServerTool => {
            properties.insert("max_uses".into(), json!({"type":"integer","minimum":0}));
            properties.insert(
                "max_total_results".into(),
                json!({"type":"integer","minimum":1}),
            );
            properties.insert(
                "max_characters".into(),
                json!({"type":"integer","minimum":1,"maximum":100000}),
            );
            properties.insert("user_location".into(), json!({"type":"object"}));
        }
        Form::Plugin => {
            properties.insert("search_prompt".into(), json!({"type":"string"}));
        }
    }
    json!({"type":"object","properties":properties,"additionalProperties":false})
}
/// Source data for a general native-search builder: array location and option translations are explicit.
#[must_use]
pub fn declaration(protocol: &str, base: &Url, model: &str, form: Form) -> Option<Json> {
    offered(protocol, base, model)?;
    let paths = match endpoint(protocol, base)? {
        Endpoint::Chat => json!(["/chat/completions"]),
        Endpoint::Responses => json!(["/responses"]),
        Endpoint::Messages => json!(["/v1/messages"]),
    };
    let (container, tool, pointer, identities) = match form {
        Form::ServerTool => (
            "/tools",
            json!({"type":"openrouter:web_search","parameters":{}}),
            "/parameters",
            json!([{"type":"openrouter:web_search"}]),
        ),
        Form::Plugin => ("/plugins", json!({"id":"web"}), "", json!([{"id":"web"}])),
    };
    Some(
        json!({"container":container,"paths":paths,"path_suffixes":[],"tool":tool,"options_pointer":pointer,"identities":identities,
        "modes":{"live":{}},"options_schema":options_schema(form,Engine::Auto),"option_names":if form==Form::Plugin{json!({"engine_mode":"mode","allowed_domains":"include_domains","excluded_domains":"exclude_domains"})}else{json!({"engine_mode":"mode"})},
        "root_option_names":if form==Form::Plugin{json!({"search_context_size":"/web_search_options/search_context_size"})}else{json!({})},
        "model_variant_identity":if form==Form::Plugin{json!("online")}else{Json::Null},"optional_options_object":form==Form::ServerTool,"requires_tool_calling":form==Form::ServerTool,"supports_fetch":false,
        "automatic_engine_price":"provider_dependent","root_selection_identity":if form==Form::Plugin{json!({"web_search_options":null})}else{Json::Null}}),
    )
}
/// Build only the source's native fragment. The common merger preserves existing client tools/plugins.
/// This neither resolves settings nor admits a model or decides who may spend its provider's money.
pub fn request_patch(form: Form, options: &Json) -> Result<Json, &'static str> {
    let engine = options
        .get("engine")
        .map(|value| {
            value
                .as_str()
                .and_then(Engine::parse)
                .ok_or("unknown search engine")
        })
        .transpose()?
        .unwrap_or(Engine::Auto);
    orch_types::schema::validate(&options_schema(form, engine), options)
        .map_err(|_| "search options do not fit the selected engine/form")?;
    let mut values = options
        .as_object()
        .cloned()
        .ok_or("search options must be an object")?;
    if let Some(mode) = values.remove("engine_mode") {
        values.insert("mode".into(), mode);
    }
    if form == Form::Plugin {
        if engine == Engine::Auto {
            values.remove("engine");
        }
        if let Some(domains) = values.remove("allowed_domains") {
            values.insert("include_domains".into(), domains);
        }
        if let Some(domains) = values.remove("excluded_domains") {
            values.insert("exclude_domains".into(), domains);
        }
        let context = values.remove("search_context_size");
        values.insert("id".into(), json!("web"));
        let mut patch = json!({"plugins":[values]});
        if let Some(context) = context {
            patch["web_search_options"] = json!({"search_context_size":context});
        }
        Ok(patch)
    } else {
        Ok(json!({"tools":[{"type":"openrouter:web_search","parameters":values}]}))
    }
}
/// What the actual wire request selected, including the documented implicit plugin alias.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Selection {
    /// The one active wire form.
    pub form: Form,
    /// Exact native engine; absent selection remains Auto.
    pub engine: Engine,
    /// The engine's native mode, distinct from neutral live.
    pub engine_mode: Option<String>,
    /// Original native options, not rewritten to match a different engine.
    pub options: Json,
    /// The configured routing name itself implies the plugin and cannot silently be renamed.
    pub implicit_online: bool,
    /// Native root search options, retained independently of a tool's nested parameters.
    pub root_options: Option<Json>,
}
/// Inspect owned native identities only. A same-named client function is not a server tool.
pub fn selection(body: &Json) -> Result<Option<Selection>, &'static str> {
    if body.get("tools").is_some_and(|value| !value.is_array())
        || body.get("plugins").is_some_and(|value| !value.is_array())
    {
        return Err("native tool/plugin containers must be arrays");
    }
    let tools = body
        .get("tools")
        .and_then(Json::as_array)
        .into_iter()
        .flatten()
        .filter(|value| value.get("type").and_then(Json::as_str) == Some("openrouter:web_search"))
        .collect::<Vec<_>>();
    let plugins = body
        .get("plugins")
        .and_then(Json::as_array)
        .into_iter()
        .flatten()
        .filter(|value| value.get("id").and_then(Json::as_str) == Some("web"))
        .collect::<Vec<_>>();
    let online = body
        .get("model")
        .and_then(Json::as_str)
        .is_some_and(|model| model.split(':').skip(1).any(|part| part == "online"));
    if tools.len() > 1 || plugins.len() > 1 || !tools.is_empty() && (!plugins.is_empty() || online)
    {
        return Err("a request must select one native search form");
    }
    let root_options = body.get("web_search_options").cloned();
    if root_options
        .as_ref()
        .is_some_and(|value| !value.is_object())
    {
        return Err("native root search options must be an object");
    }
    let (form, options) = if let Some(tool) = tools.first() {
        (
            Form::ServerTool,
            tool.get("parameters").cloned().unwrap_or_else(|| json!({})),
        )
    } else if let Some(plugin) = plugins.first() {
        let mut options = plugin
            .as_object()
            .cloned()
            .ok_or("web plugin must be an object")?;
        options.remove("id");
        (Form::Plugin, Json::Object(options))
    } else if online || root_options.is_some() {
        (Form::Plugin, json!({}))
    } else {
        return Ok(None);
    };
    let engine = options
        .get("engine")
        .map(|value| {
            value
                .as_str()
                .and_then(Engine::parse)
                .ok_or("unknown search engine")
        })
        .transpose()?
        .unwrap_or(Engine::Auto);
    if !options.is_object() {
        return Err("native search options must be an object");
    }
    Ok(Some(Selection {
        form,
        engine,
        engine_mode: options
            .get("mode")
            .and_then(Json::as_str)
            .map(str::to_owned),
        options,
        implicit_online: online,
        root_options,
    }))
}
/// Remove native fragments only. An implicit routing variant needs a different explicit model choice.
pub fn disable(body: &mut Json) -> Result<(), &'static str> {
    let selected = selection(body)?;
    if selected.as_ref().is_some_and(|value| value.implicit_online) {
        return Err("the configured online model variant requires native search");
    }
    if let Some(tools) = body.get_mut("tools").and_then(Json::as_array_mut) {
        tools.retain(|value| {
            value.get("type").and_then(Json::as_str) != Some("openrouter:web_search")
        });
    }
    if let Some(plugins) = body.get_mut("plugins").and_then(Json::as_array_mut) {
        plugins.retain(|value| value.get("id").and_then(Json::as_str) != Some("web"));
    }
    if let Some(body) = body.as_object_mut() {
        body.remove("web_search_options");
    }
    Ok(())
}

fn annotations(node: &Json, found: &mut Vec<Json>) {
    for annotation in node
        .get("annotations")
        .and_then(Json::as_array)
        .into_iter()
        .flatten()
    {
        citation(annotation, found);
    }
}
fn citation(value: &Json, found: &mut Vec<Json>) {
    if value.get("type").and_then(Json::as_str) != Some("url_citation") {
        return;
    }
    let value = value.get("url_citation").unwrap_or(value);
    let Some(url) = value.get("url").and_then(Json::as_str) else {
        return;
    };
    let title = value.get("title").and_then(Json::as_str).unwrap_or(url);
    let mut result = json!({"title":title,"url":url});
    for field in ["content", "snippet", "date"] {
        if let Some(value) = value.get(field).filter(|value| value.is_string()) {
            result[field] = value.clone();
        }
    }
    found.push(result);
}
/// Read structured citation locations, never JSON-looking text or client tool arguments.
#[must_use]
pub fn results(value: &Json) -> Vec<Json> {
    let mut found = Vec::new();
    annotations(value, &mut found);
    if let Some(message) = value.get("message") {
        annotations(message, &mut found);
    }
    for choice in value
        .get("choices")
        .and_then(Json::as_array)
        .into_iter()
        .flatten()
    {
        for key in ["message", "delta"] {
            if let Some(node) = choice.get(key) {
                annotations(node, &mut found);
            }
        }
    }
    for item in value
        .get("output")
        .or_else(|| value.pointer("/response/output"))
        .and_then(Json::as_array)
        .into_iter()
        .flatten()
        .filter(|item| item.get("type").and_then(Json::as_str) == Some("message"))
    {
        for content in item
            .get("content")
            .and_then(Json::as_array)
            .into_iter()
            .flatten()
        {
            annotations(content, &mut found);
        }
    }
    if value.get("type").and_then(Json::as_str) == Some("response.output_text.annotation.added")
        && let Some(annotation) = value.get("annotation")
    {
        citation(annotation, &mut found);
    }
    found
}
/// Request counters and a vendor-reported total charge; no fee is reconstructed from citations.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Usage {
    /// Reported search queries. This field alone proves no per-query success or result count.
    pub search_requests: Option<i64>,
    /// Actual original-unit total, already inclusive of applicable fees. Never add quotes again.
    pub reported_total_usd: Option<Json>,
}
fn reported_cost(value: &Json) -> Option<Json> {
    match value {
        Json::Number(number)
            if number
                .as_f64()
                .is_some_and(|number| number.is_finite() && number >= 0.0) =>
        {
            Some(value.clone())
        }
        Json::String(text)
            if orch_provider_contract::limits::is_decimal(text) && !text.starts_with('-') =>
        {
            Some(value.clone())
        }
        _ => None,
    }
}
/// A view over already-decoded frames. Raw forwarding, token parsing and the common item writer stay intact.
#[derive(Debug, Default)]
pub struct Stream {
    results: Vec<Json>,
    positions: BTreeMap<String, usize>,
    usage: Usage,
}
impl Stream {
    /// Admit only a known endpoint/model mapping; it offers no fetch.
    #[must_use]
    pub fn new(protocol: &str, base: &Url, model: &str) -> Option<Self> {
        offered(protocol, base, model)?;
        Some(Self::default())
    }
    /// One native frame's newly supplied citation metadata, deduplicated by actual URL across snapshots.
    pub fn frame(&mut self, value: &Json) {
        for result in results(value) {
            let Some(url) = result.get("url").and_then(Json::as_str) else {
                continue;
            };
            if let Some(index) = self.positions.get(url).copied() {
                if let Some(old) = self.results.get_mut(index)
                    && let (Some(old), Some(new)) = (old.as_object_mut(), result.as_object())
                {
                    old.extend(new.iter().map(|(key, value)| (key.clone(), value.clone())));
                }
            } else {
                self.positions.insert(url.into(), self.results.len());
                self.results.push(result);
            }
        }
        if let Some(usage) = value
            .get("usage")
            .or_else(|| value.pointer("/response/usage"))
            .filter(|usage| usage.is_object())
        {
            if let Some(requests) = usage
                .pointer("/server_tool_use_details/web_search_requests")
                .and_then(Json::as_i64)
                .filter(|value| *value >= 0)
            {
                self.usage.search_requests = Some(requests);
            }
            if let Some(cost) = usage.get("cost").and_then(reported_cost) {
                self.usage.reported_total_usd = Some(cost);
            }
        }
    }
    /// Actual source results, including available native excerpts.
    #[must_use]
    pub fn results(&self) -> &[Json] {
        &self.results
    }
    /// Actual aggregate facts, not sums of streamed snapshots.
    #[must_use]
    pub fn usage(&self) -> &Usage {
        &self.usage
    }
    /// The existing search presentation with this caller's actual service query.
    #[must_use]
    pub fn presentation(&self, query: &str) -> Json {
        json!({"kind":"web_search","query":query,"results":self.results.iter().filter_map(|result|Some(json!({"title":result.get("title")?.as_str()?,"url":result.get("url")?.as_str()?}))).collect::<Vec<_>>()})
    }
}

#[cfg(test)]
mod tests;
