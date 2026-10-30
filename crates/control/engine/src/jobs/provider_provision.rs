//! A provider's managed runtime as jobs of the runner: made when the provider is saved as managed, started again when a thread needs it, a person asks, or the sign-in it holds is to be looked at, and taken away with the provider.
//!
//! The job `provider_provision` makes the unit a managed provider's credential lives in, or starts it
//! when it is stopped. Its phases are those of an environment's, without the attach and the init and
//! with the one volume a provider's runtime has: `schedule` (the host: the system's, or one the
//! provider's owner manages, or the one the provider names), `image`, `volumes` (`state`, which is made
//! with the provider and kept as long as it exists), `tools` (the connector, and the builds of the
//! harnesses that may run in the runtime), `create` (the unit, made and started by the host's node),
//! `enroll` (its connector comes online), `verify` (its roles are those of a provider's runtime) and
//! `commit` (one transaction: the runtime is `running`, and the provider says where it is). A start has
//! `start`, `enroll`, `verify` and `commit`; a start to renew a sign-in has one more, `refresh`, after the
//! commit: the harness looks at what it holds, which cannot undo a unit that is up.
//!
//! The job `environment_remove` with a provider takes away a retired unit. Deleting the provider
//! removes its owned volumes; a host manager chooses whether to keep them when removing its unit.
//! A failed removal retains that choice when retried, with the same backoff as an environment's.
//!
//! Every phase but `commit` can be asked again: the commands carry identifiers derived from the job, so a
//! command that was sent is not sent twice, and what a phase decided is kept in the job's plan. A failure,
//! or a cancel before `commit`, is undone: the unit is removed, and the volume with it when this was the
//! first time it was made. A sign-in that waited for the runtime is forgotten with it.

use crate::commands::{CommandsError, Outgoing};
use crate::engine::Engine;
use crate::error::EngineError;
use crate::hosting::{Enrollment, EnrollmentAsk, HostServices, HostingError, ToolBuild};
use crate::ids;
use crate::jobs::provision::{CONNECTOR_TOOL, SpecInputs, image_key, runtime_spec};
use crate::jobs::runner::{Done, Failure, INTERNAL, Job, Phase, Phased, Stop, Undone};
use async_trait::async_trait;
use chrono::{DateTime, Utc};
use orch_access_contract::decisions::ObjectRef;
use orch_access_contract::permissions::{Permission, ResourceKind};
use orch_authz::nodes::Page;
use orch_authz::{Caller, Policy, Principal, resources};
use orch_db::DbError;
use orch_db::queries::environments::hosting::{self as env_rows, RuntimeFull, runtime_state};
use orch_db::queries::jobs::{JobRow, provider_jobs};
use orch_db::queries::resources::hosting::{self as host_rows, HostState, TemplateState};
use orch_db::queries::resources::provider_runtime::{self as rows, State};
use orch_db::queries::resources::thread_engine as machine_rows;
use orch_link_contract::commands::{
    NodeImageEnsure, NodeImageEnsureResult, NodeRuntimeCreate, NodeRuntimeRemove, NodeRuntimeStart,
    NodeRuntimeStop, NodeToolEnsure, NodeVolumeEnsure, NodeVolumeRemove, SerialKey,
};
use orch_settings::{Place as SettingsPlace, get as setting};
use orch_settings_contract::registry::keys;
use orch_types::{Id, Json, Time, new_id};
use serde_json::json;
use std::collections::BTreeMap;
use std::time::Duration;

/// How many hex digits of the runtime's identifier a unit's name holds: the last ones, since the first
/// twelve of an identifier are the moment it was made in and two runtimes made together share them.
const NAME_DIGITS: usize = 12;
/// How many times a unit's name is drawn again when it is taken.
const NAME_TRIES: usize = 4;
/// The roles the connector of a provider's runtime plays: the provider role keeps the credential and
/// forwards model calls, the agent role hosts the harnesses.
pub const ROLES: [&str; 2] = ["provider", "agent"];
/// The provider's always-owned private connector state and unconfigured owned harness homes.
pub const STATE_VOLUME: &str = "state";
/// How many hosts a list of candidates holds at most.
const HOSTS_MAX: usize = 500;

/// What a provisioning job does.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    /// Makes a unit, for a provider that has none or whose unit failed.
    Make,
    /// Starts the unit of a provider whose runtime is stopped.
    Start,
    /// Starts a stopped unit so that its harness looks at the sign-in it holds.
    Renew,
}

impl Mode {
    /// The word the plan keeps.
    #[must_use]
    pub const fn word(self) -> &'static str {
        match self {
            Self::Make => "make",
            Self::Start => "start",
            Self::Renew => "renew",
        }
    }

    fn of(plan: &Json) -> Self {
        match plan.get("mode").and_then(Json::as_str) {
            Some("start") => Self::Start,
            Some("renew") => Self::Renew,
            _ => Self::Make,
        }
    }

    /// The phases of this mode with the weights that shape its bar.
    fn table(self) -> &'static [(&'static str, u32)] {
        match self {
            Self::Make => &provider_jobs::MAKE,
            Self::Start => &provider_jobs::START,
            Self::Renew => &provider_jobs::RENEW,
        }
    }
}

/// The phases of a job in a mode.
#[must_use]
pub fn phases(mode: Mode) -> Vec<Phase> {
    mode.table()
        .iter()
        .map(|(name, weight)| Phase {
            name,
            weight: *weight,
        })
        .collect()
}

/// The phases of taking a provider's runtime away.
#[must_use]
pub fn removal_phases() -> Vec<Phase> {
    provider_jobs::REMOVAL
        .iter()
        .map(|(name, weight)| Phase {
            name,
            weight: *weight,
        })
        .collect()
}

/// Queues the job that makes, starts or renews the runtime of a provider, in the caller's
/// transaction. A job of the kinds that run one at a time that is not over is the answer instead.
pub async fn queue(
    conn: &mut orch_db::Conn,
    now: DateTime<Utc>,
    provider: Id,
    mode: Mode,
    by: Option<Id>,
) -> Result<provider_jobs::Queued, DbError> {
    provider_jobs::queue(
        conn,
        &provider_jobs::Making {
            kind: provider_jobs::PROVISION,
            provider,
            plan: json!({ "mode": mode.word(), "by": by }),
            phases: mode.table(),
            created_by: by,
        },
        now,
    )
    .await
}

/// Queues the removal of a retired unit of a provider's runtime, or of the provider's current one.
pub async fn queue_removal(
    conn: &mut orch_db::Conn,
    now: DateTime<Utc>,
    provider: Id,
    runtime: Id,
) -> Result<provider_jobs::Queued, DbError> {
    let volumes = provider_jobs::removal_volumes(conn, provider, runtime).await?;
    queue_removal_with_options(conn, now, provider, runtime, volumes, None).await
}

/// Removes this exact unit with the host manager's volume choice, in the acceptance transaction.
pub async fn queue_removal_with_options(
    conn: &mut orch_db::Conn,
    now: DateTime<Utc>,
    provider: Id,
    runtime: Id,
    volumes: bool,
    by: Option<Id>,
) -> Result<provider_jobs::Queued, DbError> {
    provider_jobs::queue(
        conn,
        &provider_jobs::Making {
            kind: provider_jobs::REMOVE,
            provider,
            plan: json!({ "runtime": runtime, "volumes": volumes }),
            phases: &provider_jobs::REMOVAL,
            created_by: by,
        },
        now,
    )
    .await
}

/// The name of the unit of a provider's runtime: `orch-provider-` and the last twelve hex digits of the
/// runtime's identifier.
#[must_use]
pub fn unit_name(runtime: &Id) -> String {
    let simple = runtime.simple().to_string();
    let digits = &simple[simple.len().saturating_sub(NAME_DIGITS)..];
    format!("orch-provider-{digits}")
}

/// The name of a provider's `state` volume.
#[must_use]
pub fn state_volume_name(provider: &Id) -> String {
    format!("orch-state-{provider}")
}

/// The body of `node.runtime.create` for a provider's runtime: the stored form of an environment's,
/// with the roles, private state/optional home bindings and labels of a provider's runtime.
#[must_use]
pub fn provider_runtime_spec(provider: Id, inputs: &SpecInputs<'_>) -> Json {
    let mut spec = runtime_spec(inputs);
    spec["kind"] = json!("provider");
    spec["roles"] = json!(ROLES);
    spec["mounts"] = json!([]);
    if let Some(home) = inputs.volumes.get("home") {
        spec["harness_home"] = match home.pointer("/directory/path").and_then(Json::as_str) {
            Some(path) => json!({"kind":"existing_directory","path":path}),
            None => json!({"kind":"made_by_orch"}),
        };
    }
    // Unlike template text, runtime variables can include sealed references resolved on the link.
    if let Some(env) = inputs.spec.get("env") {
        spec["env"] = env.clone();
    }
    spec["labels"]["orch.kind"] = json!("provider");
    spec["labels"]["orch.provider_id"] = json!(provider);
    spec
}

/// The kinds of harness whose builds a provider's runtime is given: its own for a subscription, and
/// for a provider on a key every kind the system has a profile of, since any of them may run there when
/// the thread's arrangement leaves the harness with the provider.
#[must_use]
pub fn harnesses_wanted(provider: &State, profiled: &[String]) -> Vec<String> {
    let mut wanted: Vec<String> = Vec::new();
    let own = provider.harness_kind.iter().cloned();
    let others = profiled
        .iter()
        .filter(|_| !provider.protocols.is_empty())
        .cloned();
    for kind in own.chain(others) {
        if !wanted.contains(&kind) {
            wanted.push(kind);
        }
    }
    wanted
}

fn failed(code: &str, message: impl Into<String>) -> Stop {
    Stop::Failed(Failure::new(code, message))
}

fn body_stop(error: CommandsError) -> Stop {
    failed(INTERNAL, error.to_string())
}

fn hosting_stop(error: &HostingError) -> Stop {
    failed("host_services", error.to_string())
}

// ---------------------------------------------------------------- where it is made

/// A host a provider's runtime could be made on, with what it was judged by.
#[derive(Debug, Clone, PartialEq)]
pub struct Candidate {
    /// The host's row.
    pub host: HostState,
    /// The memory it declares free, in mebibytes.
    pub free_memory_mb: i64,
    /// Whether it is full by the system's measure.
    pub full: bool,
    /// Whether it declares less free memory than the runtime needs.
    pub no_room: bool,
    /// Whether a full host refuses.
    pub blocks: bool,
    /// Why it cannot be chosen, when it cannot.
    pub lacks: Vec<&'static str>,
}

impl Candidate {
    /// Whether the host can be chosen.
    #[must_use]
    pub fn usable(&self) -> bool {
        self.lacks.is_empty()
    }
}

fn number(value: &Json, member: &str) -> i64 {
    value.get(member).and_then(Json::as_i64).unwrap_or(0)
}

fn texts(value: Option<&Json>) -> Vec<String> {
    value
        .and_then(Json::as_array)
        .map(|list| {
            list.iter()
                .filter_map(|item| item.as_str().map(str::to_owned))
                .collect()
        })
        .unwrap_or_default()
}

/// The hosts a provider's runtime could be made on, judged: the hosts its owner may use, of which the
/// system's, the owner's own and the one the provider names are considered; the one with the most free
/// memory first, those that have room before those that only warn.
///
/// A provider's runtime holds a credential and whoever manages a host can read what is on it, so a
/// host that belongs to another member is used only when the provider's own values name it.
pub async fn candidates(
    conn: &mut orch_db::Conn,
    provider: &State,
    template: &TemplateState,
    named: Option<Id>,
    now: DateTime<Utc>,
) -> Result<Vec<Candidate>, EngineError> {
    let principal =
        Principal::load(conn, Caller::session(provider.owner_id), Time::new(now)).await?;
    let seen = resources::visible(
        conn,
        &principal,
        ResourceKind::Host,
        Page {
            after: None,
            limit: HOSTS_MAX,
        },
        Policy::default(),
    )
    .await?;
    let may_use: Vec<Id> = seen
        .items
        .iter()
        .filter(|each| {
            each.held.iter().any(|held| {
                matches!(
                    held.permission,
                    Permission::HostUse | Permission::HostManage
                )
            })
        })
        .map(|each| each.resource.id)
        .collect();
    let template_allowed = resources::check(
        conn,
        &principal,
        ObjectRef {
            kind: ResourceKind::Template.into(),
            id: template.id,
        },
        Permission::TemplateUse,
        Policy::default(),
    )
    .await?
    .is_allowed();
    if may_use.is_empty() {
        return Ok(Vec::new());
    }
    let hosts = host_rows::hosts(conn, Some(&may_use)).await?;
    let mut loads = crate::places::loads_by_host(host_rows::loads(conn, &may_use).await?);
    let own_limits: Option<Json> = setting(
        conn,
        SettingsPlace::Provider(provider.id),
        keys::RUNTIME_RESOURCES,
    )
    .await
    .map_err(EngineError::from)?;
    let own_needs: Option<Json> = setting(
        conn,
        SettingsPlace::Provider(provider.id),
        keys::RUNTIME_NEEDS,
    )
    .await
    .map_err(EngineError::from)?;
    let storage: Option<Json> = setting(
        conn,
        SettingsPlace::Provider(provider.id),
        keys::RUNTIME_STORAGE,
    )
    .await
    .map_err(EngineError::from)?;
    let storage = storage
        .as_ref()
        .and_then(|value| value.get(STATE_VOLUME))
        .and_then(Json::as_str);
    let limits = crate::places::limits_of(&template.spec, own_limits.as_ref());
    let needs = crate::places::needs_of(&template.spec, own_needs.as_ref(), Some(&limits));
    let system_when_full = crate::places::system_when_full(conn).await?;
    let full_percent = crate::places::system_full_percent(conn).await?;
    let source = crate::places::source_of(&template.spec);
    let labels = texts(template.spec.get("host_labels"));
    let runtime_class = template
        .spec
        .get("runtime")
        .and_then(Json::as_str)
        .map(str::to_owned);
    let mut found: Vec<Candidate> = Vec::new();
    for host in hosts {
        let held = loads.remove(&host.id).unwrap_or_default();
        let declared = crate::places::declared_of(&held);
        let capacity_memory = number(&host.capacity, "memory_mb");
        let capacity = orch_api_contract::environment::Amounts {
            cpu_millis: 0,
            memory_mb: capacity_memory,
            disk_gb: 0,
        };
        let full = crate::places::is_full(&capacity, &declared, full_percent);
        let no_room = crate::places::lacks_room(&capacity, &declared, &needs);
        let blocks = host.when_full.as_deref().map_or(
            system_when_full == orch_api_contract::environment::WhenFull::Block,
            |word| word == "block",
        );
        let mut lacks: Vec<&'static str> = Vec::new();
        if !host.schedulable || !host.online || host.machine_status != "active" {
            lacks.push("it takes no new unit");
        }
        let mine = host.owner_id.is_none() || host.owner_id == Some(provider.owner_id);
        if !mine && named != Some(host.id) {
            lacks.push("it belongs to another member, and a provider's runtime holds a sign-in");
        }
        if !template_allowed {
            lacks.push("the provider's owner may not use the template");
        }
        let descriptor = host.drivers.get(&template.driver);
        let makes = descriptor.is_some_and(|descriptor| {
            descriptor
                .get("spec_schemas")
                .and_then(Json::as_object)
                .is_none_or(|pools| pools.contains_key(&template.kind))
        });
        if !makes {
            lacks.push("its driver cannot make this template");
        } else if let Some(descriptor) = descriptor {
            let classes = texts(descriptor.get("runtime_classes"));
            if runtime_class
                .as_ref()
                .is_some_and(|class| !classes.is_empty() && !classes.contains(class))
            {
                lacks.push("its driver cannot make this template");
            }
            let sources = texts(descriptor.get("sources"));
            if !sources.is_empty() && !sources.iter().any(|each| each == source) {
                lacks.push("it does not offer the source the template names");
            }
        }
        if !labels.iter().all(|label| host.labels.contains(label)) {
            lacks.push("it lacks a label the template asks for");
        }
        if let Some(storage) = storage
            && !host.storages.as_array().is_some_and(|storages| {
                storages
                    .iter()
                    .any(|found| found.get("name").and_then(Json::as_str) == Some(storage))
            })
        {
            lacks.push("it does not offer the state volume's named storage");
        }
        if blocks && full {
            lacks.push("it is full");
        }
        if blocks && no_room {
            lacks.push("it declares less free memory than the runtime needs");
        }
        found.push(Candidate {
            free_memory_mb: (capacity_memory - declared.memory_mb).max(0),
            host,
            full,
            no_room,
            blocks,
            lacks,
        });
    }
    found.sort_by(|a, b| {
        b.free_memory_mb
            .cmp(&a.free_memory_mb)
            .then_with(|| a.host.id.cmp(&b.host.id))
    });
    Ok(found)
}

/// Chooses the host: the one named, which must be usable, else the usable one that has room before
/// those that only warn and declares the most free memory. `no_capacity` when a host would take it and
/// refuses for want of room, `no_host` for any other lack.
pub fn choose(found: &[Candidate], named: Option<Id>) -> Result<&Candidate, Failure> {
    let describe =
        |candidate: &Candidate| format!("{}: {}", candidate.host.name, candidate.lacks.join(", "));
    if let Some(named) = named {
        return match found.iter().find(|each| each.host.id == named) {
            None => Err(Failure::new(
                "no_host",
                "the host that was named is not one the provider's owner may use",
            )),
            Some(each) if each.usable() => Ok(each),
            Some(each) => Err(Failure::new(
                if each
                    .lacks
                    .iter()
                    .any(|word| word.contains("full") || word.contains("room"))
                {
                    "no_capacity"
                } else {
                    "no_host"
                },
                describe(each),
            )),
        };
    }
    if let Some(each) = found
        .iter()
        .find(|each| each.usable() && !each.full && !each.no_room)
        .or_else(|| found.iter().find(|each| each.usable()))
    {
        return Ok(each);
    }
    if found.is_empty() {
        return Err(Failure::new(
            "no_host",
            "the provider's owner may use no host that can make its runtime: the system has none, and the owner manages none",
        ));
    }
    let only_room = found.iter().all(|each| {
        each.lacks
            .iter()
            .all(|word| word.contains("full") || word.contains("room"))
    });
    Err(Failure::new(
        if only_room { "no_capacity" } else { "no_host" },
        found.iter().map(describe).collect::<Vec<_>>().join("; "),
    ))
}

// ---------------------------------------------------------------- the job

/// The phases of making, starting or renewing the unit of a provider's runtime.
#[derive(Debug, Clone)]
pub struct ProviderProvision {
    engine: Engine,
    provider: Id,
    mode: Mode,
}

/// Where a unit is, and what it is made from.
struct Placed {
    provider: State,
    runtime: RuntimeFull,
    host: HostState,
    template: TemplateState,
}

impl ProviderProvision {
    /// The work of a job row of the kind `provider_provision`.
    #[must_use]
    pub fn of(engine: Engine, row: &JobRow) -> Self {
        Self {
            engine,
            provider: row.provider_id.unwrap_or_else(Id::nil),
            mode: Mode::of(&row.plan),
        }
    }

    fn services(&self) -> Result<std::sync::Arc<dyn HostServices>, Stop> {
        self.engine
            .host_services()
            .ok_or_else(|| hosting_stop(&HostingError::NotAttached))
    }

    async fn placed(&self, job: &Job) -> Result<Placed, Stop> {
        let plan = job.plan().await?;
        let mut conn = self
            .engine
            .inner
            .db
            .acquire()
            .await
            .map_err(EngineError::from)?;
        let provider = rows::state(&mut conn, self.provider)
            .await?
            .filter(|state| state.deleted_at.is_none())
            .ok_or_else(|| failed("provider_gone", "the provider was deleted"))?;
        let planned = plan
            .get("runtime")
            .and_then(Json::as_str)
            .and_then(|id| id.parse::<Id>().ok());
        let runtime = match planned {
            Some(id) => env_rows::runtime(&mut conn, id).await?,
            None => rows::live_runtime(&mut conn, provider.id).await?,
        }
        .ok_or_else(|| failed("runtime_gone", "the provider has no unit to work on"))?;
        let host = host_rows::host(&mut conn, runtime.host_id)
            .await?
            .ok_or_else(|| failed("no_host", "the host the unit is on is gone"))?;
        if host.disabled || provider.status == "disabled" {
            return Err(failed(
                "provider_unavailable",
                "the provider or its host was disabled",
            ));
        }
        let mut template = host_rows::template(&mut conn, runtime.template_id)
            .await?
            .ok_or_else(|| failed("template_unavailable", "the template is gone"))?;
        // Runtime replacement uses its confirmed definition.
        if let Some(definition) = plan.get("definition") {
            template.spec = definition.clone();
        } else {
            template.spec =
                crate::provider_runtime::definition(&mut conn, provider.id, &template.spec).await?;
        }
        Ok(Placed {
            provider,
            runtime,
            host,
            template,
        })
    }

    // ------------------------------------------------------------ schedule

    async fn schedule(&self, job: &Job) -> Result<Done, Stop> {
        let plan = job.plan().await?;
        if plan.get("runtime").is_some() {
            return Ok(Done::Finished);
        }
        let now = job.now();
        let mut tx = self
            .engine
            .inner
            .db
            .begin()
            .await
            .map_err(EngineError::from)?;
        let provider = rows::lock(&mut tx, self.provider)
            .await?
            .filter(|state| state.deleted_at.is_none())
            .ok_or_else(|| failed("provider_gone", "the provider was deleted"))?;
        if orch_db::queries::environment_facts::of_provider(&mut tx, provider.id)
            .await?
            .is_some_and(|machine| machine.attaches())
        {
            return Err(failed(
                "no_unit_lifecycle",
                "this provider runtime declares directory attachment; it does not make a unit",
            ));
        }
        let template_id: Option<Id> = setting(
            &mut tx,
            SettingsPlace::Provider(provider.id),
            keys::RUNTIME_TEMPLATE_ID,
        )
        .await
        .map_err(EngineError::from)?;
        let template_id = plan
            .pointer("/provider_recovery/template")
            .cloned()
            .map(serde_json::from_value::<Id>)
            .transpose()
            .map_err(EngineError::from)?
            .unwrap_or_else(|| template_id.unwrap_or(orch_db::seeds::PROVIDER_RUNTIME_TEMPLATE_ID));
        let mut template = host_rows::template(&mut tx, template_id)
            .await?
            .filter(|template| {
                template.enabled && template.deleted_at.is_none() && template.kind == "provider"
            })
            .ok_or_else(|| {
                failed(
                    "template_unavailable",
                    "the template of the provider's runtime is gone, switched off, or not one for a runtime",
                )
            })?;
        // A runtime that has volumes stays on the host that has them.
        let previous = rows::live_runtime(&mut tx, provider.id).await?;
        let recovery = plan
            .get("provider_recovery")
            .cloned()
            .map(serde_json::from_value::<crate::provider_recovery::Recipe>)
            .transpose()
            .map_err(EngineError::from)?;
        if let Some(recipe) = &recovery {
            template.spec = recipe.definition.clone();
        }
        let mut named: Option<Id> = setting(
            &mut tx,
            SettingsPlace::Provider(provider.id),
            keys::RUNTIME_HOST_ID,
        )
        .await
        .map_err(EngineError::from)?;
        if let Some(recipe) = &recovery {
            named = Some(recipe.host);
        }
        let listed = candidates(&mut tx, &provider, &template, named, now)
            .await
            .map_err(Stop::from)?;
        let (host, warnings): (HostState, Vec<&str>) = match &previous {
            Some(old) => {
                let candidate = listed
                    .iter()
                    .find(|each| each.host.id == old.host_id)
                    .ok_or_else(|| {
                        failed(
                            "no_host",
                            "the host that has its volume is not one the provider's owner may use",
                        )
                    })?;
                (candidate.host.clone(), warnings_of(candidate))
            }
            None => {
                let chosen = choose(&listed, named).map_err(Stop::Failed)?;
                (chosen.host.clone(), warnings_of(chosen))
            }
        };
        // A unit that failed is taken away before its successor is made, so that two never run at once.
        let fresh = previous.is_none();
        if let Some(old) = &previous {
            env_rows::set_runtime_state(
                &mut tx,
                old.id,
                &runtime_state::LIVE,
                runtime_state::RETIRED,
                Some("replaced by a new unit after it failed"),
                now,
            )
            .await?;
            env_rows::retire_runtime(&mut tx, old.id, Some("replaced after it failed"), now)
                .await?;
            host_rows::revoke_machine_of(&mut tx, old.id, now).await?;
        }
        let generation = previous.as_ref().map_or(1, |old| old.generation + 1);
        let hash = orch_types::Hash::of(
            json!([template.driver, template.spec])
                .to_string()
                .as_bytes(),
        );
        let storage: Option<Json> = setting(
            &mut tx,
            SettingsPlace::Provider(provider.id),
            keys::RUNTIME_STORAGE,
        )
        .await
        .map_err(EngineError::from)?;
        let mut volumes = previous.as_ref().map(|old|old.volumes.clone()).unwrap_or_else(||json!({
            STATE_VOLUME: { "name": state_volume_name(&provider.id), "storage": storage.as_ref().and_then(|value|value.get(STATE_VOLUME)).cloned().unwrap_or(Json::Null) },
        }));
        if recovery.is_some() {
            volumes[STATE_VOLUME]["name"] =
                json!(format!("orch-state-{}-{}", provider.id, job.id()));
        }
        if previous.is_none() {
            let home = orch_settings::provider_home(&mut tx, provider.id)
                .await
                .map_err(EngineError::from)?;
            let selected_storage = storage
                .as_ref()
                .and_then(|value| value.get("home"))
                .filter(|value| !value.is_null());
            // Unconfigured owned homes keep their established location inside private state.
            if home.directory().is_some() || selected_storage.is_some() {
                volumes["home"] = json!({"name":format!("orch-home-{}",provider.id),"storage":selected_storage.cloned().unwrap_or(Json::Null)});
                if let Some(path) = home.directory() {
                    volumes["home"]["directory"] = json!({"path":path});
                    volumes["home"]["storage"] = Json::Null;
                }
            }
        }
        let mut made: Option<(Id, String)> = None;
        for _ in 0..NAME_TRIES {
            let id = new_id();
            let name = unit_name(&id);
            let inserted = rows::insert_runtime(
                &mut tx,
                &rows::NewRuntime {
                    id,
                    host_id: host.id,
                    unit_name: &name,
                    provider_id: provider.id,
                    driver: &template.driver,
                    template_id: template.id,
                    spec_hash: hash.as_bytes(),
                    generation,
                    volumes: &volumes,
                },
                now,
            )
            .await;
            match inserted {
                Ok(()) => {
                    made = Some((id, name));
                    break;
                }
                Err(DbError::Unique { .. }) => {}
                Err(error) => return Err(error.into()),
            }
        }
        let (runtime_id, name) =
            made.ok_or_else(|| failed(INTERNAL, "no free name for the unit was found"))?;
        orch_db::queries::jobs::merge_plan(
            &mut tx,
            job.id(),
            &json!({
                "host": host.id,
                "host_name": host.name,
                "runtime": runtime_id,
                "unit": name,
                "fresh": fresh,
                "warnings": warnings,
            }),
            now,
        )
        .await?;
        tx.commit().await.map_err(DbError::from)?;
        Ok(Done::Finished)
    }

    // ------------------------------------------------------------ image, volumes, tools

    async fn image(&self, job: &Job) -> Result<Done, Stop> {
        let placed = self.placed(job).await?;
        let body = NodeImageEnsure {
            driver: placed.runtime.driver.clone(),
            spec: placed.template.spec.clone(),
            job_id: Some(job.id()),
        };
        let command = Outgoing::new(
            ids::derived("node.image.ensure", &[job.id().as_bytes()]),
            placed.host.machine_id,
            SerialKey::Image(image_key(&placed.template.spec)),
            &body,
        )
        .map_err(body_stop)?;
        let result = job.command(command).await.map_err(|stop| match stop {
            Stop::Failed(failure) => Stop::Failed(failure.with_log(None)),
            other => other,
        })?;
        if let Ok(result) = serde_json::from_value::<NodeImageEnsureResult>(result) {
            job.remember(&json!({
                "image_digest": result.digest,
                "image_facts": crate::isolation::observed_image_facts(
                    &placed.template.spec, &result.programs,
                ),
            }))
            .await?;
        }
        Ok(Done::Finished)
    }

    async fn volumes(&self, job: &Job) -> Result<Done, Stop> {
        let placed = self.placed(job).await?;
        for (role, volume) in placed.runtime.volumes.as_object().into_iter().flatten() {
            if volume
                .get("directory")
                .is_some_and(|directory| !directory.is_null())
            {
                continue;
            }
            let name = volume["name"].as_str().unwrap_or_default().to_owned();
            let body = NodeVolumeEnsure {
                driver: placed.runtime.driver.clone(),
                name: name.clone(),
                storage: volume["storage"].as_str().map(str::to_owned),
                labels: BTreeMap::from([
                    (
                        "orch.provider_id".to_owned(),
                        placed.provider.id.to_string(),
                    ),
                    ("orch.kind".to_owned(), "provider".to_owned()),
                ]),
            };
            let command = Outgoing::new(
                ids::derived(
                    "node.volume.ensure",
                    &[job.id().as_bytes(), role.as_bytes()],
                ),
                placed.host.machine_id,
                SerialKey::Volume(name),
                &body,
            )
            .map_err(body_stop)?;
            job.command(command).await?;
        }
        Ok(Done::Finished)
    }

    async fn tools(&self, job: &Job) -> Result<Done, Stop> {
        let placed = self.placed(job).await?;
        let services = self.services()?;
        let builds = {
            let mut conn = self
                .engine
                .inner
                .db
                .acquire()
                .await
                .map_err(EngineError::from)?;
            let profiled = rows_of_profiles(&mut conn).await?;
            let wanted = harnesses_wanted(&placed.provider, &profiled);
            // Runtime replacement has already selected its builds in the plan.
            if job.plan().await?.get("target").is_some() {
                serde_json::from_value(job.plan().await?["tools"].clone())
                    .map_err(EngineError::from)?
            } else {
                let requested = crate::hosting::harness_requests(
                    &mut conn,
                    crate::updates::Target::Provider(placed.provider.id),
                    &wanted,
                )
                .await?;
                services
                    .requested_tools(
                        &mut conn,
                        &placed.host.platform_os,
                        &placed.host.platform_arch,
                        &requested,
                    )
                    .await
                    .map_err(|error| hosting_stop(&error))?
            }
        };
        for (index, build) in builds.iter().enumerate() {
            let body = NodeToolEnsure {
                job_id: Some(job.id()),
                name: build.name.clone(),
                version: build.version.clone(),
                url: build.url.clone(),
                sha256: build.sha256.clone(),
            };
            let command = Outgoing::new(
                ids::derived(
                    "node.tool.ensure",
                    &[
                        job.id().as_bytes(),
                        build.name.as_bytes(),
                        build.version.as_bytes(),
                    ],
                ),
                placed.host.machine_id,
                SerialKey::Tool(build.name.clone()),
                &body,
            )
            .map_err(body_stop)?;
            job.command(command).await?;
            job.progress((index + 1) as f64 / builds.len().max(1) as f64, None)
                .await?;
        }
        job.remember(&json!({ "tools": builds })).await?;
        Ok(Done::Finished)
    }

    // ------------------------------------------------------------ create, start

    async fn create(&self, job: &Job) -> Result<Done, Stop> {
        let placed = self.placed(job).await?;
        let plan = job.plan().await?;
        let services = self.services()?;
        let enrollment: Enrollment = match plan.get("enrollment") {
            Some(kept) => serde_json::from_value(kept.clone()).map_err(EngineError::from)?,
            None => {
                let mut conn = self
                    .engine
                    .inner
                    .db
                    .acquire()
                    .await
                    .map_err(EngineError::from)?;
                let made = services
                    .enrollment(
                        &mut conn,
                        &EnrollmentAsk {
                            runtime: placed.runtime.id,
                            host: placed.host.id,
                            created_by: placed.provider.owner_id,
                            name_hint: format!("{}-enroll", placed.runtime.unit_name),
                        },
                    )
                    .await
                    .map_err(|error| hosting_stop(&error))?;
                drop(conn);
                job.remember(&json!({ "enrollment": made })).await?;
                made
            }
        };
        let tools: Vec<ToolBuild> = plan
            .get("tools")
            .cloned()
            .and_then(|kept| serde_json::from_value(kept).ok())
            .unwrap_or_default();
        let harness_builds: Vec<ToolBuild> = tools
            .iter()
            .filter(|tool| tool.name != CONNECTOR_TOOL)
            .cloned()
            .collect();
        let limits = if let Some(limits) = plan.get("resolved_resources") {
            limits.clone()
        } else {
            let own: Option<Json> = {
                let mut conn = self
                    .engine
                    .inner
                    .db
                    .acquire()
                    .await
                    .map_err(EngineError::from)?;
                setting(
                    &mut conn,
                    SettingsPlace::Provider(placed.provider.id),
                    keys::RUNTIME_RESOURCES,
                )
                .await
                .map_err(EngineError::from)?
            };
            crate::places::limits_of(&placed.template.spec, own.as_ref())
        };
        let mut spec = provider_runtime_spec(
            placed.provider.id,
            &SpecInputs {
                runtime: placed.runtime.id,
                name: &placed.runtime.unit_name,
                driver: &placed.runtime.driver,
                spec: &placed.template.spec,
                limits: &limits,
                enrollment: &enrollment,
                tools: &harness_builds,
                volumes: &placed.runtime.volumes,
            },
        );
        if let Some(facts) = plan.get("image_facts") {
            spec["spec"]["image_facts"] = facts.clone();
        }
        spec["labels"]["orch.template_id"] = json!(placed.template.id);
        let network_policy = match plan.get("network_policy") {
            Some(policy) => serde_json::from_value(policy.clone()).map_err(EngineError::from)?,
            None => {
                let mut conn = self
                    .engine
                    .db()
                    .acquire()
                    .await
                    .map_err(EngineError::from)?;
                crate::network::policy(
                    &mut conn,
                    SettingsPlace::Provider(placed.provider.id),
                    &placed.template.spec,
                )
                .await?
            }
        };
        spec["network_policy"] =
            serde_json::to_value(&network_policy).map_err(EngineError::from)?;
        job.remember(&json!({ "network_policy": network_policy }))
            .await?;
        // What the unit serves and why it is made, for the operator's hooks on its host.
        spec["subject"] = crate::unit_subjects::provider(
            &job.row().await?.kind,
            placed.provider.id,
            &placed.provider.name,
        );
        // Runtime replacement records what it made for a later plain recreation.
        job.remember(&json!({ "created_spec": spec })).await?;
        {
            let hash = crate::updates::definition_hash(
                &placed.runtime.driver,
                &placed.template.spec,
                &limits,
                &tools,
            );
            let mut conn = self
                .engine
                .db()
                .acquire()
                .await
                .map_err(EngineError::from)?;
            orch_db::queries::environments::changes::save_hash(
                &mut conn,
                placed.runtime.id,
                hash.as_bytes(),
                job.now(),
            )
            .await?;
        }
        let key = SerialKey::Runtime(placed.runtime.id);
        let made = Outgoing::new(
            ids::derived("node.runtime.create", &[job.id().as_bytes()]),
            placed.host.machine_id,
            key.clone(),
            &NodeRuntimeCreate(serde_json::from_value(spec).map_err(EngineError::from)?),
        )
        .map_err(body_stop)?;
        if let Err(stop) = job.command(made).await {
            return Err(explain_creation(&plan, &placed, stop));
        }
        job.progress(0.5, Some("starting the unit")).await?;
        let started = Outgoing::new(
            ids::derived("node.runtime.start", &[job.id().as_bytes(), b"create"]),
            placed.host.machine_id,
            key,
            &NodeRuntimeStart {
                runtime_id: placed.runtime.id,
            },
        )
        .map_err(body_stop)?;
        job.command(started)
            .await
            .map_err(|stop| explain_creation(&plan, &placed, stop))?;
        Ok(Done::Finished)
    }

    async fn start(&self, job: &Job) -> Result<Done, Stop> {
        let placed = self.placed(job).await?;
        if self.mode == Mode::Renew && placed.runtime.state == runtime_state::RUNNING {
            job.remember(&json!({"after_up_reused":true})).await?;
            return Ok(Done::Skipped(
                "the running runtime is reused for renewal".to_owned(),
            ));
        }
        {
            let mut conn = self
                .engine
                .db()
                .acquire()
                .await
                .map_err(EngineError::from)?;
            if let Some(blocked) = crate::network::blocked(&mut conn, placed.runtime.id).await? {
                return Err(failed(
                    "network.restricted_unavailable",
                    blocked.detail.unwrap_or_default(),
                ));
            }
            let policy = crate::network::policy(
                &mut conn,
                SettingsPlace::Provider(placed.provider.id),
                &placed.template.spec,
            )
            .await?;
            drop(conn);
            if policy.restricted() {
                job.command(
                    Outgoing::new(
                        ids::derived("node.network.apply", &[job.id().as_bytes(), b"start"]),
                        placed.host.machine_id,
                        SerialKey::Runtime(placed.runtime.id),
                        &crate::network::apply(placed.runtime.id, &policy),
                    )
                    .map_err(EngineError::from)?,
                )
                .await?;
            }
        }
        let mut conn = self
            .engine
            .inner
            .db
            .acquire()
            .await
            .map_err(EngineError::from)?;
        env_rows::set_runtime_state(
            &mut conn,
            placed.runtime.id,
            &[runtime_state::STOPPED, runtime_state::FAILED],
            runtime_state::CREATING,
            None,
            job.now(),
        )
        .await?;
        drop(conn);
        let command = Outgoing::new(
            ids::derived("node.runtime.start", &[job.id().as_bytes(), b"start"]),
            placed.host.machine_id,
            SerialKey::Runtime(placed.runtime.id),
            &NodeRuntimeStart {
                runtime_id: placed.runtime.id,
            },
        )
        .map_err(body_stop)?;
        job.command(command)
            .await
            .map_err(|stop| explain_creation(&json!({}), &placed, stop))?;
        Ok(Done::Finished)
    }

    // ------------------------------------------------------------ enroll, verify, commit

    async fn enroll(&self, job: &Job) -> Result<Done, Stop> {
        let placed = self.placed(job).await?;
        let allowed = self.engine.inner.config.hosting.enroll_timeout;
        let began = tokio::time::Instant::now();
        loop {
            job.check()?;
            let (runtime, online) = {
                let mut conn = self
                    .engine
                    .inner
                    .db
                    .acquire()
                    .await
                    .map_err(EngineError::from)?;
                (
                    env_rows::runtime(&mut conn, placed.runtime.id).await?,
                    host_rows::machine_of_runtime(&mut conn, placed.runtime.id).await?,
                )
            };
            let Some(runtime) = runtime else {
                return Err(failed("runtime_gone", "the unit's row is gone"));
            };
            if runtime.state == runtime_state::FAILED {
                return Err(failed(
                    "unit_failed",
                    runtime.state_detail.unwrap_or_else(|| {
                        "the unit failed before its connector came online".to_owned()
                    }),
                ));
            }
            if matches!(online, Some((_, true))) {
                return Ok(Done::Finished);
            }
            let waited = began.elapsed();
            if waited >= allowed {
                return Err(failed(
                    "enroll_timeout",
                    format!(
                        "the connector in the unit did not come online within {} seconds",
                        allowed.as_secs()
                    ),
                ));
            }
            job.progress(
                waited.as_secs_f64() / allowed.as_secs_f64().max(1.0),
                Some("waiting for the connector in the unit"),
            )
            .await?;
            job.pause(Duration::from_millis(250)).await?;
        }
    }

    async fn verify(&self, job: &Job) -> Result<Done, Stop> {
        super::after_up::run(job).await?;
        let placed = self.placed(job).await?;
        let mut conn = self
            .engine
            .inner
            .db
            .acquire()
            .await
            .map_err(EngineError::from)?;
        let machine = host_rows::machine_of_runtime(&mut conn, placed.runtime.id).await?;
        let Some((machine, true)) = machine else {
            return Err(failed("unhealthy", "the unit's connector is not online"));
        };
        let place = machine_rows::machine_place(&mut conn, machine).await?;
        let mut wrong: Vec<String> = Vec::new();
        match place {
            Some(place) => {
                for role in ROLES {
                    if !place.roles.iter().any(|each| each == role) {
                        wrong.push(format!("its connector does not take the role of a {role}"));
                    }
                }
            }
            None => wrong.push("the unit's machine is not known".to_owned()),
        }
        let runtime = env_rows::runtime(&mut conn, placed.runtime.id).await?;
        if runtime.is_none_or(|runtime| runtime.state == runtime_state::FAILED) {
            wrong.push("the unit failed".to_owned());
        }
        if wrong.is_empty() {
            Ok(Done::Finished)
        } else {
            Err(failed("unhealthy", wrong.join("; ")))
        }
    }

    async fn commit(&self, job: &Job) -> Result<Done, Stop> {
        job.pass_commit().await?;
        let placed = self.placed(job).await?;
        let now = job.now();
        let recovering = job.plan().await?.get("provider_recovery").is_some();

        let mut tx = self
            .engine
            .inner
            .db
            .begin()
            .await
            .map_err(EngineError::from)?;
        let (machine, _) = host_rows::machine_of_runtime(&mut tx, placed.runtime.id)
            .await?
            .ok_or_else(|| failed("unit_failed", "the unit's connector is not known"))?;
        let locked = rows::lock(&mut tx, placed.provider.id)
            .await?
            .filter(|state| state.deleted_at.is_none())
            .ok_or_else(|| failed("provider_gone", "the provider was deleted"))?;
        env_rows::set_runtime_state(
            &mut tx,
            placed.runtime.id,
            &[runtime_state::CREATING, runtime_state::RUNNING],
            runtime_state::RUNNING,
            None,
            now,
        )
        .await?;
        env_rows::touch_runtime(&mut tx, placed.runtime.id, now).await?;
        rows::set_machine(&mut tx, locked.id, Some(machine), now).await?;
        if recovering {
            sqlx::query("UPDATE providers SET runtime_unavailable=NULL WHERE id=$1")
                .bind(self.provider)
                .execute(&mut *tx)
                .await
                .map_err(DbError::from)?;
        }
        host_rows::delete_runtime_secret(&mut tx, placed.runtime.id, "enroll_token").await?;
        if self.mode == Mode::Make {
            let actor = orch_db::queries::jobs::job(&mut tx, job.id())
                .await?
                .and_then(|row| row.created_by);
            crate::resource_facts::library::created(
                &mut tx,
                placed.template.id,
                None,
                placed.provider.id,
                true,
                actor,
                orch_types::Time::new(now),
            )
            .await?;
        }
        tx.commit().await.map_err(DbError::from)?;
        Ok(Done::Finished)
    }

    /// The renewal's last phase: the harness is started and looks at the sign-in it holds. The unit is
    /// up already, so a harness that cannot say is the provider's managers' to hear of, and does not
    /// fail the job.
    async fn refresh(&self, job: &Job) -> Result<Done, Stop> {
        job.check()?;
        let outcome = self
            .engine
            .renew_sign_in(self.provider)
            .await
            .map_err(Stop::from)?;
        job.progress(1.0, None).await?;
        Ok(match outcome {
            crate::provider_setup::Renewed::Held => Done::Finished,
            crate::provider_setup::Renewed::Lapsed(why) => Done::Skipped(why),
        })
    }

    // ------------------------------------------------------------ undoing

    async fn remove_unit(&self, job: &Job, placed: &Placed, word: &[u8]) -> Result<(), String> {
        let command = Outgoing::new(
            ids::derived("node.runtime.remove", &[job.id().as_bytes(), word]),
            placed.host.machine_id,
            SerialKey::Runtime(placed.runtime.id),
            &NodeRuntimeRemove {
                runtime_id: placed.runtime.id,
                keep_volumes: true,
                volume_refs: Vec::new(),
            },
        )
        .map_err(|error| error.to_string())?;
        match job.command_to_undo(command).await {
            Ok(_) => Ok(()),
            Err(Stop::Failed(failure)) if failure.code == "not_found" => Ok(()),
            Err(Stop::Failed(failure)) => Err(failure.message),
            Err(_) => Err("the system is stopping".to_owned()),
        }
    }

    async fn undo_make(&self, job: &Job, why: &Stop) -> Undone {
        let Ok(plan) = job.plan().await else {
            return Undone::Partly("the job's plan could not be read".to_owned());
        };
        let fresh = plan.get("fresh").and_then(Json::as_bool).unwrap_or(true);
        let Ok(placed) = self.placed(job).await else {
            // Nothing was scheduled: nothing was made.
            self.forget_sign_in(job, why).await;
            return Undone::Everything;
        };
        let mut left: Vec<String> = Vec::new();
        if let Err(why) = self.remove_unit(job, &placed, b"undo").await {
            left.push(format!(
                "the unit {} could not be removed: {why}",
                placed.runtime.unit_name
            ));
        }
        if fresh && left.is_empty() {
            for (role, volume) in placed.runtime.volumes.as_object().into_iter().flatten() {
                if volume
                    .get("directory")
                    .is_some_and(|directory| !directory.is_null())
                {
                    continue;
                }
                let name = volume["name"].as_str().unwrap_or_default().to_owned();
                let body = NodeVolumeRemove {
                    driver: placed.runtime.driver.clone(),
                    name: name.clone(),
                    storage: volume["storage"].as_str().map(str::to_owned),
                    labels: BTreeMap::new(),
                };
                if let Ok(command) = Outgoing::new(
                    ids::derived(
                        "node.volume.remove",
                        &[job.id().as_bytes(), role.as_bytes()],
                    ),
                    placed.host.machine_id,
                    SerialKey::Volume(name.clone()),
                    &body,
                ) {
                    match job.command_to_undo(command).await {
                        Ok(_) => {}
                        Err(Stop::Failed(failure)) if failure.code == "not_found" => {}
                        Err(Stop::Failed(failure)) => left.push(format!(
                            "the volume {name} could not be removed: {}",
                            failure.message
                        )),
                        Err(_) => left.push(format!("the volume {name} could not be removed")),
                    }
                }
            }
        }
        let now = job.now();
        if let Ok(mut conn) = self.engine.inner.db.acquire().await {
            if left.is_empty() {
                let _ = env_rows::set_runtime_state(
                    &mut conn,
                    placed.runtime.id,
                    &runtime_state::LIVE,
                    runtime_state::REMOVED,
                    Some("undone"),
                    now,
                )
                .await;
                let _ = host_rows::revoke_machine_of(&mut conn, placed.runtime.id, now).await;
                let _ =
                    host_rows::delete_runtime_secret(&mut conn, placed.runtime.id, "enroll_token")
                        .await;
            } else {
                // What is left is the reaper's: the runtime waits to be removed.
                let _ = env_rows::retire_runtime(
                    &mut conn,
                    placed.runtime.id,
                    Some(&left.join("; ")),
                    now,
                )
                .await;
            }
        }
        self.forget_sign_in(job, why).await;
        if left.is_empty() {
            Undone::Everything
        } else {
            Undone::Partly(left.join("; "))
        }
    }

    /// A sign-in that waited for the runtime is forgotten with its making: its secrets are deleted, and
    /// the provider says what became of the runtime.
    async fn forget_sign_in(&self, job: &Job, why: &Stop) {
        let phase = job
            .row()
            .await
            .ok()
            .map(|row| crate::jobs::runner::read_progress(&row).0.phase)
            .unwrap_or_default();
        let detail = match why {
            Stop::Cancelled => "making its runtime was cancelled".to_owned(),
            Stop::Failed(failure) => format!(
                "its runtime could not be made ({phase}): {}",
                failure.message
            ),
            Stop::Lost => return,
        };
        if let Err(error) = self
            .engine
            .runtime_failed(self.provider, &detail, job.now())
            .await
        {
            tracing::warn!(%error, provider = %self.provider, "a sign-in that waited for a runtime that was not made could not be forgotten");
        }
    }

    async fn undo_start(&self, job: &Job, why: &Stop) -> Undone {
        let Ok(placed) = self.placed(job).await else {
            return Undone::Everything;
        };
        // The unit was started for nothing: it is stopped again, and the provider stands where it did.
        let stopped = Outgoing::new(
            ids::derived("node.runtime.stop", &[job.id().as_bytes(), b"undo"]),
            placed.host.machine_id,
            SerialKey::Runtime(placed.runtime.id),
            &NodeRuntimeStop {
                runtime_id: placed.runtime.id,
                reason: None,
            },
        );
        let mut left = None;
        if let Ok(command) = stopped
            && let Err(Stop::Failed(failure)) = job.command_to_undo(command).await
            && failure.code != "not_found"
        {
            left = Some(failure.message);
        }
        let now = job.now();
        if let Ok(mut conn) = self.engine.inner.db.acquire().await {
            let (to, detail) = match why {
                Stop::Cancelled => (runtime_state::STOPPED, None),
                Stop::Failed(failure) => (
                    runtime_state::FAILED,
                    Some(format!("starting it: {}", failure.message)),
                ),
                Stop::Lost => return Undone::Everything,
            };
            let _ = env_rows::set_runtime_state(
                &mut conn,
                placed.runtime.id,
                &[runtime_state::CREATING, runtime_state::RUNNING],
                to,
                detail.as_deref(),
                now,
            )
            .await;
        }
        left.map_or(Undone::Everything, Undone::Partly)
    }
}

/// What a host that warns says to the job: what the host declared when it was chosen.
fn warnings_of(candidate: &Candidate) -> Vec<&'static str> {
    let mut warnings = Vec::new();
    if candidate.full {
        warnings.push("full");
    }
    if candidate.no_room {
        warnings.push("no_room");
    }
    warnings
}

/// What a host that could not make a unit says, with what the system had declared of it.
fn explain_creation(plan: &Json, placed: &Placed, stop: Stop) -> Stop {
    let Stop::Failed(failure) = stop else {
        return stop;
    };
    let warned = plan
        .get("warnings")
        .and_then(Json::as_array)
        .is_some_and(|warnings| !warnings.is_empty());
    let message = if warned && failure.code == "no_capacity" {
        format!(
            "{} could not start it: {}. It had declared little room",
            placed.host.name, failure.message
        )
    } else {
        format!(
            "{} could not start it: {}",
            placed.host.name, failure.message
        )
    };
    Stop::Failed(Failure { message, ..failure })
}

/// The kinds of harness of the profiles of the system, which a runtime of a provider on a key is given
/// the builds of.
async fn rows_of_profiles(conn: &mut orch_db::Conn) -> Result<Vec<String>, EngineError> {
    Ok(orch_db::queries::resources::provider_runtime::system_harness_kinds(conn).await?)
}

#[async_trait]
impl Phased for ProviderProvision {
    async fn run(&self, job: &Job, phase: &str) -> Result<Done, Stop> {
        if let Some(recipe) = job.plan().await?.get("provider_recovery").cloned() {
            let recipe = serde_json::from_value::<crate::provider_recovery::Recipe>(recipe)
                .map_err(EngineError::from)?;
            crate::provider_recovery::guard(job, &recipe).await?;
        }

        match phase {
            "schedule" => self.schedule(job).await,
            "image" => self.image(job).await,
            "volumes" => self.volumes(job).await,
            "tools" => self.tools(job).await,
            "create" => self.create(job).await,
            "start" => self.start(job).await,
            "enroll" => self.enroll(job).await,
            "after_up" => super::after_up::run(job).await,
            "verify" => self.verify(job).await,
            "commit" => self.commit(job).await,
            "refresh" => self.refresh(job).await,
            other => Err(failed(
                INTERNAL,
                format!("a provider's provisioning has no phase {other}"),
            )),
        }
    }

    async fn undo(&self, job: &Job, why: &Stop) -> Undone {
        if super::after_up::record_interrupted(job).await.is_err() {
            tracing::warn!("an interrupted provider command outcome could not be recorded");
        }
        if job
            .plan()
            .await
            .ok()
            .is_some_and(|plan| plan.get("after_up_started").and_then(Json::as_bool) == Some(true))
        {
            let undone = super::after_up::stop_failed(job, why).await;
            self.forget_sign_in(job, why).await;
            return undone;
        }
        match self.mode {
            Mode::Make => self.undo_make(job, why).await,
            Mode::Start => self.undo_start(job, why).await,
            Mode::Renew => Undone::Partly(
                "the runtime is kept; stopping or recreating it needs a manager's confirmation"
                    .to_owned(),
            ),
        }
    }

    async fn result(&self, job: &Job) -> Option<Json> {
        let plan = job.plan().await.ok()?;
        Some(json!({
            "provider": self.provider,
            "runtime": plan.get("runtime"),
            "host": plan.get("host"),
            "warnings": plan.get("warnings"),
        }))
    }
}

// ---------------------------------------------------------------- taking it away

/// Removes a retired provider unit and, only when requested, its unshared owned volumes.
#[derive(Debug, Clone)]
pub struct ProviderRemoval {
    engine: Engine,
    provider: Id,
    runtime: Id,
    volumes: bool,
}

impl ProviderRemoval {
    /// The work of a job row of the kind `environment_remove` that is about a provider's runtime.
    #[must_use]
    pub fn of(engine: Engine, row: &JobRow) -> Self {
        Self {
            engine,
            provider: row.provider_id.unwrap_or_else(Id::nil),
            runtime: row
                .plan
                .get("runtime")
                .and_then(Json::as_str)
                .and_then(|id| id.parse().ok())
                .unwrap_or_else(Id::nil),
            volumes: row
                .plan
                .get("volumes")
                .and_then(Json::as_bool)
                .unwrap_or(true),
        }
    }

    async fn target(&self) -> Result<Option<(RuntimeFull, Id)>, Stop> {
        let mut conn = self
            .engine
            .inner
            .db
            .acquire()
            .await
            .map_err(EngineError::from)?;
        let Some(runtime) = env_rows::runtime(&mut conn, self.runtime)
            .await?
            .filter(|runtime| runtime.state != runtime_state::REMOVED)
        else {
            return Ok(None);
        };
        Ok(host_rows::host(&mut conn, runtime.host_id)
            .await?
            .map(|host| (runtime, host.machine_id)))
    }

    async fn ask(&self, job: &Job, command: Outgoing) -> Result<(), Stop> {
        match job.command(command).await {
            Ok(_) => Ok(()),
            Err(Stop::Failed(failure)) if failure.code == "not_found" => Ok(()),
            Err(other) => Err(other),
        }
    }
}

#[async_trait]
impl Phased for ProviderRemoval {
    async fn run(&self, job: &Job, phase: &str) -> Result<Done, Stop> {
        let failure = |error: CommandsError| failed(INTERNAL, error.to_string());
        match phase {
            "stop" => {
                let Some((runtime, machine)) = self.target().await? else {
                    return Ok(Done::Skipped("it has no unit".to_owned()));
                };
                // A unit that was retired may be running still: it is stopped before it is removed.
                if !matches!(
                    runtime.state.as_str(),
                    runtime_state::RUNNING
                        | runtime_state::STOPPING
                        | runtime_state::CREATING
                        | runtime_state::RETIRED
                ) {
                    return Ok(Done::Skipped("its unit is not running".to_owned()));
                }
                let command = Outgoing::new(
                    ids::derived(
                        "node.runtime.stop",
                        &[job.id().as_bytes().as_slice(), b"removal"],
                    ),
                    machine,
                    SerialKey::Runtime(runtime.id),
                    &NodeRuntimeStop {
                        runtime_id: runtime.id,
                        reason: Some(orch_driver_contract::runtime::UnitReason::Delete),
                    },
                )
                .map_err(failure)?;
                self.ask(job, command).await?;
                Ok(Done::Finished)
            }
            "unit" => {
                let Some((runtime, machine)) = self.target().await? else {
                    return Ok(Done::Skipped("it has no unit".to_owned()));
                };
                let command = Outgoing::new(
                    ids::derived(
                        "node.runtime.remove",
                        &[job.id().as_bytes().as_slice(), b"removal"],
                    ),
                    machine,
                    SerialKey::Runtime(runtime.id),
                    &NodeRuntimeRemove {
                        runtime_id: runtime.id,
                        keep_volumes: true,
                        volume_refs: Vec::new(),
                    },
                )
                .map_err(failure)?;
                self.ask(job, command).await?;
                Ok(Done::Finished)
            }
            "volumes" => {
                if !self.volumes {
                    return Ok(Done::Skipped(
                        "the host manager kept the volumes".to_owned(),
                    ));
                }
                let Some((runtime, machine)) = self.target().await? else {
                    return Ok(Done::Skipped("it has no volume".to_owned()));
                };
                for (role, volume) in runtime.volumes.as_object().into_iter().flatten() {
                    if volume
                        .get("directory")
                        .is_some_and(|directory| !directory.is_null())
                    {
                        continue;
                    }
                    let Some(name) = volume["name"].as_str() else {
                        continue;
                    };
                    let in_use = {
                        let mut conn = self
                            .engine
                            .inner
                            .db
                            .acquire()
                            .await
                            .map_err(EngineError::from)?;
                        env_rows::volume_in_use(&mut conn, runtime.host_id, name, runtime.id)
                            .await?
                    };
                    if in_use {
                        continue;
                    }
                    let command = Outgoing::new(
                        ids::derived(
                            "node.volume.remove",
                            &[job.id().as_bytes().as_slice(), role.as_bytes()],
                        ),
                        machine,
                        SerialKey::Volume(name.to_owned()),
                        &NodeVolumeRemove {
                            driver: runtime.driver.clone(),
                            name: name.to_owned(),
                            storage: volume["storage"].as_str().map(str::to_owned),
                            labels: BTreeMap::new(),
                        },
                    )
                    .map_err(failure)?;
                    self.ask(job, command).await?;
                }
                Ok(Done::Finished)
            }
            "forget" => {
                job.pass_commit().await?;
                let target = self.target().await?;
                let now = job.now();
                let mut tx = self
                    .engine
                    .inner
                    .db
                    .begin()
                    .await
                    .map_err(EngineError::from)?;
                if let Some((runtime, _)) = target {
                    env_rows::set_runtime_state(
                        &mut tx,
                        runtime.id,
                        &[
                            runtime_state::CREATING,
                            runtime_state::RUNNING,
                            runtime_state::STOPPING,
                            runtime_state::STOPPED,
                            runtime_state::FAILED,
                            runtime_state::RETIRED,
                        ],
                        runtime_state::REMOVED,
                        None,
                        now,
                    )
                    .await?;
                    host_rows::revoke_machine_of(&mut tx, runtime.id, now).await?;
                    host_rows::delete_runtime_secret(&mut tx, runtime.id, "enroll_token").await?;
                }
                tx.commit().await.map_err(DbError::from)?;
                Ok(Done::Finished)
            }
            other => Err(failed(
                INTERNAL,
                format!("the removal of a provider's runtime has no phase {other}"),
            )),
        }
    }

    /// A removal is not undone: what it took away stays away. What a failure does is date the next
    /// attempt, after a pause that grows, and say why on the runtime.
    async fn undo(&self, job: &Job, why: &Stop) -> Undone {
        let Stop::Failed(failure) = why else {
            return Undone::Everything;
        };
        let now = job.now();
        let config = self.engine.inner.config.hosting;
        let Ok(mut conn) = self.engine.inner.db.acquire().await else {
            return Undone::Partly("the database could not be reached".to_owned());
        };
        let failures = provider_jobs::failed_removals(&mut conn, self.provider, self.runtime)
            .await
            .unwrap_or(0)
            + 1;
        let pause = crate::jobs::remove::pause_after(
            failures,
            config.removal_first_pause,
            config.reaper_retry_max,
        );
        let again = now + chrono::Duration::from_std(pause).unwrap_or_default();
        let detail = format!(
            "removing it failed ({} times): {}; it is tried again at {}",
            failures,
            failure.message,
            again.format("%Y-%m-%dT%H:%M:%SZ")
        );
        let _ = env_rows::postpone_removal(&mut conn, self.runtime, &detail, again, now).await;
        Undone::Everything
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_phases_of_a_providers_runtime_have_no_attach_and_a_renewal_looks_at_the_sign_in_last() {
        let names = |mode| {
            phases(mode)
                .iter()
                .map(|phase| phase.name)
                .collect::<Vec<_>>()
        };
        assert_eq!(
            names(Mode::Make),
            [
                "schedule", "image", "volumes", "tools", "create", "enroll", "after_up", "verify",
                "commit"
            ]
        );
        assert_eq!(
            names(Mode::Start),
            ["start", "enroll", "after_up", "verify", "commit"]
        );
        assert_eq!(
            names(Mode::Renew),
            ["start", "enroll", "after_up", "verify", "commit", "refresh"],
            "the harness looks at its sign-in once the unit is up, which cannot undo the unit"
        );
        assert_eq!(
            removal_phases()
                .iter()
                .map(|phase| phase.name)
                .collect::<Vec<_>>(),
            ["stop", "unit", "volumes", "forget"]
        );
    }

    #[test]
    fn the_unit_of_a_providers_runtime_is_named_after_the_last_digits_of_its_identifier() {
        let first = new_id();
        let second = new_id();
        assert_ne!(
            unit_name(&first),
            unit_name(&second),
            "the last digits differ between two runtimes made in one millisecond"
        );
        let name = unit_name(&first);
        assert!(name.starts_with("orch-provider-"), "{name}");
        assert!(
            first
                .simple()
                .to_string()
                .ends_with(&name["orch-provider-".len()..]),
            "{name}"
        );
        assert_eq!(state_volume_name(&first), format!("orch-state-{first}"));
    }

    #[test]
    fn the_body_of_the_unit_is_a_providers_with_its_roles_one_volume_and_no_mount() {
        let provider = new_id();
        let runtime = new_id();
        let enrollment = Enrollment {
            secret: new_id(),
            gateway_url: "wss://gateway.orch.internal:9443".to_owned(),
            ca_cert: "-----BEGIN CERTIFICATE-----".to_owned(),
            system: "acme".to_owned(),
        };
        let volumes = json!({ "state": { "name": state_volume_name(&provider), "storage": null } });
        let spec = provider_runtime_spec(
            provider,
            &SpecInputs {
                runtime,
                name: &unit_name(&runtime),
                driver: "docker",
                spec: &json!({ "image": "busybox", "colocated_harnesses": ["codex"], "env":{"REGION":"east","SERVICE_TOKEN":{"$secret":enrollment.secret}} }),
                limits: &json!({ "memory_mb": 2048 }),
                enrollment: &enrollment,
                tools: &[],
                volumes: &volumes,
            },
        );
        assert_eq!(spec["kind"], "provider");
        assert_eq!(spec["roles"], json!(["provider", "agent"]));
        assert_eq!(spec["mounts"], json!([]));
        assert_eq!(spec["env"]["REGION"], "east");
        assert_eq!(
            spec["env"]["SERVICE_TOKEN"],
            json!({"$secret":enrollment.secret})
        );
        assert_eq!(spec["labels"]["orch.kind"], "provider");
        assert_eq!(spec["labels"]["orch.provider_id"], json!(provider));
        assert_eq!(spec["volumes"], volumes);
        assert_eq!(
            spec["enroll_token"],
            json!({ "$secret": enrollment.secret }),
            "the token is a reference and is never held by the body"
        );
    }

    fn state(kind: &str, harness: Option<&str>) -> State {
        State {
            id: new_id(),
            name: "p".to_owned(),
            owner_id: new_id(),
            org_id: new_id(),
            home_id: new_id(),
            kind: kind.to_owned(),
            protocols: if kind == "api" {
                vec!["openai-responses".into()]
            } else {
                Vec::new()
            },
            harness_kind: harness.map(str::to_owned),
            agent_id: harness.map(|_| new_id()),
            placement: "managed".to_owned(),
            machine_id: None,
            key_machine_id: None,
            status: "needs_setup".to_owned(),
            status_detail: None,
            runtime_config: json!({}),
            config: json!({}),
            login_state: None,
            login_pending: None,
            login_pending_at: None,
            held_by_environment_id: None,
            sources: json!({}),
            facts: json!({}),
            deleted_at: None,
            revision: 1,
        }
    }

    #[test]
    fn a_subscriptions_runtime_is_given_its_harness_and_a_keys_every_harness_the_system_has() {
        let profiled = vec!["claude-code".to_owned(), "codex".to_owned()];
        assert_eq!(
            harnesses_wanted(&state("subscription", Some("codex")), &profiled),
            ["codex"]
        );
        assert_eq!(
            harnesses_wanted(&state("api", None), &profiled),
            ["claude-code", "codex"]
        );
        let mut sign_in = state("subscription", Some("codex"));
        sign_in.protocols = vec!["openai-responses".into()];
        assert_eq!(
            harnesses_wanted(&sign_in, &profiled),
            ["codex", "claude-code"],
            "forwarded access needs profiled harnesses regardless of the credential's kind"
        );
    }

    fn candidate(
        name: &str,
        free: i64,
        full: bool,
        blocks: bool,
        lacks: &[&'static str],
    ) -> Candidate {
        Candidate {
            host: HostState {
                lost_at: None,
                id: new_id(),
                machine_id: new_id(),
                owner_id: None,
                org_id: None,
                home_id: None,
                name: name.to_owned(),
                labels: Vec::new(),
                drivers: json!({}),
                driver_configuration_schemas: json!({}),
                capacity: json!({}),
                load: json!({}),
                storages: json!([]),
                schedulable: true,
                disabled: false,
                when_full: None,
                online: true,
                offline_since: None,
                machine_status: "active".to_owned(),
                platform_os: "linux".to_owned(),
                platform_arch: "x86_64".to_owned(),
            },
            free_memory_mb: free,
            full,
            no_room: false,
            blocks,
            lacks: lacks.to_vec(),
        }
    }

    #[test]
    fn the_host_with_room_is_chosen_before_one_that_only_warns_and_a_named_host_is_never_replaced()
    {
        let roomy = candidate("roomy", 4_000, false, false, &[]);
        let full = candidate("full", 9_000, true, false, &[]);
        let list = vec![full.clone(), roomy.clone()];
        assert_eq!(choose(&list, None).unwrap().host.name, "roomy");
        assert_eq!(
            choose(std::slice::from_ref(&full), None).unwrap().host.name,
            "full",
            "a host that warns is still a host"
        );
        let refused = candidate("refuses", 0, true, true, &["it is full"]);
        let failure = choose(std::slice::from_ref(&refused), Some(refused.host.id)).unwrap_err();
        assert_eq!(failure.code, "no_capacity");
        assert!(failure.message.contains("refuses"), "{}", failure.message);
        let other = choose(&[roomy.clone(), refused.clone()], Some(refused.host.id)).unwrap_err();
        assert_eq!(
            other.code, "no_capacity",
            "another host is not taken instead of the one named"
        );
        let unnamed = choose(&[], None).unwrap_err();
        assert_eq!(unnamed.code, "no_host");
        let lacking = candidate(
            "elsewhere",
            1,
            false,
            false,
            &["it belongs to another member"],
        );
        assert_eq!(choose(&[lacking], None).unwrap_err().code, "no_host");
    }
}
