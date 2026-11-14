//! What a thread may be put on and putting it there: the providers with their models and the profiles that run on them, in every arrangement, with the reason one is not available or not permitted; the environments the caller manages and what a thread would get there; the defaults; and the binding and the environment of a thread that has no history yet.
//!
//! Which provider, which profile and which arrangement a thread may be bound to is the rule of the
//! engine ([`orch_engine::binding`]) applied to the facts of the database; who may use a provider,
//! a profile or an environment is the access engine's. This module puts the two side by side for
//! the dialog that binds a thread: every provider the caller can see, whether they may use it and
//! if not why, and for each profile that could run on it every arrangement with whose decision it
//! is when one is not allowed. A thread that has history is not bound again here: it is switched,
//! and the request says so.

use crate::middleware::authorization;

use crate::middleware::authorization::refusals;

use crate::middleware::requests::ApiError;

use crate::middleware::requests::ApiResult;

use crate::middleware::transaction_guard;

use crate::registry::Ctx;

use crate::registry::Registry;

use crate::routes::access::page::entries_named;

use crate::routes::nodes::invalid;

use crate::routes::nodes::policy_at;

use crate::routes::nodes::view as node_view;

use crate::routes::resources::LIST_LIMIT;

use crate::routes::resources::providers::home_policies;

use crate::routes::resources::view::addresses;

use crate::routes::resources::view::agent_refs;

use crate::routes::resources::view::environment_refs;

use crate::routes::resources::view::machine_refs;

use crate::routes::resources::view::provider_refs;

use crate::routes::settings::resolved_keys;

use orch_access_contract::decisions::Decision;

use orch_access_contract::pages as stored;

use orch_access_contract::permissions::Choice;

use orch_access_contract::permissions::ObjectKind;

use orch_access_contract::permissions::Permission;

use orch_access_contract::permissions::Profile;

use orch_access_contract::permissions::ResourceKind;

use orch_access_contract::roles::PlaceRef;

use orch_agent_contract::arrangement::Incompatible;

use orch_agent_contract::arrangement;

use orch_agent_contract::arrangement::Serves;

use orch_agent_contract::descriptor::AgentDescriptor;

use orch_agent_contract::descriptor::ArrangementType as AgentArrangement;

use orch_agent_contract::descriptor::OrchToolsSupport;

use orch_api_contract::access::AccessSummary;

use orch_api_contract::access::Holder;

use orch_api_contract::binding::Arrangement;

use orch_api_contract::binding::ArrangementType;

use orch_api_contract::binding::BindingAccess;

use orch_api_contract::binding::BindingAgent;

use orch_api_contract::binding::BindingCanCreate;

use orch_api_contract::binding::BindingDefaults;

use orch_api_contract::binding::BindingEnvironment;

use orch_api_contract::binding::BindingMachine;

use orch_api_contract::binding::BindingOptions;

use orch_api_contract::binding::BindingProgramReport;

use orch_api_contract::binding::BindingProvider;

use orch_api_contract::binding::CredentialHolder;

use orch_api_contract::binding::CredentialPath;

use orch_api_contract::binding::EnvironmentChoice;

use orch_api_contract::binding::EnvironmentDefault;

use orch_api_contract::binding::EnvironmentUse;

use orch_api_contract::binding::HarnessPlace;

use orch_api_contract::binding::Isolation;

use orch_api_contract::binding::IsolationOrchTools;

use orch_api_contract::binding::IsolationTools;

use orch_api_contract::binding::Metering;

use orch_api_contract::binding::ModelEnforcement;

use orch_api_contract::binding::NotAllowedBy;

use orch_api_contract::binding::OrchToolsReason;

use orch_api_contract::binding::ThreadBindingOptionsGet;

use orch_api_contract::binding::ThreadBindingSet;

use orch_api_contract::binding::ThreadBindingSetBody;

use orch_api_contract::binding::ThreadEnvironmentSet;

use orch_api_contract::binding::ThreadEnvironmentSetBody;

use orch_api_contract::common::ErrorCode;

use orch_api_contract::nodes::NodeOrSystem;

use orch_api_contract::nodes::NodeRef;

use orch_api_contract::resources::AgentRef;

use orch_api_contract::resources::HarnessStatus;

use orch_api_contract::resources::HostRef;

use orch_api_contract::resources::ProviderRef;

use orch_api_contract::settings::ModelOption;

use orch_api_contract::settings::ModelOptionReason;

use orch_api_contract::settings::ModelParameters;

use orch_api_contract::threads::Thread;

use orch_authz::nodes::Page;

use orch_authz::usable;

use orch_authz::usable::Policies;

use orch_authz::usable::ResourcePolicy;

use orch_authz::Act;

use orch_authz::Policy;

use orch_authz::Principal;

use orch_authz::defaults;

use orch_authz::nodes;

use orch_authz::resources;

use orch_db::Conn;

use orch_db::queries::environments::binding as environment_choice;

use orch_db::queries::environments::routes as environment_queries;

use orch_db::queries::environments::thread_engine as environments;

use orch_db::queries::resources::models_of_provider;

use orch_db::queries::resources::routes::HarnessProcessRow;

use orch_db::queries::resources::routes::harness_processes;

use orch_db::queries::resources::thread_engine as engine_rows;

use orch_db::queries::resources::thread_engine::AgentRow;

use orch_db::queries::resources::thread_engine::ProviderRow;

use orch_db::queries::threads as thread_rows;

use orch_db::queries::threads::binding as history;

use orch_engine::binding;

use orch_engine::binding::ArrangementOption;

use orch_engine::binding::EnvironmentOffers;

use orch_engine::binding::Rebind;

use orch_engine::binding::RebindError;

use orch_engine::binding::Replace;

use orch_engine::harnesses::descriptor_of;

use orch_engine::models::allowed_by;

use orch_engine::provider_setup::Learning;

use orch_engine::resolved;

use orch_engine::resolved::Resolved;

use orch_settings::Place;

use orch_settings::get as setting;

use orch_settings_contract::registry::keys;

use orch_types::Id;

use serde::Serialize;

use serde::de::DeserializeOwned;

use std::collections::HashMap;

use std::time::Instant;
/// The reason a provider or a profile is not offered because the caller's pages give them no use
/// of it.
const NO_USE: &str = "no_use";
/// The reason a provider cannot be chosen where the thread would run: the entry the caller uses it
/// through leaves that environment out.
const NOT_ALLOWED_WHERE: &str = "not_allowed_where";

/// The reason for a resource that is switched off.
const DISABLED: &str = "disabled";

/// The reason for a subscription its home keeps to its first owner.
const SUBSCRIPTION_NOT_SHARED: &str = "subscription_not_shared";

/// The reason for a profile whose harness never reported what it is: nothing is known of what it
/// could be placed in.
const HARNESS_UNREPORTED: &str = "harness_unreported";

/// The reason for an environment whose removal is scheduled: nothing can be bound to it.
const BEING_REMOVED: &str = "being_removed";

/// Phase timings let a slow starter be traced without logging credentials or asking a machine.
struct PreviewTiming {
    node: Id,
    request: &'static str,
    began: Instant,
    last: Instant,
}

impl PreviewTiming {
    fn new(ctx: &Ctx, node: Id) -> Self {
        let began = Instant::now();
        Self {
            node,
            request: ctx.route.name,
            began,
            last: began,
        }
    }

    fn phase(&mut self, phase: &'static str) {
        let now = Instant::now();
        tracing::debug!(request = self.request, node = %self.node, phase,
            phase_ms = now.duration_since(self.last).as_millis(), total_ms = now.duration_since(self.began).as_millis(),
            "binding preview read");
        self.last = now;
    }
}

/// Binds this module's requests.
pub fn mount(registry: &mut Registry) {
    start::mount(registry);
    registry
        .on(ThreadBindingOptionsGet, options)
        .on_with_initial_operation(
            ThreadBindingSet,
            crate::key_operations::native::effects::initial::<rebinding::Bind>,
            rebinding::prepare,
            rebinding::run,
        )
        .on_with_operation(
            ThreadEnvironmentSet,
            crate::key_operations::native::effects::prepare::<placement::PlaceThread>,
            placement::run,
        );
}

/// A word of one contract as the same word of another.
fn same<A: Serialize, B: DeserializeOwned>(word: &A) -> Option<B> {
    serde_json::to_value(word)
        .ok()
        .and_then(|value| serde_json::from_value(value).ok())
}

// ---------------------------------------------------------------- the answer to a request that fails

/// Why a thread could not be put on something else, as the problem a client acts on.
async fn rebind_failed(conn: &mut Conn, thread: Id, error: RebindError) -> ApiError {
    match error {
        RebindError::NoSuchThread => ApiError::not_found(),
        RebindError::Archived => match refusals::named_node(conn, thread).await {
            Ok(node) => ApiError::new(
                ErrorCode::NodeArchived,
                "this is archived, and so is read-only",
            )
            .with("archived_node", node),
            Err(error) => error,
        },
        RebindError::SideThread => ApiError::new(
            ErrorCode::ThreadSide,
            "a side thread is set up as the thread it was taken from is",
        ),
        RebindError::UseSwitch => ApiError::new(
            ErrorCode::ThreadUseSwitch,
            "a thread with history changes its provider, profile or environment by a switch",
        ),
        RebindError::Denied(refusal) => match refusals::denied(conn, *refusal).await {
            Ok(error) | Err(error) => error,
        },
        RebindError::AgentUnavailable => invalid(
            "agent_id",
            "the agent profile is not there or is switched off",
        ),
        RebindError::ProviderUnavailable => invalid(
            "provider_id",
            "the provider is not there or is switched off",
        ),
        RebindError::ProviderNotAllowed(why) => super::provider_policy_failed(conn, why).await,
        RebindError::EnvironmentUnavailable => {
            invalid("use", "the environment is not there or is being removed")
        }
        RebindError::NoEnvironment => ApiError::new(
            ErrorCode::BindingIncompatible,
            "the thread is in no environment yet, and a binding is worked out in one",
        )
        .with("reason", "no_environment"),
        RebindError::AgentNotReported => ApiError::new(
            ErrorCode::BindingIncompatible,
            "the agent profile has not reported what its harness is",
        )
        .with("reason", HARNESS_UNREPORTED),
        RebindError::ModelNotAllowed(model) => ApiError::new(
            ErrorCode::ModelNotAllowed,
            format!("the model {model} is not available here"),
        )
        .with("reason", "model")
        .with("model", model),
        RebindError::EffortNotListed(effort) => ApiError::new(
            ErrorCode::ModelNotAllowed,
            format!("the model does not take the effort {effort}"),
        )
        .with("reason", "effort")
        .with("effort", effort),
        RebindError::InvalidDirectory => invalid(
            "directory",
            "is relative to the workspace root and does not go up",
        ),
        RebindError::Incompatible(why) => {
            let error = ApiError::new(ErrorCode::BindingIncompatible, binding::sentence(&why))
                .with("reason", why.code());
            match why {
                Incompatible::ArrangementNotAllowed { by } => {
                    error.with("not_allowed_by", same::<_, NotAllowedBy>(&by))
                }
                _ => error,
            }
        }
        RebindError::Setting(message) => invalid("", &message),
        RebindError::Internal(error) => ApiError::engine(error),
    }
}

// ---------------------------------------------------------------- binding a thread

// ---------------------------------------------------------------- what may be chosen

/// The policies of the homes the resources lie in, so that a subscription kept to its owner is
/// told wherever it is listed.
async fn policies_for(conn: &mut Conn, homes: &[Id]) -> ApiResult<Policies> {
    let mut policies = Policies::everywhere(ResourcePolicy {
        owners_name_outsiders: false,
        shared_subscriptions: None,
    });
    let mut seen: Vec<Id> = Vec::new();
    for home in homes {
        if seen.contains(home) {
            continue;
        }
        seen.push(*home);
        policies = policies.at_home(
            *home,
            ResourcePolicy {
                owners_name_outsiders: policy_at(conn, *home).await?.owners_name_outsiders,
                shared_subscriptions: home_policies(conn, *home).await?.subscription_sharing,
            },
        );
    }
    Ok(policies)
}

/// What the settings of the topic choose when a thread is bound without being told: a scope that
/// locks them makes every thread below it use them.
async fn defaults_of(
    conn: &mut Conn,
    principal: &Principal,
    topic: Id,
) -> ApiResult<BindingDefaults> {
    let found = resolved_keys(
        conn,
        principal,
        topic,
        &[
            keys::AGENT_DEFAULT_PROVIDER_ID,
            keys::AGENT_DEFAULT_AGENT_ID,
            keys::AGENT_MODEL,
            keys::AGENT_EFFORT,
        ],
    )
    .await?;
    let value = |key: &str| {
        found
            .iter()
            .find(|setting| setting.key == key)
            .and_then(|setting| setting.value.as_ref())
            .and_then(|value| value.as_str())
            .map(str::to_owned)
    };
    let id = |key: &str| value(key).and_then(|text| text.parse::<Id>().ok());
    Ok(BindingDefaults {
        provider_id: id(keys::AGENT_DEFAULT_PROVIDER_ID),
        agent_id: id(keys::AGENT_DEFAULT_AGENT_ID),
        model_name: value(keys::AGENT_MODEL),
        effort: value(keys::AGENT_EFFORT),
        locked: found.iter().any(|setting| setting.locked_by.is_some()),
    })
}

/// What decides whether the agent of a thread is offered Orch's own tools, apart from its harness.
struct OrchToolsOf {
    /// The environment switched them off for the agents that work in it.
    environment_off: bool,
    /// The setting that offers them resolves to none.
    setting_empty: bool,
}

impl OrchToolsOf {
    async fn read(conn: &mut Conn, thread: Id, environment: Option<Id>) -> ApiResult<Self> {
        let environment_off = match environment {
            Some(environment) => {
                setting::<bool>(conn, Place::Environment(environment), keys::EXEC_ORCH_TOOLS)
                    .await
                    .map_err(ApiError::internal)?
                    == Some(false)
            }
            None => false,
        };
        let setting_empty =
            setting::<Vec<String>>(conn, Place::Node(thread), keys::ORCH_TOOLS_ENABLED)
                .await
                .map_err(ApiError::internal)?
                .is_some_and(|enabled| enabled.is_empty());
        Ok(Self {
            environment_off,
            setting_empty,
        })
    }

    /// Whether the agent gets them, and when not why.
    fn of(&self, descriptor: &AgentDescriptor) -> IsolationOrchTools {
        let reason = if descriptor.capabilities.orch_tools != OrchToolsSupport::Mcp {
            Some(OrchToolsReason::Harness)
        } else if self.environment_off {
            Some(OrchToolsReason::Environment)
        } else if self.setting_empty {
            Some(OrchToolsReason::Setting)
        } else {
            None
        };
        IsolationOrchTools {
            offered: reason.is_none(),
            reason,
        }
    }
}

/// An arrangement as the dialog lists it.
fn arrangement_view(
    option: &ArrangementOption,
    provider: &ProviderRow,
    agent: &AgentRow,
    descriptor: &AgentDescriptor,
    orch_tools: &IsolationOrchTools,
    scope: Option<&NodeRef>,
) -> ApiResult<Arrangement> {
    let isolation = binding::isolation(
        option,
        provider,
        provider.agent_id == Some(agent.id),
        descriptor.capabilities.usage_tokens,
    );
    let kind: ArrangementType = same(&option.kind)
        .ok_or_else(|| ApiError::internal("an arrangement type does not read as the API's"))?;
    let not_allowed_by = match &option.why_not {
        Some(Incompatible::ArrangementNotAllowed { by }) => same::<_, NotAllowedBy>(by),
        _ => None,
    };
    let executor_ref = crate::programs::executor_ref(
        orch_agent_programs::Programs::builtin(),
        isolation.executor.as_deref(),
        isolation.tool_set.as_deref(),
    );
    Ok(Arrangement {
        kind,
        executor: isolation.executor.clone(),
        tool_set: isolation.tool_set.clone(),
        executor_ref: executor_ref.clone(),
        isolation: Isolation {
            kind,
            harness_runs_in: if isolation.harness_with_provider {
                HarnessPlace::Provider
            } else {
                HarnessPlace::Environment
            },
            tools: if isolation.executor.is_some() {
                IsolationTools::Executor
            } else {
                IsolationTools::Own
            },
            executor: isolation.executor,
            executor_ref,
            tool_set: isolation.tool_set,
            orch_tools: orch_tools.clone(),
            credential_path: same::<_, CredentialPath>(&isolation.path).ok_or_else(|| {
                ApiError::internal("a credential path does not read as the API's")
            })?,
            held_by: if isolation.handed_over {
                CredentialHolder::Environment
            } else {
                CredentialHolder::Provider
            },
            form: isolation.form,
            lifetime_secs: isolation.lifetime_secs,
            model_enforcement: same::<_, ModelEnforcement>(&isolation.model_enforcement)
                .unwrap_or(ModelEnforcement::Adapter),
            metering: same::<_, Metering>(&isolation.metering).unwrap_or(Metering::Nobody),
        },
        rank: option.rank,
        available: option.available,
        permitted: option.permitted,
        reason: option.why_not.as_ref().map(|why| why.code().to_owned()),
        scope: scope
            .filter(|_| not_allowed_by == Some(NotAllowedBy::Policy))
            .cloned(),
        not_allowed_by,
        detail: option
            .detail
            .clone()
            .or_else(|| option.why_not.as_ref().map(binding::sentence)),
    })
}

/// What the models of a provider are for the caller: each with whether they may choose it and why
/// not.
fn model_options(
    rows: Vec<orch_db::queries::resources::ProviderModelRow>,
    settings: &Resolved,
    entries: Option<&[&orch_access_contract::permissions::ProviderUse]>,
) -> ApiResult<Vec<ModelOption>> {
    rows.into_iter()
        .map(|row| {
            let reason = if !row.enabled {
                Some(ModelOptionReason::Disabled)
            } else if settings
                .models_allowed
                .as_ref()
                .is_some_and(|allowed| !allowed_by(allowed, &row.model_name, &row.groups))
            {
                Some(ModelOptionReason::ExcludedByPolicy)
            } else if entries.is_some_and(|entries| {
                !entries.iter().any(|entry| {
                    orch_authz::options::allows_model(entry, &row.model_name, &row.groups)
                })
            }) {
                Some(ModelOptionReason::ExcludedByEntry)
            } else {
                None
            };
            let parameters: ModelParameters =
                orch_provider_contract::models::composed_model_parameters(
                    &row.parameters,
                    &row.groups,
                    &row.option_layers,
                    None,
                )
                .map_err(ApiError::internal)?;
            Ok(ModelOption {
                token_estimate: orch_engine::token_density::of(&row.model_name),
                groups: row.groups.clone(),
                display_name: if row.display_name.is_empty() {
                    row.model_name.clone()
                } else {
                    row.display_name.clone()
                },
                option_constraints: orch_model_contract::parameters::constraint_catalogue(
                    &parameters,
                ),
                parameters,
                model_name: row.model_name,
                allowed: reason.is_none(),
                reason,
            })
        })
        .collect()
}

/// The same native model/Use/policy choices for a no-environment Ask. This
/// deliberately has no topic/project creation gate and borrows no such authority.
pub(crate) async fn ask_models(
    conn: &mut Conn,
    principal: &Principal,
    root: Id,
) -> ApiResult<Vec<orch_api_contract::ask::AskModelOption>> {
    ask_models::read(conn, principal, root).await
}
orch_api_base::__api_bind_9aa49dd569!();

/// Whether the profile can reach what the provider serves; an unreported keeper still remains visible.
fn runs_on(descriptor: &AgentDescriptor, provider: &ProviderRow, agent: &AgentRow) -> bool {
    let served = binding::serves(provider);
    (provider.agent_id == Some(agent.id)
        && served
            .iter()
            .any(|access| matches!(access, Serves::Native { .. })))
        || arrangement::pairs(descriptor, &served)
}

/// Everything the dialog needs to know about the place a thread is in, read once.
struct Ground<'a> {
    thread: Id,
    settings: &'a Resolved,
    environment: Option<&'a environments::EnvironmentRow>,
    offers: Option<&'a EnvironmentOffers>,
    orch_tools: &'a OrchToolsOf,
    scope: Option<&'a NodeRef>,
}

/// The providers the caller can see, each with whether they may use it, its models, and the
/// profiles that could run on it with every arrangement.
struct ProviderCandidates {
    seen: resources::Listing<resources::Seen>,
    agents_seen: resources::Listing<resources::Seen>,
    may_use: resources::Listing<usable::Usables>,
    may_use_agent: resources::Listing<usable::Usables>,
    policies: Policies,
}

struct ChoiceProjection {
    names: crate::routes::access::page::NameBounds,
    hosts: std::collections::BTreeSet<Id>,
    machines: std::collections::BTreeSet<Id>,
}

async fn provider_candidates(
    conn: &mut Conn,
    principal: &Principal,
    thread: Id,
    bounds: Option<&dyn choice_window::Bounds>,
) -> ApiResult<ProviderCandidates> {
    let seen = choice_window::select(
        conn,
        &choice_window::Visible {
            principal,
            kind: ResourceKind::Provider,
        },
        bounds,
    )
    .await?;
    let agents_seen = choice_window::select(
        conn,
        &choice_window::Visible {
            principal,
            kind: ResourceKind::Agent,
        },
        bounds,
    )
    .await?;
    let homes: Vec<Id> = seen
        .items
        .iter()
        .chain(agents_seen.items.iter())
        .filter_map(|each| each.resource.home)
        .collect();
    let policies = policies_for(conn, &homes).await?;
    let may_use = choice_window::select(
        conn,
        &choice_window::Usable {
            principal,
            kind: ResourceKind::Provider,
            thread,
            policies: &policies,
            manage_environments: false,
        },
        bounds,
    )
    .await?;
    let may_use_agent = choice_window::select(
        conn,
        &choice_window::Usable {
            principal,
            kind: ResourceKind::Agent,
            thread,
            policies: &policies,
            manage_environments: false,
        },
        bounds,
    )
    .await?;

    Ok(ProviderCandidates {
        seen,
        agents_seen,
        may_use,
        may_use_agent,
        policies,
    })
}

struct ProviderInputs {
    agents: Vec<(AgentRow, Option<AgentDescriptor>)>,
    providers: HashMap<Id, ProviderRow>,
    programs: Vec<HarnessProcessRow>,
}

fn program_report(
    inputs: &ProviderInputs,
    provider: &ProviderRow,
    agent: Id,
) -> Option<BindingProgramReport> {
    let rank = |status: &str| match status {
        "online" => 0,
        "starting" => 1,
        "error" => 2,
        "offline" => 3,
        _ => 4,
    };
    let process = inputs
        .programs
        .iter()
        .filter(|process| {
            process.provider_id == provider.id
                && process.agent_id == agent
                && provider
                    .machine_id
                    .is_none_or(|machine| process.machine_id == machine)
        })
        .min_by(|one, other| {
            rank(&one.status)
                .cmp(&rank(&other.status))
                .then_with(|| other.last_active_at.cmp(&one.last_active_at))
                .then_with(|| one.machine_id.cmp(&other.machine_id))
        })?;
    let status = match process.status.as_str() {
        "online" => HarnessStatus::Online,
        "starting" => HarnessStatus::Starting,
        "error" => HarnessStatus::Error,
        "offline" => HarnessStatus::Offline,
        _ => return None,
    };
    Some(BindingProgramReport {
        status,
        detail: process.status_detail.clone(),
    })
}

fn agent_offered(
    agent: &AgentRow,
    descriptor: Option<&AgentDescriptor>,
    provider: &ProviderRow,
) -> bool {
    match descriptor {
        Some(descriptor) => runs_on(descriptor, provider, agent),
        None => {
            let served = binding::serves(provider);
            let native_only = served
                .iter()
                .any(|access| matches!(access, Serves::Native { .. }))
                && !served
                    .iter()
                    .any(|access| matches!(access, Serves::Forwarded { .. }));
            !native_only || provider.agent_id == Some(agent.id)
        }
    }
}

async fn provider_inputs(
    conn: &mut Conn,
    candidates: &ProviderCandidates,
) -> ApiResult<ProviderInputs> {
    let agent_ids = candidates
        .agents_seen
        .items
        .iter()
        .map(|row| row.resource.id)
        .collect::<Vec<_>>();
    let provider_ids = candidates
        .seen
        .items
        .iter()
        .map(|row| row.resource.id)
        .collect::<Vec<_>>();
    let mut agent_rows: Vec<(AgentRow, Option<AgentDescriptor>)> = Vec::new();
    for id in &agent_ids {
        if let Some(row) = engine_rows::agent(conn, *id)
            .await?
            .filter(|row| row.deleted_at.is_none())
        {
            let descriptor = descriptor_of(&row).map_err(ApiError::internal)?;
            agent_rows.push((row, descriptor));
        }
    }
    // Reuse the same provider rows throughout this projection and batch only the new program builds.
    let mut provider_rows = HashMap::new();
    for id in &provider_ids {
        if let Some(row) = engine_rows::provider(conn, *id)
            .await?
            .filter(|row| row.deleted_at.is_none())
        {
            provider_rows.insert(*id, row);
        }
    }
    // The ordinary provider process reader requires Manage. Apply the same gate before
    // reading any process details, within the already admitted native choice window.
    let inspectable: Vec<Id> = candidates
        .seen
        .items
        .iter()
        .filter(|each| {
            each.held
                .iter()
                .any(|held| held.permission == Permission::ProviderManage)
        })
        .map(|each| each.resource.id)
        .collect();
    let programs = if inspectable.is_empty() {
        Vec::new()
    } else {
        harness_processes(conn, &inspectable).await?
    };
    Ok(ProviderInputs {
        agents: agent_rows,
        providers: provider_rows,
        programs,
    })
}

async fn providers_of(
    conn: &mut Conn,
    principal: &Principal,
    app: &crate::state::AppState,
    ground: &Ground<'_>,
) -> ApiResult<Vec<BindingProvider>> {
    let candidates = provider_candidates(conn, principal, ground.thread, None).await?;
    providers_from(conn, principal, app, ground, &candidates, None, None).await
}

async fn providers_from(
    conn: &mut Conn,
    principal: &Principal,
    app: &crate::state::AppState,
    ground: &Ground<'_>,
    candidates: &ProviderCandidates,
    selected_inputs: Option<&ProviderInputs>,
    projection: Option<&ChoiceProjection>,
) -> ApiResult<Vec<BindingProvider>> {
    let ProviderCandidates {
        seen,
        agents_seen,
        may_use,
        may_use_agent,
        policies,
    } = candidates;
    let owner_ids = seen
        .items
        .iter()
        .chain(agents_seen.items.iter())
        .filter_map(|row| row.resource.first_owner)
        .collect::<Vec<_>>();
    let owners = refusals::user_refs(conn, &owner_ids).await?;
    let agent_owners = agents_seen
        .items
        .iter()
        .map(|row| (row.resource.id, row.resource.first_owner))
        .collect::<HashMap<_, _>>();
    let provider_ids: Vec<Id> = seen.items.iter().map(|each| each.resource.id).collect();
    let agent_ids: Vec<Id> = agents_seen
        .items
        .iter()
        .map(|each| each.resource.id)
        .collect();
    let provider_names: HashMap<Id, ProviderRef> = provider_refs(conn, &provider_ids, &[])
        .await?
        .into_iter()
        .map(|(_, each)| (each.id, each))
        .collect();
    let subjects = provider_ids
        .iter()
        .map(|&id| ("provider".to_owned(), id))
        .chain(agent_ids.iter().map(|&id| ("agent".to_owned(), id)))
        .collect::<Vec<_>>();
    let grouped = grouping::metadata(app, conn, principal, &subjects, projection).await?;
    let agent_names: HashMap<Id, AgentRef> = agent_refs(app, conn, &agent_ids, &[])
        .await?
        .into_iter()
        .map(|(_, each)| (each.id, each))
        .collect();
    let owned_inputs;
    let inputs = match selected_inputs {
        Some(inputs) => inputs,
        None => {
            owned_inputs = provider_inputs(conn, candidates).await?;
            &owned_inputs
        }
    };
    let agent_rows = &inputs.agents;
    let provider_rows = &inputs.providers;
    let program_facts = if let Some(environment) = ground.environment {
        let providers: Vec<_> = provider_rows.values().collect();
        let agents: Vec<_> = agent_rows
            .iter()
            .filter_map(|(agent, descriptor)| {
                descriptor.as_ref().map(|descriptor| (agent, descriptor))
            })
            .collect();
        binding::prepare_machine_programs(conn, environment, &providers, &agents)
            .await
            .map_err(ApiError::internal)?
    } else {
        None
    };

    let policy_def = orch_settings_contract::registry::get(keys::PROVIDERS_ALLOWED)
        .ok_or_else(|| ApiError::internal("the provider policy is not registered"))?;
    let policy_snapshot = orch_settings::Snapshot::load(
        conn,
        Place::Node(ground.thread),
        orch_settings::Wanted::Keys(&[policy_def]),
    )
    .await
    .map_err(ApiError::internal)?;
    let provider_policy = policy_snapshot.resolve(policy_def);
    let policy_places: Vec<Id> = provider_policy
        .parts
        .iter()
        .filter_map(|part| part.source.node)
        .collect();
    let policy_names = match projection {
        Some(projection) => {
            projection
                .names
                .refs(conn, principal, &policy_places)
                .await?
        }
        None => node_view::refs(conn, principal, &policy_places, Policy::default()).await?,
    };
    // Where the thread would run: an entry that leaves the place out does not let a provider be chosen.
    let place = match ground.environment {
        Some(environment) => Some(
            orch_authz::usable::place_of(conn, environment.id)
                .await
                .map_err(ApiError::internal)?,
        ),
        None => None,
    };
    let mut listed = Vec::new();
    for each in &seen.items {
        let id = each.resource.id;
        let Some(row) = provider_rows.get(&id).cloned() else {
            continue;
        };
        let Some(name) = provider_names.get(&id).cloned() else {
            continue;
        };
        let granted = may_use
            .items
            .iter()
            .find(|each| each.resource.id == id)
            .map(|each| &each.granted);
        // What the entries that allow the place give together; the models are those of any of them.
        let here = orch_authz::options::Wanted {
            at: place.as_ref(),
            ..orch_authz::options::Wanted::default()
        };
        let allowing: Vec<&orch_access_contract::permissions::ProviderUse> = granted
            .map(|granted| {
                granted
                    .allowing(&here)
                    .into_iter()
                    .map(|allowance| &allowance.provider)
                    .collect()
            })
            .unwrap_or_default();
        let reason = if let Some(granted) = granted {
            allowing.is_empty().then(|| {
                if granted.allows(&orch_authz::options::Wanted::default()) {
                    NOT_ALLOWED_WHERE
                } else {
                    NO_USE
                }
            })
        } else if each.resource.disabled {
            Some(DISABLED)
        } else if each.resource.subscription.is_some()
            && policies
                .at(each.resource.home)
                .keeps_to_its_owner(&each.resource)
            && !each.owner
        {
            Some(SUBSCRIPTION_NOT_SHARED)
        } else {
            Some(NO_USE)
        };
        let entry_types: Option<Vec<AgentArrangement>> = granted.map(|_| {
            orch_authz::options::arrangements_of(&allowing)
                .iter()
                .filter_map(same::<_, AgentArrangement>)
                .collect()
        });
        let machine = binding::machine_arrangements_of(conn, &row)
            .await
            .map_err(ApiError::internal)?;
        // Where the place leaves out every entry, the provider is not offered there at all; its
        // models are then told by all the entries, as they are for any place.
        let for_models: Vec<&orch_access_contract::permissions::ProviderUse> =
            if allowing.is_empty() {
                granted
                    .map(|granted| {
                        granted
                            .each
                            .iter()
                            .map(|allowance| &allowance.provider)
                            .collect()
                    })
                    .unwrap_or_default()
            } else {
                allowing.clone()
            };
        let models = model_options(
            models_of_provider(conn, id).await?,
            ground.settings,
            granted.map(|_| for_models.as_slice()),
        )?;
        let mut agents = Vec::new();
        for (agent, descriptor) in agent_rows {
            if !agent_offered(agent, descriptor.as_ref(), &row) {
                continue;
            }
            let Some(agent_name) = agent_names.get(&agent.id).cloned() else {
                continue;
            };
            let agent_may = may_use_agent
                .items
                .iter()
                .any(|usable| usable.resource.id == agent.id);
            let agent_reason = if agent.disabled {
                Some(DISABLED)
            } else if !agent_may {
                Some(NO_USE)
            } else {
                None
            };
            let report = program_report(inputs, &row, agent.id);
            let Some(descriptor) = descriptor else {
                // A stored failure is an answer, not a discovery that is still pending.
                let failed = report
                    .as_ref()
                    .is_some_and(|report| report.status == HarnessStatus::Error);
                agents.push(BindingAgent {
                    owner: agent_owners
                        .get(&agent.id)
                        .copied()
                        .flatten()
                        .and_then(|id| owners.get(&id).cloned()),
                    grouping: grouped
                        .get(&("agent".to_owned(), agent.id))
                        .cloned()
                        .unwrap_or_default(),
                    agent: agent_name,
                    usable: false,
                    reason: Some(
                        agent_reason
                            .unwrap_or(if failed {
                                "harness_error"
                            } else {
                                HARNESS_UNREPORTED
                            })
                            .to_owned(),
                    ),
                    program_report: report,
                    arrangements: Vec::new(),
                });
                continue;
            };
            let arrangements = match (ground.environment, ground.offers) {
                (Some(environment), Some(offers)) => {
                    let facts = binding::Facts {
                        descriptor,
                        provider: &row,
                        environment: Some(environment),
                        offers: offers.offers.clone(),
                        settings: ground.settings,
                        entry: entry_types.clone(),
                        colocated: offers.colocated.clone(),
                        runtime: binding::runtime_facts_prepared(
                            conn,
                            agent,
                            &row,
                            environment,
                            program_facts.as_ref(),
                        )
                        .await
                        .map_err(ApiError::internal)?,
                    };
                    let orch_tools = ground.orch_tools.of(descriptor);
                    let place = binding::initial_place(conn, &facts)
                        .await
                        .map_err(ApiError::internal)?;
                    let options = binding::options_placed(&facts, &place, machine.clone());
                    options
                        .iter()
                        .map(|option| {
                            arrangement_view(
                                option,
                                &row,
                                agent,
                                descriptor,
                                &orch_tools,
                                ground.scope,
                            )
                        })
                        .collect::<ApiResult<Vec<_>>>()?
                }
                _ => Vec::new(),
            };
            let compatible = arrangements
                .iter()
                .any(|choice| choice.available && choice.permitted);
            let reason = agent_reason.map(str::to_owned).or_else(|| {
                (!compatible).then(|| {
                    arrangements
                        .first()
                        .and_then(|choice| choice.reason.clone())
                        .unwrap_or_else(|| "binding_incompatible".to_owned())
                })
            });
            let mut metadata = grouped
                .get(&("agent".to_owned(), agent.id))
                .cloned()
                .unwrap_or_default();
            metadata.availability = grouping::arrangements(ground, projection, &arrangements);
            agents.push(BindingAgent {
                owner: agent_owners
                    .get(&agent.id)
                    .copied()
                    .flatten()
                    .and_then(|id| owners.get(&id).cloned()),
                grouping: metadata,
                agent: agent_name,
                usable: reason.is_none(),
                reason,
                program_report: report,
                arrangements,
            });
        }
        let policy_source =
            orch_engine::provider_policy::refusing_scope(&provider_policy.parts, &name.address);
        let policy_scope = policy_source.as_ref().and_then(|source| match source.node {
            Some(id) => policy_names.get(&id).cloned().map(NodeOrSystem::Node),
            None => Some(NodeOrSystem::SYSTEM),
        });
        let mut metadata = grouped
            .get(&("provider".to_owned(), id))
            .cloned()
            .unwrap_or_default();
        let arrangements = agents
            .iter()
            .flat_map(|agent| agent.arrangements.iter())
            .cloned()
            .collect::<Vec<_>>();
        metadata.availability = grouping::arrangements(ground, projection, &arrangements);
        listed.push(BindingProvider {
            owner: each
                .resource
                .first_owner
                .and_then(|id| owners.get(&id).cloned()),
            grouping: metadata,
            provider: name,
            usable: reason.is_none() && policy_source.is_none(),
            reason: if policy_source.is_some() {
                Some("provider_not_allowed".to_owned())
            } else {
                reason.map(str::to_owned)
            },
            policy_scope,
            models,
            agents,
        });
    }
    Ok(listed)
}

/// The reason an environment the caller sees, and may not start a thread in, is listed with: asked for
/// through an access request, as a missing Use is.
const ENVIRONMENT_USE_REQUIRED: &str = "environment_use_required";

/// Moving an existing thread also needs management of its destination.
const ENVIRONMENT_MANAGE_REQUIRED: &str = "environment_manage_required";

/// The environments the caller may start a thread in, this topic's first, each with what a thread would
/// get there; then those they only see, with the reason that asks for their Use.
struct EnvironmentCandidates {
    ids: Vec<Id>,
    placeable: std::collections::HashSet<Id>,
}

async fn environment_candidates(
    conn: &mut Conn,
    principal: &Principal,
    thread: Id,
    required: Permission,
    bounds: Option<&dyn choice_window::Bounds>,
) -> ApiResult<EnvironmentCandidates> {
    let policies = Policies::everywhere(ResourcePolicy {
        owners_name_outsiders: false,
        shared_subscriptions: None,
    });
    let managed = choice_window::select(
        conn,
        &choice_window::Usable {
            principal,
            kind: ResourceKind::Environment,
            thread,
            policies: &policies,
            manage_environments: required == Permission::EnvironmentManage,
        },
        bounds,
    )
    .await?;
    let mut ids: Vec<Id> = managed.items.iter().map(|each| each.resource.id).collect();
    let placeable: std::collections::HashSet<Id> = ids.iter().copied().collect();
    let seen = choice_window::select(
        conn,
        &choice_window::Visible {
            principal,
            kind: ResourceKind::Environment,
        },
        bounds,
    )
    .await?;
    let already: std::collections::HashSet<Id> = ids.iter().copied().collect();
    ids.extend(
        seen.items
            .iter()
            .map(|each| each.resource.id)
            .filter(|id| !already.contains(id)),
    );
    Ok(EnvironmentCandidates { ids, placeable })
}

async fn environments_of(
    conn: &mut Conn,
    principal: &Principal,
    app: &crate::state::AppState,
    topic: Id,
    thread: Id,
    required: Permission,
) -> ApiResult<Vec<BindingEnvironment>> {
    let candidates = environment_candidates(conn, principal, thread, required, None).await?;
    environments_from(conn, principal, app, topic, required, &candidates, None).await
}

fn environment_reason(
    row: &environment_queries::EnvironmentFull,
    placeable: bool,
    required: Permission,
) -> Option<&'static str> {
    if row.remove_at.is_some() {
        Some(BEING_REMOVED)
    } else if row.status == "lost" {
        Some("environment_lost")
    } else if row.status == "disabled" {
        Some(DISABLED)
    } else if row.status == "failed" || row.status == "removed" {
        Some("environment_failed")
    } else if !placeable {
        Some(if required == Permission::EnvironmentUse {
            ENVIRONMENT_USE_REQUIRED
        } else {
            ENVIRONMENT_MANAGE_REQUIRED
        })
    } else {
        None
    }
}

async fn environments_from(
    conn: &mut Conn,
    principal: &Principal,
    app: &crate::state::AppState,
    topic: Id,
    required: Permission,
    candidates: &EnvironmentCandidates,
    projection: Option<&ChoiceProjection>,
) -> ApiResult<Vec<BindingEnvironment>> {
    let placeable = &candidates.placeable;
    let rows = environment_queries::environments(conn, &candidates.ids).await?;
    let ids: Vec<Id> = rows.iter().map(|row| row.id).collect();
    let subjects = ids
        .iter()
        .map(|&id| ("environment".to_owned(), id))
        .collect::<Vec<_>>();
    let grouped = grouping::metadata(app, conn, principal, &subjects, projection).await?;
    let uses = history::environment_uses(conn, &ids, topic).await?;
    let hosts = environment_choice::hosts_of(conn, &ids).await?;
    let default = environment_queries::default_for(conn, &ids)
        .await?
        .into_iter()
        .find(|(_, of)| *of == topic)
        .map(|(environment, _)| environment);
    let topic_name = thread_rows::node_name(conn, topic)
        .await?
        .unwrap_or_default();
    let refs: HashMap<Id, _> = environment_refs(conn, &ids, &[], principal.now())
        .await?
        .into_iter()
        .map(|(_, each)| (each.id, each))
        .collect();
    let host_ids: Vec<Id> = hosts.iter().map(|(_, host, _)| *host).collect();
    let host_addresses = match projection {
        Some(projection) => {
            crate::routes::resources::view::addresses_with_names(
                conn,
                &host_ids,
                Some(&projection.names),
            )
            .await?
        }
        None => addresses(conn, &host_ids).await?,
    };
    let machine_ids: Vec<Id> = rows
        .iter()
        .filter(|row| row.placement == "connected")
        .filter_map(|row| row.machine_id)
        .collect();
    let machines = machine_refs(app, conn, &machine_ids).await?;
    let mut topic_ids: Vec<Id> = uses.iter().flat_map(|each| each.topics.clone()).collect();
    topic_ids.sort_unstable();
    topic_ids.dedup();
    let topic_refs = match projection {
        Some(projection) => projection.names.refs(conn, principal, &topic_ids).await?,
        None => node_view::refs(conn, principal, &topic_ids, Policy::default()).await?,
    };

    let mut listed: Vec<(u8, BindingEnvironment)> = Vec::new();
    for row in &rows {
        let Some(environment) = refs.get(&row.id).cloned() else {
            continue;
        };
        let used_here = uses.iter().find(|each| each.environment == row.id);
        let reason = environment_reason(row, placeable.contains(&row.id), required);
        let (viewable, hidden): (Vec<NodeRef>, i64) = used_here.map_or((Vec::new(), 0), |used| {
            let mut shown = Vec::new();
            let mut hidden = 0_i64;
            for each in &used.topics {
                match topic_refs.get(each) {
                    Some(node) if !node.redacted => shown.push(node.clone()),
                    _ => hidden += 1,
                }
            }
            (shown, hidden)
        });
        let is_default = default == Some(row.id) && reason.is_none();
        let group: u8 = if is_default {
            0
        } else if used_here.is_some_and(|used| used.in_topic) || row.home_node_id == topic {
            1
        } else {
            2
        };
        listed.push((
            group,
            BindingEnvironment {
                owner: Some(environment.owner.clone()),
                grouping: {
                    let mut metadata = grouped
                        .get(&("environment".to_owned(), row.id))
                        .cloned()
                        .unwrap_or_default();
                    if let Some(machine_id) = row
                        .machine_id
                        .filter(|id| projection.is_none_or(|p| p.machines.contains(id)))
                    {
                        let reason = environment_reason(row, true, required).map(str::to_owned);
                        metadata.availability.push(
                            orch_api_contract::resource_groups::ResourceMachineAvailability {
                                machine_id,
                                available: reason.is_none(),
                                reason,
                            },
                        );
                    }
                    metadata
                },
                environment,
                host: hosts
                    .iter()
                    .find(|(environment, _, _)| *environment == row.id)
                    .filter(|(_, host, _)| {
                        projection.is_none_or(|projection| projection.hosts.contains(host))
                    })
                    .map(|(_, host, name)| HostRef {
                        id: *host,
                        name: name.clone(),
                        address: host_addresses.get(host).cloned().unwrap_or_default(),
                    }),
                machine: row
                    .machine_id
                    .filter(|_| row.placement == "connected")
                    .filter(|machine| {
                        projection.is_none_or(|projection| projection.machines.contains(machine))
                    })
                    .and_then(|machine| machines.get(&machine).cloned()),
                threads: used_here.map_or(0, |used| used.threads),
                usable: reason.is_none(),
                reason: reason.map(str::to_owned),
                is_default,
                used: match used_here {
                    Some(used) if used.in_topic => EnvironmentUse::Topic,
                    Some(used) if used.in_project => EnvironmentUse::Project,
                    Some(_) => EnvironmentUse::Elsewhere,
                    None => EnvironmentUse::Unused,
                },
                topics: viewable,
                topics_hidden: hidden,
                directory: binding::default_directory(&topic_name, used_here),
            },
        ));
    }
    // Stable: within a group the environments stay in the order they were made.
    listed.sort_by_key(|(group, _)| *group);
    Ok(listed.into_iter().map(|(_, each)| each).collect())
}

/// Whether a row that names `entry` is held by `who`: the same holder, or for a role the same role
/// or a higher one in the same place (a row that names a role names everyone above it too).
fn covers(entry: &Holder, who: &Holder) -> bool {
    match (entry, who) {
        (
            Holder::Role {
                role: named,
                at: here,
                ..
            },
            Holder::Role { role, at, .. },
        ) => here == at && named <= role,
        (named, who) => named == who,
    }
}

/// The row that makes a choice what it is on a kind of thing: the level of its area.
fn level_of(kind: ObjectKind, choice: Choice) -> Option<Permission> {
    match (kind, choice) {
        (ObjectKind::Thread, Choice::FullView) => Some(Permission::ThreadView),
        (ObjectKind::Thread, Choice::FullInteract) => Some(Permission::ThreadInteract),
        (ObjectKind::Thread, Choice::FullManage) => Some(Permission::ThreadManage),
        (ObjectKind::Environment, Choice::FullView) => Some(Permission::EnvironmentView),
        (ObjectKind::Environment, Choice::FullInteract) => Some(Permission::EnvironmentUse),
        (ObjectKind::Environment, Choice::FullManage) => Some(Permission::EnvironmentManage),
        _ => None,
    }
}

/// A permission page in three lines, as the defaults of a topic give it to something made in it:
/// who holds each of the three everyday choices in every area of it, whether somebody holds only
/// some areas, and the nearest place whose defaults decide it. Everybody is shown under the highest
/// choice they hold in every one of its rows that the defaults give to anybody.
// The part that forks: the fork's plan shows the page its result would have, which this tells.
pub(crate) async fn summary_of(
    conn: &mut Conn,
    principal: &Principal,
    topic: Id,
    kind: ObjectKind,
    profile: Profile,
) -> ApiResult<AccessSummary> {
    summaries_of(conn, principal, topic, &[(kind, profile)])
        .await?
        .pop()
        .ok_or_else(|| ApiError::internal("the requested access summary is missing"))
}

/// The three pages a starter previews, from one reading of the defaults and holder names.
async fn access_of(
    conn: &mut Conn,
    principal: &Principal,
    topic: Id,
) -> ApiResult<(BindingAccess, AccessSummary)> {
    let [managed, connected, new_environment]: [AccessSummary; 3] = summaries_of(
        conn,
        principal,
        topic,
        &[
            (ObjectKind::Thread, Profile::Managed),
            (ObjectKind::Thread, Profile::Connected),
            (ObjectKind::Environment, Profile::Managed),
        ],
    )
    .await?
    .try_into()
    .map_err(|_| ApiError::internal("the requested access summaries are missing"))?;
    Ok((BindingAccess { managed, connected }, new_environment))
}

/// Batch the database work, keeping each page's profile and each row's inheritance separate.
async fn summaries_of(
    conn: &mut Conn,
    principal: &Principal,
    topic: Id,
    pages: &[(ObjectKind, Profile)],
) -> ApiResult<Vec<AccessSummary>> {
    let choices = [Choice::FullManage, Choice::FullInteract, Choice::FullView];
    let requested: Vec<Vec<(Permission, Profile)>> = pages
        .iter()
        .map(|(kind, profile)| {
            let mut rows = Vec::new();
            for choice in choices {
                for permission in choice.permissions(*kind) {
                    let row = (*permission, *profile);
                    if !rows.contains(&row) {
                        rows.push(row);
                    }
                }
            }
            rows
        })
        .collect();
    let wanted: Vec<_> = requested.iter().flatten().copied().collect();
    let inherited = defaults::resolve_rows(conn, principal, PlaceRef::Node(topic), &wanted)
        .await
        .map_err(ApiError::internal)?;
    let entries: Vec<stored::Entry> = inherited
        .iter()
        .flatten()
        .map(|each| each.entry.clone())
        .collect();
    let named = entries_named(conn, principal, None, &entries).await?;
    let mut nodes: Vec<Id> = inherited
        .iter()
        .flatten()
        .filter_map(|each| match each.from {
            stored::Origin::Defaults(PlaceRef::Node(node)) => Some(node),
            _ => None,
        })
        .collect();
    nodes.sort_unstable();
    nodes.dedup();
    let refs = if nodes.is_empty() {
        HashMap::new()
    } else {
        node_view::refs(conn, principal, &nodes, Policy::default()).await?
    };
    let mut inherited = inherited.into_iter();
    let mut summaries = Vec::new();
    for (rows, (kind, _)) in requested.into_iter().zip(pages) {
        let mut nearest: Option<PlaceRef> = None;
        let mut holders = Vec::new();
        for (permission, _) in rows {
            let entries = inherited
                .next()
                .ok_or_else(|| ApiError::internal("a requested defaults row is missing"))?;
            let mut row = Vec::new();
            for each in entries {
                if let Some(entry) = named.get(&each.entry.id)
                    && !row.contains(&entry.holder)
                {
                    row.push(entry.holder.clone());
                }
                if let stored::Origin::Defaults(place) = each.from {
                    nearest = nearest.or(Some(place));
                }
            }
            holders.push((permission, row));
        }
        let from = match nearest {
            Some(PlaceRef::Node(node)) => refs.get(&node).cloned().map(NodeOrSystem::Node),
            Some(PlaceRef::System) => Some(NodeOrSystem::SYSTEM),
            None => None,
        };
        summaries.push(summary_rows(*kind, &holders, from));
    }
    Ok(summaries)
}

/// The highest complete choice held in the already-read rows, with any partial grants indicated.
fn summary_rows(
    kind: ObjectKind,
    rows: &[(Permission, Vec<Holder>)],
    from: Option<NodeOrSystem>,
) -> AccessSummary {
    let choices = [Choice::FullManage, Choice::FullInteract, Choice::FullView];
    let mut candidates: Vec<Holder> = Vec::new();
    for (_, holders) in rows {
        for holder in holders {
            if !candidates.contains(holder) {
                candidates.push(holder.clone());
            }
        }
    }
    let holds = |who: &Holder, permission: &Permission| {
        rows.iter()
            .find(|(known, _)| known == permission)
            .is_some_and(|(_, named)| named.iter().any(|entry| covers(entry, who)))
    };
    let (mut manage, mut interact, mut view) = (Vec::new(), Vec::new(), Vec::new());
    let mut custom = false;
    for who in candidates {
        // The level of a choice is the row that makes it one; a row beyond it that nobody holds by
        // default says nothing about it.
        let highest = choices.into_iter().find(|choice| {
            let Some(level) = level_of(kind, *choice) else {
                return false;
            };
            holds(&who, &level)
                && choice.permissions(kind).iter().all(|permission| {
                    let given = rows
                        .iter()
                        .any(|(known, named)| known == permission && !named.is_empty());
                    !given || holds(&who, permission)
                })
        });
        match highest {
            Some(Choice::FullManage) => manage.push(who),
            Some(Choice::FullInteract) => interact.push(who),
            Some(Choice::FullView) => view.push(who),
            None => custom = true,
        }
    }
    AccessSummary {
        view,
        interact,
        manage,
        custom,
        from,
    }
}

/// Whether the caller may make a new environment for the topic: work may be started there, and a
/// host and a template they may use are there, or a machine of their own.
async fn can_create_in(
    conn: &mut Conn,
    principal: &Principal,
    topic: Id,
    thread: Id,
) -> ApiResult<BindingCanCreate> {
    let permission =
        if thread_rows::tools::node_kind(conn, topic).await?.as_deref() == Some("project") {
            Permission::ProjectCreateTopics
        } else {
            Permission::TopicStartThreads
        };
    let interact = nodes::check(
        conn,
        principal,
        topic,
        permission,
        Act::Use,
        Policy::default(),
    )
    .await;
    match interact {
        Ok(Decision::Allowed { .. }) => {}
        Ok(Decision::Refused(refusal)) => {
            return Ok(BindingCanCreate {
                allowed: false,
                missing: refusals::missing_access(conn, &refusal.missing).await?,
            });
        }
        Err(_) => {
            return Ok(BindingCanCreate {
                allowed: false,
                missing: Vec::new(),
            });
        }
    }
    if orch_db::queries::binding_start::environment_machine_owned(conn, principal.user()).await? {
        return Ok(BindingCanCreate {
            allowed: true,
            missing: Vec::new(),
        });
    }
    let page = Page {
        after: None,
        limit: LIST_LIMIT,
    };
    let policies = Policies::everywhere(ResourcePolicy {
        owners_name_outsiders: false,
        shared_subscriptions: None,
    });
    let hosts = usable::usable_list(
        conn,
        principal,
        ResourceKind::Host,
        Some(thread),
        &policies,
        page,
    )
    .await
    .map_err(ApiError::internal)?;
    let templates = usable::usable_list(
        conn,
        principal,
        ResourceKind::Template,
        Some(thread),
        &policies,
        page,
    )
    .await
    .map_err(ApiError::internal)?;
    let template_ids: Vec<Id> = templates
        .items
        .iter()
        .map(|each| each.resource.id)
        .collect();
    let any_template = !environment_choice::environment_templates(conn, &template_ids)
        .await?
        .is_empty();
    Ok(BindingCanCreate {
        allowed: !hosts.items.is_empty() && any_template,
        missing: Vec::new(),
    })
}

/// What the caller could choose when binding a thread: the providers with their models and the
/// profiles that run on them in every arrangement, the environments they manage, the defaults,
/// and what a thread made here starts with.
async fn options(ctx: Ctx, (): (), (): ()) -> ApiResult<BindingOptions> {
    let thread = ctx.id("id")?;
    let mut timing = PreviewTiming::new(&ctx, thread);
    let mut conn = ctx.app.db.acquire().await?;
    timing.phase("connection");
    let principal = authorization::principal(&ctx, &mut conn).await?;
    let row = thread_rows::thread(&mut conn, thread)
        .await?
        .ok_or_else(ApiError::not_found)?;
    let topic = row.topic_id;
    timing.phase("authority");
    let settings = resolved::load(&mut conn, thread)
        .await
        .map_err(ApiError::internal)?;
    let environment = match row.environment_id {
        Some(id) => environments::environment(&mut conn, id)
            .await?
            .filter(|environment| environment.deleted_at.is_none()),
        None => None,
    };
    let offers = match &environment {
        Some(environment) => Some(
            binding::environment_offers(&mut conn, environment)
                .await
                .map_err(ApiError::internal)?,
        ),
        None => None,
    };
    let orch_tools = OrchToolsOf::read(&mut conn, thread, row.environment_id).await?;
    let scope = match binding::allowed_set_at(&mut conn, thread)
        .await
        .map_err(ApiError::internal)?
    {
        Some(node) => node_view::refs(&mut conn, &principal, &[node], Policy::default())
            .await?
            .remove(&node),
        None => None,
    };
    let defaults = defaults_of(&mut conn, &principal, topic).await?;
    timing.phase("settings_and_environment");
    let providers = providers_of(
        &mut conn,
        &principal,
        &ctx.app,
        &Ground {
            thread,
            settings: &settings,
            environment: environment.as_ref(),
            offers: offers.as_ref(),
            orch_tools: &orch_tools,
            scope: scope.as_ref(),
        },
    )
    .await?;
    timing.phase("providers");
    let environments = environments_of(
        &mut conn,
        &principal,
        &ctx.app,
        topic,
        thread,
        Permission::EnvironmentManage,
    )
    .await?;
    timing.phase("environments");
    let environment_default =
        match setting::<String>(&mut conn, Place::Node(topic), keys::ENV_THREAD_DEFAULT)
            .await
            .map_err(ApiError::internal)?
            .as_deref()
        {
            Some("fresh") => EnvironmentDefault::Fresh,
            _ => EnvironmentDefault::Shared,
        };
    let (access, new_environment) = access_of(&mut conn, &principal, topic).await?;
    timing.phase("defaults_and_access");
    let can_create = can_create_in(&mut conn, &principal, topic, thread).await?;
    let hosts = hosts_for(&ctx, &mut conn, &principal, topic, &can_create).await;
    let machines = machines_for(&ctx, &mut conn, &principal, &environments).await?;
    let source = source_for(&mut conn, &principal, thread).await?;
    timing.phase("placement_and_source");
    Ok(BindingOptions {
        viewer_default: None,
        environment_readers: None,
        defaults,
        providers,
        environments,
        environment_default,
        source,
        access,
        agent_access: None,
        new_environment,
        can_create,
        hosts,
        machines,
    })
}

/// Source defaults use the repository setting's provenance, with the usual visible node reference.
async fn source_for(
    conn: &mut Conn,
    principal: &Principal,
    node: Id,
) -> ApiResult<orch_api_contract::binding::BindingSource> {
    let repository = orch_engine::jobs::source::resolved(conn, node)
        .await
        .map_err(ApiError::internal)?;
    let resolved = orch_settings::resolve(conn, Place::Node(node), keys::SOURCE_REPOSITORY_URL)
        .await
        .map_err(ApiError::internal)?;
    let from = match resolved.resolved.source.node {
        Some(id) => node_view::refs(conn, principal, &[id], Policy::default())
            .await?
            .remove(&id),
        None => None,
    };
    Ok(orch_api_contract::binding::BindingSource { repository, from })
}

/// The hosts a new environment made for `home` could be on, for whoever may make one there, the one with the most
/// free memory first, with what each has free. Nothing where the caller may make none, or the place names no
/// template it could be made from: the choices of the screen are then the environments and the machines.
pub(super) async fn hosts_for(
    ctx: &Ctx,
    conn: &mut Conn,
    principal: &Principal,
    home: Id,
    can_create: &BindingCanCreate,
) -> Vec<orch_api_contract::environment::PlaceHost> {
    if !can_create.allowed {
        return Vec::new();
    }
    match crate::routes::environments::host_choices(
        &ctx.app,
        conn,
        principal,
        home,
        &orch_api_contract::environment::PlaceListQuery::default(),
        ctx.now.into(),
    )
    .await
    {
        Ok(hosts) => hosts,
        Err(error) => {
            tracing::debug!(%home, ?error, "no hosts are offered for a new environment here");
            Vec::new()
        }
    }
}

/// The caller's own connected machines, each with the environments on it that they manage.
pub(super) async fn machines_for(
    ctx: &Ctx,
    conn: &mut Conn,
    principal: &Principal,
    environments: &[BindingEnvironment],
) -> ApiResult<Vec<BindingMachine>> {
    let own =
        orch_db::queries::resources::routes::machines_of(&mut *conn, principal.user()).await?;
    let ids: Vec<Id> = own.iter().map(|row| row.id).collect();
    let refs = machine_refs(&ctx.app, conn, &ids).await?;
    let owner_ids = own
        .iter()
        .filter_map(|row| row.owner_id)
        .collect::<Vec<_>>();
    let owners = refusals::user_refs(conn, &owner_ids).await?;
    let owner_ids = own
        .iter()
        .map(|row| (row.id, row.owner_id))
        .collect::<HashMap<_, _>>();
    let subjects = ids
        .iter()
        .map(|&id| ("machine".to_owned(), id))
        .collect::<Vec<_>>();
    let grouped = grouping::metadata(&ctx.app, conn, principal, &subjects, None).await?;
    Ok(own
        .iter()
        .filter_map(|row| refs.get(&row.id).cloned())
        .map(|machine| BindingMachine {
            owner: owner_ids
                .get(&machine.id)
                .copied()
                .flatten()
                .and_then(|id| owners.get(&id).cloned()),
            grouping: grouped
                .get(&("machine".to_owned(), machine.id))
                .cloned()
                .unwrap_or_default(),
            environments: environments
                .iter()
                .filter(|entry| entry.machine.as_ref().is_some_and(|on| on.id == machine.id))
                .map(|entry| entry.environment.id)
                .collect(),
            machine,
        })
        .collect())
}

#[path = "binding/ask_models.rs"]
pub mod ask_models;

#[path = "binding/choice_window.rs"]
pub mod choice_window;

#[path = "binding/creation.rs"]
pub mod creation;

#[path = "binding/grouping.rs"]
pub mod grouping;

#[path = "binding/placement.rs"]
pub mod placement;

#[path = "binding/rebinding.rs"]
pub mod rebinding;

#[path = "binding/start.rs"]
pub mod start;
