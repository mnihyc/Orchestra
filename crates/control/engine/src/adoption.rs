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

use crate::access::{Answer, ProviderUse};
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
use serde::Deserialize;
use serde_json::json;
use std::time::Duration;

/// The name a thread has when neither its maker nor the session gives it one.
pub const DEFAULT_NAME: &str = "Adopted session";

/// How long the machine is waited for while it reads what a session recorded: as long as the command may
/// wait to be taken up, since a long session is read and uploaded whole within it.
const READ_WAIT: Duration = Duration::from_secs(300);

/// The most bytes of a record that are taken in: the bound the adapters write within, and a little more for
/// a record whose last line ends past it.
const RECORD_READ_MAX_BYTES: usize = orch_agent_contract::discovery::RECORD_MAX_BYTES + 64 * 1024;

/// What a person asks when they take in a session.
#[derive(Debug, Clone)]
pub struct Adoption {
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
    async fn held_session(&self, adoption: &Adoption) -> Result<(Held, Id), AdoptError> {
        let keeper = {
            let mut conn = self.inner.db.acquire().await?;
            resources::provider(&mut conn, adoption.provider)
                .await?
                .filter(|row| row.deleted_at.is_none())
                .ok_or(AdoptError::Gone)?
        };
        let listed = match self
            .discover_sessions(adoption.provider, adoption.agent)
            .await?
        {
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
        // The machine that listed it, which has the folders to put it in and the record to read.
        let machine = self
            .harness_machine(&keeper, adoption.agent)
            .await?
            .ok_or(AdoptError::Offline)?;
        Ok((session, machine))
    }

    /// Where the thread of a session that ran in `cwd` stands on `machine`: the environment named, which has
    /// to hold that directory, or else the caller's environment there whose folder holds it most nearly.
    async fn adopted_place(
        &self,
        conn: &mut Conn,
        person: Id,
        machine: Id,
        named: Option<Id>,
        cwd: Option<&str>,
    ) -> Result<Place, AdoptError> {
        if let Some(named) = named {
            let root = environments::environment(conn, named)
                .await?
                .filter(|row| row.deleted_at.is_none() && row.remove_at.is_none())
                .ok_or(CreateThreadError::EnvironmentUnavailable)?
                .workspace_root;
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
            let managed = self
                .inner
                .access
                .environment_place(conn, person, each.id)
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

    /// Asks the harness of `provider` and `agent` on `machine` what it recorded of a session; with `write`,
    /// its earlier turns come up as a record. What goes wrong is said, and is no failure of the adoption.
    async fn read_session_record(
        &self,
        machine: Id,
        adoption: &Adoption,
        write: bool,
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

    /// The entries of the record a machine uploaded.
    async fn record_entries(
        &self,
        read: &AgentReadSessionResult,
    ) -> Result<Vec<RecordedEntry>, String> {
        let Some(record) = read.record else {
            return Ok(Vec::new());
        };
        let store = self
            .blobs()
            .ok_or_else(|| "no store of content is attached".to_owned())?;
        let mut body = store
            .get(&record.blob, None)
            .await
            .map_err(|error| error.to_string())?
            .body;
        let mut bytes: Vec<u8> = Vec::new();
        while let Some(piece) = body.next().await {
            let piece = piece.map_err(|error| error.to_string())?;
            if bytes.len() + piece.len() > RECORD_READ_MAX_BYTES {
                return Err("the record is larger than what is taken in".to_owned());
            }
            bytes.extend_from_slice(&piece);
        }
        let (entries, unreadable) = read_record(&String::from_utf8_lossy(&bytes));
        if unreadable > 0 {
            tracing::warn!(unreadable, "lines of a session's record could not be read");
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
        let person = caller.user();
        self.may_adopt(caller, &adoption).await?;
        let (session, machine) = self.held_session(&adoption).await?;
        let recorded = self.read_session_record(machine, &adoption, false).await?;
        let mut conn = self.inner.db.acquire().await?;
        let adopted_thread_id = thread_rows::adopting_threads(
            &mut conn,
            std::slice::from_ref(&adoption.native_session_ref),
        )
        .await?
        .into_iter()
        .next()
        .map(|(_, thread)| thread);
        let place = self
            .adopted_place(
                &mut conn,
                person,
                machine,
                adoption.environment,
                session.cwd.as_deref(),
            )
            .await?;
        let (environment, directory, register_first) = match place {
            Place::In {
                environment,
                directory,
            } => {
                let row = environments::environment(&mut conn, environment)
                    .await?
                    .ok_or(CreateThreadError::EnvironmentUnavailable)?;
                let named = crate::views::environment_ref_json(
                    &mut conn,
                    &row,
                    Time::new(self.inner.clock.now()),
                )
                .await?;
                (
                    Some(serde_json::from_value(named).map_err(EngineError::from)?),
                    Some(directory),
                    None,
                )
            }
            Place::RegisterFirst(directory) => (None, None, directory),
        };
        let (reported, effort, turns, history_missing) = match &recorded {
            Recorded::Read(read) => (read.model.clone(), read.effort.clone(), read.turns, None),
            Recorded::Unsupported(_) => (None, None, 0, Some(AdoptHistoryMissing::Unsupported)),
            Recorded::Failed(why) => {
                tracing::info!(%why, "what a session recorded could not be read for its plan");
                (None, None, 0, Some(AdoptHistoryMissing::Unreadable))
            }
        };
        let choice = self
            .adopted_model(
                &mut conn,
                person,
                &adoption,
                reported.as_deref(),
                effort.as_deref(),
            )
            .await?;
        Ok(AdoptPlan {
            session: orch_api_contract::resources::NativeSession {
                native_session_ref: session.native_session_ref.clone(),
                title: session.title.clone(),
                cwd: session.cwd.clone(),
                updated_at: session.updated_at,
                adopted_thread_id,
            },
            name: thread_name(adoption.name.as_deref(), session.title.as_deref()).0,
            environment,
            directory,
            register_first,
            session_model: reported,
            model: choice.model,
            model_reason: choice.reason,
            turns,
            history_missing,
            adopted_thread_id,
        })
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
        let person = caller.user();
        self.may_adopt(caller, &adoption).await?;
        let (session, machine) = self.held_session(&adoption).await?;
        // What the session recorded is read before anything is written, outside any transaction: a long
        // session takes a while to read and upload.
        let recorded = self.read_session_record(machine, &adoption, true).await?;

        // From the look for a thread that has the session to the making of the thread and the writing of
        // its history, no other request takes in the same session: this transaction holds the lock of the
        // session's name until the thread is there to be found by whoever waits for it.
        // This coordinator only holds the session-name advisory lock and reads
        // environments. The nested creation transaction takes authority first;
        // taking it here too would make that transaction wait for this one.
        let mut guard =
            orch_db::queries::transaction_guard::scope(false, self.inner.db.begin()).await?;
        adoption::lock_session(&mut guard, &adoption.native_session_ref).await?;
        if let Some((_, thread)) = thread_rows::adopting_threads(
            &mut guard,
            std::slice::from_ref(&adoption.native_session_ref),
        )
        .await?
        .into_iter()
        .next()
        {
            return Err(AdoptError::AlreadyAdopted { thread });
        }

        // The environment and the thread's directory in it.
        let (environment, directory) = match self
            .adopted_place(
                &mut guard,
                person,
                machine,
                adoption.environment,
                session.cwd.as_deref(),
            )
            .await?
        {
            Place::In {
                environment,
                directory,
            } => (environment, directory),
            Place::RegisterFirst(directory) => {
                return Err(AdoptError::NoEnvironment { directory });
            }
        };

        // The model it runs on, and the history it brings.
        let mut history_error = match &recorded {
            Recorded::Unsupported(why) | Recorded::Failed(why) => Some(why.clone()),
            Recorded::Read(_) => None,
        };
        let (reported, effort, history) = match &recorded {
            Recorded::Read(read) => match self.record_entries(read).await {
                Ok(entries) => (read.model.clone(), read.effort.clone(), Ok((read, entries))),
                Err(why) => {
                    history_error = Some(why.clone());
                    tracing::warn!(%why, "the record of an adopted session could not be read");
                    (
                        read.model.clone(),
                        read.effort.clone(),
                        Err(AdoptHistoryMissing::Unreadable),
                    )
                }
            },
            Recorded::Unsupported(_) => (None, None, Err(AdoptHistoryMissing::Unsupported)),
            Recorded::Failed(why) => {
                tracing::warn!(%why, "the harness could not read what an adopted session recorded");
                (None, None, Err(AdoptHistoryMissing::Unreadable))
            }
        };
        let choice = self
            .adopted_model(
                &mut guard,
                person,
                &adoption,
                reported.as_deref(),
                effort.as_deref(),
            )
            .await?;

        let (name, titled) = thread_name(adoption.name.as_deref(), session.title.as_deref());
        let created = self
            .create_thread_taking(
                caller,
                NewThread {
                    name,
                    topic: adoption.topic,
                    agent: adoption.agent,
                    provider: adoption.provider,
                    environment,
                    directory,
                    arrangement: None,
                    model: choice.model.clone(),
                    effort: choice.effort.clone(),
                },
                Birth {
                    prepare_source: Some(false),
                    // A name the session gave is the harness's to change as it goes on.
                    titled_by_automation: titled,
                    ..Birth::default()
                },
                None,
                Some(&adoption.native_session_ref),
            )
            .await?;

        // The line that says so, then what the session held before, in the transaction that holds the lock.
        let author = adoption::machine_owner(&mut guard, machine).await?;
        let _finished = {
            let mut unit = Unit::new(&mut guard, self.inner.clock.now());
            let (turns, items, missing, report) = match &history {
                Ok((read, entries)) => (
                    read.turns,
                    i64::try_from(entries.len()).unwrap_or(i64::MAX),
                    None,
                    Some(serde_json::to_value(&read.report).map_err(EngineError::from)?),
                ),
                Err(missing) => (0, 0, Some(*missing), None),
            };
            let marker = json!({
                "marker": "adopted",
                "data": {
                    "by": crate::views::user_ref_json(&mut *unit.conn, person).await?,
                    "native_session_ref": adoption.native_session_ref,
                    "turns": turns,
                    "items": items,
                    "history_missing": missing,
                    "history_error": history_error,
                    "session_model": reported,
                    "model_reason": choice.reason,
                    "history_author": author,
                    "history_before": session.updated_at.unwrap_or_else(|| Time::new(unit.now())),
                }
            });
            ingest::add_marker(
                &mut unit,
                created.thread,
                created.segment,
                None,
                &marker,
                "adopted",
            )
            .await?;
            if let Ok((_, entries)) = &history {
                self.write_session_record(
                    &mut unit,
                    created.thread,
                    created.segment,
                    author,
                    entries,
                )
                .await?;
            }
            if let Some(report) = &report {
                adoption::set_adopted_report(&mut *unit.conn, created.segment, report).await?;
            }
            unit.finish().await?
        };
        guard.commit().await.map_err(orch_db::DbError::from)?;
        Ok(created)
    }

    /// Reads an adopted session's missing earlier history again. Its ordinary inputs, model and queue
    /// remain untouched; the original segment and its stable native references make importing it repeatable.
    pub async fn retry_adopted_history(
        &self,
        caller: &Caller,
        thread: Id,
    ) -> Result<(), AdoptError> {
        let mut conn = self.inner.db.acquire().await?;
        self.may_retry_history(&mut conn, caller, thread).await?;
        drop(conn);
        if let Some(status) = self.adoption_history_status(thread).await? {
            use orch_api_contract::threads::AdoptHistoryState;
            match status.state {
                AdoptHistoryState::Read => return Ok(()),
                AdoptHistoryState::Unsupported => return Err(AdoptError::Unsupported),
                AdoptHistoryState::Offline => return Err(AdoptError::Offline),
                AdoptHistoryState::Unavailable => return Err(AdoptError::NoMachine),
                AdoptHistoryState::Unreadable => {}
            }
        }
        let mut conn = self.inner.db.acquire().await?;
        // Original marker, segment, provider, agent, topic, payload and observation time.
        type HistoryOrigin = (
            Id,
            Id,
            Id,
            Id,
            Id,
            orch_types::Json,
            chrono::DateTime<chrono::Utc>,
        );
        let original: Option<HistoryOrigin> = sqlx::query_as(
            "SELECT i.id,s.id,s.provider_id,s.agent_id,t.topic_id,i.payload,i.created_at
             FROM items i JOIN segments s ON s.id=i.segment_id JOIN threads t ON t.node_id=i.thread_id
             JOIN nodes n ON n.id=t.node_id
             WHERE i.thread_id=$1 AND i.native_ref='adopted' AND i.kind='marker' AND n.deleted_at IS NULL
             ORDER BY i.seq_created LIMIT 1")
            .bind(thread).fetch_optional(&mut *conn).await.map_err(orch_db::DbError::from)?;
        let Some((marker_id, segment, provider, agent, topic, payload, created_at)) = original
        else {
            return Err(AdoptError::Gone);
        };
        if payload["data"]["history_missing"].is_null() {
            return Ok(());
        }
        let reference = payload["data"]["native_session_ref"]
            .as_str()
            .ok_or(AdoptError::NoSession)?
            .to_owned();
        let before = payload["data"]
            .get("history_before")
            .cloned()
            .map(serde_json::from_value::<Time>)
            .transpose()
            .map_err(EngineError::from)?
            .map_or(created_at, Time::get);
        let author = payload["data"]
            .get("history_author")
            .cloned()
            .map(serde_json::from_value::<Option<Id>>)
            .transpose()
            .map_err(EngineError::from)?
            .flatten();
        let has_turns: bool =
            sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM turns WHERE segment_id=$1)")
                .bind(segment)
                .fetch_one(&mut *conn)
                .await
                .map_err(orch_db::DbError::from)?;
        drop(conn);
        let adoption = Adoption {
            topic,
            provider,
            agent,
            native_session_ref: reference,
            environment: None,
            name: None,
        };
        let (held, machine) = self.held_session(&adoption).await?;
        let read = match self.read_session_record(machine, &adoption, true).await? {
            Recorded::Read(read) => read,
            Recorded::Unsupported(why) => {
                self.record_history_failure(
                    caller,
                    thread,
                    marker_id,
                    AdoptHistoryMissing::Unsupported,
                    &why,
                )
                .await?;
                return Ok(());
            }
            Recorded::Failed(why) => {
                self.record_history_failure(
                    caller,
                    thread,
                    marker_id,
                    AdoptHistoryMissing::Unreadable,
                    &why,
                )
                .await?;
                return Ok(());
            }
        };
        let mut entries = match self.record_entries(&read).await {
            Ok(entries) => entries,
            Err(why) => {
                self.record_history_failure(
                    caller,
                    thread,
                    marker_id,
                    AdoptHistoryMissing::Unreadable,
                    &why,
                )
                .await?;
                return Ok(());
            }
        };
        let at = |entry: &RecordedEntry| match entry {
            RecordedEntry::Said(words) => words.at,
            RecordedEntry::Item(item) => item.at,
        };
        if (has_turns
            || held
                .updated_at
                .is_some_and(|updated| updated.get() > before))
            && entries.iter().any(|entry| at(entry).is_none())
        {
            return Err(AdoptError::Failed("the harness supplies no time for some entries, so earlier history cannot be separated from work done after adoption; nothing was imported".to_owned()));
        }
        entries.retain(|entry| at(entry).is_none_or(|at| at.get() <= before));
        let turns = entries
            .iter()
            .map(|entry| match entry {
                RecordedEntry::Said(words) => words.turn,
                RecordedEntry::Item(item) => item.turn,
            })
            .collect::<std::collections::HashSet<_>>()
            .len();
        let mut tx = self.inner.db.begin().await?;
        // A revocation committed while this runs refuses its commit; no epoch row is held meanwhile.
        orch_db::queries::transaction_guard::lock_action(&mut tx).await?;
        self.may_retry_history(&mut tx, caller, thread).await?;
        adoption::lock_session(&mut tx, &adoption.native_session_ref).await?;
        let finished = {
            let mut unit = Unit::new(&mut tx, self.inner.clock.now());
            if orch_db::queries::threads::lock(&mut *unit.conn, thread)
                .await?
                .is_none()
            {
                return Err(AdoptError::Gone);
            }
            let current: Option<orch_types::Json> =
                sqlx::query_scalar("SELECT payload FROM items WHERE id=$1 AND thread_id=$2")
                    .bind(marker_id)
                    .bind(thread)
                    .fetch_optional(&mut *unit.conn)
                    .await
                    .map_err(orch_db::DbError::from)?;
            let Some(mut current) = current else {
                return Err(AdoptError::Gone);
            };
            if current["data"]["history_missing"].is_null() {
                return Ok(());
            }
            // Work may have started while the record was being read. A missing native boundary never
            // turns that work into read-only history from before Orch.
            let now_has_turns: bool =
                sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM turns WHERE segment_id=$1)")
                    .bind(segment)
                    .fetch_one(&mut *unit.conn)
                    .await
                    .map_err(orch_db::DbError::from)?;
            if now_has_turns && entries.iter().any(|entry| at(entry).is_none()) {
                return Err(AdoptError::Failed("the session ran while its history was read, and the harness supplies no time boundary; nothing was imported".to_owned()));
            }
            self.write_session_record(&mut unit, thread, segment, author, &entries)
                .await?;
            let total: i64 = sqlx::query_scalar(
                "SELECT count(*) FROM items WHERE thread_id=$1 AND segment_id=$2 AND before_orch",
            )
            .bind(thread)
            .bind(segment)
            .fetch_one(&mut *unit.conn)
            .await
            .map_err(orch_db::DbError::from)?;
            current["data"]["history_missing"] = serde_json::Value::Null;
            current["data"]["history_error"] = serde_json::Value::Null;
            current["data"]["turns"] = json!(turns);
            current["data"]["items"] = json!(total);
            let seq = unit.stamp(thread).await?;
            sqlx::query("UPDATE items SET payload=$2,seq_updated=$3,updated_at=$4 WHERE id=$1")
                .bind(marker_id)
                .bind(current)
                .bind(seq)
                .bind(unit.now())
                .execute(&mut *unit.conn)
                .await
                .map_err(orch_db::DbError::from)?;
            let mut report = serde_json::to_value(&read.report).map_err(EngineError::from)?;
            report["items_total"] = json!(total);
            adoption::set_adopted_report(&mut *unit.conn, segment, &report).await?;
            unit.finish().await?
        };
        self.commit(tx, finished).await?;
        Ok(())
    }

    /// The band's availability is read from current machine facts; checking it queues no native work.
    pub async fn adoption_history_status(
        &self,
        thread: Id,
    ) -> Result<Option<orch_api_contract::threads::AdoptHistoryStatus>, EngineError> {
        use orch_api_contract::threads::{AdoptHistoryState, AdoptHistoryStatus};
        let mut conn = self.inner.db.acquire().await?;
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
                let provider = resources::provider(&mut conn, provider)
                    .await?
                    .filter(|row| row.deleted_at.is_none());
                let environment = environments::environment(&mut conn, environment)
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
                    Some(machine) => match resources::machine_place(&mut conn, machine).await? {
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

    /// A failed read has a readable reason, and can never overwrite a successful concurrent import.
    async fn record_history_failure(
        &self,
        caller: &Caller,
        thread: Id,
        marker: Id,
        missing: AdoptHistoryMissing,
        reason: &str,
    ) -> Result<(), AdoptError> {
        let mut tx = self.inner.db.begin().await?;
        // A revocation committed while this runs refuses its commit; no epoch row is held meanwhile.
        orch_db::queries::transaction_guard::lock_action(&mut tx).await?;
        self.may_retry_history(&mut tx, caller, thread).await?;
        let finished = {
            let mut unit = Unit::new(&mut tx, self.inner.clock.now());
            if orch_db::queries::threads::lock(&mut *unit.conn, thread)
                .await?
                .is_none()
            {
                return Err(AdoptError::Gone);
            }
            let payload: Option<orch_types::Json> =
                sqlx::query_scalar("SELECT payload FROM items WHERE id=$1 AND thread_id=$2")
                    .bind(marker)
                    .bind(thread)
                    .fetch_optional(&mut *unit.conn)
                    .await
                    .map_err(orch_db::DbError::from)?;
            let Some(mut payload) = payload else {
                return Err(AdoptError::Gone);
            };
            if payload["data"]["history_missing"].is_null() {
                return Ok(());
            }
            payload["data"]["history_missing"] =
                serde_json::to_value(missing).map_err(EngineError::from)?;
            payload["data"]["history_error"] = json!(reason);
            let seq = unit.stamp(thread).await?;
            sqlx::query("UPDATE items SET payload=$2,seq_updated=$3,updated_at=$4 WHERE id=$1")
                .bind(marker)
                .bind(payload)
                .bind(seq)
                .bind(unit.now())
                .execute(&mut *unit.conn)
                .await
                .map_err(orch_db::DbError::from)?;
            unit.finish().await?
        };
        self.commit(tx, finished).await?;
        Ok(())
    }

    async fn may_retry_history(
        &self,
        conn: &mut Conn,
        caller: &Caller,
        thread: Id,
    ) -> Result<(), AdoptError> {
        let principal = Principal::load(conn, caller.clone(), Time::new(self.inner.clock.now()))
            .await
            .map_err(EngineError::from)?;
        match nodes::check(
            conn,
            &principal,
            thread,
            Permission::ThreadManage,
            Act::Use,
            Policy::default(),
        )
        .await
        .map_err(EngineError::from)?
        {
            Decision::Allowed { .. } => Ok(()),
            Decision::Refused(refusal) => Err(CreateThreadError::Denied(Box::new(refusal)).into()),
        }
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
