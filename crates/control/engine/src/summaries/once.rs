//! What one effect-free read of summaries has already decided.
//!
//! A read is one moment of one snapshot. Its views ask the same questions of the same rows again: whether the reader
//! may read the chat they are about (for its catch-up, its title, their ranges and their stored paragraphs), and
//! whether the reader may use the place's model (for each view's default, route and proposal). Inside [`deciding`],
//! each of these is decided once for its caller and kept for the rest of the read; outside it nothing is kept.

use super::resource::Reason;
use orch_authz::Caller;
use orch_types::Id;
use std::cell::RefCell;
use std::collections::HashMap;
use std::future::Future;

/// One model choice for one caller at one place: everything its judgment depends on besides the snapshot.
#[derive(PartialEq, Eq, Hash)]
pub(super) struct ModelChoice {
    pub(super) caller: Caller,
    pub(super) place: Option<Id>,
    pub(super) backend: String,
    pub(super) params: String,
}

#[derive(Default)]
struct Decided {
    models: HashMap<ModelChoice, Result<Option<Id>, Reason>>,
    /// For each caller and chat: whether its ordinary Read opens the chat, and if so whether only as an inspection.
    chats: HashMap<(Caller, Id), Option<bool>>,
    /// For each caller and provider: whether its ordinary View holds.
    providers: HashMap<(Caller, Id), bool>,
}

tokio::task_local! {
    static DECIDED: RefCell<Decided>;
}

/// Runs one effect-free read so that what it asks again is decided once. `work` must not write; inside a read that
/// is already deciding once, it shares that read's decisions.
pub async fn deciding<T>(work: impl Future<Output = T>) -> T {
    if DECIDED.try_with(|_| ()).is_ok() {
        return work.await;
    }
    DECIDED.scope(RefCell::new(Decided::default()), work).await
}

pub(super) fn model(choice: &ModelChoice) -> Option<Result<Option<Id>, Reason>> {
    DECIDED
        .try_with(|decided| decided.borrow().models.get(choice).copied())
        .ok()
        .flatten()
}

pub(super) fn keep_model(choice: ModelChoice, judged: Result<Option<Id>, Reason>) {
    let _ = DECIDED.try_with(|decided| decided.borrow_mut().models.insert(choice, judged));
}

pub(super) fn chat(caller: &Caller, chat: Id) -> Option<Option<bool>> {
    DECIDED
        .try_with(|decided| decided.borrow().chats.get(&(caller.clone(), chat)).copied())
        .ok()
        .flatten()
}

/// Whether the caller's ordinary View of a provider holds, as already decided in this read.
pub fn provider_view(caller: &Caller, provider: Id) -> Option<bool> {
    DECIDED
        .try_with(|decided| {
            decided
                .borrow()
                .providers
                .get(&(caller.clone(), provider))
                .copied()
        })
        .ok()
        .flatten()
}

/// Keeps whether the caller's ordinary View of a provider holds, for the rest of a read deciding once.
pub fn keep_provider_view(caller: &Caller, provider: Id, viewable: bool) {
    let _ = DECIDED.try_with(|decided| {
        decided
            .borrow_mut()
            .providers
            .insert((caller.clone(), provider), viewable)
    });
}

pub(super) fn keep_chat(caller: &Caller, chat: Id, opened: Option<bool>) {
    let _ = DECIDED.try_with(|decided| {
        decided
            .borrow_mut()
            .chats
            .insert((caller.clone(), chat), opened)
    });
}
