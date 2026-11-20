//! Rows reused while one effect-free projection holds the same repeatable-read connection.

use super::access::RoleRow;
use super::environment_facts::EnvironmentMachine;
use super::hierarchy::GovernedNode;
use super::resources::AccessFacts;
use super::settings::{SettingRow, ThreadEnvironment};
use crate::{Conn, DbError, Tx};
use chrono::{DateTime, Utc};
use orch_access_contract::permissions::Placement;
use orch_types::Id;
use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::future::Future;
use std::pin::Pin;

type Read<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;
type Groups<T> = HashMap<Id, Vec<T>>;

#[derive(Default)]
pub(super) struct Rows {
    pub nodes: HashMap<Id, Option<GovernedNode>>,
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

/// Remember every row selected for each requested identity, including an empty group.
/// Overlapping requests send only the new identities through the original batch query.
pub(super) async fn matching<T: Clone>(
    conn: &mut Conn,
    ids: &[Id],
    table: fn(&mut Rows) -> &mut Groups<T>,
    group: fn(&T) -> Id,
    load: impl for<'a> FnOnce(&'a mut Conn, &'a [Id]) -> Read<'a, Result<Vec<T>, DbError>>,
) -> Result<Vec<T>, DbError> {
    let parts = with(conn, |rows| {
        let known = table(rows);
        let mut seen = HashSet::new();
        let mut found = Vec::new();
        let mut missing = Vec::new();
        for &id in ids {
            if seen.insert(id) {
                match known.get(&id) {
                    Some(rows) => found.extend_from_slice(rows),
                    None => missing.push(id),
                }
            }
        }
        (found, missing)
    });
    let Some((mut found, missing)) = parts else {
        return load(conn, ids).await;
    };
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
}
