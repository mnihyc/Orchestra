//! Publication receipts and retirement share one physical-content lifetime.
//!
//! A publisher keeps its receipt until metadata takes custody. Retirement excludes new publishers
//! before it checks that metadata and unlinks the file. A cancelled waiter removes itself.
//! Publications remain admitted while retirement waits, avoiding an upgrade deadlock when one
//! transaction publishes several hashes. Only live hashes are indexed; releasing a hash removes its cell.

use crate::store::BlobError;
use futures::future::poll_fn;
use orch_types::Hash;
use std::collections::{BTreeMap, HashMap};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError, Weak};
use std::task::{Poll, Waker};

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

#[derive(Debug, Default)]
struct Index {
    hashes: Mutex<HashMap<Hash, Weak<Cell>>>,
}

/// One store's coordination, shared by all its clones and forwarding handles.
#[derive(Debug, Default, Clone)]
pub struct Lifetimes(Arc<Index>);

#[derive(Debug)]
struct Cell {
    owner: Weak<Index>,
    hash: Hash,
    state: Mutex<State>,
}

impl Drop for Cell {
    fn drop(&mut self) {
        if let Some(owner) = self.owner.upgrade() {
            let mut hashes = lock(&owner.hashes);
            if hashes
                .get(&self.hash)
                .is_some_and(|cell| std::ptr::eq(cell.as_ptr(), self))
            {
                hashes.remove(&self.hash);
            }
        }
    }
}

#[derive(Debug, Default)]
struct State {
    next: u64,
    publications: usize,
    retiring: bool,
    publishers: BTreeMap<u64, Waker>,
    retirements: BTreeMap<u64, Waker>,
}

impl State {
    fn ready(&self) -> Vec<Waker> {
        if self.retiring {
            return Vec::new();
        }
        if self.publications == 0
            && let Some((_, first)) = self.retirements.first_key_value()
        {
            return vec![first.clone()];
        }
        self.publishers.values().cloned().collect()
    }
}

fn wake(wakers: Vec<Waker>) {
    for waker in wakers {
        waker.wake();
    }
}

impl Lifetimes {
    fn cell(&self, hash: Hash) -> Arc<Cell> {
        let mut hashes = lock(&self.0.hashes);
        if let Some(cell) = hashes.get(&hash).and_then(Weak::upgrade) {
            return cell;
        }
        let cell = Arc::new(Cell {
            owner: Arc::downgrade(&self.0),
            hash,
            state: Mutex::new(State::default()),
        });
        hashes.insert(hash, Arc::downgrade(&cell));
        cell
    }

    /// Begins physical publication. Its guard follows the stored receipt through metadata publication.
    pub async fn publishing(&self, hash: Hash) -> Result<Publication, BlobError> {
        let cell = self.cell(hash);
        let ticket = Ticket::new(Arc::clone(&cell), false)?;
        let publication = poll_fn(|context| {
            let mut state = lock(&cell.state);
            if !state.retiring {
                state.publishers.remove(&ticket.id);
                let Some(next) = state.publications.checked_add(1) else {
                    return Poll::Ready(Err(BlobError::Io(
                        "the number of publication receipts cannot be represented".into(),
                    )));
                };
                state.publications = next;
                Poll::Ready(Ok(Publication(Arc::new(Hold {
                    cell: Arc::clone(&cell),
                    retiring: false,
                }))))
            } else {
                state.publishers.insert(ticket.id, context.waker().clone());
                Poll::Pending
            }
        })
        .await;
        drop(ticket);
        publication
    }

    /// Attempts exclusion without waiting while another resource such as a DB transaction is held.
    pub fn try_retiring(&self, hash: Hash) -> Option<Retirement> {
        let cell = self.cell(hash);
        let mut state = lock(&cell.state);
        if state.retiring || state.publications != 0 || !state.retirements.is_empty() {
            return None;
        }
        state.retiring = true;
        drop(state);
        Some(Retirement(Arc::new(Hold {
            cell,
            retiring: true,
        })))
    }

    /// Excludes physical publication before the caller rechecks references and removes content.
    pub async fn retiring(&self, hash: Hash) -> Result<Retirement, BlobError> {
        let cell = self.cell(hash);
        let ticket = Ticket::new(Arc::clone(&cell), true)?;
        let retirement = poll_fn(|context| {
            let mut state = lock(&cell.state);
            state.retirements.insert(ticket.id, context.waker().clone());
            if !state.retiring
                && state.publications == 0
                && state
                    .retirements
                    .first_key_value()
                    .is_some_and(|(id, _)| *id == ticket.id)
            {
                state.retirements.remove(&ticket.id);
                state.retiring = true;
                Poll::Ready(Retirement(Arc::new(Hold {
                    cell: Arc::clone(&cell),
                    retiring: true,
                })))
            } else {
                Poll::Pending
            }
        })
        .await;
        drop(ticket);
        Ok(retirement)
    }
}

#[derive(Debug)]
struct Hold {
    cell: Arc<Cell>,
    retiring: bool,
}

impl Drop for Hold {
    fn drop(&mut self) {
        let wakers = {
            let mut state = lock(&self.cell.state);
            if self.retiring {
                state.retiring = false;
            } else {
                state.publications -= 1;
            }
            state.ready()
        };
        wake(wakers);
    }
}

/// Keeps a physical file alive while its writer records who owns it. Filesystem tasks may retain a clone.
#[derive(Debug, Clone)]
pub struct Publication(Arc<Hold>);

impl Publication {
    /// Checks that an implementation is publishing the hash covered by its caller's guard.
    pub fn check(&self, lifetimes: &Lifetimes, hash: &Hash) -> Result<(), BlobError> {
        if self.0.cell.hash == *hash
            && Weak::ptr_eq(&self.0.cell.owner, &Arc::downgrade(&lifetimes.0))
        {
            return Ok(());
        }
        Err(BlobError::Io(
            "the publication does not belong to this content in this store".into(),
        ))
    }
}

/// Excludes new publishers through reference rechecking and the last physical filesystem operation.
#[derive(Debug, Clone)]
pub struct Retirement(Arc<Hold>);

impl Retirement {
    /// Prevents a deletion helper from accidentally using another store's or another hash's guard.
    pub fn check(&self, lifetimes: &Lifetimes, hash: &Hash) -> Result<(), BlobError> {
        if self.0.cell.hash == *hash
            && Weak::ptr_eq(&self.0.cell.owner, &Arc::downgrade(&lifetimes.0))
        {
            return Ok(());
        }
        Err(BlobError::Io(
            "the retirement does not belong to this content in this store".into(),
        ))
    }
}

struct Ticket {
    cell: Arc<Cell>,
    id: u64,
    retirement: bool,
}

impl Ticket {
    fn new(cell: Arc<Cell>, retirement: bool) -> Result<Self, BlobError> {
        let id = {
            let mut state = lock(&cell.state);
            let id = state.next;
            state.next = id.checked_add(1).ok_or_else(|| {
                BlobError::Io("the publication waiter sequence cannot be represented".into())
            })?;
            id
        };
        Ok(Self {
            cell,
            id,
            retirement,
        })
    }
}

impl Drop for Ticket {
    fn drop(&mut self) {
        let wakers = {
            let mut state = lock(&self.cell.state);
            if self.retirement {
                state.retirements.remove(&self.id);
            } else {
                state.publishers.remove(&self.id);
            }
            state.ready()
        };
        wake(wakers);
    }
}
