//! Resizable admission that keeps existing native owners charged through actual exit.

use super::lock;
use crate::error::{HostError, Result};
use std::sync::{Arc, Mutex};
use tokio::sync::Notify;

pub(super) struct Allowance {
    state: Mutex<(u64, u64)>,
    changed: Notify,
}

pub(super) struct Held(Arc<Allowance>);

impl Drop for Held {
    fn drop(&mut self) {
        let mut state = lock(&self.0.state);
        state.1 -= 1;
        drop(state);
        self.0.changed.notify_waiters();
    }
}

impl Allowance {
    pub(super) fn new(maximum: u64) -> Arc<Self> {
        Arc::new(Self {
            state: Mutex::new((maximum, 0)),
            changed: Notify::new(),
        })
    }

    pub(super) fn configure(&self, maximum: u64) {
        lock(&self.state).0 = maximum;
        self.changed.notify_waiters();
    }

    pub(super) fn maximum(&self) -> usize {
        usize::try_from(lock(&self.state).0).unwrap_or(usize::MAX)
    }

    #[cfg(all(test, target_os = "linux"))]
    pub(super) fn available_permits(&self) -> usize {
        let state = lock(&self.state);
        usize::try_from(state.0.saturating_sub(state.1)).unwrap_or(usize::MAX)
    }

    pub(super) fn try_acquire_owned(self: Arc<Self>) -> Result<Held> {
        let mut state = lock(&self.state);
        if state.1 >= state.0 {
            return Err(HostError::Busy(
                "the host's directory working allowance is in use".to_owned(),
            ));
        }
        state.1 += 1;
        drop(state);
        Ok(Held(self))
    }

    pub(super) async fn acquire_owned(self: Arc<Self>) -> Result<Held> {
        loop {
            let changed = self.changed.notified();
            tokio::pin!(changed);
            changed.as_mut().enable();
            if let Ok(held) = Arc::clone(&self).try_acquire_owned() {
                return Ok(held);
            }
            changed.await;
        }
    }
}
