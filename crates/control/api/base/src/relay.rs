//! Bounded memory rendezvous for file content whose two ends are outside the server.
//!
//! An active transfer holds one offered piece. The destination's next read acknowledges the
//! prefix it has written, so a lost response can be retried from the destination's actual offset.
//! Admission waits for a memory slot before either request body is collected. Waiting transfers
//! hold no environment serial key, file content or database transaction.

use bytes::Bytes;
use orch_types::{Id, new_id};
use std::collections::HashMap;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError, Weak};
use tokio::sync::{Mutex as AsyncMutex, Notify};

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

/// Why a piece could not continue its transfer.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum RelayError {
    /// The destination holds a different prefix.
    #[error("the piece does not follow the destination's {received} bytes")]
    Offset { received: u64 },
    /// A range extends outside the announced file or skips bytes nobody offered.
    #[error("the range does not belong to this transfer")]
    Range,
    /// An end of the transfer went away.
    #[error("the transfer ended")]
    Ended,
}

#[derive(Debug, Default)]
struct Registry {
    open: Mutex<HashMap<Id, Weak<Relay>>>,
    available: Notify,
    sessions: Mutex<HashMap<Id, Weak<AsyncMutex<()>>>>,
}

/// The live transfers of this API process. Its clones share the same memory admission.
#[derive(Debug, Default, Clone)]
pub struct Relays(Arc<Registry>);

impl Relays {
    /// Waits for a slot under the resolved concurrency setting, then creates one transfer.
    /// The caller restores its current authority after this wait and before issuing a command.
    pub async fn create(&self, concurrency: usize, size: u64) -> Arc<Relay> {
        loop {
            let available = self.0.available.notified();
            tokio::pin!(available);
            available.as_mut().enable();
            {
                let mut open = lock(&self.0.open);
                open.retain(|_, relay| relay.strong_count() > 0);
                if open.len() < concurrency.max(1) {
                    let relay = Arc::new(Relay {
                        id: new_id(),
                        size,
                        registry: Arc::downgrade(&self.0),
                        state: Mutex::new(State::default()),
                        input: AsyncMutex::new(()),
                        changed: Notify::new(),
                    });
                    open.insert(relay.id, Arc::downgrade(&relay));
                    return relay;
                }
            }
            available.await;
        }
    }

    /// Serializes one upload before its body or a memory slot is allocated.
    pub async fn session(&self, id: Id) -> tokio::sync::OwnedMutexGuard<()> {
        let gate = {
            let mut sessions = lock(&self.0.sessions);
            sessions.retain(|_, gate| gate.strong_count() > 0);
            match sessions.get(&id).and_then(Weak::upgrade) {
                Some(gate) => gate,
                None => {
                    let gate = Arc::new(AsyncMutex::new(()));
                    sessions.insert(id, Arc::downgrade(&gate));
                    gate
                }
            }
        };
        gate.lock_owned().await
    }

    /// Finds a live meeting place; identity alone grants no authority to use it.
    pub fn get(&self, id: Id) -> Option<Arc<Relay>> {
        lock(&self.0.open).get(&id).and_then(Weak::upgrade)
    }

    /// The number of active transfers, without the empty entries of ended ones.
    pub fn count(&self) -> usize {
        lock(&self.0.open)
            .values()
            .filter(|relay| relay.strong_count() > 0)
            .count()
    }
}

#[derive(Debug, Default)]
struct State {
    peer: bool,
    received: u64,
    offered: u64,
    piece: Option<(u64, Bytes)>,
    finished: bool,
    cancelled: bool,
}

/// One command's file transfer. Only its authenticated ends may obtain this handle.
#[derive(Debug)]
pub struct Relay {
    id: Id,
    size: u64,
    registry: Weak<Registry>,
    state: Mutex<State>,
    input: AsyncMutex<()>,
    changed: Notify,
}

impl Drop for Relay {
    fn drop(&mut self) {
        if let Some(registry) = self.registry.upgrade() {
            registry.available.notify_waiters();
        }
    }
}

impl Relay {
    /// The rendezvous identity named by the upload session or command.
    pub fn id(&self) -> Id {
        self.id
    }

    /// The destination's durable offset, not the number of bytes accepted into API memory.
    pub fn received(&self) -> u64 {
        lock(&self.state).received
    }

    /// Serializes producers before they collect a request body. At most one piece is buffered.
    pub async fn input(&self) -> Input<'_> {
        Input {
            relay: self,
            _held: self.input.lock().await,
        }
    }

    /// Offers one browser piece and waits for the destination to acknowledge it.
    pub async fn offer(&self, offset: u64, bytes: Bytes) -> Result<u64, RelayError> {
        self.input().await.offer(offset, bytes).await
    }

    /// Offers one machine piece and waits for the browser's response stream to take it.
    pub async fn push(&self, offset: u64, bytes: Bytes) -> Result<u64, RelayError> {
        self.input().await.push(offset, bytes).await
    }

    /// The machine reports its on-disk prefix, initially or after writing the final piece.
    pub fn stand(&self, received: u64) -> Result<(), RelayError> {
        {
            let mut state = lock(&self.state);
            if state.cancelled {
                return Err(RelayError::Ended);
            }
            if received > self.size || (state.peer && received > state.offered) {
                return Err(RelayError::Range);
            }
            if state.peer && received < state.received {
                return Err(RelayError::Offset {
                    received: state.received,
                });
            }
            state.peer = true;
            state.received = received;
            state.offered = state.offered.max(received);
        }
        self.changed.notify_waiters();
        Ok(())
    }

    /// Reads a range for a machine, acknowledging only the prefix it says it already wrote.
    pub async fn pull(&self, offset: u64, len: u64) -> Result<Option<Bytes>, RelayError> {
        if len == 0 || offset > self.size {
            return Err(RelayError::Range);
        }
        self.stand(offset)?;
        loop {
            let changed = self.changed.notified();
            tokio::pin!(changed);
            changed.as_mut().enable();
            {
                let state = lock(&self.state);
                if state.cancelled {
                    return Err(RelayError::Ended);
                }
                if let Some((from, bytes)) = &state.piece
                    && *from <= offset
                    && offset - *from < bytes.len() as u64
                {
                    let start = usize::try_from(offset - *from).map_err(|_| RelayError::Range)?;
                    let len = usize::try_from(len).unwrap_or(usize::MAX);
                    return Ok(Some(
                        bytes.slice(start..start.saturating_add(len).min(bytes.len())),
                    ));
                }
                if state.finished || offset == self.size {
                    return Ok(None);
                }
            }
            changed.await;
        }
    }

    /// Takes the next piece for a browser. End-of-stream is explicit, including an empty file.
    pub async fn next(&self) -> Result<Option<Bytes>, RelayError> {
        loop {
            let changed = self.changed.notified();
            tokio::pin!(changed);
            changed.as_mut().enable();
            let result = {
                let mut state = lock(&self.state);
                if state.cancelled {
                    return Err(RelayError::Ended);
                }
                if let Some((offset, bytes)) = state.piece.take() {
                    state.received = offset + bytes.len() as u64;
                    Some(Some(bytes))
                } else if state.finished {
                    Some(None)
                } else {
                    None
                }
            };
            if let Some(result) = result {
                self.changed.notify_waiters();
                return Ok(result);
            }
            changed.await;
        }
    }

    /// Finishes successfully; an already offered final piece remains readable.
    pub fn finish(&self) {
        lock(&self.state).finished = true;
        self.changed.notify_waiters();
    }

    /// Ends a dropped or failed transfer and discards its unacknowledged memory.
    pub fn cancel(&self) {
        let mut state = lock(&self.state);
        state.cancelled = true;
        state.piece = None;
        drop(state);
        self.changed.notify_waiters();
    }

    async fn publish(&self, offset: u64, bytes: Bytes) -> Result<u64, RelayError> {
        let end = offset
            .checked_add(bytes.len() as u64)
            .ok_or(RelayError::Range)?;
        if end > self.size {
            return Err(RelayError::Range);
        }
        loop {
            let changed = self.changed.notified();
            tokio::pin!(changed);
            changed.as_mut().enable();
            {
                let mut state = lock(&self.state);
                if state.cancelled || state.finished {
                    return Err(RelayError::Ended);
                }
                if state.peer {
                    if state.received != offset {
                        return Err(RelayError::Offset {
                            received: state.received,
                        });
                    }
                    state.offered = end;
                    state.piece = Some((offset, bytes));
                    break;
                }
            }
            changed.await;
        }
        let _withdraw = Withdraw(self, offset);
        self.changed.notify_waiters();
        loop {
            let changed = self.changed.notified();
            tokio::pin!(changed);
            changed.as_mut().enable();
            {
                let state = lock(&self.state);
                if state.received >= end {
                    return Ok(state.received);
                }
                if state.cancelled || state.finished {
                    return Err(RelayError::Ended);
                }
            }
            changed.await;
        }
    }
}

/// A producer slot acquired before buffering its bounded HTTP body.
pub struct Input<'a> {
    relay: &'a Relay,
    _held: tokio::sync::MutexGuard<'a, ()>,
}

impl Input<'_> {
    /// Publishes browser input; the machine establishes its resumed offset first.
    pub async fn offer(self, offset: u64, bytes: Bytes) -> Result<u64, RelayError> {
        self.relay.publish(offset, bytes).await
    }

    /// Publishes machine input; the browser consumes from the start of this ranged response.
    pub async fn push(self, offset: u64, bytes: Bytes) -> Result<u64, RelayError> {
        {
            let mut state = lock(&self.relay.state);
            if !state.peer {
                state.peer = true;
                state.received = offset;
                state.offered = offset;
            }
        }
        self.relay.publish(offset, bytes).await
    }
}

struct Withdraw<'a>(&'a Relay, u64);

impl Drop for Withdraw<'_> {
    fn drop(&mut self) {
        let mut state = lock(&self.0.state);
        if state
            .piece
            .as_ref()
            .is_some_and(|(offset, _)| *offset == self.1)
        {
            state.piece = None;
        }
    }
}

mod reading;

pub use reading::{Options, Read};
