//! Side threads: a quick aside from a thread, set up as its parent is and starting from its history at any message; asked of the parent's page; how many may be open, closed by hand or when idle, and kept by an ordinary fork.
//!
//! A side thread is the fork with nothing to choose (`ForkRequest` needs no plan for it): one request
//! makes it, and it is a thread whose node lies in the thread it was taken from, so that everything
//! that is asked of it by its own identifier is asked of its parent's page. It runs on what the parent
//! runs on at that moment, locks nothing and copies no file; its session begins, at its first turn, from
//! the parent's through the turn it was taken at: forked by the harness where the adapter does that,
//! imported from the history as items otherwise (`Engine::side_session`).
//!
//! **How many.** At most `thread.side_max` are open for one thread, and none where `thread.side_threads` is
//! off. A closed one is deleted, and can be brought back until the days of the soft delete pass.
//!
//! **Closing.** By its maker or by whoever manages its parent or its environment (the request decides who
//! may); by [`Engine::close_idle_side_threads`] when no turn has been under way for `thread.side_idle_hours`;
//! and by a fork that keeps it. Each leaves one line in the parent, that says who, why and how many turns,
//! and never what was said. A parent that is archived or deleted takes the side threads that lie under it
//! along, as it takes everything under it, and leaves no line.

use crate::engine::Engine;
use crate::error::EngineError;
use crate::fork_history;
use crate::inputs::{InputError, InputRequest, Submitted};
use crate::sessions::SessionImport;
use crate::unit::Unit;
use orch_agent_contract::descriptor::SideReading;
use orch_api_contract::nodes::{NodeKind, NodeRef};
use orch_api_contract::threads::{ContentPart, Delivery, MarkerPayload, MarkerSideStarted};
use orch_authz::{Act, Caller, Principal, nodes};
use orch_db::queries::threads::{self, Binding, forking};
use orch_link_contract::commands::{SessionMode, SessionOpenHistory};
use orch_settings::{Place, get as setting};
use orch_settings_contract::registry::keys;
use orch_types::{Id, Time, new_id};

pub(crate) mod history;

/// What goes after a thread's name in the name of its first side thread.
const SIDE_SUFFIX: &str = " (side)";

/// How many side threads of the idle ones one look closes at most: the rest are closed by the next.
const IDLE_BATCH: i64 = 200;

/// Why a side thread could not be started.
#[derive(Debug, thiserror::Error)]
pub enum SideError {
    /// There is no such thread, or the caller may not see it.
    #[error("there is no such thread")]
    NotFound,
    /// The thread, or a place above it, is archived.
    #[error("the thread is archived")]
    Archived,
    /// A side thread has none of its own.
    #[error("a side thread has no side threads of its own")]
    Side,
    /// The thread is bound to nothing, so there is nothing for an aside to run on.
    #[error("the thread is bound to nothing yet")]
    Unbound,
    /// No further side thread can be opened: the limit, or the setting that switches them off.
    #[error("no further side thread can be opened")]
    Limit {
        /// The side threads that are open.
        open: Vec<Id>,
        /// How many may be.
        max: i64,
        /// Whether side threads are allowed here at all.
        allowed: bool,
    },
    /// The turn the request names is not one the thread completed.
    #[error("{0}")]
    Invalid(String),
    /// The thread's harness has verified no way for a side conversation to read without acting: an aside
    /// is refused rather than opened with the tools of an ordinary thread.
    #[error("{0}")]
    NoReading(String),
    /// The thread's harness can neither fork its conversation nor take in its history, so an aside would
    /// begin without the conversation it is about.
    #[error("the agent can neither fork its conversation nor take in its history")]
    NoHistory,
    /// The caller lacks something, and this is all of it.
    #[error("the sender lacks what an input needs")]
    Denied(Box<orch_access_contract::decisions::Refusal>),
    /// The first message could not be taken.
    #[error(transparent)]
    Input(#[from] InputError),
    /// Something the engine stands on failed.
    #[error(transparent)]
    Internal(#[from] EngineError),
}

impl From<orch_db::DbError> for SideError {
    fn from(error: orch_db::DbError) -> Self {
        Self::Internal(error.into())
    }
}

impl From<orch_authz::Error> for SideError {
    fn from(error: orch_authz::Error) -> Self {
        Self::Internal(error.into())
    }
}

impl From<orch_settings::Error> for SideError {
    fn from(error: orch_settings::Error) -> Self {
        Self::Internal(error.into())
    }
}

/// A side thread that was started.
#[derive(Debug, Clone, PartialEq)]
pub struct Started {
    /// The side thread.
    pub thread: Id,
    /// Its first input, as it stands.
    pub submitted: Submitted,
}

impl Engine {
    /// Starts a side thread of `parent`, with `content` queued in it as its first input, in one
    /// transaction: nothing is planned and nothing is chosen. The caller needs `thread.interact` on the
    /// parent and what an input needs.
    pub async fn start_side_thread(
        &self,
        caller: &Caller,
        parent_id: Id,
        content: Vec<ContentPart>,
        client_id: String,
        to_turn: Option<i64>,
    ) -> Result<Started, SideError> {
        let now = self.inner.clock.now();
        let mut tx = self.inner.db.begin().await?;
        let (finished, started) = {
            let mut unit = Unit::new(&mut tx, now);
            let started = self
                .start_side_in(&mut unit, caller, parent_id, content, client_id, to_turn)
                .await?;
            (unit.finish().await?, started)
        };
        self.commit(tx, finished).await?;
        self.wake(started.thread);
        Ok(started)
    }

    async fn start_side_in(
        &self,
        unit: &mut Unit<'_>,
        caller: &Caller,
        parent_id: Id,
        content: Vec<ContentPart>,
        client_id: String,
        to_turn: Option<i64>,
    ) -> Result<Started, SideError> {
        let now = unit.now();
        let person = caller.user();
        // The parent is looked at without its row locked first: the same request sent again is answered
        // by the side thread it made, and must not wait behind the parent for it. The turn of that side
        // thread may be starting at that moment, and the two would hold one row each and wait for the
        // other's.
        if threads::thread(&mut *unit.conn, parent_id)
            .await?
            .is_none_or(|row| row.deleted_at.is_some())
        {
            return Err(SideError::NotFound);
        }
        let principal = Principal::load(&mut *unit.conn, caller.clone(), Time::new(now)).await?;
        match nodes::check(
            &mut *unit.conn,
            &principal,
            parent_id,
            orch_access_contract::permissions::Permission::ThreadInteract,
            Act::Use,
            orch_authz::Policy::default(),
        )
        .await
        {
            Ok(orch_access_contract::decisions::Decision::Allowed { .. }) => {}
            Ok(orch_access_contract::decisions::Decision::Refused(refusal)) => {
                // Someone who cannot even see the thread is told there is none.
                return match nodes::check(
                    &mut *unit.conn,
                    &principal,
                    parent_id,
                    orch_access_contract::permissions::Permission::ThreadView,
                    Act::Use,
                    orch_authz::Policy::default(),
                )
                .await
                {
                    Ok(orch_access_contract::decisions::Decision::Allowed { .. }) => {
                        Err(SideError::Denied(Box::new(refusal)))
                    }
                    _ => Err(SideError::NotFound),
                };
            }
            Err(orch_authz::Error::Archived { .. }) => return Err(SideError::Archived),
            Err(orch_authz::Error::NotFound(_)) => return Err(SideError::NotFound),
            Err(other) => return Err(other.into()),
        }
        if let Some(made) =
            forking::side_thread_started_with(&mut *unit.conn, parent_id, person, &client_id)
                .await?
        {
            return self
                .first_message(unit, caller, made, content, client_id)
                .await;
        }
        // Making one changes what the parent has of them: one request at a time. A request with the same
        // value that was waiting meanwhile finds what the first made.
        let parent = threads::lock(&mut *unit.conn, parent_id)
            .await?
            .filter(|row| row.deleted_at.is_none())
            .ok_or(SideError::NotFound)?;
        if orch_db::queries::agent_history::removing(unit.conn, parent_id).await? {
            return Err(SideError::Invalid(
                "Its agent history is being removed; wait until that operation finishes.".into(),
            ));
        }
        if let Some(made) =
            forking::side_thread_started_with(&mut *unit.conn, parent_id, person, &client_id)
                .await?
        {
            return self
                .first_message(unit, caller, made, content, client_id)
                .await;
        }
        if parent.archived_at.is_some() {
            return Err(SideError::Archived);
        }
        if parent.side_of.is_some() {
            return Err(SideError::Side);
        }
        let (Some(agent_id), Some(provider_id), Some(environment_id)) =
            (parent.agent_id, parent.provider_id, parent.environment_id)
        else {
            return Err(SideError::Unbound);
        };
        let allowed = setting::<bool>(
            &mut *unit.conn,
            Place::Node(parent_id),
            keys::THREAD_SIDE_THREADS,
        )
        .await?
        .unwrap_or(true);
        let max = setting::<i64>(
            &mut *unit.conn,
            Place::Node(parent_id),
            keys::THREAD_SIDE_MAX,
        )
        .await?
        .unwrap_or(DEFAULT_SIDE_MAX);
        let open = forking::side_threads_of(&mut *unit.conn, parent_id, false).await?;
        if !allowed || i64::try_from(open.len()).unwrap_or(i64::MAX) >= max {
            return Err(SideError::Limit { open, max, allowed });
        }
        // An aside reads and never acts: by the first mechanism its harness verified, or not at all.
        let (reading, takes_history) = match aside_of(&mut *unit.conn, agent_id).await? {
            Ok(aside) => aside,
            Err(why) => return Err(SideError::NoReading(why)),
        };
        let last = orch_db::queries::thread_history_cuts::last_completed(
            unit.conn,
            parent_id,
            parent.active_continuation_id,
        )
        .await?;
        let from_turn = match (to_turn, last) {
            (Some(turn), Some(last)) if (1..=i64::from(last)).contains(&turn) => {
                i32::try_from(turn).unwrap_or(last)
            }
            (Some(turn), _) => {
                return Err(SideError::Invalid(format!(
                    "the thread has not completed a turn {turn} to start from"
                )));
            }
            (None, Some(last)) => last,
            (None, None) => 0,
        };
        // An aside about a conversation begins with it: a harness that can neither fork nor take it in would
        // start one without it, which is not what was asked for.
        let has_before = orch_db::queries::thread_history_cuts::has_before_items(
            unit.conn,
            parent_id,
            parent.active_continuation_id,
        )
        .await?;
        if orch_db::queries::thread_history_cuts::unconfirmed_inputs(
            unit.conn,
            parent_id,
            parent.active_continuation_id,
            Some((1, from_turn)),
        )
        .await?
        {
            return Err(SideError::Invalid("This history contains an unconfirmed earlier input delivery; its queued words cannot be copied into a new side conversation.".into()));
        }
        if (from_turn > 0 || has_before) && !takes_history {
            return Err(SideError::NoHistory);
        }
        let names = forking::side_thread_names(&mut *unit.conn, parent_id).await?;
        let name = if names.is_empty() {
            format!("{}{SIDE_SUFFIX}", parent.name)
        } else {
            format!("{}{SIDE_SUFFIX} {}", parent.name, names.len() + 1)
        };
        let side = new_id();
        if !forking::insert_side_thread(
            &mut *unit.conn,
            &forking::NewSide {
                id: side,
                parent: parent_id,
                name: &name,
                owner: person,
                from_turn,
                at: now,
            },
        )
        .await?
        {
            return Err(SideError::NotFound);
        }
        orch_db::queries::hierarchy::stamp(&mut *unit.conn, side).await?;
        threads::set_side_reading(&mut *unit.conn, side, reading_word(reading)).await?;
        let seq = unit.stamp(side).await?;
        threads::set_binding(
            &mut *unit.conn,
            side,
            &Binding {
                agent: agent_id,
                provider: provider_id,
                arrangement: parent.arrangement.as_deref().unwrap_or_default(),
                executor: parent.executor.as_deref(),
                tool_set: parent.tool_set.as_deref(),
            },
            seq,
        )
        .await?;
        threads::set_environment(
            &mut *unit.conn,
            side,
            environment_id,
            &parent.workspace_dir,
            seq,
        )
        .await?;
        // Its first segment lies on what its parent's lies on; its session begins at its first turn.
        let parent_segment = match parent.current_segment_id {
            Some(segment) => threads::segment(&mut *unit.conn, segment).await?,
            None => None,
        };
        let segment_id = new_id();
        let segment = threads::insert_segment(
            &mut *unit.conn,
            &threads::NewSegment {
                id: segment_id,
                thread_id: side,
                agent_id,
                provider_id,
                environment_id,
                env_generation: parent_segment
                    .as_ref()
                    .map_or(1, |segment| segment.env_generation),
                arrangement: parent.arrangement.as_deref().unwrap_or_default(),
                executor: parent.executor.as_deref(),
                tool_set: parent.tool_set.as_deref(),
                credential_path: parent_segment
                    .as_ref()
                    .map_or("native", |segment| segment.credential_path.as_str()),
                native_session_ref: None,
                started_by: Some(person),
                started_at: now,
            },
            seq,
        )
        .await?;
        threads::set_current(&mut *unit.conn, side, Some(segment.id), None, seq).await?;
        // The cut precedes both the parent marker and the side's first authored
        // input. Future parent selection or mutable item payloads cannot replace it.
        history::capture(unit, &parent, side, from_turn, caller).await?;
        // The line in the parent.
        if let Some(segment) = parent.current_segment_id {
            let by: orch_api_contract::users::UserRef =
                serde_json::from_value(crate::views::user_ref_json(&mut *unit.conn, person).await?)
                    .map_err(EngineError::from)?;
            let marker = MarkerPayload::SideStarted(MarkerSideStarted {
                side: NodeRef {
                    thread_display: None,
                    id: side,
                    kind: NodeKind::Thread,
                    name: Some(name.clone()),
                    redacted: false,
                },
                from_turn: i64::from(from_turn),
                by,
            });
            crate::ingest::add_marker(
                unit,
                parent_id,
                segment,
                None,
                &serde_json::to_value(&marker).map_err(EngineError::from)?,
                &format!("side_started:{side}"),
            )
            .await?;
        }
        self.first_message(unit, caller, side, content, client_id)
            .await
    }

    /// The first message of a side thread, queued in it, or as it stands where the request that made the
    /// thread is made again.
    async fn first_message(
        &self,
        unit: &mut Unit<'_>,
        caller: &Caller,
        side: Id,
        content: Vec<ContentPart>,
        client_id: String,
    ) -> Result<Started, SideError> {
        let taken = self
            .submit_input_in(
                unit,
                caller,
                side,
                &InputRequest {
                    plan: false,
                    content,
                    delivery: Delivery::Queue,
                    client_id,
                    bypass: None,
                },
            )
            .await;
        match taken {
            Ok(taken) => {
                let value = serde_json::json!({"input_id":taken.submitted.input.id});
                if !orch_db::queries::thread_history_cuts::keep_artifact(
                    unit.conn,
                    side,
                    "side_initial_input",
                    &value,
                    &[],
                )
                .await?
                {
                    return Err(EngineError::unreadable(
                        "The retained side's first input identity changed.",
                    )
                    .into());
                }
                Ok(Started {
                    thread: side,
                    submitted: taken.submitted,
                })
            }
            Err(InputError::Denied(refusal)) => Err(SideError::Denied(refusal)),
            Err(other) => Err(other.into()),
        }
    }

    /// The side threads of a thread, oldest first: those that are open, or those that were closed and are
    /// not purged yet.
    pub async fn side_threads(&self, parent: Id, closed: bool) -> Result<Vec<Id>, EngineError> {
        let mut conn = self.inner.db.acquire().await?;
        Ok(forking::side_threads_of(&mut conn, parent, closed).await?)
    }

    /// Closes a side thread, for `why`, by `by` where somebody did; into the thread `kept_as` where an
    /// ordinary fork kept it. A turn that is under way is stopped first. The answer says whether this call
    /// closed it.
    pub async fn close_side_thread(
        &self,
        side: Id,
        why: &str,
        by: Option<Id>,
        kept_as: Option<&forking::KeptAs>,
    ) -> Result<bool, EngineError> {
        let mut tx = self.inner.db.begin().await?;
        orch_db::queries::transaction_guard::lock_authority(&mut tx).await?;
        let now = Time::new(self.inner.clock.now());
        self.close_side_thread_in(tx, side, why, by, kept_as, now)
            .await
    }

    /// Closes a side thread in the caller's guarded transaction, retaining its authority locks through commit.
    pub async fn close_side_thread_in(
        &self,
        mut tx: orch_db::Tx,
        side: Id,
        why: &str,
        by: Option<Id>,
        kept_as: Option<&forking::KeptAs>,
        now: Time,
    ) -> Result<bool, EngineError> {
        self.settle_side_in(&mut tx, side, by, now).await?;
        let closed = forking::close_with_marker(&mut tx, side, why, by, kept_as, now.get()).await?;
        tx.commit().await.map_err(orch_db::DbError::from)?;
        let Some(closed) = closed else {
            return Ok(false);
        };
        self.wake(closed.parent);
        self.kick();
        Ok(true)
    }

    /// Closes the side threads that no turn has been under way in for as long as their place allows
    /// (`thread.side_idle_hours`), as closing one by hand does. Returns how many were closed.
    pub async fn close_idle_side_threads(&self) -> Result<usize, EngineError> {
        let now = self.inner.clock.now();
        let idle = {
            let mut conn = self.inner.db.acquire().await?;
            forking::idle_side_threads(&mut conn, IDLE_BATCH).await?
        };
        let mut closed = 0;
        for side in idle {
            let hours = {
                let mut conn = self.inner.db.acquire().await?;
                setting::<i64>(
                    &mut conn,
                    Place::Node(side.parent),
                    keys::THREAD_SIDE_IDLE_HOURS,
                )
                .await?
                .unwrap_or(DEFAULT_IDLE_HOURS)
            };
            if side.idle_since + chrono::Duration::hours(hours) <= now
                && self
                    .close_side_thread(side.thread, "idle", None, None)
                    .await?
            {
                closed += 1;
            }
        }
        Ok(closed)
    }

    /// How the session of a side thread begins, at its first turn: forked by the harness from its
    /// parent's through the turn it was taken at, where the adapter does that; the history as items,
    /// otherwise; nothing, for an aside that was taken before the parent had said anything. A side thread
    /// whose session exists already, and any thread that is no side thread, is left to the ordinary way.
    pub(crate) async fn side_session(
        &self,
        snap: &crate::sessions::StartSnapshot,
    ) -> Result<Option<(SessionMode, SessionImport)>, EngineError> {
        let Some(_) = snap.thread.side_of else {
            return Ok(None);
        };
        if snap.segment.native_session_ref.is_some()
            && !history::needs_verification(self, snap).await?
        {
            return Ok(None);
        }
        let (cut, actual) = {
            let mut conn = self.inner.db.acquire().await?;
            let empty:bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM thread_native_guards WHERE thread_id=$1 AND empty_segment_id=$2 AND ready)")
                .bind(snap.thread.node_id).bind(snap.segment.id).fetch_one(&mut *conn).await.map_err(orch_db::DbError::from)?;
            if empty {
                return Ok(None);
            }
            let cut = orch_db::queries::thread_history_cuts::cut(&mut conn, snap.thread.node_id)
                .await?
                .filter(|cut| {
                    cut.owner_thread_id == Some(snap.thread.node_id)
                        && Some(cut.thread_id) == snap.thread.side_of
                })
                .ok_or_else(|| {
                    EngineError::unreadable("This side has no retained original preparation.")
                })?;
            let actual = threads::segment(&mut conn, snap.segment.id)
                .await?
                .ok_or_else(|| {
                    EngineError::unreadable("The side's actual segment is unavailable.")
                })?;
            (cut, actual)
        };
        // Retaining an already admitted native outcome is factual housekeeping.
        // Current/frozen creator rights still gate verification and delivery.
        let recovered = history::recovered(self, snap, &cut).await?;
        let cut = {
            let mut conn = self.db().acquire().await?;
            history::authorize(self, &mut conn, &snap.thread, self.inner.clock.now()).await?
        };
        if let Some(native) = recovered {
            return Ok(Some((
                SessionMode::Resume,
                SessionImport {
                    native_session_ref: Some(native),
                    ..SessionImport::default()
                },
            )));
        }
        if actual.native_session_ref.is_some() {
            return Err(EngineError::unreadable(
                "An observed side session has no retained successful original creation; another child or input is not admitted.",
            ));
        }
        let counted = {
            let mut conn = self.db().acquire().await?;
            orch_db::queries::thread_history_cuts::counts(&mut conn, cut.id).await?
        };
        if counted.turns == 0 && counted.items.is_empty() {
            return Ok(Some((SessionMode::New, SessionImport::default())));
        }
        let capabilities = &snap.descriptor.capabilities;
        // Only the original segment's binding identifies the native source home.
        // A side rebound elsewhere may import its frozen text, never query that
        // other profile/provider's home using the original native identifier.
        let compatible = {
            let mut conn = self.db().acquire().await?;
            match cut.source_segment_id {
                Some(id) => threads::segment(&mut conn, id)
                    .await?
                    .is_some_and(|segment| {
                        segment.agent_id == snap.agent.id && segment.provider_id == snap.provider.id
                    }),
                None => false,
            }
        };
        if compatible
            && capabilities.fork_native
            && cut.artifacts.get("side_fork_unsupported").is_none()
            && let Some(boundary) = fork_history::cut::native_boundary(
                self,
                &cut,
                snap.key.machine,
                snap.provider.id,
                snap.agent.id,
                crate::ids::derived("side.native-record", &[cut.id.as_bytes()]),
            )
            .await?
        {
            return Ok(Some((
                SessionMode::Fork,
                SessionImport {
                    native_session_ref: Some(boundary.native_ref),
                    upto_turn_ordinal: Some(boundary.ordinal),
                    ..SessionImport::default()
                },
            )));
        }
        if !capabilities.import_neutral {
            return Err(EngineError::unreadable(
                "This side's confirmed history cannot be prepared exactly by the selected native adapter; an empty session is not substituted.",
            ));
        }
        let archive = if let Some(old) = cut.artifacts.get("side_neutral") {
            old.clone()
        } else {
            // The newest turns that fit the share of the new model's window, read from the end of the cut a page at a
            // time. A side that may use no tools needs the whole of it told exactly, or none.
            let budget = {
                let mut conn = self.db().acquire().await?;
                fork_history::carry_budget(
                    &mut conn,
                    snap.thread.node_id,
                    snap.model
                        .as_ref()
                        .and_then(|model| fork_history::model_window(&model.parameters)),
                )
                .await?
            }
            .unwrap_or(fork_history::UNKNOWN_WINDOW_NEUTRAL_BYTES);
            let newest = {
                let mut conn = self.db().acquire().await?;
                fork_history::newest(
                    &mut conn,
                    crate::history_pages::Source::Cut(&cut),
                    (i64::from(cut.first_turn.max(1)), i64::from(cut.last_turn)),
                    i64::from(cut.last_turn),
                    budget,
                )
                .await?
            };
            if !newest.complete && snap.thread.side_reading.as_deref() == Some("no_tools") {
                return Err(EngineError::unreadable(
                    "This side's confirmed history does not fit the share of the new model's window, and a side that uses no tools needs all of it told exactly.",
                ));
            }
            let (blob, size) = fork_history::cut::keep_bytes(
                self,
                cut.id,
                "side_neutral",
                fork_history::neutral_lines(&newest.selected).into_bytes(),
            )
            .await?;
            serde_json::json!({"blob":blob,"size":size})
        };
        Ok(Some((
            SessionMode::ImportNeutral,
            SessionImport {
                history: Some(SessionOpenHistory {
                    blob: serde_json::from_value(archive["blob"].clone())?,
                    size: archive["size"].as_i64().ok_or_else(|| {
                        EngineError::unreadable("The retained side archive size is invalid.")
                    })?,
                }),
                ..SessionImport::default()
            },
        )))
    }
}

/// How many side threads may be open for one thread when the setting is not there: what it ships with.
const DEFAULT_SIDE_MAX: i64 = 4;

/// How an aside of a thread bound to the profile `agent` reads without acting, with whether its harness can
/// begin it from the parent's conversation (forked natively, or the history taken in); or, in words a person
/// reads, why no aside can be had.
pub async fn aside_of(
    conn: &mut orch_db::Conn,
    agent: Id,
) -> Result<Result<(SideReading, bool), String>, EngineError> {
    let Some(row) = orch_db::queries::resources::thread_engine::agent(conn, agent)
        .await?
        .filter(|row| row.deleted_at.is_none())
    else {
        return Ok(Err(
            "This thread’s agent profile is no longer available.".to_owned()
        ));
    };
    let descriptor = match crate::harnesses::descriptor_of(&row) {
        Ok(descriptor) => descriptor,
        // A persisted invalid profile cannot provide a verified read-only
        // capability. It must not hide its parent thread's authoritative state.
        Err(_) => {
            return Ok(Err(
                "This agent profile's overrides do not match its harness declaration.".to_owned(),
            ));
        }
    };
    let Some(descriptor) = descriptor else {
        return Ok(Err(
            "This agent has not reported whether it can answer without changing things yet."
                .to_owned(),
        ));
    };
    let takes_history =
        descriptor.capabilities.fork_native || descriptor.capabilities.import_neutral;
    Ok(SideReading::first_of(&descriptor.side_thread_modes)
        .map(|reading| (reading, takes_history))
        .ok_or_else(|| "This agent cannot answer without being able to change things.".to_owned()))
}

/// The word a side thread's row keeps for how it reads.
#[must_use]
pub const fn reading_word(reading: SideReading) -> &'static str {
    match reading {
        SideReading::ReadOnly => "read_only",
        SideReading::ApprovalGated => "approval_gated",
        SideReading::NoTools => "no_tools",
    }
}

/// How many hours a side thread may stand idle when the setting is not there: what it ships with.
const DEFAULT_IDLE_HOURS: i64 = 24;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_first_side_thread_is_named_after_its_parent_and_the_others_are_numbered() {
        let name = |parent: &str, existing: usize| {
            if existing == 0 {
                format!("{parent}{SIDE_SUFFIX}")
            } else {
                format!("{parent}{SIDE_SUFFIX} {}", existing + 1)
            }
        };
        assert_eq!(name("Plan", 0), "Plan (side)");
        assert_eq!(name("Plan", 1), "Plan (side) 2");
        assert_eq!(name("Plan", 2), "Plan (side) 3");
    }
}
