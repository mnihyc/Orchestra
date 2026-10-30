//! A managed provider's runtime through its life: started when a thread needs it and by hand, stopped by hand and when idle, retired with the provider and taken away, and what a turn waits for while the unit comes up.
//!
//! A managed provider has one runtime for as long as it exists, shared by every thread bound to it
//! (`jobs::provider_provision` makes and starts it). It stops when it has had no turn, lease, session
//! or sign-in under way for as long as `runtime.idle_stop_minutes` of the provider says (a day, as
//! shipped), and the next turn, a sign-in, or a person starts it again; its `state` volume, with the
//! sign-in and the conversations, stays. While the unit is not up the provider has no machine
//! (`providers.machine_id` is empty), which is how a turn's pre-flight knows to start it.
//!
//! **A turn that needs it** goes through [`Engine::await_provider_runtime`], at the head of the start
//! sequence beside the wait for the environment: the unit is started, or the job that starts it is
//! waited for, with the step `provider` shown on the thread. A unit that could not be made is not tried
//! again by every turn that comes: the thread is blocked with the reason, and a person starts it again.
//!
//! **Stopping** is refused while a turn runs on the provider unless the request says to interrupt it, and
//! is a compare-and-set on the runtime's state, like an environment's. A unit that was begun to be stopped
//! by a process that died is finished by the schedule.

use crate::commands::{Outcome, Outgoing};
use crate::engine::{Engine, lock};
use crate::environments::Awaited;
use crate::error::EngineError;
use crate::ids;
use crate::jobs::provider_provision::{self, Mode};
use crate::preflight::Blocked;
use crate::turns::{InputAfter, TurnEnd, error_json};
use crate::unit::Unit;
use chrono::{DateTime, Utc};
use orch_db::DbError;
use orch_db::queries::environments::hosting::{self as env_rows, RuntimeFull, runtime_state};
use orch_db::queries::jobs::{JobRow, provider_jobs};
use orch_db::queries::resources::hosting as host_rows;
use orch_db::queries::resources::provider_runtime::{self as rows, State};
use orch_db::queries::resources::thread_engine as machine_rows;
use orch_db::queries::threads::{self, thread_state, turn_state};
use orch_link_contract::commands::{NodeRuntimeStop, SerialKey};
use orch_settings::{Place as SettingsPlace, get as setting};
use orch_settings_contract::registry::keys;
use orch_types::{Id, Json};
use std::time::Duration;

/// How many runtimes or providers one look at the schedule takes up for each kind of work.
const REAP_BATCH: i64 = 100;
/// How long a unit that could not be made is left alone before a turn tries to make it again: a turn
/// does not make a unit that failed every second, and a person who starts it again is not made to wait.
pub const RETRY_AFTER: Duration = Duration::from_secs(300);
/// How often a stop that a turn or the schedule waits for looks at the turns it interrupted.
const STOP_LOOK_EVERY: Duration = Duration::from_millis(100);
/// How long a stop that interrupts turns waits for them to end, beyond the engine's own wait for an
/// interrupt to be answered.
const INTERRUPT_MARGIN: Duration = Duration::from_secs(5);

/// Resolves the environment of a provider's unit, retaining only sealed secret references in plans.
pub(crate) async fn definition(
    conn: &mut orch_db::Conn,
    provider: Id,
    template: &Json,
) -> Result<Json, EngineError> {
    let mut definition = template.clone();
    let plugins = orch_settings_contract::unit::template_plugins(template, true)
        .map_err(EngineError::unreadable)?;
    let needs: Option<Json> =
        setting(conn, SettingsPlace::Provider(provider), keys::RUNTIME_NEEDS).await?;
    let limits: Option<Json> = setting(
        conn,
        SettingsPlace::Provider(provider),
        keys::RUNTIME_RESOURCES,
    )
    .await?;
    let limits = crate::places::limits_of(template, limits.as_ref());
    let needs = crate::places::needs_of(template, needs.as_ref(), Some(&limits));
    // The frozen unit declaration also counts inherited needs when a host reports its load.
    definition["needs"] = serde_json::json!({"cpu_millis":needs.cpu_millis,"memory_mb":needs.memory_mb,"disk_gb":needs.disk_gb});
    let mut env = definition
        .get("env")
        .and_then(Json::as_object)
        .cloned()
        .unwrap_or_default();
    env.extend(variables(conn, provider).await?);
    env.insert(
        "ORCH_PLUGINS".into(),
        Json::String(serde_json::to_string(&plugins).map_err(EngineError::from)?),
    );
    definition["env"] = Json::Object(env);
    Ok(definition)
}

/// One resolution for units and directly hosted harnesses; secrets remain references here.
pub(crate) async fn variables(
    conn: &mut orch_db::Conn,
    provider: Id,
) -> Result<serde_json::Map<String, Json>, EngineError> {
    let mut env = serde_json::Map::new();
    let variables: Option<Json> = setting(
        conn,
        SettingsPlace::Provider(provider),
        keys::RUNTIME_VARIABLES,
    )
    .await?;
    if let Some(values) = variables.as_ref().and_then(Json::as_object) {
        if values
            .keys()
            .any(|name| name.is_empty() || name.contains(['=', '\0']) || name.starts_with("ORCH_"))
        {
            return Err(EngineError::unreadable(
                "runtime variable names cannot be empty, contain equals or NUL, or replace ORCH_ variables",
            ));
        }
        env.extend(values.clone());
    }
    let names: Vec<String> = setting(
        conn,
        SettingsPlace::Provider(provider),
        keys::RUNTIME_SECRET_NAMES,
    )
    .await?
    .unwrap_or_default();
    for name in names {
        if name.starts_with("ORCH_") {
            return Err(EngineError::unreadable(
                "ORCH_ variables belong to the connector",
            ));
        }
        let stored = orch_db::queries::settings::secret(
            conn,
            "provider",
            provider,
            &format!("runtime:{name}"),
        )
        .await?
        .ok_or_else(|| {
            EngineError::unreadable(format!("the provider's runtime secret {name} has no value"))
        })?;
        env.insert(name, serde_json::json!({"$secret":stored.id}));
    }
    Ok(env)
}

/// Runtime values go only to this provider's runtime machine, never to an unrelated thread environment.
pub(crate) async fn harness_variables(
    conn: &mut orch_db::Conn,
    provider: Id,
    agent: Id,
    machine: Id,
) -> Result<Option<orch_link_contract::commands::AgentRuntime>, EngineError> {
    let locations: Option<(Option<Id>, Option<Id>)> = sqlx::query_as(
        "SELECT machine_id,key_machine_id FROM providers WHERE id=$1 AND deleted_at IS NULL",
    )
    .bind(provider)
    .fetch_optional(&mut *conn)
    .await
    .map_err(DbError::from)?;
    let Some((runtime, _)) = locations else {
        return Ok(None);
    };
    if runtime == Some(machine) {
        return Ok(Some(harness_values(conn, provider).await?));
    }
    let home = home_for_harness(conn, provider, agent, machine).await?;
    Ok(Some(orch_link_contract::commands::AgentRuntime {
        resources: None,
        variables: Default::default(),
        home,
        home_storage: None,
    }))
}

/// The same home selection used in agent.start, without reading another runtime's secret values.
pub(crate) async fn home_for_harness(
    conn: &mut orch_db::Conn,
    provider: Id,
    agent: Id,
    machine: Id,
) -> Result<Option<orch_driver_contract::runtime::EnvironmentDataSource>, EngineError> {
    let locations: Option<(Option<Id>, Option<Id>)> = sqlx::query_as(
        "SELECT machine_id,key_machine_id FROM providers WHERE id=$1 AND deleted_at IS NULL",
    )
    .bind(provider)
    .fetch_optional(&mut *conn)
    .await
    .map_err(DbError::from)?;
    let Some((runtime, keeper)) = locations else {
        return Err(EngineError::unreadable("the provider is gone"));
    };
    Ok(if runtime == Some(machine) {
        orch_settings::provider_home_choice(conn, provider).await?
    } else if keeper == Some(machine) {
        orch_settings::credential_home_choice(conn, provider).await?
    } else {
        orch_settings::profile_home_choice(conn, Some(agent)).await?
    })
}

/// A verified candidate uses the same frozen provider values before its machine becomes current.
pub(crate) async fn harness_values(
    conn: &mut orch_db::Conn,
    provider: Id,
) -> Result<orch_link_contract::commands::AgentRuntime, EngineError> {
    let values = variables(conn, provider).await?;
    let variables = serde_json::from_value(Json::Object(values))?;
    let home = orch_settings::provider_home_choice(conn, provider).await?;
    let storage: Option<orch_settings_contract::unit::UnitStorage> = orch_settings::get(
        conn,
        orch_settings::Place::Provider(provider),
        orch_settings_contract::registry::keys::RUNTIME_STORAGE,
    )
    .await?;
    let resources = orch_settings::get(
        conn,
        orch_settings::Place::Provider(provider),
        orch_settings_contract::registry::keys::RUNTIME_RESOURCES,
    )
    .await?;
    Ok(orch_link_contract::commands::AgentRuntime {
        resources,
        variables,
        home,
        home_storage: storage.and_then(|storage| storage.home),
    })
}

/// What starting a provider's runtime came to.
#[derive(Debug, Clone)]
pub enum RuntimeStart {
    /// It is up already.
    Up,
    /// A job makes it ready: the one just queued, or the one that was under way.
    Job(Box<JobRow>),
}

/// Why a provider's runtime cannot be started or stopped.
#[derive(Debug, thiserror::Error)]
pub enum RuntimeError {
    /// There is no such provider.
    #[error("there is no such provider")]
    NotFound,
    /// It is a provider on a person's machine: there is no unit of Orch's to start.
    #[error("this provider is on a person's machine, which is not started from here")]
    NotManaged,
    /// The provider is switched off or deleted.
    #[error("the provider is {0}")]
    Unavailable(String),
    /// A turn runs on it, and the request did not say to interrupt.
    #[error("{0} turns run on it")]
    Busy(i64),
    /// The host could not stop the unit.
    #[error("the host could not stop it: {0}")]
    Host(String),
    /// The system failed.
    #[error(transparent)]
    Internal(#[from] EngineError),
}

impl From<DbError> for RuntimeError {
    fn from(error: DbError) -> Self {
        Self::Internal(error.into())
    }
}

/// Why a unit is being stopped.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StopWhy {
    /// A person asked.
    Person(Id),
    /// It had no activity for as long as its setting allows.
    Idle,
}

/// The words a provider that waits for its runtime says.
pub const STARTING_SENTENCE: &str = "its runtime is being started";

impl Engine {
    /// Starts or attaches the selected runtime from its declared lifecycle. An existing unit is reused;
    /// a new unit is made only when its unit runtime has none or failed.
    pub async fn start_provider_runtime(
        &self,
        provider: Id,
        by: Option<Id>,
    ) -> Result<RuntimeStart, RuntimeError> {
        self.queue_provider_runtime(provider, by, false).await
    }

    /// Renews the native sign-in in its running runtime, or starts the same stopped runtime first.
    pub async fn renew_provider_runtime(&self, provider: Id) -> Result<RuntimeStart, RuntimeError> {
        self.queue_provider_runtime(provider, None, true).await
    }

    async fn queue_provider_runtime(
        &self,
        provider: Id,
        by: Option<Id>,
        renew: bool,
    ) -> Result<RuntimeStart, RuntimeError> {
        let mut tx = self.inner.db.begin().await?;
        let now = self.inner.clock.now();
        let state = rows::lock(&mut tx, provider)
            .await?
            .ok_or(RuntimeError::NotFound)?;
        if state.deleted_at.is_some() {
            return Err(RuntimeError::Unavailable("deleted".to_owned()));
        }
        if state.status == "disabled" {
            return Err(RuntimeError::Unavailable("switched off".to_owned()));
        }
        let unavailable: Option<String> =
            sqlx::query_scalar("SELECT runtime_unavailable FROM providers WHERE id=$1")
                .bind(provider)
                .fetch_one(&mut *tx)
                .await
                .map_err(DbError::from)?;
        if let Some(reason) = unavailable {
            return Err(RuntimeError::Unavailable(format!(
                "{reason}; review a re-creation before choosing another host"
            )));
        }
        if renew
            && state.key_machine_id.is_some()
            && !rows::keeper_has_unit(&mut tx, provider).await?
        {
            if !orch_settings::provider_sign_in(&mut tx, provider)
                .await
                .map_err(EngineError::from)?
                .allowed
            {
                return Err(RuntimeError::Unavailable("provider.signin: this credential keeper has not permitted renewal in its existing harness home".into()));
            }
            tx.commit().await.map_err(DbError::from)?;
            return match self.renew_sign_in(provider).await? {
                crate::provider_setup::Renewed::Held => Ok(RuntimeStart::Up),
                crate::provider_setup::Renewed::Lapsed(reason) => {
                    Err(RuntimeError::Unavailable(reason))
                }
            };
        }
        let machine_facts =
            orch_db::queries::environment_facts::of_provider(&mut tx, provider).await?;
        if state.machine_id.is_some()
            && !machine_facts.as_ref().is_some_and(|facts| facts.attaches())
            && rows::live_runtime(&mut tx, provider).await?.is_none()
        {
            return Err(RuntimeError::Unavailable(
                "the selected machine has not reported usable provider runtime services".into(),
            ));
        }
        if machine_facts.as_ref().is_some_and(|facts| facts.attaches()) {
            if renew
                && !orch_settings::provider_sign_in(&mut tx, provider)
                    .await
                    .map_err(EngineError::from)?
                    .allowed
            {
                return Err(RuntimeError::Unavailable("provider.signin: the credential keeper has not permitted renewal in this existing harness home".into()));
            }
            let machine = state
                .machine_id
                .ok_or_else(|| RuntimeError::Unavailable("the runtime has no machine".into()))?;
            let online = machine_rows::machine_place(&mut tx, machine)
                .await?
                .is_some_and(|m| m.online && m.status == "active" && m.deleted_at.is_none());
            if !online {
                return Err(RuntimeError::Unavailable(
                    "the runtime's machine is away".into(),
                ));
            }
            let profile = match state.agent_id {
                Some(agent) => machine_rows::agent(&mut tx, agent).await?,
                None => None,
            };
            tx.commit().await.map_err(DbError::from)?;
            if state.kind == "subscription" {
                let profile = profile.ok_or_else(|| {
                    RuntimeError::Unavailable("the sign-in's agent profile is gone".into())
                })?;
                // A machine-bound source profile stays with its keeper. Other compatible profiles
                // attach to this runtime when a binding actually chooses them.
                if profile
                    .machine_id
                    .is_none_or(|required| required == machine)
                {
                    let key = crate::harnesses::HarnessKey {
                        provider,
                        agent: profile.id,
                        machine,
                    };
                    match self
                        .ensure_harness(key, &profile, orch_types::new_id())
                        .await?
                    {
                        crate::harnesses::Started::Up => {}
                        crate::harnesses::Started::Failed(outcome) => {
                            return Err(RuntimeError::Host(crate::provider_setup::failure_text(
                                &outcome,
                            )));
                        }
                    }
                }
            } else if let Err(reason) = Box::pin(self.ensure_provider_up(provider)).await? {
                return Err(RuntimeError::Host(reason));
            }
            if renew
                && let crate::provider_setup::Renewed::Lapsed(reason) =
                    self.renew_sign_in(provider).await?
            {
                return Err(RuntimeError::Unavailable(reason));
            }
            return Ok(RuntimeStart::Up);
        }
        // Runtime updates honor the owner's policy before waking a stopped unit.
        if !renew && let Some(job) = self.provider_update_on_start(&mut tx, provider).await? {
            tx.commit().await.map_err(DbError::from)?;
            self.kick();
            return Ok(RuntimeStart::Job(Box::new(job)));
        }
        let live = rows::live_runtime(&mut tx, provider).await?;
        if renew
            && live.as_ref().is_none_or(|runtime| {
                !matches!(
                    runtime.state.as_str(),
                    runtime_state::RUNNING | runtime_state::STOPPED | runtime_state::CREATING
                )
            })
        {
            return Err(RuntimeError::Unavailable(
                "renewal needs a manager to repair or recreate the runtime".to_owned(),
            ));
        }
        if let Some(runtime) = live.as_ref()
            && let Some(blocked) = crate::network::blocked(&mut tx, runtime.id).await?
        {
            return Err(RuntimeError::Unavailable(
                blocked.detail.unwrap_or_else(|| blocked.reason.to_owned()),
            ));
        }
        let mode = match live.as_ref().map(|runtime| runtime.state.as_str()) {
            Some(runtime_state::RUNNING) => {
                if renew {
                    Mode::Renew
                } else {
                    return Ok(RuntimeStart::Up);
                }
            }
            Some(runtime_state::STOPPED) => {
                if renew {
                    Mode::Renew
                } else {
                    Mode::Start
                }
            }
            Some(runtime_state::CREATING) => {
                return Ok(
                    match provider_jobs::open_of_provider(&mut tx, provider)
                        .await?
                        .into_iter()
                        .next()
                    {
                        Some(job) => RuntimeStart::Job(Box::new(job)),
                        None => RuntimeStart::Up,
                    },
                );
            }
            Some(runtime_state::STOPPING) => {
                return Err(RuntimeError::Unavailable(
                    "its runtime is being stopped".to_owned(),
                ));
            }
            None | Some(_) => Mode::Make,
        };
        match provider_provision::queue(&mut tx, now, provider, mode, by).await? {
            provider_jobs::Queued::Made(job) => {
                tx.commit().await.map_err(DbError::from)?;
                self.kick();
                Ok(RuntimeStart::Job(job))
            }
            provider_jobs::Queued::Running(job) => Ok(RuntimeStart::Job(job)),
            provider_jobs::Queued::NoProvider => Err(RuntimeError::NotFound),
        }
    }

    // ------------------------------------------------------------ what a turn waits for

    /// What a turn's start sequence does about the runtime of its provider. A provider whose unit is up
    /// lets the sequence go on. One whose unit is stopped, failed or not made is started, and one whose
    /// unit is being made is waited for: the thread says that it waits for the provider, and the sweep
    /// brings the sequence back. A unit that could not be made ends the turn's start, which blocks the
    /// thread as `provider_unavailable` with the reason and keeps the input.
    pub async fn await_provider_runtime(&self, thread: Id) -> Result<Awaited, EngineError> {
        let (turn, provider_id) = {
            let mut conn = self.inner.db.acquire().await?;
            let Some(row) = threads::thread(&mut conn, thread).await? else {
                return Ok(Awaited::Ready);
            };
            let Some(provider) = row.provider_id else {
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
            (turn.id, provider)
        };
        let (state, runtime, machine_online) = {
            let mut conn = self.inner.db.acquire().await?;
            let Some(state) = rows::state(&mut conn, provider_id).await? else {
                return Ok(Awaited::Ready);
            };
            let runtime = rows::live_runtime(&mut conn, provider_id).await?;
            let online = match state.machine_id {
                Some(machine) => machine_rows::machine_place(&mut conn, machine)
                    .await?
                    .is_some_and(|place| place.online && place.status == "active"),
                None => false,
            };
            (state, runtime, online)
        };
        if (runtime.is_none() && state.machine_id.is_some())
            || state.deleted_at.is_some()
            || state.status == "disabled"
        {
            return Ok(Awaited::Ready);
        }
        match runtime.as_ref().map(|runtime| runtime.state.as_str()) {
            Some(runtime_state::RUNNING) if state.machine_id.is_some() && machine_online => {
                let mut conn = self.inner.db.acquire().await?;
                if let Some(runtime) = &runtime {
                    env_rows::touch_runtime(&mut conn, runtime.id, self.inner.clock.now()).await?;
                }
                return Ok(Awaited::Ready);
            }
            Some(runtime_state::RUNNING) => {
                // The unit is up as far as the rows say and its connector is not: it is waited for, and
                // the host's word that the unit is gone makes it a failed one, which is made again.
                self.set_step(thread, "provider").await?;
                return Ok(Awaited::Waiting);
            }
            _ => {}
        }
        if let Some(why) = self.recently_failed(provider_id).await? {
            self.fail_turn_for_provider(thread, turn, &why).await?;
            return Ok(Awaited::Failed);
        }
        match self.start_provider_runtime(provider_id, None).await {
            Ok(_) => {}
            Err(RuntimeError::Internal(error)) => return Err(error),
            Err(other) => {
                self.fail_turn_for_provider(thread, turn, &other.to_string())
                    .await?;
                return Ok(Awaited::Failed);
            }
        }
        self.set_step(thread, "provider").await?;
        Ok(Awaited::Waiting)
    }

    /// Why the last attempt to make or start the unit failed, when it did so recently enough that a
    /// turn does not try again by itself.
    async fn recently_failed(&self, provider: Id) -> Result<Option<String>, EngineError> {
        let mut conn = self.inner.db.acquire().await?;
        let Some(job) = provider_jobs::latest_of_provider(&mut conn, provider).await? else {
            return Ok(None);
        };
        if job.kind != provider_jobs::PROVISION
            || !matches!(job.state.as_str(), "failed" | "cancelled")
        {
            return Ok(None);
        }
        let ended = job.finished_at.unwrap_or(job.updated_at);
        let wait = chrono::Duration::from_std(RETRY_AFTER).unwrap_or_default();
        if self.inner.clock.now() - ended >= wait {
            return Ok(None);
        }
        let said = job
            .error
            .as_ref()
            .and_then(|error| error.get("message"))
            .and_then(orch_types::Json::as_str)
            .unwrap_or("it was cancelled")
            .to_owned();
        Ok(Some(format!("its runtime could not be made: {said}")))
    }

    /// Ends the start of a turn whose provider's runtime cannot be brought up: the turn fails, its input
    /// waits again where it stood, and the thread is blocked as `provider_unavailable` with the reason.
    async fn fail_turn_for_provider(
        &self,
        thread_id: Id,
        turn_id: Id,
        why: &str,
    ) -> Result<(), EngineError> {
        let message = format!("the provider is not available: {why}");
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
                "provider_unavailable",
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
                    error: Some(error_json("provider_unavailable", &message, true)),
                    input: InputAfter::Retry { retryable: true },
                    then_block: Some(Blocked {
                        reason: "provider_unavailable",
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

    // ------------------------------------------------------------ stopping

    /// Accepts removal of the exact provider unit selected by a host manager. The durable job
    /// owns all machine calls; no transaction is held while its stop or removal is awaited.
    pub async fn remove_host_provider_unit_in(
        &self,
        mut tx: orch_db::Tx,
        principal: &orch_authz::Principal,
        origin: &orch_db::audit::Origin,
        host: Id,
        runtime: Id,
        volumes: bool,
    ) -> Result<(), RuntimeError> {
        let row = env_rows::runtime(&mut tx, runtime)
            .await?
            .filter(|row| row.host_id == host && row.kind == "provider")
            .ok_or(RuntimeError::NotFound)?;
        let provider = row.provider_id.ok_or(RuntimeError::NotFound)?;
        if row.state == runtime_state::REMOVED {
            tx.commit().await.map_err(DbError::from)?;
            return Ok(());
        }
        let now = principal.now().get();
        // queue locks the provider, serializing this acceptance with provisioning/recreation.
        match provider_provision::queue_removal_with_options(
            &mut tx,
            now,
            provider,
            runtime,
            volumes,
            Some(principal.user()),
        )
        .await?
        {
            provider_jobs::Queued::Made(_) => {}
            provider_jobs::Queued::Running(job)
                if job.kind == provider_jobs::REMOVE
                    && job
                        .plan
                        .get("runtime")
                        .and_then(Json::as_str)
                        .and_then(|id| id.parse::<Id>().ok())
                        == Some(runtime)
                    && job
                        .plan
                        .get("volumes")
                        .and_then(Json::as_bool)
                        .unwrap_or(true)
                        == volumes =>
            {
                tx.commit().await.map_err(DbError::from)?;
                self.kick();
                return Ok(());
            }
            provider_jobs::Queued::Running(_) => {
                return Err(RuntimeError::Unavailable(
                    "a job works on its provider runtime".to_owned(),
                ));
            }
            provider_jobs::Queued::NoProvider => return Err(RuntimeError::NotFound),
        }
        if orch_db::queries::jobs::changes::provider_hold(&mut tx, provider)
            .await?
            .is_some()
        {
            return Err(RuntimeError::Unavailable(
                "a job uses its provider runtime".to_owned(),
            ));
        }
        let current = rows::live_runtime(&mut tx, provider)
            .await?
            .is_some_and(|current| current.id == runtime);
        if current {
            let busy = rows::busy(&mut tx, provider).await?.turns;
            if busy > 0 {
                return Err(RuntimeError::Busy(busy));
            }
            // The next turn must make a unit, never restart this retired unit. A different
            // current runtime is untouched when a manager removes an older kept one.
            self.unit_is_gone(&mut tx, provider, row.machine_id, now)
                .await?;
        }
        env_rows::retire_runtime(&mut tx, runtime, Some("removed by the host's manager"), now)
            .await?;
        let home = host_rows::host(&mut tx, host)
            .await?
            .ok_or(RuntimeError::NotFound)?;
        let mut entry = orch_db::audit::Entry::new(
            orch_db::audit::Actor::User(principal.user()),
            orch_access_contract::audit::AuditAction::HostUpdated.as_str(),
            "host",
            Some(host),
            "A host manager asked to remove a provider unit",
        );
        entry.history = true;
        entry.node_id = home.home_id;
        entry.org_id = home.org_id;
        entry.details = serde_json::json!({"host":host,"runtime":runtime,"provider":provider,"volumes":volumes,"decision":"accepted"});
        orch_db::audit::append(&mut tx, &entry, origin, now).await?;
        tx.commit().await.map_err(DbError::from)?;
        self.kick();
        Ok(())
    }

    /// Detaches a directory's harnesses or stops its unit and keeps its data. Running turns require
    /// an explicit interruption first.
    pub async fn stop_provider_runtime(
        &self,
        provider: Id,
        interrupt: bool,
        why: StopWhy,
    ) -> Result<(), RuntimeError> {
        let state = {
            let mut conn = self.inner.db.acquire().await?;
            rows::state(&mut conn, provider)
                .await?
                .ok_or(RuntimeError::NotFound)?
        };
        let directory = orch_db::queries::environment_facts::of_provider(
            &mut *self.inner.db.acquire().await?,
            provider,
        )
        .await?
        .is_some_and(|facts| facts.attaches());
        if directory {
            self.clear_provider_turns(provider, interrupt, why).await?;
            let agents: Vec<Id> = sqlx::query_scalar(
                "SELECT DISTINCT agent_id FROM harnesses WHERE provider_id=$1 AND status='online'",
            )
            .bind(provider)
            .fetch_all(&mut *self.inner.db.acquire().await?)
            .await
            .map_err(DbError::from)?;
            for agent in agents {
                self.stop_harness_at(provider, agent, state.machine_id)
                    .await
                    .map_err(|error| RuntimeError::Host(error.to_string()))?;
            }
            return Ok(());
        }
        let runtime = {
            let mut conn = self.inner.db.acquire().await?;
            rows::live_runtime(&mut conn, provider).await?
        };
        let Some(runtime) = runtime else {
            return Ok(());
        };
        self.stop_provider_unit(provider, runtime.id, interrupt, why)
            .await
    }

    /// Stops this unit alone; a host action must never follow a provider onto its newer unit.
    pub(crate) async fn stop_provider_unit(
        &self,
        provider: Id,
        runtime: Id,
        interrupt: bool,
        why: StopWhy,
    ) -> Result<(), RuntimeError> {
        let mut conn = self.inner.db.acquire().await?;
        let runtime = env_rows::runtime(&mut conn, runtime)
            .await?
            .filter(|runtime| runtime.provider_id == Some(provider))
            .ok_or(RuntimeError::NotFound)?;
        if runtime.state == runtime_state::RETIRED {
            let host = host_rows::host(&mut conn, runtime.host_id)
                .await?
                .ok_or_else(|| RuntimeError::Host("the host is gone".to_owned()))?;
            drop(conn);
            let command = Outgoing::new(
                ids::derived(
                    "node.runtime.stop.kept",
                    &[runtime.id.as_bytes(), orch_types::new_id().as_bytes()],
                ),
                host.machine_id,
                SerialKey::Runtime(runtime.id),
                &NodeRuntimeStop {
                    runtime_id: runtime.id,
                    reason: None,
                },
            )
            .map_err(EngineError::from)?;
            return match tokio::time::timeout(
                self.inner.config.hosting.stop_wait,
                crate::commands::run(self.inner.commands.as_ref(), command),
            )
            .await
            {
                Ok(Ok(Outcome::Succeeded(_))) => Ok(()),
                Ok(Ok(Outcome::Failed(error))) => Err(RuntimeError::Host(format!(
                    "{}: {}",
                    error.code, error.detail
                ))),
                _ => Err(RuntimeError::Host(
                    "the host did not acknowledge the kept unit's stop".to_owned(),
                )),
            };
        }
        let state = rows::state(&mut conn, provider)
            .await?
            .ok_or(RuntimeError::NotFound)?;
        let current = rows::live_runtime(&mut conn, provider).await?;
        if current.is_none_or(|current| current.id != runtime.id) {
            return Err(RuntimeError::Unavailable(
                "a job owns this candidate unit".to_owned(),
            ));
        }
        drop(conn);
        match runtime.state.as_str() {
            runtime_state::STOPPED | runtime_state::FAILED => return Ok(()),
            runtime_state::RUNNING | runtime_state::STOPPING => {}
            other => {
                return Err(RuntimeError::Unavailable(format!("its runtime is {other}")));
            }
        }
        if why == StopWhy::Idle && !self.runtime_is_idle(&state, &runtime).await? {
            return Ok(());
        }
        self.clear_provider_turns(provider, interrupt, why).await?;
        let began = {
            let mut conn = self.inner.db.acquire().await?;
            env_rows::set_runtime_state(
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
            return Err(RuntimeError::Unavailable(format!(
                "its runtime is {}",
                runtime.state
            )));
        }
        self.finish_provider_stop(provider, runtime.id).await
    }

    /// Carries a stop through: the host stops the unit, and the rows say it. A stop that a process that
    /// died began is finished by the schedule with this.
    pub(crate) async fn finish_provider_stop(
        &self,
        provider: Id,
        runtime: Id,
    ) -> Result<(), RuntimeError> {
        let (row, host) = {
            let mut conn = self.inner.db.acquire().await?;
            let Some(row) = env_rows::runtime(&mut conn, runtime).await? else {
                return Ok(());
            };
            let host = host_rows::host(&mut conn, row.host_id).await?;
            (row, host)
        };
        let Some(host) = host else {
            return Err(RuntimeError::Host("the host is gone".to_owned()));
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
        let mut tx = self.inner.db.begin().await?;
        match waited {
            Ok(Ok(Outcome::Succeeded(_))) => {
                env_rows::set_runtime_state(
                    &mut tx,
                    runtime,
                    &[runtime_state::STOPPING],
                    runtime_state::STOPPED,
                    None,
                    now,
                )
                .await?;
                if rows::live_runtime(&mut tx, provider)
                    .await?
                    .is_some_and(|current| current.id == runtime)
                {
                    self.unit_is_gone(&mut tx, provider, row.machine_id, now)
                        .await?;
                }
                tx.commit().await.map_err(DbError::from)?;
                Ok(())
            }
            Ok(Ok(Outcome::Failed(error))) => {
                env_rows::set_runtime_state(
                    &mut tx,
                    runtime,
                    &[runtime_state::STOPPING],
                    runtime_state::RUNNING,
                    None,
                    now,
                )
                .await?;
                tx.commit().await.map_err(DbError::from)?;
                Err(RuntimeError::Host(format!(
                    "{}: {}",
                    error.code, error.detail
                )))
            }
            Ok(Ok(_)) | Ok(Err(_)) | Err(_) => {
                // The host has not said: the unit stands `stopping` and the schedule asks again.
                Err(RuntimeError::Host(
                    "the host did not answer in time; the stop is tried again".to_owned(),
                ))
            }
        }
    }

    /// What is so once the unit of a provider's runtime is not up: the provider has no machine, the
    /// harnesses it held there are not online, and the sessions the engine thought open there are not.
    async fn unit_is_gone(
        &self,
        conn: &mut orch_db::Conn,
        provider: Id,
        machine: Option<Id>,
        now: DateTime<Utc>,
    ) -> Result<(), EngineError> {
        rows::set_machine(conn, provider, None, now).await?;
        if let Some(machine) = machine {
            machine_rows::set_machine_harnesses_offline(conn, machine, now).await?;
            lock(&self.inner.sessions).machine_lost(machine);
        }
        Ok(())
    }

    /// Makes sure no turn runs on a provider: it fails when there is one and the request did not say to
    /// interrupt; otherwise the turns are asked to stop, and waited for.
    async fn clear_provider_turns(
        &self,
        provider: Id,
        interrupt: bool,
        why: StopWhy,
    ) -> Result<(), RuntimeError> {
        let running = {
            let mut conn = self.inner.db.acquire().await?;
            rows::busy(&mut conn, provider).await?.turns
        };
        if running == 0 {
            return Ok(());
        }
        let by = match why {
            StopWhy::Person(by) if interrupt => by,
            _ => return Err(RuntimeError::Busy(running)),
        };
        let bound = {
            let mut conn = self.inner.db.acquire().await?;
            rows::threads_with_turns(&mut conn, provider).await?
        };
        for thread in bound {
            let mut tx = self.inner.db.begin().await?;
            let finished = {
                let mut unit = Unit::new(&mut tx, self.inner.clock.now());
                if threads::lock(&mut *unit.conn, thread).await?.is_none() {
                    continue;
                }
                if let Some(turn) = threads::active_turn(&mut *unit.conn, thread).await? {
                    let seq = unit.stamp(thread).await?;
                    threads::mark_interrupted_by(&mut *unit.conn, turn.id, by, seq).await?;
                    self.withdraw_interrupted_harness(&mut unit, thread, turn.id)
                        .await?;
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
                rows::busy(&mut conn, provider).await?.turns
            };
            if left == 0 {
                return Ok(());
            }
            if began.elapsed() >= wait {
                return Err(RuntimeError::Busy(left));
            }
            tokio::time::sleep(STOP_LOOK_EVERY).await;
        }
    }

    // ------------------------------------------------------------ being idle

    /// How long the runtime of a provider may be left without a turn, a lease, a session or a sign-in
    /// under way before it is stopped: the provider's own `runtime.idle_stop_minutes`, else what its home
    /// resolves. Zero never stops it.
    async fn idle_minutes(&self, provider: Id) -> Result<i64, EngineError> {
        let mut conn = self.inner.db.acquire().await?;
        Ok(setting::<i64>(
            &mut conn,
            SettingsPlace::Provider(provider),
            keys::RUNTIME_IDLE_STOP_MINUTES,
        )
        .await?
        .unwrap_or(0))
    }

    /// Whether the runtime has been idle for as long as its provider's setting allows: nothing is under
    /// way on it (no turn, no lease, no session, no sign-in that waits for the person) and the last
    /// thing that was is longer ago than the timeout.
    async fn runtime_is_idle(
        &self,
        provider: &State,
        runtime: &RuntimeFull,
    ) -> Result<bool, EngineError> {
        let minutes = self.idle_minutes(provider.id).await?;
        if minutes <= 0 {
            return Ok(false);
        }
        let mut conn = self.inner.db.acquire().await?;
        let busy = rows::busy(&mut conn, provider.id).await?;
        if busy.turns > 0 || busy.leases > 0 {
            return Ok(false);
        }
        let signing_in = provider
            .login_state
            .as_ref()
            .and_then(|state| state.get("state"))
            .and_then(orch_types::Json::as_str)
            .is_some_and(|word| matches!(word, "starting" | "awaiting_user"));
        if signing_in || provider.login_pending.is_some() {
            return Ok(false);
        }
        let seen = rows::activity_at(&mut conn, provider.id).await?;
        let last = [runtime.last_active_at, seen, Some(runtime.created_at)]
            .into_iter()
            .flatten()
            .max()
            .unwrap_or(runtime.created_at);
        Ok(self.inner.clock.now() - last >= chrono::Duration::minutes(minutes))
    }

    // ------------------------------------------------------------ with the provider

    /// Says that the unit of a provider's runtime failed, in a sentence: the provider says why it is not
    /// ready, and a sign-in that waited for the unit is forgotten with its secrets.
    pub(crate) async fn runtime_failed(
        &self,
        provider: Id,
        detail: &str,
        now: DateTime<Utc>,
    ) -> Result<(), EngineError> {
        let mut tx = self.inner.db.begin().await?;
        let mut forgotten = None;
        if let Some(state) = rows::lock(&mut tx, provider).await?
            && state.deleted_at.is_none()
        {
            forgotten = state
                .login_pending
                .as_deref()
                .and_then(|bytes| {
                    serde_json::from_slice::<crate::provider_setup::Pending>(bytes).ok()
                })
                .map(|pending| pending.method);
            rows::delete_login_secrets(&mut tx, provider, rows::LOGIN_SECRET_PREFIX).await?;
            rows::clear_pending(&mut tx, provider).await?;
            if matches!(state.status.as_str(), "needs_setup" | "ready") {
                orch_db::queries::resources::keeper::move_status(
                    &mut tx,
                    provider,
                    &["needs_setup", "ready"],
                    "needs_setup",
                    Some(detail),
                    now,
                )
                .await?;
            }
        }
        tx.commit().await.map_err(DbError::from)?;
        // The person who began a sign-in that waited for the unit is told that it will not go on.
        if let Some(method) = forgotten {
            self.note_login(
                provider,
                &serde_json::json!({
                    "state": "failed", "method": method, "shows": [], "asks": null,
                    "expires_at": null,
                    "error": format!("the provider's runtime could not be made ready: {detail}"),
                }),
            )
            .await?;
        }
        self.reevaluate_provider(provider).await?;
        Ok(())
    }

    // ------------------------------------------------------------ the schedule

    /// What the schedule does about providers' runtimes at its own pace: finishes the stops a process that
    /// died began, stops the idle, starts the removal of what was retired, takes the sign-in that waits
    /// for a unit that is up, forgets the one that waited too long, ends the sign-in that nobody finished,
    /// looks at the facts after a sign-in that completed, and tells of a token that is about to lapse.
    pub(crate) async fn reap_provider_runtimes(&self) -> Result<(), EngineError> {
        self.finish_stale_provider_stops().await?;
        self.stop_idle_providers().await?;
        self.start_due_provider_removals().await?;
        self.take_waiting_sign_ins().await?;
        self.look_after_sign_ins().await?;
        self.watch_token_ends().await?;
        self.refresh_due_provider_facts().await?;
        Ok(())
    }

    async fn finish_stale_provider_stops(&self) -> Result<(), EngineError> {
        let wait =
            chrono::Duration::from_std(self.inner.config.hosting.stop_wait).unwrap_or_default();
        let before = self.inner.clock.now() - wait - wait;
        let stale = {
            let mut conn = self.inner.db.acquire().await?;
            rows::stale_stops(&mut conn, before, REAP_BATCH).await?
        };
        for runtime in stale {
            if let Some(provider) = runtime.provider_id {
                let engine = self.clone();
                tokio::spawn(async move {
                    if let Err(error) = engine.finish_provider_stop(provider, runtime.id).await {
                        tracing::warn!(%error, %provider, "a stop of a provider's runtime that was begun could not be finished");
                    }
                });
            }
        }
        Ok(())
    }

    async fn stop_idle_providers(&self) -> Result<(), EngineError> {
        let running = {
            let mut conn = self.inner.db.acquire().await?;
            rows::running(&mut conn, REAP_BATCH).await?
        };
        for runtime in running {
            let Some(provider) = runtime.provider_id else {
                continue;
            };
            let state = {
                let mut conn = self.inner.db.acquire().await?;
                rows::state(&mut conn, provider).await?
            };
            let Some(state) = state.filter(|state| state.deleted_at.is_none()) else {
                continue;
            };
            if !self.runtime_is_idle(&state, &runtime).await? {
                continue;
            }
            // A stop of this one that an earlier look began is not over: asking again before it has
            // answered is a second command to stop the same unit.
            if !lock(&self.inner.stopping).insert(provider) {
                continue;
            }
            let engine = self.clone();
            tokio::spawn(async move {
                let stopped = engine
                    .stop_provider_runtime(provider, false, StopWhy::Idle)
                    .await;
                lock(&engine.inner.stopping).remove(&provider);
                if let Err(error) = stopped {
                    tracing::debug!(%error, %provider, "an idle provider's runtime was not stopped");
                }
            });
        }
        Ok(())
    }

    async fn start_due_provider_removals(&self) -> Result<(), EngineError> {
        let now = self.inner.clock.now();
        let retired = {
            let mut conn = self.inner.db.acquire().await?;
            rows::retired_due(&mut conn, now, REAP_BATCH).await?
        };
        let mut queued = false;
        for runtime in retired {
            let Some(provider) = runtime.provider_id else {
                continue;
            };
            let mut tx = self.inner.db.begin().await?;
            match provider_provision::queue_removal(&mut tx, now, provider, runtime.id).await? {
                provider_jobs::Queued::Made(_) => {
                    tx.commit().await.map_err(DbError::from)?;
                    queued = true;
                }
                provider_jobs::Queued::Running(_) | provider_jobs::Queued::NoProvider => {}
            }
        }
        if queued {
            self.kick();
        }
        Ok(())
    }
}

// ---------------------------------------------------------------- what the rest of the system asks

/// Whether a turn on this provider finds its runtime not up and starts it: the provider is a managed
/// one that is not deleted or switched off, and has no machine that is there. Pre-flight does not ask
/// such a provider's machine; the start of the turn does what is needed.
pub(crate) async fn waits_for_runtime(
    conn: &mut orch_db::Conn,
    provider: &machine_rows::ProviderRow,
) -> Result<bool, EngineError> {
    let paused: bool =
        sqlx::query_scalar("SELECT runtime_unavailable IS NOT NULL FROM providers WHERE id=$1")
            .bind(provider.id)
            .fetch_one(&mut *conn)
            .await
            .map_err(DbError::from)?;
    if paused {
        return Ok(false);
    }
    if provider.deleted_at.is_some() || provider.status == "disabled" {
        return Ok(false);
    }
    if orch_db::queries::environment_facts::of_provider(conn, provider.id)
        .await?
        .is_some_and(|facts| facts.attaches())
    {
        return Ok(false);
    }
    if provider.machine_id.is_some() && rows::live_runtime(conn, provider.id).await?.is_none() {
        return Ok(false);
    }
    let Some(machine) = provider.machine_id else {
        return Ok(true);
    };
    Ok(!machine_rows::machine_place(conn, machine)
        .await?
        .is_some_and(|place| {
            place.online && place.status == "active" && place.deleted_at.is_none()
        }))
}

/// Retires the runtime of a provider that is deleted, in the caller's transaction: its unit and its
/// `state` volume, with the sign-in in it, wait for the reaper to remove them. Returns the runtime that
/// was retired.
pub async fn retire_runtime(
    conn: &mut orch_db::Conn,
    provider: Id,
    now: DateTime<Utc>,
) -> Result<Option<Id>, EngineError> {
    let Some(runtime) = rows::live_runtime(conn, provider).await? else {
        return Ok(None);
    };
    env_rows::retire_runtime(conn, runtime.id, Some("its provider was deleted"), now).await?;
    rows::set_machine(conn, provider, None, now).await?;
    rows::delete_login_secrets(conn, provider, rows::LOGIN_SECRET_PREFIX).await?;
    rows::clear_pending(conn, provider).await?;
    Ok(Some(runtime.id))
}

impl Engine {
    /// What follows a job about a provider's runtime that ended: the threads that waited for the
    /// provider look again, and the sign-in that waited for the unit is given to its harness, or
    /// forgotten when the unit was not made.
    pub(crate) async fn after_provider_job(&self, row: &JobRow, provider: Id) {
        if let Err(error) = self.reevaluate_provider(provider).await {
            tracing::warn!(%error, %provider, "the threads of a provider could not be woken");
        }
        if row.kind != provider_jobs::PROVISION {
            return;
        }
        let ended = match self.inner.db.acquire().await {
            Ok(mut conn) => orch_db::queries::jobs::job(&mut conn, row.id)
                .await
                .ok()
                .flatten(),
            Err(_) => None,
        };
        let Some(ended) = ended else {
            return;
        };
        if ended.state == orch_db::queries::jobs::state::SUCCEEDED {
            if let Err(error) = self.deliver_sign_in(provider).await {
                tracing::warn!(%error, %provider, "a sign-in that waited for the unit could not be given to the harness");
            }
            // Renewal has already performed the immediate read in its refresh phase.
            if ended.plan.get("mode").and_then(orch_types::Json::as_str)
                != Some(crate::jobs::provider_provision::Mode::Renew.word())
                && let Err(error) = self.refresh_provider_now(provider).await
            {
                tracing::debug!(%error,%provider,"provider facts were not available after its runtime started");
            }
            let selected = match self.inner.db.acquire().await {
                Ok(mut conn) => machine_rows::provider(&mut conn, provider)
                    .await
                    .ok()
                    .flatten()
                    .is_some_and(|row| {
                        row.sources
                            .get("models")
                            .is_some_and(|choice| !choice.is_null())
                    }),
                Err(_) => false,
            };
            if selected && let Err(error) = self.discover_provider_models(provider).await {
                tracing::debug!(%error,%provider,"provider models were not available after its runtime started");
            }
        } else {
            let said = ended
                .error
                .as_ref()
                .and_then(|error| error.get("message"))
                .and_then(orch_types::Json::as_str)
                .unwrap_or("it was cancelled")
                .to_owned();
            if let Err(error) = self
                .runtime_failed(
                    provider,
                    &format!("its runtime could not be made: {said}"),
                    self.inner.clock.now(),
                )
                .await
            {
                tracing::warn!(%error, %provider, "a sign-in that waited for a unit that did not come up could not be forgotten");
            }
        }
    }

    /// The unit of a provider's runtime was found failed by its host: the provider has no machine, its
    /// harnesses there are not online, and the sessions the engine thought open there are not.
    pub(crate) async fn provider_unit_failed(
        &self,
        conn: &mut orch_db::Conn,
        provider: Id,
        machine: Option<Id>,
        now: DateTime<Utc>,
    ) -> Result<(), EngineError> {
        self.unit_is_gone(conn, provider, machine, now).await
    }
}
