//! What one effect-free read of summaries has already decided.
//!
//! A read is one moment of one snapshot. Its views ask the same questions of the same rows again: whether the reader
//! may read the chat they are about (for its catch-up, its title, their ranges and their stored paragraphs), and
//! whether the reader may use the place's model (for each view's default, route and proposal). Inside [`deciding`],
//! each of these is decided once for its caller and kept for the rest of the read; outside it nothing is kept.

use super::resource::Reason;
use crate::EngineError;
use orch_authz::Caller;
use orch_authz::usable::Usable;
use orch_db::Conn;
use orch_db::queries::resources::thread_engine::{self as resources, ModelRow, ProviderRow};
use orch_types::Id;
use std::cell::RefCell;
use std::collections::HashMap;
use std::future::Future;
use std::hash::Hash;

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
    /// The rows a read's model judgments read, and what is decided of a provider at a place: a place's default
    /// model judges every model of every provider until one passes, once for each of the views that ask for it.
    provider_rows: HashMap<Id, Option<ProviderRow>>,
    model_rows: HashMap<(Id, String), Option<ModelRow>>,
    allowed: HashMap<Id, Option<Vec<String>>>,
    forbidden: HashMap<(Id, Id), bool>,
    usable: HashMap<(Caller, Option<Id>, Id), Usable>,
    keepers: HashMap<Id, bool>,
    candidates: HashMap<Id, std::sync::Arc<super::resource::Candidates>>,
}

/// What `read` answers for `key`, read once in a read that is deciding once (`kept` says where it is kept).
async fn once<K, V, F>(
    kept: fn(&mut Decided) -> &mut HashMap<K, V>,
    key: K,
    read: F,
) -> Result<V, EngineError>
where
    K: Eq + Hash,
    V: Clone,
    F: Future<Output = Result<V, EngineError>>,
{
    if let Some(known) = DECIDED
        .try_with(|decided| kept(&mut decided.borrow_mut()).get(&key).cloned())
        .ok()
        .flatten()
    {
        return Ok(known);
    }
    let value = read.await?;
    let _ = DECIDED.try_with(|decided| kept(&mut decided.borrow_mut()).insert(key, value.clone()));
    Ok(value)
}

/// The provider `id` (deleted or not), as this read first read it.
pub(super) async fn provider_row(
    conn: &mut Conn,
    id: Id,
) -> Result<Option<ProviderRow>, EngineError> {
    once(|decided| &mut decided.provider_rows, id, async {
        Ok::<_, EngineError>(resources::provider(conn, id).await?)
    })
    .await
}

/// The model `name` of `provider`, as this read first read it.
pub(super) async fn model_row(
    conn: &mut Conn,
    provider: Id,
    name: &str,
) -> Result<Option<ModelRow>, EngineError> {
    once(
        |decided| &mut decided.model_rows,
        (provider, name.to_owned()),
        async { Ok::<_, EngineError>(resources::model(conn, provider, name).await?) },
    )
    .await
}

/// Keeps rows read together for the rest of a read deciding once.
pub(super) fn keep_rows(
    providers: impl IntoIterator<Item = (Id, Option<ProviderRow>)>,
    models: impl IntoIterator<Item = ((Id, String), Option<ModelRow>)>,
) {
    let _ = DECIDED.try_with(|decided| {
        let mut decided = decided.borrow_mut();
        decided.provider_rows.extend(providers);
        decided.model_rows.extend(models);
    });
}

/// The models allowed at `place`, as this read first resolved them.
pub(super) async fn models_allowed(
    conn: &mut Conn,
    place: Id,
) -> Result<Option<Vec<String>>, EngineError> {
    once(
        |decided| &mut decided.allowed,
        place,
        crate::resolved::models_allowed(conn, place),
    )
    .await
}

/// Whether `place` forbids `provider`, as this read first decided it.
pub(super) async fn forbidden(
    conn: &mut Conn,
    place: Id,
    provider: Id,
) -> Result<bool, EngineError> {
    once(|decided| &mut decided.forbidden, (place, provider), async {
        Ok::<_, EngineError>(
            crate::provider_policy::forbidden(conn, place, provider)
                .await?
                .is_some(),
        )
    })
    .await
}

/// The caller's use of `provider` at `place`, as this read first decided it.
pub(super) async fn usable(
    conn: &mut Conn,
    principal: &orch_authz::Principal,
    provider: Id,
    place: Option<Id>,
) -> Result<Usable, EngineError> {
    once(
        |decided| &mut decided.usable,
        (principal.caller().clone(), place, provider),
        async {
            let policies = crate::access::resource_policies(conn, provider).await?;
            Ok::<_, EngineError>(
                orch_authz::usable::usable(conn, principal, provider, place, &policies).await?,
            )
        },
    )
    .await
}

/// Whether the machine that keeps a key is online, as this read first read it.
pub(super) async fn keeper_online(conn: &mut Conn, machine: Id) -> Result<bool, EngineError> {
    once(|decided| &mut decided.keepers, machine, async {
        Ok::<_, EngineError>(
            orch_db::queries::resources::machine(conn, machine)
                .await?
                .is_some_and(|machine| machine.online),
        )
    })
    .await
}

/// A place's default model candidates, as this read first read them.
pub(super) async fn candidates<F>(
    place: Id,
    read: F,
) -> Result<std::sync::Arc<super::resource::Candidates>, EngineError>
where
    F: Future<Output = Result<std::sync::Arc<super::resource::Candidates>, EngineError>>,
{
    once(|decided| &mut decided.candidates, place, read).await
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
