//! Internal wake hints share the sequenced source registry, not any public feed's admission.

use super::Entry;

use super::Inner;

use super::Realtime;

use super::Source;

use orch_types::Seq;

use std::sync::Arc;

use tokio::sync::watch;
pub struct Changes {
    inner: Arc<Inner>,
    source: Source,
    receiver: watch::Receiver<Option<Seq>>,
}

impl Realtime {
    pub fn source_changes(&self, source: Source) -> Changes {
        let receiver = self
            .inner
            .registry()
            .sources
            .entry(source)
            .or_insert_with(Entry::new)
            .changes
            .subscribe();
        Changes {
            inner: Arc::clone(&self.inner),
            source,
            receiver,
        }
    }
}

impl Changes {
    /// Consume hints BEFORE reading current rows, so a concurrent commit cannot be lost.
    pub fn acknowledge(&mut self) {
        self.receiver.borrow_and_update();
    }

    pub async fn changed(&mut self) -> Result<(), watch::error::RecvError> {
        self.receiver.changed().await
    }
}

impl Drop for Changes {
    fn drop(&mut self) {
        let mut registry = self.inner.registry();
        if registry.sources.get(&self.source).is_some_and(|entry| {
            entry.subs.is_empty() && !entry.running && entry.changes.receiver_count() == 1
        }) {
            registry.sources.remove(&self.source);
        }
    }
}

impl Inner {
    /// Identity changes have no source sequence but must wake that person's current readers.
    pub(crate) fn invalidate_source(&self, source: Source) {
        if let Some(entry) = self.registry().sources.get(&source) {
            entry.changes.send_modify(|_| {});
        }
    }
}
