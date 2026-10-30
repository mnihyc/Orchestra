//! A person's shells in an environment: theirs alone, kept while nobody is attached, one screen at a time, how many a person and an environment may have and which a person gives up for one more, the end of one that nobody came back to, and the last of what it printed kept in the environment's history.
//!
//! A shell is a pseudo-terminal that its environment's machine owns. It lives there until it exits or is
//! closed, whether a screen is attached or not; what lives here is the row that says whose it is and where
//! it stands, the rules about it, and the line of the environment's history that opening and closing it each
//! write. The browser's socket and the requests are the API's ([`Shells`] is what they call); the engine
//! decides what they may do and keeps the row true.
//!
//! **Who may do what.** A shell is its opener's alone. Opening one asks the access engine for `terminal.manage`
//! on the environment, whichever thread it is opened beside, which gives nothing (the API asks); then the
//! gates that no permission lifts, which this module checks in a fixed order
//! ([`TerminalNotAllowedReason`]); then the two limits. A person at the limit is not turned away: the request
//! names the shells of theirs they give up, and those are closed first, in the same step.
//!
//! **How a shell ends.** Every way a shell ends goes through [`Shells::finish`]: it asks the machine to close
//! the shell, which answers how the process ended, how much it printed and the last of it where that was
//! asked for; the row is closed by compare-and-set, so that of two that end a shell at once only one writes
//! its line of the history; the last output is masked and put away, and the shell's line says how it ended.
//! A shell ends when it exits, when it is closed, when its environment stops or is made again, when its
//! opener loses what it was opened by (checked on each move of the authorization epoch, attached or not),
//! and when nobody has been attached for `terminal.detached_minutes`. [`Shells::sweep`] and [`Shells::recheck`]
//! are how the last three are found; a shell that exited while nobody was attached is told by its machine on
//! the environment's stream ([`Shells::apply_exits`]).
//!
//! **The last of what a shell printed** is the machine's to keep and to upload; masking it, and keeping it
//! where it is read from, is the program's, which gives the engine its [`Recordings`] once at start.

use crate::commands::{Outcome, Outgoing};
use crate::engine::Engine;
use crate::error::EngineError;
use crate::ids;
use async_trait::async_trait;
use chrono::{DateTime, Duration as Span, TimeZone, Utc};
use orch_agent_contract::descriptor::AgentTerminal;
use orch_api_contract::terminals::{TERMINAL_RECORDING_MAX_BYTES, TerminalNotAllowedReason};
use orch_authz::environments::{Shell, stays_open};
use orch_authz::{Caller, Policy, Principal};
use orch_db::audit::{self, Actor, Entry, Origin};
use orch_db::queries::identity;
use orch_db::queries::resources::thread_engine as resources;
use orch_db::queries::terminals::{
    self as rows, After, Closing, NewSession, SessionRow, Site, reason,
};
use orch_db::queries::threads::{self, ThreadRow};
use orch_link::streams::DeliveredEvent;
use orch_link_contract::commands::{
    AgentPtyOpen, BlobRef, CommandError, PtyClose, PtyCloseResult, PtyOpen, PtyOpenResult,
    SerialKey,
};
use orch_link_contract::events::{Event, PtyExited};
use orch_settings::Place;
use orch_settings_contract::registry::keys;
use orch_types::{Hash, Id, Time, new_id};
use serde::de::DeserializeOwned;
use serde_json::json;
use std::collections::HashMap;
use std::sync::{Arc, OnceLock};
use std::time::Duration;

/// How long the row of a shell may stay `opening` before the program that asked for it is taken to have
/// gone away: the machine has thirty seconds to start one, and a minute more is room for a slow line.
pub const OPENING_GRACE: Span = Span::minutes(2);
/// The shortest time a shell is kept with nobody attached: the least the setting allows. A sweep looks
/// only at shells left at least this long, and reads the setting of the place of each of those.
pub const SHORTEST_KEPT: Span = Span::minutes(1);
/// How many shells one sweep takes up of each kind, so that a system with a great many is worked through
/// over several sweeps.
pub const SWEEP_BATCH: i64 = 200;
/// How long the closing of a shell waits for its machine's word on how it ended (the command's own
/// deadline is longer, and the command is not given up): a machine that does not answer in this time
/// does not hold up the person who closed the shell, and the shell is closed without what it would have
/// said.
pub const CLOSE_WAIT: Duration = Duration::from_secs(10);
/// How long a request to open a shell waits for the machine to start it: a little more than the
/// command's own deadline, so that the command is what ends it.
pub const OPEN_WAIT: Duration = Duration::from_secs(40);
/// The native observation has a ten-second deadline, with two seconds for its final link response.
const STATUS_WAIT: Duration = Duration::from_secs(12);
/// The shipped value of `terminal.detached_minutes`, for a place whose settings cannot be read.
const DETACHED_MINUTES: i64 = 480;
/// The shipped value of `terminal.max_per_person`.
const MAX_PER_PERSON: i64 = 4;
/// The shipped value of `terminal.max_per_environment`.
const MAX_PER_ENVIRONMENT: i64 = 16;

// ---------------------------------------------------------------- what the program gives the engine

/// Why the last output of a shell could not be kept or read.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("{0}")]
pub struct RecordingFault(pub String);

/// Where the last output of shells is put away: the program that starts the API gives it, because the
/// store of content is its own.
#[async_trait]
pub trait Recordings: Send + Sync {
    /// Takes the content `kept` that a machine uploaded, takes the values of the secrets of
    /// `environment` out of it where `mask` is set, and puts the result away. The blob returned is the
    /// one the shell's line points at: `kept` itself where nothing had to be replaced.
    async fn keep(
        &self,
        environment: Id,
        kept: BlobRef,
        mask: bool,
    ) -> Result<BlobRef, RecordingFault>;

    /// The bytes of a blob that [`Recordings::keep`] returned; none where the store has no such blob.
    async fn read(&self, hash: &Hash) -> Result<Option<Vec<u8>>, RecordingFault>;
}

static RECORDINGS: OnceLock<Arc<dyn Recordings>> = OnceLock::new();

/// Gives the engine the place where the last output of shells is kept. A program does this once, at
/// start, as it gives the master key to the places that seal with it; asked again it changes nothing.
/// Until it is done, nothing is kept of what a shell printed, and a shell's line says so.
pub fn install_recordings(recordings: Arc<dyn Recordings>) -> bool {
    RECORDINGS.set(recordings).is_ok()
}

/// What stands in the place of a secret's value in a stored text.
pub const MASKED: &str = "[secret]";
/// The shortest value that is taken for a secret when output is masked: a shorter one would be a letter
/// or two of every line, and replacing it would spoil the text and protect nothing.
pub const MASK_MIN_BYTES: usize = 4;

/// `content` with every value of `secrets` replaced by [`MASKED`], the longest value first, so that a
/// value that holds another is replaced whole. Values shorter than [`MASK_MIN_BYTES`] are left alone.
/// What a program prints between the characters of a value (as a terminal does with a typed one) is not
/// recognised.
#[must_use]
pub fn mask_secrets(content: &[u8], secrets: &[&str]) -> Vec<u8> {
    let mut values: Vec<&[u8]> = secrets
        .iter()
        .map(|value| value.as_bytes())
        .filter(|value| value.len() >= MASK_MIN_BYTES)
        .collect();
    values.sort_by_key(|value| std::cmp::Reverse(value.len()));
    values.dedup();
    let mut text = content.to_vec();
    for value in values {
        let mut masked = Vec::with_capacity(text.len());
        let mut at = 0;
        while at < text.len() {
            if text[at..].starts_with(value) {
                masked.extend_from_slice(MASKED.as_bytes());
                at += value.len();
            } else {
                masked.push(text[at]);
                at += 1;
            }
        }
        text = masked;
    }
    text
}

// ---------------------------------------------------------------- opening

/// What a request to open a shell asks for.
#[derive(Debug, Clone)]
pub struct OpenRequest {
    /// Who opens it; the caller has asked the access engine already.
    pub person: Id,
    /// The environment it runs in.
    pub environment: Id,
    /// The thread it is opened through.
    pub through: Option<Id>,
    /// The width of its screen.
    pub cols: u16,
    /// The height of its screen.
    pub rows: u16,
    /// Where it starts, relative to the workspace root.
    pub directory: Option<String>,
    /// The shells of the person's own in that environment to close first.
    pub close: Vec<Id>,
    /// Where the request came from, for the rows it writes.
    pub origin: Origin,
}

/// A shell that was opened.
#[derive(Debug, Clone)]
pub struct Opened {
    /// The shell.
    pub session: SessionRow,
    /// The shells that were closed to make room for it.
    pub closed: Vec<Id>,
}

/// Which of the two limits is reached.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Limit {
    /// What one person may have open in one environment.
    Person,
    /// What everybody together may have open in one.
    Environment,
}

/// A limit that stands in the way of one more shell, with what the person needs to choose from.
#[derive(Debug, Clone)]
pub struct AtLimit {
    /// Which limit it is.
    pub limit: Limit,
    /// The person's own open shells there, the one used longest ago first.
    pub mine: Vec<SessionRow>,
    /// How many shells are open there in all.
    pub open: i64,
    /// How many people have them.
    pub people: i64,
}

/// Why a shell was not opened.
#[derive(Debug, thiserror::Error)]
pub enum OpenError {
    /// There is no such environment.
    #[error("there is no such environment")]
    NoEnvironment,
    /// The thread it was to be opened through does not run in that environment.
    #[error("that thread does not run in this environment")]
    WrongThread,
    /// A setting, the environment's switch, its machine's policy or its state forbids it; no permission
    /// lifts these.
    #[error("a shell is not allowed here: {0:?}")]
    NotAllowed(TerminalNotAllowedReason),
    /// The directory is not a path inside the workspace.
    #[error("the directory is not a path inside the workspace: {0}")]
    BadDirectory(String),
    /// A shell named to be closed is not one of the person's own here.
    #[error("{0} is not a shell of yours in this environment")]
    NotYours(Id),
    /// A limit is reached.
    #[error("a limit of shells is reached")]
    AtLimit(Box<AtLimit>),
    /// The machine did not answer in time, or has no session.
    #[error("the machine could not be reached: {0}")]
    Unreachable(String),
    /// The machine would not start it.
    #[error("{0}")]
    Refused(CommandError),
    /// A part the engine stands on failed.
    #[error(transparent)]
    Engine(#[from] EngineError),
}

impl From<orch_db::DbError> for OpenError {
    fn from(error: orch_db::DbError) -> Self {
        Self::Engine(error.into())
    }
}

/// How a shell is to end.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Ending {
    /// One of the reasons of [`orch_db::queries::terminals::reason`].
    pub reason: &'static str,
    /// What the process exited with, where the one who ends the shell saw it.
    pub exit_code: Option<i64>,
}

impl Ending {
    /// Its person, or whoever manages its environment, closed it.
    pub const CLOSED: Self = Self::because(reason::CLOSED_BY_USER);
    /// Its opener lost what it was opened by.
    pub const REVOKED: Self = Self::because(reason::REVOKED);

    /// An ending for `reason` with no exit code seen.
    #[must_use]
    pub const fn because(reason: &'static str) -> Self {
        Self {
            reason,
            exit_code: None,
        }
    }

    /// The process exited with `code`.
    #[must_use]
    pub const fn exited(code: i64) -> Self {
        Self {
            reason: reason::EXITED,
            exit_code: Some(code),
        }
    }
}

/// What a shell is kept by, which a reader of its row needs: whether its last output will be kept and
/// when it is closed for being left alone.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Keeping {
    /// Whether the end of what it prints is kept when it closes.
    pub record: bool,
    /// For how many minutes it runs on with nobody attached.
    pub detached_minutes: i64,
}

/// Whether a shell's opener still holds what it was opened by.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Holding {
    /// They do.
    Yes,
    /// They no longer hold the permission it was opened by, or are not in the system any more.
    Permission,
    /// A gate that no permission lifts has closed.
    Gate(TerminalNotAllowedReason),
}

/// The shell program and the directory, as a list shows them.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Where {
    /// The machine the shell runs on.
    pub machine: Id,
    /// The environment's workspace root.
    pub workspace_root: String,
}

/// Called with a shell and how it is to end, just before it is ended: whoever keeps the screens of shells
/// tells the screen of this one why it is going, before the end can be seen from the machine's side and
/// told as something else.
pub type AboutTo<'a> = &'a (dyn Fn(&SessionRow, Ending) + Send + Sync);

/// The engine's part in shells.
#[derive(Clone)]
pub struct Shells {
    engine: Engine,
}

impl std::fmt::Debug for Shells {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Shells").finish_non_exhaustive()
    }
}

impl Engine {
    /// The engine's part in shells.
    #[must_use]
    pub fn shells(&self) -> Shells {
        Shells {
            engine: self.clone(),
        }
    }
}

/// The value of a setting at a place, or the shipped one where the key is unset.
async fn setting<T: DeserializeOwned>(
    conn: &mut orch_db::Conn,
    place: Place,
    key: &str,
    shipped: T,
) -> Result<T, EngineError> {
    Ok(orch_settings::get::<T>(conn, place, key)
        .await?
        .unwrap_or(shipped))
}

/// Where the settings of a shell are read: at its thread when it was opened through one, at its
/// environment otherwise.
#[must_use]
pub fn place_of(thread: Option<Id>, environment: Id) -> Place {
    thread.map_or(Place::Environment(environment), Place::Node)
}

/// A path as the connector is given it: relative to the workspace root, without `..`, `.` or empty
/// parts, and without a drive or a root of its own.
fn relative(path: &str) -> Result<String, OpenError> {
    let bad = |why: &str| OpenError::BadDirectory(format!("{path:?} {why}"));
    if path.contains('\0') || path.contains('\\') {
        return Err(bad("has a character that a path here does not"));
    }
    if path.starts_with('/') {
        return Err(bad("is not relative to the workspace root"));
    }
    let mut kept = Vec::new();
    for part in path.split('/') {
        match part {
            "" | "." => {}
            ".." => return Err(bad("leaves the workspace")),
            other => kept.push(other),
        }
    }
    Ok(kept.join("/"))
}

/// The directory `rel` of a workspace rooted at `root`, as a shell's row shows it.
fn absolute(root: &str, rel: &str) -> String {
    if rel.is_empty() {
        root.to_owned()
    } else {
        format!("{}/{rel}", root.trim_end_matches('/'))
    }
}

impl Shells {
    /// Measures only the live identities already selected by an authorized session reader. A machine
    /// that is away is not asked and nothing is observed of its shells, which are still listed: a list
    /// of records does not fail because one machine is offline. A machine that is asked and fails is
    /// reported as unavailable.
    pub async fn busy(
        &self,
        conn: &mut orch_db::Conn,
        sessions: &[SessionRow],
    ) -> Result<HashMap<Id, bool>, EngineError> {
        let mut groups: HashMap<Id, Vec<Id>> = HashMap::new();
        for session in sessions
            .iter()
            .filter(|session| session.state == rows::state::OPEN)
        {
            groups
                .entry(session.environment_id)
                .or_default()
                .push(session.id);
        }
        let mut observed = HashMap::new();
        for (environment, ids) in groups {
            let site = rows::site(conn, environment)
                .await?
                .ok_or_else(|| EngineError::unreadable("the shell's environment is unavailable"))?;
            // A machine that is away cannot be asked; its shells are still listed, with nothing
            // observed of what runs in them.
            let Some(machine) = site
                .machine_id
                .filter(|_| site.machine_online == Some(true))
            else {
                continue;
            };
            for selected in ids.chunks(200) {
                let command_id = new_id();
                let outgoing = Outgoing::new(
                    command_id,
                    machine,
                    SerialKey::None,
                    &orch_link_contract::commands::PtyStatus {
                        environment_id: environment,
                        terminal_ids: selected.to_vec(),
                    },
                )?;
                self.engine.inner.commands.issue(outgoing).await?;
                let outcome =
                    tokio::time::timeout(STATUS_WAIT, self.engine.inner.commands.wait(command_id))
                        .await
                        .map_err(|_| {
                            EngineError::unreadable("the shell's foreground observation timed out")
                        })??;
                let Outcome::Succeeded(value) = outcome else {
                    return Err(EngineError::unreadable(
                        "the machine could not observe this shell's foreground state",
                    ));
                };
                let result: orch_link_contract::commands::PtyStatusResult =
                    serde_json::from_value(value)?;
                for fact in result.terminals {
                    if selected.contains(&fact.terminal_id) {
                        observed.insert(fact.terminal_id, fact.busy);
                    }
                }
                if selected.iter().any(|id| !observed.contains_key(id)) {
                    return Err(EngineError::unreadable(
                        "a selected live shell is no longer available on its machine",
                    ));
                }
            }
        }
        Ok(observed)
    }

    fn db(&self) -> &orch_db::Db {
        &self.engine.inner.db
    }

    fn now(&self) -> DateTime<Utc> {
        self.engine.inner.clock.now()
    }

    /// The gates that no permission lifts, in the order the refusals are listed: a setting, the
    /// environment's own switch, the local policy of a person's machine, and whether the environment can
    /// be reached.
    async fn gates(
        &self,
        conn: &mut orch_db::Conn,
        site: &Site,
        place: Place,
    ) -> Result<Option<TerminalNotAllowedReason>, EngineError> {
        if !setting(conn, place, keys::TERMINAL_ENABLED, true).await? {
            return Ok(Some(TerminalNotAllowedReason::DisabledByPolicy));
        }
        if !site.terminal {
            return Ok(Some(TerminalNotAllowedReason::EnvironmentSwitchOff));
        }
        if orch_db::queries::environment_facts::refuses(conn, site.id, "environment.terminal")
            .await?
        {
            return Ok(Some(TerminalNotAllowedReason::MachinePolicy));
        }
        if site.status != "running"
            || site.machine_id.is_none()
            || site.machine_online != Some(true)
        {
            return Ok(Some(TerminalNotAllowedReason::EnvironmentOffline));
        }
        Ok(None)
    }

    /// The thread a shell is to be opened through, if it runs in `environment`.
    async fn thread_in(
        &self,
        conn: &mut orch_db::Conn,
        thread: Id,
        environment: Id,
    ) -> Result<ThreadRow, OpenError> {
        match threads::thread(conn, thread).await? {
            Some(row) if row.environment_id == Some(environment) && row.deleted_at.is_none() => {
                Ok(row)
            }
            _ => Err(OpenError::WrongThread),
        }
    }

    /// Where the shell starts, relative to the workspace root: the directory asked for, or else the
    /// thread's own, or the root.
    async fn directory(
        &self,
        conn: &mut orch_db::Conn,
        thread: Option<&ThreadRow>,
        asked: Option<&str>,
    ) -> Result<String, OpenError> {
        if let Some(asked) = asked {
            return relative(asked);
        }
        let Some(thread) = thread else {
            return Ok(String::new());
        };
        let inside: String = setting(
            conn,
            Place::Node(thread.node_id),
            keys::THREAD_CWD,
            String::new(),
        )
        .await?;
        relative(&format!("{}/{inside}", thread.workspace_dir))
    }

    /// Opens a shell: checks the gates and the limits, closes the shells the person gives up (`about` is
    /// told of each before it is ended), has the machine start the new one, and writes the line of the
    /// history.
    pub async fn open(
        &self,
        request: OpenRequest,
        about: AboutTo<'_>,
    ) -> Result<Opened, OpenError> {
        let id = new_id();
        let now = self.now();
        let reserved = {
            let mut tx = self.db().begin().await?;
            rows::lock(&mut tx, request.environment).await?;
            let site = rows::site(&mut tx, request.environment)
                .await?
                .ok_or(OpenError::NoEnvironment)?;
            let thread = match request.through {
                Some(thread) => Some(self.thread_in(&mut tx, thread, request.environment).await?),
                None => None,
            };
            let place = place_of(thread.as_ref().map(|row| row.node_id), request.environment);
            if let Some(reason) = self.gates(&mut tx, &site, place).await? {
                return Err(OpenError::NotAllowed(reason));
            }
            let directory = self
                .directory(&mut tx, thread.as_ref(), request.directory.as_deref())
                .await?;
            let live = rows::live_in(&mut tx, request.environment).await?;
            let mut give_up: Vec<SessionRow> = Vec::new();
            for named in &request.close {
                if give_up.iter().any(|row| row.id == *named) {
                    continue;
                }
                match live.iter().find(|row| row.id == *named) {
                    Some(row) if row.opened_by == request.person => give_up.push(row.clone()),
                    // One that has ended already is no longer a thing to give up.
                    None if rows::get(&mut tx, *named).await?.is_some_and(|row| {
                        row.opened_by == request.person && row.environment_id == request.environment
                    }) => {}
                    _ => return Err(OpenError::NotYours(*named)),
                }
            }
            let max_person = setting(
                &mut tx,
                place,
                keys::TERMINAL_MAX_PER_PERSON,
                MAX_PER_PERSON,
            )
            .await?;
            let max_environment = setting(
                &mut tx,
                place,
                keys::TERMINAL_MAX_PER_ENVIRONMENT,
                MAX_PER_ENVIRONMENT,
            )
            .await?;
            let mine_after = to_count(
                live.iter()
                    .filter(|row| {
                        row.opened_by == request.person
                            && !give_up.iter().any(|given| given.id == row.id)
                    })
                    .count(),
            );
            let all_after = to_count(live.len()) - to_count(give_up.len());
            let limit = if mine_after >= max_person {
                Some(Limit::Person)
            } else if all_after >= max_environment {
                Some(Limit::Environment)
            } else {
                None
            };
            if let Some(limit) = limit {
                let mut people: Vec<Id> = live.iter().map(|row| row.opened_by).collect();
                people.sort_unstable();
                people.dedup();
                return Err(OpenError::AtLimit(Box::new(AtLimit {
                    limit,
                    mine: live
                        .iter()
                        .filter(|row| row.opened_by == request.person)
                        .cloned()
                        .collect(),
                    open: to_count(live.len()),
                    people: to_count(people.len()),
                })));
            }
            let record = setting(&mut tx, place, keys::TERMINAL_RECORD, true).await?;
            let shown = absolute(&site.workspace_root, &directory);
            rows::insert(
                &mut tx,
                &NewSession {
                    id,
                    thread_id: thread.as_ref().map(|row| row.node_id),
                    environment_id: request.environment,
                    env_generation: site.generation,
                    opened_by: request.person,
                    cwd: &shown,
                    cols: i32::from(request.cols),
                    rows: i32::from(request.rows),
                },
                now,
            )
            .await?;
            tx.commit().await.map_err(orch_db::DbError::from)?;
            Reserved {
                site,
                directory,
                record,
                give_up,
            }
        };
        let mut closed = Vec::new();
        for given in &reserved.give_up {
            about(given, Ending::CLOSED);
            if self
                .finish(given.id, Ending::CLOSED, &request.origin)
                .await?
                .is_some()
            {
                closed.push(given.id);
            }
        }
        let started = self.start(id, &request, &reserved).await;
        let result = match started {
            Ok(started) => started,
            Err(error) => {
                let mut conn = self.db().acquire().await?;
                rows::drop_opening(&mut conn, id).await?;
                return Err(error);
            }
        };
        let mut tx = self.db().begin().await?;
        let Some(session) = rows::opened(&mut tx, id, &result.shell, self.now()).await? else {
            // Taken for abandoned and removed while the machine was still starting it.
            drop(tx);
            self.close_on_machine(reserved.site.machine_id, id).await;
            return Err(OpenError::Unreachable(
                "the shell was given up while it was being started".to_owned(),
            ));
        };
        self.write_opened(
            &mut tx,
            &session,
            &reserved.site,
            reserved.record,
            &request.origin,
        )
        .await?;
        tx.commit().await.map_err(orch_db::DbError::from)?;
        Ok(Opened { session, closed })
    }

    /// Sends `pty.open` and waits for the machine's answer.
    async fn start(
        &self,
        id: Id,
        request: &OpenRequest,
        reserved: &Reserved,
    ) -> Result<PtyOpenResult, OpenError> {
        let machine = reserved
            .site
            .machine_id
            .ok_or_else(|| OpenError::Unreachable("the environment has no machine".to_owned()))?;
        let body = PtyOpen {
            environment_id: request.environment,
            terminal_id: id,
            cols: request.cols,
            rows: request.rows,
            cwd: reserved.directory.clone(),
            record: reserved.record,
        };
        let command_id = ids::derived("pty.open", &[id.as_bytes().as_slice()]);
        let command = Outgoing::new(command_id, machine, SerialKey::None, &body)
            .map_err(EngineError::from)?;
        let commands = &self.engine.inner.commands;
        commands.issue(command).await.map_err(EngineError::from)?;
        let outcome = match tokio::time::timeout(OPEN_WAIT, commands.wait(command_id)).await {
            Ok(outcome) => outcome.map_err(EngineError::from)?,
            Err(_) => {
                let _ = commands.cancel(command_id).await;
                return Err(OpenError::Unreachable("it took too long".to_owned()));
            }
        };
        match outcome {
            Outcome::Succeeded(result) => serde_json::from_value(result)
                .map_err(|error| OpenError::Engine(EngineError::unreadable(error))),
            Outcome::Failed(error) => Err(OpenError::Refused(error)),
            Outcome::Expired => Err(OpenError::Unreachable(
                "the machine did not take the command in time".to_owned(),
            )),
            Outcome::Cancelled => Err(OpenError::Unreachable(
                "the command was cancelled".to_owned(),
            )),
        }
    }

    /// Tells the machine to end a shell that nobody here wants; nothing is waited for.
    async fn close_on_machine(&self, machine: Option<Id>, shell: Id) {
        let Some(machine) = machine else { return };
        let command_id = ids::derived("pty.close", &[shell.as_bytes().as_slice()]);
        if let Ok(command) = Outgoing::new(
            command_id,
            machine,
            SerialKey::None,
            &PtyClose { terminal_id: shell },
        ) {
            let _ = self.engine.inner.commands.issue(command).await;
        }
    }

    // ---------------------------------------------------------------- the history

    async fn name_of(&self, conn: &mut orch_db::Conn, person: Id) -> Result<String, EngineError> {
        Ok(identity::user_by_id(conn, person)
            .await?
            .map_or_else(|| "Somebody".to_owned(), |user| user.display_name))
    }

    /// The line of the environment's history that says a shell was opened.
    async fn write_opened(
        &self,
        conn: &mut orch_db::Conn,
        session: &SessionRow,
        site: &Site,
        record: bool,
        origin: &Origin,
    ) -> Result<(), EngineError> {
        let who = self.name_of(conn, session.opened_by).await?;
        let summary = format!("{who} opened a shell");
        let mut entry = Entry::new(
            Actor::User(session.opened_by),
            orch_db::audit::name!("terminal.opened"),
            "terminal",
            Some(session.id),
            &summary,
        );
        entry.history = true;
        entry.node_id = session.thread_id;
        entry.org_id = Some(site.org_id);
        entry.environment_id = Some(session.environment_id);
        entry.details = json!({
            "shell": session.shell,
            "cwd": session.cwd,
            "thread_id": session.thread_id,
            "record": record,
        });
        audit::append(conn, &entry, origin, session.opened_at).await?;
        Ok(())
    }

    /// The line of the environment's history that says how a shell ended: who, for how long, how it
    /// ended, how much it printed, and whether the last of that was kept.
    async fn write_closed(
        &self,
        conn: &mut orch_db::Conn,
        session: &SessionRow,
        org: Option<Id>,
        origin: &Origin,
    ) -> Result<(), EngineError> {
        let who = self.name_of(conn, session.opened_by).await?;
        let closed_at = session.closed_at.unwrap_or_else(|| self.now());
        let seconds = (closed_at - session.opened_at).num_seconds().max(0);
        let how = session.close_reason.as_deref().unwrap_or(reason::ERROR);
        let kept = session.recording_blob.is_some();
        let summary = format!(
            "{who}'s shell closed after {}: {}, {} printed, {}",
            long(seconds),
            ended_in_words(how, session.exit_code),
            bytes_in_words(session.bytes_out),
            if kept {
                "the last of it kept"
            } else {
                "nothing of it kept"
            },
        );
        let mut entry = Entry::new(
            Actor::User(session.opened_by),
            orch_db::audit::name!("terminal.closed"),
            "terminal",
            Some(session.id),
            &summary,
        );
        entry.history = true;
        entry.node_id = session.thread_id;
        entry.org_id = org;
        entry.environment_id = Some(session.environment_id);
        entry.details = json!({
            "seconds": seconds,
            "how": how,
            "exit_code": session.exit_code,
            "bytes_in": session.bytes_in,
            "bytes_out": session.bytes_out,
            "kept": kept,
        });
        audit::append(conn, &entry, origin, closed_at).await?;
        Ok(())
    }

    /// Says that somebody other than a shell's opener read what it last printed, once for each reader,
    /// shell and day. True where a line was written.
    pub async fn note_read(
        &self,
        session: &SessionRow,
        reader: Id,
        origin: &Origin,
    ) -> Result<bool, EngineError> {
        let now = self.now();
        let day = day_of(now);
        let mut tx = self.db().begin().await?;
        // The statement and the write are one step, so that two reads at once write one line.
        rows::lock(&mut tx, session.id).await?;
        if rows::read_between(&mut tx, reader, session.id, day, day + Span::days(1)).await? {
            return Ok(false);
        }
        let reader_name = self.name_of(&mut tx, reader).await?;
        let owner_name = self.name_of(&mut tx, session.opened_by).await?;
        let summary = format!("{reader_name} read what {owner_name}'s shell last printed");
        let mut entry = Entry::new(
            Actor::User(reader),
            orch_db::audit::name!("terminal.recording_read"),
            "terminal",
            Some(session.id),
            &summary,
        );
        entry.history = true;
        entry.node_id = session.thread_id;
        entry.environment_id = Some(session.environment_id);
        entry.details = json!({ "opened_by": session.opened_by });
        audit::append(&mut tx, &entry, origin, now).await?;
        tx.commit().await.map_err(orch_db::DbError::from)?;
        Ok(true)
    }

    // ---------------------------------------------------------------- ending

    /// Ends a shell. The machine is asked to close it and says how the process ended and what was kept of
    /// its output; the row is closed by compare-and-set and its line written. Returns the closed row,
    /// and none where the shell was closed already, or never was.
    pub async fn finish(
        &self,
        id: Id,
        ending: Ending,
        origin: &Origin,
    ) -> Result<Option<SessionRow>, EngineError> {
        let mut conn = self.db().acquire().await?;
        let Some(row) = rows::get(&mut conn, id).await? else {
            return Ok(None);
        };
        if !row.is_live() {
            return Ok(None);
        }
        let site = rows::site(&mut conn, row.environment_id).await?;
        drop(conn);
        let said = match site.as_ref().and_then(|site| site.machine_id) {
            Some(machine) => self.ask_to_close(machine, id).await,
            None => None,
        };
        let mut tx = self.db().begin().await?;
        let place = place_of(row.thread_id, row.environment_id);
        let kept = match said.as_ref().and_then(|said| said.kept) {
            Some(kept) => self.keep(&mut tx, row.environment_id, place, kept).await?,
            None => None,
        };
        let closing = Closing {
            reason: ending.reason,
            exit_code: said
                .as_ref()
                .and_then(|said| said.exit_code)
                .or(ending.exit_code),
            bytes_out: said.as_ref().map(|said| said.bytes_out),
            recording: kept,
        };
        let closed = rows::close(&mut tx, id, &closing, self.now()).await?;
        match &closed {
            Some(closed) => {
                self.write_closed(
                    &mut tx,
                    closed,
                    site.as_ref().map(|site| site.org_id),
                    origin,
                )
                .await?;
            }
            None => {
                if let Some(hash) = kept {
                    rows::unpin_recording(&mut tx, &hash).await?;
                }
            }
        }
        tx.commit().await.map_err(orch_db::DbError::from)?;
        Ok(closed)
    }

    /// Sends `pty.close` and waits a little for the answer.
    async fn ask_to_close(&self, machine: Id, shell: Id) -> Option<PtyCloseResult> {
        let command_id = ids::derived("pty.close", &[shell.as_bytes().as_slice()]);
        let command = Outgoing::new(
            command_id,
            machine,
            SerialKey::None,
            &PtyClose { terminal_id: shell },
        )
        .ok()?;
        let commands = &self.engine.inner.commands;
        commands.issue(command).await.ok()?;
        match tokio::time::timeout(CLOSE_WAIT, commands.wait(command_id)).await {
            Ok(Ok(Outcome::Succeeded(result))) => serde_json::from_value(result).ok(),
            _ => None,
        }
    }

    /// Puts away what a machine kept of a shell's output, masked, and holds it for the shell's line.
    /// None where nothing is to be kept: no place to keep it, or more than may be.
    async fn keep(
        &self,
        conn: &mut orch_db::Conn,
        environment: Id,
        place: Place,
        kept: BlobRef,
    ) -> Result<Option<Hash>, EngineError> {
        if kept.size > TERMINAL_RECORDING_MAX_BYTES {
            tracing::warn!(
                size = kept.size,
                "a shell kept more than may be put away; it is left out"
            );
            return Ok(None);
        }
        let Some(recordings) = RECORDINGS.get() else {
            tracing::warn!("there is no place to put the last output of shells; it is not kept");
            return Ok(None);
        };
        let mask = setting(conn, place, keys::REDACTION_MASK_SECRETS, true).await?;
        let put = match recordings.keep(environment, kept, mask).await {
            Ok(put) => put,
            Err(error) => {
                tracing::warn!(%error, "the last output of a shell could not be put away");
                return Ok(None);
            }
        };
        rows::record_blob(conn, &put.hash, put.size, self.now()).await?;
        Ok(Some(put.hash))
    }

    /// The last output of a shell, as it was put away; none where nothing was kept or the store lost it.
    pub async fn recording(&self, session: &SessionRow) -> Result<Option<Vec<u8>>, EngineError> {
        let (Some(hash), Some(recordings)) = (session.recording(), RECORDINGS.get()) else {
            return Ok(None);
        };
        recordings
            .read(&hash)
            .await
            .map_err(EngineError::unreadable)
    }

    /// Applies what a machine said, on its environment's stream, of shells that ended while nobody was
    /// attached. Applying the same events again changes nothing: a shell is closed once.
    pub async fn apply_exits(
        &self,
        conn: &mut orch_db::Conn,
        environment: Id,
        events: &[DeliveredEvent],
    ) -> Result<(), EngineError> {
        for event in events.iter().filter(|event| event.kind == "pty.exited") {
            let exited: PtyExited = match Event::decode(&event.kind, event.body.clone()) {
                Ok(Event::PtyExited(exited)) => exited,
                Ok(_) => continue,
                Err(error) => {
                    tracing::warn!(%error, "a report of a shell that ended does not read");
                    continue;
                }
            };
            let Some(row) = rows::get(conn, exited.terminal_id).await? else {
                continue;
            };
            if row.environment_id != environment || !row.is_live() {
                continue;
            }
            let place = place_of(row.thread_id, row.environment_id);
            let kept = match exited.kept {
                Some(kept) => self.keep(conn, environment, place, kept).await?,
                None => None,
            };
            // A connector that was restarted no longer has the shell, and says so without a code.
            let how = if exited.exit_code.is_some() {
                reason::EXITED
            } else {
                reason::LINK_LOST
            };
            let closing = Closing {
                reason: how,
                exit_code: exited.exit_code,
                bytes_out: Some(exited.bytes_out),
                recording: kept,
            };
            let at = event.at.get();
            match rows::close(conn, row.id, &closing, at).await? {
                Some(closed) => {
                    let org = rows::site(conn, environment).await?.map(|site| site.org_id);
                    self.write_closed(conn, &closed, org, &Origin::default())
                        .await?;
                }
                None => {
                    if let Some(hash) = kept {
                        rows::unpin_recording(conn, &hash).await?;
                    }
                }
            }
        }
        Ok(())
    }

    // ---------------------------------------------------------------- what is held

    /// Whether a shell's opener still holds what it was opened by: the permission, and the gates that no
    /// permission lifts. Whether the environment can be reached is not one of them: a machine that
    /// loses its line does not lose its shells.
    pub async fn holds(
        &self,
        conn: &mut orch_db::Conn,
        session: &SessionRow,
    ) -> Result<Holding, EngineError> {
        let Some(site) = rows::site(conn, session.environment_id).await? else {
            return Ok(Holding::Gate(TerminalNotAllowedReason::EnvironmentOffline));
        };
        let principal = match Principal::load(
            conn,
            Caller::session(session.opened_by),
            Time::new(self.now()),
        )
        .await
        {
            Ok(principal) => principal,
            Err(
                orch_authz::Error::Suspended { .. }
                | orch_authz::Error::NoSuchUser(_)
                | orch_authz::Error::NotFound(_)
                | orch_authz::Error::Archived { .. },
            ) => return Ok(Holding::Permission),
            Err(other) => return Err(other.into()),
        };
        let shell = Shell {
            environment: session.environment_id,
            thread: session.thread_id,
            opened_by: session.opened_by,
        };
        if !stays_open(conn, &principal, &shell, Policy::default()).await? {
            return Ok(Holding::Permission);
        }
        let place = place_of(session.thread_id, session.environment_id);
        Ok(match self.gates(conn, &site, place).await? {
            Some(TerminalNotAllowedReason::EnvironmentOffline) | None => Holding::Yes,
            Some(reason) => Holding::Gate(reason),
        })
    }

    /// What a shell is kept by, at the place it was opened at.
    pub async fn keeping(
        &self,
        conn: &mut orch_db::Conn,
        session: &SessionRow,
    ) -> Result<Keeping, EngineError> {
        let place = place_of(session.thread_id, session.environment_id);
        Ok(Keeping {
            record: setting(conn, place, keys::TERMINAL_RECORD, true).await?,
            detached_minutes: setting(
                conn,
                place,
                keys::TERMINAL_DETACHED_MINUTES,
                DETACHED_MINUTES,
            )
            .await?,
        })
    }

    /// The machine a shell runs on and the root of its workspace; none where its environment is gone.
    pub async fn whereabouts(
        &self,
        conn: &mut orch_db::Conn,
        session: &SessionRow,
    ) -> Result<Option<Where>, EngineError> {
        Ok(rows::site(conn, session.environment_id)
            .await?
            .and_then(|site| {
                site.machine_id.map(|machine| Where {
                    machine,
                    workspace_root: site.workspace_root,
                })
            }))
    }

    // ---------------------------------------------------------------- the sweeps

    /// Looks for shells that have to end for a reason of time or place, and ends them: those whose
    /// environment is gone, stopped or made again; those nobody has been attached to for as long as the
    /// setting of their place says; and those whose opening was abandoned. Returns the shells that were
    /// closed. `about` is told of each before it is ended.
    pub async fn sweep(&self, about: AboutTo<'_>) -> Result<Vec<SessionRow>, EngineError> {
        let now = self.now();
        let origin = Origin::default();
        let mut closed = Vec::new();
        let (stale, ended, left) = {
            let mut conn = self.db().acquire().await?;
            (
                rows::stale_opening(&mut conn, now - OPENING_GRACE, SWEEP_BATCH).await?,
                rows::of_ended_environments(&mut conn, SWEEP_BATCH).await?,
                rows::left_since(&mut conn, now - SHORTEST_KEPT, SWEEP_BATCH).await?,
            )
        };
        if !stale.is_empty() {
            let mut conn = self.db().acquire().await?;
            for row in stale {
                // The program that asked for it is gone and nothing happened that a history would say.
                if rows::drop_opening(&mut conn, row.id).await? {
                    let site = rows::site(&mut conn, row.environment_id).await?;
                    self.close_on_machine(site.and_then(|site| site.machine_id), row.id)
                        .await;
                }
            }
        }
        for row in ended {
            let ending = Ending::because(reason::ENVIRONMENT_STOPPED);
            about(&row, ending);
            if let Some(done) = self.finish(row.id, ending, &origin).await? {
                closed.push(done);
            }
        }
        let mut minutes: HashMap<(Option<Id>, Id), i64> = HashMap::new();
        for row in left {
            let key = (row.thread_id, row.environment_id);
            let kept = match minutes.get(&key) {
                Some(kept) => *kept,
                None => {
                    let mut conn = self.db().acquire().await?;
                    let kept = self.keeping(&mut conn, &row).await?.detached_minutes;
                    minutes.insert(key, kept);
                    kept
                }
            };
            let Some(since) = row.detached_at else {
                continue;
            };
            if now >= since + Span::try_minutes(kept.max(1)).unwrap_or(Span::MAX) {
                let ending = Ending::because(reason::DETACHED);
                about(&row, ending);
                if let Some(done) = self.finish(row.id, ending, &origin).await? {
                    closed.push(done);
                }
            }
        }
        Ok(closed)
    }

    /// Looks again at what every open shell was opened by, as is done whenever the authorization epoch
    /// moves, and ends those whose opener lost it. Returns the shells that were closed; `about` is told of
    /// each before it is ended.
    pub async fn recheck(&self, about: AboutTo<'_>) -> Result<Vec<SessionRow>, EngineError> {
        let live = {
            let mut conn = self.db().acquire().await?;
            rows::all_live(&mut conn).await?
        };
        let mut closed = Vec::new();
        for row in live
            .into_iter()
            .filter(|row| row.state == rows::state::OPEN)
        {
            let holding = {
                let mut conn = self.db().acquire().await?;
                self.holds(&mut conn, &row).await?
            };
            if holding != Holding::Yes {
                about(&row, Ending::REVOKED);
                if let Some(done) = self
                    .finish(row.id, Ending::REVOKED, &Origin::default())
                    .await?
                {
                    closed.push(done);
                }
            }
        }
        Ok(closed)
    }

    // ---------------------------------------------------------------- what an attachment does to the row

    /// A screen takes the shell up. None where the shell is not open.
    pub async fn attached(&self, id: Id) -> Result<Option<SessionRow>, EngineError> {
        let mut conn = self.db().acquire().await?;
        Ok(rows::attach(&mut conn, id, self.now()).await?)
    }

    /// The screen that had the shell let go of it. A screen that was taken over does not call this.
    pub async fn detached(&self, id: Id) -> Result<(), EngineError> {
        let mut conn = self.db().acquire().await?;
        rows::detach(&mut conn, id, self.now()).await?;
        Ok(())
    }

    /// Bytes went through the attachment, `typed` into the shell and `printed` by it. The shell and its
    /// environment count as used.
    pub async fn used(&self, id: Id, typed: i64, printed: i64) -> Result<(), EngineError> {
        let now = self.now();
        let mut tx = self.db().begin().await?;
        if let Some(row) = rows::get(&mut tx, id).await?
            && rows::touch(&mut tx, id, typed, printed, now).await?
        {
            rows::touch_environment(&mut tx, row.environment_id, now).await?;
        }
        tx.commit().await.map_err(orch_db::DbError::from)?;
        Ok(())
    }

    /// The screen has a new size.
    pub async fn resized(&self, id: Id, cols: u16, rows_: u16) -> Result<(), EngineError> {
        let mut conn = self.db().acquire().await?;
        rows::resized(&mut conn, id, i32::from(cols), i32::from(rows_)).await?;
        Ok(())
    }

    // ---------------------------------------------------------------- the agent's own terminal

    /// What the agent of a thread declares of its own terminal, and the machine its harness runs on; none
    /// where the thread has no agent that has told what it is.
    pub async fn agent_screen(
        &self,
        conn: &mut orch_db::Conn,
        thread: Id,
    ) -> Result<Option<AgentScreen>, EngineError> {
        let Some(row) = threads::thread(conn, thread).await? else {
            return Ok(None);
        };
        let Some(agent_id) = row.agent_id else {
            return Ok(None);
        };
        let Some(agent) = resources::agent(conn, agent_id).await? else {
            return Ok(None);
        };
        let Some(descriptor) = crate::harnesses::descriptor_of(&agent)? else {
            return Ok(None);
        };
        Ok(Some(AgentScreen {
            declared: descriptor.capabilities.agent_terminal,
            machine: self.engine.harness_machine_of(conn, thread).await?,
        }))
    }

    /// Has the machine of the thread's harness open its terminal for a screen that watches, or with
    /// `control` types into it, and says which machine a tunnel is to be opened to. None where the agent
    /// declares no such terminal, or the machine would not.
    pub async fn open_agent_terminal(
        &self,
        thread: Id,
        control: bool,
    ) -> Result<Option<Id>, EngineError> {
        let screen = {
            let mut conn = self.db().acquire().await?;
            self.agent_screen(&mut conn, thread).await?
        };
        let Some(AgentScreen {
            declared,
            machine: Some(machine),
        }) = screen
        else {
            return Ok(None);
        };
        let allowed = match declared {
            AgentTerminal::None => false,
            AgentTerminal::View => !control,
            AgentTerminal::Interactive => true,
        };
        if !allowed {
            return Ok(None);
        }
        let command_id = new_id();
        let body = AgentPtyOpen {
            thread_id: thread,
            control,
        };
        let command = Outgoing::new(command_id, machine, SerialKey::Thread(thread), &body)?;
        let commands = &self.engine.inner.commands;
        commands.issue(command).await?;
        match tokio::time::timeout(OPEN_WAIT, commands.wait(command_id)).await {
            Ok(Ok(Outcome::Succeeded(_))) => Ok(Some(machine)),
            Ok(Ok(_)) => Ok(None),
            Ok(Err(error)) => Err(error.into()),
            Err(_) => {
                let _ = commands.cancel(command_id).await;
                Ok(None)
            }
        }
    }

    /// Says in the audit trail that somebody typed into the agent's own terminal: what `terminal.manage`
    /// was used for, and how many bytes.
    pub async fn note_agent_typing(
        &self,
        thread: Id,
        person: Id,
        bytes: i64,
        origin: &Origin,
    ) -> Result<(), EngineError> {
        let mut conn = self.db().acquire().await?;
        let who = self.name_of(&mut conn, person).await?;
        let summary = format!(
            "{who} typed into the agent's own terminal ({})",
            bytes_in_words(bytes)
        );
        let mut entry = Entry::new(
            Actor::User(person),
            "terminal.closed",
            "terminal",
            Some(thread),
            &summary,
        );
        entry.node_id = Some(thread);
        entry.details = json!({ "agent_terminal": true, "bytes_typed": bytes });
        audit::append(&mut conn, &entry, origin, self.now()).await?;
        Ok(())
    }

    /// The shells of `environment`, newest first.
    pub async fn list(
        &self,
        environment: Id,
        opener: Option<Id>,
        which: rows::Which,
        after: Option<After>,
        limit: i64,
    ) -> Result<Vec<SessionRow>, EngineError> {
        let mut conn = self.db().acquire().await?;
        Ok(rows::list(&mut conn, environment, opener, which, after, limit).await?)
    }
}

/// What the agent of a thread declares of its own terminal.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AgentScreen {
    /// Whether there is one, and whether it can be typed into.
    pub declared: AgentTerminal,
    /// The machine its harness runs on.
    pub machine: Option<Id>,
}

/// What opening a shell reserved before the machine was asked.
struct Reserved {
    site: Site,
    directory: String,
    record: bool,
    give_up: Vec<SessionRow>,
}

/// A count of rows as the settings count: a number that is never negative.
fn to_count(count: usize) -> i64 {
    i64::try_from(count).unwrap_or(i64::MAX)
}

/// The start of the UTC day `at` falls in.
fn day_of(at: DateTime<Utc>) -> DateTime<Utc> {
    let date = at.date_naive();
    Utc.from_utc_datetime(&date.and_time(chrono::NaiveTime::MIN))
}

/// A length of time in the few words a line of a history uses.
fn long(seconds: i64) -> String {
    const MINUTE: i64 = 60;
    const HOUR: i64 = 60 * MINUTE;
    const DAY: i64 = 24 * HOUR;
    let (count, unit) = if seconds >= DAY {
        (seconds / DAY, "day")
    } else if seconds >= HOUR {
        (seconds / HOUR, "hour")
    } else if seconds >= MINUTE {
        (seconds / MINUTE, "minute")
    } else {
        (seconds, "second")
    };
    format!("{count} {unit}{}", if count == 1 { "" } else { "s" })
}

/// How a shell ended, in words.
fn ended_in_words(how: &str, exit_code: Option<i32>) -> String {
    match (how, exit_code) {
        (reason::EXITED, Some(code)) => format!("it exited with {code}"),
        (reason::EXITED, None) => "it exited".to_owned(),
        (reason::CLOSED_BY_USER, _) => "it was closed".to_owned(),
        (reason::DETACHED, _) => "nobody came back to it".to_owned(),
        (reason::ENVIRONMENT_STOPPED, _) => "its environment stopped".to_owned(),
        (reason::LINK_LOST, _) => "its machine's connector was restarted under it".to_owned(),
        (reason::REVOKED, _) => "its person lost the right to it".to_owned(),
        _ => "it failed".to_owned(),
    }
}

/// A count of bytes in the few words a line of a history uses.
fn bytes_in_words(bytes: i64) -> String {
    const KIB: i64 = 1024;
    const MIB: i64 = 1024 * KIB;
    if bytes >= MIB {
        format!("{} MiB", bytes / MIB)
    } else if bytes >= KIB {
        format!("{} KiB", bytes / KIB)
    } else {
        format!("{bytes} bytes")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_directory_is_a_path_inside_the_workspace_and_nothing_else() {
        assert_eq!(relative("").unwrap(), "");
        assert_eq!(relative("web/./src/").unwrap(), "web/src");
        for bad in ["/etc", "../x", "a/../../b", "a\\b", "a\0b"] {
            assert!(relative(bad).is_err(), "{bad:?}");
        }
    }

    #[test]
    fn the_directory_a_shell_shows_is_under_the_root_of_its_workspace() {
        assert_eq!(absolute("/workspace", ""), "/workspace");
        assert_eq!(absolute("/workspace/", "web"), "/workspace/web");
    }

    #[test]
    fn a_secrets_value_is_replaced_wherever_it_stands_and_a_value_too_short_to_be_one_is_left() {
        let text = b"token abc12345 and abc12345-extra, then ab";
        let masked = mask_secrets(text, &["abc12345", "abc12345-extra", "ab", ""]);
        assert_eq!(
            String::from_utf8(masked).unwrap(),
            "token [secret] and [secret], then ab",
            "the longer value is replaced whole, and two letters are no secret"
        );
        assert_eq!(
            mask_secrets(b"nothing here", &["abc12345"]),
            b"nothing here"
        );
    }

    #[test]
    fn a_line_of_the_history_says_how_long_and_how_much_in_few_words() {
        assert_eq!(long(1), "1 second");
        assert_eq!(long(90), "1 minute");
        assert_eq!(long(3 * 3600 + 5), "3 hours");
        assert_eq!(bytes_in_words(5), "5 bytes");
        assert_eq!(bytes_in_words(300 * 1024), "300 KiB");
        assert_eq!(ended_in_words(reason::EXITED, Some(3)), "it exited with 3");
    }
}
