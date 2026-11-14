//! Reuse only the source proof on the transaction that just checked its actual insertion.
use super::*;
use std::future::Future;

/// Private, in-memory and scoped to one awaited post-insertion check; never a durable grant.
pub(crate) struct Check {
    source: Authority,
    command: Id,
    transaction: i64,
    callers: Vec<(Option<Caller>, Option<Time>)>,
}

tokio::task_local! { static CHECK: Check; }

impl Check {
    pub(crate) async fn capture(
        conn: &mut Conn,
        source: &Authority,
        command: Id,
        callers: &[(Option<Caller>, Option<Time>)],
    ) -> Result<Self, EngineError> {
        if !orch_db::queries::transaction_guard::in_transaction(conn) {
            return Err(EngineError::unreadable(
                "a summary's issue check requires its transaction",
            ));
        }
        Ok(Self {
            source: source.clone(),
            command,
            transaction: orch_db::queries::transaction_guard::current_transaction_id(conn).await?,
            callers: callers.to_vec(),
        })
    }

    pub(crate) async fn scope<T>(self, work: impl Future<Output = T>) -> T {
        CHECK.scope(self, work).await
    }
}

pub(super) struct CheckedCallers {
    pub(super) transaction: i64,
    pub(super) callers: Vec<(Option<Caller>, Option<Time>)>,
}

pub(super) fn callers(source: &Authority, command: Id) -> Option<CheckedCallers> {
    CHECK
        .try_with(|check| {
            (check.source == *source && check.command == command).then(|| CheckedCallers {
                transaction: check.transaction,
                callers: check.callers.clone(),
            })
        })
        .ok()
        .flatten()
}
