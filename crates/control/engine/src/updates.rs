//! What an environment or a provider's runtime has fallen behind on, what a release requires of it, and updating a stopped one when it starts where its owner asked for that.
//!
//! A plan records the definition it promises and expires after fifteen minutes. Confirmation checks
//! permissions, the current runtime and its threads again before it queues a replacement. A stopped
//! unit is only updated automatically when its own resolved update policy requests that.

use crate::hosting::ToolBuild;
use crate::jobs::{estimates, provision};
use crate::{Engine, EngineError};
use chrono::{DateTime, Utc};
use orch_access_contract::decisions::ObjectRef;
use orch_access_contract::permissions::{Permission, ResourceKind};
use orch_api_contract::environment::{PendingUpdate, UpdatePreflightReason, UpdateWhat};
use orch_api_contract::forks::{ActiveTurn, AppliedChangeOptions, ChangeOptions, PlanBlocker};
use orch_authz::{Policy, Principal};
use orch_db::queries::{
    environments::hosting as envs,
    jobs::changes as rows,
    resources::{hosting as hosts, provider_runtime as providers},
};
use orch_db::{Conn, DbError};
use orch_settings::{Place, get};
use orch_settings_contract::registry::keys;
use orch_types::{Hash, Id, Json};
use serde::{Deserialize, Serialize};
use serde_json::json;

mod live;
mod observations;

/// A plan is short lived because the template, host and permissions can change meanwhile.
pub const PLAN_MINUTES: i64 = 15;
/// Four concurrent jobs is the switching setting's shipped default when no value is stored.
pub const DEFAULT_BATCH_PARALLEL: i64 = 4;
/// Ten minutes gives an ordinary running turn time to finish.
const DEFAULT_WAIT: i64 = 10;
/// Two minutes covers a connector and its executor coming online.
const DEFAULT_VERIFY: i64 = 120;
/// Verification may be given between ten seconds and one hour.
const VERIFY_RANGE: std::ops::RangeInclusive<i64> = 10..=3_600;
/// The shipped release accepts connectors from the first public protocol version.
pub const CONNECTOR_MINIMUM: &str = "0.1.0";

/// The object whose unit is replaced.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Target {
    /// An environment, with all its threads.
    Environment(Id),
    /// A provider's runtime, with all the provider's threads.
    Provider(Id),
}

impl Target {
    /// Its identifier.
    pub const fn id(self) -> Id {
        match self {
            Self::Environment(id) | Self::Provider(id) => id,
        }
    }
    /// Where its own settings resolve.
    pub const fn place(self) -> Place {
        match self {
            Self::Environment(id) => Place::Environment(id),
            Self::Provider(id) => Place::Provider(id),
        }
    }
    /// The permission its replacement requires.
    pub const fn permission(self) -> Permission {
        match self {
            Self::Environment(_) => Permission::EnvironmentManage,
            Self::Provider(_) => Permission::ProviderManage,
        }
    }
    /// The resource kind.
    pub const fn kind(self) -> ResourceKind {
        match self {
            Self::Environment(_) => ResourceKind::Environment,
            Self::Provider(_) => ResourceKind::Provider,
        }
    }
    /// The job kind of its replacement.
    pub const fn job_kind(self) -> &'static str {
        match self {
            Self::Environment(_) => "environment_recreate",
            Self::Provider(_) => "provider_recreate",
        }
    }
}

/// Why a runtime change cannot be planned or accepted.
#[derive(Debug, thiserror::Error)]
pub enum UpdateError {
    /// The object or plan is not there for this caller.
    #[error("there is no such object or plan")]
    NotFound,
    /// The caller may not manage this object.
    #[error("the caller may not manage this object")]
    Denied,
    /// A preflight has expired.
    #[error("the plan has expired")]
    Expired,
    /// Another job already holds it.
    #[error("a job already holds this object")]
    Held(Id),
    /// A plan has blockers.
    #[error("the plan cannot be carried out")]
    Blocked(Vec<PlanBlocker>),
    /// A request option is outside its range.
    #[error("{0}")]
    Invalid(String),
    /// The system could not read or write its state.
    #[error(transparent)]
    Internal(#[from] EngineError),
}
impl From<DbError> for UpdateError {
    fn from(error: DbError) -> Self {
        Self::Internal(error.into())
    }
}

/// The durable inputs of a replacement, separate from the names shown to a caller.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Recipe {
    /// Whole-home transfer through the common selector/archive protocol, before target creation.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub volume_carry: Option<Json>,
    /// Storage of newly owned provider home data, frozen with the reviewed choice.
    #[serde(default)]
    pub provider_home_storage: Option<String>,
    /// Explicit update selection, frozen with the plan. Recreate retains the current bindings.
    #[serde(default)]
    pub data_sources: Option<orch_driver_contract::runtime::EnvironmentDataSources>,
    /// A native capability change, decided independently from runtime-management authority.
    #[serde(default)]
    pub capability: Option<String>,
    /// The readable reason for that change, without replacing the executable recipe's reason enum.
    #[serde(default)]
    pub capability_reason: Option<String>,
    /// Capability requests use their seconds setting, without rounding it to the wizard's minutes.
    #[serde(default)]
    pub capability_grace_secs: Option<i64>,
    /// Same template/source on a driver that preserves installed files; image/template changes start fresh.
    #[serde(default)]
    pub preserve_files: bool,
    /// Local stopped-filesystem recording measured before the change is accepted.
    #[serde(default)]
    pub keep_estimate: Option<orch_driver_contract::kept::KeepEstimate>,
    /// The resource changed.
    pub target: Target,
    /// The current unit, which remains usable until commit.
    pub old: Id,
    /// Who asked for it.
    pub by: Id,
    /// Applying pending changes, or making the same definition again.
    pub reason: UpdatePreflightReason,
    /// The template to apply.
    pub template: Id,
    /// The stored template definition.
    pub definition: Json,
    /// The network policy frozen at confirmation; the node supplies its trusted endpoints.
    #[serde(default)]
    pub network_policy: orch_driver_contract::network::NetworkPolicy,
    /// The resolved limits promised by the plan.
    pub resolved_resources: Json,
    /// The programs selected for this unit.
    pub tools: Vec<ToolBuild>,
    /// The hash used to compare future changes.
    pub hash: Vec<u8>,
    /// The options used by all phases.
    pub options: AppliedChangeOptions,
    /// Every affected thread, including ones hidden from the caller.
    pub threads: Vec<Id>,
    /// Whether the old unit was stopped when accepted.
    pub was_stopped: bool,
    /// Its environment facts before any command is sent.
    pub old_facts: Json,
    /// An owner-requested update on wake falls back to starting the previous unit.
    #[serde(default)]
    pub auto_start: bool,
    /// For a provider's runtime whose host is changed: the host its new unit is made on, which is given the sign-ins
    /// and harness homes of the old unit before it first starts. `None`: the same host.
    #[serde(default)]
    pub host: Option<Id>,
}

/// What the wizard shows and what its confirmation needs.
#[derive(Debug, Clone)]
pub struct Draft {
    /// The definition to execute.
    pub recipe: Recipe,
    /// What the current unit is behind on.
    pub updates: Vec<PendingUpdate>,
    /// What stays after replacement.
    pub kept: Vec<String>,
    /// What ends or is removed.
    pub lost: Vec<String>,
    /// Obstacles that must be resolved before confirmation.
    pub blockers: Vec<PlanBlocker>,
}

/// A concise blocker used by the update and move wizards.
pub fn blocker(code: &str, message: impl Into<String>) -> PlanBlocker {
    PlanBlocker {
        code: code.to_owned(),
        message: message.into(),
        missing: None,
    }
}

/// The resolved options, with the request's choices applied and bounds checked.
pub async fn options(
    conn: &mut Conn,
    place: Place,
    given: Option<&ChangeOptions>,
) -> Result<AppliedChangeOptions, UpdateError> {
    let active_turn = match given.and_then(|o| o.active_turn) {
        Some(value) => value,
        None => {
            if get::<String>(conn, place, keys::SWITCH_ACTIVE_TURN)
                .await
                .map_err(EngineError::from)?
                .as_deref()
                == Some("interrupt")
            {
                ActiveTurn::Interrupt
            } else {
                ActiveTurn::Wait
            }
        }
    };
    let wait_minutes = match given.and_then(|o| o.wait_minutes) {
        Some(value) => value,
        None => get::<i64>(conn, place, keys::SWITCH_WAIT_MINUTES)
            .await
            .map_err(EngineError::from)?
            .unwrap_or(DEFAULT_WAIT),
    };
    let verify_timeout_secs = match given.and_then(|o| o.verify_timeout_secs) {
        Some(value) => value,
        None => get::<i64>(conn, place, keys::SWITCH_VERIFY_TIMEOUT_SECS)
            .await
            .map_err(EngineError::from)?
            .unwrap_or(DEFAULT_VERIFY),
    };
    if wait_minutes < 0 || chrono::Duration::try_minutes(wait_minutes).is_none() {
        return Err(UpdateError::Invalid(
            "options.wait_minutes must be a nonnegative representable duration".to_owned(),
        ));
    }
    if !VERIFY_RANGE.contains(&verify_timeout_secs) {
        return Err(UpdateError::Invalid(
            "options.verify_timeout_secs is between 10 and 3600".to_owned(),
        ));
    }
    let keep_previous = match given.and_then(|o| o.keep_previous) {
        Some(value) => value,
        None => get::<bool>(conn, place, keys::SWITCH_KEEP_PREVIOUS)
            .await
            .map_err(EngineError::from)?
            .unwrap_or(false),
    };
    Ok(AppliedChangeOptions {
        active_turn,
        wait_minutes,
        verify_timeout_secs,
        keep_previous,
        stop_processes: given.and_then(|o| o.stop_processes).unwrap_or(true),
    })
}

/// Whether the caller may manage the target, through its ordinary page.
pub async fn may_manage(
    conn: &mut Conn,
    principal: &Principal,
    target: Target,
) -> Result<bool, EngineError> {
    Ok(orch_authz::resources::check(
        conn,
        principal,
        ObjectRef {
            kind: target.kind().into(),
            id: target.id(),
        },
        target.permission(),
        Policy::default(),
    )
    .await?
    .is_allowed())
}

/// The release requires an update when a reported connector precedes its minimum version.
pub fn update_required(version: Option<&str>, minimum: &str) -> bool {
    fn parts(version: &str) -> Option<(u64, u64, u64)> {
        let mut pieces = version.trim_start_matches('v').split(['.', '-', '+']);
        Some((
            pieces.next()?.parse().ok()?,
            pieces.next()?.parse().ok()?,
            pieces.next()?.parse().ok()?,
        ))
    }
    version
        .and_then(parts)
        .zip(parts(minimum))
        .is_some_and(|(held, needed)| held < needed)
}

/// The hash of what a runtime would be made with; transient enrollment material is excluded.
pub fn definition_hash(
    driver: &str,
    definition: &Json,
    resources: &Json,
    tools: &[ToolBuild],
) -> Hash {
    // Catalogue and frozen-job order are not part of a runtime's definition.
    let mut versions: Vec<_> = tools
        .iter()
        .map(|tool| (&tool.name, &tool.version))
        .collect();
    versions.sort_unstable();
    let mut value = json!([driver, definition, resources, versions]);
    // serde_json preserves insertion order here; PostgreSQL jsonb does not. Object key
    // order is not an update either. Ordered arrays (argv, commands) still keep their order.
    value.sort_all_objects();
    Hash::of(value.to_string().as_bytes())
}

/// The recorded definition of a unit without what the node observed of its image: `image_facts` are recorded beside the
/// template's own fields, and a unit's creation replaces any the template's author stated with them, so their presence
/// is no change of the template.
fn without_observations(recorded: &Json) -> Json {
    let mut recorded = recorded.clone();
    if let Some(spec) = recorded.get_mut("spec").and_then(Json::as_object_mut) {
        spec.remove("image_facts");
    }
    recorded
}

/// The template's definition without what its author states of the image (`image_facts`, as the shipped Default and
/// Developer tools templates do): the recorded copy holds what the node observed instead, so the two are compared
/// without either. A different image, runtime, resources or tools are still differences.
fn without_declared_facts(definition: &Json) -> Json {
    let mut definition = definition.clone();
    if let Some(members) = definition.as_object_mut() {
        members.remove("image_facts");
    }
    definition
}

/// The ordinary definition differences used by both a preflight and a runtime list.
fn differences(
    old: &envs::RuntimeFull,
    template: &hosts::TemplateState,
    definition: &Json,
    resources: &Json,
    recorded: Option<&Json>,
    held_tools: &[ToolBuild],
    tools: &[ToolBuild],
) -> Vec<PendingUpdate> {
    let stated = recorded.map(without_observations);
    let recorded = stated.as_ref();
    let declared = without_declared_facts(definition);
    // `hash` is what a unit's stored hash means (the template's definition as it stands, facts and all); `comparable` is
    // the same without the template's own statements of the image, to be held against a recorded definition that has
    // none either.
    let hash = definition_hash(&template.driver, definition, resources, tools);
    let comparable = definition_hash(&template.driver, &declared, resources, tools);
    let mut updates = Vec::new();
    for tool in tools {
        let held = held_tools.iter().find(|held| held.name == tool.name);
        if !held_tools.is_empty() && held.is_none_or(|held| held.version != tool.version) {
            updates.push(PendingUpdate {
                what: if tool.name == provision::CONNECTOR_TOOL {
                    UpdateWhat::Connector
                } else if tool.name == "orch-exec" {
                    UpdateWhat::Executor
                } else {
                    UpdateWhat::Harness
                },
                name: tool.name.clone(),
                from: held.map(|held| held.version.clone()),
                to: Some(tool.version.clone()),
            });
        }
    }
    if recorded.is_some_and(|recorded| recorded.get("spec") != Some(&declared))
        || template.id != old.template_id
    {
        updates.push(PendingUpdate {
            what: UpdateWhat::Template,
            name: template.name.clone(),
            from: Some(hex_hash(&old.spec_hash)),
            to: Some(hash.to_string()),
        });
    }
    if recorded.is_some_and(|recorded| recorded.get("resources") != Some(resources)) {
        updates.push(PendingUpdate {
            what: UpdateWhat::Settings,
            name: "Resource limits".to_owned(),
            from: recorded
                .and_then(|recorded| recorded.get("resources"))
                .map(Json::to_string),
            to: Some(resources.to_string()),
        });
    }
    // Older units hashed their catalogue's iteration order. Prefer the recorded semantic
    // definition when available, so merely reordering the same builds is not an update.
    let recorded_hash = recorded
        .and_then(|recorded| Some((recorded.get("spec")?, recorded.get("resources")?)))
        .filter(|_| !held_tools.is_empty())
        .map(|(spec, resources)| definition_hash(&old.driver, spec, resources, held_tools));
    let changed = recorded_hash.as_ref().map_or_else(
        || old.spec_hash != hash.as_bytes(),
        |held| held != &comparable,
    );
    if changed && updates.is_empty() {
        updates.push(PendingUpdate {
            what: UpdateWhat::Settings,
            name: "Runtime definition".to_owned(),
            from: Some(hex_hash(&old.spec_hash)),
            to: Some(hash.to_string()),
        });
    }
    if old
        .state_detail
        .as_deref()
        .is_some_and(|detail| detail.contains("an update is available"))
        && definition
            .get("image")
            .and_then(Json::as_str)
            .is_some_and(|image| !image.contains('@'))
    {
        updates.push(PendingUpdate {
            what: UpdateWhat::Image,
            name: "Image".to_owned(),
            from: None,
            to: Some("newer image available on the host".to_owned()),
        });
    }
    updates
}

impl Engine {
    /// A borrowed provider home crosses machines only as an explicit copy to newly owned data.
    pub async fn draft_provider_update_with_copy(
        &self,
        conn: &mut Conn,
        principal: &Principal,
        provider: Id,
        reason: UpdatePreflightReason,
        copy_directories: bool,
    ) -> Result<Draft, UpdateError> {
        let mut draft = self
            .draft_update(conn, principal, Target::Provider(provider), reason, None)
            .await?;
        if copy_directories
            && draft
                .blockers
                .iter()
                .any(|blocker| blocker.code == "existing_directory_on_source_machine")
        {
            draft
                .blockers
                .retain(|blocker| blocker.code != "existing_directory_on_source_machine");
            draft.recipe.data_sources =
                Some(orch_driver_contract::runtime::EnvironmentDataSources::default());
            draft.kept.push("The complete harness home is copied into new storage made by Orch; the original directory is kept".into());
        }
        Ok(draft)
    }

    /// Plans the replacement of one unit, without taking a hold or issuing a command.
    pub async fn draft_update(
        &self,
        conn: &mut Conn,
        principal: &Principal,
        target: Target,
        reason: UpdatePreflightReason,
        given: Option<&ChangeOptions>,
    ) -> Result<Draft, UpdateError> {
        if !may_manage(conn, principal, target).await? {
            return Err(UpdateError::Denied);
        }
        self.inspect_update(conn, principal, target, reason, given)
            .await
    }

    pub(crate) async fn inspect_update(
        &self,
        conn: &mut Conn,
        principal: &Principal,
        target: Target,
        reason: UpdatePreflightReason,
        given: Option<&ChangeOptions>,
    ) -> Result<Draft, UpdateError> {
        let (old, owner, template_id, own_resources, old_facts, stopped) = match target {
            Target::Environment(id) => {
                let state = envs::environment(conn, id)
                    .await?
                    .filter(|s| s.deleted_at.is_none())
                    .ok_or(UpdateError::NotFound)?;
                let template = state
                    .config
                    .get(keys::ENV_TEMPLATE_ID)
                    .and_then(Json::as_str)
                    .and_then(|id| id.parse().ok())
                    .or(state.template_id);
                let resources = get::<Json>(conn, target.place(), keys::ENV_RESOURCES)
                    .await
                    .map_err(EngineError::from)?;
                (
                    envs::live_runtime(conn, id).await?,
                    state.owner_id,
                    template,
                    resources,
                    state.facts,
                    state.status == "stopped",
                )
            }
            Target::Provider(id) => {
                let state = providers::state(conn, id)
                    .await?
                    .filter(|s| s.deleted_at.is_none())
                    .ok_or(UpdateError::NotFound)?;
                let template = state
                    .runtime_config
                    .get(keys::RUNTIME_TEMPLATE_ID)
                    .and_then(Json::as_str)
                    .and_then(|id| id.parse().ok());
                let resources = get::<Json>(conn, target.place(), keys::RUNTIME_RESOURCES)
                    .await
                    .map_err(EngineError::from)?;
                let old = providers::live_runtime(conn, id).await?;
                let stopped = old.as_ref().is_some_and(|r| r.state == "stopped");
                (
                    old,
                    state.owner_id,
                    template,
                    resources,
                    json!({"provider_status":state.status}),
                    stopped,
                )
            }
        };
        let machine = match target {
            Target::Environment(id) => orch_db::queries::environment_facts::of(conn, id).await?,
            Target::Provider(id) => {
                orch_db::queries::environment_facts::of_provider(conn, id).await?
            }
        };
        if machine.is_some_and(|facts| facts.attaches()) {
            return Err(UpdateError::Blocked(vec![blocker(
                "unit_lifecycle_unavailable",
                "this runtime attaches a directory and declares no unit to replace; update its programs on the machine",
            )]));
        }
        let old = old.ok_or_else(|| {
            UpdateError::Blocked(vec![blocker(
                "no_runtime",
                "there is no unit to replace; start it first",
            )])
        })?;
        let template_id = if reason == UpdatePreflightReason::Recreate {
            old.template_id
        } else {
            template_id.unwrap_or(old.template_id)
        };
        let template = hosts::template(conn, template_id)
            .await?
            .filter(|t| t.enabled && t.deleted_at.is_none())
            .ok_or_else(|| {
                UpdateError::Blocked(vec![blocker(
                    "template_unavailable",
                    "the template is gone or disabled",
                )])
            })?;
        let host = hosts::host(conn, old.host_id)
            .await?
            .ok_or(UpdateError::NotFound)?;
        let recorded = rows::definition(conn, old.id).await?;
        let held_tools: Vec<ToolBuild> = rows::tools_of(conn, old.id)
            .await?
            .and_then(|v| serde_json::from_value(v).ok())
            .unwrap_or_default();
        let definition = if reason == UpdatePreflightReason::Recreate {
            recorded
                .as_ref()
                .and_then(|s| s.get("spec"))
                .cloned()
                .ok_or_else(|| {
                    UpdateError::Blocked(vec![blocker(
                        "definition_unavailable",
                        "the unit's original definition is no longer recorded",
                    )])
                })?
        } else {
            match target {
                Target::Provider(id) => {
                    crate::provider_runtime::definition(conn, id, &template.spec).await?
                }
                Target::Environment(id) => {
                    // The environment's own choice of runtime lies over the template's, so that a change of it is
                    // an update like a change of the template's.
                    let mut spec = template.spec.clone();
                    let chosen = crate::isolation::chosen_runtime(conn, id).await?;
                    crate::isolation::apply_choice(&mut spec, chosen.as_deref());
                    spec
                }
            }
        };
        let resources = if reason == UpdatePreflightReason::Recreate {
            recorded
                .as_ref()
                .and_then(|s| s.get("resources"))
                .cloned()
                .unwrap_or_else(|| json!({}))
        } else {
            crate::places::limits_of(&definition, own_resources.as_ref())
        };
        let wanted = match target {
            Target::Environment(id) => provision::harnesses_wanted(
                &definition,
                &crate::binding::offers_of(conn, id).await?,
            ),
            Target::Provider(id) => crate::jobs::provider_provision::harnesses_wanted(
                &providers::state(conn, id)
                    .await?
                    .ok_or(UpdateError::NotFound)?,
                &providers::system_harness_kinds(conn).await?,
            ),
        };
        let services = self
            .host_services()
            .ok_or_else(|| EngineError::unreadable("no host services are attached"))?;
        let tools = if reason == UpdatePreflightReason::Recreate && !held_tools.is_empty() {
            held_tools.clone()
        } else if reason == UpdatePreflightReason::Recreate {
            // Legacy records are checked against their recorded build names below. A later
            // profile edit must not substitute its pin into the original definition.
            services
                .tools(conn, &host.platform_os, &host.platform_arch, &wanted)
                .await
                .map_err(|error| {
                    UpdateError::Blocked(vec![blocker("build_unavailable", error.to_string())])
                })?
        } else {
            let requested = crate::hosting::harness_requests(conn, target, &wanted).await?;
            services
                .requested_tools(conn, &host.platform_os, &host.platform_arch, &requested)
                .await
                .map_err(|error| {
                    UpdateError::Blocked(vec![blocker("build_unavailable", error.to_string())])
                })?
        };
        let mut missing_old_build = false;
        if reason == UpdatePreflightReason::Recreate {
            let held: Vec<Json> = recorded
                .as_ref()
                .and_then(|s| s.get("tools"))
                .and_then(Json::as_array)
                .cloned()
                .unwrap_or_default();
            missing_old_build = held.iter().any(|held| {
                !tools
                    .iter()
                    .any(|tool| held["name"] == tool.name && held["version"] == tool.version)
            });
        }
        let hash = definition_hash(
            if reason == UpdatePreflightReason::Recreate {
                &old.driver
            } else {
                &template.driver
            },
            &definition,
            &resources,
            &tools,
        );
        let mut updates = if reason == UpdatePreflightReason::Update {
            differences(
                &old,
                &template,
                &definition,
                &resources,
                recorded.as_ref(),
                &held_tools,
                &tools,
            )
        } else {
            Vec::new()
        };
        let mut blockers = Vec::new();
        // A provider's runtime whose host is changed moves: its new unit is made on the selected host and given the
        // sign-ins and harness homes of the old one before it first starts, as the plan says before anything is done.
        let mut move_to: Option<(Id, String)> = None;
        if matches!(
            reason,
            UpdatePreflightReason::Update | UpdatePreflightReason::Recreate
        ) && let Target::Provider(_) = target
        {
            let desired: Option<Id> = get(conn, target.place(), keys::RUNTIME_HOST_ID)
                .await
                .map_err(EngineError::from)?;
            if let Some(desired) = desired.filter(|desired| *desired != old.host_id) {
                let to = orch_db::queries::resources::hosting::host(conn, desired)
                    .await
                    .map_err(EngineError::from)?;
                updates.push(PendingUpdate {
                    what: UpdateWhat::Settings,
                    name: "Host".to_owned(),
                    from: Some(host.name.clone()),
                    to: Some(
                        to.as_ref()
                            .map_or_else(|| "the selected host".to_owned(), |to| to.name.clone()),
                    ),
                });
                match to {
                    Some(to) => move_to = Some((desired, to.name)),
                    None => blockers.push(blocker("no_host", "the selected host is gone")),
                }
            }
        }
        if missing_old_build {
            blockers.push(blocker(
                "tool_build_unavailable",
                "a program's original build is no longer recorded; use Update to choose its current build",
            ));
        }
        if matches!(target, Target::Environment(_)) {
            let mounts = serde_json::json!(
                crate::mounts::resolve(conn, target.id(), self.inner.clock.now())
                    .await?
                    .into_iter()
                    .filter_map(|mount| mount.spec)
                    .collect::<Vec<_>>()
            );
            let applied_mounts = orch_db::queries::resources::mounts::applied(conn, old.id).await?;
            let wants_mounts = applied_mounts
                .as_ref()
                .or_else(|| recorded.as_ref().and_then(|spec| spec.get("mounts")))
                .is_some_and(|held| held != &mounts)
                || old.state_detail.as_deref().is_some_and(|detail| {
                    detail.starts_with("mount changes pending:")
                        || detail.starts_with("mount changes need recreate:")
                });
            if reason == UpdatePreflightReason::Update {
                let sources = get::<orch_driver_contract::runtime::EnvironmentDataSources>(
                    conn,
                    target.place(),
                    keys::ENV_DATA_SOURCES,
                )
                .await
                .map_err(EngineError::from)?
                .unwrap_or_default();
                if [("workspace", &sources.workspace), ("home", &sources.home)]
                    .iter()
                    .any(|(role, source)| {
                        old.volumes
                            .get(*role)
                            .and_then(|volume| volume.get("directory"))
                            .and_then(|directory| directory.get("path"))
                            .and_then(Json::as_str)
                            != source.directory()
                    })
                {
                    updates.push(PendingUpdate {
                        what: UpdateWhat::Settings,
                        name: "Workspace and home".into(),
                        from: Some(old.volumes.to_string()),
                        to: Some(serde_json::to_string(&sources).map_err(EngineError::from)?),
                    });
                }
                let storage = get::<Json>(conn, target.place(), keys::ENV_STORAGE)
                    .await
                    .map_err(EngineError::from)?;
                if storage
                    .as_ref()
                    .and_then(Json::as_object)
                    .is_some_and(|storage| {
                        storage.iter().any(|(role, wanted)| {
                            wanted.as_str().is_some_and(|name| {
                                old.volumes
                                    .get(role)
                                    .and_then(|v| v.get("storage"))
                                    .and_then(Json::as_str)
                                    != Some(name)
                            })
                        })
                    })
                {
                    updates.push(PendingUpdate {
                        what: UpdateWhat::Settings,
                        name: "Storage".to_owned(),
                        from: Some(old.volumes.to_string()),
                        to: storage.as_ref().map(Json::to_string),
                    });
                    blockers.push(blocker(
                        "storage_transfer_unavailable",
                        "changing storage needs persistent volumes copied before the old ones are replaced",
                    ));
                }
                if wants_mounts {
                    updates.push(PendingUpdate {
                        what: UpdateWhat::Settings,
                        name: "Mount attachments".to_owned(),
                        from: old_facts.get("mounts").map(Json::to_string),
                        to: Some(mounts.to_string()),
                    });
                }
            }
        }
        let hold = match target {
            Target::Environment(id) => rows::environment_hold(conn, id).await?,
            Target::Provider(id) => rows::provider_hold(conn, id).await?,
        };
        if let Some(job) = hold {
            blockers.push(blocker(
                "already_running",
                format!("job {} is already using this unit or its threads", job.id),
            ));
        }
        if !host.online {
            blockers.push(blocker("host_offline", "the current host is offline"));
        }
        if reason == UpdatePreflightReason::Update && template.driver != old.driver {
            blockers.push(blocker(
                "driver_changed",
                "changing the driver cannot preserve these volumes",
            ));
        }
        if template_id != old.template_id
            && !orch_authz::resources::check(
                conn,
                principal,
                ObjectRef {
                    kind: ResourceKind::Template.into(),
                    id: template_id,
                },
                Permission::TemplateUse,
                Policy::default(),
            )
            .await
            .map_err(EngineError::from)?
            .is_allowed()
        {
            blockers.push(blocker(
                "template_unusable",
                "the caller may not use the newly chosen template",
            ));
        }
        let kind = if matches!(target, Target::Environment(_)) {
            "environment"
        } else {
            "provider"
        };
        if template.kind != kind {
            blockers.push(blocker(
                "template_kind",
                "the template is for another kind of unit",
            ));
        }
        let threads = rows::threads(
            conn,
            match target {
                Target::Environment(id) => Some(id),
                _ => None,
            },
            match target {
                Target::Provider(id) => Some(id),
                _ => None,
            },
            false,
        )
        .await?;
        let network_policy = crate::network::policy(conn, target.place(), &definition).await?;
        let preserve_files = matches!(target, Target::Environment(_))
            && old.template_id == template_id
            && template.driver == old.driver
            // What the driver declares for the runtime the unit runs under: a runtime that keeps nothing of the unit's
            // own files across a replacement does not, whatever the driver does elsewhere.
            && crate::isolation::unit_driver(&host.drivers, &old.driver, recorded.as_ref())
                .is_some_and(|driver| {
                    driver.get("recreate").and_then(Json::as_str) == Some("restart")
                        || driver
                            .get("recreate_preserves_files")
                            .and_then(Json::as_bool)
                            == Some(true)
                })
            && recorded
                .as_ref()
                .and_then(|record| record.get("spec"))
                .is_some_and(|old| {
                    // Another image, or another runtime, starts fresh: kept files are of the unit that was.
                    ["image", "image_digest", "build", "archive", "runtime"]
                        .iter()
                        .all(|key| old.get(*key) == definition.get(*key))
                })
            && !updates
                .iter()
                .any(|update| update.what == UpdateWhat::Image);
        let records_files = preserve_files
            && host
                .drivers
                .get(&old.driver)
                .and_then(|driver| driver.get("recreate"))
                .and_then(Json::as_str)
                != Some("restart");
        let keep_estimate = if records_files && host.online {
            self.estimate_runtime_files(host.machine_id, old.id).await?
        } else {
            None
        };
        let provider_home_copy = matches!(target, Target::Provider(_))
            && move_to.is_some()
            && old.volumes.get("home").is_some();
        if provider_home_copy
            && old
                .volumes
                .pointer("/home/directory/path")
                .and_then(Json::as_str)
                .is_some()
        {
            blockers.push(blocker("existing_directory_on_source_machine","The harness home is an existing directory on the source machine. Choose to copy its complete content into a new volume on the destination; the original directory will be kept."));
        }
        if let Target::Provider(id) = target
            && reason == UpdatePreflightReason::Update
        {
            let source = orch_settings::provider_home(conn, id)
                .await
                .map_err(EngineError::from)?;
            if provider_home_copy
                && source.directory().is_some()
                && source.directory()
                    != old
                        .volumes
                        .pointer("/home/directory/path")
                        .and_then(Json::as_str)
            {
                blockers.push(blocker("home_changed_during_move","Apply the harness home change on its current machine before moving, or choose a copy of its current home into new storage on the destination."));
            }
            if old
                .volumes
                .pointer("/home/directory/path")
                .and_then(Json::as_str)
                != source.directory()
            {
                updates.push(PendingUpdate {
                    what: UpdateWhat::Settings,
                    name: "Harness home".into(),
                    from: old.volumes.get("home").map(Json::to_string),
                    to: Some(serde_json::to_string(&source).map_err(EngineError::from)?),
                });
            }
        }
        let recipe = Recipe {
            volume_carry: provider_home_copy
                .then(|| json!({"source_runtime":old.id,"selectors":["home"]})),
            provider_home_storage: if matches!(target, Target::Provider(_)) {
                get::<orch_settings_contract::unit::UnitStorage>(
                    conn,
                    target.place(),
                    keys::RUNTIME_STORAGE,
                )
                .await
                .map_err(EngineError::from)?
                .and_then(|storage| storage.home)
            } else {
                None
            },
            data_sources: if reason == UpdatePreflightReason::Update {
                match target {
                    Target::Environment(_) => Some(
                        get(conn, target.place(), keys::ENV_DATA_SOURCES)
                            .await
                            .map_err(EngineError::from)?
                            .unwrap_or_default(),
                    ),
                    Target::Provider(id) => {
                        Some(orch_driver_contract::runtime::EnvironmentDataSources {
                            workspace: Default::default(),
                            home: orch_settings::provider_home(conn, id)
                                .await
                                .map_err(EngineError::from)?,
                        })
                    }
                }
            } else {
                None
            },
            capability: None,
            capability_reason: None,
            capability_grace_secs: None,
            preserve_files,
            keep_estimate,
            target,
            old: old.id,
            by: principal.user(),
            reason,
            template: if reason == UpdatePreflightReason::Recreate {
                old.template_id
            } else {
                template_id
            },
            definition,
            network_policy,
            resolved_resources: resources,
            tools,
            hash: hash.as_bytes().to_vec(),
            options: options(conn, target.place(), given).await?,
            threads,
            was_stopped: stopped,
            old_facts,
            auto_start: false,
            host: move_to.as_ref().map(|(id, _)| *id),
        };
        let _ = owner;
        let mut draft = Draft {
            recipe,
            updates,
            kept: if kind == "environment" {
                vec![
                    "Workspace".to_owned(),
                    "Home".to_owned(),
                    "Persistent state and private views".to_owned(),
                    "Conversations, ownership and access".to_owned(),
                ]
            } else if let Some((_, to)) = &move_to {
                vec![
                    format!(
                        "Sign-ins and harness homes, copied to {to} before its new unit starts"
                    ),
                    "Ownership and access".to_owned(),
                ]
            } else {
                vec![
                    "Sign-in and conversations in the state volume".to_owned(),
                    "Ownership and access".to_owned(),
                ]
            },
            lost: if move_to.is_some() {
                vec![
                    "Running processes".to_owned(),
                    format!(
                        "Its unit on {}: stopped, and kept there with its files until it is removed from that host's page",
                        host.name
                    ),
                ]
            } else {
                vec![
                    "Running processes".to_owned(),
                    "Files outside persistent volumes".to_owned(),
                ]
            },
            blockers,
        };
        if preserve_files {
            draft
                .kept
                .push("All installed programs and files outside the volumes".into());
            draft
                .lost
                .retain(|line| line != "Files outside persistent volumes");
            if records_files {
                draft
                    .kept
                    .push(crate::kept_images::recording_note(keep_estimate));
            }
        } else if kind == "environment" {
            draft.lost.push(crate::isolation::REPLACED_PLAN_NOTE.into());
        }
        Ok(draft)
    }

    /// The pending changes reported on an environment or provider page.
    pub async fn pending_updates(
        &self,
        conn: &mut Conn,
        _principal: &Principal,
        target: Target,
    ) -> Result<Vec<PendingUpdate>, UpdateError> {
        let runtime = match target {
            Target::Environment(id) => envs::live_runtime(conn, id).await?,
            Target::Provider(id) => providers::live_runtime(conn, id).await?,
        };
        let Some(runtime) = runtime else {
            return Ok(Vec::new());
        };
        Ok(self
            .pending_runtime_updates(conn, &[runtime.id])
            .await?
            .remove(&runtime.id)
            .unwrap_or_default())
    }

    /// Queues an update before waking a stopped unit only where its owner requested that policy.
    pub(crate) async fn update_on_start(
        &self,
        conn: &mut Conn,
        environment: Id,
        requested: Option<&Principal>,
        now: DateTime<Utc>,
    ) -> Result<Option<orch_db::queries::jobs::JobRow>, EngineError> {
        let Some(state) = envs::lock_environment(conn, environment).await? else {
            return Ok(None);
        };
        if state.deleted_at.is_some()
            || state.remove_at.is_some()
            || !orch_db::queries::environment_facts::of(conn, environment)
                .await?
                .is_some_and(|facts| facts.makes_units())
        {
            return Ok(None);
        }
        if let Some(job) = rows::environment_hold(conn, environment).await? {
            // A fork may be waking its own target; it must not wait for itself.
            return Ok((job.environment_id == Some(environment)
                && matches!(
                    job.kind.as_str(),
                    "environment_provision"
                        | "environment_recreate"
                        | "environment_move"
                        | "environment_remove"
                ))
            .then_some(job));
        }
        if state.status != "stopped"
            || get::<String>(conn, Place::Environment(environment), keys::ENV_UPDATE)
                .await?
                .as_deref()
                != Some("on_start")
        {
            return Ok(None);
        }
        if let Some(runtime) = envs::live_runtime(conn, environment).await?
            && crate::network::blocked(conn, runtime.id).await?.is_some()
        {
            return Ok(None);
        }
        let owner;
        let principal = match requested {
            Some(principal) => principal,
            None => {
                owner = Principal::load(
                    conn,
                    orch_authz::Caller::session(state.owner_id),
                    orch_types::Time::new(now),
                )
                .await?;
                &owner
            }
        };
        let mut draft = match self
            .draft_update(
                conn,
                principal,
                Target::Environment(environment),
                UpdatePreflightReason::Update,
                None,
            )
            .await
        {
            Ok(draft) => draft,
            Err(UpdateError::Internal(error)) => return Err(error),
            Err(_) => return Ok(None),
        };
        if draft.updates.is_empty() || !draft.blockers.is_empty() {
            return Ok(None);
        }
        draft.recipe.auto_start = true;
        crate::jobs::recreate::queue(conn, draft, None, now)
            .await
            .map(Some)
            .map_err(|e| EngineError::unreadable(e.to_string()))
    }

    /// Applies a provider owner's update policy before waking its stopped runtime.
    pub(crate) async fn provider_update_on_start(
        &self,
        conn: &mut Conn,
        provider: Id,
    ) -> Result<Option<orch_db::queries::jobs::JobRow>, EngineError> {
        if let Some(job) = rows::provider_hold(conn, provider).await? {
            // Work that holds a provider's threads can still start its unchanged runtime for credentials.
            return Ok((job.provider_id == Some(provider)).then_some(job));
        }
        let Some(state) = providers::state(conn, provider).await? else {
            return Ok(None);
        };
        let stopped = providers::live_runtime(conn, provider)
            .await?
            .is_some_and(|r| r.state == "stopped");
        if let Some(runtime) = providers::live_runtime(conn, provider).await?
            && crate::network::blocked(conn, runtime.id).await?.is_some()
        {
            return Ok(None);
        }
        if !stopped
            || get::<String>(conn, Place::Provider(provider), keys::RUNTIME_UPDATE)
                .await?
                .as_deref()
                != Some("on_start")
        {
            return Ok(None);
        }
        let principal = Principal::load(
            conn,
            orch_authz::Caller::session(state.owner_id),
            orch_types::Time::new(self.inner.clock.now()),
        )
        .await?;
        let mut draft = match self
            .draft_update(
                conn,
                &principal,
                Target::Provider(provider),
                UpdatePreflightReason::Update,
                None,
            )
            .await
        {
            Ok(draft) => draft,
            Err(UpdateError::Internal(error)) => return Err(error),
            Err(_) => return Ok(None),
        };
        if draft.updates.is_empty() || !draft.blockers.is_empty() {
            return Ok(None);
        }
        draft.recipe.auto_start = true;
        crate::jobs::recreate::queue(conn, draft, None, self.inner.clock.now())
            .await
            .map(Some)
            .map_err(|e| EngineError::unreadable(e.to_string()))
    }
}

fn hex_hash(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// The range shown while an update provisions a new unit and checks it.
pub fn estimate() -> orch_api_contract::forks::Estimate {
    estimates::estimate(
        &estimates::Work {
            making: estimates::Making::Environment,
            ..Default::default()
        },
        &estimates::Rates::assumed(),
    )
}

/// When an update plan becomes too old to confirm.
pub fn expires(now: DateTime<Utc>) -> DateTime<Utc> {
    now + chrono::Duration::minutes(PLAN_MINUTES)
}

#[cfg(test)]
mod observation_tests {
    use super::*;

    fn runtime(driver: &str, template: Id, hash: &Hash) -> envs::RuntimeFull {
        let now = Utc::now();
        envs::RuntimeFull {
            kept_image: None,
            id: orch_types::new_id(),
            kind: "environment".to_owned(),
            host_id: orch_types::new_id(),
            machine_id: None,
            unit_name: "orch-environment-000000000001".to_owned(),
            successor_of: None,
            provider_id: None,
            environment_id: Some(orch_types::new_id()),
            driver: driver.to_owned(),
            template_id: template,
            spec_hash: hash.as_bytes().to_vec(),
            generation: 1,
            state: "running".to_owned(),
            state_detail: None,
            remove_at: None,
            volumes: json!({}),
            last_active_at: None,
            created_at: now,
            updated_at: now,
        }
    }

    fn template(id: Id, spec: &Json) -> hosts::TemplateState {
        hosts::TemplateState {
            id,
            owner_id: None,
            org_id: None,
            home_id: None,
            name: "Busybox".to_owned(),
            kind: "environment".to_owned(),
            driver: "kubernetes".to_owned(),
            spec: spec.clone(),
            enabled: true,
            revision: 1,
            deleted_at: None,
        }
    }

    fn connector(version: &str) -> ToolBuild {
        ToolBuild {
            name: provision::CONNECTOR_TOOL.to_owned(),
            version: version.to_owned(),
            url: "/api/v1/downloads/connector/linux/x86_64".to_owned(),
            sha256: "0".repeat(64),
        }
    }

    #[test]
    fn what_the_node_observed_of_an_image_is_no_change_of_the_template() {
        let recorded = json!({
            "spec": {"image": "busybox:1", "runtime": "runc", "image_facts": {"source": "a", "docker": false}},
            "resources": {"memory_mb": 512}
        });
        let stated = without_observations(&recorded);
        assert_eq!(
            stated["spec"],
            json!({"image": "busybox:1", "runtime": "runc"})
        );
        assert_eq!(stated["resources"], json!({"memory_mb": 512}));
        assert_eq!(
            without_observations(&json!({"no": "spec"})),
            json!({"no": "spec"})
        );
    }

    #[test]
    fn a_unit_made_with_the_nodes_observations_of_its_image_is_not_behind() {
        // Both comparisons of `differences` are met: the recorded spec against the template's, and the hash of the
        // recorded definition against the hash of what would be made now (held builds are listed, so it is used).
        let spec = json!({"image": "busybox:1", "runtime": "runc"});
        let resources = json!({"memory_mb": 512});
        let tools = vec![connector("1")];
        let hash = definition_hash("kubernetes", &spec, &resources, &tools);
        let template_id = orch_types::new_id();
        let recorded = json!({
            "spec": {"image": "busybox:1", "runtime": "runc", "image_facts": {"source": "a", "docker": false}},
            "resources": resources
        });
        let found = differences(
            &runtime("kubernetes", template_id, &hash),
            &template(template_id, &spec),
            &spec,
            &resources,
            Some(&recorded),
            &tools,
            &tools,
        );
        assert_eq!(found, Vec::<PendingUpdate>::new());
    }

    #[test]
    fn a_changed_template_is_still_behind_whatever_was_observed() {
        let before = json!({"image": "busybox:1", "runtime": "runc"});
        let now = json!({"image": "busybox:2", "runtime": "runc"});
        let resources = json!({"memory_mb": 512});
        let tools = vec![connector("1")];
        let hash = definition_hash("kubernetes", &before, &resources, &tools);
        let template_id = orch_types::new_id();
        let recorded = json!({
            "spec": {"image": "busybox:1", "runtime": "runc", "image_facts": {"source": "a"}},
            "resources": resources
        });
        let found = differences(
            &runtime("kubernetes", template_id, &hash),
            &template(template_id, &now),
            &now,
            &resources,
            Some(&recorded),
            &tools,
            &tools,
        );
        assert_eq!(found.len(), 1, "{found:?}");
        assert_eq!(found[0].what, UpdateWhat::Template);
    }

    #[test]
    fn the_facts_a_shipped_template_states_of_its_image_are_no_change_either() {
        // The shipped Default and Developer tools templates state facts of their image; the unit made from one holds
        // what the node observed instead, and its stored hash is that of the template as it stood.
        let declared = json!({
            "image": "debian:12", "runtime": "runc",
            "image_facts": {"image": "debian:12", "docker": false, "qemu": false}
        });
        let resources = json!({"memory_mb": 512});
        let tools = vec![connector("1")];
        let template_id = orch_types::new_id();
        let stored = definition_hash("kubernetes", &declared, &resources, &tools);
        let recorded = json!({
            "spec": {"image": "debian:12", "runtime": "runc", "image_facts": {"source": "abc", "docker": true}},
            "resources": resources
        });
        let found = differences(
            &runtime("kubernetes", template_id, &stored),
            &template(template_id, &declared),
            &declared,
            &resources,
            Some(&recorded),
            &tools,
            &tools,
        );
        assert_eq!(found, Vec::<PendingUpdate>::new());
    }

    #[test]
    fn a_real_change_is_still_an_update_whatever_the_facts_say() {
        let before = json!({
            "image": "debian:12", "runtime": "runc",
            "image_facts": {"image": "debian:12", "docker": false}
        });
        let resources = json!({"memory_mb": 512});
        let tools = vec![connector("1")];
        let template_id = orch_types::new_id();
        let stored = definition_hash("kubernetes", &before, &resources, &tools);
        let recorded = json!({
            "spec": {"image": "debian:12", "runtime": "runc", "image_facts": {"source": "abc", "docker": true}},
            "resources": resources
        });
        let made = runtime("kubernetes", template_id, &stored);
        // Another image.
        let after = json!({
            "image": "debian:13", "runtime": "runc",
            "image_facts": {"image": "debian:13", "docker": false}
        });
        let found = differences(
            &made,
            &template(template_id, &after),
            &after,
            &resources,
            Some(&recorded),
            &tools,
            &tools,
        );
        assert_eq!(found.len(), 1, "{found:?}");
        assert_eq!(found[0].what, UpdateWhat::Template);
        // Other limits.
        let more = json!({"memory_mb": 1024});
        let found = differences(
            &made,
            &template(template_id, &before),
            &before,
            &more,
            Some(&recorded),
            &tools,
            &tools,
        );
        assert_eq!(found.len(), 1, "{found:?}");
        assert_eq!(found[0].what, UpdateWhat::Settings);
        // A newer connector build.
        let newer = vec![connector("2")];
        let found = differences(
            &made,
            &template(template_id, &before),
            &before,
            &resources,
            Some(&recorded),
            &tools,
            &newer,
        );
        assert_eq!(found.len(), 1, "{found:?}");
        assert_eq!(found[0].what, UpdateWhat::Connector);
    }
}
