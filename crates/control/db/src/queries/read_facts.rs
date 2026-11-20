//! Facts read on one connection during an effect-free repeatable-read transaction.

use super::access::{EntryRow, RoleRow};
use super::environment_facts::EnvironmentMachine;
use super::source::ThreadDisplayFacts;
use crate::Conn;
use chrono::{DateTime, Utc};
use orch_access_contract::permissions::Placement;
use orch_types::{Id, Time};
use std::{cell::RefCell, collections::HashMap};

#[derive(Default)]
pub(super) struct SnapshotFacts {
    pub machine: Option<(Id, EnvironmentMachine)>,
    pub profile: Option<(Id, Placement)>,
    pub display: Option<(Option<Id>, Time, ThreadDisplayFacts)>,
    pub owners: HashMap<Vec<(String, Id)>, Vec<EntryRow>>,
    pub roles: HashMap<Vec<Id>, (DateTime<Utc>, Vec<RoleRow>)>,
}

tokio::task_local! { static SNAPSHOT: (usize, RefCell<SnapshotFacts>); }

pub(crate) async fn sharing<T>(connection: usize, work: impl Future<Output = T>) -> T {
    SNAPSHOT.scope((connection, RefCell::default()), work).await
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
