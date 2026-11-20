//! Rows reused while one effect-free projection holds the same repeatable-read connection.

use super::environment_facts::EnvironmentMachine;
use super::hierarchy::GovernedNode;
use crate::{Conn, DbError, Tx};
use orch_access_contract::permissions::Placement;
use orch_types::Id;
use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::future::Future;
use std::pin::Pin;

type Read<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;
type ById<T> = HashMap<Id, Option<T>>;

#[derive(Default)]
pub(super) struct Rows {
    pub nodes: ById<GovernedNode>,
    pub machines: ById<EnvironmentMachine>,
    pub placements: ById<Placement>,
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

/// A SQL ANY selection returns each matching identity once. Remember missing identities
/// too, and send only new ones through the same batch query when two pages overlap.
pub(super) async fn matching<T: Clone>(
    conn: &mut Conn,
    ids: &[Id],
    table: fn(&mut Rows) -> &mut ById<T>,
    load: impl for<'a> FnOnce(&'a mut Conn, &'a [Id]) -> Read<'a, Result<Vec<(Id, T)>, DbError>>,
) -> Result<Vec<(Id, T)>, DbError> {
    let parts = with(conn, |rows| {
        let known = table(rows);
        let mut seen = HashSet::new();
        let mut found = Vec::new();
        let mut missing = Vec::new();
        for &id in ids {
            if seen.insert(id) {
                match known.get(&id) {
                    Some(Some(row)) => found.push((id, row.clone())),
                    Some(None) => (),
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
            known.extend(missing.into_iter().map(|id| (id, None)));
            known.extend(fresh.iter().map(|(id, row)| (*id, Some(row.clone()))));
        });
        found.extend(fresh);
    }
    Ok(found)
}
