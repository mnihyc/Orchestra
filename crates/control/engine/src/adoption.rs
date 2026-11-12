//! Taking in a session that a person began in a harness by hand: the thread that is attached to it, the environment it stands in, and the directory it works in.
//!
//! A harness on a person's machine holds the sessions they began in it, and its adapter can list
//! them (`capabilities.discover_sessions`). To take one in, Orch asks the harness for its sessions,
//! finds the one named, and looks for an environment of the person's on that machine whose folder
//! holds the directory the session ran in; the thread's directory is that directory, relative to
//! the folder. The thread is made as any is, bound to the provider, the agent profile and the
//! environment, and checked as any is for what the caller may use; its first segment already holds
//! the harness's name for the session. The first turn of the thread opens the session as one that
//! was adopted (`SessionMode::Adopt`), which an adapter answers by resuming it.
//!
//! **What the session held before.** At adoption, and only then, the harness is asked what it
//! recorded of the session (`agent.read_session`): its adapter reads the harness's own record (the
//! thread read with its turns, or the session's file in its home) and uploads its earlier turns as
//! entries, what a person said and what the agent said and did, under the names the adapter gives the
//! same items live. They are written into the thread once, in the order they happened and under no
//! turn of it, each marked as from the session before Orch: read-only, and never sent to the agent
//! again, since its harness holds them already and they are no input of the thread. A person's words
//! are attributed to the owner of the machine, who is the harness's user there; on a machine Orch
//! manages they are nobody's. A record that cannot be read leaves the thread without its history, and
//! the line that says the session was adopted says why.
//!
//! **The model.** The thread runs on the model the session ran on, where the harness says which and
//! the caller may run it here (the provider offers it, the place allows it, and what the caller may
//! use of the provider includes it), with the session's effort where that model takes it. Otherwise it
//! has none, a person chooses one as for any thread the place gives none, and the line says why.
//!
//! **Before anything happens.** [`Engine::adoption_plan`] answers what adopting would do: the
//! environment registered for the session's folder, or the folder to register first; the model taken;
//! how much of the history comes. It asks the harness the same questions and writes nothing.

use crate::access::{Answer, ProviderUse, RunsIn};
use crate::binding::{Birth, CreateThreadError, CreatedThread, NAME_MAX_CHARS, NewThread};
use crate::commands::{Outcome, Outgoing};
use crate::engine::Engine;
use crate::error::EngineError;
use crate::harnesses::Discovered;
use crate::ingest::{self, ItemSpec};
use crate::unit::Unit;
use futures::StreamExt;
use orch_access_contract::decisions::{Decision, ObjectRef, Refusal};
use orch_access_contract::permissions::{ObjectKind, Permission};
use orch_agent_contract::discovery::{RecordedEntry, read_record};
use orch_api_contract::nodes::AdoptPlan;
use orch_api_contract::threads::{AdoptHistoryMissing, AdoptModelReason};
use orch_authz::{Act, Caller, Policy, Principal, nodes};
use orch_db::Conn;
use orch_db::queries::adoption;
use orch_db::queries::environments::thread_engine as environments;
use orch_db::queries::resources::thread_engine as resources;
use orch_db::queries::threads::binding as thread_rows;
use orch_link_contract::commands::{AgentReadSession, AgentReadSessionResult, SerialKey};
use orch_types::{Id, Time};
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::time::Duration;

mod birth;
pub mod operation;
mod plan;
mod retained;
pub mod retry;
pub use birth::{Adopted, AdoptionWrite, PreparedAdoption};
pub use plan::{Fetched, Planned};
pub use retained::{
    FetchedHistory, HistoryRecord, RetainedHistory, require_retry_history, retained_history_source,
};

/// The name a thread has when neither its maker nor the session gives it one.
pub const DEFAULT_NAME: &str = "Adopted session";

/// How long the machine is waited for while it reads what a session recorded: as long as the command may
/// wait to be taken up, since a long session is read and uploaded whole within it.
const READ_WAIT: Duration = Duration::from_secs(300);

/// The most bytes of a record that are taken in: the bound the adapters write within, and a little more for
/// a record whose last line ends past it.
const RECORD_READ_MAX_BYTES: usize = orch_agent_contract::discovery::RECORD_MAX_BYTES + 64 * 1024;

/// What a person asks when they take in a session.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Adoption {
    /// The confirmed agent contribution for the new holder; history retry does not apply it.
    pub agent_access: Option<orch_api_contract::nodes::AgentAccessChoice>,
    /// The topic the thread is made in.
    pub topic: Id,
    /// The provider whose harness holds the session.
    pub provider: Id,
    /// The agent profile.
    pub agent: Id,
    /// The harness's own reference to the session.
    pub native_session_ref: String,
    /// The environment; the caller's one on the harness's machine whose folder holds the session's
    /// directory when none is named.
    pub environment: Option<Id>,
    /// The thread's name; the session's title when none is given.
    pub name: Option<String>,
}

/// Why a session was not taken in.
#[derive(Debug, thiserror::Error)]
pub enum AdoptError {
    /// The provider or the profile is not there, or not the harness's.
    #[error("there is no such provider or profile")]
    Gone,
    /// The harness holds no session of that name.
    #[error("the harness holds no such session")]
    NoSession,
    /// A thread has taken the session in already.
    #[error("a thread has taken this session in already")]
    AlreadyAdopted {
        /// The thread.
        thread: Id,
    },
    /// The caller has no environment on the machine whose folder holds the session's directory.
    #[error("no environment of the caller's holds the directory the session ran in")]
    NoEnvironment {
        /// The directory the session ran in, if it said.
        directory: Option<String>,
    },
    /// The environment named does not hold the session's directory.
    #[error("the directory {directory} is not inside the folder {root}")]
    Outside {
        /// The folder of the environment.
        root: String,
        /// The directory the session ran in.
        directory: String,
    },
    /// The harness cannot list the sessions it holds.
    #[error("this harness cannot list the sessions it holds")]
    Unsupported,
    /// The provider has no machine to run on.
    #[error("the provider has no machine to run on")]
    NoMachine,
    /// The machine the harness runs on is not there.
    #[error("the machine the harness runs on is offline")]
    Offline,
    /// The harness or the machine could not do it.
    #[error("the harness could not list its sessions: {0}")]
    Failed(String),
    /// The thread could not be made.
    #[error(transparent)]
    Create(#[from] CreateThreadError),
    /// Something the engine stands on failed.
    #[error(transparent)]
    Internal(#[from] EngineError),
}

impl From<orch_db::DbError> for AdoptError {
    fn from(error: orch_db::DbError) -> Self {
        Self::Internal(error.into())
    }
}

/// What the harness says of a session it holds.
#[derive(Debug, Clone, Deserialize)]
struct Held {
    native_session_ref: String,
    #[serde(default)]
    title: Option<String>,
    #[serde(default)]
    cwd: Option<String>,
    #[serde(default)]
    updated_at: Option<Time>,
}

/// Where an adopted thread would stand: in an environment, at a directory of it, or nowhere until the
/// session's folder is registered.
enum Place {
    In { environment: Id, directory: String },
    RegisterFirst(Option<String>),
}

/// What the harness answered about what it recorded of a session.
enum Recorded {
    /// What it read.
    Read(Box<AgentReadSessionResult>),
    /// Its adapter cannot read a session's record.
    Unsupported(String),
    /// It could not, with what was said.
    Failed(String),
}

#[derive(Debug, thiserror::Error)]
enum RecordReadError {
    #[error(transparent)]
    Source(#[from] EngineError),
    #[error("{0}")]
    Content(String),
}

/// The model an adopted thread runs on, and why not the session's, where not.
struct ModelChoice {
    model: Option<String>,
    effort: Option<String>,
    reason: Option<AdoptModelReason>,
}

/// What was written of a record into a thread.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Written {
    /// The items written now.
    pub items: i64,
    /// Entries that were in the thread already, under the same name, and were not written again.
    pub kept: i64,
}

/// A path split into its steps, and whether the machine writes paths the way Windows does.
struct Steps {
    steps: Vec<String>,
    windows: bool,
}

fn steps_of(path: &str) -> Steps {
    let drive =
        path.len() >= 2 && path.as_bytes()[0].is_ascii_alphabetic() && path.as_bytes()[1] == b':';
    let windows = drive || path.contains('\\');
    Steps {
        steps: path
            .split(['/', '\\'])
            .filter(|step| !step.is_empty() && *step != ".")
            .map(str::to_owned)
            .collect(),
        windows,
    }
}

/// The directory `directory` relative to the folder `root`, written with `/`: none where the
/// folder does not hold it, an empty text where it is the folder itself.
#[must_use]
pub fn relative_to(root: &str, directory: &str) -> Option<String> {
    let (root, directory) = (steps_of(root), steps_of(directory));
    if root.windows != directory.windows || directory.steps.iter().any(|step| step == "..") {
        return None;
    }
    let same = |a: &str, b: &str| {
        if root.windows {
            a.eq_ignore_ascii_case(b)
        } else {
            a == b
        }
    };
    if directory.steps.len() < root.steps.len()
        || !root
            .steps
            .iter()
            .zip(&directory.steps)
            .all(|(a, b)| same(a, b))
    {
        return None;
    }
    Some(directory.steps[root.steps.len()..].join("/"))
}

/// The status an item from a record is written with.
const fn status_word(status: orch_agent_contract::events::ItemStatus) -> &'static str {
    match status {
        orch_agent_contract::events::ItemStatus::InProgress => "in_progress",
        orch_agent_contract::events::ItemStatus::Completed => "completed",
        orch_agent_contract::events::ItemStatus::Failed => "failed",
        orch_agent_contract::events::ItemStatus::Cancelled => "cancelled",
    }
}

/// The same stored adoption-history status used by the engine and HTTP reader.
/// Availability reads current machine facts and queues no native work.
pub async fn history_status(
    conn: &mut Conn,
    thread: Id,
) -> Result<Option<orch_api_contract::threads::AdoptHistoryStatus>, EngineError> {
    use orch_api_contract::threads::{AdoptHistoryState, AdoptHistoryStatus};
    let original: Option<(orch_types::Json, Id, Id, String)> = sqlx::query_as(
        "SELECT i.payload,s.provider_id,s.environment_id,s.arrangement FROM items i JOIN segments s ON s.id=i.segment_id
         WHERE i.thread_id=$1 AND i.kind='marker' AND i.native_ref='adopted' ORDER BY i.seq_created LIMIT 1")
        .bind(thread).fetch_optional(&mut *conn).await.map_err(orch_db::DbError::from)?;
    let Some((payload, provider, environment, arrangement)) = original else {
        return Ok(None);
    };
    let missing = payload["data"]["history_missing"].as_str();
    let reason = payload["data"]["history_error"].as_str().map(str::to_owned);
    let state = match missing {
        None => AdoptHistoryState::Read,
        Some("unsupported") => AdoptHistoryState::Unsupported,
        Some(_) => {
            let provider = resources::provider(conn, provider)
                .await?
                .filter(|row| row.deleted_at.is_none());
            let environment = environments::environment(conn, environment)
                .await?
                .filter(|row| row.deleted_at.is_none());
            let machine = provider
                .zip(environment)
                .and_then(|(provider, environment)| {
                    crate::harnesses::machine_of(
                        Some(&arrangement),
                        &provider,
                        environment.machine_id,
                    )
                });
            match machine {
                Some(machine) => match resources::machine_place(conn, machine).await? {
                    Some(machine) if machine.online => AdoptHistoryState::Unreadable,
                    Some(_) => AdoptHistoryState::Offline,
                    None => AdoptHistoryState::Unavailable,
                },
                None => AdoptHistoryState::Unavailable,
            }
        }
    };
    Ok(Some(AdoptHistoryStatus {
        state,
        reason,
        can_read_again: state == AdoptHistoryState::Unreadable,
    }))
}

impl Engine {
    /// What may be asked by whom: making a thread in the topic, and the use of the provider and of the
    /// profile. Who may not use them learns nothing about the sessions their harness holds.
    async fn may_adopt(&self, caller: &Caller, adoption: &Adoption) -> Result<(), AdoptError> {
        let person = caller.user();
        let mut conn = self.inner.db.acquire().await?;
        let principal =
            Principal::load(&mut conn, caller.clone(), Time::new(self.inner.clock.now()))
                .await
                .map_err(EngineError::from)?;
        let topic = nodes::check(
            &mut conn,
            &principal,
            adoption.topic,
            Permission::TopicStartThreads,
            Act::Use,
            Policy::default(),
        )
        .await
        .map_err(|error| match error {
            orch_authz::Error::NotFound(_) => AdoptError::Create(CreateThreadError::NoSuchTopic),
            other => EngineError::from(other).into(),
        })?;
        if let Decision::Refused(refusal) = topic {
            return Err(CreateThreadError::Denied(Box::new(refusal)).into());
        }
        if let Answer::Refused(missing) = self
            .inner
            .access
            .agent_use(&mut conn, person, adoption.topic, adoption.agent)
            .await
            .map_err(EngineError::from)?
        {
            return Err(denied(
                Permission::AgentUse,
                ObjectKind::Agent,
                adoption.agent,
                missing,
            ));
        }
        if let Answer::Refused(missing) = self
            .inner
            .access
            .provider_use(
                &mut conn,
                &ProviderUse {
                    person,
                    node: adoption.topic,
                    provider: adoption.provider,
                    model: None,
                    effort: None,
                    // Without an environment named, the one found for it is asked about when the
                    // thread is made in it.
                    runs_in: RunsIn::of(adoption.environment),
                },
            )
            .await
            .map_err(EngineError::from)?
        {
            return Err(denied(
                Permission::ProviderUse,
                ObjectKind::Provider,
                adoption.provider,
                missing,
            ));
        }
        Ok(())
    }

    /// The session named, as the harness lists it, and the machine that listed it.
    async fn held_session_with_preparation(
        &self,
        adoption: &Adoption,
        preparation: Option<&operation::Preparation>,
    ) -> Result<(Held, Id), AdoptError> {
        let (observation, machine) = self
            .discover_sessions_observed(adoption.provider, adoption.agent, preparation)
            .await?;
        let listed = match observation {
            Discovered::Sessions(listed) => listed,
            Discovered::Gone => return Err(AdoptError::Gone),
            Discovered::Unsupported => return Err(AdoptError::Unsupported),
            Discovered::NoMachine => return Err(AdoptError::NoMachine),
            Discovered::Offline => return Err(AdoptError::Offline),
            Discovered::Failed(why) => return Err(AdoptError::Failed(why)),
        };
        let session = listed
            .into_iter()
            .filter_map(|each| serde_json::from_value::<Held>(each).ok())
            .find(|each| each.native_session_ref == adoption.native_session_ref)
            .ok_or(AdoptError::NoSession)?;
        // History belongs to the machine which actually listed this session. A later
        // placement observation must not substitute another machine under the same reference.
        let machine = machine.ok_or_else(|| {
            EngineError::unreadable("the native session observation has no supplying machine")
        })?;
        Ok((session, machine))
    }

    /// Where the thread of a session that ran in `cwd` stands on `machine`: the environment named, which has
    /// to hold that directory, or else the caller's environment there whose folder holds it most nearly.
    async fn adopted_place(
        &self,
        conn: &mut Conn,
        principal: &Principal,
        adoption: &Adoption,
        machine: Id,
        cwd: Option<&str>,
        selection: &str,
    ) -> Result<Place, AdoptError> {
        let filter = operation::PlaceFilter::prepare(
            conn,
            principal,
            adoption,
            selection,
            Time::new(self.inner.clock.now()),
        )
        .await?;
        if let Some(named) = adoption.environment {
            let root = environments::environment(conn, named)
                .await?
                .filter(|row| row.deleted_at.is_none() && row.remove_at.is_none())
                .ok_or(CreateThreadError::EnvironmentUnavailable)?
                .workspace_root;
            filter.allows(conn, named).await?;
            if let Answer::Refused(missing) = self
                .inner
                .access
                .environment_place(conn, principal.user(), named)
                .await
                .map_err(EngineError::from)?
            {
                return Err(denied(
                    Permission::EnvironmentUse,
                    ObjectKind::Environment,
                    named,
                    missing,
                ));
            }
            let directory = match cwd {
                Some(cwd) => relative_to(&root, cwd).ok_or_else(|| AdoptError::Outside {
                    root: root.clone(),
                    directory: cwd.to_owned(),
                })?,
                None => String::new(),
            };
            return Ok(Place::In {
                environment: named,
                directory,
            });
        }
        let Some(cwd) = cwd else {
            return Ok(Place::RegisterFirst(None));
        };
        let mut best: Option<(usize, Id, String)> = None;
        for each in adoption::registered_on(conn, machine).await? {
            let Some(directory) = relative_to(&each.workspace_root, cwd) else {
                continue;
            };
            match filter.allows(conn, each.id).await {
                Ok(()) => {}
                Err(AdoptError::Create(
                    CreateThreadError::Denied(_) | CreateThreadError::OperationScope,
                )) => continue,
                Err(error) => return Err(error),
            }
            let managed = self
                .inner
                .access
                .environment_place(conn, principal.user(), each.id)
                .await
                .map_err(EngineError::from)?;
            if matches!(managed, Answer::Refused(_)) {
                continue;
            }
            // The folder nearest to the session's directory holds it most exactly.
            let depth = steps_of(&each.workspace_root).steps.len();
            if best.as_ref().is_none_or(|(deepest, _, _)| depth > *deepest) {
                best = Some((depth, each.id, directory));
            }
        }
        Ok(match best {
            Some((_, environment, directory)) => Place::In {
                environment,
                directory,
            },
            None => Place::RegisterFirst(Some(cwd.to_owned())),
        })
    }

    /// The one native session-record command supplier, with an optional actual source observer.
    async fn read_session_record_observed<E: operation::Entry>(
        &self,
        machine: Id,
        adoption: &Adoption,
        write: bool,
        preparation: Option<&operation::Preparation>,
        observer: Option<&mut operation::RecordObservation<E>>,
    ) -> Result<Recorded, EngineError> {
        let asked = orch_types::new_id();
        let command = Outgoing::new(
            asked,
            machine,
            SerialKey::Harness {
                provider_id: adoption.provider,
                agent_id: adoption.agent,
            },
            &AgentReadSession {
                provider_id: adoption.provider,
                agent_id: adoption.agent,
                native_session_ref: adoption.native_session_ref.clone(),
                write,
            },
        )?;
        let command = match preparation {
            Some(preparation) => {
                preparation
                    .command(
                        self,
                        crate::harnesses::HarnessKey {
                            provider: adoption.provider,
                            agent: adoption.agent,
                            machine,
                        },
                        command,
                    )
                    .await?
            }
            None => command,
        };
        if let Some(observer) = observer {
            let Some(crate::commands::CommandAuthority::AdoptionPreparation(evidence)) =
                command.authority.as_ref()
            else {
                return Err(EngineError::BeforeEffect(Box::new(
                    operation::PreparationEnded,
                )));
            };
            observer.retain_command(evidence.preparation())?;
        }
        let answered = tokio::time::timeout(
            READ_WAIT,
            crate::commands::run(self.inner.commands.as_ref(), command),
        )
        .await;
        Ok(match answered {
            Err(_elapsed) => {
                self.inner.commands.cancel(asked).await?;
                Recorded::Failed("the machine did not answer in time".to_owned())
            }
            Ok(outcome) => match outcome? {
                Outcome::Succeeded(result) => {
                    Recorded::Read(Box::new(serde_json::from_value(result)?))
                }
                Outcome::Failed(failure) if failure.code == "unsupported" => {
                    Recorded::Unsupported(failure.detail)
                }
                other => Recorded::Failed(crate::provider_setup::failure_text(&other)),
            },
        })
    }

    /// The entries of the record a machine uploaded, using the one native parser and bounds.
    async fn record_entries_from<E: operation::Entry>(
        &self,
        read: &AgentReadSessionResult,
        mut source: Option<&mut operation::RecordObservation<E>>,
    ) -> Result<Vec<RecordedEntry>, RecordReadError> {
        let Some(record) = read.record else {
            return Ok(Vec::new());
        };
        let store = self.blobs().ok_or_else(|| {
            RecordReadError::Content("no store of content is attached".to_owned())
        })?;
        let mut deadline = match source.as_deref_mut() {
            Some(source) => Some(source.refresh(self).await?),
            None => None,
        };
        let response = store.get(&record.blob, None).await;
        if let Some(source) = source {
            deadline = Some(source.refresh(self).await?);
        }
        let mut body = response
            .map_err(|error| RecordReadError::Content(error.to_string()))?
            .body;
        let mut bytes = Vec::new();
        loop {
            let piece = body.next().await;
            if let Some(deadline) = &deadline {
                deadline.check(self.inner.clock.as_ref())?;
            }
            let Some(piece) = piece else {
                break;
            };
            let piece = piece.map_err(|error| RecordReadError::Content(error.to_string()))?;
            if bytes.len() + piece.len() > RECORD_READ_MAX_BYTES {
                return Err(RecordReadError::Content(
                    "the record is larger than what is taken in".to_owned(),
                ));
            }
            bytes.extend_from_slice(&piece);
        }
        let (entries, unreadable) = read_record(&String::from_utf8_lossy(&bytes));
        if unreadable > 0 {
            tracing::warn!(unreadable, "lines of a session's record could not be read");
        }
        if let Some(deadline) = &deadline {
            deadline.check(self.inner.clock.as_ref())?;
        }
        Ok(entries)
    }

    /// The model an adopted thread runs on: the session's own where its harness says which and `person` may
    /// run it here, with the session's effort where that model takes it.
    async fn adopted_model(
        &self,
        conn: &mut Conn,
        person: Id,
        adoption: &Adoption,
        reported: Option<&str>,
        effort: Option<&str>,
    ) -> Result<ModelChoice, EngineError> {
        let refused = |reason| ModelChoice {
            model: None,
            effort: None,
            reason: Some(reason),
        };
        let Some(model) = reported.map(str::trim).filter(|model| !model.is_empty()) else {
            return Ok(refused(AdoptModelReason::NotReported));
        };
        let entry = resources::model(conn, adoption.provider, model).await?;
        let settings = crate::resolved::load(conn, adoption.topic).await?;
        match crate::models::check_allowed(
            entry.as_ref(),
            settings.models_allowed.as_deref(),
            model,
        ) {
            Err(crate::models::NotAllowed::NotEnabled) => {
                return Ok(refused(AdoptModelReason::NotOffered));
            }
            Err(crate::models::NotAllowed::LeftOutByThePlace) => {
                return Ok(refused(AdoptModelReason::NotAllowed));
            }
            Ok(()) => {}
        }
        let Some(entry) = entry else {
            return Ok(refused(AdoptModelReason::NotOffered));
        };
        let effort = match effort {
            Some(effort)
                if crate::binding::model_parameters(&entry)?
                    .efforts()
                    .iter()
                    .any(|listed| listed == effort) =>
            {
                Some(effort.to_owned())
            }
            _ => None,
        };
        let usable = self
            .inner
            .access
            .provider_use(
                conn,
                &ProviderUse {
                    person,
                    node: adoption.topic,
                    provider: adoption.provider,
                    model: Some(model),
                    effort: effort.as_deref(),
                    runs_in: RunsIn::of(adoption.environment),
                },
            )
            .await?;
        if matches!(usable, Answer::Refused(_)) {
            return Ok(refused(AdoptModelReason::NotAllowed));
        }
        Ok(ModelChoice {
            model: Some(model.to_owned()),
            effort,
            reason: None,
        })
    }

    /// Writes the entries of a session's record into the thread as items from the session before Orch, in a
    /// transaction of its own: what the segment holds already is not written
    /// again.
    pub async fn take_in_record(
        &self,
        thread: Id,
        segment: Id,
        author: Option<Id>,
        entries: &[RecordedEntry],
    ) -> Result<Written, EngineError> {
        let mut tx = self.inner.db.begin().await?;
        let (written, finished) = {
            let mut unit = Unit::new(&mut tx, self.inner.clock.now());
            let written = self
                .write_session_record(&mut unit, thread, segment, author, entries)
                .await?;
            (written, unit.finish().await?)
        };
        self.commit(tx, finished).await?;
        Ok(written)
    }

    /// Writes the entries of a session's record into the thread as items from the session before Orch, in
    /// their order and under no turn, once: an entry whose name the segment holds already is not written again.
    /// A person's words are `author`'s, the harness's user.
    pub(crate) async fn write_session_record(
        &self,
        unit: &mut Unit<'_>,
        thread: Id,
        segment: Id,
        author: Option<Id>,
        entries: &[RecordedEntry],
    ) -> Result<Written, EngineError> {
        let mut written = Written::default();
        for entry in entries {
            let (native_ref, kind, status, payload, stream, parent, by, at) = match entry {
                RecordedEntry::Said(words) => (
                    words.native_ref.as_str(),
                    "user_message".to_owned(),
                    "completed",
                    json!({
                        "parts": [{ "type": "text", "text": words.text }],
                        "input_id": null, "delivery": "queue", "source": "user",
                    }),
                    orch_agent_contract::events::MAIN_STREAM,
                    None,
                    author,
                    words.at,
                ),
                RecordedEntry::Item(item) => {
                    let content = serde_json::to_value(&item.content)?;
                    let parent = match item.parent_native_ref.as_deref() {
                        Some(parent) => orch_db::queries::threads::item_by_native_ref(
                            &mut *unit.conn,
                            segment,
                            parent,
                        )
                        .await?
                        .map(|row| row.id),
                        None => None,
                    };
                    (
                        item.native_ref.as_str(),
                        content["kind"].as_str().unwrap_or_default().to_owned(),
                        status_word(item.status),
                        content["payload"].clone(),
                        item.stream.as_str(),
                        parent,
                        None,
                        item.at,
                    )
                }
            };
            let made = ingest::add_item(
                unit,
                &ItemSpec {
                    thread,
                    segment,
                    turn: None,
                    stream,
                    parent,
                    kind: &kind,
                    status,
                    author: by,
                    payload: &payload,
                    native_ref: Some(native_ref),
                },
            )
            .await?;
            match made {
                Some(row) => {
                    adoption::mark_before_orch(&mut *unit.conn, row.id, at.map(Time::get)).await?;
                    written.items += 1;
                }
                None => written.kept += 1,
            }
        }
        Ok(written)
    }

    /// Answers what adopting a session would do, before anything does: where the thread would stand (or the
    /// folder to register first), the model it would run on, and how much of what the session held before
    /// would come. The caller needs what adopting needs; nothing is written.
    pub async fn adoption_plan(
        &self,
        caller: &Caller,
        adoption: Adoption,
    ) -> Result<AdoptPlan, AdoptError> {
        let fetched = self
            .fetch_native_for::<operation::Previewing>(caller, adoption)
            .await?;
        let mut conn = self.inner.db.acquire().await?;
        let principal =
            Principal::load(&mut conn, caller.clone(), Time::new(self.inner.clock.now()))
                .await
                .map_err(EngineError::from)?;
        let planned = self
            .prepare_adoption_plan_in(&mut conn, &principal, &fetched)
            .await?;
        self.render_adoption_plan_in(&mut conn, &fetched, &planned)
            .await
    }

    /// Takes in a session that exists in a provider's harness as a thread of a topic. The caller needs what
    /// making a thread needs (the topic, the use of the provider and of the profile, the management of the
    /// environment); the session is found by asking the harness, so a caller who may not use the provider is
    /// told nothing of what it holds. What the session held before comes with it, once, and it runs on the
    /// session's model where that may run here.
    pub async fn adopt_thread(
        &self,
        caller: &Caller,
        adoption: Adoption,
    ) -> Result<CreatedThread, AdoptError> {
        let fetched = std::sync::Arc::new(
            self.fetch_native_for::<operation::Adopting>(caller, adoption)
                .await?,
        );
        let mut tx = self.inner.db.begin().await?;
        orch_db::queries::transaction_guard::lock_native_actor(&mut tx, caller.user())
            .await?
            .ok_or(AdoptError::Gone)?;
        let original = Principal::load(&mut tx, caller.clone(), self.inner.clock.time())
            .await
            .map_err(EngineError::from)?;
        let prepared = self
            .prepare_adoption_birth_in(&mut tx, &original, fetched)
            .await?;
        let prefix = crate::operation_scope::creators::Prefix::prepare(
            &mut tx,
            prepared.principal(),
            "ThreadAdopt",
            prepared.new_thread(),
            None,
        )
        .await?;
        let (created, complete) = self
            .create_thread_staged_taking_in_transaction(
                &mut tx,
                prefix,
                prepared.new_thread(),
                prepared.birth(),
                orch_types::new_id(),
                prepared.native_session_ref(),
            )
            .await?;
        let principal = Principal::load(&mut tx, complete.caller, self.inner.clock.time())
            .await
            .map_err(EngineError::from)?;
        let actual = principal.caller().clone();
        let until = prepared.record_until();
        let clock = self.inner.clock.clone();
        let write = self
            .write_adoption_in(&mut tx, &principal, &prepared, &created, move || {
                if actual.key_deadline_allows(clock.as_ref())
                    && until.is_none_or(|end| clock.time() < end)
                {
                    Ok(())
                } else {
                    Err(EngineError::BeforeEffect(Box::new(
                        operation::PreparationEnded,
                    )))
                }
            })
            .await?;
        if !principal
            .caller()
            .key_deadline_allows(self.inner.clock.as_ref())
            || until.is_some_and(|end| self.inner.clock.time() >= end)
        {
            return Err(EngineError::BeforeEffect(Box::new(operation::PreparationEnded)).into());
        }
        Ok(self.commit_adoption(tx, write).await?.created)
    }

    /// Reads an adopted session's missing earlier history again. Its ordinary inputs, model and queue
    /// remain untouched; the original segment and its stable native references make importing it repeatable.
    pub async fn retry_adopted_history(
        &self,
        caller: &Caller,
        thread: Id,
    ) -> Result<(), AdoptError> {
        use orch_api_contract::{
            adoption_sources, routes::Endpoint, threads::ThreadAdoptRetryHistory,
        };
        let mut conn = self.inner.db.acquire().await?;
        let target = crate::operation_scope::targets::node(&mut conn, thread)
            .await
            .map_err(EngineError::unreadable)?;
        let root = crate::operation_scope::sources::Facts::one(
            adoption_sources::Source::RetainedThread,
            orch_access_contract::key_scope::KeyTargetKind::Object(ObjectKind::Thread),
            target,
        );
        let known = crate::operation_scope::sources::bind_known(
            ThreadAdoptRetryHistory::ROUTE,
            &adoption_sources::RETRY_PREPARATION,
            &[root],
        )
        .map_err(EngineError::unreadable)?;
        let selecting = known
            .preparation_caller(caller)
            .map_err(EngineError::unreadable)?;
        let selecting = Principal::load(&mut conn, selecting, Time::new(self.inner.clock.now()))
            .await
            .map_err(EngineError::from)?;
        retry::require_thread(&mut conn, &selecting, thread).await?;
        let source = retry::select(&mut conn, thread).await?;
        let principal = retry::principal(
            &mut conn,
            caller,
            &source,
            Time::new(self.inner.clock.now()),
        )
        .await?;
        let preparation = operation::Preparation::capture_retry(
            principal.caller(),
            operation::HttpCredential::Native,
            source.clone(),
        )?;
        drop(conn);
        let fetched = std::sync::Arc::new(
            self.fetch_retry_adopted_history(
                principal.caller(),
                source,
                operation::HttpCredential::Native,
                None,
                None,
            )
            .await?,
        );
        let mut tx = self.inner.db.begin().await?;
        orch_db::queries::transaction_guard::lock_action(&mut tx).await?;
        let (mut current, until) = crate::operation_scope::request_origin::current(
            &mut tx,
            self.inner.clock.as_ref(),
            preparation.origin(),
            &operation::HttpCredential::Native,
            None,
            "ThreadAdoptRetryHistory",
        )
        .await
        .map_err(|error| EngineError::BeforeEffect(Box::new(error)))?;
        if let Some(origin) = fetched.origin() {
            let earlier = origin
                .restore_current(&mut tx, self.inner.clock.as_ref())
                .await
                .map_err(EngineError::from)?
                .ok_or_else(|| EngineError::BeforeEffect(Box::new(operation::PreparationEnded)))?;
            current = current
                .intersect_restrictions(&earlier)
                .map_err(EngineError::unreadable)?;
        }
        let principal = retry::principal(
            &mut tx,
            &current,
            fetched.source(),
            Time::new(self.inner.clock.now()),
        )
        .await?;
        let prepared = self
            .prepare_retry_adopted_history(&mut tx, &principal, fetched)
            .await?;
        let principal = retry::principal(
            &mut tx,
            &current,
            prepared.source(),
            Time::new(self.inner.clock.now()),
        )
        .await?;
        let caller = principal.caller().clone();
        let deadline = prepared.until().into_iter().chain(until).min();
        let check = || {
            if caller.key_deadline_allows(self.inner.clock.as_ref())
                && deadline.is_none_or(|until| self.inner.clock.now() < until.get())
            {
                Ok(())
            } else {
                Err(EngineError::BeforeEffect(Box::new(
                    operation::PreparationEnded,
                )))
            }
        };
        self.apply_retry_adopted_history(tx, &principal, prepared, check, check)
            .await?;
        Ok(())
    }

    /// The band's availability is read from current machine facts; checking it queues no native work.
    pub async fn adoption_history_status(
        &self,
        thread: Id,
    ) -> Result<Option<orch_api_contract::threads::AdoptHistoryStatus>, EngineError> {
        let mut conn = self.inner.db.acquire().await?;
        history_status(&mut conn, thread).await
    }
}

/// The name of an adopted thread, and whether automation may change it: the one given, else the session's
/// title, else [`DEFAULT_NAME`]. A name the person gave is theirs.
fn thread_name(given: Option<&str>, title: Option<&str>) -> (String, bool) {
    let given = given.map(str::trim).filter(|name| !name.is_empty());
    let titled = title
        .map(str::trim)
        .filter(|title| !title.is_empty())
        .map(name_of_title);
    (
        given
            .map(str::to_owned)
            .or(titled)
            .unwrap_or_else(|| DEFAULT_NAME.to_owned()),
        given.is_none(),
    )
}

/// A name made of the title a session has: the title as far as a name may go, since a harness
/// titles a session by its first words, which are as long as a person wrote them.
fn name_of_title(title: &str) -> String {
    title
        .chars()
        .take(NAME_MAX_CHARS)
        .collect::<String>()
        .trim_end()
        .to_owned()
}

/// What is refused when something the thread is made of is not the caller's to use.
fn denied(
    permission: Permission,
    kind: ObjectKind,
    id: Id,
    missing: Vec<orch_access_contract::decisions::MissingAccess>,
) -> AdoptError {
    CreateThreadError::Denied(Box::new(Refusal::of(
        permission,
        ObjectRef { kind, id },
        missing,
    )))
    .into()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_directory_inside_a_folder_is_written_relative_to_it() {
        assert_eq!(
            relative_to("/srv/ada/work", "/srv/ada/work"),
            Some(String::new())
        );
        assert_eq!(
            relative_to("/srv/ada/work/", "/srv/ada/work/web/app"),
            Some("web/app".to_owned())
        );
        assert_eq!(
            relative_to("/srv/ada/work", "/srv/ada/worker"),
            None,
            "a name that begins alike is another folder"
        );
        assert_eq!(relative_to("/srv/ada/work", "/srv/ada"), None);
        assert_eq!(relative_to("/srv/ada/work", "/srv/ada/work/../other"), None);
    }

    #[test]
    fn a_title_longer_than_a_name_may_be_is_cut_where_a_name_ends() {
        assert_eq!(name_of_title("Fix the login"), "Fix the login");
        let long = format!(
            "{} {}",
            "a".repeat(NAME_MAX_CHARS - 1),
            "words after the end"
        );
        let name = name_of_title(&long);
        assert_eq!(
            name.chars().count(),
            NAME_MAX_CHARS - 1,
            "no space is left at its end"
        );
        assert!(name.chars().all(|each| each == 'a'));
        let wide = "é".repeat(NAME_MAX_CHARS + 5);
        assert_eq!(name_of_title(&wide).chars().count(), NAME_MAX_CHARS);
    }

    #[test]
    fn a_windows_path_is_compared_without_regard_to_case_and_never_with_a_unix_one() {
        assert_eq!(
            relative_to(r"C:\Users\Ada\Work", r"c:\users\ada\work\Web"),
            Some("Web".to_owned())
        );
        assert_eq!(relative_to(r"C:\Users\Ada\Work", "/srv/ada/work"), None);
        assert_eq!(relative_to("/srv/ada/work", r"C:\Users\Ada\Work"), None);
    }

    #[test]
    fn a_name_given_is_the_person_s_and_a_session_s_title_is_left_to_automation() {
        assert_eq!(
            thread_name(Some(" Mine "), Some("Fix")),
            ("Mine".to_owned(), false)
        );
        assert_eq!(thread_name(None, Some(" Fix ")), ("Fix".to_owned(), true));
        assert_eq!(
            thread_name(Some("  "), None),
            (DEFAULT_NAME.to_owned(), true)
        );
    }
}
