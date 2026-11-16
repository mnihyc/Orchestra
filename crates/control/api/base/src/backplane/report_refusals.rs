//! Body-free machine refusal counts and hourly system audit publication.

use super::Link;

use chrono::DateTime;

use chrono::Utc;

use orch_db::queries::machine_refusals;

use orch_db::queries::machine_reports;

use orch_db::Db;

use orch_db::DbError;

use orch_link_contract::streams::Stream;

use orch_link_contract::streams::StreamFamily;

use orch_types::Id;

use std::collections::HashMap;

use tokio::sync::Mutex;
#[derive(Default)]
struct Count {
    total: i64,
    pending: i64,
    last_audit: Option<DateTime<Utc>>,
}

#[derive(Default)]
pub struct Refusals {
    counts: Mutex<HashMap<(Id, StreamFamily), Count>>,
}

impl Refusals {
    async fn record(
        &self,
        db: &Db,
        machine: Id,
        kind: StreamFamily,
        count: i64,
        at: DateTime<Utc>,
    ) -> Result<(), DbError> {
        if count <= 0 {
            return Ok(());
        }
        let mut counts = self.counts.lock().await;
        let state = counts.entry((machine, kind)).or_default();
        state.total = state.total.saturating_add(count);
        state.pending = state.pending.saturating_add(count);
        tracing::warn!(%machine, stream_kind=kind.prefix(), count=state.total,
            "reports from another machine were refused");
        if state
            .last_audit
            .is_some_and(|last| at - last < machine_refusals::AUDIT_INTERVAL)
        {
            return Ok(());
        }
        let mut tx = db.begin().await?;
        let (written, last) =
            machine_refusals::record(&mut tx, machine, kind.prefix(), state.pending, at).await?;
        tx.commit().await.map_err(DbError::from)?;
        state.last_audit = Some(last);
        if written {
            state.pending = 0;
        }
        Ok(())
    }

    #[cfg(any(test, feature = "test-support"))]
    pub(super) async fn total(&self, machine: Id, kind: StreamFamily) -> i64 {
        self.counts
            .lock()
            .await
            .get(&(machine, kind))
            .map_or(0, |count| count.total)
    }
}

impl Link {
    /// Whether `stream` is an environment's, and `machine` is the unit that enrolled for it before its row names
    /// that unit. What the unit reports then waits, unanswered so that it is sent again: it is no other machine's
    /// report, and is neither refused nor counted.
    pub(super) async fn awaits_its_environment(&self, machine: Id, stream: &Stream) -> bool {
        let Stream::Env { environment_id } = *stream else {
            return false;
        };
        let waits: Result<bool, DbError> = async {
            let mut conn = self.inner.db.acquire().await?;
            machine_reports::awaits_attachment(&mut conn, machine, environment_id).await
        }
        .await;
        waits.unwrap_or(false)
    }

    pub async fn record_report_refusal(&self, machine: Id, stream: &Stream, count: usize) {
        let count = i64::try_from(count).unwrap_or(i64::MAX);
        if let Err(error) = self
            .inner
            .refusals
            .record(
                &self.inner.db,
                machine,
                stream.family(),
                count,
                self.inner.clock.now(),
            )
            .await
        {
            tracing::warn!(%machine, stream_kind=stream.family().prefix(), %error,
                "a machine report refusal could not be audited");
        }
    }
}
