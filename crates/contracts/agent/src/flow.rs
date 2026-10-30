//! Byte-aware bounded handoff queues that propagate consumer backpressure to native producers.

use std::pin::Pin;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::task::{Context, Poll};

use futures::Stream;
use tokio::sync::{OwnedSemaphorePermit, Semaphore, mpsc};

/// Small control messages cannot retain an unlimited number of queue nodes.
pub const QUEUE_ITEMS: usize = 64;
/// A normal native burst retains at most four MiB before its reader waits.
pub const QUEUE_BYTES: usize = 4 * 1024 * 1024;

#[derive(Debug, Default)]
struct Counts {
    items: AtomicUsize,
    bytes: AtomicUsize,
    peak_bytes: AtomicUsize,
}

/// Current charged payload weight and its high-water mark, including capacity held during delivery.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct QueueUsage {
    /// Admitted nodes, including capacity retained by a consumer's delivery batch.
    pub items: usize,
    /// Retained payload weight as supplied by the producer's measurement.
    pub bytes: usize,
    /// Largest retained payload weight observed by this queue.
    pub peak_bytes: usize,
}

struct Lease {
    _node: OwnedSemaphorePermit,
    _bytes: OwnedSemaphorePermit,
    counts: Arc<Counts>,
    weight: usize,
}

impl Drop for Lease {
    fn drop(&mut self) {
        self.counts.items.fetch_sub(1, Ordering::AcqRel);
        self.counts.bytes.fetch_sub(self.weight, Ordering::AcqRel);
    }
}

struct Queued<T> {
    item: T,
    _lease: Lease,
}

/// Keeps admission capacity beside a payload while a consumer finishes a bounded delivery batch.
pub struct Capacity {
    _lease: Lease,
}

impl std::fmt::Debug for Capacity {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Capacity").finish_non_exhaustive()
    }
}

/// A consumer has gone away; the unsent item is returned to its producer.
#[derive(Debug)]
pub struct SendError<T>(
    /// The item that was not delivered.
    pub T,
);

/// A clonable producer that waits for both a queue node and payload capacity.
pub struct Sender<T> {
    sender: mpsc::Sender<Queued<T>>,
    nodes: Arc<Semaphore>,
    bytes: Arc<Semaphore>,
    counts: Arc<Counts>,
    weight: fn(&T) -> usize,
}

impl<T> Clone for Sender<T> {
    fn clone(&self) -> Self {
        Self {
            sender: self.sender.clone(),
            nodes: Arc::clone(&self.nodes),
            bytes: Arc::clone(&self.bytes),
            counts: Arc::clone(&self.counts),
            weight: self.weight,
        }
    }
}

impl<T> std::fmt::Debug for Sender<T> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Sender")
            .field("usage", &self.usage())
            .finish()
    }
}

impl<T> Sender<T> {
    /// Waits without allocating another task or dropping a full-queue item.
    ///
    /// One atomic item larger than the ordinary burst budget is admitted exclusively. Its source must
    /// already bound an individual frame; no atomic native event is truncated to fit a transport queue.
    pub async fn send(&self, item: T) -> Result<(), SendError<T>> {
        let slot = match self.sender.reserve().await {
            Ok(slot) => slot,
            Err(_) => return Err(SendError(item)),
        };
        let node = match Arc::clone(&self.nodes).acquire_owned().await {
            Ok(node) => node,
            Err(_) => return Err(SendError(item)),
        };
        let weight = (self.weight)(&item).max(1);
        // The budget is four MiB, so its clamped permit count always fits the semaphore's u32 API.
        let Ok(permits) = u32::try_from(weight.min(QUEUE_BYTES)) else {
            return Err(SendError(item));
        };
        let bytes = match Arc::clone(&self.bytes).acquire_many_owned(permits).await {
            Ok(bytes) => bytes,
            Err(_) => return Err(SendError(item)),
        };
        self.counts.items.fetch_add(1, Ordering::AcqRel);
        let total = self
            .counts
            .bytes
            .fetch_add(weight, Ordering::AcqRel)
            .saturating_add(weight);
        self.counts.peak_bytes.fetch_max(total, Ordering::AcqRel);
        slot.send(Queued {
            item,
            _lease: Lease {
                _node: node,
                _bytes: bytes,
                counts: Arc::clone(&self.counts),
                weight,
            },
        });
        Ok(())
    }

    /// Whether no consumer can receive another item.
    #[must_use]
    pub fn is_closed(&self) -> bool {
        self.sender.is_closed()
    }

    /// Waits for consumer loss without polling or allocating another queue.
    pub async fn closed(&self) {
        self.sender.closed().await;
    }

    /// Observes retained queue payload without examining or copying its items.
    #[must_use]
    pub fn usage(&self) -> QueueUsage {
        QueueUsage {
            items: self.counts.items.load(Ordering::Acquire),
            bytes: self.counts.bytes.load(Ordering::Acquire),
            peak_bytes: self.counts.peak_bytes.load(Ordering::Acquire),
        }
    }
}

/// A single bounded consumer; ordinary receives release capacity and held receives carry it through delivery.
pub struct Receiver<T> {
    receiver: mpsc::Receiver<Queued<T>>,
    nodes: Arc<Semaphore>,
    bytes: Arc<Semaphore>,
}

impl<T> std::fmt::Debug for Receiver<T> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Receiver")
            .field("queued", &self.receiver.len())
            .field("closed", &self.receiver.is_closed())
            .finish()
    }
}

impl<T> Receiver<T> {
    /// Waits for the next item in native order.
    pub async fn recv(&mut self) -> Option<T> {
        self.receiver.recv().await.map(|queued| queued.item)
    }

    /// Receives one payload while retaining its queue capacity until the returned guard is dropped.
    pub async fn recv_held(&mut self) -> Option<(T, Capacity)> {
        self.receiver.recv().await.map(|queued| {
            (
                queued.item,
                Capacity {
                    _lease: queued._lease,
                },
            )
        })
    }

    /// Receives an immediately available payload and retains its capacity through consumer processing.
    pub fn try_recv_held(&mut self) -> Result<(T, Capacity), mpsc::error::TryRecvError> {
        self.receiver.try_recv().map(|queued| {
            (
                queued.item,
                Capacity {
                    _lease: queued._lease,
                },
            )
        })
    }

    /// Takes an immediately available item, releasing capacity as the ordinary receive does.
    pub fn try_recv(&mut self) -> Result<T, mpsc::error::TryRecvError> {
        self.receiver.try_recv().map(|queued| queued.item)
    }

    /// Takes one immediately available item; an ended stream is distinct from an empty live queue.
    pub fn try_next(&mut self) -> Result<Option<T>, mpsc::error::TryRecvError> {
        match self.receiver.try_recv() {
            Ok(queued) => Ok(Some(queued.item)),
            Err(mpsc::error::TryRecvError::Disconnected) => Ok(None),
            Err(error) => Err(error),
        }
    }

    /// Rejects new producers while preserving the already queued items for an ordered drain.
    pub fn close(&mut self) {
        self.receiver.close();
        self.nodes.close();
        self.bytes.close();
    }
}

impl<T: Unpin> Stream for Receiver<T> {
    type Item = T;
    fn poll_next(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<T>> {
        self.get_mut()
            .receiver
            .poll_recv(cx)
            .map(|item| item.map(|queued| queued.item))
    }
}

impl<T> Drop for Receiver<T> {
    fn drop(&mut self) {
        self.nodes.close();
        self.bytes.close();
    }
}

/// Creates a bounded queue; callers measure retained payload, including string and collection storage.
#[must_use]
pub fn channel<T>(weight: fn(&T) -> usize) -> (Sender<T>, Receiver<T>) {
    let (sender, receiver) = mpsc::channel(QUEUE_ITEMS);
    let nodes = Arc::new(Semaphore::new(QUEUE_ITEMS));
    let bytes = Arc::new(Semaphore::new(QUEUE_BYTES));
    (
        Sender {
            sender,
            nodes: Arc::clone(&nodes),
            bytes: Arc::clone(&bytes),
            counts: Arc::default(),
            weight,
        },
        Receiver {
            receiver,
            nodes,
            bytes,
        },
    )
}

/// Conservative retained JSON weight, including spare array capacity and tree-node overhead.
#[must_use]
pub fn json_weight(value: &orch_types::Json) -> usize {
    fn heap(value: &orch_types::Json) -> usize {
        match value {
            orch_types::Json::String(text) => text.capacity(),
            orch_types::Json::Array(items) => items
                .capacity()
                .saturating_mul(std::mem::size_of::<orch_types::Json>())
                .saturating_add(items.iter().map(heap).fold(0usize, usize::saturating_add)),
            orch_types::Json::Object(items) => {
                // A sparsely occupied tree node reserves more slots than its first key uses.
                const TREE_NODE_ALLOWANCE: usize = 1024;
                items
                    .iter()
                    .map(|(key, value)| {
                        TREE_NODE_ALLOWANCE
                            .saturating_add(key.capacity())
                            .saturating_add(heap(value))
                    })
                    .fold(0usize, usize::saturating_add)
            }
            _ => 0,
        }
    }
    std::mem::size_of::<orch_types::Json>().saturating_add(heap(value))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Debug)]
    struct Payload {
        body: Vec<u8>,
        live: Arc<AtomicUsize>,
    }
    impl Payload {
        fn new(bytes: usize, live: &Arc<AtomicUsize>) -> Self {
            live.fetch_add(bytes, Ordering::AcqRel);
            Self {
                body: vec![0; bytes],
                live: Arc::clone(live),
            }
        }
    }
    impl Drop for Payload {
        fn drop(&mut self) {
            self.live.fetch_sub(self.body.len(), Ordering::AcqRel);
        }
    }

    #[tokio::test]
    async fn a_stalled_consumer_bounds_live_payload_and_pushes_back_a_million_item_producer() {
        const ITEMS: usize = 1_000_000;
        const ITEM_BYTES: usize = 1024;
        let live = Arc::new(AtomicUsize::new(0));
        let (sender, mut receiver) = channel(|item: &Payload| item.body.len());
        let observed = sender.clone();
        let (full_tx, full_rx) = tokio::sync::oneshot::channel();
        let produced = Arc::new(AtomicUsize::new(0));
        let count = Arc::clone(&produced);
        let memory = Arc::clone(&live);
        let producer = tokio::spawn(async move {
            let mut full_tx = Some(full_tx);
            for index in 0..ITEMS {
                sender
                    .send(Payload::new(ITEM_BYTES, &memory))
                    .await
                    .unwrap();
                count.fetch_add(1, Ordering::AcqRel);
                if index + 1 == QUEUE_ITEMS {
                    full_tx.take().unwrap().send(()).unwrap();
                }
            }
        });
        full_rx.await.unwrap();
        tokio::task::yield_now().await;
        assert_eq!(produced.load(Ordering::Acquire), QUEUE_ITEMS);
        assert!(!producer.is_finished());
        assert!(live.load(Ordering::Acquire) <= (QUEUE_ITEMS + 1) * ITEM_BYTES);
        assert_eq!(observed.usage().items, QUEUE_ITEMS);
        for _ in 0..ITEMS {
            drop(receiver.recv().await.unwrap());
            assert!(live.load(Ordering::Acquire) <= (QUEUE_ITEMS + 1) * ITEM_BYTES);
        }
        producer.await.unwrap();
        assert_eq!(produced.load(Ordering::Acquire), ITEMS);
        assert_eq!(live.load(Ordering::Acquire), 0);
    }

    #[tokio::test]
    async fn payload_bytes_stop_a_queue_before_its_node_limit_and_closed_consumers_wake_producers()
    {
        const ITEM_BYTES: usize = 1024 * 1024;
        let live = Arc::new(AtomicUsize::new(0));
        let (sender, receiver) = channel(|item: &Payload| item.body.len());
        for _ in 0..QUEUE_BYTES / ITEM_BYTES {
            sender.send(Payload::new(ITEM_BYTES, &live)).await.unwrap();
        }
        let another = sender.clone();
        let memory = Arc::clone(&live);
        let producer =
            tokio::spawn(async move { another.send(Payload::new(ITEM_BYTES, &memory)).await });
        tokio::task::yield_now().await;
        assert!(!producer.is_finished());
        assert_eq!(sender.usage().bytes, QUEUE_BYTES);
        assert!(sender.usage().items < QUEUE_ITEMS);
        assert!(live.load(Ordering::Acquire) <= QUEUE_BYTES + ITEM_BYTES);
        drop(receiver);
        drop(producer.await.unwrap().unwrap_err());
        assert_eq!(live.load(Ordering::Acquire), 0);
    }

    #[tokio::test]
    async fn a_held_delivery_batch_pushes_back_a_million_item_producer_until_delivery_finishes() {
        const ITEMS: usize = 1_000_000;
        const ITEM_BYTES: usize = 1024;
        let live = Arc::new(AtomicUsize::new(0));
        let produced = Arc::new(AtomicUsize::new(0));
        let (sender, mut receiver) = channel(|item: &Payload| item.body.len());
        let observed = sender.clone();
        let memory = Arc::clone(&live);
        let count = Arc::clone(&produced);
        let producer = tokio::spawn(async move {
            for _ in 0..ITEMS {
                sender
                    .send(Payload::new(ITEM_BYTES, &memory))
                    .await
                    .unwrap();
                count.fetch_add(1, Ordering::AcqRel);
            }
        });
        let mut batch = Vec::new();
        let mut capacity = Vec::new();
        for _ in 0..QUEUE_ITEMS {
            let (item, guard) = receiver.recv_held().await.unwrap();
            batch.push(item);
            capacity.push(guard);
        }
        tokio::task::yield_now().await;
        assert_eq!(produced.load(Ordering::Acquire), QUEUE_ITEMS);
        assert_eq!(observed.usage().items, QUEUE_ITEMS);
        assert_eq!(observed.usage().bytes, QUEUE_ITEMS * ITEM_BYTES);
        assert!(live.load(Ordering::Acquire) <= (QUEUE_ITEMS + 1) * ITEM_BYTES);
        assert!(!producer.is_finished());
        drop(batch);
        drop(capacity);
        for _ in QUEUE_ITEMS..ITEMS {
            drop(receiver.recv().await.unwrap());
            assert!(live.load(Ordering::Acquire) <= (QUEUE_ITEMS + 1) * ITEM_BYTES);
        }
        producer.await.unwrap();
        assert_eq!(produced.load(Ordering::Acquire), ITEMS);
        assert_eq!(live.load(Ordering::Acquire), 0);
    }
}
