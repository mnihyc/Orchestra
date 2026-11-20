//! Shared ordinary facts within one effect-free repeatable read, never a retained admission.

use crate::engine::{PageRef, Store};
use crate::holders::AccountFacts;
use orch_access_contract::permissions::Permission;
use orch_db::{Conn, Tx};
use orch_types::{Id, Time};
use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::future::Future;
use std::pin::Pin;

struct Facts {
    connection: usize,
    accounts: HashMap<(Id, bool), AccountFacts>,
    rows: Store,
    loaded: HashSet<(PageRef, Permission)>,
}

tokio::task_local! {static CURRENT:RefCell<Facts>;}

/// Run only effect-free factual selection/projection on this still-open snapshot.
/// Credential restoration, source admission, deadlines and final publication still run normally.
/// The shared store ends with this future or its cancellation; another connection and
/// a fresh attempt read their own facts.
/// Ordinary write transactions take the uncached path, including their own subsequent writes.
pub async fn scope<'a, T>(
    tx: &'a mut Tx,
    read: impl FnOnce(&'a mut Conn) -> Pin<Box<dyn Future<Output = T> + Send + 'a>> + Send + 'a,
) -> T {
    if !tx.repeatable_read() {
        return read(tx).await;
    }
    let facts = Facts {
        connection: std::ptr::from_ref::<Conn>(tx).addr(),
        accounts: HashMap::new(),
        rows: Store::default(),
        loaded: HashSet::new(),
    };
    CURRENT
        .scope(
            RefCell::new(facts),
            orch_db::queries::snapshot::scope(tx, read),
        )
        .await
}

fn connection(conn: &Conn) -> usize {
    std::ptr::from_ref(conn).addr()
}

pub(crate) fn account(conn: &Conn, user: Id, native: bool, now: Time) -> Option<AccountFacts> {
    if !orch_db::pool::read_facts_unchanged() {
        return None;
    }
    CURRENT
        .try_with(|current| {
            let facts = current.borrow();
            if facts.connection != connection(conn) {
                return None;
            }
            facts
                .accounts
                .get(&(user, native))
                .and_then(|account| account.at(now))
        })
        .ok()
        .flatten()
}

pub(crate) fn remember_account(conn: &Conn, user: Id, native: bool, account: &AccountFacts) {
    if !orch_db::pool::read_facts_unchanged() {
        return;
    }
    let _ = CURRENT.try_with(|current| {
        let mut facts = current.borrow_mut();
        if facts.connection == connection(conn) {
            facts.accounts.insert((user, native), account.clone());
        }
    });
}

pub(crate) fn stored(conn: &Conn, pages: &[PageRef], permissions: &[Permission]) -> Option<Store> {
    if !orch_db::pool::read_facts_unchanged() {
        return None;
    }
    CURRENT
        .try_with(|current| {
            let facts = current.borrow();
            if facts.connection != connection(conn)
                || !pages.iter().all(|page| {
                    permissions
                        .iter()
                        .all(|permission| facts.loaded.contains(&(*page, *permission)))
                })
            {
                return None;
            }
            Some(facts.rows.selected(pages, permissions))
        })
        .ok()
        .flatten()
}

pub(crate) fn remember_stored(
    conn: &Conn,
    pages: &[PageRef],
    permissions: &[Permission],
    rows: &Store,
) {
    if !orch_db::pool::read_facts_unchanged() {
        return;
    }
    let _ = CURRENT.try_with(|current| {
        let mut facts = current.borrow_mut();
        if facts.connection != connection(conn) {
            return;
        }
        facts.loaded.extend(pages.iter().flat_map(|page| {
            permissions
                .iter()
                .map(move |permission| (*page, *permission))
        }));
        facts.rows.extend(rows.clone());
    });
}
