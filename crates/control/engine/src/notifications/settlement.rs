//! Approval recipients settle from a private receipt, outside every thread and approval transaction.

use crate::engine::{Engine, lock};
use crate::error::EngineError;
use orch_db::queries::notifications::inbox;
use orch_types::Id;

impl Engine {
    pub(crate) async fn keep_approval_notifications(&self) -> Result<(), EngineError> {
        let pending = {
            let mut conn = self.inner.db.acquire().await?;
            inbox::pending_approval_settlements(&mut conn, self.inner.config.sweep_batch).await?
        };
        for receipt in pending {
            if self.is_stopped() || !lock(&self.inner.approval_notifications).insert(receipt) {
                continue;
            }
            let engine = self.clone();
            tokio::spawn(async move {
                if let Err(error) = engine.settle_approval_notifications(receipt).await {
                    tracing::warn!(%error, %receipt, "approval recipients await their next settlement attempt");
                }
                lock(&engine.inner.approval_notifications).remove(&receipt);
            });
        }
        Ok(())
    }

    async fn settle_approval_notifications(&self, receipt: Id) -> Result<(), EngineError> {
        let mut tx = self.inner.db.begin().await?;
        let Some(work) = inbox::lock_approval_settlement(&mut tx, receipt).await? else {
            return Ok(());
        };
        inbox::settle_approval(&mut tx, work.approval_id, work.settled_at).await?;
        inbox::finish_approval_settlement(&mut tx, work.id, self.inner.clock.now()).await?;
        tx.commit().await.map_err(orch_db::DbError::from)?;
        Ok(())
    }
}
