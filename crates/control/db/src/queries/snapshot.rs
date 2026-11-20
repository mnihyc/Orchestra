//! Rows reused while one effect-free projection holds the same repeatable-read connection.

use super::access::{EntryRow, PageKey, RoleRow};
use super::environment_facts::EnvironmentMachine;
use super::environments::thread_engine::EnvironmentRow;
use super::hierarchy::GovernedNode;
use super::resources::AccessFacts;
use super::settings::{SettingRow, ThreadEnvironment};
use super::threads::ThreadRow;
use crate::{Conn, DbError, Tx};
use chrono::{DateTime, Utc};
use orch_access_contract::permissions::Placement;
use orch_types::{Id, Json};
use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::future::Future;
use std::hash::Hash;
use std::pin::Pin;

type Read<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;
type LoadRow<T> = for<'a> fn(&'a mut Conn, Id) -> Read<'a, Result<Option<T>, DbError>>;
type Groups<T, K = Id> = HashMap<K, Vec<T>>;

#[derive(Default)]
pub(super) struct Rows {
    pub nodes: HashMap<Id, Option<GovernedNode>>,
    pub threads: HashMap<Id, Option<ThreadRow>>,
    pub workspaces: HashMap<Id, Option<EnvironmentRow>>,
    pub model_parameters: HashMap<Id, HashMap<String, Option<Json>>>,
    pub owners: Groups<EntryRow, PageKey>,
    pub machines: Groups<(Id, EnvironmentMachine)>,
    pub placements: Groups<(Id, Placement)>,
    pub environments: Groups<(Id, ThreadEnvironment)>,
    pub resources: Groups<AccessFacts>,
    pub roles: Groups<RoleRow>,
    pub roles_since: Option<DateTime<Utc>>,
    pub all_settings: HashMap<Option<Id>, Vec<SettingRow>>,
}

tokio::task_local! { static CURRENT: (usize, RefCell<Rows>); }

/// Run the factual portion of one projection. A preview that begins a trial drops reuse
/// for the rest of the attempt. Another connection inside the same task reads for itself.
pub async fn scope<'a, T>(
    tx: &'a mut Tx,
    read: impl FnOnce(&'a mut Conn) -> Read<'a, T> + Send + 'a,
) -> T {
    if !tx.repeatable_read() {
        return read(tx).await;
    }
    let connection = std::ptr::from_ref::<Conn>(tx).addr();
    crate::pool::reading_facts(CURRENT.scope((connection, RefCell::default()), read(tx))).await
}

pub(super) fn with<T>(conn: &Conn, use_rows: impl FnOnce(&mut Rows) -> T) -> Option<T> {
    if !crate::pool::read_facts_unchanged() {
        return None;
    }
    CURRENT
        .try_with(|(connection, rows)| {
            (*connection == std::ptr::from_ref(conn).addr())
                .then(|| use_rows(&mut rows.borrow_mut()))
        })
        .ok()
        .flatten()
}

/// Fetch one possibly absent row, keeping it only for this read's exact connection.
pub(super) fn by_id<'a, T: Clone + Send + 'a>(
    conn: &'a mut Conn,
    id: Id,
    table: fn(&mut Rows) -> &mut HashMap<Id, Option<T>>,
    load: LoadRow<T>,
) -> Read<'a, Result<Option<T>, DbError>> {
    match with(conn, |rows| table(rows).get(&id).cloned()) {
        None => load(conn, id),
        Some(Some(found)) => Box::pin(std::future::ready(Ok(found))),
        Some(None) => Box::pin(async move {
            let value = load(conn, id).await?;
            let _ = with(conn, |rows| {
                table(rows).insert(id, value.clone());
            });
            Ok(value)
        }),
    }
}

/// Remember every row selected for each requested identity, including an empty group.
/// Overlapping requests send only the new identities through the original batch query.
pub(super) fn matching<'a, K, T>(
    conn: &'a mut Conn,
    ids: &'a [K],
    table: fn(&mut Rows) -> &mut Groups<T, K>,
    group: fn(&T) -> K,
    load: impl for<'c> FnOnce(&'c mut Conn, &'c [K]) -> Read<'c, Result<Vec<T>, DbError>> + Send + 'a,
) -> Read<'a, Result<Vec<T>, DbError>>
where
    K: Eq + Hash + Clone + Send + Sync + 'a,
    T: Clone + Send + 'a,
{
    let parts = with(conn, |rows| {
        let known = table(rows);
        let mut seen = HashSet::new();
        let mut found = Vec::new();
        let mut missing = Vec::new();
        for id in ids {
            if seen.insert(id) {
                match known.get(id) {
                    Some(rows) => found.extend_from_slice(rows),
                    None => missing.push(id.clone()),
                }
            }
        }
        (found, missing)
    });
    let Some((mut found, missing)) = parts else {
        return load(conn, ids);
    };
    Box::pin(async move {
        if !missing.is_empty() {
            let fresh = load(conn, &missing).await?;
            let _ = with(conn, |rows| {
                let known = table(rows);
                for id in missing {
                    known.entry(id).or_default();
                }
                for row in &fresh {
                    known.entry(group(row)).or_default().push(row.clone());
                }
            });
            found.extend(fresh);
        }
        Ok(found)
    })
}
