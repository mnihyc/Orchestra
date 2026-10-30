//! Display lifecycle checks the environment door and audits only safe committed facts.

pub use crate::browsers::{
    BrowserFailure as DisplayFailure, CLOSED_LIST_KEPT, Requester, SITTING_GAP,
};
use crate::commands::{Outcome, Outgoing};
use crate::engine::Engine;
use crate::error::EngineError;
use async_trait::async_trait;
use orch_access_contract::decisions::ObjectRef;
use orch_access_contract::permissions::{ObjectKind, Permission};
use orch_api_contract::browsers::BrowsersOff;
use orch_authz::{Policy, Principal};
use orch_db::audit::{self, Entry, Origin};
use orch_db::queries::{displays as rows, threads};
use orch_link_contract::commands::{CommandBody, Secret, SerialKey};
use orch_link_contract::display::{DisplayAdopt, DisplayClose, DisplayOpen, DisplayState};
use orch_types::{Id, Json, new_id};
use serde_json::json;
use std::sync::{Arc, PoisonError, RwLock};
use std::time::Duration;

/// The longest display command takes thirty seconds, with ten seconds for a busy link.
const COMMAND_WAIT: Duration = Duration::from_secs(40);
/// Matches the shipped display policy.
pub const DEFAULT_MAX: i64 = 4;
/// The shipped laptop-sized virtual screen.
pub const DEFAULT_GEOMETRY: &str = "1280x800";
/// The contract's short purpose line, counted as Unicode characters.
pub const NOTE_MAX: usize = 200;

/// The API supplies at-rest sealing and publishes whole transient display lists.
#[async_trait]
pub trait DisplayActions: Send + Sync {
    /// Seals a server credential to this display's row; it must never be logged.
    fn seal_password(&self, display: Id, password: &[u8]) -> Result<Vec<u8>, String>;
    /// Publishes after a lifecycle change has committed.
    async fn changed(&self, environment: Id);
}

/// Application hooks contain no images or input.
#[derive(Default)]
pub struct Memory {
    actions: RwLock<Option<Arc<dyn DisplayActions>>>,
}

/// A request to open a virtual screen.
pub struct OpenDisplay {
    /// Its environment.
    pub environment: Id,
    /// Who asked.
    pub requester: Requester,
    /// The associated thread.
    pub thread: Option<Id>,
    /// Its listed name, or the X display by default.
    pub name: Option<String>,
    /// Its dimensions, or the resolved setting.
    pub geometry: Option<String>,
    /// The agent's purpose line.
    pub note: Option<String>,
    /// The request's safe audit origin.
    pub origin: Origin,
}

/// Display operations using the existing command and environment machinery.
#[derive(Clone)]
pub struct Displays {
    engine: Engine,
}

impl Engine {
    /// The display lifecycle service.
    pub fn displays(&self) -> Displays {
        Displays {
            engine: self.clone(),
        }
    }
    /// Installs the application's sealer and transient publication hook.
    pub fn install_display_actions(&self, actions: Arc<dyn DisplayActions>) {
        *self
            .inner
            .display_memory
            .actions
            .write()
            .unwrap_or_else(PoisonError::into_inner) = Some(actions);
    }
}

impl Displays {
    fn actions(&self) -> Option<Arc<dyn DisplayActions>> {
        self.engine
            .inner
            .display_memory
            .actions
            .read()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }
    /// Publishes the complete current list after committing a change.
    pub async fn changed(&self, environment: Id) {
        if let Some(actions) = self.actions() {
            actions.changed(environment).await;
        }
    }

    /// Watching keeps the environment awake without storing a frame or a person's input.
    pub async fn used(&self, id: Id) -> Result<(), EngineError> {
        let mut conn = self.engine.inner.db.acquire().await?;
        if let Some(row) = rows::get(&mut conn, id).await? {
            rows::touch(&mut conn, id, self.engine.inner.clock.now()).await?;
            orch_db::queries::terminals::touch_environment(
                &mut conn,
                row.environment_id,
                self.engine.inner.clock.now(),
            )
            .await?;
        }
        Ok(())
    }

    /// Resolves policy, the environment switch, local policy and actual reachability separately.
    pub async fn off(
        conn: &mut orch_db::Conn,
        site: &rows::Site,
    ) -> Result<Option<BrowsersOff>, EngineError> {
        let enabled = orch_settings::get::<bool>(
            conn,
            orch_settings::Place::Environment(site.id),
            "vnc.enabled",
        )
        .await?
        .unwrap_or(true);
        if !enabled {
            return Ok(Some(BrowsersOff::DisabledByPolicy));
        }
        if !site.vnc {
            return Ok(Some(BrowsersOff::EnvironmentSwitchOff));
        }
        if orch_db::queries::environment_facts::refuses(conn, site.id, "environment.vnc").await? {
            return Ok(Some(BrowsersOff::MachinePolicy));
        }
        if site.status != "running"
            || site.machine_id.is_none()
            || site.machine_online != Some(true)
        {
            return Ok(Some(BrowsersOff::EnvironmentOffline));
        }
        Ok(None)
    }

    async fn check(
        &self,
        conn: &mut orch_db::Conn,
        site: &rows::Site,
        requester: &Requester,
    ) -> Result<(), DisplayFailure> {
        match requester {
            Requester::Person(caller) => {
                let principal =
                    Principal::load(conn, caller.clone(), self.engine.inner.clock.time()).await?;
                let decision = orch_authz::resources::check(
                    conn,
                    &principal,
                    ObjectRef {
                        kind: ObjectKind::Environment,
                        id: site.id,
                    },
                    Permission::VncManage,
                    Policy::default(),
                )
                .await?;
                if !decision.is_allowed() {
                    return Err(DisplayFailure::Permission(Box::new(decision)));
                }
            }
            Requester::Agent(thread) => {
                crate::tools::source::check(conn).await?;
                if !threads::thread(conn, *thread)
                    .await?
                    .is_some_and(|t| t.environment_id == Some(site.id) && t.deleted_at.is_none())
                {
                    return Err(DisplayFailure::NotFound);
                }
            }
            Requester::System => return Ok(()),
        }
        if let Some(off) = Self::off(conn, site).await? {
            return Err(DisplayFailure::Off(off));
        }
        Ok(())
    }

    async fn ask<B: CommandBody>(
        &self,
        site: &rows::Site,
        key: Id,
        body: &B,
    ) -> Result<B::Result, DisplayFailure> {
        let machine = site
            .machine_id
            .ok_or(DisplayFailure::Off(BrowsersOff::EnvironmentOffline))?;
        let command = Outgoing::new(key, machine, SerialKey::Environment(site.id), body)
            .map_err(EngineError::from)?;
        let command = crate::tools::source::issue(&self.engine, command).await?;
        match tokio::time::timeout(COMMAND_WAIT, self.engine.inner.commands.wait(command.id)).await
        {
            Ok(Ok(Outcome::Succeeded(value))) => {
                #[cfg(test)]
                if B::KIND == orch_link_contract::commands::CommandKind::DisplayOpen {
                    crate::tools::source::tests::completed_native(
                        crate::tools::source::current_thread().unwrap_or_default(),
                    )
                    .await;
                }
                serde_json::from_value(value)
                    .map_err(|e| DisplayFailure::Engine(EngineError::unreadable(e)))
            }
            Ok(Ok(Outcome::Failed(error))) => Err(DisplayFailure::Command(error)),
            _ => {
                if !crate::tools::source::is_tool_call() {
                    let _ = self.engine.inner.commands.cancel(command.id).await;
                }
                Err(DisplayFailure::Unreachable(
                    "the display did not answer in time".to_owned(),
                ))
            }
        }
    }

    /// Opens a virtual screen with an atomic shared limit reservation.
    pub async fn open(&self, request: OpenDisplay) -> Result<rows::DisplayRow, DisplayFailure> {
        validate_note(request.note.as_deref())?;
        if request
            .name
            .as_ref()
            .is_some_and(|name| name.trim().is_empty())
        {
            return Err(DisplayFailure::Invalid("name must not be empty".to_owned()));
        }
        let id = crate::tools::source::resource_id("display.open", new_id());
        let (site, geometry) = {
            let mut tx = self.engine.inner.db.begin().await?;
            rows::lock(&mut tx, request.environment).await?;
            let site = rows::site(&mut tx, request.environment)
                .await?
                .ok_or(DisplayFailure::NotFound)?;
            self.check(&mut tx, &site, &request.requester).await?;
            if let Some(thread) = request.thread
                && !threads::thread(&mut tx, thread)
                    .await?
                    .is_some_and(|t| t.environment_id == Some(site.id) && t.deleted_at.is_none())
            {
                return Err(DisplayFailure::NotFound);
            }
            if site.facts["vnc"]["servers"]
                .as_array()
                .is_none_or(|servers| servers.is_empty())
            {
                return Err(DisplayFailure::Unsupported);
            }
            let existing = rows::get(&mut tx, id).await?;
            if let Some(row) = &existing {
                if row.environment_id != site.id
                    || row.env_generation != site.generation
                    || row.thread_id != request.thread
                    || row.opened_by != request.requester.person()
                    || row.origin != "started"
                    || row.state == "closed"
                {
                    return Err(DisplayFailure::Engine(EngineError::ToolCallIdentity));
                }
                if row.state == "open" {
                    return Ok(row.clone());
                }
            }
            let max = orch_settings::get::<i64>(
                &mut tx,
                orch_settings::Place::Environment(site.id),
                "vnc.max_per_environment",
            )
            .await
            .map_err(EngineError::from)?
            .unwrap_or(DEFAULT_MAX);
            let open = rows::owned_count(&mut tx, site.id).await?;
            if existing.is_none() && open >= max {
                return Err(DisplayFailure::TooMany { open, max });
            }
            let geometry = match existing
                .as_ref()
                .and_then(|row| row.geometry.clone())
                .or_else(|| request.geometry.clone())
            {
                Some(value) => value,
                None => orch_settings::get::<String>(
                    &mut tx,
                    orch_settings::Place::Environment(site.id),
                    "vnc.geometry",
                )
                .await
                .map_err(EngineError::from)?
                .unwrap_or_else(|| DEFAULT_GEOMETRY.to_owned()),
            };
            validate_geometry(&geometry)?;
            if existing.is_none() {
                rows::insert(
                    &mut tx,
                    &rows::NewDisplay {
                        id,
                        environment: site.id,
                        generation: site.generation,
                        name: request.name.as_deref().unwrap_or(""),
                        origin: "started",
                        person: request.requester.person(),
                        thread: request.thread,
                        note: request.note.as_deref(),
                        password: None,
                    },
                    self.engine.inner.clock.now(),
                )
                .await?;
                sqlx::query("UPDATE display_sessions SET geometry=$2 WHERE id=$1")
                    .bind(id)
                    .bind(&geometry)
                    .execute(&mut *tx)
                    .await
                    .map_err(orch_db::DbError::from)?;
            }
            tx.commit().await.map_err(orch_db::DbError::from)?;
            (site, geometry)
        };
        self.changed(site.id).await;
        let result = self
            .ask(
                &site,
                crate::ids::derived("display.open", &[id.as_bytes()]),
                &DisplayOpen {
                    environment_id: site.id,
                    display_id: id,
                    geometry: geometry.clone(),
                },
            )
            .await;
        match result {
            Ok(result) => {
                self.finish(
                    &site,
                    id,
                    &request.requester,
                    &result.endpoint,
                    Some(&result.x_display),
                    None,
                    Some(&geometry),
                    &request.origin,
                )
                .await
            }
            Err(error) => {
                self.failed(&site, id).await?;
                Err(error)
            }
        }
    }

    /// Announces only an agent's verified local server, sealing its password before any durable write.
    pub async fn adopt(
        &self,
        environment: Id,
        thread: Id,
        endpoint: String,
        password: Option<String>,
        note: Option<String>,
    ) -> Result<rows::DisplayRow, DisplayFailure> {
        validate_note(note.as_deref())?;
        let id = crate::tools::source::resource_id("display.announce", new_id());
        let requester = Requester::Agent(thread);
        let sealed = match password.as_deref() {
            Some(password) => {
                if password.len() > orch_settings::secrets::SECRET_MAX_BYTES {
                    return Err(DisplayFailure::Invalid(
                        "the display credential is too large".to_owned(),
                    ));
                }
                Some(
                    self.actions()
                        .ok_or_else(|| {
                            DisplayFailure::Unreachable(
                                "display credential storage is unavailable".to_owned(),
                            )
                        })?
                        .seal_password(id, password.as_bytes())
                        .map_err(DisplayFailure::Unreachable)?,
                )
            }
            None => None,
        };
        let site = {
            let mut tx = self.engine.inner.db.begin().await?;
            rows::lock(&mut tx, environment).await?;
            let site = rows::site(&mut tx, environment)
                .await?
                .ok_or(DisplayFailure::NotFound)?;
            self.check(&mut tx, &site, &requester).await?;
            if let Some(row) = rows::get(&mut tx, id).await? {
                if row.environment_id != environment
                    || row.env_generation != site.generation
                    || row.thread_id != Some(thread)
                    || row.opened_by.is_some()
                    || row.origin != "announced"
                    || row.state == "closed"
                {
                    return Err(DisplayFailure::Engine(EngineError::ToolCallIdentity));
                }
                if row.state == "open" {
                    return Ok(row);
                }
            } else {
                rows::insert(
                    &mut tx,
                    &rows::NewDisplay {
                        id,
                        environment,
                        generation: site.generation,
                        name: "",
                        origin: "announced",
                        person: None,
                        thread: Some(thread),
                        note: note.as_deref(),
                        password: sealed.as_deref(),
                    },
                    self.engine.inner.clock.now(),
                )
                .await?;
            }
            tx.commit().await.map_err(orch_db::DbError::from)?;
            site
        };
        self.changed(environment).await;
        let result = self
            .ask(
                &site,
                new_id(),
                &DisplayAdopt {
                    environment_id: environment,
                    display_id: id,
                    endpoint: endpoint.clone(),
                    password: sealed.as_ref().map(|_| Secret::Ref(id)),
                },
            )
            .await;
        match result {
            Ok(result) => {
                self.finish(
                    &site,
                    id,
                    &requester,
                    &endpoint,
                    None,
                    result.name.as_deref(),
                    result.geometry.as_deref(),
                    &Origin::default(),
                )
                .await
            }
            Err(error) => {
                self.failed(&site, id).await?;
                Err(error)
            }
        }
    }

    async fn failed(&self, site: &rows::Site, id: Id) -> Result<(), DisplayFailure> {
        let _ = self
            .ask(site, new_id(), &DisplayClose { display_id: id })
            .await;
        let mut conn = self.engine.inner.db.acquire().await?;
        rows::drop_opening(&mut conn, id).await?;
        drop(conn);
        self.changed(site.id).await;
        Ok(())
    }

    #[expect(
        clippy::too_many_arguments,
        reason = "the completion binds the exact reserved display, metadata, actor and origin"
    )]
    async fn finish(
        &self,
        before: &rows::Site,
        id: Id,
        requester: &Requester,
        endpoint: &str,
        x_display: Option<&str>,
        name: Option<&str>,
        geometry: Option<&str>,
        origin: &Origin,
    ) -> Result<rows::DisplayRow, DisplayFailure> {
        let mut tx = self.engine.inner.db.begin().await?;
        rows::lock(&mut tx, before.id).await?;
        let site = rows::site(&mut tx, before.id)
            .await?
            .ok_or(DisplayFailure::NotFound)?;
        let checked = self.check(&mut tx, &site, requester).await;
        if checked.is_err()
            || site.generation != before.generation
            || site.machine_id != before.machine_id
        {
            drop(tx);
            self.failed(before, id).await?;
            return Err(checked.err().unwrap_or(DisplayFailure::NotFound));
        }
        if let Some(row) = rows::get(&mut tx, id).await?
            && row.state == "open"
        {
            tx.commit().await.map_err(orch_db::DbError::from)?;
            return Ok(row);
        }
        let Some(row) = rows::opened(
            &mut tx,
            id,
            endpoint,
            x_display,
            name,
            geometry,
            self.engine.inner.clock.now(),
        )
        .await?
        else {
            drop(tx);
            self.failed(before, id).await?;
            return Err(DisplayFailure::NotFound);
        };
        history(
            &mut tx,
            &site,
            requester,
            audit::name!("display.opened"),
            &row,
            json!({"origin":row.origin}),
            origin,
            self.engine.inner.clock.now(),
        )
        .await?;
        tx.commit().await.map_err(orch_db::DbError::from)?;
        self.changed(site.id).await;
        Ok(row)
    }

    /// Closes once with a history line; only owned programs are stopped at the host.
    pub async fn close(
        &self,
        id: Id,
        reason: &str,
        requester: Requester,
        origin: &Origin,
    ) -> Result<Option<rows::DisplayRow>, DisplayFailure> {
        let mut tx = self.engine.inner.db.begin().await?;
        let Some(before) = rows::get(&mut tx, id).await? else {
            return Ok(None);
        };
        rows::lock(&mut tx, before.environment_id).await?;
        let site = rows::site(&mut tx, before.environment_id)
            .await?
            .ok_or(DisplayFailure::NotFound)?;
        self.check(&mut tx, &site, &requester).await?;
        if let Requester::Agent(thread) = requester
            && (before.opened_by.is_some() || before.thread_id != Some(thread))
        {
            return Err(DisplayFailure::Invalid("not_yours".to_owned()));
        }
        let row = rows::close(&mut tx, id, reason, self.engine.inner.clock.now()).await?;
        if let Some(row) = &row {
            history(
                &mut tx,
                &site,
                &requester,
                audit::name!("display.closed"),
                row,
                json!({"origin":row.origin,"reason":reason}),
                origin,
                self.engine.inner.clock.now(),
            )
            .await?;
        }
        tx.commit().await.map_err(orch_db::DbError::from)?;
        if row.is_some() && site.machine_online == Some(true) {
            let _ = self
                .ask(&site, new_id(), &DisplayClose { display_id: id })
                .await;
        }
        self.changed(site.id).await;
        Ok(row)
    }

    /// Applies only the current owning connector's report in its acknowledgement transaction.
    pub async fn apply(
        &self,
        conn: &mut orch_db::Conn,
        machine: Id,
        environment: Id,
        state: &DisplayState,
    ) -> Result<(), EngineError> {
        let Some(row) = rows::get(conn, state.display_id).await? else {
            return Ok(());
        };
        let Some(site) = rows::site(conn, environment).await? else {
            return Ok(());
        };
        if row.environment_id != environment
            || site.machine_id != Some(machine)
            || row.env_generation != site.generation
            || row.state == "closed"
        {
            return Ok(());
        }
        if state.state == orch_link_contract::browser::BrowserReportedState::Closed {
            let reason = state
                .reason
                .as_deref()
                .filter(|r| {
                    matches!(
                        *r,
                        "closed_by_user"
                            | "closed_by_agent"
                            | "exited"
                            | "environment_stopped"
                            | "link_lost"
                            | "switched_off"
                            | "error"
                    )
                })
                .unwrap_or("exited");
            if let Some(row) =
                rows::close(conn, row.id, reason, self.engine.inner.clock.now()).await?
            {
                history(
                    conn,
                    &site,
                    &Requester::System,
                    "display.closed",
                    &row,
                    json!({"reason":reason}),
                    &Origin::default(),
                    self.engine.inner.clock.now(),
                )
                .await?;
            }
        }
        Ok(())
    }

    /// Writes a sitting's actor and times without retaining any actions, text or pixels.
    pub async fn sitting(
        &self,
        display: Id,
        user: Id,
        from: chrono::DateTime<chrono::Utc>,
        to: chrono::DateTime<chrono::Utc>,
    ) -> Result<(), EngineError> {
        let mut tx = self.engine.inner.db.begin().await?;
        let Some(row) = rows::get(&mut tx, display).await? else {
            return Ok(());
        };
        let Some(site) = rows::history_site(&mut tx, row.environment_id).await? else {
            return Ok(());
        };
        rows::lock(&mut tx, site.id).await?;
        let already:bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM audit_log WHERE action='display.driven' AND object_id=$1 AND actor_kind='user' AND actor_id=$2 AND details->'from'=$3 AND details->'to'=$4)")
            .bind(display).bind(user).bind(json!(from)).bind(json!(to)).fetch_one(&mut *tx).await.map_err(orch_db::DbError::from)?;
        if already {
            return Ok(());
        }
        let mut entry = Entry::new(
            orch_db::audit::Actor::User(user),
            audit::name!("display.driven"),
            "display",
            Some(display),
            "A person worked in a display.",
        );
        entry.org_id = Some(site.org_id);
        entry.environment_id = Some(site.id);
        entry.node_id = row.thread_id;
        entry.history = true;
        entry.details = json!({"from":from,"to":to});
        audit::append(&mut tx, &entry, &Origin::default(), to).await?;
        tx.commit().await.map_err(orch_db::DbError::from)?;
        Ok(())
    }

    /// Closes only current open rows omitted from a completed screen discovery.
    pub async fn reconcile(
        &self,
        conn: &mut orch_db::Conn,
        site: &rows::Site,
        found: &[orch_link_contract::browser::ScreenLookDisplay],
    ) -> Result<(), EngineError> {
        for row in rows::list(
            conn,
            site.id,
            self.engine.inner.clock.now() - CLOSED_LIST_KEPT,
        )
        .await?
        {
            if row.state == "open"
                && row.env_generation == site.generation
                && !found.iter().any(|d| d.display_id == row.id)
                && let Some(row) =
                    rows::close(conn, row.id, "link_lost", self.engine.inner.clock.now()).await?
            {
                history(
                    conn,
                    site,
                    &Requester::System,
                    "display.closed",
                    &row,
                    json!({"reason":"link_lost"}),
                    &Origin::default(),
                    self.engine.inner.clock.now(),
                )
                .await?;
            }
        }
        Ok(())
    }

    /// Repairs ended generations and policy withdrawal even when no viewer is connected.
    pub async fn keep(&self) -> Result<(), DisplayFailure> {
        let mut conn = self.engine.inner.db.acquire().await?;
        let live = rows::live(&mut conn).await?;
        drop(conn);
        for row in live {
            let mut conn = self.engine.inner.db.acquire().await?;
            let site = rows::site(&mut conn, row.environment_id).await?;
            let reason = match &site {
                None => Some("environment_stopped"),
                Some(site)
                    if site.generation != row.env_generation
                        || !matches!(site.status.as_str(), "running" | "offline") =>
                {
                    Some("environment_stopped")
                }
                Some(_)
                    if row.state == "opening"
                        && self.engine.inner.clock.now() - row.opened_at
                            > chrono::Duration::from_std(COMMAND_WAIT)
                                .unwrap_or(chrono::Duration::MAX) =>
                {
                    Some("link_lost")
                }
                Some(site) => match Self::off(&mut conn, site).await? {
                    Some(BrowsersOff::EnvironmentOffline) => None,
                    Some(_) => Some("switched_off"),
                    None => None,
                },
            };
            drop(conn);
            if let Some(reason) = reason {
                if site.is_some() {
                    let _ = self
                        .close(row.id, reason, Requester::System, &Origin::default())
                        .await?;
                } else {
                    let mut tx = self.engine.inner.db.begin().await?;
                    if let Some(site) = rows::history_site(&mut tx, row.environment_id).await?
                        && let Some(row) =
                            rows::close(&mut tx, row.id, reason, self.engine.inner.clock.now())
                                .await?
                    {
                        history(
                            &mut tx,
                            &site,
                            &Requester::System,
                            "display.closed",
                            &row,
                            json!({"reason":reason}),
                            &Origin::default(),
                            self.engine.inner.clock.now(),
                        )
                        .await?;
                    }
                    tx.commit().await.map_err(orch_db::DbError::from)?;
                }
            }
        }
        Ok(())
    }
}

fn validate_note(note: Option<&str>) -> Result<(), DisplayFailure> {
    if note.is_some_and(|n| n.chars().count() > NOTE_MAX) {
        return Err(DisplayFailure::Invalid(
            "note must be at most 200 characters".to_owned(),
        ));
    }
    Ok(())
}

/// Enforces the display setting's declared width and height range before a command is written.
pub fn validate_geometry(value: &str) -> Result<(), DisplayFailure> {
    let dimensions = value
        .split_once('x')
        .and_then(|(w, h)| Some((w.parse::<u16>().ok()?, h.parse::<u16>().ok()?)));
    match dimensions {
        Some((320..=3840, 240..=2160)) => Ok(()),
        _ => Err(DisplayFailure::Invalid(
            "geometry must be 320x240 through 3840x2160".to_owned(),
        )),
    }
}

#[expect(
    clippy::too_many_arguments,
    reason = "a factual audit names its actor, object, place, action and time explicitly"
)]
async fn history(
    conn: &mut orch_db::Conn,
    site: &rows::Site,
    requester: &Requester,
    action: &str,
    row: &rows::DisplayRow,
    details: Json,
    origin: &Origin,
    at: chrono::DateTime<chrono::Utc>,
) -> Result<(), EngineError> {
    let mut entry = Entry::new(
        requester.actor(),
        action,
        "display",
        Some(row.id),
        if action == "display.opened" {
            "A display was opened."
        } else {
            "A display was closed."
        },
    );
    entry.history = true;
    entry.org_id = Some(site.org_id);
    entry.environment_id = Some(site.id);
    entry.node_id = row.thread_id;
    entry.details = details;
    audit::append(conn, &entry, origin, at).await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn a_screen_size_and_purpose_are_refused_before_a_program_is_started() {
        assert!(validate_geometry("1280x800").is_ok());
        for geometry in ["319x800", "1280x2161", "1280x800 -SecurityTypes VncAuth"] {
            assert!(validate_geometry(geometry).is_err());
        }
        assert!(validate_note(Some(&"界".repeat(NOTE_MAX))).is_ok());
        assert!(validate_note(Some(&"界".repeat(NOTE_MAX + 1))).is_err());
    }
}
