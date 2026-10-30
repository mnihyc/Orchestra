//! Environments owned by people: creation with what is inherited, a topic's default, threads put into them by whoever manages them and their directories, what each is given (variables and secrets of its home node and its own), what it declares it needs, what its exec node offers, settings changed while it runs, lifecycle, idle stop, keep awake and recycle.
//!
//! **Making one.** [`create`] works out what an environment would be made of: the template (the
//! request's, else what its home node resolves), the values it takes from its home with its own laid
//! over them, and whether a host can take it. A host that is named and cannot is an error that names
//! the places; no other is used in its stead. The environment is then a row in status `none`, and the
//! job that makes its unit is queued in the same transaction (`jobs::provision`), so that a person who
//! asked for an environment has one being made when the answer comes.
//!
//! **Starting and stopping.** A stopped environment is started by a job of the same kind
//! ([`Engine::start_environment`]); a turn's start sequence starts one by itself and waits for it
//! ([`Engine::await_environment`]). [`Engine::stop_environment`] stops the unit and keeps it and its
//! volumes: no turn may be running in it unless the request says to interrupt them, and a credential
//! that was handed over to it is returned first (an environment whose credential cannot be brought home
//! is not stopped). **Idle** stops, **keep awake** and the removal of unused environments are the
//! schedule's (`reaper`).
//!
//! **A directory on a person's machine** is brought up by its machine. Every sweep looks at the
//! environments whose status does not follow their machine (`keep_connected`): when the
//! machine has a session that the environment has not been attached in, `env.attach` is sent (with the
//! root it was registered with and what its home node gives it, the variables and the secrets), the
//! executors its exec node offers are started, and only then is it `running`; when the machine has no
//! session it is `offline`. A connector keeps nothing across a restart, so a session that began later
//! is attached in again. A turn that needs such an environment waits for the attach
//! ([`Engine::await_environment`]) instead of being blocked, and a refused attach is a failed
//! environment that says why and is tried again after a pause and in the machine's next session.

use crate::binding;
use crate::commands::{Outcome, Outgoing};
use crate::engine::{Engine, lock};
use crate::error::EngineError;
use crate::ids;
use crate::jobs::provision::{self, Mode};
use crate::jobs::runner::MakeError;
use crate::places::{self, Ask, IDLE_MINUTES_DEFAULT, NoHost};
use crate::preflight::{Blocked, environment_state_sentence};
use crate::turns::{InputAfter, TurnEnd, error_json};
use crate::unit::Unit;
use chrono::{DateTime, Utc};
use orch_authz::Principal;
use orch_db::DbError;
use orch_db::queries::environments::hosting::{
    self as rows, Connected, EnvironmentState, NewManaged, runtime_state, status,
};
use orch_db::queries::jobs::JobRow;
use orch_db::queries::resources::hosting as host_rows;
use orch_db::queries::threads::{self, thread_state, turn_state};
use orch_link_contract::commands::{
    EnvAttach, ExecStart, IndexMode, NodeRuntimeStop, Secret, SerialKey,
};
use orch_settings::{Place as SettingsPlace, get as setting};
use orch_settings_contract::SourceScope;
use orch_settings_contract::registry::keys;
use orch_types::{Id, Json, new_id};
use serde_json::json;
use std::collections::BTreeMap;
use std::time::Duration;

/// What an environment is called when its maker names it nothing.
pub const DEFAULT_NAME: &str = "Main";
/// How long a stop that has to interrupt turns waits for them to end: the wait the engine gives a
/// harness to answer an interrupt, and a margin for the engine to end the turn by force.
const INTERRUPT_MARGIN: Duration = Duration::from_secs(5);
/// How often a stop looks again at the turns it waits for.
const STOP_LOOK_EVERY: Duration = Duration::from_millis(100);
/// What an environment says while its machine has no session.
const AWAY: &str = "its machine is offline";
/// What an environment says when its machine has a session but does not serve environments.
const NOT_SERVED: &str = "the connector on its machine does not take the role of an environment";

// ---------------------------------------------------------------- making one

/// What a person asks when they make an environment.
#[derive(Debug, Clone)]
pub struct Request<'a> {
    /// Who makes it, and owns it.
    pub owner: Id,
    /// Its name; `Main` when left out.
    pub name: Option<&'a str>,
    /// The node whose settings it takes.
    pub home: Id,
    /// The template; what the home resolves when left out.
    pub template: Option<Id>,
    /// The host: `Some(Some(host))` names one, `Some(None)` and `None` leave the choice to the
    /// scheduler or to what the home resolves.
    pub host: Option<Option<Id>>,
    /// Its own values, an object of keys of scope environment.
    pub config: &'a Json,
}

/// Why an environment could not be made.
#[derive(Debug, thiserror::Error)]
pub enum CreateError {
    /// The home resolves no template and the request names none.
    #[error("no template is named, and none is set for the place the environment is made in")]
    NoTemplate,
    /// The template cannot be used.
    #[error("{0}")]
    Template(String),
    /// The runtime chosen, or the template's own, is not one this place or this template allows.
    #[error("{0}")]
    Runtime(String),
    /// A data root is unsupported or outside this machine's own roots policy.
    #[error("{0}")]
    DataSources(String),
    /// The caller has an environment by that name in that org.
    #[error("you have an environment by that name already")]
    Taken(Option<Id>),
    /// No host takes it.
    #[error("{}", .0.message)]
    NoPlace(NoHost),
    /// The home node is not there.
    #[error("there is no such place to make it in")]
    NoHome,
    /// The system failed.
    #[error(transparent)]
    Internal(#[from] EngineError),
}

impl From<DbError> for CreateError {
    fn from(error: DbError) -> Self {
        Self::Internal(error.into())
    }
}

impl From<orch_settings::Error> for CreateError {
    fn from(error: orch_settings::Error) -> Self {
        Self::Internal(error.into())
    }
}

impl From<orch_authz::Error> for CreateError {
    fn from(error: orch_authz::Error) -> Self {
        Self::Internal(error.into())
    }
}

/// An environment that was made, with the job that makes its unit.
#[derive(Debug, Clone)]
pub struct Made {
    /// The environment.
    pub id: Id,
    /// The job that is making it.
    pub job: JobRow,
}

/// Makes an environment in the caller's transaction, owned by `request.owner`, and queues the job
/// that makes its unit. The caller has asked the access engine whether they may; what is checked
/// here is that something can be made: the template, a host.
pub async fn create(
    conn: &mut orch_db::Conn,
    now: DateTime<Utc>,
    principal: &Principal,
    request: &Request<'_>,
) -> Result<Made, CreateError> {
    let template_id: Id = match request.template {
        Some(id) => id,
        None => setting(
            conn,
            SettingsPlace::Node(request.home),
            keys::ENV_TEMPLATE_ID,
        )
        .await?
        .ok_or(CreateError::NoTemplate)?,
    };
    let template = host_rows::template(conn, template_id)
        .await?
        .ok_or_else(|| CreateError::Template("there is no such template".to_owned()))?;
    if template.deleted_at.is_some() || !template.enabled {
        return Err(CreateError::Template(
            "the template is switched off or deleted".to_owned(),
        ));
    }
    if template.kind != "environment" {
        return Err(CreateError::Template(
            "this template makes the runtimes of providers, not environments".to_owned(),
        ));
    }
    let mut own = request.config.as_object().cloned().unwrap_or_default();
    if request.template.is_some() {
        own.insert(keys::ENV_TEMPLATE_ID.to_owned(), json!(template_id));
    }
    if let Some(Some(host)) = request.host {
        own.insert(keys::ENV_HOST_ID.to_owned(), json!(host));
    }
    let own = Json::Object(own);
    let wanted = places::wanted(conn, request.home, template, &own).await?;
    if let Some(runtime) = wanted.runtime_class.as_deref() {
        // The explicit runtime, or the template's default, must be one the template permits.
        // The place's policy is checked against the actual class after selecting the host.
        crate::isolation::check_template(&wanted.template.spec, runtime)
            .map_err(CreateError::Runtime)?;
    }
    let named: Option<Id> = match request.host {
        Some(Some(host)) => Some(host),
        _ => places::resolved_for_new(conn, request.home, &own, keys::ENV_HOST_ID)
            .await?
            .and_then(|value| serde_json::from_value(value).ok()),
    };
    let minutes_idle =
        places::resolved_for_new(conn, request.home, &own, keys::ENV_RECYCLE_IDLE_MINUTES)
            .await?
            .and_then(|value| value.as_i64())
            .unwrap_or(IDLE_MINUTES_DEFAULT);
    let listed = places::host_places(
        conn,
        principal,
        &Ask {
            home: request.home,
            wanted: &wanted,
            current: None,
            minutes_idle,
            now,
        },
    )
    .await?;
    let chosen = places::choose(&listed, named).map_err(CreateError::NoPlace)?;
    // An omitted choice still uses the host's default, which the template may restrict.
    if wanted.runtime_class.is_none() {
        let descriptor = chosen
            .host
            .drivers
            .get(&wanted.template.driver)
            .unwrap_or(&Json::Null);
        let default = orch_driver_contract::isolation::class_in(descriptor, None)
            .map(|class| class.name)
            .or_else(|| {
                descriptor
                    .get("default_runtime")
                    .and_then(Json::as_str)
                    .map(str::to_owned)
            });
        if let Some(runtime) = default {
            crate::isolation::check_template(&wanted.template.spec, &runtime)
                .map_err(CreateError::Runtime)?;
        } else if wanted
            .template
            .spec
            .get("allowed_runtimes")
            .and_then(Json::as_array)
            .is_some_and(|allowed| !allowed.is_empty())
        {
            return Err(CreateError::Runtime("the host must report its default runtime before this template's allowed runtimes can be checked".to_owned()));
        }
    }
    if let Some(policy) = setting::<Json>(
        conn,
        SettingsPlace::Node(request.home),
        keys::ENVIRONMENTS_RUNTIMES,
    )
    .await?
    {
        crate::isolation::check_host_policy(
            &policy,
            chosen
                .host
                .drivers
                .get(&wanted.template.driver)
                .unwrap_or(&Json::Null),
            wanted.runtime_class.as_deref(),
        )
        .map_err(CreateError::Runtime)?;
    }
    let data_sources = places::resolved_for_new(conn, request.home, &own, keys::ENV_DATA_SOURCES)
        .await?
        .map(serde_json::from_value::<orch_driver_contract::runtime::EnvironmentDataSources>)
        .transpose()
        .map_err(|error| CreateError::DataSources(error.to_string()))?
        .unwrap_or_default();
    if data_sources.borrows_directories() {
        if named.is_none() {
            return Err(CreateError::DataSources(
                "choose a machine before choosing an existing directory there".into(),
            ));
        }
        let host = host_rows::host(conn, chosen.host.id)
            .await?
            .ok_or(CreateError::NoHome)?;
        let machine = orch_db::queries::resources::machine(conn, host.machine_id)
            .await?
            .ok_or(CreateError::NoHome)?;
        check_machine_data_sources(conn, &machine, &data_sources)
            .await
            .map_err(CreateError::DataSources)?;
        if host
            .drivers
            .get(&wanted.template.driver)
            .and_then(|driver| driver.get("data_storage"))
            .and_then(|data| data.get("existing_directory"))
            .and_then(Json::as_bool)
            != Some(true)
        {
            return Err(CreateError::DataSources(
                "this driver has not offered existing directories; choose data made by Orch".into(),
            ));
        }
    }
    if let Some(created) = &chosen.created
        && !orch_authz::environments::reserve_capacity(conn, created).await?
    {
        return Err(CreateError::NoPlace(places::capacity_reached(chosen)));
    }
    let (host_entry, template_entry) = match &chosen.created {
        Some(created) => (
            created.host.as_ref().and_then(|(entry, _)| *entry),
            created.template.as_ref().and_then(|(entry, _)| *entry),
        ),
        _ => (None, None),
    };
    let name = request.name.unwrap_or(DEFAULT_NAME);
    // A name that is taken is found before the insert, which a transaction would not survive.
    if let Some(org) = rows::root_org_of(conn, request.home).await?
        && let Some(existing) =
            orch_db::queries::environments::routes::named(conn, request.owner, org, name).await?
    {
        return Err(CreateError::Taken(Some(existing)));
    }
    let id = new_id();
    let made = rows::insert_managed(
        conn,
        &NewManaged {
            id,
            name,
            owner_id: request.owner,
            home_node_id: request.home,
            template_id,
            config: &own,
            host_entry_id: host_entry,
            template_entry_id: template_entry,
        },
        now,
    )
    .await;
    match made {
        Ok(true) => {}
        Ok(false) => return Err(CreateError::NoHome),
        Err(DbError::Unique { .. }) => {
            let org = rows::root_org_of(conn, request.home).await?;
            let existing = match org {
                Some(org) => {
                    orch_db::queries::environments::routes::named(conn, request.owner, org, name)
                        .await?
                }
                None => None,
            };
            return Err(CreateError::Taken(existing));
        }
        Err(error) => return Err(error.into()),
    }
    let state = rows::environment(conn, id)
        .await?
        .ok_or(CreateError::NoHome)?;
    let job = match provision::queue(conn, now, &state, Mode::Make, Some(request.owner)).await {
        Ok(job) => job,
        Err(MakeError::Db(error)) => return Err(error.into()),
        Err(other) => {
            return Err(CreateError::Internal(EngineError::unreadable(format!(
                "a job for a new environment could not be queued: {other}"
            ))));
        }
    };
    crate::temporary_environments::ensure(conn, id, now).await?;
    Ok(Made { id, job })
}

/// The controller can reject an obviously outside path from reported policy. The machine always
/// resolves and checks it again itself; the controller never resolves another machine's paths.
pub async fn check_machine_data_sources(
    conn: &mut orch_db::Conn,
    machine: &orch_db::queries::resources::MachineRow,
    sources: &orch_driver_contract::runtime::EnvironmentDataSources,
) -> Result<(), String> {
    if !sources.borrows_directories() {
        return Ok(());
    }
    let reported = orch_db::queries::environment_facts::machine(conn, machine.id, None)
        .await
        .map_err(|error| error.to_string())?;
    let policy: orch_link_contract::policy::LocalPolicy = reported
        .and_then(|facts| facts.policy)
        .filter(|value| !value.is_null())
        .ok_or_else(|| "this machine has not reported any allowed environment.roots".to_owned())
        .and_then(|value| {
            serde_json::from_value(value)
                .map_err(|_| "the machine's roots policy is unavailable".to_owned())
        })?;
    for path in [sources.workspace.directory(), sources.home.directory()]
        .into_iter()
        .flatten()
    {
        policy.check_root(std::path::Path::new(path)).map_err(|_| format!("{path} is outside this machine's environment.roots; its owners choose the allowed directories"))?;
    }
    Ok(())
}

// ---------------------------------------------------------------- starting

/// What starting an environment came to.
#[derive(Debug, Clone)]
pub enum Starting {
    /// It is up already.
    Up,
    /// A job makes it ready: the one just queued, or the one that was under way.
    Job(Box<JobRow>),
}

/// Why an environment cannot be started.
#[derive(Debug, thiserror::Error)]
pub enum StartError {
    /// There is no such environment.
    #[error("there is no such environment")]
    NotFound,
    /// It is a directory on a person's machine: there is no unit to start.
    #[error("this environment is a directory on a machine, which is not started from here")]
    NotManaged,
    /// Its removal is scheduled.
    #[error("its removal is scheduled")]
    BeingRemoved,
    /// It is in a state that is not started from.
    #[error("the environment is {0}")]
    Unavailable(String),
    /// The system failed.
    #[error(transparent)]
    Internal(#[from] EngineError),
}

impl From<DbError> for StartError {
    fn from(error: DbError) -> Self {
        Self::Internal(error.into())
    }
}

/// Starts an environment in the caller's transaction: nothing for one that is up, the job in progress
/// for one that is being made, a new job for one that is `none`, `failed` or `stopped`.
pub async fn start(
    conn: &mut orch_db::Conn,
    now: DateTime<Utc>,
    environment: Id,
    by: Option<Id>,
) -> Result<Starting, StartError> {
    let state = rows::lock_environment(conn, environment)
        .await?
        .filter(|state| state.deleted_at.is_none())
        .ok_or(StartError::NotFound)?;
    if !orch_db::queries::environment_facts::has_unit(conn, environment).await? {
        return Err(StartError::NotManaged);
    }
    if state.remove_at.is_some() {
        return Err(StartError::BeingRemoved);
    }
    if rows::live_runtime(conn, environment)
        .await?
        .is_some_and(|runtime| runtime.state == "stopping")
    {
        return Err(StartError::Unavailable(
            "the host has not confirmed the previous stop".to_owned(),
        ));
    }
    if let Some(runtime) = rows::live_runtime(conn, environment).await?
        && let Some(blocked) = crate::network::blocked(conn, runtime.id).await?
    {
        return Err(StartError::Unavailable(
            blocked.detail.unwrap_or_else(|| blocked.reason.to_owned()),
        ));
    }
    let mode = match state.status.as_str() {
        status::RUNNING => return Ok(Starting::Up),
        status::NONE | status::FAILED => Mode::Make,
        status::STOPPED => {
            if rows::live_runtime(conn, environment).await?.is_some() {
                Mode::Start
            } else {
                Mode::Make
            }
        }
        status::PROVISIONING => {
            return Ok(
                match orch_db::queries::jobs::open_of_environment(conn, environment)
                    .await?
                    .into_iter()
                    .next()
                {
                    Some(job) => Starting::Job(Box::new(job)),
                    None => Starting::Up,
                },
            );
        }
        other => return Err(StartError::Unavailable(other.to_owned())),
    };
    match provision::queue(conn, now, &state, mode, by).await {
        Ok(job) => Ok(Starting::Job(Box::new(job))),
        Err(MakeError::Running(job)) => Ok(Starting::Job(job)),
        Err(MakeError::NoEnvironment) => Err(StartError::NotFound),
        Err(MakeError::Db(error)) => Err(error.into()),
    }
}

/// What a turn's start sequence does about its environment.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Awaited {
    /// The environment is up, or is not one this part brings up: go on.
    Ready,
    /// The environment is being brought up: the thread says so and the sequence is taken up again.
    Waiting,
    /// Bringing it up failed: the turn's start ended.
    Failed,
}

/// Where an environment stands for work about to be done in it ([`Engine::environment_standing`]).
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Standing {
    /// Work can be done in it now, or it is of a kind this part does not bring up.
    Ready,
    /// It is being started, made or attached; whoever waits for it is brought back when it is up.
    Waiting,
    /// It cannot be had now, for this reason, in words a person reads.
    Unavailable(String),
}

// ---------------------------------------------------------------- stopping

/// Why an environment cannot be stopped.
#[derive(Debug, thiserror::Error)]
pub enum StopError {
    /// There is no such environment.
    #[error("there is no such environment")]
    NotFound,
    /// It is a directory on a person's machine.
    #[error("this environment is a directory on a machine, which is not stopped from here")]
    NotManaged,
    /// A turn runs in it, and the request did not say to interrupt.
    #[error("{0} turns run in it")]
    Busy(i64),
    /// A job holds it.
    #[error("the environment is {0}")]
    Unavailable(String),
    /// A credential handed over to it could not be returned first.
    #[error("a credential that was handed over to it could not be returned: {0}")]
    CredentialReturnFailed(String),
    /// The host could not stop the unit.
    #[error("the host could not stop it: {0}")]
    Host(String),
    /// The system failed.
    #[error(transparent)]
    Internal(#[from] EngineError),
}

impl From<DbError> for StopError {
    fn from(error: DbError) -> Self {
        Self::Internal(error.into())
    }
}

/// Why the unit is being stopped.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StopReason {
    /// A person asked.
    Person(Id),
    /// It had no activity for as long as its setting allows.
    Idle,
    /// A recycle by a person.
    Recycle(Id),
}

impl Engine {
    /// Starts an environment in a transaction of its own and sees that the job is taken up.
    pub async fn start_environment(
        &self,
        environment: Id,
        by: Option<Id>,
    ) -> Result<Starting, StartError> {
        let tx = self.inner.db.begin().await?;
        self.start_environment_at(tx, environment, by, None, self.inner.clock.now())
            .await
    }

    /// Queues the start inside the request's existing guard, with its current caller and time.
    pub async fn start_environment_in(
        &self,
        tx: orch_db::Tx,
        principal: &orch_authz::Principal,
        environment: Id,
    ) -> Result<Starting, StartError> {
        self.start_environment_at(
            tx,
            environment,
            Some(principal.user()),
            Some(principal),
            principal.now().get(),
        )
        .await
    }

    async fn start_environment_at(
        &self,
        mut tx: orch_db::Tx,
        environment: Id,
        by: Option<Id>,
        principal: Option<&orch_authz::Principal>,
        now: DateTime<Utc>,
    ) -> Result<Starting, StartError> {
        // Runtime updates: an owner-requested wake updates before starting the old unit.
        let started = match self
            .update_on_start(&mut tx, environment, principal, now)
            .await?
        {
            Some(job) => Starting::Job(Box::new(job)),
            None => start(&mut tx, now, environment, by).await?,
        };
        tx.commit().await.map_err(DbError::from)?;
        if matches!(started, Starting::Job(_)) {
            self.kick();
        }
        Ok(started)
    }

    /// Says that something was done in an environment now: the clock of its idle stop starts over.
    pub async fn keep_awake(&self, environment: Id) -> Result<bool, EngineError> {
        let mut conn = self.inner.db.acquire().await?;
        Ok(rows::keep_awake(&mut conn, environment, self.inner.clock.now()).await?)
    }

    /// What a turn's start sequence does about the environment its thread runs in, as
    /// `Engine::environment_standing` decides it. One that is up lets the sequence go on. One that is
    /// being started, made or attached is waited for: the thread says that it waits for the environment,
    /// and the sweep brings the sequence back each second. One that cannot be had ends the turn's start,
    /// which blocks the thread as `environment_unavailable` with the reason and keeps the input.
    pub async fn await_environment(&self, thread: Id) -> Result<Awaited, EngineError> {
        let (turn, environment) = {
            let mut conn = self.inner.db.acquire().await?;
            let Some(row) = threads::thread(&mut conn, thread).await? else {
                return Ok(Awaited::Ready);
            };
            let Some(environment) = row.environment_id else {
                return Ok(Awaited::Ready);
            };
            if row.state != thread_state::STARTING {
                return Ok(Awaited::Ready);
            }
            let Some(turn) = threads::active_turn(&mut conn, thread).await? else {
                return Ok(Awaited::Ready);
            };
            if turn.state != turn_state::STARTING {
                return Ok(Awaited::Ready);
            }
            (turn.id, environment)
        };
        match self.environment_standing(environment).await? {
            Standing::Ready => Ok(Awaited::Ready),
            Standing::Waiting => {
                self.set_step(thread, "environment").await?;
                Ok(Awaited::Waiting)
            }
            Standing::Unavailable(why) => {
                self.fail_turn_start(thread, turn, &why).await?;
                Ok(Awaited::Failed)
            }
        }
    }

    /// Where `environment` stands for work about to be done in it: the one decision the start of a turn
    /// ([`Engine::await_environment`]) and the preparation of an input before its turn
    /// (`drive_preparation`) take, so that the two never disagree about one environment (C-turns: the
    /// preparation said "unavailable" of an environment the start took as ready, and the input went round
    /// between pre-flight and preparation about fifteen times a second). One that is up is ready, and so
    /// is one of a kind this part does not bring up (the command that does the work says what it finds).
    /// One that is `none` or `stopped` is started and one that is being made is waited for; a directory on
    /// a person's machine is waited for while its machine attaches it. One whose making failed, whose
    /// removal is scheduled, or whose machine is away or does not take environments cannot be had.
    pub(crate) async fn environment_standing(
        &self,
        environment: Id,
    ) -> Result<Standing, EngineError> {
        let (state, machine_facts) = {
            let mut conn = self.inner.db.acquire().await?;
            (
                rows::environment(&mut conn, environment).await?,
                orch_db::queries::environment_facts::of(&mut conn, environment).await?,
            )
        };
        let Some(state) = state else {
            return Ok(Standing::Ready);
        };
        if machine_facts.as_ref().is_some_and(|m| m.attaches()) {
            return self.attach_standing(environment).await;
        }
        if !machine_facts
            .as_ref()
            .and_then(|m| m.descriptor.as_ref())
            .is_some_and(|d| d.get("lifecycle").and_then(Json::as_str) == Some("unit"))
        {
            return Ok(Standing::Ready);
        }
        match state.status.as_str() {
            status::RUNNING => {
                let mut conn = self.inner.db.acquire().await?;
                orch_db::queries::environments::thread_engine::touch_activity(
                    &mut conn,
                    environment,
                    self.inner.clock.now(),
                )
                .await?;
                Ok(Standing::Ready)
            }
            status::NONE | status::STOPPED | status::PROVISIONING | "recreating" | "moving" => {
                if state.remove_at.is_some() {
                    return Ok(Standing::Unavailable("its removal is scheduled".to_owned()));
                }
                match self.start_environment(environment, None).await {
                    Ok(_) => Ok(Standing::Waiting),
                    Err(StartError::Internal(error)) => Err(error),
                    Err(other) => Ok(Standing::Unavailable(other.to_string())),
                }
            }
            status::FAILED => Ok(Standing::Unavailable(environment_state_sentence(
                status::FAILED,
                state.status_detail.as_deref(),
            ))),
            _ => Ok(Standing::Ready),
        }
    }

    /// Ends the start of a turn whose environment cannot be brought up: the turn fails, its input waits
    /// again where it stood, and the thread is blocked as `environment_unavailable` with the reason. A
    /// person starts the environment again, or puts the thread elsewhere.
    async fn fail_turn_start(
        &self,
        thread_id: Id,
        turn_id: Id,
        why: &str,
    ) -> Result<(), EngineError> {
        let message = format!("the environment is not available: {why}");
        let mut tx = self.inner.db.begin().await?;
        let finished = {
            let mut unit = Unit::new(&mut tx, self.inner.clock.now());
            let Some(thread) = threads::lock(&mut *unit.conn, thread_id).await? else {
                return Ok(());
            };
            let Some(turn) = threads::turn(&mut *unit.conn, turn_id).await? else {
                return Ok(());
            };
            if turn.state != turn_state::STARTING {
                return Ok(());
            }
            crate::ingest::add_error_item(
                &mut unit,
                thread_id,
                turn.segment_id,
                Some(turn.id),
                "environment_unavailable",
                &message,
                true,
            )
            .await?;
            self.end_turn(
                &mut unit,
                &thread,
                &turn,
                TurnEnd {
                    state: turn_state::FAILED,
                    error: Some(error_json("environment_unavailable", &message, true)),
                    input: InputAfter::Retry { retryable: true },
                    then_block: Some(Blocked {
                        reason: "environment_unavailable",
                        detail: Some(why.to_owned()),
                    }),
                },
            )
            .await?;
            unit.finish().await?
        };
        self.commit(tx, finished).await?;
        Ok(())
    }

    /// Stops the unit of an environment and keeps it with its volumes. No turn may run in it, unless
    /// `interrupt` says that they are interrupted first. The shells in it are closed with the unit.
    pub async fn stop_environment(
        &self,
        environment: Id,
        interrupt: bool,
        reason: StopReason,
    ) -> Result<(), StopError> {
        let state = {
            let mut conn = self.inner.db.acquire().await?;
            rows::environment(&mut conn, environment)
                .await?
                .filter(|state| state.deleted_at.is_none())
                .ok_or(StopError::NotFound)?
        };
        let res = {
            let mut conn = self.inner.db.acquire().await?;
            !orch_db::queries::environment_facts::has_unit(&mut conn, environment).await?
        };
        if res {
            return Err(StopError::NotManaged);
        }
        // Runtime updates and moves use the same hold as thread switches.
        {
            let mut conn = self.inner.db.acquire().await?;
            if let Some(job) =
                orch_db::queries::jobs::changes::environment_hold(&mut conn, environment).await?
            {
                return Err(StopError::Unavailable(format!("held by job {}", job.id)));
            }
        }
        match state.status.as_str() {
            status::STOPPED | status::NONE => return Ok(()),
            status::RUNNING | status::OFFLINE => {}
            other => return Err(StopError::Unavailable(other.to_owned())),
        }
        if reason == StopReason::Idle {
            // Whoever found it idle looked a while ago, and what is done in it is written down as it is
            // done: it is stopped for being idle if it still is by what is stored now.
            let mut conn = self.inner.db.acquire().await?;
            let due = idle_stop_at(&mut conn, &state).await?;
            if due.is_none_or(|at| at > self.inner.clock.now()) {
                return Ok(());
            }
        }
        self.clear_turns(environment, interrupt, reason).await?;
        // A credential that was handed over is brought home before the unit stops, as the first thing
        // that is done; one that cannot be leaves the environment as it is (the part that keeps
        // providers).
        self.return_credentials_of(environment)
            .await
            .map_err(|error| match error {
                crate::credentials::handover::ReturnError::Busy(running) => {
                    StopError::Busy(running)
                }
                crate::credentials::handover::ReturnError::Internal(error) => {
                    StopError::Internal(error)
                }
                other => StopError::CredentialReturnFailed(other.to_string()),
            })?;
        let runtime = {
            let mut conn = self.inner.db.acquire().await?;
            rows::live_runtime(&mut conn, environment).await?
        };
        let Some(runtime) = runtime else {
            // No unit: only the record stands.
            let mut conn = self.inner.db.acquire().await?;
            rows::set_status(
                &mut conn,
                environment,
                &[status::RUNNING, status::OFFLINE],
                status::STOPPED,
                None,
                self.inner.clock.now(),
            )
            .await?;
            return Ok(());
        };
        let began = {
            let mut conn = self.inner.db.acquire().await?;
            rows::set_runtime_state(
                &mut conn,
                runtime.id,
                &[runtime_state::RUNNING, runtime_state::STOPPING],
                runtime_state::STOPPING,
                None,
                self.inner.clock.now(),
            )
            .await?
        };
        if !began {
            return Err(StopError::Unavailable(runtime.state));
        }
        self.finish_stop(environment, runtime.id).await
    }

    /// Carries a stop through: the host stops the unit, and the rows say it. A stop that a process that
    /// died began is finished by the schedule with this.
    pub(crate) async fn finish_stop(&self, environment: Id, runtime: Id) -> Result<(), StopError> {
        let (row, host) = {
            let mut conn = self.inner.db.acquire().await?;
            let Some(row) = rows::runtime(&mut conn, runtime).await? else {
                return Ok(());
            };
            let host = host_rows::host(&mut conn, row.host_id).await?;
            (row, host)
        };
        let Some(host) = host else {
            return Err(StopError::Host("the host is gone".to_owned()));
        };
        let stamp = row.updated_at.timestamp_micros().to_le_bytes();
        let command = Outgoing::new(
            ids::derived(
                "node.runtime.stop",
                &[runtime.as_bytes().as_slice(), &stamp],
            ),
            host.machine_id,
            SerialKey::Runtime(runtime),
            &NodeRuntimeStop {
                runtime_id: runtime,
                reason: None,
            },
        )
        .map_err(EngineError::from)?;
        let id = command.id;
        self.inner
            .commands
            .issue(command)
            .await
            .map_err(EngineError::from)?;
        let waited = tokio::time::timeout(
            self.inner.config.hosting.stop_wait,
            self.inner.commands.wait(id),
        )
        .await;
        let now = self.inner.clock.now();
        let mut conn = self.inner.db.acquire().await?;
        match waited {
            Ok(Ok(crate::commands::Outcome::Succeeded(_))) => {
                rows::set_runtime_state(
                    &mut conn,
                    runtime,
                    &[runtime_state::STOPPING],
                    runtime_state::STOPPED,
                    None,
                    now,
                )
                .await?;
                rows::set_status(
                    &mut conn,
                    environment,
                    &[status::RUNNING, status::OFFLINE],
                    status::STOPPED,
                    None,
                    now,
                )
                .await?;
                rows::close_shells(&mut conn, environment, now).await?;
                Ok(())
            }
            Ok(Ok(crate::commands::Outcome::Failed(error))) => {
                rows::set_runtime_state(
                    &mut conn,
                    runtime,
                    &[runtime_state::STOPPING],
                    runtime_state::RUNNING,
                    None,
                    now,
                )
                .await?;
                Err(StopError::Host(format!("{}: {}", error.code, error.detail)))
            }
            Ok(Ok(_)) | Ok(Err(_)) | Err(_) => {
                // The host has not said: the unit stands `stopping` and the schedule asks again.
                Err(StopError::Host(
                    "the host did not answer in time; the stop is tried again".to_owned(),
                ))
            }
        }
    }

    /// Makes sure no turn runs in an environment: it fails when there is one and the request did not
    /// say to interrupt; otherwise the turns are asked to stop, and waited for.
    async fn clear_turns(
        &self,
        environment: Id,
        interrupt: bool,
        reason: StopReason,
    ) -> Result<(), StopError> {
        let running = {
            let mut conn = self.inner.db.acquire().await?;
            rows::active_turns(&mut conn, environment).await?
        };
        if running == 0 {
            return Ok(());
        }
        let by = match reason {
            StopReason::Person(by) | StopReason::Recycle(by) if interrupt => by,
            _ => return Err(StopError::Busy(running)),
        };
        let threads_of = {
            let mut conn = self.inner.db.acquire().await?;
            rows::threads_of(&mut conn, environment).await?
        };
        for thread in threads_of {
            let mut tx = self.inner.db.begin().await?;
            let finished = {
                let mut unit = Unit::new(&mut tx, self.inner.clock.now());
                if threads::lock(&mut *unit.conn, thread).await?.is_none() {
                    continue;
                }
                if let Some(turn) = threads::active_turn(&mut *unit.conn, thread).await? {
                    let seq = unit.stamp(thread).await?;
                    // The turn is cut because its environment changes, not by a person (T8).
                    threads::set_interrupt_cause(&mut *unit.conn, turn.id, "environment_change")
                        .await?;
                    threads::mark_interrupted_by(&mut *unit.conn, turn.id, by, seq).await?;
                    unit.wake(thread);
                }
                unit.finish().await?
            };
            self.commit(tx, finished).await?;
            self.drive_interrupt(thread).await?;
        }
        let wait = self.inner.config.interrupt_force + INTERRUPT_MARGIN;
        let began = tokio::time::Instant::now();
        loop {
            let left = {
                let mut conn = self.inner.db.acquire().await?;
                rows::active_turns(&mut conn, environment).await?
            };
            if left == 0 {
                return Ok(());
            }
            if began.elapsed() >= wait {
                return Err(StopError::Busy(left));
            }
            tokio::time::sleep(STOP_LOOK_EVERY).await;
        }
    }
}

// ---------------------------------------------------------------- what an attach carries

/// What an `env.attach` carries of an environment besides where it is: what its home node and it give
/// its commands, and the executors to start once it is attached.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct AttachWith {
    pub(crate) resources: Option<orch_driver_contract::runtime::Resources>,
    /// The current owned directory/store generation, retained across reconnects.
    pub(crate) data_generation: Option<i64>,
    /// Commands for this attachment, in the same name order and journal as a unit start.
    pub(crate) after_up: Vec<orch_settings_contract::unit::NamedAfterUpCommand>,
    pub(crate) storage: Option<orch_settings_contract::unit::UnitStorage>,
    pub(crate) data_sources: Option<orch_driver_contract::runtime::EnvironmentDataSources>,
    /// Distinguishes the registry's new-data default from an explicit own or inherited choice.
    pub(crate) data_sources_scope: SourceScope,
    /// Mounts requested from a connected machine, already resolved by scope and location.
    pub(crate) mounts: Vec<orch_link_contract::commands::EnvAttachMount>,
    /// The variables its commands find in their environment: the resolved `env.variables`.
    pub(crate) variables: BTreeMap<String, String>,
    /// The secrets its commands find, by variable name, as references to the stored secrets (the link
    /// seals each to the key of the machine when the command is written).
    pub(crate) secrets: BTreeMap<String, Secret>,
    /// The executors to start: what its exec node offers, and any other that a thread bound to it
    /// works through.
    pub(crate) executors: Vec<String>,
    /// Whether the machine indexes the whole tree for use while it is offline, or only lists it when asked:
    /// the resolved `workspace.index`.
    pub(crate) index: IndexMode,
}

impl Default for AttachWith {
    fn default() -> Self {
        Self {
            resources: None,
            data_generation: None,
            after_up: Vec::new(),
            storage: None,
            data_sources: None,
            data_sources_scope: SourceScope::Default,
            mounts: Vec::new(),
            variables: BTreeMap::new(),
            secrets: BTreeMap::new(),
            executors: Vec::new(),
            // The same fallback as attach_with when workspace.index has not been set.
            index: IndexMode::Full,
        }
    }
}

/// Works out what an attach carries, from what the environment and its home node give it.
pub(crate) async fn attach_with(
    conn: &mut orch_db::Conn,
    environment: Id,
    at: DateTime<Utc>,
) -> Result<AttachWith, EngineError> {
    let mounts = crate::mounts::resolve(conn, environment, at)
        .await?
        .into_iter()
        .filter_map(|mount| mount.attach)
        .collect();
    let variables = setting::<BTreeMap<String, String>>(
        conn,
        SettingsPlace::Environment(environment),
        keys::ENV_VARIABLES,
    )
    .await?
    .unwrap_or_default();
    let secrets = secret_references(conn, environment).await?;
    // What an exec node offers is told in the words of the offers; the connector starts executors by their kind.
    let mut executors: Vec<String> = Vec::new();
    let offered = binding::offers_of(conn, environment).await?;
    let worked_through = rows::executors_of(conn, environment).await?;
    let programs = orch_agent_programs::Programs::builtin();
    for offer in offered {
        // An offer names an executor, or a tool set of one that has several (`orch-exec:plain`, or `plain` as
        // offers were written before they named their executor); the connector starts executors by their kind.
        let executor = programs.offered_executor(&offer);
        if !executors.contains(&executor) {
            executors.push(executor);
        }
    }
    for executor in worked_through {
        if !executors.contains(&executor) {
            executors.push(executor);
        }
    }
    let index = setting::<IndexMode>(
        conn,
        SettingsPlace::Environment(environment),
        keys::WORKSPACE_INDEX,
    )
    .await?
    .unwrap_or(IndexMode::Full);
    let sources = orch_settings::resolve(
        conn,
        SettingsPlace::Environment(environment),
        keys::ENV_DATA_SOURCES,
    )
    .await?
    .resolved;
    let data_sources_scope = sources.source.scope;
    let data_sources = sources.value.map(serde_json::from_value).transpose()?;
    Ok(AttachWith {
        resources: setting(
            conn,
            SettingsPlace::Environment(environment),
            keys::ENV_RESOURCES,
        )
        .await?,
        data_generation: rows::environment(conn, environment)
            .await?
            .map(|row| i64::from(row.generation)),
        after_up: {
            let scoped: Json = setting(
                conn,
                SettingsPlace::Environment(environment),
                keys::ENV_AFTER_UP,
            )
            .await?
            .unwrap_or_else(|| json!({}));
            orch_settings_contract::unit::setting_commands(&scoped)
                .map_err(EngineError::unreadable)?
                .into_iter()
                .filter_map(|(name, run)| {
                    run.map(|run| orch_settings_contract::unit::NamedAfterUpCommand::new(name, run))
                })
                .collect()
        },
        storage: setting::<orch_settings_contract::unit::UnitStorage>(
            conn,
            SettingsPlace::Environment(environment),
            keys::ENV_STORAGE,
        )
        .await?
        .filter(|storage| *storage != Default::default()),
        data_sources,
        data_sources_scope,
        mounts,
        variables,
        secrets,
        executors,
        index,
    })
}

/// The secrets an environment's commands are to find, by variable name, as references to the stored
/// secrets: the nearest owner's when two have a name.
async fn secret_references(
    conn: &mut orch_db::Conn,
    environment: Id,
) -> Result<BTreeMap<String, Secret>, EngineError> {
    let wanted: Vec<String> = setting(
        conn,
        SettingsPlace::Environment(environment),
        keys::ENV_SECRET_NAMES,
    )
    .await?
    .unwrap_or_default();
    if wanted.is_empty() {
        return Ok(BTreeMap::new());
    }
    let listed = orch_settings::secrets::list(
        conn,
        orch_settings::secrets::Owner::Environment(environment),
    )
    .await
    .map_err(EngineError::unreadable)?;
    let mut references = BTreeMap::new();
    for info in listed
        .into_iter()
        .filter(|info| wanted.contains(&info.name))
    {
        let (owner_kind, owner) = match info.node {
            Some(node) => ("setting", node),
            None => ("environment", environment),
        };
        if let Some(row) =
            orch_db::queries::settings::secret(conn, owner_kind, owner, &info.name).await?
        {
            references.insert(info.name, Secret::Ref(row.id));
        }
    }
    Ok(references)
}

/// The body of `env.attach`. A directory on a person's machine is attached with the root it was
/// registered with (its machine checks that the root lies where its owner allows); a managed
/// environment is attached with none, since the driver gave its unit the places it uses.
pub(crate) fn attach_body(environment: Id, root: Option<&str>, with: &AttachWith) -> EnvAttach {
    // A registry default for newly made data cannot replace a registered existing root.
    // An explicit empty selection still means MadeByOrch; only provenance distinguishes it.
    let data_sources = if root.is_some_and(|root| !root.is_empty())
        && with.data_sources_scope == SourceScope::Default
    {
        None
    } else {
        with.data_sources.clone()
    };
    EnvAttach {
        resources: with.resources,
        data_generation: with.data_generation,
        storage: with.storage.clone(),
        data_sources,
        environment_id: environment,
        workspace_root: root.map(str::to_owned),
        ignore: Vec::new(),
        index: with.index,
        mounts: root.map(|_| with.mounts.clone()),
        variables: with.variables.clone(),
        secrets: with.secrets.clone(),
    }
}

// ---------------------------------------------------------------- directories on people's machines

impl Engine {
    /// Brings the status of the environments on people's machines in step with their machines, which
    /// the sweep does each time it comes round: one whose machine has a session that it has not been
    /// attached in is attached; one whose machine has none is `offline`.
    pub(crate) async fn keep_connected(&self) {
        if let Err(error) = self.follow_machines().await {
            tracing::warn!(%error, "the environments on people's machines could not be brought in step with them; the next sweep looks again");
        }
    }

    async fn follow_machines(&self) -> Result<(), EngineError> {
        let now = self.inner.clock.now();
        let pause =
            chrono::Duration::from_std(self.inner.config.hosting.attach_retry).unwrap_or_default();
        let due = {
            let mut conn = self.inner.db.acquire().await?;
            rows::connected_out_of_step(&mut conn, now - pause, self.inner.config.sweep_batch)
                .await?
        };
        for item in due {
            if !item.here {
                let mut conn = self.inner.db.acquire().await?;
                rows::set_status(
                    &mut conn,
                    item.id,
                    &[status::RUNNING],
                    status::OFFLINE,
                    Some(AWAY),
                    now,
                )
                .await?;
            } else if !item.serves {
                let mut conn = self.inner.db.acquire().await?;
                rows::connected_attach_failed(&mut conn, item.id, item.session, NOT_SERVED, now)
                    .await?;
            } else {
                self.attach_in_background(item);
            }
        }
        Ok(())
    }

    /// Attaches one environment in a task of its own, unless this process is at it already: the wait
    /// for a connector is not the sweep's.
    fn attach_in_background(&self, item: Connected) {
        if !lock(&self.inner.attaching).insert(item.id) {
            return;
        }
        let engine = self.clone();
        tokio::spawn(async move {
            if !engine.is_stopped()
                && let Err(error) = engine.attach_connected(&item).await
            {
                tracing::warn!(%error, environment = %item.id, "an environment on a person's machine could not be attached; the next sweep looks again");
            }
            lock(&engine.inner.attaching).remove(&item.id);
        });
    }

    /// Sends `env.attach` to the machine of a directory, starts the executors its exec node offers, and
    /// then says it is `running`: from the answer, with the facts the connector gave. A machine that
    /// refuses, or does not answer, makes it `failed` with what it said.
    async fn attach_connected(&self, item: &Connected) -> Result<(), EngineError> {
        let mut with = {
            let mut conn = self.inner.db.acquire().await?;
            attach_with(&mut conn, item.id, self.inner.clock.now()).await?
        };
        let mut skipped = None;
        if !with.after_up.is_empty() {
            let facts = {
                let mut conn = self.inner.db.acquire().await?;
                orch_db::queries::environment_facts::of(&mut conn, item.id).await?
            };
            match facts.as_ref().map(crate::jobs::after_up::admission) {
                Some(Ok(true)) => {}
                Some(Ok(false)) => {
                    with.after_up.clear();
                    skipped = Some(crate::jobs::after_up::MACHINE_POLICY_OFF.to_owned());
                }
                Some(Err(why)) => return self.attach_refused(item, &why).await,
                None => {
                    return self
                        .attach_refused(
                            item,
                            "the machine has not reported support for commands after start",
                        )
                        .await;
                }
            }
        }
        // The identifier names the session, so that a process that asks again in the same session asks
        // for the same command; a retry after a refusal names the moment of the refusal, so that it is
        // a command of its own.
        let session = item.session.map_or(0, |at| at.timestamp_micros());
        let attempt = if item.status == status::FAILED {
            item.updated_at.timestamp_micros()
        } else {
            0
        };
        let id = ids::derived(
            "env.attach",
            &[
                item.id.as_bytes().as_slice(),
                &session.to_le_bytes(),
                &attempt.to_le_bytes(),
            ],
        );
        let key = SerialKey::Environment(item.id);
        let body = attach_body(item.id, Some(&item.workspace_root), &with);
        let attach = Outgoing::new(id, item.machine_id, key.clone(), &body)?;
        let mut facts = match self.ask_bounded(attach).await? {
            Some(Outcome::Succeeded(facts)) => facts,
            Some(Outcome::Failed(error)) => {
                return self
                    .attach_refused(item, &format!("its machine refused it: {error}"))
                    .await;
            }
            Some(Outcome::Expired | Outcome::Cancelled) | None => {
                return self
                    .attach_refused(item, "its machine did not take it in time")
                    .await;
            }
        };
        if let Some(reason) = skipped {
            facts["after_up"] = json!({"state":"skipped","reason":reason});
        }
        for (position, entry) in with.after_up.iter().enumerate() {
            let position = u32::try_from(position)
                .map_err(|_| EngineError::unreadable("too many commands after attach"))?;
            let body = orch_link_contract::commands::UnitAfterUp {
                run_id: id,
                environment_id: Some(item.id),
                provider_id: None,
                agent_id: None,
                position,
                name: entry.name.clone(),
                run: orch_settings_contract::unit::AfterUpCommand {
                    command: entry.command.clone(),
                    timeout_secs: entry.timeout_secs,
                },
            };
            let outgoing = Outgoing::new(
                ids::derived("unit.after_up", &[id.as_bytes(), &position.to_be_bytes()]),
                item.machine_id,
                key.clone(),
                &body,
            )?;
            let result = Some(crate::commands::run(self.inner.commands.as_ref(), outgoing).await?);
            if matches!(&result,Some(Outcome::Failed(error)) if error.details.as_ref().and_then(|details|details.get("policy_key")).and_then(Json::as_str)==Some("environment.after_up"))
            {
                facts["after_up"] =
                    json!({"state":"skipped","reason":crate::jobs::after_up::MACHINE_POLICY_OFF});
                break;
            }
            let ended = match &result {
                Some(Outcome::Succeeded(value)) => serde_json::from_value::<
                    orch_link_contract::commands::UnitAfterUpResult,
                >(value.clone())
                .ok(),
                _ => None,
            };
            crate::jobs::after_up::record_direct(self, &body, ended.as_ref()).await?;
            if !ended
                .as_ref()
                .is_some_and(|result| result.name == body.name && result.passed())
            {
                let diagnostic = ended
                    .as_ref()
                    .map(|result| {
                        result
                            .first_lines
                            .lines()
                            .take(10)
                            .collect::<Vec<_>>()
                            .join("\n")
                            .chars()
                            .take(1000)
                            .collect::<String>()
                    })
                    .unwrap_or_default();
                return self.attach_refused(item,&format!("after-start command {} failed or was refused by this machine: {diagnostic}",entry.name)).await;
            }
        }
        for executor in &with.executors {
            let start = Outgoing::new(
                ids::derived(
                    "exec.start",
                    &[id.as_bytes().as_slice(), executor.as_bytes()],
                ),
                item.machine_id,
                key.clone(),
                &ExecStart {
                    environment_id: item.id,
                    executor: executor.clone(),
                },
            )?;
            // An executor that cannot be started is listed as unavailable in the facts and passed over: a
            // thread that needs it says so at its pre-flight.
            match self.ask_bounded(start).await? {
                Some(Outcome::Succeeded(_)) => {}
                other => {
                    tracing::warn!(%executor, environment = %item.id, outcome = ?other, "an executor could not be started");
                }
            }
        }
        if self.is_stopped() {
            return Ok(());
        }
        let attached = {
            let mut conn = self.inner.db.acquire().await?;
            rows::connected_attached(
                &mut conn,
                item.id,
                item.session,
                &facts,
                self.inner.clock.now(),
            )
            .await?
        };
        if attached {
            self.reevaluate_environment(item.id).await?;
        }
        Ok(())
    }

    /// Sends a command and waits for its end, for no longer than an attach is given: none when it did
    /// not end in time, in which case it is not to be carried out any more.
    pub(crate) async fn ask_bounded(
        &self,
        command: Outgoing,
    ) -> Result<Option<Outcome>, EngineError> {
        let id = command.id;
        self.inner.commands.issue(command).await?;
        match tokio::time::timeout(
            self.inner.config.hosting.attach_wait,
            self.inner.commands.wait(id),
        )
        .await
        {
            Ok(outcome) => Ok(Some(outcome?)),
            Err(_) => {
                self.inner.commands.cancel(id).await?;
                Ok(None)
            }
        }
    }

    /// Says that a directory could not be attached, unless its machine has gone since: that says
    /// nothing of the directory, and it is looked at when the machine is back.
    async fn attach_refused(&self, item: &Connected, why: &str) -> Result<(), EngineError> {
        let mut conn = self.inner.db.acquire().await?;
        let still_here = rows::connected(&mut conn, item.id)
            .await?
            .is_some_and(|standing| standing.here);
        if still_here {
            rows::connected_attach_failed(
                &mut conn,
                item.id,
                item.session,
                why,
                self.inner.clock.now(),
            )
            .await?;
        }
        Ok(())
    }

    /// Where a directory on a person's machine stands ([`Engine::environment_standing`]). One that is
    /// attached in the session its machine has is ready. One that is not yet is waited for: the sweep
    /// attaches it and brings whoever waits back. One whose machine is away, or does not serve
    /// environments, or refused the attach, cannot be had, with the reason.
    async fn attach_standing(&self, environment: Id) -> Result<Standing, EngineError> {
        let standing = {
            let mut conn = self.inner.db.acquire().await?;
            rows::connected(&mut conn, environment).await?
        };
        let Some(standing) = standing else {
            return Ok(Standing::Ready);
        };
        let unavailable = if standing.remove_at.is_some() {
            Some("its removal is scheduled".to_owned())
        } else if !standing.here {
            Some(AWAY.to_owned())
        } else if !standing.serves {
            Some(NOT_SERVED.to_owned())
        } else if standing.status == status::FAILED {
            Some(environment_state_sentence(
                status::FAILED,
                standing.status_detail.as_deref(),
            ))
        } else {
            None
        };
        if let Some(why) = unavailable {
            return Ok(Standing::Unavailable(why));
        }
        if standing.attached() {
            let mut conn = self.inner.db.acquire().await?;
            rows::set_status(
                &mut conn,
                environment,
                &[status::STOPPED],
                status::RUNNING,
                None,
                self.inner.clock.now(),
            )
            .await?;
            orch_db::queries::environments::thread_engine::touch_activity(
                &mut conn,
                environment,
                self.inner.clock.now(),
            )
            .await?;
            return Ok(Standing::Ready);
        }
        self.kick();
        Ok(Standing::Waiting)
    }
}

// ---------------------------------------------------------------- what idle and unused mean

/// When an environment will be stopped for being idle: its last activity and the minutes its
/// `env.idle_stop_minutes` allows; none for a stopped one, or when the setting is zero (never).
pub async fn idle_stop_at(
    conn: &mut orch_db::Conn,
    state: &EnvironmentState,
) -> Result<Option<DateTime<Utc>>, EngineError> {
    if state.status != status::RUNNING
        || !orch_db::queries::environment_facts::has_unit(conn, state.id).await?
    {
        return Ok(None);
    }
    let minutes: i64 = setting(
        conn,
        SettingsPlace::Environment(state.id),
        keys::ENV_IDLE_STOP_MINUTES,
    )
    .await?
    .unwrap_or(0);
    if minutes <= 0 {
        return Ok(None);
    }
    let last = state.last_activity_at.unwrap_or(state.created_at);
    Ok(Some(last + chrono::Duration::minutes(minutes)))
}

#[cfg(test)]
mod storage_tests {
    use super::*;
    #[test]
    fn attaching_owned_data_carries_its_named_storage_choice_to_the_machine() {
        let id = orch_types::new_id();
        let with = AttachWith {
            storage: Some(orch_settings_contract::unit::UnitStorage {
                workspace: Some("fast".into()),
                home: Some("bulk".into()),
                state: Some("fast".into()),
            }),
            ..Default::default()
        };
        let command = attach_body(id, None, &with);
        let wire = serde_json::to_value(command).unwrap();
        assert_eq!(
            wire["storage"],
            serde_json::json!({"workspace":"fast","home":"bulk","state":"fast"})
        );
        assert_eq!(wire["environment_id"], serde_json::json!(id));
    }
}
