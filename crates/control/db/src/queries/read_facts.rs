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
use orch_types::{Id, Json, Time};
use std::{cell::RefCell, collections::HashMap};

#[derive(Default)]
pub(super) struct SnapshotFacts {
    opening: Option<OpeningFacts>,
    pub machine: Option<(Id, EnvironmentMachine)>,
    pub profile: Option<(Id, Placement)>,
    pub display: Option<(Option<Id>, Time, ThreadDisplayFacts)>,
    pub owners: HashMap<Vec<(String, Id)>, Vec<EntryRow>>,
    pub roles: HashMap<Vec<Id>, (DateTime<Utc>, Vec<RoleRow>)>,
}

/// Only a database-enforced read-only snapshot can reuse queries that would
/// otherwise record authority dependencies when a writer arms its guard.
#[derive(Default)]
pub(super) struct OpeningFacts {
    pub settings: super::settings::OpeningSettings,
    pub ancestors: HashMap<Id, GovernedNode>,
    pub resources: HashMap<Vec<Id>, Vec<AccessFacts>>,
    pub thread: Option<ThreadRow>,
    pub histories_without_spans: std::collections::HashSet<(Id, Id)>,
    pub projections: HashMap<(Id, Option<Id>), super::thread_continuations::Projection>,
    pub environments: HashMap<Id, super::environments::thread_engine::EnvironmentRow>,
    pub thread_environments: HashMap<Id, Option<super::settings::ThreadEnvironment>>,
    pub thread_model_parameters: HashMap<Id, HashMap<String, Option<Json>>>,
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

/// SQL orders the rows independently of the order of the requested places.
/// Keep repetitions because the ownership query joins each supplied object.
pub(super) fn batch_key<K: Ord>(conn: &Conn, keys: impl Iterator<Item = K>) -> Option<Vec<K>> {
    with_snapshot(conn, |_| {
        let mut keys: Vec<_> = keys.collect();
        keys.sort_unstable();
        keys
    })
}
