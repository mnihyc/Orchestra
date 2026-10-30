//! Environment and provider runtime provisioning, with the image phase: pulled, built or loaded.
//!
//! The job `environment_provision` makes a unit for an environment, or starts one that is stopped.
//! Its phases, in order: `schedule` (the host: where the volumes are, else the one the creator may
//! use that has the most room), `image` (the host makes the template's source available, a pull, a
//! build or a load, and reports how far it is), `volumes`, `tools` (the connector's program and the
//! builds of the harnesses the template lets run there, in the host's cache), `create` (the unit,
//! made and started by the host's node), `enroll` (the unit's connector comes online within the time
//! allowed), `attach` (the environment is attached to it, with what its commands are to find, and the
//! executors its threads work through are started), `verify` (the new side is looked at, which spends
//! no token) and `commit` (one transaction: the environment is `running`, on its machine, one unit
//! further). A start of a unit that already exists has the phases `start`, `enroll`, `attach`,
//! `verify` and `commit`.
//!
//! Every phase but `commit` can be asked again: the commands carry identifiers derived from the job,
//! so a command that was sent is not sent twice, and what a phase decided is kept in the job's plan.
//! A failure, or a cancel before `commit`, is undone: the unit is removed, and the volumes with it
//! when this was the first time they were made; the environment is `failed` (or `none` again after a
//! cancel of its first making) with the phase and the reason, and the job keeps them too.

use crate::binding;
use crate::commands::{CommandsError, Outgoing};
use crate::engine::Engine;
use crate::environments;
use crate::error::EngineError;
use crate::hosting::{Enrollment, EnrollmentAsk, HostServices, HostingError, ToolBuild};
use crate::ids;
use crate::jobs::health::{self, Side};
use crate::jobs::runner::{
    Done, Failure, INTERNAL, Job, MakeError, Making, Phase, Phased, Stop, Undone, make,
};
use crate::places::{self, Ask, IDLE_MINUTES_DEFAULT};
use async_trait::async_trait;
use chrono::{DateTime, Utc};
use orch_authz::{Caller, Principal};
use orch_db::DbError;
use orch_db::queries::environments::hosting::{
    self as env_rows, EnvironmentState, RuntimeFull, Up, runtime_state, status,
};
use orch_db::queries::jobs::{self, JobRow, kind};
use orch_db::queries::resources::hosting::{self as host_rows, HostState, TemplateState};
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

/// The phases of making a unit, and where the time usually goes: the image first (a pull or a build
/// is minutes), then the unit and the wait for its connector. They shape a bar and nothing else.
const SCHEDULE: Phase = Phase {
    name: "schedule",
    weight: 2,
};
const IMAGE: Phase = Phase {
    name: "image",
    weight: 40,
};
const VOLUMES: Phase = Phase {
    name: "volumes",
    weight: 4,
};
const TOOLS: Phase = Phase {
    name: "tools",
    weight: 8,
};
const CREATE: Phase = Phase {
    name: "create",
    weight: 16,
};
const START: Phase = Phase {
    name: "start",
    weight: 20,
};
const ENROLL: Phase = Phase {
    name: "enroll",
    weight: 12,
};
const ATTACH: Phase = Phase {
    name: "attach",
    weight: 6,
};
const AFTER_UP: Phase = Phase {
    name: "after_up",
    weight: 2,
};
/// Frozen source preparation precedes commands after start and all input files.
const FILES: Phase = Phase {
    name: "files",
    weight: 10,
};
const VERIFY: Phase = Phase {
    name: "verify",
    weight: 4,
};
const COMMIT: Phase = Phase {
    name: "commit",
    weight: 2,
};
/// A start has fewer phases and the wait for the connector is the most of it.
const START_ENROLL: Phase = Phase {
    name: "enroll",
    weight: 50,
};
const START_ATTACH: Phase = Phase {
    name: "attach",
    weight: 15,
};
const START_VERIFY: Phase = Phase {
    name: "verify",
    weight: 10,
};
const START_COMMIT: Phase = Phase {
    name: "commit",
    weight: 5,
};

/// The volumes an environment has, by what they are for: the workspace and the home, which a person
/// may put on a storage of their choice, and the state of the connector.
const VOLUME_ROLES: [&str; 3] = ["workspace", "home", "state"];
/// The program of the connector in the host's tool cache: the unit runs it.
pub const CONNECTOR_TOOL: &str = "orch-connector";
// An executor that is a harness's own program runs the build of that harness (the executor's manifest names it),
// and so a unit that is to offer it holds that build. The tool sets of Orch's executor need no program besides the
// connector.
/// How many times a unit's name is drawn again when it is taken: two runtimes made in the same
/// millisecond have the same twelve digits.
const NAME_TRIES: usize = 4;
/// How many hex digits of the runtime's identifier a unit's name holds.
const NAME_DIGITS: usize = 12;

/// What a provisioning job does.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    /// Makes a unit, for an environment that has none.
    Make,
    /// Starts the unit of an environment that is stopped.
    Start,
}

impl Mode {
    /// The word the plan keeps.
    #[must_use]
    pub const fn word(self) -> &'static str {
        match self {
            Self::Make => "make",
            Self::Start => "start",
        }
    }

    fn of(plan: &Json) -> Self {
        if plan.get("mode").and_then(Json::as_str) == Some("start") {
            Self::Start
        } else {
            Self::Make
        }
    }
}

/// Both modes run commands after attach and files, before the unit can become ready.
#[must_use]
pub fn phases(mode: Mode, _init: bool) -> Vec<Phase> {
    match mode {
        Mode::Make => {
            let mut all = vec![
                SCHEDULE, IMAGE, VOLUMES, TOOLS, CREATE, ENROLL, ATTACH, FILES,
            ];
            all.push(AFTER_UP);
            all.extend([VERIFY, COMMIT]);
            all
        }
        Mode::Start => vec![
            START,
            START_ENROLL,
            START_ATTACH,
            FILES,
            AFTER_UP,
            START_VERIFY,
            START_COMMIT,
        ],
    }
}

/// Queues the job that makes or starts the unit of an environment, in the caller's transaction. A job
/// of the kinds that run one at a time that is not over is [`MakeError::Running`].
pub async fn queue(
    conn: &mut orch_db::Conn,
    now: DateTime<Utc>,
    environment: &EnvironmentState,
    mode: Mode,
    by: Option<Id>,
) -> Result<JobRow, MakeError> {
    let list = phases(mode, false);
    let mut plan = json!({"mode":mode.word(),"by":by,"generation":environment.generation,"origin":orch_db::audit::current_origin()});
    if mode == Mode::Make
        && environment.generation == 0
        && let Some(files) =
            orch_db::queries::jobs::initial_files_request(conn, environment.id).await?
    {
        plan["initial_files"] = files;
    }
    let job = make(
        conn,
        &Making {
            kind: kind::ENVIRONMENT_PROVISION,
            environment: environment.id,
            plan,
            phases: &list,
            created_by: by,
        },
        now,
    )
    .await?;
    // An environment that is stopped and has no unit to start is made anew, so a make begins from
    // that too.
    let from: &[&str] = match mode {
        Mode::Make => &[
            status::NONE,
            status::FAILED,
            status::STOPPED,
            status::PROVISIONING,
            "lost",
        ],
        Mode::Start => &[status::STOPPED, status::PROVISIONING],
    };
    env_rows::set_status(conn, environment.id, from, status::PROVISIONING, None, now).await?;
    Ok(job)
}

// ---------------------------------------------------------------- the spec of a unit

/// The name of a unit: `orch-environment-` and the last twelve hex digits of its runtime's identifier, as the driver
/// contract, a provider's unit and a move's successor name theirs. The last digits, because an identifier begins with the
/// moment it was made: its first twelve digits are that moment to the millisecond, the same for every runtime made in it.
/// A driver that checks the name of what it is asked to make (the Kubernetes driver does) refuses any other.
#[must_use]
pub fn unit_name(runtime: &Id) -> String {
    let simple = runtime.simple().to_string();
    let digits = &simple[simple.len().saturating_sub(NAME_DIGITS)..];
    format!("orch-environment-{digits}")
}

/// The kinds of harness a unit made from a template is to be given the builds of: those the template lets
/// run beside the tools (`colocated_harnesses`), then those whose own exec server the environment offers,
/// each once. The thread that will work there is not known when the unit is made, and so what it needs is
/// what its template says harnesses there may be.
#[must_use]
pub fn harnesses_wanted(spec: &Json, offers: &[String]) -> Vec<String> {
    let programs = orch_agent_programs::Programs::builtin();
    let listed = orch_agent_programs::Programs::colocated_for(spec).into_iter();
    let offered = offers.iter().filter_map(|offer| {
        programs
            .program_of_executor(&programs.offered_executor(offer))
            .map(str::to_owned)
    });
    let mut wanted: Vec<String> = Vec::new();
    for kind in listed.chain(offered) {
        if !wanted.contains(&kind) {
            wanted.push(kind);
        }
    }
    wanted
}

/// What the body of `node.runtime.create` is made from.
#[derive(Debug, Clone)]
pub struct SpecInputs<'a> {
    /// The runtime.
    pub runtime: Id,
    /// The name of its unit.
    pub name: &'a str,
    /// The driver.
    pub driver: &'a str,
    /// The template's spec.
    pub spec: &'a Json,
    /// The limits of the unit.
    pub limits: &'a Json,
    /// What the connector enrolls with and dials.
    pub enrollment: &'a Enrollment,
    /// The harness builds to make available (not the connector's own program).
    pub tools: &'a [ToolBuild],
    /// The volumes, by what they are for.
    pub volumes: &'a Json,
}

/// The body of `node.runtime.create`, in the stored form: the token is the reference to the secret that
/// holds it, which the link seals to the host's key when the command is written.
#[must_use]
pub fn runtime_spec(inputs: &SpecInputs<'_>) -> Json {
    let env: BTreeMap<String, String> = inputs
        .spec
        .get("env")
        .and_then(Json::as_object)
        .map(|members| {
            members
                .iter()
                .filter_map(|(name, value)| {
                    value.as_str().map(|text| (name.clone(), text.to_owned()))
                })
                .collect()
        })
        .unwrap_or_default();
    // A harness that runs beside the tools needs the connector in the unit to host agents as well.
    let beside = !orch_agent_programs::Programs::colocated_for(inputs.spec).is_empty();
    let roles: &[&str] = if beside {
        &["environment", "agent"]
    } else {
        &["environment"]
    };
    json!({
        "runtime_id": inputs.runtime,
        "kind": "environment",
        "name": inputs.name,
        "driver": inputs.driver,
        "spec": inputs.spec,
        "resources": inputs.limits,
        "roles": roles,
        "gateway_url": inputs.enrollment.gateway_url,
        "enroll_token": { "$secret": inputs.enrollment.secret },
        "ca_cert": inputs.enrollment.ca_cert,
        "tools": inputs.tools.iter().map(|tool| json!({ "name": tool.name, "version": tool.version })).collect::<Vec<_>>(),
        "volumes": inputs.volumes.as_object().into_iter().flatten()
            .filter(|(role, _)| VOLUME_ROLES.contains(&role.as_str()))
            .map(|(role, volume)| (role.clone(), volume.clone())).collect::<serde_json::Map<_, _>>(),
        "mounts": [],
        "env": env,
        "labels": {
            "orch.system": inputs.enrollment.system,
            "orch.runtime_id": inputs.runtime,
            "orch.kind": "environment",
        },
    })
}

/// What names a source on its host, which serialises the work on it: the reference of an image, and
/// for a build or an archive a hash of what defines it.
#[must_use]
pub fn image_key(spec: &Json) -> String {
    for source in ["image", "build", "archive"] {
        if let Some(value) = spec.get(source).filter(|value| !value.is_null()) {
            return match value.as_str() {
                Some(reference) => reference.to_owned(),
                None => format!(
                    "{source}:{}",
                    orch_types::Hash::of(value.to_string().as_bytes())
                ),
            };
        }
    }
    "image".to_owned()
}

// ---------------------------------------------------------------- the job

/// The phases of making or starting a unit.
#[derive(Debug, Clone)]
pub struct Provision {
    engine: Engine,
    environment: Id,
    mode: Mode,
}

/// Where a unit is, and what it is made from.
struct Placed {
    environment: EnvironmentState,
    runtime: RuntimeFull,
    host: HostState,
    template: TemplateState,
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

impl Provision {
    /// The work of a job row of the kind `environment_provision`.
    #[must_use]
    pub fn of(engine: Engine, row: &JobRow) -> Self {
        Self {
            engine,
            environment: row.environment_id.unwrap_or_else(Id::nil),
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
        let environment = env_rows::environment(&mut conn, self.environment)
            .await?
            .filter(|environment| {
                environment.deleted_at.is_none()
                    || plan.get("restoring").and_then(Json::as_bool) == Some(true)
            })
            .ok_or_else(|| failed("environment_gone", "the environment was removed"))?;
        let planned = plan
            .get("runtime")
            .and_then(Json::as_str)
            .and_then(|id| id.parse::<Id>().ok());
        let runtime = match planned {
            Some(id) => env_rows::runtime(&mut conn, id).await?,
            None => env_rows::live_runtime(&mut conn, environment.id).await?,
        }
        .ok_or_else(|| failed("runtime_gone", "the environment has no unit to work on"))?;
        let host = host_rows::host(&mut conn, runtime.host_id)
            .await?
            .ok_or_else(|| failed("no_host", "the host the unit is on is gone"))?;
        if host.disabled || environment.status == "disabled" {
            return Err(failed(
                "environment_unavailable",
                "the environment or its host was disabled",
            ));
        }
        let mut template = host_rows::template(&mut conn, runtime.template_id)
            .await?
            .ok_or_else(|| failed("template_unavailable", "the template is gone"))?;
        // Runtime replacement uses the definition confirmed by its plan.
        if let Some(definition) = plan.get("definition") {
            template.spec = definition.clone();
        } else if let Some(chosen) =
            crate::isolation::chosen_runtime(&mut conn, environment.id).await?
        {
            // The runtime the environment chose when it was made, over the template's own.
            crate::isolation::apply_choice(&mut template.spec, Some(&chosen));
        }
        Ok(Placed {
            environment,
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
        let environment = env_rows::lock_environment(&mut tx, self.environment)
            .await?
            .filter(|environment| environment.deleted_at.is_none())
            .ok_or_else(|| failed("environment_gone", "the environment was removed"))?;
        let template = match environment.template_id {
            Some(id) => host_rows::template(&mut tx, id).await?,
            None => None,
        }
        .filter(|template| {
            template.enabled && template.deleted_at.is_none() && template.kind == "environment"
        })
        .ok_or_else(|| {
            failed(
                "template_unavailable",
                "the template is gone or switched off",
            )
        })?;
        let principal = Principal::load(
            &mut tx,
            Caller::session(
                plan.pointer("/recovery/by")
                    .and_then(Json::as_str)
                    .and_then(|id| id.parse().ok())
                    .unwrap_or(environment.owner_id),
            ),
            Time::new(now),
        )
        .await
        .map_err(EngineError::from)?;
        // An environment that has been made stays on the host that has its volumes.
        let previous = if environment.generation > 0 && plan.get("recovery").is_none() {
            env_rows::published_host_of(&mut tx, environment.id).await?
        } else {
            None
        };
        let wanted = places::wanted(
            &mut tx,
            environment.home_node_id,
            template.clone(),
            &environment.config,
        )
        .await?;
        let named: Option<Id> = if let Some(recovery) = plan.get("recovery") {
            serde_json::from_value(recovery["host"].clone()).map_err(EngineError::from)?
        } else {
            match previous {
                Some(host) => Some(host),
                None => setting(
                    &mut tx,
                    SettingsPlace::Environment(environment.id),
                    keys::ENV_HOST_ID,
                )
                .await
                .map_err(EngineError::from)?,
            }
        };
        let minutes_idle = setting::<i64>(
            &mut tx,
            SettingsPlace::Environment(environment.id),
            keys::ENV_RECYCLE_IDLE_MINUTES,
        )
        .await
        .map_err(EngineError::from)?
        .unwrap_or(IDLE_MINUTES_DEFAULT);
        // A completed environment keeps its admitted own-host/template behavior.
        // Only its initial pending provision is a new placement admission.
        let listed = if let Some(host) = previous {
            places::retained_host_places(
                &mut tx,
                &principal,
                &Ask {
                    home: environment.home_node_id,
                    wanted: &wanted,
                    current: previous,
                    minutes_idle,
                    now,
                },
                environment.id,
                host,
            )
            .await?
        } else {
            places::host_places_for_environment(
                &mut tx,
                &principal,
                &Ask {
                    home: environment.home_node_id,
                    wanted: &wanted,
                    current: None,
                    minutes_idle,
                    now,
                },
                environment.id,
            )
            .await?
        };
        let chosen = places::choose(&listed, named)
            .cloned()
            .map_err(|none| failed(none.code, none.message))?;
        if previous.is_none()
            && let Some(created) = &chosen.created
            && !orch_authz::environments::reserve_capacity(&mut tx, created)
                .await
                .map_err(EngineError::from)?
        {
            return Err(failed(
                "no_capacity",
                "The selected Use entry has no remaining environment capacity.",
            ));
        }
        // A unit that failed is taken away before its successor is made, so that two never run at once.
        if let Some(old) = env_rows::live_runtime(&mut tx, environment.id).await? {
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
        let (runtime_id, name) = self
            .insert_runtime(
                &mut tx,
                &environment,
                &template,
                &chosen.host,
                &crate::mount_copies::of_plan(&plan)?,
                now,
            )
            .await?;
        if let Some(recovery) = plan.get("recovery") {
            let mut volumes = recovery["volumes"].clone();
            volumes["state"] = json!({"name":format!("orch-state-{}-{}",environment.id,runtime_id.simple()),"storage":null});
            sqlx::query("UPDATE runtimes SET volumes=$2 WHERE id=$1")
                .bind(runtime_id)
                .bind(volumes)
                .execute(&mut *tx)
                .await
                .map_err(DbError::from)?;
        }
        if previous.is_none()
            && let Some(created) = &chosen.created
        {
            env_rows::set_entries(
                &mut tx,
                environment.id,
                created.host.as_ref().and_then(|(entry, _)| *entry),
                created.template.as_ref().and_then(|(entry, _)| *entry),
            )
            .await?;
        }
        let warnings: Vec<&str> = chosen
            .place
            .warnings
            .iter()
            .map(|warning| match warning {
                orch_api_contract::environment::HostWarning::Full => "full",
                orch_api_contract::environment::HostWarning::NoRoom => "no_room",
            })
            .collect();
        orch_db::queries::jobs::merge_plan(
            &mut tx,
            job.id(),
            &json!({
                "host": chosen.place.host.id,
                "host_name": chosen.place.host.name,
                "runtime": runtime_id,
                "unit": name,
                "fresh": environment.generation == 0,
                "warnings": warnings,
                "declared_memory_mb": chosen.place.declared.memory_mb,
                "free_memory_mb": chosen.place.free.memory_mb,
                "needs_memory_mb": wanted.needs.memory_mb,
            }),
            now,
        )
        .await?;
        tx.commit().await.map_err(DbError::from)?;
        Ok(Done::Finished)
    }

    /// Makes the runtime row, drawing the runtime's identifier again when the name of its unit is taken.
    async fn insert_runtime(
        &self,
        conn: &mut orch_db::Conn,
        environment: &EnvironmentState,
        template: &TemplateState,
        host: &HostState,
        decisions: &crate::mount_copies::MountDecisions,
        now: DateTime<Utc>,
    ) -> Result<(Id, String), Stop> {
        let storage = setting::<Json>(
            conn,
            SettingsPlace::Environment(environment.id),
            keys::ENV_STORAGE,
        )
        .await
        .map_err(EngineError::from)?;
        let storage_of = |role: &str| {
            storage
                .as_ref()
                .and_then(|named| named.get(role))
                .and_then(Json::as_str)
                .map(str::to_owned)
        };
        let mut volumes = json!({
            "workspace": { "name": format!("orch-ws-{}", environment.id), "storage": storage_of("workspace") },
            "home": { "name": format!("orch-home-{}", environment.id), "storage": storage_of("home") },
            "state": { "name": format!("orch-state-{}", environment.id), "storage": storage_of("state") },
        });
        let sources = orch_settings::get::<orch_driver_contract::runtime::EnvironmentDataSources>(
            conn,
            orch_settings::Place::Environment(environment.id),
            keys::ENV_DATA_SOURCES,
        )
        .await
        .map_err(EngineError::from)?
        .unwrap_or_default();
        for (role, source) in [("workspace", &sources.workspace), ("home", &sources.home)] {
            if let Some(path) = source.directory() {
                volumes[role]["directory"] = json!({"path": path});
                volumes[role]["storage"] = Json::Null;
            }
        }
        // A copy's location is the one its private layer is made for, though the host has none yet.
        let planned = decisions.planned(Some(host.machine_id), environment.owner_id, now);
        let mounts = crate::mounts::resolve_for(
            conn,
            &crate::mounts::Target {
                environment: environment.id,
                home: environment.home_node_id,
                own: &environment.config,
                owner: environment.owner_id,
                machine: Some(host.machine_id),
                driver: Some(&template.driver),
                workspace: &environment.workspace_root,
                home_path: Some("/home/agent"),
                storage: storage_of("workspace"),
                planned: &planned,
                detached: &decisions.detached,
            },
            now,
        )
        .await?;
        for mount in mounts {
            if let Some(layer) = mount.layer {
                volumes[format!("over:{}", mount.view.mount.id)] = layer;
            }
        }
        let hash = orch_types::Hash::of(
            json!([template.driver, template.spec, environment.config])
                .to_string()
                .as_bytes(),
        );
        for _ in 0..NAME_TRIES {
            let id = new_id();
            let name = unit_name(&id);
            let made = env_rows::insert_runtime(
                conn,
                &env_rows::NewRuntime {
                    id,
                    host_id: host.id,
                    unit_name: &name,
                    environment_id: environment.id,
                    driver: &template.driver,
                    template_id: template.id,
                    spec_hash: hash.as_bytes(),
                    generation: environment.generation + 1,
                    volumes: &volumes,
                },
                now,
            )
            .await;
            match made {
                Ok(()) => return Ok((id, name)),
                Err(DbError::Unique { .. }) => {}
                Err(error) => return Err(error.into()),
            }
        }
        Err(failed(INTERNAL, "no free name for the unit was found"))
    }

    // ------------------------------------------------------------ image, volumes, tools

    async fn image(&self, job: &Job) -> Result<Done, Stop> {
        if job
            .plan()
            .await?
            .get("kept_image")
            .is_some_and(|image| !image.is_null())
        {
            return Ok(Done::Skipped(
                "The environment's kept image is already available on this host.".into(),
            ));
        }
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
        let plan = job.plan().await?;
        let carry = plan
            .get("volume_carry")
            .map(|value| serde_json::from_value::<crate::volume_carry::Carry>(value.clone()))
            .transpose()
            .map_err(EngineError::from)?;
        let volumes = placed
            .runtime
            .volumes
            .as_object()
            .ok_or_else(|| failed(INTERNAL, "the runtime has no owned volume map"))?;
        for (index, (role, volume)) in volumes.iter().enumerate() {
            if volume.get("directory").is_some_and(Json::is_object) {
                continue;
            }
            let name = volume["name"].as_str().unwrap_or_default().to_owned();
            let storage = volume["storage"].as_str().map(str::to_owned);
            if plan
                .pointer("/recovery/restored")
                .and_then(Json::as_object)
                .is_some_and(|restored| restored.contains_key(role))
            {
                let request = orch_link_contract::commands::NodeVolumeInspect {
                    driver: placed.runtime.driver.clone(),
                    name: name.clone(),
                    storage: storage.clone(),
                    environment_id: placed.environment.id,
                };
                let answer = job
                    .command(
                        Outgoing::new(
                            new_id(),
                            placed.host.machine_id,
                            SerialKey::Volume(name.clone()),
                            &request,
                        )
                        .map_err(body_stop)?,
                    )
                    .await?;
                let answer: orch_link_contract::commands::NodeVolumeInspectResult =
                    serde_json::from_value(answer).map_err(EngineError::from)?;
                if !answer.exists {
                    return Err(failed(
                        "restored_volume_changed",
                        "A restored volume disappeared after review",
                    ));
                }
                job.progress((index + 1) as f64 / volumes.len().max(1) as f64, None)
                    .await?;
                continue;
            }
            let mut body = NodeVolumeEnsure {
                driver: placed.runtime.driver.clone(),
                name: name.clone(),
                storage,
                labels: BTreeMap::from([
                    (
                        "orch.environment_id".to_owned(),
                        placed.environment.id.to_string(),
                    ),
                    ("orch.kind".to_owned(), "environment".to_owned()),
                ]),
            };
            body.labels
                .insert("orch.runtime_id".into(), placed.runtime.id.to_string());
            if plan.get("recovery").is_some()
                && !plan
                    .pointer("/recovery/restored")
                    .and_then(Json::as_object)
                    .is_some_and(|restored| restored.contains_key(role))
            {
                // A backup arriving after review must not be mistaken for this job's empty scratch data.
                body.labels
                    .insert("orch.recovery_job_id".into(), job.id().to_string());
            }

            if carry.as_ref().is_some_and(|carry| {
                carry
                    .selectors
                    .iter()
                    .any(|selector| crate::volume_carry::role(*selector) == *role)
            }) {
                body.labels.insert("orch.carried".into(), "true".into());
            }
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
            job.progress((index + 1) as f64 / volumes.len().max(1) as f64, None)
                .await?;
        }
        crate::volume_carry::apply(
            job,
            &placed.runtime,
            placed.host.machine_id,
            placed.environment.id,
        )
        .await?;
        // A copy of a mount's data lands before the unit that attaches it is made.
        crate::mount_copies::apply(job, placed.host.machine_id).await?;
        Ok(Done::Finished)
    }

    async fn tools(&self, job: &Job) -> Result<Done, Stop> {
        let placed = self.placed(job).await?;
        let services = self.services()?;
        // The connector, and the builds of the harnesses the template lets run there.
        let builds = {
            let mut conn = self
                .engine
                .inner
                .db
                .acquire()
                .await
                .map_err(EngineError::from)?;
            let offers = binding::offers_of(&mut conn, placed.environment.id).await?;
            let wanted = harnesses_wanted(&placed.template.spec, &offers);
            // Runtime replacement has already selected and recorded its builds.
            if job.plan().await?.get("target").is_some() {
                serde_json::from_value(job.plan().await?["tools"].clone())
                    .map_err(EngineError::from)?
            } else {
                let requested = crate::hosting::harness_requests(
                    &mut conn,
                    crate::updates::Target::Environment(placed.environment.id),
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
                            created_by: placed.environment.owner_id,
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
            let own = {
                let mut conn = self
                    .engine
                    .inner
                    .db
                    .acquire()
                    .await
                    .map_err(EngineError::from)?;
                setting::<Json>(
                    &mut conn,
                    SettingsPlace::Environment(placed.environment.id),
                    keys::ENV_RESOURCES,
                )
                .await
                .map_err(EngineError::from)?
            };
            places::limits_of(&placed.template.spec, own.as_ref())
        };
        let mut spec = runtime_spec(&SpecInputs {
            runtime: placed.runtime.id,
            name: &placed.runtime.unit_name,
            driver: &placed.runtime.driver,
            spec: &placed.template.spec,
            limits: &limits,
            enrollment: &enrollment,
            tools: &harness_builds,
            volumes: &placed.runtime.volumes,
        });
        if let Some(facts) = plan.get("image_facts") {
            spec["spec"]["image_facts"] = facts.clone();
        }
        if let Some(image) = plan
            .get("kept_image")
            .filter(|image| !image.is_null())
            .or(placed.runtime.kept_image.as_ref())
        {
            spec["kept_image"] = image.clone();
        }
        {
            let mut conn = self
                .engine
                .db()
                .acquire()
                .await
                .map_err(EngineError::from)?;
            // Check again before making a unit: its runtime or the place's policy may have changed
            // since admission, or a confirmed replacement may carry a different definition.
            if let Some(policy) = setting::<Json>(
                &mut conn,
                SettingsPlace::Environment(placed.environment.id),
                keys::ENVIRONMENTS_RUNTIMES,
            )
            .await
            .map_err(EngineError::from)?
            {
                crate::isolation::check_host_policy(
                    &policy,
                    placed
                        .host
                        .drivers
                        .get(&placed.runtime.driver)
                        .unwrap_or(&Json::Null),
                    placed.template.spec.get("runtime").and_then(Json::as_str),
                )
                .map_err(|why| failed("runtime_policy", why))?;
            }
            spec["capabilities"] = serde_json::to_value(
                self.engine
                    .prepared_capabilities(
                        &mut conn,
                        placed.environment.id,
                        &placed.runtime.driver,
                        &placed.template.spec,
                    )
                    .await?,
            )
            .map_err(EngineError::from)?;
        }
        // What the confirmed plan decided: mounts it detaches, and the locations its copies fill on this host.
        let decisions = crate::mount_copies::of_plan(&plan)?;
        let planned = decisions.planned(
            Some(placed.host.machine_id),
            placed.environment.owner_id,
            job.now(),
        );
        let mut mounts = {
            let mut conn = self
                .engine
                .db()
                .acquire()
                .await
                .map_err(EngineError::from)?;
            crate::mounts::resolve_for(
                &mut conn,
                &crate::mounts::Target {
                    environment: placed.environment.id,
                    home: placed.environment.home_node_id,
                    own: &placed.environment.config,
                    owner: placed.environment.owner_id,
                    machine: Some(placed.host.machine_id),
                    driver: Some(&placed.runtime.driver),
                    workspace: &placed.environment.workspace_root,
                    home_path: Some("/home/agent"),
                    storage: placed
                        .runtime
                        .volumes
                        .get("workspace")
                        .and_then(|volume| volume.get("storage"))
                        .and_then(Json::as_str)
                        .map(str::to_owned),
                    planned: &planned,
                    detached: &decisions.detached,
                },
                job.now(),
            )
            .await?
        };
        // A move's candidate uses distinct private volumes even on the same physical host.
        for mount in &mut mounts {
            if let Some(layer) = placed
                .runtime
                .volumes
                .get(format!("over:{}", mount.view.mount.id))
            {
                mount.layer = Some(layer.clone());
                if let Some(spec) = &mut mount.spec {
                    spec["layer"] = layer.clone();
                }
            }
        }
        spec["mounts"] = json!(
            mounts
                .into_iter()
                .filter_map(|mount| mount.spec)
                .collect::<Vec<_>>()
        );
        spec["labels"]["orch.environment_id"] = json!(placed.environment.id);
        spec["labels"]["orch.template_id"] = json!(placed.template.id);
        // What the unit serves and why it is made, for the operator's hooks on its host.
        {
            let job_kind = job.row().await?.kind;
            let mut conn = self
                .engine
                .db()
                .acquire()
                .await
                .map_err(EngineError::from)?;
            spec["subject"] = crate::unit_subjects::environment(
                &mut conn,
                crate::unit_subjects::OfEnvironment {
                    job: &job_kind,
                    id: placed.environment.id,
                    name: &placed.environment.name,
                    home: placed.environment.home_node_id,
                    template_id: Some(placed.template.id),
                    template: &placed.template.name,
                    generation: i64::from(placed.environment.generation),
                },
            )
            .await
            .map_err(EngineError::from)?;
        }
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
                    SettingsPlace::Environment(placed.environment.id),
                    &placed.template.spec,
                )
                .await?
            }
        };
        spec["network_policy"] =
            serde_json::to_value(&network_policy).map_err(EngineError::from)?;
        job.remember(&json!({ "network_policy": network_policy }))
            .await?;
        if plan.get("restoring").and_then(Json::as_bool) == Some(true) {
            spec["labels"]["orch.restore"] = json!("true");
            let kept = plan
                .get("retained_spec")
                .ok_or_else(|| failed("unavailable", "the original unit definition is missing"))?;
            for field in ["volumes", "mounts", "resources", "env"] {
                if let Some(value) = kept.get(field) {
                    spec[field] = value.clone();
                }
            }
        }
        // Runtime replacement keeps the exact command definition for later recreation.
        job.remember(&json!({ "created_spec": spec })).await?;
        {
            let hash = crate::updates::definition_hash(
                &placed.runtime.driver,
                &placed.template.spec,
                spec.get("resources").unwrap_or(&limits),
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
            return Err(self.explain_creation(&plan, &placed, stop));
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
            .map_err(|stop| self.explain_creation(&plan, &placed, stop))?;
        Ok(Done::Finished)
    }

    /// What a host that could not make a unit says, with what the system had declared of it: a host
    /// that warned and then could not do it says both.
    fn explain_creation(&self, plan: &Json, placed: &Placed, stop: Stop) -> Stop {
        let Stop::Failed(failure) = stop else {
            return stop;
        };
        let warned = plan
            .get("warnings")
            .and_then(Json::as_array)
            .is_some_and(|warnings| !warnings.is_empty());
        let message = if warned && failure.code == "no_capacity" {
            format!(
                "{} could not start it: {}. It had declared {} MiB free, and this environment needs {} MiB",
                placed.host.name,
                failure.message,
                plan.get("free_memory_mb")
                    .and_then(Json::as_i64)
                    .unwrap_or(0),
                plan.get("needs_memory_mb")
                    .and_then(Json::as_i64)
                    .unwrap_or(0),
            )
        } else {
            format!(
                "{} could not start it: {}",
                placed.host.name, failure.message
            )
        };
        Stop::Failed(Failure { message, ..failure })
    }

    async fn start(&self, job: &Job) -> Result<Done, Stop> {
        let placed = self.placed(job).await?;
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
                SettingsPlace::Environment(placed.environment.id),
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
            .map_err(|stop| self.explain_creation(&json!({}), &placed, stop))?;
        Ok(Done::Finished)
    }

    // ------------------------------------------------------------ enroll, attach

    async fn enroll(&self, job: &Job) -> Result<Done, Stop> {
        let placed = self.placed(job).await?;
        // A unit of a virtual machine boots before its connector can enroll: its runtime's class says how long.
        let allowed = crate::isolation::enroll_time(
            self.engine.inner.config.hosting.enroll_timeout,
            &placed.host.drivers,
            &placed.runtime.driver,
            &placed.template.spec,
        );
        let began = tokio::time::Instant::now();
        loop {
            job.check()?;
            let (state, online) = {
                let mut conn = self
                    .engine
                    .inner
                    .db
                    .acquire()
                    .await
                    .map_err(EngineError::from)?;
                let runtime = env_rows::runtime(&mut conn, placed.runtime.id).await?;
                let machine = host_rows::machine_of_runtime(&mut conn, placed.runtime.id).await?;
                (runtime, machine)
            };
            let Some(runtime) = state else {
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

    async fn attach(&self, job: &Job) -> Result<Done, Stop> {
        let placed = self.placed(job).await?;
        let portable = job.plan().await?.get("portable").is_some();
        let mut conn = self
            .engine
            .inner
            .db
            .acquire()
            .await
            .map_err(EngineError::from)?;
        let (machine, _) = host_rows::machine_of_runtime(&mut conn, placed.runtime.id)
            .await?
            .ok_or_else(|| failed("unit_failed", "the unit's connector is not known"))?;
        let mut with =
            environments::attach_with(&mut conn, placed.environment.id, job.now()).await?;
        // Initial source work uses this unit before the environment is published as running.
        // Its native store must already use the generation that the final commit will publish.
        with.data_generation = Some(i64::from(placed.runtime.generation));
        if portable {
            with.data_sources = None;
            with.storage = None;
            with.data_generation = None;
        }
        drop(conn);
        let key = SerialKey::Environment(placed.environment.id);
        // A managed environment is attached with no root: the driver gave its unit the places it uses.
        let body = environments::attach_body(placed.environment.id, None, &with);
        let attach = Outgoing::new(
            ids::derived("env.attach", &[job.id().as_bytes()]),
            machine,
            key.clone(),
            &body,
        )
        .map_err(body_stop)?;
        let facts = job.command(attach).await?;
        let root = facts
            .get("workspace_root")
            .and_then(Json::as_str)
            .map(str::to_owned);
        {
            let mut conn = self
                .engine
                .inner
                .db
                .acquire()
                .await
                .map_err(EngineError::from)?;
            // A successor's facts are published only by the replacement's commit.
            if placed.runtime.successor_of.is_some() || portable {
                drop(conn);
                job.remember(&json!({ "candidate_facts": facts })).await?;
            } else {
                env_rows::record_facts(
                    &mut conn,
                    placed.environment.id,
                    &facts,
                    root.as_deref(),
                    job.now(),
                )
                .await?;
            }
        }
        for executor in with.executors {
            let command = Outgoing::new(
                ids::derived("exec.start", &[job.id().as_bytes(), executor.as_bytes()]),
                machine,
                key.clone(),
                &orch_link_contract::commands::ExecStart {
                    environment_id: placed.environment.id,
                    executor: executor.clone(),
                },
            )
            .map_err(body_stop)?;
            if let Err(Stop::Failed(failure)) = job.command(command).await {
                // An executor that cannot be started is listed as unavailable in the facts and passed
                // over: a thread that needs it says so at its pre-flight.
                tracing::warn!(%executor, code = %failure.code, "an executor could not be started");
            }
        }
        Ok(Done::Finished)
    }

    // ------------------------------------------------------------ verify, commit

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
        let current = env_rows::environment(&mut conn, placed.environment.id)
            .await?
            .ok_or_else(|| failed("environment_gone", "the environment was removed"))?;
        let machine = host_rows::machine_of_runtime(&mut conn, placed.runtime.id).await?;
        let runtime = env_rows::runtime(&mut conn, placed.runtime.id).await?;
        let wrong = health::findings(&Side {
            online: matches!(machine, Some((_, true))),
            facts: &current.facts,
            runtime_state: runtime
                .as_ref()
                .map_or("failed", |runtime| runtime.state.as_str()),
        });
        if wrong.is_empty() {
            Ok(Done::Finished)
        } else {
            Err(failed("unhealthy", wrong.join("; ")))
        }
    }

    async fn commit(&self, job: &Job) -> Result<Done, Stop> {
        let initial = job.plan().await?;
        if initial.get("initial_files").is_some()
            && initial.get("initial_files_done").and_then(Json::as_bool) != Some(true)
        {
            return Err(failed(
                "initial_files_unavailable",
                "the selected initial files have not been delivered",
            ));
        }
        if job
            .plan()
            .await?
            .get("provision_committed")
            .and_then(Json::as_bool)
            == Some(true)
        {
            return Ok(Done::Finished);
        }
        job.pass_commit().await?;
        let placed = self.placed(job).await?;
        let now = job.now();
        let mut tx = self
            .engine
            .inner
            .db
            .begin()
            .await
            .map_err(EngineError::from)?;
        orch_db::queries::transaction_guard::lock_authority(&mut tx).await?;
        let (machine, _) = host_rows::machine_of_runtime(&mut tx, placed.runtime.id)
            .await?
            .ok_or_else(|| failed("unit_failed", "the unit's connector is not known"))?;
        let current = env_rows::lock_environment(&mut tx, placed.environment.id)
            .await?
            .ok_or_else(|| failed("environment_gone", "the environment was removed"))?;
        let up = env_rows::make_running(
            &mut tx,
            current.id,
            &[status::PROVISIONING],
            &Up {
                machine,
                workspace_root: None,
                facts: None,
                new_unit: self.mode == Mode::Make,
            },
            now,
        )
        .await?;
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
        host_rows::delete_runtime_secret(&mut tx, placed.runtime.id, "enroll_token").await?;
        if !up {
            return Err(failed(
                "conflict",
                "the environment is not being made any more: it was changed meanwhile",
            ));
        }
        let row = jobs::job(&mut tx, job.id())
            .await?
            .ok_or_else(|| EngineError::unreadable("the provisioning job is missing"))?;
        let origin = row
            .plan
            .get("origin")
            .cloned()
            .and_then(|v| serde_json::from_value(v).ok())
            .unwrap_or_default();
        for action in if self.mode == Mode::Make {
            vec![
                orch_db::audit::name!("environment.provisioned"),
                orch_db::audit::name!("environment.started"),
            ]
        } else {
            vec![orch_db::audit::name!("environment.started")]
        } {
            let summary = format!(
                "{} was {}",
                current.name,
                if action == "environment.provisioned" {
                    "provisioned"
                } else {
                    "started"
                }
            );
            let mut entry = orch_db::audit::Entry::new(
                row.created_by
                    .map_or(orch_db::audit::Actor::System, orch_db::audit::Actor::User),
                action,
                "environment",
                Some(current.id),
                &summary,
            );
            entry.history = true;
            entry.node_id = Some(current.home_node_id);
            entry.org_id = Some(current.org_id);
            entry.environment_id = Some(current.id);
            entry.details = json!({"runtime":placed.runtime.id,"generation":placed.runtime.generation,"job":job.id(),"reason":if row.created_by.is_some(){"request"}else{"work"}});
            orch_db::audit::append(&mut tx, &entry, &origin, now).await?;
        }
        if let Some(recovery) = row.plan.get("recovery") {
            let recipe: crate::recovery::Recipe =
                serde_json::from_value(recovery.clone()).map_err(EngineError::from)?;
            if row.plan.get("recovery_files_done") != Some(&json!(true)) {
                return Err(failed(
                    "files_missing",
                    "Recovery files and history were not verified",
                ));
            }
            sqlx::query("UPDATE environments SET lost_at=NULL,lost_by=NULL,loss_missing=false,removal_phase=NULL,config=jsonb_set(config,'{env.host_id}',$2),revision=revision+1 WHERE id=$1").bind(current.id).bind(json!(recipe.host)).execute(&mut *tx).await.map_err(DbError::from)?;
            crate::recovery::commit_threads(
                job,
                &mut tx,
                current.id,
                placed.runtime.generation,
                recipe.by,
                recipe.restored.contains_key("home"),
                true,
            )
            .await?;
        }
        crate::initial_files::release(&mut tx, job.id(), Time::new(now)).await?;
        if self.mode == Mode::Make {
            crate::resource_facts::library::created(
                &mut tx,
                placed.template.id,
                Some(placed.host.id),
                current.id,
                false,
                row.created_by,
                Time::new(now),
            )
            .await?;
        }
        // A new environment's copied mounts become their machine's with it.
        crate::mount_copies::commit(
            &mut tx,
            &crate::mount_copies::of_plan(&row.plan)?,
            current.id,
            job.id(),
            row.created_by.unwrap_or(current.owner_id),
            now,
        )
        .await?;
        orch_db::queries::jobs::merge_plan(
            &mut tx,
            job.id(),
            &json!({"provision_committed":true}),
            now,
        )
        .await?;
        tx.commit().await.map_err(DbError::from)?;
        Ok(Done::Finished)
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
        if let Ok(mut tx) = self.engine.db().begin().await
            && crate::initial_files::release(&mut tx, job.id(), Time::new(job.now()))
                .await
                .is_ok()
        {
            let _ = tx.commit().await;
        }
        let Ok(plan) = job.plan().await else {
            return Undone::Partly("the job's plan could not be read".to_owned());
        };
        if plan.get("after_up_started").and_then(Json::as_bool) == Some(true)
            && plan.get("portable").is_none()
            && plan.get("recovery").is_none()
        {
            return self.undo_start(job, why).await;
        }
        if plan.get("portable").is_some() {
            let _ = super::after_up::record_interrupted(job).await;
        }
        let Ok(placed) = self.placed(job).await else {
            // Nothing was scheduled: nothing was made.
            return self
                .close_environment(
                    job,
                    why,
                    plan.get("fresh").and_then(Json::as_bool).unwrap_or(true),
                )
                .await;
        };
        let mut left: Vec<String> = Vec::new();
        if let Err(why) = self.remove_unit(job, &placed, b"undo").await {
            left.push(format!(
                "the unit {} could not be removed: {why}",
                placed.runtime.unit_name
            ));
        }
        let fresh = plan.get("fresh").and_then(Json::as_bool).unwrap_or(false);
        if left.is_empty() {
            left.extend(crate::mount_copies::undo(job).await);
        }
        if (fresh || plan.get("recovery").is_some()) && left.is_empty() {
            for (role, volume) in placed.runtime.volumes.as_object().into_iter().flatten() {
                if plan
                    .pointer("/recovery/restored")
                    .and_then(Json::as_object)
                    .is_some_and(|restored| restored.contains_key(role))
                {
                    continue;
                }
                if volume.get("directory").is_some_and(Json::is_object) {
                    continue;
                }
                let name = volume["name"].as_str().unwrap_or_default().to_owned();
                let body = NodeVolumeRemove {
                    driver: placed.runtime.driver.clone(),
                    name: name.clone(),
                    storage: volume["storage"].as_str().map(str::to_owned),
                    labels: if plan.get("recovery").is_some() {
                        BTreeMap::from([
                            ("orch.environment_id".into(), self.environment.to_string()),
                            ("orch.recovery_job_id".into(), job.id().to_string()),
                        ])
                    } else {
                        BTreeMap::new()
                    },
                };
                let Ok(command) = Outgoing::new(
                    ids::derived(
                        "node.volume.remove",
                        &[job.id().as_bytes(), role.as_bytes()],
                    ),
                    placed.host.machine_id,
                    SerialKey::Volume(name.clone()),
                    &body,
                ) else {
                    continue;
                };
                match job.command_to_undo(command).await {
                    Ok(_) => {}
                    Err(Stop::Failed(failure)) if failure.code == "not_found" => {}
                    Err(Stop::Failed(failure)) => {
                        left.push(format!(
                            "the volume {name} could not be removed: {}",
                            failure.message
                        ));
                    }
                    Err(_) => left.push(format!("the volume {name} could not be removed")),
                }
            }
        }
        if left.is_empty() {
            let now = job.now();
            if let Ok(mut conn) = self.engine.inner.db.acquire().await {
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
            }
        } else if let Ok(mut conn) = self.engine.inner.db.acquire().await {
            // What is left is the reaper's: the runtime waits to be removed, with a pause that grows.
            let _ = env_rows::retire_runtime(
                &mut conn,
                placed.runtime.id,
                Some(&left.join("; ")),
                job.now(),
            )
            .await;
        }
        let outcome = self.close_environment(job, why, fresh).await;
        if left.is_empty() {
            outcome
        } else {
            Undone::Partly(left.join("; "))
        }
    }

    /// Puts the environment where a failure or a cancel leaves it, with the reason.
    async fn close_environment(&self, job: &Job, why: &Stop, fresh: bool) -> Undone {
        let phase = job
            .row()
            .await
            .ok()
            .map(|row| crate::jobs::runner::read_progress(&row).0.phase)
            .unwrap_or_default();
        let recovering = job
            .plan()
            .await
            .ok()
            .is_some_and(|plan| plan.get("recovery").is_some());
        let (mut to, detail) = match why {
            Stop::Cancelled if fresh => (status::NONE, None),
            Stop::Cancelled => (status::FAILED, Some("making it was cancelled".to_owned())),
            Stop::Failed(failure) => (
                status::FAILED,
                Some(format!("{phase}: {}", failure.message)),
            ),
            Stop::Lost => return Undone::Everything,
        };
        if recovering {
            to = "lost";
        }
        if phase == "attach"
            && matches!(why,Stop::Failed(failure) if matches!(failure.code.as_str(),"not_found"|"environment_not_found"|"workspace_missing"))
            && let Ok(mut conn) = self.engine.db().acquire().await
        {
            let _ = sqlx::query("UPDATE environments SET loss_missing=true WHERE id=$1")
                .bind(self.environment)
                .execute(&mut *conn)
                .await;
        }

        match self.engine.inner.db.acquire().await {
            Ok(mut conn) => {
                let _ = env_rows::set_status(
                    &mut conn,
                    self.environment,
                    &[status::PROVISIONING],
                    to,
                    detail.as_deref(),
                    job.now(),
                )
                .await;
                Undone::Everything
            }
            Err(error) => Undone::Partly(error.to_string()),
        }
    }

    async fn undo_start(&self, job: &Job, why: &Stop) -> Undone {
        if job
            .plan()
            .await
            .ok()
            .is_some_and(|plan| plan.get("after_up_started").and_then(Json::as_bool) == Some(true))
        {
            return super::after_up::stop_failed(job, why).await;
        }
        let Ok(placed) = self.placed(job).await else {
            return Undone::Everything;
        };
        // The unit was started for nothing: it is stopped again, and the environment stands where it did.
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
            let (runtime_to, environment_to, detail) = match why {
                Stop::Cancelled => (runtime_state::STOPPED, status::STOPPED, None),
                Stop::Failed(failure) => (
                    runtime_state::FAILED,
                    status::FAILED,
                    Some(format!("starting it: {}", failure.message)),
                ),
                Stop::Lost => return Undone::Everything,
            };
            let _ = env_rows::set_runtime_state(
                &mut conn,
                placed.runtime.id,
                &[runtime_state::CREATING, runtime_state::RUNNING],
                runtime_to,
                detail.as_deref(),
                now,
            )
            .await;
            let _ = env_rows::set_status(
                &mut conn,
                self.environment,
                &[status::PROVISIONING],
                environment_to,
                detail.as_deref(),
                now,
            )
            .await;
        }
        left.map_or(Undone::Everything, Undone::Partly)
    }
}

#[async_trait]
impl Phased for Provision {
    async fn run(&self, job: &Job, phase: &str) -> Result<Done, Stop> {
        match phase {
            "schedule" => self.schedule(job).await,
            "image" => self.image(job).await,
            "volumes" => self.volumes(job).await,
            "tools" => self.tools(job).await,
            "create" => self.create(job).await,
            "start" => self.start(job).await,
            "enroll" => self.enroll(job).await,
            "attach" => self.attach(job).await,
            "files" => {
                let placed = self.placed(job).await?;
                let machine = {
                    let mut conn = self.engine.db().acquire().await?;
                    host_rows::machine_of_runtime(&mut conn, placed.runtime.id)
                        .await?
                        .ok_or_else(|| {
                            failed("unit_failed", "the source unit's connector is unknown")
                        })?
                        .0
                };
                if let Some(done) =
                    crate::initial_files::deliver(job, placed.environment.id, machine).await?
                {
                    return Ok(done);
                }
                let plan = job.plan().await?;
                if plan.get("recovery").is_some() {
                    return crate::recovery::files(
                        job,
                        machine,
                        &placed.environment.workspace_root,
                        placed.runtime.generation,
                    )
                    .await;
                }
                crate::jobs::source::provision_files(
                    job,
                    placed.environment.id,
                    machine,
                    placed.runtime.generation,
                    placed.runtime.id,
                    placed.runtime.volumes["workspace"]["name"]
                        .as_str()
                        .unwrap_or_default(),
                    self.mode == Mode::Make
                        && plan.get("fresh").and_then(Json::as_bool).unwrap_or(false),
                )
                .await
            }
            "after_up" | "init" => super::after_up::run(job).await,
            "verify" => self.verify(job).await,
            "commit" => self.commit(job).await,
            other => Err(failed(
                INTERNAL,
                format!("a provisioning has no phase {other}"),
            )),
        }
    }

    async fn undo(&self, job: &Job, why: &Stop) -> Undone {
        if super::after_up::record_interrupted(job).await.is_err() {
            tracing::warn!("an interrupted after-start outcome could not be recorded");
        }
        match self.mode {
            Mode::Make => self.undo_make(job, why).await,
            Mode::Start => self.undo_start(job, why).await,
        }
    }

    async fn result(&self, job: &Job) -> Option<Json> {
        let plan = job.plan().await.ok()?;
        Some(json!({
            "environment": self.environment,
            "runtime": plan.get("runtime"),
            "host": plan.get("host"),
            "warnings": plan.get("warnings"),
        }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_unit_is_given_the_builds_of_the_harnesses_its_template_lets_run_there_and_of_the_one_whose_exec_server_it_offers()
     {
        let spec = json!({ "colocated_harnesses": ["claude-code", "codex"] });
        let offers = |names: &[&str]| names.iter().map(ToString::to_string).collect::<Vec<_>>();
        assert_eq!(
            harnesses_wanted(
                &spec,
                &offers(&["codex-exec-server", "claude-code", "plain"])
            ),
            ["claude-code", "codex"],
            "each once, and the exec server needs no build of its own: it is the build of Codex"
        );
        assert_eq!(
            harnesses_wanted(
                &json!({ "colocated_harnesses": [] }),
                &offers(&["codex-exec-server"])
            ),
            ["codex"],
            "a unit that offers Codex's exec server holds Codex's build whatever runs beside its tools"
        );
        assert_eq!(
            harnesses_wanted(
                &json!({ "colocated_harnesses": ["codex"] }),
                &offers(&["plain", "orch-exec"])
            ),
            ["codex"]
        );
        assert!(
            harnesses_wanted(&json!({}), &offers(&["plain"])).is_empty(),
            "a template that lists none and offers none has the connector and nothing else"
        );
        assert_eq!(
            harnesses_wanted(&json!({ "colocated_harnesses": ["codex", 7, null] }), &[]),
            ["codex"],
            "what is not a name is not a harness"
        );
    }

    #[test]
    fn every_new_unit_run_has_commands_after_files_before_verification() {
        let names = |init| {
            phases(Mode::Make, init)
                .iter()
                .map(|phase| phase.name)
                .collect::<Vec<_>>()
        };
        assert_eq!(
            names(false),
            [
                "schedule", "image", "volumes", "tools", "create", "enroll", "attach", "files",
                "after_up", "verify", "commit"
            ]
        );
        assert_eq!(names(true)[7], "files");
        assert_eq!(names(true)[8], "after_up");
        assert_eq!(
            phases(Mode::Start, false)
                .iter()
                .map(|phase| phase.name)
                .collect::<Vec<_>>(),
            [
                "start", "enroll", "attach", "files", "after_up", "verify", "commit"
            ]
        );
    }

    #[test]
    fn the_body_of_node_runtime_create_refers_to_the_token_and_never_holds_it() {
        let enrollment = Enrollment {
            secret: new_id(),
            gateway_url: "wss://gateway.orch.internal:9443".to_owned(),
            ca_cert: "-----BEGIN CERTIFICATE-----".to_owned(),
            system: "acme".to_owned(),
        };
        let runtime = new_id();
        let volumes = json!({ "workspace": { "name": "w", "storage": null } });
        let spec = runtime_spec(&SpecInputs {
            runtime,
            name: &unit_name(&runtime),
            driver: "docker",
            spec: &json!({ "image": "busybox", "env": { "A": "b", "N": 3 } }),
            limits: &json!({ "memory_mb": 2048 }),
            enrollment: &enrollment,
            tools: &[],
            volumes: &volumes,
        });
        assert_eq!(
            spec["enroll_token"],
            json!({ "$secret": enrollment.secret })
        );
        assert_eq!(spec["roles"], json!(["environment"]));
        assert_eq!(spec["env"], json!({ "A": "b" }), "only text is a variable");
        assert_eq!(spec["labels"]["orch.system"], "acme");
        assert_eq!(spec["labels"]["orch.kind"], "environment");
        assert!(
            spec["name"]
                .as_str()
                .unwrap()
                .starts_with("orch-environment-")
        );
    }

    #[test]
    fn an_image_is_named_by_its_reference_and_a_build_by_what_defines_it() {
        assert_eq!(image_key(&json!({ "image": "debian:12" })), "debian:12");
        let build = image_key(&json!({ "image": null, "build": { "dockerfile": "FROM x" } }));
        assert!(build.starts_with("build:"));
        assert_ne!(
            build,
            image_key(&json!({ "build": { "dockerfile": "FROM y" } }))
        );
    }
}
