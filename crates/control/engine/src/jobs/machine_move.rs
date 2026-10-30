//! Directory and unit lifecycle adapters for the same reviewed environment move and data policy.

use super::runner::{Done, Failure, Job, Phased, Stop, Undone, initial_progress};
use crate::{
    Engine, EngineError,
    commands::{Outcome, Outgoing},
    unit::Unit,
    updates::{self, Target, UpdateError},
};
use async_trait::async_trait;
use orch_api_contract::environment::{
    EnvironmentMovePlace, EnvironmentMovePreflightBody, MoveMachine,
};
use orch_api_contract::forks::{
    AppliedChangeOptions, ChangeOptions, FilesHome, FilesMissing, FilesPlan, FilesPlanFileStates,
    FilesSource, MoveThreadAction, PathMap, PlanBlocker, PlanMount,
};
use orch_api_contract::resources::MachineRef;
use orch_authz::{Policy, Principal};
use orch_db::{
    Conn, DbError,
    queries::{
        environment_facts,
        environments::{changes, hosting as envs},
        jobs,
        resources::{self, hosting as hosts},
        storage, threads,
    },
};
use orch_driver_contract::runtime::EnvironmentDataSources;
use orch_link_contract::commands::{
    CommandBody, CommandKind, EnvDetach, ExecStart, FsManifest, FsManifestResult, FsMaterialize,
    FsMaterializeResult, FsSnapshot, FsSnapshotResult, MaterializePurpose, NodeRuntimeStop,
    ProcResume, ProcSuspend, PtyClose, Selection, SerialKey,
};
use orch_link_contract::source::{
    EnvironmentDataPart, FsDataExport, FsDataImport, FsDataMeasure, FsDataMeasureResult,
    FsRecordVerify, FsRecordVerifyResult,
};
use orch_settings::{Place, get};
use orch_settings_contract::registry::keys;
use orch_types::{Hash, Id, Json, Time, new_id};
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::collections::BTreeSet;
use std::time::Duration;

/// The cancellation observer uses the same quarter-second interval as other move phases.
const LOOK_EVERY: Duration = Duration::from_millis(250);
/// Seconds in the wait option's minute unit.
const SECONDS_PER_MINUTE: u64 = 60;

fn fail(code: &str, words: impl Into<String>) -> Stop {
    Failure::new(code, words).into()
}
fn blocked(code: &str, words: impl Into<String>) -> UpdateError {
    UpdateError::Blocked(vec![updates::blocker(code, words)])
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "lifecycle", rename_all = "snake_case")]
enum Destination {
    Attach {
        machine: Id,
    },
    Unit {
        machine: Id,
        host: Id,
        template: Id,
        driver: String,
        definition: Box<Json>,
        resources: Box<Json>,
        network_policy: Box<orch_driver_contract::network::NetworkPolicy>,
        host_entry: Option<Id>,
        template_entry: Option<Id>,
    },
}
impl Destination {
    fn machine(&self) -> Id {
        match self {
            Self::Attach { machine } | Self::Unit { machine, .. } => *machine,
        }
    }
    fn driver(&self) -> Option<&str> {
        match self {
            Self::Attach { .. } => None,
            Self::Unit { driver, .. } => Some(driver),
        }
    }
}

/// Frozen source identity, selected data and the destination's declared lifecycle.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Recipe {
    /// The environment keeps this identity, owner and home.
    pub environment: Id,
    /// The person who reviewed the move.
    pub by: Id,
    /// Current token authority must still hold at each effect.
    pub token: Option<Id>,
    /// The attached source connector, distinct from a unit's hosting node.
    pub source_machine: Id,
    /// The source configuration revision shown in the plan.
    pub source_revision: i64,
    /// The source generation that must remain current until commit.
    pub source_generation: i32,
    /// A fresh owned generation, including after an earlier failed attempt.
    pub next_generation: i32,
    /// The complete transfer costs and recorded-state dependencies.
    pub files: FilesPlan,
    /// Each thread held by this move, including ones whose names are hidden.
    pub threads: Vec<Id>,
    /// The person's existing change options.
    pub options: AppliedChangeOptions,
    source_status: String,
    source_facts: Json,
    source_config: Json,
    old_runtime: Option<Id>,
    destination: Destination,
    destination_fingerprint: Hash,
    selection: Selection,
    manifest: Hash,
    borrowed: bool,
    held_still: bool,
}

impl Recipe {
    /// The caller's own host/template access required by the selected destination.
    pub fn destination_access(
        &self,
    ) -> Vec<(
        orch_access_contract::decisions::ObjectRef,
        orch_access_contract::permissions::Permission,
    )> {
        use orch_access_contract::{
            decisions::ObjectRef,
            permissions::{ObjectKind, Permission},
        };
        match self.destination {
            Destination::Attach { machine } => vec![(
                ObjectRef {
                    kind: ObjectKind::Machine,
                    id: machine,
                },
                Permission::MachineUse,
            )],
            Destination::Unit { host, template, .. } => vec![
                (
                    ObjectRef {
                        kind: ObjectKind::Host,
                        id: host,
                    },
                    Permission::HostUse,
                ),
                (
                    ObjectRef {
                        kind: ObjectKind::Template,
                        id: template,
                    },
                    Permission::TemplateUse,
                ),
            ],
        }
    }

    /// Confirmation keeps the exact source, destination declaration and reserved data generation.
    #[must_use]
    pub fn same_choice(&self, other: &Self) -> bool {
        // Observation metadata and optional home estimates do not change what the person chose.
        let states = |files: &FilesPlan| {
            files.file_states.map(|mut state| {
                state.measurement = None;
                state
            })
        };
        self.environment == other.environment
            && self.source_machine == other.source_machine
            && self.source_generation == other.source_generation
            && self.source_revision == other.source_revision
            && self.next_generation == other.next_generation
            && self.destination == other.destination
            && self.destination_fingerprint == other.destination_fingerprint
            && self.files.file_count == other.files.file_count
            && self.files.total_bytes == other.files.total_bytes
            && self.files.home.is_some() == other.files.home.is_some()
            && states(&self.files) == states(&other.files)
    }

    /// Whether the source's descriptor supports holding its processes still.
    #[must_use]
    pub const fn held_still(&self) -> bool {
        self.held_still
    }

    /// The planned network follows the destination's declaration and actual requested policy.
    #[must_use]
    pub fn network(&self) -> orch_api_contract::environment::NetworkState {
        let mut state =
            orch_api_contract::environment::NetworkState::open(match self.destination {
                Destination::Attach { .. } => {
                    orch_api_contract::environment::UnitNetworkMode::Connected
                }
                Destination::Unit { .. } => orch_api_contract::environment::UnitNetworkMode::Bridge,
            });
        if let Destination::Unit { network_policy, .. } = &self.destination {
            let mode = match network_policy.mode {
                orch_driver_contract::network::EgressMode::Open => {
                    orch_api_contract::environment::EgressMode::Open
                }
                orch_driver_contract::network::EgressMode::Restricted => {
                    orch_api_contract::environment::EgressMode::Restricted
                }
            };
            state.mode = mode;
            state.required_mode = mode;
        }
        state
    }
}

/// A machine move before references and visibility are filled by the API.
pub struct Draft {
    /// The executable recipe, independent of page placement defaults.
    pub recipe: Recipe,
    /// The declared destination with current policy reasons.
    pub place: EnvironmentMovePlace,
    /// Each affected thread's actual session-transfer requirement.
    pub threads: Vec<(Id, MoveThreadAction)>,
    /// Actual retained/detached mount choices.
    pub mounts: Vec<PlanMount>,
    /// What becomes of each MCP server there.
    pub mcp: Vec<orch_api_contract::forks::PlanMcpServer>,
    /// Data ownership, policy or declared technical constraints that stop confirmation.
    pub blockers: Vec<PlanBlocker>,
}

/// Whether the move needs a directory lifecycle at either end; placement is never consulted.
pub async fn needed(
    conn: &mut Conn,
    environment: Id,
    request: &EnvironmentMovePreflightBody,
) -> Result<bool, EngineError> {
    Ok(request.machine_id.is_some()
        || environment_facts::of(conn, environment)
            .await?
            .is_some_and(|facts| facts.attaches())
        || envs::live_runtime(conn, environment).await?.is_none())
}

/// A machine reference is display data, not a source of policy or capability decisions.
pub fn machine_ref(machine: &resources::MachineRow) -> Result<MachineRef, EngineError> {
    Ok(serde_json::from_value(
        json!({"id":machine.id,"name":machine.name,"online":machine.online,
        "platform_os":machine.platform_os,"placement":machine.placement}),
    )?)
}

async fn fingerprint(
    conn: &mut Conn,
    machine: &resources::MachineRow,
    descriptor: &Json,
) -> Result<Hash, EngineError> {
    let policy = environment_facts::machine(conn, machine.id, None)
        .await?
        .and_then(|facts| facts.policy);
    // Free-space observations change as this move copies data; only the storage declaration is frozen.
    let storages:std::collections::BTreeMap<_,_>=machine.facts.get("storages").and_then(Json::as_array).into_iter().flatten()
        .filter_map(|storage|storage.get("name").and_then(Json::as_str).map(|name|
            (name,json!({"default":storage.get("default"),"private_views":storage.get("private_views")})))).collect();
    Ok(Hash::of(&serde_json::to_vec(&json!([
        descriptor, policy, storages
    ]))?))
}

async fn directory_place(
    conn: &mut Conn,
    principal: &Principal,
    state: &envs::EnvironmentState,
    machine: Id,
) -> Result<Option<(MoveMachine, Hash, Option<orch_authz::environments::Created>)>, EngineError> {
    let policies = crate::places::policies_at(conn, state.home_node_id).await?;
    let admission = orch_authz::environments::check_existing_placement(
        conn,
        principal,
        &orch_authz::environments::NewEnvironment {
            home: state.home_node_id,
            place: orch_authz::environments::Place::Machine { machine },
            mounts: Vec::new(),
        },
        &policies,
        state.id,
    )
    .await?;
    let created = match admission {
        orch_authz::environments::Creating::Allowed(created) => Some(*created),
        orch_authz::environments::Creating::Exceeds(_) => None,
        orch_authz::environments::Creating::Refused(_)
        | orch_authz::environments::Creating::NotYourMachine => return Ok(None),
    };
    let Some(row) = resources::machine(conn, machine)
        .await?
        .filter(|row| row.deleted_at.is_none() && row.runtime_id.is_none())
    else {
        return Ok(None);
    };
    let Some(descriptor) = environment_facts::on_machine(conn, machine, None).await? else {
        return Ok(None);
    };
    if descriptor.get("lifecycle").and_then(Json::as_str) != Some("attach") {
        return Ok(None);
    }
    let mut reasons = Vec::new();
    if created.is_none() {
        reasons.push("The Machine Use entry has no remaining environment capacity.".into());
    }
    if !row.online || row.status != "active" {
        reasons.push("This machine is offline or disabled.".into());
    }
    if descriptor
        .pointer("/data_storage/made_by_orch")
        .and_then(Json::as_str)
        .is_none()
    {
        reasons.push("This machine has not declared that it can make owned data; an existing directory is never replaced by a move.".into());
    }
    let reported = environment_facts::machine(conn, machine, None).await?;
    let policy = reported.as_ref().and_then(|facts| facts.policy.as_ref());
    if policy
        .and_then(|p| p.get("environment.remote_write"))
        .and_then(Json::as_bool)
        != Some(true)
    {
        reasons.push("The machine policy has remote writes off; its managers can enable environment.remote_write.".into());
    }
    if policy
        .and_then(|p| p.get("environment.roots"))
        .and_then(Json::as_array)
        .is_none_or(Vec::is_empty)
    {
        reasons.push(
            "The machine policy offers no environment.roots for the destination data.".into(),
        );
    }
    Ok(Some((
        MoveMachine {
            machine: machine_ref(&row)?,
            host: None,
            usable: reasons.is_empty(),
            reasons,
        },
        fingerprint(conn, &row, &descriptor).await?,
        created,
    )))
}

/// Both unit and directory choices use current facts and their ordinary resource Use authority.
pub async fn places(
    engine: &Engine,
    conn: &mut Conn,
    principal: &Principal,
    environment: Id,
    template: Option<Id>,
) -> Result<Vec<EnvironmentMovePlace>, UpdateError> {
    if !updates::may_manage(conn, principal, Target::Environment(environment)).await? {
        return Err(UpdateError::Denied);
    }
    let state = envs::environment(conn, environment)
        .await?
        .ok_or(UpdateError::NotFound)?;
    let physical = environment_facts::of(conn, environment)
        .await?
        .and_then(|facts| facts.machine_id);
    let template = template.or(state.template_id).or(get::<Id>(
        conn,
        Place::Node(state.home_node_id),
        keys::ENV_TEMPLATE_ID,
    )
    .await
    .map_err(EngineError::from)?);
    let mut found = Vec::new();
    if let Some(template) = template
        && let Some(template) = hosts::template(conn, template).await?
    {
        let mut own = state.config.clone();
        own[keys::ENV_DATA_SOURCES] = json!(EnvironmentDataSources::default());
        own[keys::ENV_STORAGE] = json!({});
        let mut wanted = crate::places::wanted(conn, state.home_node_id, template, &own).await?;
        wanted.mounts.clear();
        for place in crate::places::host_places_for_environment(
            conn,
            principal,
            &crate::places::Ask {
                home: state.home_node_id,
                wanted: &wanted,
                current: None,
                minutes_idle: crate::places::IDLE_MINUTES_DEFAULT,
                now: engine.inner.clock.now(),
            },
            state.id,
        )
        .await?
        {
            if Some(place.host.machine_id) != physical {
                found.push(EnvironmentMovePlace::Host(Box::new(place.place)));
            }
        }
    }
    let machines: Vec<Id> = sqlx::query_scalar(
        "SELECT id FROM machines WHERE deleted_at IS NULL AND runtime_id IS NULL ORDER BY name,id",
    )
    .fetch_all(&mut *conn)
    .await
    .map_err(DbError::from)?;
    for machine in machines {
        if Some(machine) == physical {
            continue;
        }
        if let Some((place, _, _)) = directory_place(conn, principal, &state, machine).await? {
            found.push(EnvironmentMovePlace::Machine(place));
        }
    }
    Ok(found)
}

async fn destination(
    engine: &Engine,
    conn: &mut Conn,
    principal: &Principal,
    state: &envs::EnvironmentState,
    request: &EnvironmentMovePreflightBody,
) -> Result<(Destination, EnvironmentMovePlace, Hash), UpdateError> {
    if request.host_id.is_some() && request.machine_id.is_some() {
        return Err(UpdateError::Invalid(
            "choose one destination machine or host".into(),
        ));
    }
    let listed = places(engine, conn, principal, state.id, request.template_id).await?;
    let selected=listed.into_iter().find(|place|match (place,request.host_id,request.machine_id) {
        (EnvironmentMovePlace::Host(place),Some(host),None)=>place.host.id==host,
        (EnvironmentMovePlace::Machine(place),None,Some(machine))=>place.machine.id==machine,
        (EnvironmentMovePlace::Host(place),None,None)=>place.usable,
        (EnvironmentMovePlace::Machine(place),None,None)=>place.usable,
        _=>false,
    }).ok_or_else(||blocked("no_place","No permitted destination offers the requested lifecycle. Choose a machine you may use, or a usable host and template."))?;
    match &selected {
        EnvironmentMovePlace::Machine(place) => {
            let (_, hash, _) = directory_place(conn, principal, state, place.machine.id)
                .await?
                .ok_or(UpdateError::Denied)?;
            Ok((
                Destination::Attach {
                    machine: place.machine.id,
                },
                selected,
                hash,
            ))
        }
        EnvironmentMovePlace::Host(place) => {
            let template = request
                .template_id
                .or(state.template_id)
                .or(
                    get::<Id>(conn, Place::Node(state.home_node_id), keys::ENV_TEMPLATE_ID)
                        .await
                        .map_err(EngineError::from)?,
                )
                .ok_or_else(|| {
                    blocked(
                        "template_required",
                        "Choose the template for the destination unit.",
                    )
                })?;
            let template = hosts::template(conn, template)
                .await?
                .ok_or(UpdateError::NotFound)?;
            let host = hosts::host(conn, place.host.id)
                .await?
                .ok_or(UpdateError::NotFound)?;
            let machine = resources::machine(conn, host.machine_id)
                .await?
                .ok_or(UpdateError::NotFound)?;
            let descriptor =
                environment_facts::on_machine(conn, machine.id, Some(&template.driver))
                    .await?
                    .ok_or_else(|| {
                        blocked(
                            "facts_unavailable",
                            "The destination has not declared this driver.",
                        )
                    })?;
            if descriptor
                .pointer("/data_storage/made_by_orch")
                .and_then(Json::as_str)
                .is_none()
            {
                return Err(blocked(
                    "data_storage_unavailable",
                    "The destination driver has not declared that it can make owned data.",
                ));
            }
            let mut own = state.config.clone();
            own[keys::ENV_DATA_SOURCES] = json!(EnvironmentDataSources::default());
            own[keys::ENV_STORAGE] = json!({});
            let mut wanted =
                crate::places::wanted(conn, state.home_node_id, template.clone(), &own).await?;
            wanted.mounts.clear();
            let allowed = crate::places::host_places_for_environment(
                conn,
                principal,
                &crate::places::Ask {
                    home: state.home_node_id,
                    wanted: &wanted,
                    current: None,
                    minutes_idle: crate::places::IDLE_MINUTES_DEFAULT,
                    now: engine.inner.clock.now(),
                },
                state.id,
            )
            .await?;
            let rights = allowed
                .iter()
                .find(|choice| choice.host.id == host.id)
                .and_then(|choice| choice.created.as_ref());
            let host_entry = rights
                .and_then(|rights| rights.host.as_ref())
                .and_then(|(entry, _)| *entry);
            let template_entry = rights
                .and_then(|rights| rights.template.as_ref())
                .and_then(|(entry, _)| *entry);
            let policy =
                crate::network::policy(conn, Place::Environment(state.id), &template.spec).await?;
            let limits = crate::places::limits_of(
                &template.spec,
                get::<Json>(conn, Place::Environment(state.id), keys::ENV_RESOURCES)
                    .await
                    .map_err(EngineError::from)?
                    .as_ref(),
            );
            Ok((
                Destination::Unit {
                    machine: machine.id,
                    host: host.id,
                    template: template.id,
                    driver: template.driver,
                    definition: Box::new(template.spec),
                    resources: Box::new(limits),
                    network_policy: Box::new(policy),
                    host_entry,
                    template_entry,
                },
                selected,
                fingerprint(conn, &machine, &descriptor).await?,
            ))
        }
    }
}

async fn measure(
    engine: &Engine,
    machine: Id,
    environment: Id,
    part: EnvironmentDataPart,
    generation: i32,
    by: Id,
) -> Result<
    (
        orch_api_contract::forks::Measurement,
        Option<FsDataMeasureResult>,
    ),
    UpdateError,
> {
    let home = part == EnvironmentDataPart::Home;
    let body = serde_json::to_value(FsDataMeasure {
        work: None,
        environment_id: environment,
        part,
    })
    .map_err(EngineError::from)?;
    let subject = json!({"environment_id":environment,"generation":generation});
    let count = if home {
        engine
            .measure_home_in_background(machine, CommandKind::FsDataMeasure, body, subject, by)
            .await?
    } else {
        engine
            .measure_in_background(machine, CommandKind::FsDataMeasure, body, subject, by)
            .await?
    };
    if count.unsupported {
        return Err(blocked(
            "home_copy_unavailable",
            "The native home cannot travel; continue from the recorded conversation",
        ));
    }
    let result = count
        .result
        .map(serde_json::from_value)
        .transpose()
        .map_err(EngineError::from)?;
    Ok((count.reference, result))
}

impl Engine {
    /// Plans the same data move when a declared directory lifecycle is involved.
    pub async fn draft_machine_move(
        &self,
        principal: &Principal,
        environment: Id,
        request: &EnvironmentMovePreflightBody,
        given: Option<&ChangeOptions>,
    ) -> Result<Draft, UpdateError> {
        let mut held = self.db().acquire().await?;
        let conn = &mut *held;
        if !updates::may_manage(conn, principal, Target::Environment(environment)).await? {
            return Err(UpdateError::Denied);
        }
        let state = envs::environment(conn, environment)
            .await?
            .ok_or(UpdateError::NotFound)?;
        if !matches!(
            state.status.as_str(),
            "running" | "stopped" | "offline" | "failed"
        ) {
            return Err(blocked(
                "environment_unavailable",
                "Wait for the environment's current operation before moving it.",
            ));
        }
        let facts = environment_facts::of(conn, environment)
            .await?
            .ok_or(UpdateError::NotFound)?;
        let workspace_root =
            crate::sessions::workspace_root(&state.workspace_root, &state.facts, state.generation)
                .to_owned();
        let source_machine = state.machine_id.ok_or_else(|| {
            blocked(
                "source_unavailable",
                "The source has no attached connector.",
            )
        })?;
        let source = resources::machine(conn, source_machine)
            .await?
            .filter(|m| m.online && m.status == "active")
            .ok_or_else(|| {
                blocked(
                    "source_unavailable",
                    "Connect the source machine before moving its recorded states.",
                )
            })?;
        let (target, place, target_hash) =
            destination(self, conn, principal, &state, request).await?;
        let mut blockers = Vec::new();
        if environment_facts::machine(conn, target.machine(), target.driver())
            .await?
            .and_then(|machine| machine.policy)
            .and_then(|policy| {
                policy
                    .get("environment.remote_write")
                    .and_then(Json::as_bool)
            })
            != Some(true)
        {
            blockers.push(updates::blocker("policy.remote_write_disabled","The destination machine policy does not allow remote writes; its managers can enable them."));
        }
        match &place {
            EnvironmentMovePlace::Machine(place) => blockers.extend(
                place
                    .reasons
                    .iter()
                    .map(|reason| updates::blocker("machine_policy", reason)),
            ),
            EnvironmentMovePlace::Host(place) if !place.usable => blockers.push(updates::blocker(
                "no_host",
                "The selected host cannot make the destination unit.",
            )),
            EnvironmentMovePlace::Host(_) => {}
        }
        let old = envs::live_runtime(conn, environment).await?;
        let mut sources =
            EnvironmentDataSources::from_config(&state.config).map_err(EngineError::from)?;
        // A legacy registered path is existing data. This follows its attach fact, never its page defaults.
        let legacy_directory = old.is_none() && state.config.get(keys::ENV_DATA_SOURCES).is_none();
        if legacy_directory {
            sources.workspace =
                orch_driver_contract::runtime::EnvironmentDataSource::ExistingDirectory {
                    path: state.workspace_root.clone(),
                };
        }
        let borrowed = sources.borrows_directories()
            || old.as_ref().is_some_and(|runtime| {
                runtime
                    .volumes
                    .as_object()
                    .into_iter()
                    .flatten()
                    .any(|(_, v)| v.get("directory").is_some_and(Json::is_object))
            });
        if borrowed && request.copy_directories != Some(true) {
            blockers.push(updates::blocker("existing_directory_on_source_machine","Existing workspace or home directories stay on their machine. Choose Copy existing directories into new storage made by Orch; the originals are kept."));
        }
        if matches!(target, Destination::Attach { .. }) {
            let descriptor = environment_facts::on_machine(conn, target.machine(), None)
                .await?
                .ok_or(UpdateError::NotFound)?;
            if old
                .as_ref()
                .is_some_and(|runtime| runtime.kept_image.is_some())
                && descriptor
                    .get("sources")
                    .and_then(Json::as_array)
                    .is_none_or(Vec::is_empty)
            {
                blockers.push(updates::blocker("unit_source_unavailable","This destination declares no unit image lifecycle and cannot run the source's kept image. Choose a machine offering that unit lifecycle."));
            }
            let required =
                crate::network::policy(conn, Place::Environment(environment), &json!({})).await?;
            if required.restricted()
                && !descriptor
                    .get("network_egress")
                    .and_then(Json::as_array)
                    .is_some_and(|modes| {
                        modes.iter().any(|mode| mode.as_str() == Some("restricted"))
                    })
            {
                blockers.push(updates::blocker("network_unavailable","The destination declares open networking only; it cannot preserve this environment's required restriction."));
            }
            for capability in &state.granted_capabilities {
                if descriptor
                    .get("capabilities")
                    .and_then(Json::as_object)
                    .is_none_or(|caps| !caps.contains_key(capability))
                    && !descriptor
                        .pointer("/live_changes/gateway_given")
                        .and_then(Json::as_array)
                        .is_some_and(|keys| keys.iter().any(|key| key.as_str() == Some(capability)))
                {
                    blockers.push(updates::blocker("capability_unavailable",format!("The destination does not declare the granted capability {capability}; choose a machine that can preserve it.")));
                }
            }
        }
        let selection = super::environment_move::move_selection(
            request.files.as_ref(),
            state.facts.get("index").and_then(Json::as_str) == Some("live"),
        )?;
        let carry_home = request.home.unwrap_or(!legacy_directory)
            || (sources.home.directory().is_some() && request.copy_directories == Some(true));
        let source_mounts = crate::mounts::views(conn, environment, self.inner.clock.now()).await?;
        let target_mounts = crate::mounts::resolve_for(
            conn,
            &crate::mounts::Target {
                environment,
                home: state.home_node_id,
                own: &state.config,
                owner: state.owner_id,
                machine: Some(target.machine()),
                driver: target.driver(),
                workspace: &state.workspace_root,
                home_path: state.facts.get("home").and_then(Json::as_str),
                storage: None,
                planned: &[],
                detached: &[],
            },
            self.inner.clock.now(),
        )
        .await?
        .into_iter()
        .map(|m| m.view)
        .collect::<Vec<_>>();
        // A directory move carries each mount as it is: its job neither detaches nor copies one.
        let mounts_from = crate::mounts::location_machine(conn, environment).await?;
        let decided = crate::mount_copies::decide(
            self,
            conn,
            &crate::mount_copies::Ask {
                choices: request.mounts.as_deref().unwrap_or_default(),
                check_fingerprints: request.check_fingerprints.unwrap_or(false).then(|| {
                    request
                        .fingerprint_request_id
                        .unwrap_or_else(orch_types::new_id)
                }),
                offer: crate::mount_copies::Offer::AsIsOnly,
                source: &source_mounts,
                target: &target_mounts,
                source_machine: mounts_from,
                target_machine: Some(target.machine()),
                principal,
                at: self.inner.clock.now(),
            },
        )
        .await?;
        blockers.extend(decided.blockers);
        let mounts = crate::mounts::plan(&source_mounts, &target_mounts, &decided.fated);
        let private_views = crate::mounts::private_plan(&source_mounts, &target_mounts, false, &[]);
        let mcp = crate::mcp::plan(
            &crate::mcp::resolve(
                conn,
                environment,
                orch_types::Time::new(self.inner.clock.now()),
            )
            .await?,
            &crate::mcp::resolve_elsewhere(
                conn,
                &crate::mcp::Elsewhere::Moved {
                    environment,
                    machine: Some(target.machine()),
                },
                orch_types::Time::new(self.inner.clock.now()),
            )
            .await?,
        );
        let ids = jobs::changes::threads(conn, Some(environment), None, false).await?;
        let mut actions = Vec::new();
        for id in &ids {
            let thread = threads::thread(conn, *id)
                .await?
                .ok_or(UpdateError::NotFound)?;
            let action = if thread.arrangement.as_deref() == Some("separated")
                && thread
                    .executor
                    .as_deref()
                    .is_none_or(|executor| executor == "orch-exec")
            {
                MoveThreadAction::Continue
            } else {
                MoveThreadAction::Reimport
            };

            actions.push((*id, action));
        }
        let latest: i64 = sqlx::query_scalar(
            "SELECT GREATEST(data_generation,generation::bigint) FROM environments WHERE id=$1",
        )
        .bind(environment)
        .fetch_one(&mut *conn)
        .await
        .map_err(DbError::from)?;
        let next_generation = i32::try_from(
            latest.max(i64::from(state.generation)).saturating_add(1),
        )
        .map_err(|_| {
            UpdateError::Invalid("the environment has exhausted its data generation numbers".into())
        })?;
        let options = updates::options(conn, Place::Environment(environment), given).await?;
        let held_still = facts
            .descriptor
            .as_ref()
            .and_then(|descriptor| descriptor.get("suspend"))
            .and_then(Json::as_bool)
            == Some(true)
            && state.facts.get("suspend").and_then(Json::as_bool) == Some(true);
        // No pool lease or transaction survives the potentially slow connector measurements.
        drop(held);
        let manifest = Outgoing::new(
            new_id(),
            source.id,
            SerialKey::Environment(environment),
            &FsManifest {
                environment_id: environment,
                selection: selection.clone(),
            },
        )
        .map_err(EngineError::from)?;
        let Some(Outcome::Succeeded(value)) = self.ask_bounded(manifest).await? else {
            return Err(blocked(
                "source_unavailable",
                "The source did not provide its selected workspace listing.",
            ));
        };
        let listed: FsManifestResult = serde_json::from_value(value).map_err(EngineError::from)?;
        let manifest = {
            let mut conn = self.db().acquire().await?;
            crate::selection::rebase_manifest(
                self,
                &mut conn,
                &listed.manifest_blob,
                &workspace_root,
            )
            .await?
        };
        let parsed = self.read_manifest(&manifest).await?;
        let (recorded_reference, recorded) = measure(
            self,
            source.id,
            environment,
            EnvironmentDataPart::FileStates,
            state.generation,
            principal.user(),
        )
        .await?;
        let counts = recorded.as_ref().and_then(|result| result.recorded);
        if recorded.is_some() && counts.is_none() {
            return Err(blocked(
                "file_states_copy_unavailable",
                "The source did not verify its recorded-state dependencies.",
            ));
        }
        if recorded.is_none() {
            blockers.push(updates::blocker(
                "measurement_pending",
                "Waiting for the file-state count.",
            ));
        }
        let home = if carry_home {
            measure(
                self,
                source.id,
                environment,
                EnvironmentDataPart::Home,
                state.generation,
                principal.user(),
            )
            .await
            .ok()
            .map(|(reference, home)| FilesHome {
                measurement: Some(reference),
                measured: home.is_some(),
                file_count: home.as_ref().map_or(0, |count| count.files),
                total_bytes: home.as_ref().map_or(0, |count| count.bytes),
            })
        } else {
            None
        };
        let current = {
            let mut conn = self.db().acquire().await?;
            envs::environment(&mut conn, environment).await?
        }
        .ok_or(UpdateError::NotFound)?;
        if current.revision != state.revision
            || current.generation != state.generation
            || current.machine_id != state.machine_id
        {
            return Err(blocked(
                "conflict.stale_revision",
                "The source changed during measurement; review a new move plan.",
            ));
        }
        let extra = recorded
            .as_ref()
            .map_or(0, |count| count.bytes)
            .saturating_add(home.as_ref().map_or(0, |home| home.total_bytes));
        let files = FilesPlan {
            source: FilesSource::Live,
            source_as_of: parsed.header().generated_at,
            file_count: listed.file_count,
            total_bytes: listed.total_bytes,
            bytes_to_upload: listed.total_bytes.saturating_add(extra),
            bytes_to_download: listed.total_bytes.saturating_add(extra),
            missing: FilesMissing {
                count: 0,
                bytes: None,
                sample: Vec::new(),
            },
            path_map: PathMap {
                from: workspace_root.clone(),
                to: workspace_root,
            },
            home,
            private_views,
            kept_image_bytes: None,
            file_states: Some(FilesPlanFileStates {
                measurement: Some(recorded_reference),
                roots: counts.map_or(0, |count| count.roots),
                objects: counts.map_or(0, |count| count.objects),
                journals: counts.map_or(0, |count| count.journals),
                total_bytes: recorded.as_ref().map_or(0, |count| count.bytes),
                complete: recorded.is_some(),
            }),
        };
        Ok(Draft {
            recipe: Recipe {
                environment,
                by: principal.user(),
                token: principal.caller().grant().and_then(|grant| grant.token()),
                source_machine,
                source_revision: state.revision,
                source_generation: state.generation,
                next_generation,
                files,
                threads: ids,
                options,
                source_status: state.status,
                source_facts: state.facts,
                source_config: state.config,
                old_runtime: old.map(|r| r.id),
                destination: target,
                destination_fingerprint: target_hash,
                selection,
                manifest,
                borrowed,
                held_still,
            },
            place,
            threads: actions,
            mounts,
            mcp,
            blockers,
        })
    }
}

/// Reserves the common environment/thread holds in the caller's transaction.
pub async fn queue(
    conn: &mut Conn,
    principal: &Principal,
    draft: Draft,
    batch: Option<Id>,
    now: chrono::DateTime<chrono::Utc>,
) -> Result<jobs::JobRow, UpdateError> {
    if !draft.blockers.is_empty() {
        return Err(UpdateError::Blocked(draft.blockers));
    }
    let mut recipe = draft.recipe;
    if principal.user() != recipe.by
        || !updates::may_manage(conn, principal, Target::Environment(recipe.environment)).await?
    {
        return Err(UpdateError::Denied);
    }
    if let Destination::Attach { machine } = &recipe.destination {
        let state = envs::environment(conn, recipe.environment)
            .await?
            .ok_or(UpdateError::NotFound)?;
        if directory_place(conn, principal, &state, *machine)
            .await?
            .and_then(|(_, _, created)| created)
            .is_none()
        {
            return Err(UpdateError::Denied);
        }
    }
    if let Destination::Unit { host, template, .. } = &recipe.destination {
        if places_for_unit(conn, principal, &recipe, *host, *template, now)
            .await?
            .is_none()
        {
            return Err(UpdateError::Denied);
        }
        for (kind, id, permission) in [
            (
                orch_access_contract::permissions::ObjectKind::Host,
                *host,
                orch_access_contract::permissions::Permission::HostUse,
            ),
            (
                orch_access_contract::permissions::ObjectKind::Template,
                *template,
                orch_access_contract::permissions::Permission::TemplateUse,
            ),
        ] {
            if !matches!(
                orch_authz::resources::check(
                    conn,
                    principal,
                    orch_access_contract::decisions::ObjectRef { kind, id },
                    permission,
                    Policy::default()
                )
                .await
                .map_err(EngineError::from)?,
                orch_access_contract::decisions::Decision::Allowed { .. }
            ) {
                return Err(UpdateError::Denied);
            }
        }
    }
    let machine = resources::machine(conn, recipe.destination.machine())
        .await?
        .ok_or(UpdateError::NotFound)?;
    let descriptor = environment_facts::on_machine(conn, machine.id, recipe.destination.driver())
        .await?
        .ok_or(UpdateError::NotFound)?;
    if !machine.online
        || machine.deleted_at.is_some()
        || fingerprint(conn, &machine, &descriptor).await? != recipe.destination_fingerprint
    {
        return Err(blocked(
            "conflict.stale_revision",
            "The destination changed while confirmation was being checked; review a new move plan.",
        ));
    }
    let current = envs::lock_environment(conn, recipe.environment)
        .await?
        .ok_or(UpdateError::NotFound)?;
    if current.generation != recipe.source_generation
        || current.revision != recipe.source_revision
        || current.machine_id != Some(recipe.source_machine)
    {
        return Err(blocked(
            "conflict.stale_revision",
            "The source changed after it was reviewed; make a new plan.",
        ));
    }
    if let Some(job) = jobs::changes::environment_hold(conn, recipe.environment).await? {
        return Err(UpdateError::Held(job.id));
    }
    let reserved: i64 = sqlx::query_scalar("SELECT data_generation FROM environments WHERE id=$1")
        .bind(recipe.environment)
        .fetch_one(&mut *conn)
        .await
        .map_err(DbError::from)?;
    if reserved >= i64::from(recipe.next_generation) {
        return Err(blocked(
            "conflict.stale_revision",
            "Another move already reserved that data generation; make a new plan.",
        ));
    }
    recipe.threads = jobs::changes::threads(conn, Some(recipe.environment), None, true).await?;
    for thread in &recipe.threads {
        if let Some(job) = jobs::changes::thread_hold(conn, *thread).await? {
            return Err(UpdateError::Held(job.id));
        }
    }
    sqlx::query("UPDATE environments SET data_generation=$2 WHERE id=$1")
        .bind(recipe.environment)
        .bind(i64::from(recipe.next_generation))
        .execute(&mut *conn)
        .await
        .map_err(DbError::from)?;
    let progress = serde_json::to_value(initial_progress(&super::environment_move::PHASES))
        .map_err(EngineError::from)?;
    let mut plan = json!({"portable":recipe,"threads":recipe.threads,"options":recipe.options,"mode":"make","fresh":true});
    if let Destination::Unit {
        definition,
        resources,
        network_policy,
        ..
    } = &recipe.destination
    {
        plan["definition"] = definition.as_ref().clone();
        plan["resolved_resources"] = resources.as_ref().clone();
        plan["network_policy"] = json!(network_policy);
    }
    let row = jobs::changes::insert(
        conn,
        &jobs::NewJob {
            id: new_id(),
            kind: jobs::kind::ENVIRONMENT_MOVE,
            node_id: None,
            environment_id: Some(recipe.environment),
            provider_id: None,
            plan: &plan,
            progress: &progress,
            created_by: Some(recipe.by),
        },
        batch,
        now,
    )
    .await?;
    envs::set_status(
        conn,
        recipe.environment,
        &["running", "stopped", "failed", "offline"],
        "moving",
        None,
        now,
    )
    .await?;
    let mut unit = Unit::new(conn, now);
    for id in &recipe.threads {
        let Some(thread) = threads::lock(&mut *unit.conn, *id).await? else {
            continue;
        };
        if threads::active_turn(&mut *unit.conn, *id).await?.is_none()
            && matches!(thread.state.as_str(), "idle" | "blocked")
        {
            let seq = unit.stamp(*id).await?;
            crate::notifications::set_state(
                &mut unit,
                *id,
                &["idle", "blocked"],
                &threads::StateChange::blocked(
                    "switching",
                    Some("The environment is moving; messages wait until its files are verified."),
                ),
                seq,
            )
            .await?;
        }
    }
    unit.finish().await?;
    Ok(row)
}

async fn places_for_unit(
    conn: &mut Conn,
    principal: &Principal,
    recipe: &Recipe,
    host: Id,
    template: Id,
    now: chrono::DateTime<chrono::Utc>,
) -> Result<Option<orch_authz::environments::Created>, EngineError> {
    let state = envs::environment(conn, recipe.environment)
        .await?
        .ok_or_else(|| EngineError::unreadable("the move's environment disappeared"))?;
    let Some(mut template) = hosts::template(conn, template).await? else {
        return Ok(None);
    };
    if let Destination::Unit {
        definition, driver, ..
    } = &recipe.destination
    {
        template.spec = definition.as_ref().clone();
        template.driver = driver.clone();
    }
    let mut own = state.config.clone();
    own[keys::ENV_DATA_SOURCES] = json!(EnvironmentDataSources::default());
    own[keys::ENV_STORAGE] = json!({});
    let mut wanted = crate::places::wanted(conn, state.home_node_id, template, &own).await?;
    wanted.mounts.clear();
    if let Destination::Unit { resources, .. } = &recipe.destination {
        wanted.limits = resources.as_ref().clone();
        wanted.needs = crate::places::needs_of(
            &wanted.template.spec,
            state.config.get(keys::ENV_NEEDS),
            Some(resources),
        );
    }
    let allocated = envs::live_runtime(conn, recipe.environment)
        .await?
        .is_some_and(|runtime| {
            runtime.host_id == host && runtime.generation == recipe.next_generation
        });
    Ok(crate::places::host_places_for_environment(
        conn,
        principal,
        &crate::places::Ask {
            home: state.home_node_id,
            wanted: &wanted,
            current: None,
            minutes_idle: crate::places::IDLE_MINUTES_DEFAULT,
            now,
        },
        state.id,
    )
    .await?
    .into_iter()
    .find(|choice| {
        choice.host.id == host
            && choice.created.is_some()
            && (choice.place.usable
                || allocated
                    && choice.place.reasons.iter().all(|reason| {
                        matches!(
                            reason,
                            orch_api_contract::environment::HostReason::Full
                                | orch_api_contract::environment::HostReason::NoRoom
                        )
                    }))
    })
    .and_then(|choice| choice.created))
}

/// Executes the existing move phases for declared directory and unit lifecycles.
pub struct Moving {
    recipe: Recipe,
}

impl Moving {
    /// Recovers the frozen candidate generation after a controller restart.
    pub fn of(row: &jobs::JobRow) -> Result<Self, EngineError> {
        Ok(Self {
            recipe: serde_json::from_value(row.plan["portable"].clone())?,
        })
    }

    async fn command<B: CommandBody>(
        &self,
        job: &Job,
        machine: Id,
        name: &str,
        body: &B,
    ) -> Result<Json, Stop> {
        let command = Outgoing::new(
            crate::ids::derived(name, &[job.id().as_bytes()]),
            machine,
            SerialKey::Environment(self.recipe.environment),
            body,
        )
        .map_err(EngineError::from)?;
        job.command(command).await
    }

    async fn guard(&self, job: &Job) -> Result<(), Stop> {
        job.check()?;
        let mut tx = job.engine().db().begin().await?;
        orch_db::queries::transaction_guard::lock_authority(&mut tx).await?;
        let principal =
            super::source::load_principal(&mut tx, job, self.recipe.by, self.recipe.token).await?;
        if !updates::may_manage(
            &mut tx,
            &principal,
            Target::Environment(self.recipe.environment),
        )
        .await
        .map_err(|error| fail("access.denied", error.to_string()))?
        {
            return Err(fail(
                "access.denied",
                "Current authority no longer permits the environment move.",
            ));
        }
        let current = envs::environment(&mut tx, self.recipe.environment)
            .await?
            .ok_or_else(|| fail("environment_gone", "The environment is gone."))?;
        if current.generation != self.recipe.source_generation
            || current.revision != self.recipe.source_revision
            || current.machine_id != Some(self.recipe.source_machine)
        {
            return Err(fail(
                "conflict.stale_revision",
                "The source changed after this move was accepted.",
            ));
        }
        let machine = resources::machine(&mut tx, self.recipe.destination.machine())
            .await?
            .filter(|machine| {
                machine.online && machine.status == "active" && machine.deleted_at.is_none()
            })
            .ok_or_else(|| {
                fail(
                    "machine_unavailable",
                    "The destination machine is offline, disabled or removed.",
                )
            })?;
        let descriptor =
            environment_facts::on_machine(&mut tx, machine.id, self.recipe.destination.driver())
                .await?
                .ok_or_else(|| {
                    fail(
                        "facts_unavailable",
                        "The destination no longer declares the selected lifecycle.",
                    )
                })?;
        if fingerprint(&mut tx, &machine, &descriptor).await? != self.recipe.destination_fingerprint
        {
            return Err(fail(
                "conflict.stale_revision",
                "The destination's policy, services or storages changed; review the move again.",
            ));
        }
        if machine
            .facts
            .pointer("/policy/environment.remote_write")
            .and_then(Json::as_bool)
            != Some(true)
        {
            return Err(fail(
                "policy.remote_write_disabled",
                "The destination machine policy does not allow remote writes.",
            ));
        }
        match &self.recipe.destination {
            Destination::Attach { .. } => {
                if directory_place(&mut tx, &principal, &current, machine.id)
                    .await?
                    .and_then(|(_, _, created)| created)
                    .is_none()
                {
                    return Err(fail(
                        "access.denied",
                        "The destination no longer admits this environment through Machine Use.",
                    ));
                }
            }
            Destination::Unit { host, template, .. } => {
                if !places_for_unit(
                    &mut tx,
                    &principal,
                    &self.recipe,
                    *host,
                    *template,
                    job.now(),
                )
                .await?
                .is_some()
                {
                    return Err(fail(
                        "access.denied",
                        "The destination no longer satisfies the caller's host/template use limits.",
                    ));
                }
                for (kind, id, permission) in [
                    (
                        orch_access_contract::permissions::ObjectKind::Host,
                        *host,
                        orch_access_contract::permissions::Permission::HostUse,
                    ),
                    (
                        orch_access_contract::permissions::ObjectKind::Template,
                        *template,
                        orch_access_contract::permissions::Permission::TemplateUse,
                    ),
                ] {
                    if !matches!(
                        orch_authz::resources::check(
                            &mut tx,
                            &principal,
                            orch_access_contract::decisions::ObjectRef { kind, id },
                            permission,
                            Policy::default()
                        )
                        .await
                        .map_err(EngineError::from)?,
                        orch_access_contract::decisions::Decision::Allowed { .. }
                    ) {
                        return Err(fail(
                            "access.denied",
                            "The caller can no longer use the destination host and template.",
                        ));
                    }
                }
            }
        }
        tx.commit().await.map_err(DbError::from)?;
        Ok(())
    }

    async fn lock(&self, job: &Job) -> Result<Done, Stop> {
        self.guard(job).await?;
        for thread in &self.recipe.threads {
            job.engine().cancel_pending_approvals(*thread).await?;
        }
        let seconds = u64::try_from(self.recipe.options.wait_minutes.max(0))
            .unwrap_or(0)
            .saturating_mul(SECONDS_PER_MINUTE);
        let allowed =
            Duration::from_secs(seconds).saturating_add(job.engine().inner.config.interrupt_force);
        let waited = tokio::time::timeout(allowed, async {
            loop {
                job.check()?;
                let mut active = Vec::new();
                let mut conn = job.engine().db().acquire().await?;
                for thread in &self.recipe.threads {
                    if let Some(turn) = threads::active_turn(&mut conn, *thread).await? {
                        active.push((*thread, turn));
                    }
                }
                drop(conn);
                if active.is_empty() {
                    return Ok::<(), Stop>(());
                }
                if self.recipe.options.active_turn
                    == orch_api_contract::forks::ActiveTurn::Interrupt
                {
                    for (thread, turn) in active {
                        job.engine()
                            .interrupt_for_change(thread, &turn, self.recipe.by)
                            .await?;
                    }
                }
                job.pause(LOOK_EVERY).await?;
            }
        })
        .await;
        waited.map_err(|_|fail("thread.not_idle","An active turn did not finish before this move's bounded wait ended; no data was changed."))??;
        job.engine()
            .return_credentials_of(self.recipe.environment)
            .await
            .map_err(|error| fail("credential.return_failed", error.to_string()))?;
        let shells = {
            let mut conn = job.engine().db().acquire().await?;
            changes::open_shells(&mut conn, self.recipe.environment).await?
        };
        for terminal_id in shells {
            let command = Outgoing::new(
                crate::ids::derived(
                    "machine-move.pty.close",
                    &[job.id().as_bytes(), terminal_id.as_bytes()],
                ),
                self.recipe.source_machine,
                SerialKey::Environment(self.recipe.environment),
                &PtyClose { terminal_id },
            )
            .map_err(EngineError::from)?;
            job.command(command).await?;
        }
        if self.recipe.held_still
            && job
                .plan()
                .await?
                .get("source_suspended")
                .and_then(Json::as_bool)
                != Some(true)
        {
            self.command(
                job,
                self.recipe.source_machine,
                "machine-move.suspend",
                &ProcSuspend {
                    environment_id: self.recipe.environment,
                },
            )
            .await?;
            job.remember(&json!({"source_suspended":true})).await?;
        }
        Ok(Done::Finished)
    }

    async fn provision(&self, job: &Job) -> Result<Done, Stop> {
        self.guard(job).await?;
        match &self.recipe.destination {
            Destination::Attach { machine } => {
                let mut conn = job.engine().db().acquire().await?;
                let mut with =
                    crate::environments::attach_with(&mut conn, self.recipe.environment, job.now())
                        .await?;
                with.data_sources = Some(EnvironmentDataSources::default());
                with.data_generation = Some(i64::from(self.recipe.next_generation));
                with.storage = None;
                let state = envs::environment(&mut conn, self.recipe.environment)
                    .await?
                    .ok_or_else(|| fail("environment_gone", "The environment is gone."))?;
                with.mounts = crate::mounts::resolve_for(
                    &mut conn,
                    &crate::mounts::Target {
                        environment: state.id,
                        home: state.home_node_id,
                        own: &state.config,
                        owner: state.owner_id,
                        machine: Some(*machine),
                        driver: None,
                        workspace: &state.workspace_root,
                        home_path: None,
                        storage: None,
                        planned: &[],
                        detached: &[],
                    },
                    job.now(),
                )
                .await?
                .into_iter()
                .filter_map(|mount| mount.attach)
                .collect();
                drop(conn);
                let mut body =
                    crate::environments::attach_body(self.recipe.environment, None, &with);
                body.mounts = Some(with.mounts.clone());
                let facts = self
                    .command(job, *machine, "machine-move.attach", &body)
                    .await?;
                job.remember(&json!({"candidate_machine":machine,"candidate_facts":facts}))
                    .await?;
                for executor in &with.executors {
                    let command = Outgoing::new(
                        crate::ids::derived(
                            "machine-move.exec",
                            &[job.id().as_bytes(), executor.as_bytes()],
                        ),
                        *machine,
                        SerialKey::Environment(self.recipe.environment),
                        &ExecStart {
                            environment_id: self.recipe.environment,
                            executor: executor.clone(),
                        },
                    )
                    .map_err(EngineError::from)?;
                    job.command(command).await?;
                }
            }
            Destination::Unit {
                host,
                template,
                driver,
                definition,
                ..
            } => {
                if job.plan().await?.get("runtime").is_none() {
                    let mut tx = job.engine().db().begin().await?;
                    let id = new_id();
                    let volumes = json!({"workspace":{"name":format!("orch-ws-{}-{id}",self.recipe.environment),"storage":null},
                        "home":{"name":format!("orch-home-{}-{id}",self.recipe.environment),"storage":null},
                        "state":{"name":format!("orch-state-{}-{id}",self.recipe.environment),"storage":null}});
                    let hash =
                        Hash::of(&serde_json::to_vec(definition).map_err(EngineError::from)?);
                    envs::insert_runtime(
                        &mut tx,
                        &envs::NewRuntime {
                            id,
                            host_id: *host,
                            unit_name: &super::provision::unit_name(&id),
                            environment_id: self.recipe.environment,
                            driver,
                            template_id: *template,
                            spec_hash: hash.as_bytes(),
                            generation: self.recipe.next_generation,
                            volumes: &volumes,
                        },
                        job.now(),
                    )
                    .await?;
                    jobs::merge_plan(
                        &mut tx,
                        job.id(),
                        &json!({"runtime":id,"host":host}),
                        job.now(),
                    )
                    .await?;
                    tx.commit().await.map_err(DbError::from)?;
                }
                for phase in ["image", "volumes", "tools", "create", "enroll", "attach"] {
                    let row = job.row().await?;
                    super::provision::Provision::of(job.engine().clone(), &row)
                        .run(job, phase)
                        .await?;
                }
                let plan = job.plan().await?;
                let runtime: Id =
                    serde_json::from_value(plan["runtime"].clone()).map_err(EngineError::from)?;
                let mut conn = job.engine().db().acquire().await?;
                let (machine, _) = hosts::machine_of_runtime(&mut conn, runtime)
                    .await?
                    .ok_or_else(|| fail("unhealthy", "The destination unit has no connector."))?;
                drop(conn);
                job.remember(&json!({"candidate_machine":machine})).await?;
            }
        }
        Ok(Done::Finished)
    }

    async fn exported(
        &self,
        job: &Job,
        name: &str,
        part: Option<EnvironmentDataPart>,
    ) -> Result<FsSnapshotResult, Stop> {
        if let Some(saved) = job
            .plan()
            .await?
            .get("move_exports")
            .and_then(|exports| exports.get(name))
            .cloned()
        {
            return serde_json::from_value(saved)
                .map_err(EngineError::from)
                .map_err(Into::into);
        }
        let value = match part {
            Some(part) => {
                self.command(
                    job,
                    self.recipe.source_machine,
                    &format!("machine-move.export.{name}"),
                    &FsDataExport {
                        work: None,
                        environment_id: self.recipe.environment,
                        job_id: job.id(),
                        part,
                    },
                )
                .await?
            }
            None => {
                self.command(
                    job,
                    self.recipe.source_machine,
                    "machine-move.workspace.snapshot",
                    &FsSnapshot {
                        work: None,
                        environment_id: self.recipe.environment,
                        job_id: job.id(),
                        selection: self.recipe.selection.clone(),
                    },
                )
                .await?
            }
        };
        let mut saved: FsSnapshotResult =
            serde_json::from_value(value).map_err(EngineError::from)?;
        if saved.missing != 0 {
            return Err(fail(
                "files_missing",
                "The source could not supply every promised file; it is kept.",
            ));
        }
        let mut tx = job.engine().db().begin().await?;
        if part.is_none() {
            saved.manifest_blob = crate::selection::rebase_manifest(
                job.engine(),
                &mut tx,
                &saved.manifest_blob,
                &self.recipe.files.path_map.from,
            )
            .await?;
        }
        let parsed = job.engine().read_manifest(&saved.manifest_blob).await?;
        let mut hashes: Vec<_> = parsed.hashes().collect();
        hashes.push(saved.manifest_blob);
        hashes.sort();
        hashes.dedup();
        let current = jobs::lock(&mut tx, job.id())
            .await?
            .ok_or_else(|| fail("job_gone", "The move disappeared."))?;
        let mut exports = current
            .plan
            .get("move_exports")
            .cloned()
            .unwrap_or_else(|| json!({}));
        if exports.get(name).is_none() {
            let mut kept: BTreeSet<Hash> = current
                .plan
                .get("move_pins")
                .cloned()
                .map(serde_json::from_value)
                .transpose()
                .map_err(EngineError::from)?
                .unwrap_or_default();
            let added: Vec<_> = hashes
                .into_iter()
                .filter(|hash| kept.insert(*hash))
                .collect();
            storage::pin_blobs(&mut tx, &added).await?;
            exports[name] = json!(saved);
            jobs::merge_plan(
                &mut tx,
                job.id(),
                &json!({"move_exports":exports,"move_pins":kept}),
                job.now(),
            )
            .await?;
        }
        tx.commit().await.map_err(DbError::from)?;
        Ok(saved)
    }

    async fn copy(&self, job: &Job) -> Result<Done, Stop> {
        self.guard(job).await?;
        let plan = job.plan().await?;
        let machine: Id =
            serde_json::from_value(plan["candidate_machine"].clone()).map_err(EngineError::from)?;
        let root = plan
            .pointer("/candidate_facts/workspace_root")
            .and_then(Json::as_str)
            .ok_or_else(|| fail("unhealthy", "The candidate has no workspace root."))?;
        let workspace = self.exported(job, "workspace", None).await?;
        let value = self
            .command(
                job,
                machine,
                "machine-move.workspace.materialize",
                &FsMaterialize {
                    work: None,
                    environment_id: self.recipe.environment,
                    job_id: Some(job.id()),
                    manifest_blob: workspace.manifest_blob,
                    target_root: root.into(),
                    purpose: MaterializePurpose::Move,
                },
            )
            .await?;
        let copied: FsMaterializeResult =
            serde_json::from_value(value).map_err(EngineError::from)?;
        if copied.files.saturating_add(copied.skipped) < workspace.file_count {
            return Err(fail(
                "files_missing",
                "The candidate did not report every promised workspace file.",
            ));
        }
        self.command(
            job,
            machine,
            "machine-move.worktrees.repair",
            &orch_link_contract::source::GitWorktreeRepair {
                environment_id: self.recipe.environment,
                job_id: job.id(),
                manifest_blob: workspace.manifest_blob,
                previous_root: self.recipe.files.path_map.from.clone(),
                target_root: root.into(),
            },
        )
        .await?;
        for (name, part) in [
            ("home", EnvironmentDataPart::Home),
            ("file_states", EnvironmentDataPart::FileStates),
        ] {
            if part == EnvironmentDataPart::Home && self.recipe.files.home.is_none() {
                continue;
            }
            let exported = self.exported(job, name, Some(part)).await?;
            let imported = self
                .command(
                    job,
                    machine,
                    &format!("machine-move.import.{name}"),
                    &FsDataImport {
                        work: None,
                        environment_id: self.recipe.environment,
                        job_id: job.id(),
                        part,
                        manifest_blob: exported.manifest_blob,
                    },
                )
                .await?;
            let imported: FsMaterializeResult =
                serde_json::from_value(imported).map_err(EngineError::from)?;
            if imported.files.saturating_add(imported.skipped) < exported.file_count {
                return Err(fail(
                    "data_missing",
                    "The candidate did not report every promised data file.",
                ));
            }
        }
        job.remember(&json!({"copied":true,"target_root":root}))
            .await?;
        Ok(Done::Finished)
    }

    async fn verify(&self, job: &Job) -> Result<Done, Stop> {
        self.guard(job).await?;
        let plan = job.plan().await?;
        if plan.get("copied").and_then(Json::as_bool) != Some(true) {
            return Err(fail("data_missing", "No complete data copy was reported."));
        }
        let machine: Id =
            serde_json::from_value(plan["candidate_machine"].clone()).map_err(EngineError::from)?;
        let value = self
            .command(
                job,
                machine,
                "machine-move.recorded.verify",
                &FsRecordVerify {
                    environment_id: self.recipe.environment,
                },
            )
            .await?;
        let actual: FsRecordVerifyResult =
            serde_json::from_value(value).map_err(EngineError::from)?;
        let expected = self.recipe.files.file_states.ok_or_else(|| {
            fail(
                "file_states_copy_unavailable",
                "The plan names no complete recorded-state store.",
            )
        })?;
        if actual.roots < expected.roots
            || actual.objects < expected.objects
            || actual.journals < expected.journals
        {
            return Err(fail(
                "file_states_copy_unavailable",
                "The candidate did not verify every planned recorded-state dependency.",
            ));
        }
        let mut conn = job.engine().db().acquire().await?;
        let online = resources::machine(&mut conn, machine)
            .await?
            .is_some_and(|row| row.online && row.status == "active");
        let wrong = super::health::findings(&super::health::Side {
            online,
            facts: &plan["candidate_facts"],
            runtime_state: "running",
        });
        if !wrong.is_empty() {
            return Err(fail("unhealthy", wrong.join("; ")));
        }
        if let Destination::Unit { network_policy, .. } = &self.recipe.destination
            && network_policy.restricted()
        {
            let runtime: Id =
                serde_json::from_value(plan["runtime"].clone()).map_err(EngineError::from)?;
            let networks = orch_db::queries::network::of_runtimes(&mut conn, &[runtime]).await?;
            if !networks.iter().any(|row| {
                row.network_state.as_ref().is_some_and(|state| {
                    state.get("mode").and_then(Json::as_str) == Some("restricted")
                        && state.get("state").and_then(Json::as_str) == Some("ready")
                })
            }) {
                return Err(fail(
                    "network_unverified",
                    "The destination has not verified the required network restriction.",
                ));
            }
        }
        drop(conn);
        job.remember(&json!({"file_states_content_verified":true}))
            .await?;
        Ok(Done::Finished)
    }

    async fn commit(&self, job: &Job) -> Result<Done, Stop> {
        if job.plan().await?.get("committed").and_then(Json::as_bool) == Some(true) {
            return Ok(Done::Finished);
        }
        self.guard(job).await?;
        let mut tx = job.engine().db().begin().await?;
        orch_db::queries::transaction_guard::lock_authority(&mut tx).await?;
        let row = jobs::lock(&mut tx, job.id())
            .await?
            .ok_or_else(|| fail("job_gone", "The move disappeared."))?;
        let principal =
            super::source::load_principal(&mut tx, job, self.recipe.by, self.recipe.token).await?;
        if !updates::may_manage(
            &mut tx,
            &principal,
            Target::Environment(self.recipe.environment),
        )
        .await
        .map_err(|error| fail("access.denied", error.to_string()))?
        {
            return Err(fail(
                "access.denied",
                "The caller no longer manages this environment.",
            ));
        }
        // Current placement authority and capacity are rechecked together below,
        // after this exact environment row is locked and its generation matched.
        let destination = resources::machine(&mut tx, self.recipe.destination.machine())
            .await?
            .ok_or_else(|| fail("machine_unavailable", "The destination is gone."))?;
        let descriptor = environment_facts::on_machine(
            &mut tx,
            destination.id,
            self.recipe.destination.driver(),
        )
        .await?
        .ok_or_else(|| fail("facts_unavailable", "The destination declaration is gone."))?;
        if fingerprint(&mut tx, &destination, &descriptor).await?
            != self.recipe.destination_fingerprint
        {
            return Err(fail(
                "conflict.stale_revision",
                "The destination's machine policy or services changed before commit.",
            ));
        }
        if row.cancel_requested() {
            return Err(Stop::Cancelled);
        }
        if row
            .plan
            .get("file_states_content_verified")
            .and_then(Json::as_bool)
            != Some(true)
        {
            return Err(fail(
                "file_states_copy_unavailable",
                "The complete recorded-state copy has not been verified.",
            ));
        }
        let old = envs::lock_environment(&mut tx, self.recipe.environment)
            .await?
            .ok_or_else(|| fail("environment_gone", "The environment disappeared."))?;
        if old.generation != self.recipe.source_generation
            || old.revision != self.recipe.source_revision
            || old.machine_id != Some(self.recipe.source_machine)
        {
            return Err(fail(
                "conflict.stale_revision",
                "The source changed before commit.",
            ));
        }
        let admitted = match &self.recipe.destination {
            Destination::Attach { machine } => directory_place(&mut tx, &principal, &old, *machine)
                .await?
                .and_then(|(_, _, created)| created),
            Destination::Unit { host, template, .. } => {
                places_for_unit(
                    &mut tx,
                    &principal,
                    &self.recipe,
                    *host,
                    *template,
                    job.now(),
                )
                .await?
            }
        }
        .ok_or_else(|| {
            fail(
                "access.denied",
                "The destination no longer admits the environment.",
            )
        })?;
        if !orch_authz::environments::reserve_capacity(&mut tx, &admitted)
            .await
            .map_err(EngineError::from)?
        {
            return Err(fail(
                "no_capacity",
                "The destination Use entry has no remaining environment capacity.",
            ));
        }
        let fresh_host_entry = admitted.host.as_ref().and_then(|(entry, _)| *entry);
        let fresh_template_entry = admitted.template.as_ref().and_then(|(entry, _)| *entry);
        let before = orch_db::queries::environments::thread_engine::environment(&mut tx, old.id)
            .await?
            .ok_or_else(|| fail("environment_gone", "The environment disappeared."))?;
        let from: orch_api_contract::environment::EnvironmentRef = serde_json::from_value(
            crate::views::environment_ref_json(&mut tx, &before, Time::new(job.now())).await?,
        )
        .map_err(EngineError::from)?;
        let machine: Id = serde_json::from_value(row.plan["candidate_machine"].clone())
            .map_err(EngineError::from)?;
        let facts = &row.plan["candidate_facts"];
        let root = facts
            .get("workspace_root")
            .and_then(Json::as_str)
            .ok_or_else(|| fail("unhealthy", "The candidate root disappeared."))?;
        let (placement, template, host_entry, template_entry) = match &self.recipe.destination {
            Destination::Attach { .. } => ("connected", None, fresh_host_entry, None),
            Destination::Unit { template, .. } => (
                "managed",
                Some(*template),
                fresh_host_entry,
                fresh_template_entry,
            ),
        };
        if let Some(previous) = self.recipe.old_runtime {
            envs::retire_runtime(
                &mut tx,
                previous,
                Some("moved after verified data transfer"),
                job.now(),
            )
            .await?;
            if self.recipe.options.keep_previous {
                sqlx::query("UPDATE runtimes SET remove_at=NULL WHERE id=$1")
                    .bind(previous)
                    .execute(&mut *tx)
                    .await
                    .map_err(DbError::from)?;
            }
        }
        if let Some(runtime) = row
            .plan
            .get("runtime")
            .and_then(Json::as_str)
            .and_then(|id| id.parse::<Id>().ok())
        {
            sqlx::query(
                "UPDATE runtimes SET state='running',successor_of=NULL,updated_at=$2 WHERE id=$1",
            )
            .bind(runtime)
            .bind(job.now())
            .execute(&mut *tx)
            .await
            .map_err(DbError::from)?;
        }
        sqlx::query("UPDATE environments SET placement=$2,template_id=$3,host_entry_id=$4,template_entry_id=$5,machine_id=$6,workspace_root=$7,facts=$8,generation=$9,status='running',status_detail=NULL,config=jsonb_set(jsonb_set(config,'{env.data_sources}',$10),'{env.storage}','{}'),revision=revision+1,updated_at=$11 WHERE id=$1")
            .bind(old.id).bind(placement).bind(template).bind(host_entry).bind(template_entry).bind(machine).bind(root).bind(facts)
            .bind(self.recipe.next_generation).bind(json!(EnvironmentDataSources::default())).bind(job.now()).execute(&mut *tx).await.map_err(DbError::from)?;
        if (old.placement == "connected" && old.template_id.is_none()) || placement == "connected" {
            // Changing the physical ownership source retires the former owners' live
            // permission cut, without advancing unrelated roots or the system epoch.
            orch_db::queries::access::bump_epoch_in_root(&mut tx, old.org_id).await?;
        }
        let after = orch_db::queries::environments::thread_engine::environment(&mut tx, old.id)
            .await?
            .ok_or_else(|| fail("environment_gone", "The moved environment disappeared."))?;
        let to: orch_api_contract::environment::EnvironmentRef = serde_json::from_value(
            crate::views::environment_ref_json(&mut tx, &after, Time::new(job.now())).await?,
        )
        .map_err(EngineError::from)?;
        let by: orch_api_contract::users::UserRef =
            serde_json::from_value(crate::views::user_ref_json(&mut tx, self.recipe.by).await?)
                .map_err(EngineError::from)?;
        let marker = orch_api_contract::threads::MarkerPayload::EnvironmentMoved(
            orch_api_contract::threads::MarkerEnvironmentMoved {
                from,
                to,
                path_map: PathMap {
                    from: self.recipe.files.path_map.from.clone(),
                    to: root.into(),
                },
                mounts_detached: Vec::new(),
                by,
            },
        );
        let mut unit = Unit::new(&mut tx, job.now());
        for thread in &self.recipe.threads {
            let Some(current) = threads::lock(&mut *unit.conn, *thread).await? else {
                continue;
            };
            if let Some(segment) = current.current_segment_id {
                crate::ingest::add_marker(
                    &mut unit,
                    *thread,
                    segment,
                    None,
                    &serde_json::to_value(&marker).map_err(EngineError::from)?,
                    &format!("moved:{}", job.id()),
                )
                .await?;
            }

            unit.wake(*thread);
        }
        let finished = unit.finish().await?;
        crate::recovery::commit_threads(
            job,
            &mut tx,
            self.recipe.environment,
            self.recipe.next_generation,
            self.recipe.by,
            self.recipe.files.home.is_some(),
            false,
        )
        .await?;
        let mut audit = orch_db::audit::Entry::new(
            orch_db::audit::Actor::User(self.recipe.by),
            orch_db::audit::name!("environment.moved"),
            "environment",
            Some(old.id),
            "The environment moved to another machine",
        );
        audit.environment_id = Some(old.id);
        audit.node_id = Some(old.home_node_id);
        audit.history = true;
        audit.details = json!({"job":job.id(),"from_machine":self.recipe.source_machine,"to_machine":self.recipe.destination.machine(),"copied_existing_directories":self.recipe.borrowed});
        if let Destination::Unit { host, template, .. } = &self.recipe.destination {
            crate::resource_facts::library::created(
                &mut tx,
                *template,
                Some(*host),
                old.id,
                false,
                Some(self.recipe.by),
                Time::new(job.now()),
            )
            .await?;
        }
        orch_db::audit::append(
            &mut tx,
            &audit,
            &orch_db::audit::Origin::default(),
            job.now(),
        )
        .await?;
        jobs::merge_plan(&mut tx, job.id(), &json!({"committed":true}), job.now()).await?;
        job.engine().commit(tx, finished).await?;
        job.pass_commit().await?;
        Ok(Done::Finished)
    }

    async fn cleanup(&self, job: &Job) -> Result<Done, Stop> {
        match self.recipe.old_runtime {
            Some(runtime) => {
                let mut conn = job.engine().db().acquire().await?;
                let old = envs::runtime(&mut conn, runtime)
                    .await?
                    .ok_or_else(|| fail("runtime_gone", "The retired unit disappeared."))?;
                let host = hosts::host(&mut conn, old.host_id)
                    .await?
                    .ok_or_else(|| fail("host_gone", "The old host disappeared."))?;
                drop(conn);
                let command = Outgoing::new(
                    crate::ids::derived("machine-move.old.stop", &[job.id().as_bytes()]),
                    host.machine_id,
                    SerialKey::Runtime(runtime),
                    &NodeRuntimeStop {
                        runtime_id: runtime,
                        reason: Some(orch_driver_contract::runtime::UnitReason::Move),
                    },
                )
                .map_err(EngineError::from)?;
                match job.command(command).await {
                    Ok(_) => {}
                    Err(Stop::Failed(_)) => {
                        return Ok(Done::Skipped(
                            "The old unit is retired; its reaper will finish stopping it.".into(),
                        ));
                    }
                    Err(error) => return Err(error),
                }
            }
            None => {
                self.command(
                    job,
                    self.recipe.source_machine,
                    "machine-move.old.detach",
                    &EnvDetach {
                        environment_id: self.recipe.environment,
                    },
                )
                .await?;
                return Ok(Done::Skipped("The old connection is closed. Source data remains a recovery copy; existing directories were not changed.".into()));
            }
        }
        Ok(Done::Finished)
    }

    async fn rollback(&self, job: &Job, why: &Stop) -> Undone {
        let mut detail = None;
        let Ok(plan) = job.plan().await else {
            return Undone::Partly(
                "The move's candidate record is unavailable; the source data is kept.".into(),
            );
        };
        if plan.get("committed").and_then(Json::as_bool) == Some(true) {
            return Undone::Everything;
        }
        if let Err(error) = super::after_up::record_interrupted(job).await {
            detail = Some(format!(
                "An attempted after-start command's history still needs recovery: {error:?}"
            ));
        }
        if matches!(self.recipe.destination, Destination::Unit { .. })
            && plan.get("runtime").is_some()
        {
            let row = match job.row().await {
                Ok(row) => row,
                Err(_) => return Undone::Partly("The candidate unit is kept for recovery.".into()),
            };
            if let Undone::Partly(why) = super::provision::Provision::of(job.engine().clone(), &row)
                .undo(job, why)
                .await
            {
                detail = Some(why);
            }
        } else if let Some(machine) = plan
            .get("candidate_machine")
            .and_then(Json::as_str)
            .and_then(|id| id.parse::<Id>().ok())
        {
            let command = Outgoing::new(
                crate::ids::derived("machine-move.candidate.detach", &[job.id().as_bytes()]),
                machine,
                SerialKey::Environment(self.recipe.environment),
                &EnvDetach {
                    environment_id: self.recipe.environment,
                },
            );
            if let Ok(command) = command
                && job.command_to_undo(command).await.is_err()
            {
                detail =
                    Some("The candidate could not be detached; its copied data is kept.".into());
            }
            if detail.is_none() {
                detail=Some("The failed candidate's owned data is kept for review; no existing directory was replaced.".into());
            }
        }
        if plan.get("source_suspended").and_then(Json::as_bool) == Some(true) {
            let command = Outgoing::new(
                crate::ids::derived("machine-move.source.resume", &[job.id().as_bytes()]),
                self.recipe.source_machine,
                SerialKey::Environment(self.recipe.environment),
                &ProcResume {
                    environment_id: self.recipe.environment,
                },
            );
            if let Ok(command) = command
                && job.command_to_undo(command).await.is_err()
            {
                detail = Some(
                    "The source data is intact but resuming its processes needs retry.".into(),
                );
            }
        }
        if let Ok(mut conn) = job.engine().db().acquire().await {
            let _ = envs::set_status(
                &mut conn,
                self.recipe.environment,
                &["moving"],
                &self.recipe.source_status,
                None,
                job.now(),
            )
            .await;
        }
        detail.map_or(Undone::Everything, Undone::Partly)
    }
}

#[async_trait]
impl Phased for Moving {
    async fn run(&self, job: &Job, phase: &str) -> Result<Done, Stop> {
        match phase {
            "lock" => self.lock(job).await,
            "provision" => self.provision(job).await,
            "copy" => self.copy(job).await,
            "history" => {
                let plan = job.plan().await?;
                let machine: Id = serde_json::from_value(plan["candidate_machine"].clone())
                    .map_err(EngineError::from)?;
                crate::recovery::prepare_histories(
                    job,
                    self.recipe.environment,
                    machine,
                    plan["target_root"]
                        .as_str()
                        .ok_or_else(|| fail("files_missing", "The target root is missing"))?,
                    self.recipe.next_generation,
                    self.recipe.files.home.is_some(),
                )
                .await
            }
            "after_up" => super::after_up::run(job).await,
            "verify" => self.verify(job).await,
            "commit" => self.commit(job).await,
            "cleanup" => self.cleanup(job).await,
            _ => Err(fail("internal", "The machine move has an unknown phase.")),
        }
    }
    async fn undo(&self, job: &Job, why: &Stop) -> Undone {
        self.rollback(job, why).await
    }
    async fn result(&self, job: &Job) -> Option<Json> {
        Some(
            json!({"environment":self.recipe.environment,"machine":self.recipe.destination.machine(),"generation":self.recipe.next_generation,"runtime":job.plan().await.ok()?.get("runtime")}),
        )
    }
}

impl Engine {
    /// Releases portable move pins once and then uses the common thread-hold release.
    pub(crate) async fn after_machine_move(&self, row: &jobs::JobRow) {
        let released = async {
            let mut tx = self.db().begin().await?;
            let current = jobs::lock(&mut tx, row.id)
                .await?
                .ok_or_else(|| EngineError::unreadable("the terminal move disappeared"))?;
            if current
                .plan
                .get("move_pins_released")
                .and_then(Json::as_bool)
                != Some(true)
            {
                let pins: Vec<Hash> = current
                    .plan
                    .get("move_pins")
                    .cloned()
                    .map(serde_json::from_value)
                    .transpose()?
                    .unwrap_or_default();
                storage::unpin_blobs(&mut tx, &pins, self.inner.clock.now()).await?;
                jobs::merge_plan(
                    &mut tx,
                    row.id,
                    &json!({"move_pins_released":true}),
                    self.inner.clock.now(),
                )
                .await?;
            }
            tx.commit().await.map_err(DbError::from)?;
            Ok::<(), EngineError>(())
        }
        .await;
        if let Err(error) = released {
            tracing::warn!(job=%row.id,%error,"portable move pins await retention recovery");
        }
        if let Ok(recipe) = serde_json::from_value::<Recipe>(row.plan["portable"].clone()) {
            for thread in recipe.threads {
                if let Err(error) = self.release_lock(thread).await {
                    tracing::warn!(%thread,%error,"a move's thread hold awaits the next sweep");
                }
            }
        }
    }
}
