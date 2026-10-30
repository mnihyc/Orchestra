//! Browser lifecycle, profile handouts, idle closure and history without page content.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::{Arc, PoisonError, RwLock};
use std::time::Duration;

use async_trait::async_trait;
use chrono::{DateTime, Duration as Span, Utc};
use orch_access_contract::decisions::{Decision, ObjectRef};
use orch_access_contract::permissions::{ObjectKind, Permission};
use orch_api_contract::browsers::{BrowserPreset, BrowsersOff};
use orch_authz::{Caller, Policy, Principal};
use orch_db::audit::{self, Actor, Entry, Origin};
use orch_db::queries::browsers::{self as rows, BrowserRow, NewBrowser, Site};
use orch_db::queries::threads;
use orch_link::streams::DeliveredEvent;
use orch_link_contract::browser::{
    BrowserAdopt, BrowserClose, BrowserOpen, BrowserProfile, BrowserProfileAction,
    BrowserProfileCommand, BrowserProfileResult, BrowserReportedState, BrowserState, ScreenLook,
    ScreenLookResult,
};
use orch_link_contract::commands::{CommandBody, CommandError, SerialKey};
use orch_link_contract::events::Event;
use orch_settings::Place;
use orch_settings_contract::registry::keys;
use orch_types::{Id, Json, Time, new_id};
use serde::de::DeserializeOwned;
use serde_json::json;

use crate::commands::{Outcome, Outgoing};
use crate::engine::Engine;
use crate::error::EngineError;

/// A launch has a twenty-second local bound and sixty-second command deadline; a slow link gets ten more seconds.
const COMMAND_WAIT: Duration = Duration::from_secs(70);
/// A caller joining a reservation waits in small polls, without holding a database connection.
const JOIN_POLL: Duration = Duration::from_millis(100);
/// Closed browsers remain in the current list for one day.
pub const CLOSED_LIST_KEPT: Span = Span::days(1);
/// Four independent profiles fit the default environment's memory budget.
const DEFAULT_MAX: i64 = 4;
/// A new browser's laptop-sized shared window.
const DEFAULT_VIEWPORT: &str = "1280x800";
/// A driving sitting ends five minutes after its last input.
pub const SITTING_GAP: Span = Span::minutes(5);

/// Volatile browser transport supplied by the API; page addresses never become durable commands.
#[async_trait]
pub trait BrowserActions: Send + Sync {
    /// Navigates the first tab of a just-opened browser over its live tunnel.
    async fn navigate(&self, browser: Id, url: &str) -> Result<(), String>;
    /// A committed list changed and should be published whole.
    async fn changed(&self, environment: Id);
}

/// Data that the schema deliberately does not retain: profile listings and live tab/viewer counts.
#[derive(Default)]
pub struct Memory {
    profiles: RwLock<BTreeMap<Id, Vec<BrowserProfile>>>,
    counts: RwLock<BTreeMap<Id, Option<i64>>>,
    actions: RwLock<Option<Arc<dyn BrowserActions>>>,
    watching: RwLock<BTreeMap<Id, i64>>,
}

impl Memory {
    /// The last profile list, where the connector has supplied one.
    pub fn profiles(&self, environment: Id) -> Option<Vec<BrowserProfile>> {
        self.profiles
            .read()
            .unwrap_or_else(PoisonError::into_inner)
            .get(&environment)
            .cloned()
    }
    /// The last tab count without a schema column for it.
    pub fn tabs(&self, browser: Id) -> Option<i64> {
        self.counts
            .read()
            .unwrap_or_else(PoisonError::into_inner)
            .get(&browser)
            .copied()
            .flatten()
    }
    /// The number of live viewers, supplied by the socket registry.
    pub fn watching(&self, browser: Id) -> i64 {
        *self
            .watching
            .read()
            .unwrap_or_else(PoisonError::into_inner)
            .get(&browser)
            .unwrap_or(&0)
    }
    /// Changes the live viewer count.
    pub fn set_watching(&self, browser: Id, count: i64) {
        self.watching
            .write()
            .unwrap_or_else(PoisonError::into_inner)
            .insert(browser, count);
    }

    fn opened_profile(&self, environment: Id, profile: &str, browser: Id, at: Time) {
        let mut all = self
            .profiles
            .write()
            .unwrap_or_else(PoisonError::into_inner);
        let profiles = all.entry(environment).or_default();
        if let Some(found) = profiles.iter_mut().find(|p| p.name == profile) {
            found.browser = Some(browser);
            found.used_at = Some(at);
        } else {
            profiles.push(BrowserProfile {
                name: profile.to_owned(),
                size_bytes: None,
                used_at: Some(at),
                browser: Some(browser),
            });
            profiles.sort_by(|a, b| a.name.cmp(&b.name));
        }
    }

    /// Forgets what is held of an environment that was removed: its profile list, and the tab and viewer
    /// counts of its browsers.
    pub fn forget_environment(&self, environment: Id, browsers: &[Id]) {
        self.profiles
            .write()
            .unwrap_or_else(PoisonError::into_inner)
            .remove(&environment);
        let mut counts = self.counts.write().unwrap_or_else(PoisonError::into_inner);
        let mut watching = self
            .watching
            .write()
            .unwrap_or_else(PoisonError::into_inner);
        for browser in browsers {
            counts.remove(browser);
            watching.remove(browser);
        }
    }

    fn closed_profile(&self, browser: Id) {
        let mut all = self
            .profiles
            .write()
            .unwrap_or_else(PoisonError::into_inner);
        for profiles in all.values_mut() {
            for profile in profiles.iter_mut().filter(|p| p.browser == Some(browser)) {
                profile.browser = None;
            }
        }
    }
}

/// A typed refusal of a browser lifecycle operation.
#[derive(Debug, thiserror::Error)]
pub enum BrowserFailure {
    /// Nothing with this identifier exists.
    #[error("not found")]
    NotFound,
    /// A gate that no permission lifts is closed.
    #[error("browsers are off: {0:?}")]
    Off(BrowsersOff),
    /// The requested window needs the environment's display door switched on.
    #[error("displays are off: {0:?}")]
    DisplayOff(BrowsersOff),
    /// No installed program can run the requested browser.
    #[error(
        "no browser program was found here; announce takes a browser started with another program"
    )]
    Unsupported,
    /// A permission is missing.
    #[error("a browser permission is missing")]
    Permission(Box<Decision>),
    /// Its profile name or another argument is invalid.
    #[error("{0}")]
    Invalid(String),
    /// The environment's owned browser limit is full.
    #[error("the environment has {open} browsers open and allows {max}")]
    TooMany {
        /// Those counted against the limit.
        open: i64,
        /// The resolved maximum.
        max: i64,
    },
    /// A browser window would exceed the shared virtual-display limit.
    #[error("the environment has {open} displays open and allows {max}")]
    DisplayTooMany {
        /// Owned screens currently reserved or open.
        open: i64,
        /// The resolved maximum.
        max: i64,
    },
    /// The connector refused it.
    #[error("{0}")]
    Command(CommandError),
    /// The link or transport could not complete it.
    #[error("{0}")]
    Unreachable(String),
    /// A server dependency failed.
    #[error(transparent)]
    Engine(#[from] EngineError),
}

impl From<orch_db::DbError> for BrowserFailure {
    fn from(error: orch_db::DbError) -> Self {
        Self::Engine(error.into())
    }
}
impl From<orch_authz::Error> for BrowserFailure {
    fn from(error: orch_authz::Error) -> Self {
        Self::Engine(error.into())
    }
}

/// Who requested a browser; an agent is checked against its environment, not a person's page.
#[derive(Debug, Clone)]
pub enum Requester {
    /// A person with the caller's token restrictions preserved.
    Person(Caller),
    /// The calling thread's own agent.
    Agent(Id),
    /// A lifecycle keeper or connector reconciliation.
    System,
}

impl Requester {
    pub(crate) fn actor(&self) -> Actor {
        match self {
            Self::Person(c) => Actor::User(c.user()),
            Self::Agent(t) => Actor::Agent(*t),
            Self::System => Actor::System,
        }
    }
    pub(crate) fn person(&self) -> Option<Id> {
        match self {
            Self::Person(c) => Some(c.user()),
            _ => None,
        }
    }
}

/// An open or join request, before a process or a row is made.
pub struct OpenBrowser {
    /// The environment.
    pub environment: Id,
    /// Who asked.
    pub requester: Requester,
    /// The thread beside which it was opened.
    pub thread: Option<Id>,
    /// The profile's directory name.
    pub profile: String,
    /// Make it if absent.
    pub create: bool,
    /// The preset, or the environment's setting.
    pub preset: Option<BrowserPreset>,
    /// Where its first tab goes, carried only over a live tunnel.
    pub url: Option<String>,
    /// An agent's short line about what the browser is for.
    pub note: Option<String>,
    /// The request's audit origin.
    pub origin: Origin,
}

/// The result of opening or joining a profile.
pub struct OpenedBrowser {
    /// The browser row.
    pub row: BrowserRow,
    /// Whether a process was started for this call.
    pub started: bool,
}

/// The engine's handle for browser operations.
#[derive(Clone)]
pub struct Browsers {
    engine: Engine,
}

impl Engine {
    /// The browser lifecycle service.
    #[must_use]
    pub fn browsers(&self) -> Browsers {
        Browsers {
            engine: self.clone(),
        }
    }
    /// Installs the API's unretained page transport and whole-list publication hook.
    pub fn install_browser_actions(&self, actions: Arc<dyn BrowserActions>) {
        *self
            .inner
            .browser_memory
            .actions
            .write()
            .unwrap_or_else(PoisonError::into_inner) = Some(actions);
    }
}

impl Browsers {
    /// Volatile counts and profile listings for a view.
    #[must_use]
    pub fn memory(&self) -> &Arc<Memory> {
        &self.engine.inner.browser_memory
    }

    fn actions(&self) -> Option<Arc<dyn BrowserActions>> {
        self.memory()
            .actions
            .read()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }
    /// Tells the API to publish after a change has committed.
    pub async fn changed(&self, environment: Id) {
        if let Some(actions) = self.actions() {
            actions.changed(environment).await;
        }
    }

    /// The four gates, resolved at the environment's home rather than at any viewing thread.
    pub async fn off(
        conn: &mut orch_db::Conn,
        site: &Site,
    ) -> Result<Option<BrowsersOff>, EngineError> {
        if !setting(conn, site.id, keys::BROWSER_ENABLED, true).await? {
            return Ok(Some(BrowsersOff::DisabledByPolicy));
        }
        if !site.browser {
            return Ok(Some(BrowsersOff::EnvironmentSwitchOff));
        }
        if orch_db::queries::environment_facts::refuses(conn, site.id, "environment.browser")
            .await?
        {
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
        site: &Site,
        requester: &Requester,
        permission: Permission,
    ) -> Result<(), BrowserFailure> {
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
                    permission,
                    Policy::default(),
                )
                .await?;
                if !decision.is_allowed() {
                    return Err(BrowserFailure::Permission(Box::new(decision)));
                }
            }
            Requester::Agent(thread) => {
                crate::tools::source::check(conn).await?;
                if !threads::thread(conn, *thread)
                    .await?
                    .is_some_and(|t| t.environment_id == Some(site.id) && t.deleted_at.is_none())
                {
                    return Err(BrowserFailure::NotFound);
                }
            }
            Requester::System => {}
        }
        if let Some(off) = Self::off(conn, site).await? {
            return Err(BrowserFailure::Off(off));
        }
        Ok(())
    }

    async fn ask<B: CommandBody>(
        &self,
        site: &Site,
        key: Id,
        body: &B,
    ) -> Result<B::Result, BrowserFailure> {
        let machine = site
            .machine_id
            .ok_or(BrowserFailure::Off(BrowsersOff::EnvironmentOffline))?;
        let command = Outgoing::new(key, machine, SerialKey::Environment(site.id), body)
            .map_err(EngineError::from)?;
        let command = crate::tools::source::issue(&self.engine, command).await?;
        match tokio::time::timeout(COMMAND_WAIT, self.engine.inner.commands.wait(command.id)).await
        {
            Ok(Ok(Outcome::Succeeded(value))) => serde_json::from_value(value)
                .map_err(|e| BrowserFailure::Engine(EngineError::unreadable(e))),
            Ok(Ok(Outcome::Failed(error))) => Err(BrowserFailure::Command(error)),
            Ok(Err(error)) => Err(BrowserFailure::Unreachable(error.to_string())),
            _ => {
                if !crate::tools::source::is_tool_call() {
                    let _ = self.engine.inner.commands.cancel(command.id).await;
                }
                Err(BrowserFailure::Unreachable(
                    "the environment did not answer in time".to_owned(),
                ))
            }
        }
    }

    /// Refreshes support, profiles and the browsers a connector still has.
    pub async fn look(&self, environment: Id) -> Result<ScreenLookResult, BrowserFailure> {
        let mut conn = self.engine.inner.db.acquire().await?;
        let site = rows::site(&mut conn, environment)
            .await?
            .ok_or(BrowserFailure::NotFound)?;
        if site.machine_online != Some(true) || site.status != "running" {
            return Err(BrowserFailure::Off(BrowsersOff::EnvironmentOffline));
        }
        drop(conn);
        let result = self
            .ask(
                &site,
                new_id(),
                &ScreenLook {
                    environment_id: environment,
                },
            )
            .await?;
        self.memory()
            .profiles
            .write()
            .unwrap_or_else(PoisonError::into_inner)
            .insert(environment, result.profiles.clone());
        let mut tx = self.engine.inner.db.begin().await?;
        rows::facts(
            &mut tx,
            environment,
            &serde_json::to_value(&result.browser).map_err(EngineError::from)?,
            &serde_json::to_value(&result.vnc).map_err(EngineError::from)?,
            self.engine.inner.clock.now(),
        )
        .await?;
        if let Some(current) = rows::site(&mut tx, environment).await?
            && current.machine_id == site.machine_id
            && current.generation == site.generation
        {
            self.engine
                .displays()
                .reconcile(&mut tx, &current, &result.displays)
                .await?;
        }
        tx.commit().await.map_err(orch_db::DbError::from)?;
        self.engine.displays().changed(environment).await;
        Ok(result)
    }

    async fn wait_open(&self, id: Id) -> Result<BrowserRow, BrowserFailure> {
        let until = tokio::time::Instant::now() + COMMAND_WAIT;
        loop {
            let mut conn = self.engine.inner.db.acquire().await?;
            let row = rows::get(&mut conn, id)
                .await?
                .ok_or(BrowserFailure::NotFound)?;
            drop(conn);
            match row.state.as_str() {
                "open" => return Ok(row),
                "closed" => {
                    return Err(BrowserFailure::Unreachable(
                        "the browser ended while it opened".to_owned(),
                    ));
                }
                _ => {}
            }
            if tokio::time::Instant::now() >= until {
                return Err(BrowserFailure::Unreachable(
                    "the browser is still opening".to_owned(),
                ));
            }
            tokio::time::sleep(JOIN_POLL).await;
        }
    }

    /// Reserves one profile once even when two API processes open it at the same moment.
    pub async fn open(&self, request: OpenBrowser) -> Result<OpenedBrowser, BrowserFailure> {
        if request
            .note
            .as_ref()
            .is_some_and(|note| note.chars().count() > crate::tools::browser_definition::NOTE_MAX)
        {
            return Err(BrowserFailure::Invalid(
                "note must be at most 200 characters".to_owned(),
            ));
        }
        if !valid_profile(&request.profile) {
            return Err(BrowserFailure::Invalid(
                "profile must match [a-z0-9][a-z0-9._-]{0,62}".to_owned(),
            ));
        }
        let profile_created = if request.create {
            let mut conn = self.engine.inner.db.acquire().await?;
            let site = rows::site(&mut conn, request.environment)
                .await?
                .ok_or(BrowserFailure::NotFound)?;
            self.check(
                &mut conn,
                &site,
                &request.requester,
                Permission::BrowserManage,
            )
            .await?;
            drop(conn);
            let looked = self.look(request.environment).await?;
            !looked.profiles.iter().any(|p| p.name == request.profile)
        } else {
            false
        };
        let profile_created = crate::tools::source::value(
            &self.engine,
            "browser.profile_created",
            serde_json::json!(profile_created),
        )
        .await?
        .as_bool()
        .ok_or_else(|| EngineError::unreadable("The retained browser default is not a flag."))?;
        let id = crate::tools::source::resource_id("browser.open", new_id());
        let (site, preset, args, size) = {
            let mut tx = self.engine.inner.db.begin().await?;
            rows::lock(&mut tx, request.environment).await?;
            let site = rows::site(&mut tx, request.environment)
                .await?
                .ok_or(BrowserFailure::NotFound)?;
            self.check(
                &mut tx,
                &site,
                &request.requester,
                Permission::BrowserManage,
            )
            .await?;
            if let Some(thread) = request.thread
                && !threads::thread(&mut tx, thread)
                    .await?
                    .is_some_and(|t| t.environment_id == Some(site.id) && t.deleted_at.is_none())
            {
                return Err(BrowserFailure::NotFound);
            }
            let existing = rows::get(&mut tx, id).await?;
            if let Some(row) = &existing {
                if row.environment_id != site.id
                    || row.env_generation != site.generation
                    || row.thread_id != request.thread
                    || row.opened_by != request.requester.person()
                    || row.profile.as_deref() != Some(request.profile.as_str())
                    || row.origin != "started"
                    || row.state == "closed"
                {
                    return Err(BrowserFailure::Engine(EngineError::ToolCallIdentity));
                }
                if row.state == "open" {
                    return Ok(OpenedBrowser {
                        row: row.clone(),
                        started: true,
                    });
                }
            }
            if existing.is_none()
                && let Some(row) = rows::on_profile(&mut tx, site.id, &request.profile).await?
            {
                rows::touch(&mut tx, row.id, self.engine.inner.clock.now()).await?;
                tx.commit().await.map_err(orch_db::DbError::from)?;
                return Ok(OpenedBrowser {
                    row: self.wait_open(row.id).await?,
                    started: false,
                });
            }
            if site.facts["browser"]["programs"]
                .as_array()
                .is_none_or(|p| p.is_empty())
            {
                return Err(BrowserFailure::Unsupported);
            }
            let max = setting(
                &mut tx,
                site.id,
                keys::BROWSER_MAX_PER_ENVIRONMENT,
                DEFAULT_MAX,
            )
            .await?;
            let open = rows::owned_count(&mut tx, site.id).await?;
            if existing.is_none() && open >= max {
                return Err(BrowserFailure::TooMany { open, max });
            }
            let requested = match existing.as_ref().and_then(|row| row.preset.as_deref()) {
                Some("window") => Some(BrowserPreset::Window),
                Some("headless") => Some(BrowserPreset::Headless),
                _ => request.preset,
            };
            let preset = match requested {
                Some(p) => p,
                None => {
                    setting(
                        &mut tx,
                        site.id,
                        keys::BROWSER_PRESET,
                        BrowserPreset::Headless,
                    )
                    .await?
                }
            };
            if preset == BrowserPreset::Window && site.facts["browser"]["window"] != true {
                return Err(BrowserFailure::Unsupported);
            }
            let args = setting(&mut tx, site.id, keys::BROWSER_ARGS, Vec::<String>::new()).await?;
            let size = setting(
                &mut tx,
                site.id,
                keys::BROWSER_VIEWPORT,
                DEFAULT_VIEWPORT.to_owned(),
            )
            .await?;
            if existing.is_none() {
                rows::insert(
                    &mut tx,
                    &NewBrowser {
                        id,
                        environment: site.id,
                        generation: site.generation,
                        profile: Some(&request.profile),
                        origin: "started",
                        person: request.requester.person(),
                        thread: request.thread,
                        preset: Some(preset_text(preset)),
                        display: None,
                        note: request.note.as_deref(),
                    },
                    self.engine.inner.clock.now(),
                )
                .await?;
            }
            tx.commit().await.map_err(orch_db::DbError::from)?;
            (site, preset, args, size)
        };
        self.changed(site.id).await;
        let virtual_window = {
            let mut conn = self.engine.inner.db.acquire().await?;
            use_virtual_window(
                &site.facts,
                orch_db::queries::environment_facts::permits(&mut conn, site.id, "environment.vnc")
                    .await?,
            )
        };
        let display = if preset == BrowserPreset::Window && virtual_window {
            match self.window(&site, id, &request, &size).await {
                Ok(display) => Some(display),
                Err(error) => {
                    let mut conn = self.engine.inner.db.acquire().await?;
                    rows::drop_opening(&mut conn, id).await?;
                    drop(conn);
                    self.changed(site.id).await;
                    return Err(error);
                }
            }
        } else {
            None
        };
        let body = BrowserOpen {
            environment_id: site.id,
            browser_id: id,
            profile: request.profile.clone(),
            create: request.create,
            preset: match preset {
                BrowserPreset::Headless => orch_link_contract::browser::BrowserPreset::Headless,
                BrowserPreset::Window => orch_link_contract::browser::BrowserPreset::Window,
            },
            args,
            viewport: size,
            url: None,
            display,
        };
        let result = match self
            .ask(
                &site,
                crate::ids::derived("browser.open", &[id.as_bytes()]),
                &body,
            )
            .await
        {
            Ok(result) => result,
            Err(error) => {
                let mut conn = self.engine.inner.db.acquire().await?;
                rows::drop_opening(&mut conn, id).await?;
                drop(conn);
                self.changed(site.id).await;
                return Err(error);
            }
        };
        if let Some(running) = result.running {
            let mut conn = self.engine.inner.db.acquire().await?;
            rows::drop_opening(&mut conn, id).await?;
            drop(conn);
            self.changed(site.id).await;
            return Ok(OpenedBrowser {
                row: self.wait_open(running).await?,
                started: false,
            });
        }
        let mut tx = self.engine.inner.db.begin().await?;
        rows::lock(&mut tx, site.id).await?;
        self.check(
            &mut tx,
            &site,
            &request.requester,
            Permission::BrowserManage,
        )
        .await?;
        if let Some(row) = rows::get(&mut tx, id).await?
            && row.state == "open"
        {
            tx.commit().await.map_err(orch_db::DbError::from)?;
            return Ok(OpenedBrowser { row, started: true });
        }
        let row = rows::opened(
            &mut tx,
            id,
            &result.endpoint,
            &result.program,
            None,
            self.engine.inner.clock.now(),
        )
        .await?
        .ok_or(BrowserFailure::NotFound)?;
        if profile_created {
            history(
                &mut tx,
                &site,
                &request.requester,
                "browser.profile_created",
                None,
                request.thread,
                "A browser profile was made.",
                json!({"name":request.profile}),
                &request.origin,
                self.engine.inner.clock.now(),
            )
            .await?;
        }
        history(
            &mut tx,
            &site,
            &request.requester,
            audit::name!("browser.opened"),
            Some(id),
            request.thread,
            "A browser was opened.",
            json!({"profile":row.profile,"origin":row.origin}),
            &request.origin,
            self.engine.inner.clock.now(),
        )
        .await?;
        tx.commit().await.map_err(orch_db::DbError::from)?;
        self.memory().opened_profile(
            site.id,
            &request.profile,
            id,
            self.engine.inner.clock.time(),
        );
        if let Some(url) = request.url
            && let Some(actions) = self.actions()
        {
            if url::Url::parse(&url).is_ok_and(|url| url.scheme() == "file") {
                let mut conn = self.engine.inner.db.acquire().await?;
                self.check(&mut conn, &site, &request.requester, Permission::FilesView)
                    .await?;
            }
            actions
                .navigate(id, &url)
                .await
                .map_err(BrowserFailure::Unreachable)?;
        }
        self.changed(site.id).await;
        Ok(OpenedBrowser {
            row,
            started: result.started,
        })
    }

    async fn window(
        &self,
        site: &Site,
        browser: Id,
        request: &OpenBrowser,
        geometry: &str,
    ) -> Result<orch_link_contract::browser::BrowserOpenDisplay, BrowserFailure> {
        let mut tx = self.engine.inner.db.begin().await?;
        rows::lock(&mut tx, site.id).await?;
        self.check(&mut tx, site, &request.requester, Permission::BrowserManage)
            .await?;
        if let Some(off) = crate::displays::Displays::off(&mut tx, site).await? {
            return Err(BrowserFailure::DisplayOff(off));
        }
        let reserved = rows::get(&mut tx, browser)
            .await?
            .and_then(|row| row.display_id);
        let (display, created, geometry) = if let Some(id) = reserved {
            let row = orch_db::queries::displays::get(&mut tx, id)
                .await?
                .ok_or(BrowserFailure::NotFound)?;
            if row.environment_id != site.id
                || row.env_generation != site.generation
                || row.state == "closed"
            {
                return Err(BrowserFailure::Engine(EngineError::ToolCallIdentity));
            }
            (
                id,
                row.state == "opening",
                row.geometry.unwrap_or_else(|| geometry.to_owned()),
            )
        } else {
            let max = setting(
                &mut tx,
                site.id,
                "vnc.max_per_environment",
                crate::displays::DEFAULT_MAX,
            )
            .await?;
            let open = orch_db::queries::displays::owned_count(&mut tx, site.id).await?;
            if open >= max
                && !orch_db::queries::displays::has_idle_window(&mut tx, site.id, site.generation)
                    .await?
            {
                return Err(BrowserFailure::DisplayTooMany { open, max });
            }
            let (display, created) = rows::reserve_window(
                &mut tx,
                site.id,
                site.generation,
                &request.profile,
                request.requester.person(),
                request.thread,
                self.engine.inner.clock.now(),
            )
            .await?;
            (display, created, geometry.to_owned())
        };
        rows::set_window(&mut tx, browser, display).await?;
        sqlx::query("UPDATE display_sessions SET geometry=COALESCE(geometry,$2) WHERE id=$1")
            .bind(display)
            .bind(&geometry)
            .execute(&mut *tx)
            .await
            .map_err(orch_db::DbError::from)?;
        tx.commit().await.map_err(orch_db::DbError::from)?;
        let ready = self
            .ask(
                site,
                new_id(),
                &orch_link_contract::browser_window::DisplayOpen {
                    environment_id: site.id,
                    display_id: display,
                    geometry: geometry.clone(),
                },
            )
            .await?;
        let mut tx = self.engine.inner.db.begin().await?;
        rows::lock(&mut tx, site.id).await?;
        self.check(&mut tx, site, &request.requester, Permission::BrowserManage)
            .await?;
        let already = orch_db::queries::displays::get(&mut tx, display)
            .await?
            .ok_or(BrowserFailure::NotFound)?;
        if already.state == "open" {
            tx.commit().await.map_err(orch_db::DbError::from)?;
            return Ok(orch_link_contract::browser::BrowserOpenDisplay {
                display_id: display,
                x_display: ready.x_display,
            });
        }
        rows::window_ready(
            &mut tx,
            display,
            &ready.endpoint,
            &ready.x_display,
            &geometry,
        )
        .await?;
        if created {
            let mut entry = Entry::new(
                request.requester.actor(),
                "display.opened",
                "display",
                Some(display),
                "A display was opened for a browser.",
            );
            entry.history = true;
            entry.environment_id = Some(site.id);
            entry.org_id = Some(site.org_id);
            entry.node_id = request.thread;
            entry.details = json!({"profile":request.profile});
            audit::append(
                &mut tx,
                &entry,
                &request.origin,
                self.engine.inner.clock.now(),
            )
            .await?;
        }
        tx.commit().await.map_err(orch_db::DbError::from)?;
        self.changed(site.id).await;
        self.engine.displays().changed(site.id).await;
        Ok(orch_link_contract::browser::BrowserOpenDisplay {
            display_id: display,
            x_display: ready.x_display,
        })
    }

    /// Verifies an announced endpoint through the environment before making it a visible row.
    pub async fn adopt(
        &self,
        environment: Id,
        thread: Id,
        endpoint: String,
        note: Option<String>,
    ) -> Result<BrowserRow, BrowserFailure> {
        if note
            .as_ref()
            .is_some_and(|note| note.chars().count() > crate::tools::browser_definition::NOTE_MAX)
        {
            return Err(BrowserFailure::Invalid(
                "note must be at most 200 characters".to_owned(),
            ));
        }
        let id = crate::tools::source::resource_id("browser.announce", new_id());
        let requester = Requester::Agent(thread);
        let site = {
            let mut tx = self.engine.inner.db.begin().await?;
            rows::lock(&mut tx, environment).await?;
            let site = rows::site(&mut tx, environment)
                .await?
                .ok_or(BrowserFailure::NotFound)?;
            self.check(&mut tx, &site, &requester, Permission::BrowserManage)
                .await?;
            if let Some(row) = rows::get(&mut tx, id).await? {
                if row.environment_id != environment
                    || row.env_generation != site.generation
                    || row.thread_id != Some(thread)
                    || row.opened_by.is_some()
                    || row.origin != "announced"
                    || row.state == "closed"
                {
                    return Err(BrowserFailure::Engine(EngineError::ToolCallIdentity));
                }
                if row.state == "open" {
                    return Ok(row);
                }
            } else {
                rows::insert(
                    &mut tx,
                    &NewBrowser {
                        id,
                        environment,
                        generation: site.generation,
                        profile: None,
                        origin: "announced",
                        person: None,
                        thread: Some(thread),
                        preset: None,
                        display: None,
                        note: note.as_deref(),
                    },
                    self.engine.inner.clock.now(),
                )
                .await?;
            }
            tx.commit().await.map_err(orch_db::DbError::from)?;
            site
        };
        self.changed(environment).await;
        let result = match self
            .ask(
                &site,
                new_id(),
                &BrowserAdopt {
                    environment_id: environment,
                    browser_id: id,
                    endpoint: endpoint.clone(),
                },
            )
            .await
        {
            Ok(r) => r,
            Err(e) => {
                let mut conn = self.engine.inner.db.acquire().await?;
                rows::drop_opening(&mut conn, id).await?;
                drop(conn);
                self.changed(environment).await;
                return Err(e);
            }
        };
        let mut tx = self.engine.inner.db.begin().await?;
        rows::lock(&mut tx, site.id).await?;
        self.check(&mut tx, &site, &requester, Permission::BrowserManage)
            .await?;
        if let Some(row) = rows::get(&mut tx, id).await?
            && row.state == "open"
        {
            tx.commit().await.map_err(orch_db::DbError::from)?;
            return Ok(row);
        }
        if let Ok(orch_link_contract::tunnels::Service::Browser { browser_id }) =
            result.service.parse()
            && browser_id != id
        {
            rows::drop_opening(&mut tx, id).await?;
            tx.commit().await.map_err(orch_db::DbError::from)?;
            let row = self.wait_open(browser_id).await?;
            if row.environment_id != environment {
                return Err(BrowserFailure::NotFound);
            }
            self.used(row.id).await?;
            self.changed(environment).await;
            return Ok(row);
        }
        let row = rows::opened(
            &mut tx,
            id,
            &endpoint,
            &result.program,
            result.profile.as_deref(),
            self.engine.inner.clock.now(),
        )
        .await?
        .ok_or(BrowserFailure::NotFound)?;
        history(
            &mut tx,
            &site,
            &requester,
            "browser.opened",
            Some(id),
            Some(thread),
            "An agent announced a browser.",
            json!({"origin":"announced","profile":row.profile}),
            &Origin::default(),
            self.engine.inner.clock.now(),
        )
        .await?;
        tx.commit().await.map_err(orch_db::DbError::from)?;
        self.changed(environment).await;
        Ok(row)
    }

    /// Closes a browser row once and asks its host to quit it.
    pub async fn close(
        &self,
        id: Id,
        reason: &str,
        requester: Requester,
        origin: &Origin,
    ) -> Result<Option<BrowserRow>, BrowserFailure> {
        let mut tx = self.engine.inner.db.begin().await?;
        let Some(before) = rows::get(&mut tx, id).await? else {
            return Ok(None);
        };
        let site = rows::site(&mut tx, before.environment_id)
            .await?
            .ok_or(BrowserFailure::NotFound)?;
        if !matches!(requester, Requester::System) {
            self.check(&mut tx, &site, &requester, Permission::BrowserManage)
                .await?;
        }
        let row = rows::close(&mut tx, id, reason, self.engine.inner.clock.now()).await?;
        if let Some(row) = &row {
            history(
                &mut tx,
                &site,
                &requester,
                audit::name!("browser.closed"),
                Some(id),
                row.thread_id,
                "A browser was closed.",
                json!({"profile":row.profile,"origin":row.origin,"reason":reason}),
                origin,
                self.engine.inner.clock.now(),
            )
            .await?;
        }
        tx.commit().await.map_err(orch_db::DbError::from)?;
        self.memory().closed_profile(id);
        if row.is_some() && site.machine_online == Some(true) {
            let _ = self
                .ask(&site, new_id(), &BrowserClose { browser_id: id })
                .await;
        }
        self.changed(site.id).await;
        Ok(row)
    }

    /// Changes a profile on the host and writes one line after it succeeds.
    pub async fn profile(
        &self,
        environment: Id,
        requester: Requester,
        action: BrowserProfileAction,
        name: String,
        to: Option<String>,
        origin: &Origin,
    ) -> Result<BrowserProfileResult, BrowserFailure> {
        if !valid_profile(&name) || to.as_ref().is_some_and(|n| !valid_profile(n)) {
            return Err(BrowserFailure::Invalid(
                "profile must be a name, never a path".to_owned(),
            ));
        }
        let site = {
            let mut conn = self.engine.inner.db.acquire().await?;
            let site = rows::site(&mut conn, environment)
                .await?
                .ok_or(BrowserFailure::NotFound)?;
            self.check(&mut conn, &site, &requester, Permission::BrowserManage)
                .await?;
            site
        };
        let result = self
            .ask(
                &site,
                new_id(),
                &BrowserProfileCommand {
                    environment_id: environment,
                    action,
                    name: name.clone(),
                    to: to.clone(),
                },
            )
            .await?;
        let mut tx = self.engine.inner.db.begin().await?;
        self.check(&mut tx, &site, &requester, Permission::BrowserManage)
            .await?;
        let action = match action {
            BrowserProfileAction::Create => audit::name!("browser.profile_created"),
            BrowserProfileAction::Rename => audit::name!("browser.profile_renamed"),
            BrowserProfileAction::Delete => audit::name!("browser.profile_deleted"),
        };
        history(
            &mut tx,
            &site,
            &requester,
            action,
            None,
            None,
            "A browser profile changed.",
            json!({"name":name,"to":to}),
            origin,
            self.engine.inner.clock.now(),
        )
        .await?;
        tx.commit().await.map_err(orch_db::DbError::from)?;
        self.memory()
            .profiles
            .write()
            .unwrap_or_else(PoisonError::into_inner)
            .insert(environment, result.profiles.clone());
        self.changed(environment).await;
        Ok(result)
    }

    /// Applies only a report from the connector and generation that own the browser row.
    pub async fn apply(
        &self,
        conn: &mut orch_db::Conn,
        machine: Id,
        environment: Id,
        state: &BrowserState,
    ) -> Result<(), EngineError> {
        let Some(row) = rows::get(conn, state.browser_id).await? else {
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
        self.memory()
            .counts
            .write()
            .unwrap_or_else(PoisonError::into_inner)
            .insert(row.id, state.tabs);
        if state.state == BrowserReportedState::Closed {
            self.memory().closed_profile(row.id);
            let reason = state
                .reason
                .as_deref()
                .filter(|r| {
                    matches!(
                        *r,
                        "quit"
                            | "closed_by_agent"
                            | "idle"
                            | "exited"
                            | "environment_stopped"
                            | "link_lost"
                            | "switched_off"
                            | "error"
                    )
                })
                .unwrap_or("exited");
            if let Some(closed) =
                rows::close(conn, row.id, reason, self.engine.inner.clock.now()).await?
            {
                history(
                    conn,
                    &site,
                    &Requester::System,
                    "browser.closed",
                    Some(row.id),
                    closed.thread_id,
                    "A browser ended.",
                    json!({"reason":reason,"profile":closed.profile}),
                    &Origin::default(),
                    self.engine.inner.clock.now(),
                )
                .await?;
            }
        } else {
            rows::touch(conn, row.id, self.engine.inner.clock.now()).await?;
            orch_db::queries::terminals::touch_environment(
                conn,
                environment,
                self.engine.inner.clock.now(),
            )
            .await?;
        }
        Ok(())
    }

    /// Marks use while somebody watches, drives, or names a browser through the tool.
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

    /// Closes browsers whose environment ended, whose gates closed, or whose configured idle time elapsed.
    pub async fn sweep(&self) -> Result<(), EngineError> {
        let mut conn = self.engine.inner.db.acquire().await?;
        let live = rows::live(&mut conn).await?;
        drop(conn);
        for row in live {
            let mut conn = self.engine.inner.db.acquire().await?;
            let site = rows::site(&mut conn, row.environment_id).await?;
            let reason = match site.as_ref() {
                None => Some("environment_stopped"),
                Some(site)
                    if site.generation != row.env_generation
                        || !matches!(site.status.as_str(), "running" | "offline") =>
                {
                    Some("environment_stopped")
                }
                Some(site) => match Self::off(&mut conn, site).await? {
                    Some(off) if off != BrowsersOff::EnvironmentOffline => {
                        if row.origin == "started" {
                            Some("switched_off")
                        } else {
                            None
                        }
                    }
                    _ => {
                        let idle =
                            setting(&mut conn, site.id, keys::BROWSER_IDLE_MINUTES, 0_i64).await?;
                        if row.state == "opening"
                            && self.engine.inner.clock.now() - row.opened_at
                                > Span::from_std(COMMAND_WAIT).unwrap_or_default()
                        {
                            Some("link_lost")
                        } else if row.origin == "started"
                            && idle > 0
                            && self.memory().watching(row.id) == 0
                            && self.engine.inner.clock.now() - row.last_used_at
                                >= Span::try_minutes(idle).unwrap_or(Span::MAX)
                        {
                            Some("idle")
                        } else {
                            None
                        }
                    }
                },
            };
            drop(conn);
            if let Some(reason) = reason {
                let _ = self
                    .close(row.id, reason, Requester::System, &Origin::default())
                    .await;
            }
        }
        self.engine
            .displays()
            .keep()
            .await
            .map_err(EngineError::unreadable)?;
        Ok(())
    }

    /// Reconciles saved rows with the processes the connector says remain.
    pub async fn reconcile(&self, environment: Id) -> Result<(), BrowserFailure> {
        let looked = self.look(environment).await?;
        let known: BTreeSet<_> = looked.browsers.iter().map(|b| b.browser_id).collect();
        let mut conn = self.engine.inner.db.acquire().await?;
        let live = rows::list(
            &mut conn,
            environment,
            self.engine.inner.clock.now() - CLOSED_LIST_KEPT,
        )
        .await?;
        drop(conn);
        for row in live
            .into_iter()
            .filter(|r| r.state == "open" && !known.contains(&r.id))
        {
            let _ = self
                .close(row.id, "link_lost", Requester::System, &Origin::default())
                .await?;
        }
        self.changed(environment).await;
        Ok(())
    }

    /// Writes one sitting from its first input through five minutes after its last, with hosts only.
    pub async fn sitting(
        &self,
        browser: Id,
        user: Id,
        from: DateTime<Utc>,
        last: DateTime<Utc>,
        hosts: BTreeSet<String>,
    ) -> Result<(), EngineError> {
        let mut tx = self.engine.inner.db.begin().await?;
        if let Some(row) = rows::get(&mut tx, browser).await?
            && let Some(site) = rows::site(&mut tx, row.environment_id).await?
        {
            history(
                &mut tx,
                &site,
                &Requester::Person(Caller::session(user)),
                audit::name!("browser.driven"),
                Some(browser),
                row.thread_id,
                "A person drove a browser.",
                json!({"from":Time::new(from),"until":Time::new(last+SITTING_GAP),"hosts":hosts}),
                &Origin::default(),
                self.engine.inner.clock.now(),
            )
            .await?;
        }
        tx.commit().await.map_err(orch_db::DbError::from)?;
        Ok(())
    }
}

/// The one syntax accepted by profile APIs and tools.
#[must_use]
pub fn valid_profile(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 63
        && name
            .bytes()
            .next()
            .is_some_and(|c| c.is_ascii_lowercase() || c.is_ascii_digit())
        && name.bytes().all(|c| {
            c.is_ascii_lowercase() || c.is_ascii_digit() || matches!(c, b'.' | b'_' | b'-')
        })
}

/// Its stable wire label.
#[must_use]
pub const fn preset_text(preset: BrowserPreset) -> &'static str {
    match preset {
        BrowserPreset::Headless => "headless",
        BrowserPreset::Window => "window",
    }
}

async fn setting<T: DeserializeOwned>(
    conn: &mut orch_db::Conn,
    environment: Id,
    key: &str,
    default: T,
) -> Result<T, EngineError> {
    Ok(
        orch_settings::get(conn, Place::Environment(environment), key)
            .await?
            .unwrap_or(default),
    )
}

#[expect(
    clippy::too_many_arguments,
    reason = "an audit row names its actor, object, context, action and time explicitly"
)]
async fn history(
    conn: &mut orch_db::Conn,
    site: &Site,
    requester: &Requester,
    action: &str,
    browser: Option<Id>,
    thread: Option<Id>,
    summary: &str,
    details: Json,
    origin: &Origin,
    at: DateTime<Utc>,
) -> Result<(), EngineError> {
    let mut entry = Entry::new(requester.actor(), action, "browser", browser, summary);
    entry.history = true;
    entry.org_id = Some(site.org_id);
    entry.environment_id = Some(site.id);
    entry.node_id = thread;
    entry.details = details;
    audit::append(conn, &entry, origin, at).await?;
    Ok(())
}

/// Applies browser lifecycle reports inside the transaction that acknowledges their stream.
pub async fn apply_reports(
    engine: &Engine,
    conn: &mut orch_db::Conn,
    machine: Id,
    environment: Id,
    events: &[DeliveredEvent],
) -> Result<(), EngineError> {
    for delivered in events {
        if let Ok(Event::BrowserState(state)) =
            Event::decode(&delivered.kind, delivered.body.clone())
        {
            engine
                .browsers()
                .apply(conn, machine, environment, &state)
                .await?;
        }
        if let Ok(Event::DisplayState(state)) =
            Event::decode(&delivered.kind, delivered.body.clone())
        {
            engine
                .displays()
                .apply(conn, machine, environment, &state)
                .await?;
        }
    }
    Ok(())
}

fn use_virtual_window(facts: &Json, allowed: bool) -> bool {
    allowed
        && facts
            .pointer("/browser/virtual_window")
            .and_then(Json::as_bool)
            == Some(true)
}

#[cfg(test)]
mod memory_tests {
    use super::*;

    #[test]
    fn a_window_uses_a_virtual_display_from_facts_and_policy_on_either_display_placement() {
        for placement in ["managed", "connected"] {
            let facts =
                serde_json::json!({"browser":{"virtual_window":true},"placement":placement});
            assert!(use_virtual_window(&facts, true));
            assert!(
                !use_virtual_window(&facts, false),
                "a machine's display refusal holds"
            );
            assert!(
                !use_virtual_window(&serde_json::json!({"browser":{}}), true),
                "unreported support is not invented"
            );
        }
    }

    #[test]
    fn a_removed_environment_leaves_neither_its_profiles_nor_its_browsers_counts() {
        let memory = Memory::default();
        let (environment, other, browser, elsewhere) = (new_id(), new_id(), new_id(), new_id());
        let at = Time::new(Utc::now());
        memory.opened_profile(environment, "work", browser, at);
        memory.opened_profile(other, "home", elsewhere, at);
        memory.counts.write().unwrap().insert(browser, Some(3));
        memory.counts.write().unwrap().insert(elsewhere, Some(1));
        memory.set_watching(browser, 2);
        memory.forget_environment(environment, &[browser]);
        assert!(memory.profiles(environment).is_none());
        assert_eq!(memory.tabs(browser), None);
        assert_eq!(memory.watching(browser), 0);
        assert_eq!(
            memory.profiles(other).map(|profiles| profiles.len()),
            Some(1),
            "another environment keeps its own"
        );
        assert_eq!(memory.tabs(elsewhere), Some(1));
    }
}
