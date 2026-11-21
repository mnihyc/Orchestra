//! Ordered socket controls, bounded by the connection's existing byte budget.

use orch_realtime_contract::frames::ClientFrame;
use std::collections::VecDeque;
use std::sync::{Mutex, MutexGuard, PoisonError};
use tokio::sync::Notify;

struct Queue {
    frames: VecDeque<(ClientFrame, usize)>,
    bytes: usize,
}

pub(super) struct Incoming {
    queue: Mutex<Queue>,
    ready: Notify,
    space: Notify,
    max_bytes: usize,
}

impl Incoming {
    pub(super) fn new(max_bytes: i64) -> Self {
        Self {
            queue: Mutex::new(Queue {
                frames: VecDeque::new(),
                bytes: 0,
            }),
            ready: Notify::new(),
            space: Notify::new(),
            max_bytes: usize::try_from(max_bytes).unwrap_or_default(),
        }
    }

    fn lock(&self) -> MutexGuard<'_, Queue> {
        self.queue.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Apply network backpressure without losing or reordering a control. One individually
    /// valid frame always fits an empty queue, even with a smaller configured byte budget.
    /// Besides this queue, the worker and the reader can each own one protocol-bounded frame.
    pub(super) async fn push(&self, frame: ClientFrame, wire_bytes: usize) {
        let weight = wire_bytes.saturating_add(size_of::<ClientFrame>());
        loop {
            let available = self.space.notified();
            tokio::pin!(available);
            available.as_mut().enable();
            {
                let mut queue = self.lock();
                if queue.frames.is_empty() || queue.bytes.saturating_add(weight) <= self.max_bytes {
                    queue.bytes = queue.bytes.saturating_add(weight);
                    queue.frames.push_back((frame, weight));
                    self.ready.notify_one();
                    return;
                }
            }
            available.await;
        }
    }

    pub(super) async fn next(&self) -> ClientFrame {
        loop {
            let ready = self.ready.notified();
            tokio::pin!(ready);
            ready.as_mut().enable();
            {
                let mut queue = self.lock();
                if let Some((frame, weight)) = queue.frames.pop_front() {
                    queue.bytes = queue.bytes.saturating_sub(weight);
                    self.space.notify_one();
                    return frame;
                }
            }
            ready.await;
        }
    }
}
