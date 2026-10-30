//! Host-owned finalization of an exact retired attachment, independent of an RPC's lifetime.

use super::{DIR_WATCHES_MAX, Environment, HostInner, lock};
use crate::directory_pages::DirectoryPages;
use crate::error::{HostError, Result};
use crate::state_store::DestructiveLease;
use orch_types::Id;
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::oneshot;

#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum Kind {
    Detach,
    Replacement,
}

struct State {
    task: Option<tokio::task::JoinHandle<Result<()>>>,
    outcome: Option<std::result::Result<(), String>>,
}

pub(super) struct Finalizer {
    pub(super) attachment: Id,
    pub(super) kind: Kind,
    state: tokio::sync::Mutex<State>,
}

impl Finalizer {
    /// Called only after removal/replacement commits, under the live and retiring map locks.
    /// The latch lets a replacement start its new background tasks before stopping the old ones.
    /// A cancelled/dropped latch still runs this already committed cleanup.
    pub(super) fn launch(
        host: &Arc<HostInner>,
        environment: Arc<Environment>,
        kind: Kind,
        store: Option<Arc<DestructiveLease>>,
    ) -> (Arc<Self>, oneshot::Sender<()>) {
        let id = environment.id;
        let attachment = environment.directory_attachment;
        let pages = Arc::clone(&host.directory_pages);
        let host = Arc::downgrade(host);
        let (start, ready) = oneshot::channel();
        let task = tokio::spawn(async move {
            let _ = ready.await;
            // finish owns/drops the old Environment and lease before successful removal below.
            let outcome = finish(environment, pages, kind, store).await;
            if outcome.is_ok()
                && let Some(host) = host.upgrade()
            {
                let mut retiring = lock(&host.retiring);
                if retiring
                    .get(&id)
                    .is_some_and(|pending| pending.attachment == attachment)
                {
                    retiring.remove(&id);
                }
            }
            // Failure stays host-reachable: an absent live entry must never imply successful
            // cleanup after a failed join or unexpected finalizer panic.
            outcome
        });
        let finalizer = Arc::new(Self {
            attachment,
            kind,
            state: tokio::sync::Mutex::new(State {
                task: Some(task),
                outcome: None,
            }),
        });
        (finalizer, start)
    }

    pub(super) async fn wait(&self) -> Result<()> {
        let mut state = self.state.lock().await;
        if let Some(task) = state.task.as_mut() {
            // Await by reference: cancellation leaves the same actual task handle available
            // to the next public retry. The task is never aborted by an RPC or its timeout.
            let outcome = match task.await {
                Ok(result) => result,
                Err(error) => Err(HostError::Internal(format!(
                    "the retired attachment finalizer failed: {error}"
                ))),
            };
            state.task = None;
            state.outcome = Some(match &outcome {
                Ok(()) => Ok(()),
                Err(error) => Err(error.to_string()),
            });
            return outcome;
        }
        match state.outcome.as_ref() {
            Some(Ok(())) => Ok(()),
            // Finalizer failures are join/internal failures; retain their words across retries.
            Some(Err(message)) => Err(HostError::Internal(message.clone())),
            None => Err(HostError::Internal(
                "the retired attachment has no finalization outcome".to_owned(),
            )),
        }
    }
}

/// One existing host directory-owner allowance, not an unbounded retired-attachment history.
pub(super) fn require_room(retiring: &HashMap<Id, Arc<Finalizer>>, id: Id) -> Result<()> {
    if retiring.contains_key(&id) {
        return Err(HostError::Busy(
            "the previous attachment is still being closed; try again after it finishes".to_owned(),
        ));
    }
    if retiring.len() >= DIR_WATCHES_MAX {
        return Err(HostError::Busy(
            "the host has no unused attachment finalizer owner".to_owned(),
        ));
    }
    Ok(())
}

async fn finish(
    environment: Arc<Environment>,
    pages: Arc<DirectoryPages>,
    kind: Kind,
    store: Option<Arc<DestructiveLease>>,
) -> Result<()> {
    let _store = store;
    let reads = environment.directory_operations.join().await;
    let captures = pages
        .join_attachment(environment.id, environment.directory_attachment)
        .await;
    environment.stop_tasks();
    if kind == Kind::Detach {
        environment.browsers.close_all().await;
        environment.displays.close_all().await;
        environment.windows.stop_all().await;
        let _ = environment.terminals.close_all().await;
        environment.mcp_host.stop_all().await;
        environment.executors.stop_all().await;
        if let Some(group) = &environment.resource_group {
            group.close().await;
        }
    }
    // Preserve cleanup after a metadata join error. Replacement services belong to the new
    // attachment, so only old background tasks are stopped for that kind.
    reads.and(captures)
}
