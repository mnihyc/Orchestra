//! Volatile native command observations, caller-bound reviews and captured End targets.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use chrono::{DateTime, Duration, Utc};
use orch_access_contract::decisions::{Decision, ObjectRef};
use orch_access_contract::permissions::{ObjectKind, Permission};
use orch_api_contract::background::{
    BackgroundCommand, BackgroundEndRequest, BackgroundPlace, BackgroundState, ThreadBackground,
};
use orch_authz::{Act, Caller, Policy, Principal, nodes, resources};
use orch_db::queries::{jobs, threads};
use orch_db::{Conn, DbError, Tx};
use orch_link_contract::background::{BackgroundReadLimits, NativeBackground, PAGE_BYTES};
use orch_link_contract::commands::{SerialKey, SessionBackground};
use orch_types::{Id, Time, new_id};
use serde::{Deserialize, Serialize};
use serde_json::json;

mod projection;
mod reading;
pub use projection::PreparedProjection;
pub use reading::{read_scope, read_scope_for_capture};

use crate::commands::{Outcome, Outgoing};
use crate::engine::{Engine, lock};
use crate::error::EngineError;

/// Native reads coalesce within one change interval, rather than sampling for every viewer.
const READ_GAP: Duration = Duration::seconds(1);
/// Ordinary roster pages contain fifty commands.
const PAGE_DEFAULT: usize = 50;
/// Selected End and each page carry at most two hundred command identities.
const PAGE_MAX: usize = 200;
/// Bounded reviews prevent many abandoned pages from retaining unbounded process data.
const REVIEW_MAX: usize = 256;

/// The original credential whose current authority must still hold at each End.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Identity {
    /// The original session or API-token identifier, never bearer bytes.
    pub credential_id: Id,
    /// Whether that identifier names an API token.
    pub token: bool,
}

/// Immutable source identity of one retained session observation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, sqlx::FromRow)]
pub struct Source {
    /// Its exact recorded segment.
    pub segment_id: Id,
    /// Where this segment's harness was opened, not the environment's current machine.
    pub machine_id: Option<Id>,
    /// Its recorded provider, for directory authority.
    pub provider_id: Id,
    /// Its recorded environment, for redaction and directory cuts.
    pub environment_id: Id,
    /// The native generation observed when this segment was opened.
    pub native_session_ref: String,
}

/// An internal volatile snapshot shared by the watch coordinator before per-reader projection.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Capture {
    /// When its bounded native reads ended.
    pub at: Time,
    /// Exact sources and their facts; nothing here is stored in a table or a job.
    pub sessions: Vec<(Source, NativeBackground)>,
}

/// The identities accepted for a mutation, without command text, directories or load.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Target {
    /// The opaque API command identity the person reviewed.
    pub id: String,
    /// The captured segment.
    pub segment_id: Id,
    /// The recorded native source machine.
    pub machine_id: Id,
    /// The captured native session reference, protecting resumed/replaced sessions.
    pub native_session_ref: String,
    /// The adapter's exact opaque command identity.
    pub command_id: String,
    /// The recorded source turn, when known.
    pub turn_id: Option<Id>,
    /// The recorded source item, when known.
    pub item_id: Option<Id>,
}

/// A bounded native page selection, kept outside effect-free projection retries.
#[derive(Debug, Clone)]
pub struct PreparedPage {
    caller: Caller,
    identity: Identity,
    thread: Id,
    reference: String,
    capture: Capture,
    offset: usize,
    limit: usize,
    idle: Duration,
}

impl PreparedPage {
    /// The exact recorded native sources of this page.
    pub fn capture(&self) -> &Capture {
        &self.capture
    }
}

struct Review {
    caller: Caller,
    identity: Identity,
    thread: Id,
    until: DateTime<Utc>,
    capture: Capture,
    targets: Vec<Target>,
    total: Option<i64>,
}

/// Live state is forgotten at restart and never participates in thread sync or replay.
#[derive(Default)]
pub struct Memory {
    reads: std::sync::Mutex<HashMap<Id, Arc<tokio::sync::Mutex<Option<Capture>>>>>,
    reviews: std::sync::Mutex<HashMap<String, Review>>,
}

/// A roster or End refusal that the person can act on.
#[derive(Debug, thiserror::Error)]
pub enum BackgroundError {
    /// This read is outside the credential's current operation or source bounds.
    #[error("the key does not allow this background read")]
    Scope,
    /// The thread does not exist.
    #[error("the thread does not exist")]
    NotFound,
    /// Current rights refuse this operation.
    #[error("the caller lacks the thread permission")]
    Denied(Box<Decision>),
    /// The reviewed native generation, count or idle deadline changed.
    #[error("the reviewed command list is stale; read the current list again")]
    Stale,
    /// The exact selected set is not a valid mutation request.
    #[error("{0}")]
    Invalid(&'static str),
    /// A dependency failed.
    #[error(transparent)]
    Internal(#[from] EngineError),
}

impl From<DbError> for BackgroundError {
    fn from(error: DbError) -> Self {
        Self::Internal(error.into())
    }
}
impl From<sqlx::Error> for BackgroundError {
    fn from(error: sqlx::Error) -> Self {
        Self::Internal(DbError::from(error).into())
    }
}
impl From<orch_authz::Error> for BackgroundError {
    fn from(error: orch_authz::Error) -> Self {
        Self::Internal(error.into())
    }
}
impl From<serde_json::Error> for BackgroundError {
    fn from(error: serde_json::Error) -> Self {
        Self::Internal(error.into())
    }
}

async fn sources(conn: &mut Conn, thread: Id) -> Result<Vec<Source>, EngineError> {
    Ok(sqlx::query_as(
        "SELECT id AS segment_id, machine_id, provider_id, environment_id, native_session_ref
        FROM segments WHERE thread_id=$1 AND native_session_ref IS NOT NULL ORDER BY ordinal",
    )
    .bind(thread)
    .fetch_all(conn)
    .await
    .map_err(DbError::from)?)
}

fn permitted(decision: Decision) -> Result<(), BackgroundError> {
    if decision.is_allowed() {
        Ok(())
    } else {
        Err(BackgroundError::Denied(Box::new(decision)))
    }
}

impl Engine {
    async fn background_read_limits(
        &self,
        thread: Id,
    ) -> Result<BackgroundReadLimits, EngineError> {
        let mut conn = self.db().acquire().await?;
        let place = orch_settings::Place::Node(thread);
        use orch_settings_contract::registry::keys;
        Ok(BackgroundReadLimits {
            idle_seconds: orch_settings::get(
                &mut conn,
                place,
                keys::THREADS_BACKGROUND_SNAPSHOT_SECONDS,
            )
            .await?
            .unwrap_or(30),
            page_items: orch_settings::get(&mut conn, place, keys::THREADS_BACKGROUND_PAGE_ITEMS)
                .await?
                .unwrap_or(200),
            page_bytes: orch_settings::get(&mut conn, place, keys::THREADS_BACKGROUND_PAGE_BYTES)
                .await?
                .unwrap_or(131_072),
            preview_bytes: orch_settings::get(
                &mut conn,
                place,
                keys::THREADS_BACKGROUND_PREVIEW_BYTES,
            )
            .await?
            .unwrap_or(4_096),
        })
    }

    /// Reads the thread's existing native sessions without opening any process, session, turn or lease.
    pub async fn capture_background(&self, thread: Id) -> Result<Capture, EngineError> {
        let read = {
            let mut reads = lock(&self.inner.background.reads);
            // Dead thread caches have no durable purpose and should not survive indefinitely in memory.
            if reads.len() >= REVIEW_MAX {
                reads.retain(|_, read| Arc::strong_count(read) > 1);
            }
            Arc::clone(reads.entry(thread).or_default())
        };
        let mut current = read.lock().await;
        let now = self.inner.clock.now();
        if let Some(capture) = current
            .as_ref()
            .filter(|capture| now - capture.at.get() < READ_GAP)
        {
            return Ok(capture.clone());
        }
        let all = {
            let mut conn = self.inner.db.acquire().await?;
            sources(&mut conn, thread).await?
        };
        let mut sessions = Vec::new();
        for source in all {
            let observation = if let Some(machine) = source.machine_id {
                self.read_background_session(thread, machine, source.segment_id)
                    .await?
            } else {
                NativeBackground::unsupported("this segment has no recorded source machine")
            };
            sessions.push((source, observation));
        }
        let capture = Capture {
            at: self.inner.clock.time(),
            sessions,
        };
        *current = Some(capture.clone());
        Ok(capture)
    }

    async fn read_background_session(
        &self,
        thread: Id,
        machine: Id,
        segment: Id,
    ) -> Result<NativeBackground, EngineError> {
        let limits = self.background_read_limits(thread).await?;
        let mut cursor = None;
        let mut seen = HashSet::new();
        let mut combined: Option<NativeBackground> = None;
        loop {
            let command = Outgoing::new(
                new_id(),
                machine,
                SerialKey::Thread(thread),
                &SessionBackground {
                    thread_id: thread,
                    segment_id: segment,
                    cursor: cursor.clone(),
                    limits,
                },
            )?;
            let mut page: NativeBackground = match self.inner.commands.observe(command).await {
                Ok(Outcome::Succeeded(value)) => {
                    serde_json::from_value(value).unwrap_or_else(|_| {
                        NativeBackground::unsupported("the native reply could not be read")
                    })
                }
                Ok(Outcome::Failed(error))
                    if cursor.is_some()
                        && error.code
                            == orch_link_contract::commands::ErrorCode::Conflict.as_str() =>
                {
                    cursor = None;
                    seen.clear();
                    combined = None;
                    continue;
                }
                _ => {
                    return Ok(NativeBackground {
                        supported: true,
                        as_of: None,
                        state: orch_link_contract::background::BackgroundState::Offline,
                        reason: Some("this retained native session cannot be reached".to_owned()),
                        items: Vec::new(),
                        next_cursor: None,
                        total: None,
                    });
                }
            };
            let next = page.next_cursor.take();
            match &mut combined {
                Some(combined) => combined.items.append(&mut page.items),
                None => combined = Some(page),
            };
            let snapshot = combined
                .as_mut()
                .ok_or_else(|| EngineError::unreadable("no native snapshot was read"))?;
            let Some(next) = next else { break };
            if !seen.insert(next.clone()) {
                snapshot.total = None;
                snapshot.reason = Some("the native list repeated its page; refresh it".to_owned());
                break;
            }
            cursor = Some(next);
        }
        combined.ok_or_else(|| EngineError::unreadable("no native snapshot was read"))
    }

    /// Native convenience projection. Transports use the explicit actual-reader render handoff.
    pub async fn project_background(
        &self,
        conn: &mut Conn,
        principal: &Principal,
        thread: Id,
        capture: &Capture,
    ) -> Result<(ThreadBackground, Vec<Target>), BackgroundError> {
        let prepared = self
            .prepare_background_projection(conn, thread, capture)
            .await?;
        let caller = if principal.caller().grant().is_some() {
            // A native key caller retains its existing accepted/current key semantics.
            // Session context is supplied by the native caller, never reconstructed here.
            orch_authz::CapturedCaller::capture(principal.caller())
                .restore_current(conn, self.inner.clock.as_ref())
                .await?
                .ok_or(BackgroundError::Scope)?
        } else {
            principal.caller().clone()
        };
        let (scope, directories) = read_scope_for_capture(conn, &caller, thread, capture).await?;
        let admitted = scope.admit(&caller, true).map_err(reading::binding)?;
        let reader = if caller.grant().is_some() {
            let reader = Principal::load(conn, admitted, self.inner.clock.time()).await?;
            match principal.second_step_hold() {
                Some(hold) => reader.held_for_second_step(hold.clone()),
                None => reader,
            }
        } else {
            principal.clone()
        };
        let result = self
            .render_background_projection(conn, &reader, &prepared, &directories)
            .await;
        if !reader
            .caller()
            .key_deadline_allows(self.inner.clock.as_ref())
        {
            return Err(BackgroundError::Scope);
        }
        result
    }

    async fn project_background_for(
        &self,
        conn: &mut Conn,
        principal: &Principal,
        thread: Id,
        capture: &Capture,
        provider_directories: &HashSet<Id>,
    ) -> Result<(ThreadBackground, Vec<Target>), BackgroundError> {
        let prepared = self
            .prepare_background_projection(conn, thread, capture)
            .await?;
        self.render_background_projection(conn, principal, &prepared, provider_directories)
            .await
    }

    /// Reads or continues a native caller's roster. Transports use the explicit current-reader handoff.
    pub async fn background(
        &self,
        caller: &Caller,
        identity: Identity,
        thread: Id,
        cursor: Option<&str>,
        limit: Option<u32>,
    ) -> Result<ThreadBackground, BackgroundError> {
        let page = self
            .prepare_background_page(caller, identity, thread, cursor, limit)
            .await?;
        let mut conn = self.inner.db.acquire().await?;
        let principal = Principal::load(&mut conn, caller.clone(), self.inner.clock.time()).await?;
        let (roster, targets) = self
            .project_background(&mut conn, &principal, thread, page.capture())
            .await?;
        self.finish_background_page(page, roster, targets)
    }

    /// Capture or retrieve the original reviewed native observation, without a publication reader.
    pub async fn prepare_background_page(
        &self,
        caller: &Caller,
        identity: Identity,
        thread: Id,
        cursor: Option<&str>,
        limit: Option<u32>,
    ) -> Result<PreparedPage, BackgroundError> {
        let limit = usize::try_from(limit.unwrap_or(PAGE_DEFAULT as u32)).unwrap_or(PAGE_MAX);
        if limit == 0 || limit > PAGE_MAX {
            return Err(BackgroundError::Invalid(
                "a roster page contains one to two hundred commands",
            ));
        }
        let now = self.inner.clock.now();
        let (reference, capture, offset) = if let Some(cursor) = cursor {
            let (reference, offset) = cursor.rsplit_once('/').ok_or(BackgroundError::Stale)?;
            let offset: usize = offset.parse().map_err(|_| BackgroundError::Stale)?;
            let capture = {
                let reviews = lock(&self.inner.background.reviews);
                let review = reviews.get(reference).ok_or(BackgroundError::Stale)?;
                if review.caller != *caller
                    || review.identity != identity
                    || review.thread != thread
                    || review.until <= now
                {
                    return Err(BackgroundError::Stale);
                }
                review.capture.clone()
            };
            (reference.to_owned(), capture, offset)
        } else {
            (
                new_id().to_string(),
                self.capture_background(thread).await?,
                0,
            )
        };
        Ok(PreparedPage {
            caller: caller.clone(),
            identity,
            thread,
            reference,
            capture,
            offset,
            limit,
            idle: Duration::try_seconds(
                i64::try_from(self.background_read_limits(thread).await?.idle_seconds)
                    .unwrap_or(i64::MAX),
            )
            .unwrap_or(Duration::MAX),
        })
    }

    /// Register a review only after its page has passed the transport's final current-reader cut.
    pub fn finish_background_page(
        &self,
        page: PreparedPage,
        mut roster: ThreadBackground,
        targets: Vec<Target>,
    ) -> Result<ThreadBackground, BackgroundError> {
        let PreparedPage {
            caller,
            identity,
            thread,
            reference,
            capture,
            offset,
            limit,
            idle,
        } = page;
        let now = self.inner.clock.now();
        if offset > roster.items.len() {
            return Err(BackgroundError::Stale);
        }
        let full = std::mem::take(&mut roster.items);
        let mut end = offset;
        while end < full.len() && end - offset < limit {
            roster.items.push(full[end].clone());
            if serde_json::to_vec(&roster)?.len() > PAGE_BYTES / 2 && roster.items.len() > 1 {
                roster.items.pop();
                break;
            }
            end += 1;
        }
        if end < full.len() {
            roster.next_cursor = Some(format!("{reference}/{end}"));
        }
        let until = self
            .inner
            .clock
            .now()
            .checked_add_signed(idle)
            .unwrap_or(DateTime::<Utc>::MAX_UTC);
        roster.list_ref = Some(reference.clone());
        roster.expires_at = Some(Time::new(until));
        if serde_json::to_vec(&roster)?.len() > PAGE_BYTES {
            return Err(BackgroundError::Invalid(
                "the native facts exceed the roster page bound",
            ));
        }
        let mut reviews = lock(&self.inner.background.reviews);
        reviews.retain(|_, review| review.until > now);
        reviews.insert(
            reference,
            Review {
                caller,
                identity,
                thread,
                until,
                capture,
                targets,
                total: roster.total,
            },
        );
        Ok(roster)
    }

    /// Accepts only the captured mutation identities in the current guard's transaction.
    pub async fn end_background_guarded(
        &self,
        mut tx: Tx,
        principal: Principal,
        identity: Identity,
        thread: Id,
        request: BackgroundEndRequest,
    ) -> Result<jobs::JobRow, BackgroundError> {
        let now = principal.now().get();
        permitted(
            nodes::check(
                &mut tx,
                &principal,
                thread,
                Permission::ThreadInteract,
                Act::Use,
                Policy::default(),
            )
            .await?,
        )?;
        let reference = match &request {
            BackgroundEndRequest::Selected(request) => &request.list_ref,
            BackgroundEndRequest::All(request) => &request.list_ref,
        };
        let (capture, targets) = {
            let reviews = lock(&self.inner.background.reviews);
            let review = reviews.get(reference).ok_or(BackgroundError::Stale)?;
            if review.caller != *principal.caller()
                || review.identity != identity
                || review.thread != thread
                || review.until <= now
            {
                return Err(BackgroundError::Stale);
            }
            let selected = select_targets(&review.targets, review.total, &request)?;
            (review.capture.clone(), selected)
        };
        // End retains its own operation boundary; do not add the roster read's
        // key selection to this existing ordinary control check.
        let provider_directories = capture
            .sessions
            .iter()
            .map(|(source, _)| source.provider_id)
            .collect();
        let (roster, _) = self
            .project_background_for(&mut tx, &principal, thread, &capture, &provider_directories)
            .await?;
        if !roster.can_end {
            return Err(BackgroundError::Stale);
        }
        let current = sources(&mut tx, thread).await?;
        if targets.iter().any(|target| {
            !current.iter().any(|source| {
                source.segment_id == target.segment_id
                    && source.machine_id == Some(target.machine_id)
                    && source.native_session_ref == target.native_session_ref
            })
        }) {
            return Err(BackgroundError::Stale);
        }
        let credential = principal
            .caller()
            .grant()
            .map(|grant| json!({"scopes":grant.scopes().to_strings(), "subtree":grant.subtree()}));
        let plan = json!({"thread":thread, "user":principal.user(), "identity":identity, "credential":credential, "targets":targets});
        let progress = serde_json::to_value(crate::jobs::runner::initial_progress(
            crate::jobs::background_end::PHASES,
        ))?;
        let job = jobs::insert(
            &mut tx,
            &jobs::NewJob {
                id: new_id(),
                kind: crate::jobs::background_end::KIND,
                node_id: Some(thread),
                environment_id: None,
                provider_id: None,
                plan: &plan,
                progress: &progress,
                created_by: Some(principal.user()),
            },
            now,
        )
        .await?;
        crate::audit_facts::person(&mut tx, principal.user(), thread, "thread.background_end", json!({"job_id":job.id, "targets": targets.iter().map(|target|json!({"id":target.id,"segment_id":target.segment_id,"turn_id":target.turn_id,"item_id":target.item_id})).collect::<Vec<_>>() }), now).await?;
        tx.commit().await.map_err(DbError::from)?;
        lock(&self.inner.background.reviews).remove(reference);
        Ok(job)
    }
}

fn select_targets(
    targets: &[Target],
    total: Option<i64>,
    request: &BackgroundEndRequest,
) -> Result<Vec<Target>, BackgroundError> {
    match request {
        BackgroundEndRequest::All(request) => {
            if !request.all
                || request.count <= 0
                || total != Some(request.count)
                || request.count != i64::try_from(targets.len()).unwrap_or(i64::MAX)
            {
                return Err(BackgroundError::Stale);
            }
            Ok(targets.to_vec())
        }
        BackgroundEndRequest::Selected(request) => {
            let wanted: HashSet<_> = request.command_ids.iter().collect();
            if wanted.is_empty()
                || wanted.len() > PAGE_MAX
                || wanted.len() != request.command_ids.len()
            {
                return Err(BackgroundError::Invalid(
                    "select one to two hundred distinct reviewed command ids",
                ));
            }
            let selected: Vec<_> = targets
                .iter()
                .filter(|target| wanted.contains(&target.id))
                .cloned()
                .collect();
            if selected.len() != wanted.len() {
                return Err(BackgroundError::Stale);
            }
            Ok(selected)
        }
    }
}

/// A forced controller end leaves native work blocked until an exact native completion arrives.
pub(crate) async fn native_busy(conn: &mut Conn, thread: Id) -> Result<bool, EngineError> {
    Ok(sqlx::query_scalar(
        "SELECT EXISTS (SELECT 1 FROM turns WHERE thread_id=$1 AND state='interrupted'
        AND error->>'code'='forced' AND NOT native_end_confirmed)",
    )
    .bind(thread)
    .fetch_one(conn)
    .await
    .map_err(DbError::from)?)
}

/// The stable explanation used only for an unresolved native turn, not other thread failures.
pub(crate) const NATIVE_BUSY: &str = "native_turn_still_running: the native turn's end is not known; fresh work waits for its actual completion";

#[cfg(test)]
mod tests {
    use super::*;
    fn target(id: &str) -> Target {
        Target {
            id: id.into(),
            segment_id: new_id(),
            machine_id: new_id(),
            native_session_ref: "native".into(),
            command_id: id.into(),
            turn_id: None,
            item_id: None,
        }
    }

    #[test]
    fn end_all_accepts_a_complete_large_review_and_never_adds_later_work() {
        let reviewed: Vec<_> = (0..250).map(|index| target(&index.to_string())).collect();
        let request = BackgroundEndRequest::All(orch_api_contract::background::BackgroundEndAll {
            list_ref: "review".into(),
            all: true,
            count: 250,
        });
        let accepted = select_targets(&reviewed, Some(250), &request).unwrap();
        let later = target("later");
        assert!(!accepted.contains(&later));
        assert_eq!(accepted.len(), 250);
        assert!(matches!(
            select_targets(&reviewed, None, &request),
            Err(BackgroundError::Stale)
        ));
    }

    #[test]
    fn exact_selection_refuses_duplicates_unknown_ids_and_a_count_from_a_different_review() {
        let reviewed = vec![target("one")];
        let selected = |ids: Vec<&str>| {
            BackgroundEndRequest::Selected(orch_api_contract::background::BackgroundEndSelected {
                list_ref: "review".into(),
                command_ids: ids.into_iter().map(str::to_owned).collect(),
            })
        };
        assert!(select_targets(&reviewed, Some(1), &selected(vec!["one", "one"])).is_err());
        assert!(matches!(
            select_targets(&reviewed, Some(1), &selected(vec!["another"])),
            Err(BackgroundError::Stale)
        ));
        assert_eq!(
            select_targets(&reviewed, Some(1), &selected(vec!["one"])).unwrap(),
            reviewed
        );
    }
}
