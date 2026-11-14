//! An opt-in observer of the source read, scoped to the future a case drives.
use futures::future::BoxFuture;
use orch_types::Id;
use std::{future::Future, sync::Arc};

type Observer = Arc<dyn Fn(Id) -> BoxFuture<'static, ()> + Send + Sync>;
tokio::task_local! { static OBSERVER: Observer; }

/// Observe the exact source-read boundary without a timer, log subscriber or global gate.
pub async fn follow<T>(
    observer: impl Fn(Id) -> BoxFuture<'static, ()> + Send + Sync + 'static,
    work: impl Future<Output = T>,
) -> T {
    OBSERVER.scope(Arc::new(observer), work).await
}

pub(super) async fn source_read(request: Id) {
    if let Ok(observer) = OBSERVER.try_with(Arc::clone) {
        observer(request).await;
    }
}
