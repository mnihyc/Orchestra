//! Facts read on one connection during an effect-free repeatable-read transaction.

use super::access::{EntryRow, RoleRow};
use super::environment_facts::EnvironmentMachine;
use super::hierarchy::GovernedNode;
use super::resources::AccessFacts;
use super::source::ThreadDisplayFacts;
use super::threads::ThreadRow;
use crate::Conn;
use chrono::{DateTime, Utc};
use orch_access_contract::permissions::Placement;
use orch_types::{Id, Json, Seq, Time};
use std::{cell::RefCell, collections::HashMap};

#[derive(Default)]
pub(super) struct SnapshotFacts {
    opening: Option<OpeningFacts>,
    pub participation:
        HashMap<super::access::participation::MarkSources, Vec<super::access::participation::Mark>>,
    pub audiences: HashMap<(DateTime<Utc>, super::access::HolderPeopleQuery), Vec<Id>>,
    pub chats: HashMap<Id, Option<super::chats::ChatRow>>,
    pub machine: Option<(Id, EnvironmentMachine)>,
    pub profile: Option<(Id, Placement)>,
    pub display: Option<(Option<Id>, Time, ThreadDisplayFacts)>,
    pub owners: HashMap<(String, Id), Vec<EntryRow>>,
    pub roles: HashMap<(DateTime<Utc>, Id), Vec<RoleRow>>,
}

/// Only a database-enforced read-only snapshot can reuse queries that would
/// otherwise record authority dependencies when a writer arms its guard.
#[derive(Default)]
pub(super) struct OpeningFacts {
    pub people: HashMap<Id, Option<super::identity::UserBrief>>,
    pub settings: super::settings::OpeningSettings,
    pub ancestors: HashMap<Id, GovernedNode>,
    /// The kind is absent when all resource tables have been read for this ID.
    pub resources: HashMap<(Id, Option<&'static str>), Vec<AccessFacts>>,
    pub thread: Option<ThreadRow>,
    pub details: Option<super::thread_continuations::OpeningDetails>,
    pub covering_caps: Option<(Id, bool)>,
    pub schedules: Option<super::thread_continuations::OpeningSchedules>,
    pub no_call_recovery: Option<Id>,
    pub activity: Option<(Option<Id>, super::threads::ThreadActivity)>,
    /// Thread, active continuation and current segment observed without background work.
    pub quiet_background: Option<(Id, Id, Option<Id>)>,
    /// The active read position for one (viewer, thread, continuation).
    pub active_read: Option<((Id, Id, Id), Seq)>,
    pub history_sources: HashMap<(Id, Id), super::thread_continuations::HistorySources>,
    pub projections: HashMap<(Id, Option<Id>), super::thread_continuations::Projection>,
    pub environments: HashMap<Id, super::environments::thread_engine::EnvironmentRow>,
    pub thread_environments: HashMap<Id, Option<super::settings::ThreadEnvironment>>,
    pub thread_model_parameters: HashMap<Id, HashMap<String, Option<Json>>>,
    /// Provider, routing name and display name seen while resolving the selected model.
    pub selected_model_name: Option<(Id, String, String)>,
    pub ownership_sources: HashMap<Id, Vec<(Id, String, Id)>>,
}

pub(super) fn with_opening<T>(
    conn: &Conn,
    use_facts: impl FnOnce(&mut OpeningFacts) -> T,
) -> Option<T> {
    with_snapshot(conn, |facts| facts.opening.as_mut().map(use_facts)).flatten()
}

tokio::task_local! { static SNAPSHOT: (usize, RefCell<SnapshotFacts>); }

pub(crate) async fn sharing<T>(
    connection: usize,
    read_only: bool,
    work: impl Future<Output = T>,
) -> T {
    let facts = SnapshotFacts {
        opening: read_only.then(OpeningFacts::default),
        ..SnapshotFacts::default()
    };
    SNAPSHOT
        .scope((connection, RefCell::new(facts)), work)
        .await
}

pub(super) fn with_snapshot<T>(
    conn: &Conn,
    use_facts: impl FnOnce(&mut SnapshotFacts) -> T,
) -> Option<T> {
    if !crate::pool::read_facts_unchanged() {
        return None;
    }
    SNAPSHOT
        .try_with(|(connection, facts)| {
            (*connection == std::ptr::from_ref(conn).addr())
                .then(|| use_facts(&mut facts.borrow_mut()))
        })
        .ok()
        .flatten()
}
