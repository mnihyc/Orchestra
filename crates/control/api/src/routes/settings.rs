//! Settings of a node, an environment, a provider and the system; secrets metadata and parameters.
//!
//! The registry says which keys there are and where each is written; the
//! settings resolver says what holds at a node and why; the access engine
//! says who may write. Writing a key asks the engine the question the key's
//! class and the scope it is written at give (`validation::needs`), and the
//! resolver then writes the value, or takes it back, with its audit row in
//! the same transaction. A secret is written once and never read back: only
//! its name is listed. What belongs to an environment, a provider or a
//! thread's parameters is read by the routes of those, and is not bound here.

use crate::middleware::authorization::{self, refusals};
use crate::middleware::requests::{ApiError, ApiResult};
use crate::middleware::transaction_guard;
use crate::registry::{Ctx, Registry};
use crate::routes::access::page::same;
use crate::routes::nodes::{invalid, placed, policy_at, view};
use crate::sign_in::origin;
use crate::thread_markers::{self, Line};
use orch_access_contract::decisions::ObjectRef;
use orch_access_contract::permissions::{ObjectKind, Permission, ResourceKind};
use orch_api_contract::common::{ErrorCode, FieldFault, Problem};
use orch_api_contract::nodes::{NodeKind, NodeRef};
use orch_api_contract::settings as api;
use orch_authz::usable::{self, BindingAsk, BindingDecision, Blocker, ProviderChoice};
use orch_authz::{Act, Policy, Principal, nodes, resources};
use orch_db::audit::Actor;
use orch_db::secrets::MasterKey;
use orch_db::{Conn, DbError};
use orch_settings::secrets::{self, Owner, SecretError};
use orch_settings::validation::{self, Need};
use orch_settings::write::{Clear, Write, WriteError, clear, write};
use orch_settings::{Place, SettingView, Target, resolve_all};
use orch_settings_contract::registry::{self, Invalid, Scope, keys};
use orch_types::{Id, Json, Time};
use std::collections::{HashMap, HashSet};
use std::sync::OnceLock;

/// Where a deployment keeps the master key that seals secrets, when no key
/// was installed: the same file that signing in reads.
const MASTER_KEY_VARIABLE: &str = "ORCH_MASTER_KEY_FILE";
/// The path the deployment's container runtime puts that key at.
const MASTER_KEY_PATH: &str = "/run/secrets/orch_master_key";

static MASTER_KEY: OnceLock<MasterKey> = OnceLock::new();

pub(crate) mod reach;

/// Gives the key that secrets are sealed under, for composition and for
/// tests. The first key given stays.
pub fn install_master_key(key: MasterKey) -> bool {
    MASTER_KEY.set(key).is_ok()
}

/// The master key: the one that was given, or else the one in the file the
/// deployment provides.
fn master_key() -> ApiResult<&'static MasterKey> {
    if let Some(key) = MASTER_KEY.get() {
        return Ok(key);
    }
    let path = std::env::var(MASTER_KEY_VARIABLE).unwrap_or_else(|_| MASTER_KEY_PATH.to_owned());
    let bytes = orch_config::read_secret_hex::<32>(std::path::Path::new(&path))
        .map_err(|error| ApiError::internal(format!("the master key cannot be read: {error}")))?;
    Ok(MASTER_KEY.get_or_init(|| MasterKey::new(bytes)))
}

/// Binds this module's requests.
pub fn mount(registry: &mut Registry) {
    registry
        .on(api::SettingRegistryGet, registry_get)
        .on(api::NodeSettingList, setting_list)
        .on(api::NodeSettingReachGet, reach::at_node)
        .on(api::NodeSettingSet, setting_set)
        .on(api::NodeSettingDelete, setting_delete)
        .on(api::NodeSecretList, secret_list)
        .on(api::NodeSecretSet, secret_set)
        .on(api::NodeSecretDelete, secret_delete);
}

/// The registry, as the API's definitions.
async fn registry_get(_ctx: Ctx, (): (), (): ()) -> ApiResult<Vec<api::SettingDef>> {
    registry::all()
        .iter()
        .map(|def| {
            Ok(api::SettingDef {
                key: def.key.to_owned(),
                title: def.title.to_owned(),
                description: def.description.to_owned(),
                group: def.group.as_str().to_owned(),
                schema: offered_schema(def),
                class: same(&def.class).ok_or_else(|| ApiError::internal("a class is none"))?,
                scopes: def
                    .scopes
                    .iter()
                    .map(|scope| scope.as_str().to_owned())
                    .collect(),
                merge: same(&def.merge).ok_or_else(|| ApiError::internal("a merge is none"))?,
                default: def.default_value(),
                example: def.example_value().map_err(|error| {
                    ApiError::internal(format!("invalid help example for {}: {error}", def.key))
                })?,
                text_key: Some(format!("{TEXT_NAMESPACE}.{}", def.key)),
                keywords: Vec::new(),
                display: display_of(def),
            })
        })
        .collect()
}

/// Where the application's own catalogue keeps the words of the settings it
/// ships: a key's title, description and keywords lie under this and the key.
const TEXT_NAMESPACE: &str = "day100.builtin";

/// A schema describes choices that may be offered now, without reinterpreting retained values.
fn offered_schema(def: &registry::SettingDef) -> Json {
    let mut schema = def.schema.json_schema();
    if !orch_agent_programs::Programs::builtin().auto_possible() {
        let path = match def.key {
            keys::APPROVAL_MODE => Some("/enum"),
            keys::APPROVAL_ALLOWED_MODES => Some("/items/enum"),
            _ => None,
        };
        if let Some(values) = path
            .and_then(|path| schema.pointer_mut(path))
            .and_then(Json::as_array_mut)
        {
            values.retain(|value| value.as_str() != Some("auto"));
        }
    }
    schema
}

/// How a number of `def` is shown, as its registry entry declares it: a
/// count as the number alone, an amount of bytes or of time with its unit,
/// and whether zero switches the behavior off. None for what is no number.
fn display_of(def: &registry::SettingDef) -> Option<api::SettingDisplay> {
    let registry::Schema::Int { unit, .. } = &def.schema else {
        return None;
    };
    let unit = match unit {
        registry::Unit::Count => api::SettingDisplayUnit::Raw,
        registry::Unit::Bytes => api::SettingDisplayUnit::Bytes,
        registry::Unit::Milliseconds => api::SettingDisplayUnit::Milliseconds,
        registry::Unit::Seconds => api::SettingDisplayUnit::Seconds,
        registry::Unit::Minutes => api::SettingDisplayUnit::Minutes,
        registry::Unit::Hours => api::SettingDisplayUnit::Hours,
        registry::Unit::Days => api::SettingDisplayUnit::Days,
        registry::Unit::Percent => api::SettingDisplayUnit::Percent,
    };
    Some(api::SettingDisplay {
        unit,
        zero: if def.zero_disables() {
            Some(api::SettingDisplayZero::Off)
        } else if def.key == keys::THREAD_GOAL_MAX_TURNS || def.key == keys::THREAD_GOAL_MAX_MINUTES
        {
            Some(api::SettingDisplayZero::NoLimit)
        } else {
            None
        },
    })
}

fn scope_of(kind: NodeKind) -> Scope {
    match kind {
        NodeKind::Org => Scope::Org,
        NodeKind::Project => Scope::Project,
        NodeKind::Topic => Scope::Topic,
        NodeKind::Thread => Scope::Thread,
    }
}

/// What the caller holds on the node, once: whether a key can be written by
/// them is read from it.
struct Holds {
    permissions: HashSet<Permission>,
    system_admin: bool,
    /// For a thread that is in an environment: that environment, and whether
    /// the caller manages it. Where a thread starts and how far its commands
    /// are confined are its environment's to decide, not the thread's.
    environment: Option<(Id, bool)>,
}

impl Holds {
    async fn of(
        conn: &mut Conn,
        principal: &Principal,
        node: Id,
        kind: NodeKind,
        policy: Policy,
    ) -> ApiResult<Self> {
        let held = nodes::effective(conn, principal, node, policy)
            .await
            .map_err(ApiError::internal)?;
        let environment = match kind {
            NodeKind::Thread => {
                match orch_db::queries::settings::thread_environment(conn, node).await? {
                    Some(found) => Some((
                        found.environment_id,
                        manages_environment(conn, principal, found.environment_id).await,
                    )),
                    None => None,
                }
            }
            NodeKind::Org | NodeKind::Project | NodeKind::Topic => None,
        };
        Ok(Self {
            permissions: held.held.iter().map(|held| held.permission).collect(),
            system_admin: principal.is_system_admin(),
            environment,
        })
    }

    /// The environment the thread asked about is in, where it is in one.
    fn thread_environment(&self) -> Option<Id> {
        self.environment.map(|(environment, _)| environment)
    }

    fn meets(&self, need: Need) -> bool {
        match need {
            Need::SystemAdmin => self.system_admin,
            Need::OnNode { permission, .. } => self.permissions.contains(&permission),
            Need::OnEnvironment {
                permission: Permission::EnvironmentManage,
                environment,
            } => self.environment == Some((environment, true)),
            Need::OnEnvironment { .. } | Need::OnProvider { .. } => false,
        }
    }
}

/// Whether the caller manages the environment `environment`, as the access
/// engine says it: a question it cannot answer is no.
async fn manages_environment(conn: &mut Conn, principal: &Principal, environment: Id) -> bool {
    resources::check(
        conn,
        principal,
        ObjectRef {
            kind: ObjectKind::Environment,
            id: environment,
        },
        Permission::EnvironmentManage,
        Policy::default(),
    )
    .await
    .is_ok_and(|decision| decision.is_allowed())
}

/// The settings as the API shows them, with the nodes they mention named.
async fn shown(
    conn: &mut Conn,
    principal: &Principal,
    views: Vec<(SettingView, bool)>,
) -> ApiResult<Vec<api::ResolvedSetting>> {
    let mut ids: Vec<Id> = views.iter().flat_map(|(view, _)| view.nodes()).collect();
    ids.sort_unstable();
    ids.dedup();
    let refs: HashMap<Id, NodeRef> = view::refs(conn, principal, &ids, Policy::default()).await?;
    views
        .into_iter()
        .map(|(view, permitted)| {
            let setting = view.into_setting(permitted).map_node(|id| {
                refs.get(&id).cloned().unwrap_or(NodeRef {
                    thread_display: None,
                    id,
                    kind: NodeKind::Org,
                    name: None,
                    redacted: true,
                })
            });
            same(&setting).ok_or_else(|| ApiError::internal("a setting does not read as the API's"))
        })
        .collect()
}

/// Every key valid at or below the node, as it holds there.
pub(crate) async fn setting_list(ctx: Ctx, (): (), (): ()) -> ApiResult<Vec<api::ResolvedSetting>> {
    let id = ctx.id("id")?;
    let mut conn = ctx.app.db.acquire().await?;
    let principal = authorization::principal(&ctx, &mut conn).await?;
    let (_, kind) = placed(&mut conn, id).await?;
    let policy = policy_at(&mut conn, id).await?;
    let holds = Holds::of(&mut conn, &principal, id, kind, policy).await?;
    let views = resolve_all(&mut conn, Place::Node(id))
        .await
        .map_err(ApiError::internal)?;
    let scope = scope_of(kind);
    let paired = views
        .into_iter()
        .map(|view| {
            let need = validation::need_for_node(view.def(), scope, id, holds.thread_environment());
            let permitted = need.is_some_and(|need| holds.meets(need));
            (view, permitted)
        })
        .collect();
    let mut rows = shown(&mut conn, &principal, paired).await?;
    add_automation_reason(&mut conn, id, &mut rows).await?;
    add_mode_choices(&ctx, &mut conn, &principal, Place::Node(id), &mut rows).await?;
    Ok(rows)
}

/// A bound thread uses its own profile. Unbound scopes use profiles the ordinary resource
/// authority permits this reader to use, rather than the registry's list of every possible mode.
pub(crate) async fn add_mode_choices(
    ctx: &Ctx,
    conn: &mut Conn,
    principal: &Principal,
    place: Place,
    rows: &mut [api::ResolvedSetting],
) -> ApiResult<()> {
    use orch_agent_contract::descriptor::{ApprovalMode, SandboxMode};
    let auto_possible = orch_agent_programs::Programs::builtin().auto_possible();
    use orch_db::queries::resources::thread_engine as profiles;
    let mode_keys = [
        keys::APPROVAL_MODE,
        keys::APPROVAL_ALLOWED_MODES,
        keys::SANDBOX_MODE,
    ];
    if !rows.iter().any(|row| mode_keys.contains(&row.key.as_str())) {
        return Ok(());
    }
    let node = match place {
        Place::Node(node) => Some(node),
        _ => None,
    };
    let thread = match node {
        Some(node) => orch_db::queries::threads::thread(conn, node).await?,
        None => None,
    };
    let agents = if let Some(thread) = &thread {
        match thread.agent_id {
            Some(agent) => profiles::agent(conn, agent).await?.into_iter().collect(),
            None => Vec::new(),
        }
    } else {
        let mut ids = Vec::new();
        let mut after = None;
        loop {
            let page = resources::visible(
                conn,
                principal,
                ResourceKind::Agent,
                orch_authz::nodes::Page {
                    after,
                    limit: crate::routes::resources::LIST_LIMIT,
                },
                Policy::default(),
            )
            .await
            .map_err(ApiError::internal)?;
            ids.extend(
                page.items
                    .iter()
                    .filter(|seen| {
                        seen.held
                            .iter()
                            .any(|held| held.permission == Permission::AgentUse)
                    })
                    .map(|seen| seen.resource.id),
            );
            let Some(next) = page.next else { break };
            after = Some(next);
        }
        profiles::agents(conn, &ids).await?
    };
    let allowed =
        orch_settings::get::<Vec<ApprovalMode>>(conn, place, keys::APPROVAL_ALLOWED_MODES)
            .await
            .map_err(ApiError::internal)?
            .unwrap_or_default();
    let environment = match &thread {
        Some(thread) => thread.environment_id,
        None => match node {
            Some(node) => orch_settings::get::<Id>(conn, Place::Node(node), keys::ENV_DEFAULT_ID)
                .await
                .map_err(ApiError::internal)?,
            None => None,
        },
    };
    let mut models = Vec::new();
    if auto_possible
        && let Some(thread) = &thread
        && let Some(provider) = thread.provider_id
    {
        let settings = orch_engine::resolved::load(conn, thread.node_id)
            .await
            .map_err(ApiError::internal)?;
        models = ctx
            .app
            .engine
            .review_models(conn, thread, provider, Some(principal.user()), &settings)
            .await
            .map_err(ApiError::internal)?;
    }
    let mut approval = Vec::<ApprovalMode>::new();
    let mut applied = Vec::<ApprovalMode>::new();
    let mut sandbox = Vec::<SandboxMode>::new();
    for agent in agents {
        let Ok(Some(descriptor)) = orch_engine::harnesses::descriptor_of(&agent) else {
            continue;
        };
        for mode in &descriptor.approval_modes {
            if (*mode != ApprovalMode::Auto || auto_possible) && !approval.contains(mode) {
                approval.push(*mode);
            }
        }
        for mode in ctx
            .app
            .engine
            .offered_approval_modes(conn, environment, &descriptor, &allowed, &models)
            .await
            .map_err(ApiError::internal)?
        {
            if !applied.contains(&mode) {
                applied.push(mode);
            }
        }
        for mode in &descriptor.sandbox_modes {
            if !sandbox.contains(mode) {
                sandbox.push(*mode);
            }
        }
    }
    let words = |modes: Json| {
        modes
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|mode| mode.as_str().map(str::to_owned))
            .collect::<Vec<_>>()
    };
    let approval = words(serde_json::to_value(approval).map_err(ApiError::internal)?);
    let applied = words(serde_json::to_value(applied).map_err(ApiError::internal)?);
    let sandbox = words(serde_json::to_value(sandbox).map_err(ApiError::internal)?);
    for row in rows {
        row.available_values = match row.key.as_str() {
            keys::APPROVAL_MODE => Some(applied.clone()),
            keys::APPROVAL_ALLOWED_MODES => Some(approval.clone()),
            keys::SANDBOX_MODE => Some(sandbox.clone()),
            _ => continue,
        };
    }
    Ok(())
}

async fn add_automation_reason(
    conn: &mut Conn,
    node: Id,
    rows: &mut [api::ResolvedSetting],
) -> ApiResult<()> {
    if let Some(row) = rows.iter_mut().find(|row| row.key == keys::AUTO_SUMMARIZER) {
        row.automation_reason = orch_engine::summaries::reason(conn, node)
            .await
            .map_err(ApiError::internal)?;
    }
    if let Some(thread) = orch_db::queries::threads::thread(conn, node).await? {
        let descriptor = match thread.agent_id {
            Some(agent) => {
                match orch_db::queries::resources::thread_engine::agent(conn, agent).await? {
                    Some(agent) => {
                        orch_engine::harnesses::descriptor_of(&agent).map_err(ApiError::internal)?
                    }
                    None => None,
                }
            }
            None => None,
        };
        for (key, control) in [
            (
                keys::AGENTS_SUBAGENTS_AT_ONCE,
                orch_agent_contract::descriptor::SubagentControl::AtOnce,
            ),
            (
                keys::AGENTS_SUBAGENT_DEPTH,
                orch_agent_contract::descriptor::SubagentControl::Depth,
            ),
        ] {
            if let Some(row) = rows.iter_mut().find(|row| row.key == key) {
                row.not_applicable_reason = (!descriptor.as_ref().is_some_and(|descriptor| {
                    descriptor.capabilities.subagents
                        && descriptor.capabilities.subagent_limits.contains(&control)
                }))
                .then(|| "harness_control_unavailable".to_owned());
            }
        }
    }
    Ok(())
}

/// What the answer to a write that was refused is.
pub(crate) async fn write_failed(
    conn: &mut Conn,
    principal: &Principal,
    node: Id,
    error: WriteError,
) -> ApiError {
    match error {
        WriteError::Db(error) => error.into(),
        WriteError::NotFound => ApiError::not_found(),
        WriteError::Invalid(Invalid::UnknownKey(_)) => ApiError::not_found(),
        WriteError::Invalid(Invalid::WrongScope { key, scope }) => {
            invalid("key", &format!("{key} is not set at the scope {scope}"))
        }
        WriteError::Invalid(Invalid::Value { faults, .. }) => Problem::invalid(
            faults
                .into_iter()
                .map(|fault| FieldFault {
                    path: if fault.path.is_empty() {
                        "value".to_owned()
                    } else {
                        format!("value.{}", fault.path)
                    },
                    message: fault.message,
                })
                .collect(),
        )
        .into(),
        WriteError::SideThread => thread_is_side(conn, node).await,
        WriteError::Locked { by } => locked(conn, by.node).await,
        WriteError::Lock(refusal) => ApiError::new(
            match refusal {
                orch_settings::locks::Refusal::NotLockable => ErrorCode::RequestInvalid,
                orch_settings::locks::Refusal::NotPermitted => ErrorCode::AuthzDenied,
            },
            refusal.to_string(),
        ),
        WriteError::Stale { current } => {
            let key = current.def().key;
            let shown = shown(conn, principal, vec![(*current, false)]).await;
            let error = ApiError::new(
                ErrorCode::ConflictStaleRevision,
                "the setting was changed since it was read",
            );
            match shown.map(|list| list.into_iter().next()) {
                Ok(setting) => {
                    // Name the writer of this exact revision from the same history a reader of
                    // this node may read. Another key's newer audit line is not its changer.
                    let by = if let Some(own) =
                        setting.as_ref().and_then(|setting| setting.own.as_ref())
                    {
                        let actor = sqlx::query_scalar::<_, Option<Id>>(
                            "SELECT actor_id FROM audit_log WHERE node_id=$1 AND action='settings.updated'
                             AND details->>'key'=$2 AND details->>'revision'=$3 AND actor_kind='user'
                             AND ($2 <> 'agent.model' OR details->'value' IS DISTINCT FROM details->'was')
                             ORDER BY at DESC,id DESC LIMIT 1",
                        ).bind(node).bind(key).bind(own.revision.to_string())
                            .fetch_optional(&mut *conn).await;
                        let actor = match actor {
                            Ok(actor) => actor.flatten(),
                            Err(failed) => return DbError::from(failed).into(),
                        };
                        match actor {
                            Some(actor) => match refusals::user_refs(conn, &[actor]).await {
                                Ok(mut people) => people.remove(&actor),
                                Err(failed) => return failed,
                            },
                            None => None,
                        }
                    } else {
                        None
                    };
                    error.with("current", setting).with("changed_by", by)
                }
                Err(failed) => failed,
            }
        }
        other @ WriteError::Unreadable { .. } => ApiError::internal(other),
    }
}

/// `409 access.locked`: a lock set at a place above decides the key; there is
/// no code of its own for a setting, and this one says the same thing.
async fn locked(conn: &mut Conn, at: Option<Id>) -> ApiError {
    let place = match at {
        Some(node) => refusals::named_node(conn, node).await.ok(),
        None => None,
    };
    ApiError::new(ErrorCode::AccessLocked, "a lock above decides this setting").with("by", place)
}

async fn thread_is_side(conn: &mut Conn, node: Id) -> ApiError {
    let parent = hierarchy_parent(conn, node).await;
    let error = ApiError::new(
        ErrorCode::ThreadSide,
        "a side thread has no settings of its own",
    );
    match parent {
        Some(parent) => error.with("parent", parent),
        None => error,
    }
}

async fn hierarchy_parent(conn: &mut Conn, node: Id) -> Option<NodeRef> {
    let rows = orch_db::queries::hierarchy::with_ancestors(conn, &[node])
        .await
        .ok()?;
    let parent = rows.iter().find(|row| row.id == node)?.parent_id?;
    refusals::named_node(conn, parent).await.ok()
}

/// What a write is asked of the caller: the question `validation::needs`
/// gives, put to the engine, and nothing besides: a policy is written by
/// whoever makes the rules there whether or not they see what is in the
/// place. A refusal carries what is missing.
async fn must_hold(
    conn: &mut Conn,
    principal: &Principal,
    need: Need,
    policy: Policy,
) -> ApiResult<()> {
    match need {
        Need::SystemAdmin => {
            if principal.is_system_admin() {
                Ok(())
            } else {
                Err(ApiError::new(
                    ErrorCode::AuthzDenied,
                    "only a system admin writes this setting",
                )
                .with("required", "system.admin")
                .with(
                    "object",
                    serde_json::json!({ "kind": "system", "id": Id::nil() }),
                )
                .with("requestable", false)
                .with("override", false)
                .with("missing", Vec::<serde_json::Value>::new()))
            }
        }
        Need::OnNode { permission, node } => {
            authorization::require(
                conn,
                principal,
                node,
                authorization::exact(permission),
                Act::Use,
                policy,
            )
            .await
        }
        // The two keys about where a thread runs, written at a thread that is
        // in an environment: whoever manages that environment decides them.
        Need::OnEnvironment {
            permission,
            environment,
        } => {
            authorization::require_resource(
                conn,
                principal,
                ResourceKind::Environment,
                environment,
                permission,
                Policy::default(),
            )
            .await
        }
        Need::OnProvider {
            permission,
            provider,
        } => {
            authorization::require_resource(
                conn,
                principal,
                ResourceKind::Provider,
                provider,
                permission,
                Policy::default(),
            )
            .await
        }
    }
}

/// Reads what writing a key needs and asks for it; the answer is whether
/// the caller may also set or lift the lock.
pub(crate) async fn permit(
    conn: &mut Conn,
    principal: &Principal,
    node: Id,
    key: &str,
    policy: Policy,
) -> ApiResult<bool> {
    let needs = validation::needs(conn, key, Target::Node(node))
        .await
        .map_err(|error| match error {
            WriteError::Invalid(Invalid::UnknownKey(_)) => ApiError::not_found(),
            WriteError::Invalid(Invalid::WrongScope { key, scope }) => {
                invalid("key", &format!("{key} is not set at the scope {scope}"))
            }
            WriteError::NotFound => ApiError::not_found(),
            other => ApiError::internal(other),
        })?;
    must_hold(conn, principal, needs.value, policy).await?;
    Ok(match needs.lock {
        Need::SystemAdmin => principal.is_system_admin(),
        Need::OnNode { permission, node } => {
            nodes::check(conn, principal, node, permission, Act::Use, policy)
                .await
                .is_ok_and(|decision| decision.is_allowed())
        }
        Need::OnEnvironment { .. } | Need::OnProvider { .. } => false,
    })
}

/// After a write: the key as it holds now, with whether the caller may write
/// it.
async fn answer(
    conn: &mut Conn,
    principal: &Principal,
    node: Id,
    view: SettingView,
) -> ApiResult<api::ResolvedSetting> {
    let policy = policy_at(conn, node).await?;
    let (_, kind) = placed(conn, node).await?;
    let holds = Holds::of(conn, principal, node, kind, policy).await?;
    let need =
        validation::need_for_node(view.def(), scope_of(kind), node, holds.thread_environment());
    let permitted = need.is_some_and(|need| holds.meets(need));
    let mut rows = shown(conn, principal, vec![(view, permitted)]).await?;
    add_automation_reason(conn, node, &mut rows).await?;
    rows.into_iter()
        .next()
        .ok_or_else(|| ApiError::internal("a setting was written and is not there"))
}

/// The effort a model is run at: the one chosen, where the model lists it;
/// otherwise the model's own (`parameters` is its entry at the provider).
fn run_at(chosen: Option<String>, parameters: Option<&Json>) -> Option<String> {
    let listed = parameters
        .and_then(|parameters| parameters.get("efforts"))
        .and_then(Json::as_array);
    let own = parameters
        .and_then(|parameters| parameters.get("default_effort"))
        .and_then(Json::as_str)
        .map(str::to_owned);
    match chosen {
        Some(effort)
            if listed.is_some_and(|efforts| {
                efforts
                    .iter()
                    .any(|listed| listed.as_str() == Some(effort.as_str()))
            }) =>
        {
            Some(effort)
        }
        _ => own,
    }
}

/// The refusal of a model or an effort that nobody may run as it is set; none
/// for what stops a thread whatever it runs (its provider or its profile
/// switched off, an arrangement no longer allowed), which a change of its
/// model does not wait on.
fn not_runnable(blocker: Blocker) -> Option<ApiError> {
    match blocker {
        Blocker::ModelNotAllowed { model } => Some(
            ApiError::new(
                ErrorCode::ModelNotAllowed,
                format!("the model {model} is not available here"),
            )
            .with("reason", "model")
            .with("model", model),
        ),
        Blocker::EffortNotAllowed { effort } => Some(
            ApiError::new(
                ErrorCode::ModelNotAllowed,
                format!("the model does not take the effort {effort}"),
            )
            .with("reason", "effort")
            .with("effort", effort),
        ),
        Blocker::ProviderDisabled
        | Blocker::ProviderDeleted
        | Blocker::ProfileDisabled
        | Blocker::ProfileDeleted
        | Blocker::ArrangementNotAllowed { .. } => None,
    }
}

/// Whoever writes the model or the effort of a thread has to be allowed to
/// run what the thread then runs with on its provider themself: the model it
/// resolves to after the write, whether written here or inherited after a
/// clear, at the effort that model is run at. A refusal names what they lack
/// and who can give it, and nothing is written. A thread that is on no
/// provider yet, or that resolves to no model, is asked nothing here: putting
/// it on one asks the same of whoever does.
async fn may_run_what_it_runs(conn: &mut Conn, principal: &Principal, thread: Id) -> ApiResult<()> {
    let Some(row) = orch_db::queries::threads::thread(conn, thread).await? else {
        return Ok(());
    };
    let Some(provider) = row.provider_id else {
        return Ok(());
    };
    let settings = orch_engine::resolved::load(conn, thread)
        .await
        .map_err(ApiError::internal)?;
    let Some(model) = settings.model else {
        return Ok(());
    };
    let parameters =
        orch_db::queries::settings::thread_model_parameters(conn, thread, &model).await?;
    let effort = run_at(settings.effort, parameters.as_ref());
    let policies = crate::thread_access::AuthzAccess::policies(conn, provider)
        .await
        .map_err(ApiError::internal)?;
    let ask = BindingAsk {
        node: thread,
        permission: Permission::ThreadManage,
        provider: Some(ProviderChoice {
            id: provider,
            model: &model,
            effort: effort.as_deref(),
            arrangement: None,
        }),
        profile: None,
        environment: None,
        models_allowed: settings.models_allowed.as_deref(),
    };
    match usable::check_binding(conn, principal, &ask, &policies).await {
        Ok(BindingDecision::Allowed(_)) => Ok(()),
        Ok(BindingDecision::Refused(refusal)) => Err(refusals::denied(conn, *refusal).await?),
        Ok(BindingDecision::Blocked(blockers)) => blockers
            .into_iter()
            .find_map(not_runnable)
            .map_or(Ok(()), Err),
        Ok(BindingDecision::Exceeds(_) | BindingDecision::NotYourMachine) => Err(
            ApiError::internal("a question about a model was answered about an environment"),
        ),
        Err(error) => Err(refusals::problem(conn, error).await),
    }
}

/// What a thread runs with before one of its settings is written: what the
/// lines of its history compare the new values with.
struct Before {
    /// The model it resolves to.
    model: Option<String>,
    /// The effort it resolves to.
    effort: Option<String>,
    /// The key written, as it resolves.
    value: Option<Json>,
}

impl Before {
    /// What `thread` runs with now, for a write of `key`.
    async fn read(conn: &mut Conn, thread: Id, key: &str) -> ApiResult<Self> {
        let settings = orch_engine::resolved::load(conn, thread)
            .await
            .map_err(ApiError::internal)?;
        let value = orch_settings::resolve(conn, Place::Node(thread), key)
            .await
            .map_err(ApiError::internal)?
            .value()
            .cloned();
        Ok(Self {
            model: settings.model,
            effort: settings.effort,
            value,
        })
    }
}

/// The parameters as a map of members, an absent or other value as none.
fn members(value: Option<&Json>) -> serde_json::Map<String, Json> {
    value.and_then(Json::as_object).cloned().unwrap_or_default()
}

/// Leaves in a thread's history what a write of `key` changed there, in the
/// write's transaction and naming the writer: a new model or effort (for a
/// thread that has had turns, which is when it tells anybody anything), each
/// member of the agent's parameters that changed, or the key itself. The
/// instructions are said to have changed, never what they say.
async fn write_lines(
    conn: &mut Conn,
    now: chrono::DateTime<chrono::Utc>,
    thread: Id,
    by: Id,
    key: &str,
    before: Before,
) -> ApiResult<()> {
    let line_of =
        |key: String, label: String, from: Option<Json>, to: Option<Json>| Line::Setting {
            key,
            label,
            from,
            to,
        };
    let mut lines = Vec::new();
    match key {
        keys::AGENT_MODEL | keys::AGENT_EFFORT => {
            let after = orch_engine::resolved::load(conn, thread)
                .await
                .map_err(ApiError::internal)?;
            let had_turns = orch_db::queries::threads::forking::turn_count(conn, thread).await? > 0;
            let changed = after.model != before.model || after.effort != before.effort;
            match after.model {
                Some(to) if changed && had_turns => lines.push(Line::Model {
                    from: before.model,
                    to,
                    effort_from: before.effort,
                    effort_to: after.effort,
                }),
                // No model left to name: the line says that the model was taken back.
                None if changed => lines.push(line_of(
                    keys::AGENT_MODEL.to_owned(),
                    registry::get(keys::AGENT_MODEL)
                        .map_or_else(String::new, |def| def.title.to_owned()),
                    before.model.map(Json::String),
                    None,
                )),
                // An unchanged value or a thread before its first turn needs no history line.
                Some(_) | None => {}
            }
        }
        keys::AGENT_PARAMS => {
            let after = orch_settings::resolve(conn, Place::Node(thread), key)
                .await
                .map_err(ApiError::internal)?;
            let (old, new) = (members(before.value.as_ref()), members(after.value()));
            let mut names: Vec<&String> = old.keys().chain(new.keys()).collect();
            names.sort_unstable();
            names.dedup();
            for name in names {
                let (from, to) = (old.get(name), new.get(name));
                if from != to {
                    lines.push(line_of(
                        format!("{}.{name}", keys::AGENT_PARAMS),
                        name.clone(),
                        from.cloned(),
                        to.cloned(),
                    ));
                }
            }
        }
        _ => {
            let after = orch_settings::resolve(conn, Place::Node(thread), key)
                .await
                .map_err(ApiError::internal)?;
            let to = after.value().cloned();
            if to != before.value {
                let secret = key == keys::AGENT_INSTRUCTIONS;
                lines.push(line_of(
                    key.to_owned(),
                    after.def().title.to_owned(),
                    if secret { None } else { before.value },
                    if secret { None } else { to },
                ));
            }
        }
    }
    for line in &lines {
        thread_markers::write(conn, now, thread, by, line).await?;
    }
    Ok(())
}

/// What a thread runs with before a write of `key`, where the node is a
/// thread; nothing for a place.
async fn before_on_a_thread(
    conn: &mut Conn,
    node: Id,
    kind: NodeKind,
    key: &str,
) -> ApiResult<Option<Before>> {
    match kind {
        NodeKind::Thread => Ok(Some(Before::read(conn, node, key).await?)),
        NodeKind::Org | NodeKind::Project | NodeKind::Topic => Ok(None),
    }
}

/// After a write or a clear that changed a key of a thread: whoever wrote the
/// model or the effort has to be allowed to run what the thread now runs
/// with, and the thread's history says what changed and who changed it.
async fn thread_changed(
    conn: &mut Conn,
    now: chrono::DateTime<chrono::Utc>,
    principal: &Principal,
    thread: Id,
    key: &str,
    before: Before,
) -> ApiResult<()> {
    if key == keys::AGENT_MODEL || key == keys::AGENT_EFFORT {
        may_run_what_it_runs(conn, principal, thread).await?;
    }
    write_lines(conn, now, thread, principal.user(), key, before).await?;
    // A setting is still a change to the thread when there is no segment in which to put a
    // history line. Stamp its ordinary row so every open reader sees the current settings.
    if let Some(seq) = orch_db::sequences::next(conn, thread).await? {
        orch_db::queries::threads::set_seq_state(conn, thread, seq).await?;
        orch_db::sequences::notify(conn, thread, seq).await?;
    }
    Ok(())
}

/// Writes a key at a node.
async fn setting_set(
    mut ctx: Ctx,
    (): (),
    body: api::NodeSettingSetBody,
) -> ApiResult<api::ResolvedSetting> {
    let node = ctx.id("id")?;
    let key = ctx.param("key")?.to_owned();
    let (mut tx, guard) = transaction_guard::begin(&ctx).await?;
    ctx.now = guard.now().get();
    let principal = guard.principal();
    let (_, kind) = placed(&mut tx, node).await?;
    let policy = policy_at(&mut tx, node).await?;
    let may_lock = permit(&mut tx, principal, node, &key, policy).await?;
    super::resources::validation::setting(&ctx.app.engine, &key, &body.value)?;
    let before = before_on_a_thread(&mut tx, node, kind, &key).await?;
    let written = write(
        &mut tx,
        &Write {
            actor: Actor::User(principal.user()),
            target: Target::Node(node),
            key: &key,
            value: body.value,
            locked: body.locked,
            expected_revision: body.expected_revision,
            may_lock,
            origin: &origin(&ctx),
        },
        ctx.now,
    )
    .await;
    let written = match written {
        Ok(written) => written,
        Err(error) => return Err(write_failed(&mut tx, principal, node, error).await),
    };
    if written.changed && key == keys::AGENT_INSTRUCTIONS {
        // Standing instructions that would leave a thread here, or one made here, told more than it may be are
        // refused, and the write with them: nothing of it is committed. Which thread is not said.
        let past = orch_engine::sessions::standing_past_limit(&mut tx, node)
            .await
            .map_err(ApiError::internal)?;
        if let Some(characters) = past {
            let limit = orch_engine::sessions::STANDING_LIMIT;
            let scope = view::refs(&mut tx, principal, &[node], Policy::default())
                .await?
                .remove(&node);
            return Err(Problem::new(
                ErrorCode::SettingsInstructionsTooLarge,
                format!(
                    "the standing instructions would come to {characters} characters with their headings, and {limit} is the most"
                ),
            )
            .with("limit", limit)
            .with("characters", characters)
            .with("scope", scope)
            .into());
        }
    }
    if written.changed {
        if key == keys::ENV_MOUNTS {
            let environments =
                orch_db::queries::environments::hosting::environments_under(&mut tx, node).await?;
            orch_db::queries::resources::mounts::changed(&mut tx, &environments, ctx.now).await?;
        }
        orch_db::queries::hierarchy::stamp(&mut tx, node).await?;
        if let Some(before) = before {
            thread_changed(&mut tx, ctx.now, principal, node, &key, before).await?;
        }
    }
    let mut reply = answer(&mut tx, principal, node, written.view).await?;
    add_mode_choices(
        &ctx,
        &mut tx,
        principal,
        Place::Node(node),
        std::slice::from_mut(&mut reply),
    )
    .await?;
    tx.commit().await.map_err(DbError::from)?;
    if key == keys::AUTH_SECOND_STEP {
        second_step_policy_changed(&ctx).await;
    }
    if key == keys::ENV_MOUNTS
        && let Err(error) = ctx.app.engine.mount_scope_changed(node).await
    {
        tracing::warn!(node = %node, %error, "inherited mount changes remain pending");
    }
    if (key == keys::ENV_RESOURCES || key == keys::RUNTIME_RESOURCES)
        && let Err(error) = ctx.app.engine.resource_scope_changed(node).await
    {
        tracing::warn!(%node,%error,"inherited resource changes remain pending");
    }
    Ok(reply)
}

/// After `auth.second_step` changed at a place: work held for its authors' second step is looked at
/// again, and what is no longer required goes by its thread's ordinary pre-flight.
pub(crate) async fn second_step_policy_changed(ctx: &Ctx) {
    if let Err(error) = ctx.app.engine.reevaluate_second_step(None).await {
        tracing::warn!(%error, "the work that waited for a second step could not be looked at again");
    }
}

/// Takes back what was written at a node, so that the key is inherited.
async fn setting_delete(
    mut ctx: Ctx,
    query: api::NodeSettingDeleteQuery,
    (): (),
) -> ApiResult<api::ResolvedSetting> {
    let node = ctx.id("id")?;
    let key = ctx.param("key")?.to_owned();
    let (mut tx, guard) = transaction_guard::begin(&ctx).await?;
    ctx.now = guard.now().get();
    let principal = guard.principal();
    let (_, kind) = placed(&mut tx, node).await?;
    let policy = policy_at(&mut tx, node).await?;
    let may_lock = permit(&mut tx, principal, node, &key, policy).await?;
    let before = before_on_a_thread(&mut tx, node, kind, &key).await?;
    let cleared = clear(
        &mut tx,
        &Clear {
            actor: Actor::User(principal.user()),
            target: Target::Node(node),
            key: &key,
            expected_revision: query.expected_revision,
            may_lock,
            origin: &origin(&ctx),
        },
        ctx.now,
    )
    .await;
    let cleared = match cleared {
        Ok(cleared) => cleared,
        Err(error) => return Err(write_failed(&mut tx, principal, node, error).await),
    };
    if cleared.changed {
        if key == keys::ENV_MOUNTS {
            let environments =
                orch_db::queries::environments::hosting::environments_under(&mut tx, node).await?;
            orch_db::queries::resources::mounts::changed(&mut tx, &environments, ctx.now).await?;
        }
        orch_db::queries::hierarchy::stamp(&mut tx, node).await?;
        if let Some(before) = before {
            thread_changed(&mut tx, ctx.now, principal, node, &key, before).await?;
        }
    }
    let mut reply = answer(&mut tx, principal, node, cleared.view).await?;
    add_mode_choices(
        &ctx,
        &mut tx,
        principal,
        Place::Node(node),
        std::slice::from_mut(&mut reply),
    )
    .await?;
    tx.commit().await.map_err(DbError::from)?;
    if key == keys::AUTH_SECOND_STEP {
        second_step_policy_changed(&ctx).await;
    }
    if key == keys::ENV_MOUNTS
        && let Err(error) = ctx.app.engine.mount_scope_changed(node).await
    {
        tracing::warn!(node = %node, %error, "inherited mount changes remain pending");
    }
    if (key == keys::ENV_RESOURCES || key == keys::RUNTIME_RESOURCES)
        && let Err(error) = ctx.app.engine.resource_scope_changed(node).await
    {
        tracing::warn!(%node,%error,"inherited resource changes remain pending");
    }
    Ok(reply)
}

// ---- secrets ----

async fn secret_failed(conn: &mut Conn, node: Id, error: SecretError) -> ApiError {
    match error {
        SecretError::Db(error) => error.into(),
        SecretError::NotFound | SecretError::NoSuchSecret => ApiError::not_found(),
        SecretError::BadName => invalid(
            "name",
            "is capitals, digits and underscores, beginning with no digit, at most 64 characters",
        ),
        SecretError::Empty => invalid("value", "must not be empty"),
        SecretError::TooLarge { max } => {
            ApiError::new(ErrorCode::RequestTooLarge, "the value is too large").with("limit", max)
        }
        SecretError::NotHere => invalid("id", "secrets are kept by an org, a project or a topic"),
        SecretError::SideThread => thread_is_side(conn, node).await,
        SecretError::Locked { by } => locked(conn, by.node).await,
        other => ApiError::internal(other),
    }
}

async fn secrets_shown(
    conn: &mut Conn,
    principal: &Principal,
    list: Vec<secrets::SecretInfo>,
) -> ApiResult<Vec<api::SecretInfo>> {
    let ids: Vec<Id> = list.iter().filter_map(|info| info.node).collect();
    let refs = view::refs(conn, principal, &ids, Policy::default()).await?;
    Ok(list
        .into_iter()
        .map(|info| api::SecretInfo {
            name: info.name,
            node: info.node.and_then(|node| refs.get(&node).cloned()),
            rotated_at: Some(Time::new(info.rotated_at.unwrap_or(info.created_at))),
        })
        .collect())
}

/// The names of the secrets that apply at a node, resolved down the
/// hierarchy.
async fn secret_list(ctx: Ctx, (): (), (): ()) -> ApiResult<Vec<api::SecretInfo>> {
    let node = ctx.id("id")?;
    let mut conn = ctx.app.db.acquire().await?;
    let principal = authorization::principal(&ctx, &mut conn).await?;
    let list = match secrets::list(&mut conn, Owner::Node(node)).await {
        Ok(list) => list,
        Err(error) => return Err(secret_failed(&mut conn, node, error).await),
    };
    secrets_shown(&mut conn, &principal, list).await
}

/// Writes a secret at a node.
async fn secret_set(
    mut ctx: Ctx,
    (): (),
    body: api::NodeSecretSetBody,
) -> ApiResult<api::SecretInfo> {
    let node = ctx.id("id")?;
    let name = ctx.param("name")?.to_owned();
    let master = master_key()?;
    let (mut tx, guard) = transaction_guard::begin(&ctx).await?;
    ctx.now = guard.now().get();
    let principal = guard.principal();
    let put = secrets::put(
        &mut tx,
        master,
        &secrets::Put {
            actor: Actor::User(principal.user()),
            owner: Owner::Node(node),
            name: &name,
            value: &body.value,
            origin: &origin(&ctx),
        },
        ctx.now,
    )
    .await;
    let put = match put {
        Ok(put) => put,
        Err(error) => return Err(secret_failed(&mut tx, node, error).await),
    };
    orch_db::queries::hierarchy::stamp(&mut tx, node).await?;
    let shown = secrets_shown(&mut tx, principal, vec![put]).await?;
    tx.commit().await.map_err(DbError::from)?;
    shown
        .into_iter()
        .next()
        .ok_or_else(|| ApiError::internal("a secret was written and is not there"))
}

/// Deletes a secret of a node.
async fn secret_delete(mut ctx: Ctx, (): (), (): ()) -> ApiResult<()> {
    let node = ctx.id("id")?;
    let name = ctx.param("name")?.to_owned();
    let (mut tx, guard) = transaction_guard::begin(&ctx).await?;
    ctx.now = guard.now().get();
    let principal = guard.principal();
    let removed = secrets::delete(
        &mut tx,
        &secrets::Delete {
            actor: Actor::User(principal.user()),
            owner: Owner::Node(node),
            name: &name,
            origin: &origin(&ctx),
        },
        ctx.now,
    )
    .await;
    if let Err(error) = removed {
        return Err(secret_failed(&mut tx, node, error).await);
    }
    orch_db::queries::hierarchy::stamp(&mut tx, node).await?;
    tx.commit().await.map_err(DbError::from)?;
    Ok(())
}

/// The settings `keys` as they hold at the node `id` for the caller: what each
/// resolves to, where that comes from, and whether the caller may write it.
/// The requests that show a part of a node's settings next to something else,
/// as a thread's parameters do, read them here.
pub(crate) async fn resolved_keys(
    conn: &mut Conn,
    principal: &Principal,
    id: Id,
    keys: &[&str],
) -> ApiResult<Vec<api::ResolvedSetting>> {
    let (_, kind) = placed(conn, id).await?;
    let policy = policy_at(conn, id).await?;
    let holds = Holds::of(conn, principal, id, kind, policy).await?;
    let views = resolve_all(conn, Place::Node(id))
        .await
        .map_err(ApiError::internal)?;
    let scope = scope_of(kind);
    let paired = views
        .into_iter()
        .filter(|view| keys.contains(&view.def().key))
        .map(|view| {
            let need = validation::need_for_node(view.def(), scope, id, holds.thread_environment());
            let permitted = need.is_some_and(|need| holds.meets(need));
            (view, permitted)
        })
        .collect();
    shown(conn, principal, paired).await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn zero_disables_these_features_without_losing_their_units() {
        for (key, unit) in [
            (keys::CALLS_REPEAT_LIMIT, api::SettingDisplayUnit::Raw),
            (keys::RELATED_COUNT, api::SettingDisplayUnit::Raw),
            (
                keys::COMPOSER_PASTE_INLINE_CONTEXT_SHARE,
                api::SettingDisplayUnit::Percent,
            ),
            (
                keys::NOTIFICATIONS_PERSON_RATE_PER_HOUR,
                api::SettingDisplayUnit::Raw,
            ),
            (
                keys::NOTIFICATIONS_PERSON_NOTE_CHARS,
                api::SettingDisplayUnit::Raw,
            ),
        ] {
            let display = display_of(registry::get(key).unwrap()).unwrap();
            assert_eq!(display.zero, Some(api::SettingDisplayZero::Off));
            assert_eq!(display.unit, unit);
        }
    }
}
