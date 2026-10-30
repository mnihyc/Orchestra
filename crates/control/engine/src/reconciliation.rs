//! Recover turns, sessions, jobs, locks and leases.
//!
//! The durable stream remains the account of a turn already running. Startup wakes existing work
//! through the same drivers as a normal sweep; phased jobs keep their stored identity and phase,
//! and terminal turns close their stale leases before their machines are told. No recovery path
//! allocates another turn, job or grant just because the API process changed.

use crate::{Engine, EngineError};

impl Engine {
    /// Restores supervision of existing work; the caller may repeat this without duplicating it.
    pub async fn reconcile_startup(&self) -> Result<(), EngineError> {
        self.close_stale_leases().await?;
        let mut after = None;
        loop {
            let mut conn = self.db().acquire().await?;
            let threads = orch_db::queries::reconciliation::threads(
                &mut conn,
                after,
                self.inner.config.sweep_batch,
            )
            .await?;
            drop(conn);
            let Some(last) = threads.last().copied() else {
                break;
            };
            for thread in threads {
                tracing::debug!(target:"orch_engine::terminal_work",%thread,phase="startup",outcome="enter",stopped=self.is_stopped());
                self.wake(thread);
            }
            after = Some(last);
        }
        Ok(())
    }
}
