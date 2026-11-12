//! Paid service commands retain their real tool source and original input authority.
use super::{CallContext, source};
use crate::{
    Engine, EngineError,
    commands::{CommandAuthority, Outgoing},
};
use orch_authz::{Caller, Principal};
use orch_db::{
    Conn,
    queries::{link, resources, threads},
};
use orch_link_contract::commands::{ProviderService, ProviderServiceResult, SerialKey};
use orch_provider_contract::{
    builtins::BuiltinSources,
    capabilities::{Endpoint, Price},
    services::ServiceConfig,
};
use orch_settings::Place;
use orch_settings_contract::registry::keys;
use orch_types::{Clock, Hash, Id, Json, Time};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
mod accepted;
mod failure;
pub use failure::{CapabilityFailure, CapabilityPause};
pub mod delegation;
pub(crate) use accepted::reap as reap_accepted_results;
pub use accepted::{AcceptedCapabilityResult, CapabilityResultReceipt, NativeImage};
mod model;
mod origin;
mod recovery;
mod recovery_binding;
mod standalone;
pub(crate) use origin::NativeSource;
use origin::Origin;
pub use origin::PaidAttribution;
pub mod selection;

#[derive(Clone, Copy)]
enum ResultBlob {
    Api,
    Native,
}
impl ResultBlob {
    fn member(self) -> &'static str {
        match self {
            Self::Api => "output_blob",
            Self::Native => "native_result_blob",
        }
    }
    fn kind(self) -> &'static str {
        match self {
            Self::Api => "provider.service",
            Self::Native => "provider.generate",
        }
    }
}

type CurrentCallers = Vec<(Option<Caller>, Option<Time>)>;

/// Actual metadata from the same native model binding used by tool planning.
/// The unmodified model ceiling is separate from the scratch copy used for option validation.
pub(crate) struct ModelBackendFacts {
    pub endpoint: Endpoint,
    pub parameters: orch_provider_contract::models::ModelParameters,
    pub binding: orch_provider_contract::capabilities::Binding,
    pub options: Json,
    pub native_switches:
        BTreeMap<String, BTreeMap<String, orch_provider_contract::capabilities::NativeSwitch>>,
}
pub(crate) fn model_backend_facts(
    sources: &BuiltinSources,
    upstreams: &Json,
    row: &resources::thread_engine::ModelRow,
    capability: &str,
) -> Result<Option<ModelBackendFacts>, EngineError> {
    model::projection(sources, upstreams, row, capability)
}

/// Every adapter's managed web calls use the same stored item family as native web actions.
/// Raw input/output remain untouched, including the fetched document and failed native replies.
pub(crate) fn normalize_item(payload: &mut Json) {
    let tool = &payload["tool"];
    if tool["kind"] != "orch" {
        return;
    }
    let search = match tool["tool"].as_str() {
        Some("web_search") => true,
        Some("web_fetch") => false,
        _ => return,
    };
    let arguments = &tool["arguments"];
    let decode = |value: &Json| -> Option<Json> {
        if let Some(text) = value.as_str() {
            serde_json::from_str(text).ok()
        } else if value.is_object() || value.is_array() {
            Some(value.clone())
        } else {
            None
        }
    };
    let body = decode(&tool["result"]).and_then(|body| {
        if let Some(value) = body
            .get("structuredContent")
            .filter(|value| value.is_object() || value.is_array())
        {
            return Some(value.clone());
        }
        if let Some(content) = body
            .get("content")
            .and_then(Json::as_array)
            .filter(|content| content.len() == 1)
            && content[0]["type"] == "text"
        {
            return decode(&content[0]["text"]);
        }
        Some(body)
    });
    if let Some(body) = body.as_ref().filter(|body| body["state"] == "paused") {
        payload["tool"]["result"] = body.clone();
        return;
    }
    let query = if search {
        arguments["query"].as_str().unwrap_or_default()
    } else {
        ""
    };
    let url = arguments["url"].as_str();
    let results: Vec<Json> = if search {
        body.as_ref().and_then(|body| body.get("results")).and_then(Json::as_array)
            .into_iter().flatten().filter_map(|result| {
                let url = result.get("url").and_then(Json::as_str)?;
                Some(serde_json::json!({"url":url,"title":result.get("title").and_then(Json::as_str).unwrap_or(url)}))
            }).collect()
    } else {
        body.as_ref().filter(|body| body.get("content").is_some_and(Json::is_string)).and_then(|body| {
            let url = body.get("url").and_then(Json::as_str).or(url)?;
            Some(serde_json::json!({"url":url,"title":body.get("title").and_then(Json::as_str).unwrap_or(url)}))
        }).into_iter().collect()
    };
    let action = if search {
        serde_json::json!({"type":"search"})
    } else {
        serde_json::json!({"type":"open_page","url":url})
    };
    payload["tool"] =
        serde_json::json!({"kind":"web_search","action":action,"query":query,"results":results});
}

/// Private factual command identity; only the actual source producer constructs it.
#[derive(Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ServiceCommand {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    recovery: Option<recovery_binding::Binding>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    model: Option<model::ModelPlan>,
    origin: Origin,
    command: Id,
    machine: Id,
    body: Hash,
    provider: Id,
    offering: String,
    capability: String,
    upstreams: Json,
    config: ServiceConfig,
    backend: Option<selection::Selection>,
    groups: Vec<String>,
    unit: String,
    counter: Option<String>,
    quote: Option<Price>,
    explicit_price: bool,
    entry: Option<Id>,
}

impl std::fmt::Debug for ServiceCommand {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("ServiceCommand([private])")
    }
}

async fn backend(
    conn: &mut Conn,
    thread: Id,
    sources: &BuiltinSources,
    capability: &str,
) -> Result<selection::Selection, EngineError> {
    let root = selection::load(
        conn,
        thread,
        sources.capabilities().declarations(),
        capability,
    )
    .await?
    .ok_or(EngineError::StaleToolSource)?;
    recovery_binding::selected(root)
}

fn api_offering<'a>(choice: &'a selection::Selection, _capability: &str) -> Option<(Id, &'a str)> {
    if choice.executor != "api-backed" {
        return None;
    }
    choice
        .offering()
        .map(|offering| (offering.provider, offering.offering.as_str()))
}

async fn service_mode(
    conn: &mut Conn,
    thread: Id,
    declarations: &orch_provider_contract::capabilities::Declarations,
    capability: &str,
) -> Result<bool, EngineError> {
    let Some(key) = declarations
        .capabilities
        .get(capability)
        .and_then(|capability| capability.mode_setting.as_deref())
    else {
        return Ok(false);
    };
    Ok(selection::setting(conn, thread, key)
        .await?
        .as_ref()
        .and_then(Json::as_str)
        == Some("service"))
}

/// Every contributing list applies, including overlapping family and group patterns.
async fn service_use(
    conn: &mut Conn,
    caller: &Caller,
    thread: Id,
    provider: Id,
    offering: &str,
    groups: &[String],
    clock: &dyn Clock,
) -> Result<Option<Option<Id>>, EngineError> {
    let principal = Principal::load(conn, caller.clone(), clock.time()).await?;
    let policies = crate::access::resource_policies(conn, provider).await?;
    let usable =
        orch_authz::usable::usable(conn, &principal, provider, Some(thread), &policies).await?;
    let Some(granted) = usable.granted() else {
        return Ok(None);
    };
    // One entry has to allow the service.
    let allowing = granted.allowing(&orch_authz::options::Wanted {
        service: Some((offering, groups)),
        ..orch_authz::options::Wanted::default()
    });
    if allowing.is_empty() {
        return Ok(None);
    }
    let def = super::definition_of(keys::SERVICES_ALLOWED)?;
    let snapshot = orch_settings::Snapshot::load(
        conn,
        Place::Node(thread),
        orch_settings::Wanted::Keys(&[def]),
    )
    .await?;
    for part in snapshot.resolve(def).parts {
        let patterns: Vec<String> = serde_json::from_value(part.value)?;
        if !orch_agent_contract::discovery::ModelPattern::list_names(&patterns, offering, groups) {
            return Ok(None);
        }
    }
    Ok(Some(granted.entry_among(&allowing)))
}

impl ServiceCommand {
    pub(crate) fn command_kind(&self) -> &'static str {
        match &self.model {
            Some(plan) if plan.complete => "provider.complete",
            Some(_) => "provider.generate",
            None => "provider.service",
        }
    }
    fn rate_source(&self) -> crate::rates::Source {
        if self.explicit_price {
            crate::rates::Source::Provider
        } else {
            crate::rates::Source::Catalogue
        }
    }

    fn matches(&self, command: &link::NewCommand<'_>) -> Result<bool, EngineError> {
        let key = SerialKey::Provider(self.provider).wire();
        Ok(command.id == self.command
            && command.machine_id == self.machine
            && command.kind == self.command_kind()
            && command.serial_key == key.as_deref()
            && source::hash_json(command.body.clone())? == self.body)
    }

    pub(crate) async fn lock_source(&self, conn: &mut Conn) -> Result<(), EngineError> {
        self.origin.lock(conn).await?;
        if let Some(recovery) = &self.recovery {
            recovery
                .lock(conn, self.origin.place()?, &self.capability)
                .await?;
        }
        Ok(())
    }

    pub(crate) async fn current_issue_callers(
        &self,
        conn: &mut Conn,
        command: &link::NewCommand<'_>,
        clock: &dyn Clock,
        access: &dyn crate::access::ResourceAccess,
    ) -> Result<Option<CurrentCallers>, EngineError> {
        self.checked(conn, command, None, clock, access)
            .await
            .or_else(current_refusal)
    }

    async fn checked(
        &self,
        conn: &mut Conn,
        command: &link::NewCommand<'_>,
        stored: Option<&link::CommandRow>,
        clock: &dyn Clock,
        access: &dyn crate::access::ResourceAccess,
    ) -> Result<Option<CurrentCallers>, EngineError> {
        if !self.matches(command)? || clock.now() >= command.deadline_at {
            #[cfg(test)]
            if std::env::var_os("ORCH_TEST_SERVICE_TRACE").is_some() {
                eprintln!("service_guard refusal 1");
            }
            return Ok(None);
        }
        let Some(origins) = self
            .origin
            .current(conn, command, stored, clock, access)
            .await?
        else {
            #[cfg(test)]
            if std::env::var_os("ORCH_TEST_SERVICE_TRACE").is_some() {
                eprintln!("service_guard refusal 2");
            }
            return Ok(None);
        };
        if let Some(recovery) = &self.recovery
            && !recovery.current(conn, self).await?
        {
            #[cfg(test)]
            if std::env::var_os("ORCH_TEST_SERVICE_TRACE").is_some() {
                eprintln!("service_guard refusal 3");
            }
            return Ok(None);
        }
        let place = self.origin.place()?;
        if let Some(backend) = &self.backend {
            if backend
                .mode_setting
                .strip_prefix("agent.params.")
                .is_some_and(|member| {
                    origins.params.as_ref().is_some_and(|params| {
                        params.get(member).and_then(Json::as_str) != Some("service")
                    })
                })
                || !backend.is_current(conn, place).await?
                || self.model.is_none()
                    && api_offering(backend, &self.capability)
                        != Some((self.provider, self.offering.as_str()))
            {
                #[cfg(test)]
                if std::env::var_os("ORCH_TEST_SERVICE_TRACE").is_some() {
                    eprintln!("service_guard refusal 4");
                }
                return Ok(None);
            }
        } else if !matches!(self.origin, Origin::Native { .. }) {
            #[cfg(test)]
            if std::env::var_os("ORCH_TEST_SERVICE_TRACE").is_some() {
                eprintln!("service_guard refusal 5");
            }
            return Ok(None);
        }
        let Some(row) = resources::routes::providers(conn, &[self.provider])
            .await?
            .into_iter()
            .next()
        else {
            #[cfg(test)]
            if std::env::var_os("ORCH_TEST_SERVICE_TRACE").is_some() {
                eprintln!("service_guard refusal 6");
            }
            return Ok(None);
        };
        if row.kind != "api"
            || row.key_machine_id != Some(self.machine)
            || !crate::providers::request_status_available(&row.status)
            || row.upstreams != self.upstreams
            || crate::provider_policy::forbidden(conn, place, self.provider)
                .await?
                .is_some()
        {
            #[cfg(test)]
            if std::env::var_os("ORCH_TEST_SERVICE_TRACE").is_some() {
                eprintln!("service_guard refusal 7");
            }
            return Ok(None);
        }
        if self.model.is_some() {
            return model::current(self, conn, command, clock, &origins).await;
        }
        let configs: BTreeMap<String, ServiceConfig> =
            serde_json::from_value(row.service_offerings)?;
        if configs.get(&self.offering) != Some(&self.config) || !self.config.enabled {
            #[cfg(test)]
            if std::env::var_os("ORCH_TEST_SERVICE_TRACE").is_some() {
                eprintln!("service_guard refusal 8");
            }
            return Ok(None);
        }
        for caller in origins.callers() {
            if service_use(
                conn,
                caller,
                place,
                self.provider,
                &self.offering,
                &self.groups,
                clock,
            )
            .await?
            .is_none()
            {
                #[cfg(test)]
                if std::env::var_os("ORCH_TEST_SERVICE_TRACE").is_some() {
                    eprintln!("service_guard refusal 9");
                }
                return Ok(None);
            }
        }
        let book = crate::rates::RateBook::load(conn, place).await?;
        let priced = book
            .price_service(
                conn,
                self.quote.as_ref(),
                self.rate_source(),
                &self.unit,
                &orch_api_contract::usage::UsageMeasurement::Told { value: 1 },
            )
            .await?;
        if origins.callers.is_empty()
            || !crate::caps::service_room(
                conn,
                &crate::caps::ServiceCapAsk {
                    place,
                    provider: self.provider,
                    offering: &self.offering,
                    groups: &self.groups,
                    purpose: &self.capability,
                    person: origins.person,
                    priced: priced.credits_per_unit.is_some(),
                    harness_kind: origins.harness_kind.as_deref(),
                },
                clock.now(),
            )
            .await?
            || origins.callers.iter().any(|(caller, until)| {
                caller
                    .as_ref()
                    .is_some_and(|caller| !caller.key_deadline_allows(clock))
                    || until.is_some_and(|until| clock.time() >= until)
            })
        {
            #[cfg(test)]
            if std::env::var_os("ORCH_TEST_SERVICE_TRACE").is_some() {
                eprintln!("service_guard refusal 10");
            }
            return Ok(None);
        }
        Ok(Some(origins.callers))
    }

    pub(crate) async fn current_callers(
        &self,
        conn: &mut Conn,
        command: &link::CommandRow,
        clock: &dyn Clock,
        access: &dyn crate::access::ResourceAccess,
    ) -> Result<Option<CurrentCallers>, EngineError> {
        self.checked(
            conn,
            &link::NewCommand {
                id: command.id,
                machine_id: command.machine_id,
                kind: &command.kind,
                body: &command.body,
                serial_key: command.serial_key(),
                deadline_at: command.deadline_at,
            },
            Some(command),
            clock,
            access,
        )
        .await
        .or_else(current_refusal)
    }
}

fn current_refusal(error: EngineError) -> Result<Option<CurrentCallers>, EngineError> {
    match error {
        EngineError::StaleToolSource | EngineError::ToolCallIdentity => Ok(None),
        other => Err(other),
    }
}

pub(crate) fn endpoints(upstreams: &Json) -> Result<BTreeMap<String, Endpoint>, EngineError> {
    let upstreams: Vec<orch_api_contract::resources::Upstream> =
        serde_json::from_value(upstreams.clone())?;
    Ok(upstreams
        .into_iter()
        .map(|upstream| {
            (
                upstream.protocol.clone(),
                Endpoint {
                    interface: upstream.protocol,
                    address: upstream.base_url,
                    ..Endpoint::default()
                },
            )
        })
        .collect())
}

pub(crate) fn managed_tool(tool: super::OrchTool) -> bool {
    matches!(tool.name(), "web_search" | "web_fetch" | "image_generation")
}

impl Engine {
    /// Run an explicitly selected capability for the actual admitted tool source.
    /// Consumers keep their own offer/input/output materializers; ids in a fabricated call grant nothing.
    pub async fn execute_capability_service(
        &self,
        call: &CallContext,
        capability: &str,
    ) -> Result<Json, EngineError> {
        let sources = self
            .inner
            .capability_services
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
            .ok_or_else(|| {
                EngineError::unreadable("the capability service runner is not installed")
            })?;
        if managed_tool(call.tool) {
            recovery::execute(self, &sources, call, capability).await
        } else {
            self.execute_capability_once(&sources, call, capability)
                .await
        }
    }

    async fn execute_capability_once(
        &self,
        sources: &BuiltinSources,
        call: &CallContext,
        capability: &str,
    ) -> Result<Json, EngineError> {
        let selected = {
            let mut conn = self.db().acquire().await?;
            source::check_call(&mut conn, call).await?;
            backend(&mut conn, call.thread, sources, capability).await?
        };
        match selected.executor.as_str() {
            "api-backed" => {
                self.execute_web_api_service(sources, call, capability)
                    .await
            }
            "model-backed" => {
                self.execute_web_model_service(sources, call, capability)
                    .await
            }
            "program-session" => delegation::execute(self, call, capability).await,
            _ => Err(EngineError::unreadable(
                "the selected capability executor is not installed",
            )),
        }
    }

    /// Supply the same installed provider declarations as the API and the credential keepers.
    pub fn attach_capability_services(&self, sources: std::sync::Arc<BuiltinSources>) {
        *self
            .inner
            .capability_services
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(sources);
    }

    /// Ordinary session tools plus explicitly selected services implemented by this process.
    pub async fn offered_tools_for_session(
        &self,
        conn: &mut Conn,
        thread: &threads::ThreadRow,
        harness_takes_mcp: bool,
        now: Time,
    ) -> Result<Vec<orch_agent_contract::events::OrchTool>, EngineError> {
        let owned: Option<Id> =
            sqlx::query_scalar("SELECT t.input_id FROM turns t WHERE t.id=$1 AND t.thread_id=$2")
                .bind(thread.current_turn_id)
                .bind(thread.node_id)
                .fetch_optional(&mut *conn)
                .await
                .map_err(orch_db::DbError::from)?;
        if let Some(input) = owned
            && orch_db::queries::capability_work::for_input(conn, input)
                .await?
                .is_some()
        {
            return Ok(Vec::new());
        }
        let mut tools = super::offered_for_session(conn, thread, harness_takes_mcp, now).await?;
        if harness_takes_mcp {
            self.append_web_service_tools(conn, thread, &mut tools)
                .await?;
        }
        Ok(tools)
    }

    pub(crate) async fn append_web_service_tools(
        &self,
        conn: &mut Conn,
        thread: &threads::ThreadRow,
        tools: &mut Vec<orch_agent_contract::events::OrchTool>,
    ) -> Result<(), EngineError> {
        use orch_agent_contract::events::OrchTool;
        let sources = self
            .inner
            .capability_services
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone();
        let Some(sources) = sources else {
            return Ok(());
        };
        if super::side_reading(thread)
            == Some(orch_agent_contract::descriptor::SideReading::NoTools)
        {
            return Ok(());
        }
        if let Some(environment) = thread.environment_id {
            let enabled: Option<bool> =
                orch_settings::get(conn, Place::Environment(environment), keys::EXEC_ORCH_TOOLS)
                    .await?;
            if enabled == Some(false) {
                return Ok(());
            }
        }
        let declarations = sources.capabilities().declarations();
        for capability in declarations.capabilities.keys() {
            if !service_mode(conn, thread.node_id, declarations, capability).await? {
                continue;
            }
            let Some(choice) =
                selection::load(conn, thread.node_id, declarations, capability).await?
            else {
                continue;
            };
            let Some(name) = declarations
                .routes
                .get(&choice.reference.route)
                .and_then(|route| route.orch_tool.as_deref())
            else {
                continue;
            };
            let Ok(tool) = serde_json::from_value::<OrchTool>(Json::String(name.into())) else {
                continue;
            };
            let Some(reference) = choice.offering() else {
                continue;
            };
            if choice.executor == "program-session" {
                // The selected executor is installed even when its backend is temporarily unavailable.
                // Its actual call performs current Use/plan checks and can return a reason or recover.
                if !tools.contains(&tool) {
                    tools.push(tool);
                }
                continue;
            }
            let Some(row) = resources::routes::providers(conn, &[reference.provider])
                .await?
                .into_iter()
                .next()
            else {
                continue;
            };
            if row.kind != "api"
                || row.key_machine_id.is_none()
                || !crate::providers::request_status_available(&row.status)
                || crate::provider_policy::forbidden(conn, thread.node_id, row.id)
                    .await?
                    .is_some()
            {
                continue;
            }
            let implemented = if choice.executor == "model-backed" {
                match resources::thread_engine::model(conn, row.id, &reference.offering).await? {
                    Some(selected) => {
                        model::available(&sources, &row.upstreams, &selected, capability)?
                    }
                    None => false,
                }
            } else if choice.executor == "api-backed" {
                let config = serde_json::from_value(row.service_offerings)?;
                let catalog = orch_provider_contract::services::catalogue(
                    declarations,
                    &endpoints(&row.upstreams)?,
                    &config,
                );
                catalog
                    .offerings
                    .get(&reference.offering)
                    .is_some_and(|offering| {
                        if !offering.enabled
                            || !offering
                                .capabilities
                                .get(capability)
                                .is_some_and(|support| support.enabled)
                        {
                            return false;
                        }
                        let Some(endpoint) = catalog.endpoints.get(&offering.endpoint) else {
                            return false;
                        };
                        declarations
                            .interfaces
                            .get(&endpoint.interface)
                            .is_some_and(|interface| {
                                orch_provider_contract::services::json_implementation(
                                    &interface.implementation,
                                )
                            })
                            && orch_provider_contract::services::service_binding(
                                declarations,
                                endpoint,
                                offering,
                                capability,
                            )
                            .is_some()
                    })
            } else {
                false
            };
            if implemented && !tools.contains(&tool) {
                tools.push(tool);
            }
        }
        Ok(())
    }

    pub(crate) async fn web_service_call(
        &self,
        call: &CallContext,
    ) -> Result<super::Answer, EngineError> {
        use orch_agent_contract::events::OrchTool;
        let sources = self
            .inner
            .capability_services
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone();
        let Some(_sources) = sources else {
            return Ok(super::definitions::Refusal::new(
                super::definitions::Code::Closed,
                "The selected web service is not installed here.",
            )
            .into());
        };
        let capability = match call.tool {
            OrchTool::WebSearch => "web_search",
            OrchTool::WebFetch => "page_fetch",
            _ => return Err(EngineError::ToolCallIdentity),
        };
        #[cfg(test)]
        let result = match source::tests::prepared_service_result(self, call, capability).await {
            Some(result) => result?,
            None => self.execute_capability_service(call, capability).await?,
        };
        #[cfg(not(test))]
        let result = self.execute_capability_service(call, capability).await?;
        if result["state"] == "paused" {
            return Ok(super::Answer::Done(result));
        }
        Ok(super::Answer::Done(if call.tool == OrchTool::WebSearch {
            serde_json::json!({"query":call.arguments["query"],"results":result})
        } else {
            result
        }))
    }

    /// Actual API service execution for an already admitted managed web-tool source.
    /// The installed extension supplies codecs; the original input suppliers supply all authority.
    pub async fn execute_web_api_service(
        &self,
        sources: &BuiltinSources,
        call: &CallContext,
        capability: &str,
    ) -> Result<Json, EngineError> {
        let input = accepted::input(call, capability)?;
        let id = recovery_binding::command(source::resource_id(
            "provider-service",
            crate::ids::tool_result(call.thread, &call.call_ref),
        ))?;
        let (command, evidence, binding) = {
            let mut tx = self.db().begin().await?;
            source::check_call(&mut tx, call).await?;
            if let Some(retained) = link::command_with_authority(&mut tx, id).await? {
                let Some(CommandAuthority::CapabilityService(evidence)) =
                    retained.authority.map(serde_json::from_value).transpose()?
                else {
                    return Err(EngineError::ToolCallIdentity);
                };
                let current_source = source::Source::captured()?;
                if evidence.origin.tool().is_none_or(|(saved, original)| {
                    saved != &current_source
                        || original.tool != call.tool
                        || original.arguments != call.arguments
                }) || evidence.capability != capability
                {
                    return Err(EngineError::ToolCallIdentity);
                }
                let request: ProviderService =
                    serde_json::from_value(retained.command.body.clone())?;
                if request.input != input || request.input_images != accepted::images() {
                    return Err(EngineError::ToolCallIdentity);
                }
                let declarations = sources.capabilities().declarations();
                let catalog = orch_provider_contract::services::catalogue(
                    declarations,
                    &endpoints(&evidence.upstreams)?,
                    &BTreeMap::new(),
                );
                let endpoint = catalog
                    .offerings
                    .get(&request.offering)
                    .and_then(|offering| catalog.endpoints.get(&offering.endpoint))
                    .ok_or(EngineError::StaleToolSource)?;
                let offering = catalog
                    .offerings
                    .get(&request.offering)
                    .ok_or(EngineError::StaleToolSource)?;
                let binding = orch_provider_contract::services::service_binding(
                    declarations,
                    endpoint,
                    offering,
                    &request.capability,
                )
                .cloned()
                .ok_or(EngineError::StaleToolSource)?;
                if orch_provider_contract::services::definition_hash(endpoint, &binding)?
                    != request.definition
                {
                    return Err(EngineError::StaleToolSource);
                }
                let mut outgoing = Outgoing::new(
                    id,
                    retained.command.machine_id,
                    SerialKey::Provider(evidence.provider),
                    &request,
                )?;
                outgoing.authority = Some(CommandAuthority::CapabilityService(evidence.clone()));
                outgoing.deadline = Some(Time::new(retained.command.deadline_at));
                tx.commit().await.map_err(orch_db::DbError::from)?;
                (outgoing, *evidence, binding)
            } else {
                let origins = source::original_callers(
                    &mut tx,
                    call,
                    self.inner.clock.as_ref(),
                    self.inner.access.as_ref(),
                )
                .await?;
                let choice = backend(&mut tx, call.thread, sources, capability).await?;
                let (provider, offering) =
                    api_offering(&choice, capability).ok_or(EngineError::StaleToolSource)?;
                let row = resources::routes::providers(&mut tx, &[provider])
                    .await?
                    .into_iter()
                    .next()
                    .ok_or(EngineError::StaleToolSource)?;
                let machine = row.key_machine_id.ok_or(EngineError::StaleToolSource)?;
                let configured: BTreeMap<String, ServiceConfig> =
                    serde_json::from_value(row.service_offerings)?;
                let declarations = sources.capabilities().declarations();
                let catalog = orch_provider_contract::services::catalogue(
                    declarations,
                    &endpoints(&row.upstreams)?,
                    &configured,
                );
                let selected = catalog
                    .offerings
                    .get(offering)
                    .filter(|offering| {
                        offering.enabled
                            && offering
                                .capabilities
                                .get(capability)
                                .is_some_and(|support| support.enabled)
                    })
                    .ok_or(EngineError::StaleToolSource)?;
                let endpoint = catalog
                    .endpoints
                    .get(&selected.endpoint)
                    .ok_or(EngineError::StaleToolSource)?;
                let _interface = declarations
                    .interfaces
                    .get(&endpoint.interface)
                    .filter(|interface| {
                        orch_provider_contract::services::json_implementation(
                            &interface.implementation,
                        )
                    })
                    .ok_or(EngineError::StaleToolSource)?;
                let binding = orch_provider_contract::services::service_binding(
                    declarations,
                    endpoint,
                    selected,
                    capability,
                )
                .cloned()
                .ok_or(EngineError::StaleToolSource)?;
                orch_provider_contract::capabilities::render_request(&binding, &input)
                    .map_err(EngineError::unreadable)?;
                let definition = declarations
                    .capabilities
                    .get(capability)
                    .ok_or(EngineError::StaleToolSource)?;
                let expected_tool = declarations
                    .routes
                    .get(&choice.reference.route)
                    .is_some_and(|route| {
                        route.executor.as_deref() == Some("api-backed")
                            && route.orch_tool.as_deref() == Some(call.tool.name())
                    });
                if !expected_tool {
                    return Err(EngineError::ToolCallIdentity);
                }
                let unit = definition.ledger_unit.clone();
                let counters: Vec<_> = binding
                    .counters
                    .iter()
                    .filter(|(_, counter)| !counter.reported_cost && counter.unit == unit)
                    .map(|(name, _)| name.clone())
                    .collect();
                if counters.len() > 1 {
                    return Err(EngineError::unreadable(
                        "the service declares ambiguous charged quantities",
                    ));
                }
                let entry = {
                    let mut entry = None;
                    for caller in origins.callers() {
                        let allowed = service_use(
                            &mut tx,
                            caller,
                            call.thread,
                            provider,
                            offering,
                            &selected.groups,
                            self.inner.clock.as_ref(),
                        )
                        .await?
                        .ok_or(EngineError::StaleToolSource)?;
                        if Some(caller.user()) == origins.attribution().user_id {
                            entry = allowed;
                        }
                    }
                    entry
                };
                let request = ProviderService {
                    input_images: accepted::images(),
                    provider_id: provider,
                    offering: offering.into(),
                    capability: capability.into(),
                    input: input.clone(),
                    definition: orch_provider_contract::services::definition_hash(
                        endpoint, &binding,
                    )?,
                };
                let mut outgoing =
                    Outgoing::new(id, machine, SerialKey::Provider(provider), &request)?;
                if managed_tool(call.tool) {
                    outgoing.deadline = Some(source::capability_deadline(&mut tx, call).await?);
                }
                let evidence = ServiceCommand {
                    recovery: recovery_binding::binding(),
                    model: None,
                    origin: Origin::Tool {
                        source: source::Source::captured()?,
                        call: Box::new(call.clone()),
                    },
                    command: id,
                    machine,
                    body: source::hash_json(outgoing.body.clone())?,
                    provider,
                    offering: offering.into(),
                    capability: capability.into(),
                    upstreams: row.upstreams,
                    config: configured
                        .get(offering)
                        .cloned()
                        .ok_or(EngineError::StaleToolSource)?,
                    backend: Some(choice.clone()),
                    groups: selected.groups.clone(),
                    unit: unit.clone(),
                    counter: counters.into_iter().next(),
                    quote: selected
                        .prices
                        .get(&unit)
                        .or_else(|| binding.prices.get(&unit))
                        .cloned(),
                    explicit_price: catalog
                        .explicit_prices
                        .get(offering)
                        .is_some_and(|units| units.contains(&unit)),
                    entry,
                };
                outgoing.authority = Some(CommandAuthority::CapabilityService(Box::new(
                    evidence.clone(),
                )));
                tx.commit().await.map_err(orch_db::DbError::from)?;
                (outgoing, evidence, binding)
            }
        };
        let (command, outcome) = self.run_capability_command(command, &evidence).await?;
        let crate::commands::Outcome::Succeeded(raw) = outcome else {
            return Err(EngineError::unreadable(
                "the admitted service did not complete; its actual measurements were retained",
            ));
        };
        let mut result: ProviderServiceResult = serde_json::from_value(raw)?;
        if let Some(blob) = result.output_blob {
            result.output = self
                .service_result_blob(call, command.id, blob, ResultBlob::Api)
                .await?;
        }
        let mut tx = self.db().begin().await?;
        source::check_call(&mut tx, call).await?;
        tx.commit().await.map_err(orch_db::DbError::from)?;
        if accepted::capture(
            call,
            capability,
            command.id,
            &input,
            &binding,
            &result.output,
        )? {
            return Ok(Json::Null);
        }
        orch_provider_contract::capabilities::project_result(
            &binding,
            &input,
            &result.output,
            call.arguments
                .get("count")
                .and_then(Json::as_u64)
                .and_then(|count| usize::try_from(count).ok())
                .unwrap_or(super::definitions::RESULTS_MAX as usize),
        )
        .map_err(EngineError::unreadable)
    }

    pub(super) async fn run_capability_command(
        &self,
        command: Outgoing,
        evidence: &ServiceCommand,
    ) -> Result<(Outgoing, crate::commands::Outcome), EngineError> {
        if self.inner.commands.status(command.id).await?.is_none() {
            let mut tx = self.db().begin().await?;
            let key = command.key.wire();
            let proposed = link::NewCommand {
                id: command.id,
                machine_id: command.machine,
                kind: command.kind.as_str(),
                body: &command.body,
                serial_key: key.as_deref(),
                deadline_at: command.deadline.map_or_else(
                    || {
                        self.inner.clock.now()
                            + chrono::Duration::seconds(i64::from(command.kind.deadline_secs()))
                    },
                    Time::get,
                ),
            };
            evidence
                .current_issue_callers(
                    &mut tx,
                    &proposed,
                    self.inner.clock.as_ref(),
                    self.inner.access.as_ref(),
                )
                .await?
                .ok_or(EngineError::StaleToolSource)?;
            tx.commit().await.map_err(orch_db::DbError::from)?;
            self.ensure_provider_up(evidence.provider)
                .await?
                .map_err(EngineError::unreadable)?;
        }
        // Provider startup cannot extend the source, caller, setting, quote or cap boundary.
        let command = source::issue(self, command).await?;
        let outcome = self.inner.commands.wait(command.id).await?;
        reconcile(self, command.id).await?;
        Ok((command, outcome))
    }

    async fn service_result_blob(
        &self,
        call: &CallContext,
        command: Id,
        blob: orch_link_contract::commands::BlobRef,
        result_field: ResultBlob,
    ) -> Result<Json, EngineError> {
        use futures::StreamExt;
        let mut tx = self.db().begin().await?;
        source::check_call(&mut tx, call).await?;
        let actual: Option<Json> = sqlx::query_scalar("SELECT c.result->$5 FROM commands c JOIN thread_tool_commands e ON e.command_id=c.id WHERE c.id=$1 AND c.kind=$6 AND c.state='succeeded' AND e.thread_id=$2 AND e.turn_id=$3 AND e.answer_id=$4")
            .bind(command).bind(call.thread).bind(call.turn).bind(crate::ids::tool_result(call.thread,&call.call_ref)).bind(result_field.member()).bind(result_field.kind())
            .fetch_optional(&mut *tx).await.map_err(orch_db::DbError::from)?;
        if actual != Some(serde_json::to_value(blob)?)
            || !(0..=orch_link_contract::commands::PROVIDER_SERVICE_RESULT_MAX_BYTES as i64)
                .contains(&blob.size)
        {
            return Err(EngineError::ToolCallIdentity);
        }
        tx.commit().await.map_err(orch_db::DbError::from)?;
        let store = self
            .blobs()
            .ok_or_else(|| EngineError::unreadable("the service result store is unavailable"))?;
        let read = store
            .get(&blob.hash, None)
            .await
            .map_err(EngineError::unreadable)?;
        if read.meta.hash != blob.hash || read.meta.size != blob.size {
            return Err(EngineError::ToolCallIdentity);
        }
        let mut stream = read.body;
        let mut bytes = Vec::new();
        let mut verifier = orch_blob_contract::store::Verifier::new(blob.hash, Some(blob.size));
        while let Some(piece) = stream.next().await {
            let piece = piece.map_err(EngineError::unreadable)?;
            if bytes.len().saturating_add(piece.len())
                > orch_link_contract::commands::PROVIDER_SERVICE_RESULT_MAX_BYTES
            {
                return Err(EngineError::unreadable(
                    "the service result exceeds its declared transport budget",
                ));
            }
            verifier.update(&piece);
            bytes.extend_from_slice(&piece);
        }
        verifier.finish().map_err(EngineError::unreadable)?;
        Ok(serde_json::from_slice(&bytes)?)
    }
}

/// Factual completion accounting also runs after a process lost its tool waiter.
pub(crate) async fn reconcile(engine: &Engine, command: Id) -> Result<(), EngineError> {
    let mut tx = engine.db().begin().await?;
    let Some(retained) = link::command_with_authority(&mut tx, command).await? else {
        return Ok(());
    };
    if !matches!(
        retained.command.kind.as_str(),
        "provider.service" | "provider.generate" | "provider.complete"
    ) {
        return Ok(());
    }
    let Some(authority) = retained
        .authority
        .map(serde_json::from_value::<CommandAuthority>)
        .transpose()?
    else {
        return Ok(());
    };
    let Some(evidence) = authority.paid_service() else {
        return Ok(());
    };
    if !evidence.matches(&link::NewCommand {
        id: retained.command.id,
        machine_id: retained.command.machine_id,
        kind: &retained.command.kind,
        body: &retained.command.body,
        serial_key: retained.command.serial_key(),
        deadline_at: retained.command.deadline_at,
    })? {
        return Err(EngineError::ToolCallIdentity);
    }
    if evidence.model.is_some() {
        model::counted(engine, &mut tx, evidence, retained.command).await?;
        tx.commit().await.map_err(orch_db::DbError::from)?;
        return Ok(());
    }
    let measured: Option<ProviderServiceResult> = match retained.command.state.as_str() {
        "succeeded" => retained
            .command
            .result
            .map(serde_json::from_value)
            .transpose()?,
        "failed" => retained
            .command
            .error
            .and_then(|error| error.pointer("/details/service_result").cloned())
            .map(serde_json::from_value)
            .transpose()?,
        _ => None,
    };
    let Some(measured) = measured else {
        return Ok(());
    };
    let attribution = evidence
        .origin
        .billing(&mut tx, evidence.provider, evidence.entry)
        .await?
        .ok_or(EngineError::StaleToolSource)?;
    let quantity = evidence
        .counter
        .as_ref()
        .and_then(|name| measured.counters.get(name))
        .copied()
        .flatten()
        .map_or(orch_api_contract::usage::UsageMeasurement::Empty, |value| {
            orch_api_contract::usage::UsageMeasurement::Told { value }
        });
    engine
        .count_service_use(
            &mut tx,
            &crate::service_usage::ServiceUse {
                id: command,
                at: retained.command.updated_at,
                source: attribution.source(),
                provider: evidence.provider,
                entry: evidence.entry,
                person: attribution.person(),
                purpose: &evidence.capability,
                offering: &evidence.offering,
                groups: &evidence.groups,
                unit: &evidence.unit,
                quantity,
                quote: evidence.quote.as_ref(),
                rate_source: evidence.rate_source(),
                timing: &measured.timing,
                request_ref: None,
            },
        )
        .await?;
    tx.commit().await.map_err(orch_db::DbError::from)?;
    Ok(())
}

#[cfg(test)]
mod paused_item_tests {
    use super::*;
    #[test]
    fn a_paused_service_is_not_rewritten_as_an_empty_completed_search() {
        let mut payload = serde_json::json!({"tool":{"kind":"orch","tool":"web_search","arguments":{"query":"paused"},
            "result":{"content":[{"type":"text","text":"{\"state\":\"paused\",\"service\":\"web_search\",\"reason\":\"wait_for_person\",\"until\":null,\"needs_person\":true}"}]}},
            "raw":{"output":"the exact native envelope"}});
        let raw = payload["raw"].clone();
        normalize_item(&mut payload);
        assert_eq!(payload["tool"]["kind"], "orch");
        assert_eq!(payload["tool"]["result"]["state"], "paused");
        assert!(payload["tool"]["result"].get("results").is_none());
        assert_eq!(payload["raw"], raw);
    }
}
