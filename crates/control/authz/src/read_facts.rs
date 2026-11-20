//! Shared ordinary facts within one effect-free repeatable read, never a retained admission.

use crate::engine::{PageRef, Store};
use crate::holders::AccountFacts;
use orch_access_contract::permissions::Permission;
use orch_db::{Conn, Tx};
use orch_types::{Id, Time};
use std::cell::RefCell;
use std::collections::HashMap;
use std::future::Future;
use std::pin::Pin;

/// Catalogue cells already fetched for one page, including cells with no stored row.
#[derive(Clone, Copy, Default)]
pub(crate) struct FetchedPermissions([u64; Permission::ALL.len().div_ceil(64)]);

impl FetchedPermissions {
    fn from_slice(permissions: &[Permission]) -> Self {
        let mut cells = Self::default();
        for &permission in permissions {
            let index = permission as usize;
            cells.0[index / 64] |= 1 << (index % 64);
        }
        cells
    }

    pub(crate) fn contains(self, permission: Permission) -> bool {
        let index = permission as usize;
        self.0[index / 64] & (1 << (index % 64)) != 0
    }

    fn covers(self, wanted: Self) -> bool {
        self.0
            .iter()
            .zip(wanted.0)
            .all(|(have, need)| have & need == need)
    }

    fn extend(&mut self, added: Self) {
        for (have, new) in self.0.iter_mut().zip(added.0) {
            *have |= new;
        }
    }
}

struct Facts {
    connection: usize,
    accounts: HashMap<(Id, bool), AccountFacts>,
    rows: Store,
    loaded: HashMap<PageRef, FetchedPermissions>,
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
        loaded: HashMap::new(),
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
            if facts.connection != connection(conn) {
                return None;
            }
            let wanted = FetchedPermissions::from_slice(permissions);
            if !pages.iter().all(|page| {
                facts
                    .loaded
                    .get(page)
                    .copied()
                    .unwrap_or_default()
                    .covers(wanted)
            }) {
                return None;
            }
            Some(facts.rows.selected(pages, wanted))
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
        let fetched = FetchedPermissions::from_slice(permissions);
        for &page in pages {
            facts.loaded.entry(page).or_default().extend(fetched);
        }
        facts.rows.extend(rows.clone());
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fetching_one_permission_records_only_its_own_catalogue_cell() {
        let mut fetched = FetchedPermissions::default();
        for (i, &permission) in Permission::ALL.iter().enumerate() {
            let wanted = FetchedPermissions::from_slice(&[permission]);
            assert!(!fetched.covers(wanted));
            fetched.extend(wanted);
            assert!(fetched.covers(wanted));
            assert!(fetched.contains(permission));
            assert!(
                Permission::ALL[i + 1..]
                    .iter()
                    .all(|&later| !fetched.contains(later))
            );
        }
        assert!(fetched.covers(FetchedPermissions::from_slice(Permission::ALL)));
    }
}
