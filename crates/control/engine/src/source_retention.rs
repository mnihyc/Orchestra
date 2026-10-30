//! Expired recorded-state metadata and temporary plans release pins while native journals keep recovery roots.

use std::collections::BTreeMap;

use orch_db::queries::{job_leases, source, storage};
use orch_link_contract::commands::SerialKey;
use orch_link_contract::source::FsRecordPrune;
use orch_types::{Hash, Id, Json, Time, new_id};

use crate::{Engine, EngineError, commands::Outgoing};

/// Uses the ordinary scheduler's page size so a single SQL statement never holds every state row.
const PAGE: i64 = 200;
/// Native collection uses the existing short machine-command expiry rather than holding an offline sweep.
const PRUNE_MINUTES: i64 = 5;

fn hash(bytes: &[u8]) -> Result<Hash, EngineError> {
    Hash::from_slice(bytes)
        .map_err(|_| EngineError::unreadable("a retained source hash is not 32 bytes"))
}

impl Engine {
    /// The same held clock and job lease govern startup and later expiry; offline stores retry next time. A
    /// lease and not a lock kept in an open transaction: the run commits page by page and waits on machines.
    pub(crate) async fn keep_file_states(&self) -> Result<(), EngineError> {
        let lease = job_leases::Lease::for_run("source.file-states", crate::schedule::RUN_LEASE);
        if !lease.hold(self.db(), self.inner.clock.now()).await? {
            return Ok(());
        }
        let kept = self.keep_file_states_held(&lease).await;
        lease.release(self.db()).await?;
        kept
    }

    /// Renews `lease` between pages and machines, and stops where another process took it after it lapsed.
    async fn keep_file_states_held(&self, lease: &job_leases::Lease) -> Result<(), EngineError> {
        let now = self.inner.clock.now();
        self.invalidate_work().await?;
        // A process can end after the fork became terminal but before its post-job pin release.
        // The job row is the receipt; release_pins locks it and records the release atomically.
        let mut after_job: Option<Id> = None;
        loop {
            let ids: Vec<Id> = {
                let mut conn = self.db().acquire().await?;
                sqlx::query_scalar("SELECT id FROM jobs WHERE state IN ('succeeded','failed','cancelled')
                    AND ((plan ? 'fork_file_pins' AND plan->>'fork_file_pins_released' IS DISTINCT FROM 'true')
                         OR (plan ? 'portable' AND plan ? 'move_pins' AND plan->>'move_pins_released' IS DISTINCT FROM 'true'))
                    AND ($1::uuid IS NULL OR id > $1) ORDER BY id LIMIT $2")
                    .bind(after_job).bind(PAGE).fetch_all(&mut *conn).await.map_err(orch_db::DbError::from)?
            };
            if ids.is_empty() {
                break;
            }
            after_job = ids.last().copied();
            for job in ids {
                let row = {
                    let mut conn = self.db().acquire().await?;
                    orch_db::queries::jobs::job(&mut conn, job).await?
                };
                if let Some(row) = row.filter(|row| row.plan.get("portable").is_some()) {
                    self.after_machine_move(&row).await;
                } else {
                    crate::fork_source::release_pins(self, job, now).await?;
                }
            }
            if !lease.hold(self.db(), self.inner.clock.now()).await? {
                return Ok(());
            }
        }
        let mut after: Option<Id> = None;
        let mut pruning = BTreeMap::<Id, Vec<Id>>::new();
        loop {
            let rows: Vec<(Id, Id)> = {
                let mut conn = self.db().acquire().await?;
                sqlx::query_as("SELECT s.id, s.environment_id FROM snapshots s WHERE s.storage = 'environment'
                    AND s.state <> 'running' AND s.expires_at <= $1 AND ($2::uuid IS NULL OR s.id > $2)
                    AND NOT EXISTS (SELECT 1 FROM jobs j WHERE (j.state IN ('queued','running')
                        OR j.kind = 'workspace_restore' AND j.plan->>'restore_recovery_required' = 'true') AND (
                        j.plan#>>'{forkplan,files_at,id}' = s.id::text OR j.plan#>>'{action,body,snapshot_id}' = s.id::text
                        OR j.plan#>>'{action,body,undo_snapshot_id}' = s.id::text OR j.plan->>'fork_source_snapshot' = s.id::text))
                    ORDER BY s.id LIMIT $3")
                    .bind(now).bind(after).bind(PAGE).fetch_all(&mut *conn).await.map_err(orch_db::DbError::from)?
            };
            if rows.is_empty() {
                break;
            }
            after = rows.last().map(|(id, _)| *id);
            let mut tx = self.db().begin().await?;
            for (snapshot, environment) in rows {
                if let Some(Some(manifest)) =
                    source::expire_recorded(&mut tx, snapshot, now).await?
                {
                    storage::unpin_blobs(&mut tx, &[manifest], now).await?;
                }
                pruning.entry(environment).or_default().push(snapshot);
            }
            tx.commit().await.map_err(orch_db::DbError::from)?;
            if !lease.hold(self.db(), self.inner.clock.now()).await? {
                return Ok(());
            }
        }
        // End namespace facts stay in the turn; the separately uploaded manifest need not outlive
        // its immutable before-state deadline. Detachment and pin release share one transaction.
        let mut tx = self.db().begin().await?;
        let expired_ends: Vec<Vec<u8>> = sqlx::query_scalar("WITH due AS (
            SELECT t.id, t.end_manifest_blob FROM turns t JOIN snapshots s ON s.id = t.before_snapshot_id
             WHERE t.end_manifest_blob IS NOT NULL AND t.completed_at IS NOT NULL AND s.expires_at <= $1 FOR UPDATE OF t
            ) UPDATE turns t SET end_manifest_blob = NULL FROM due d WHERE t.id = d.id RETURNING d.end_manifest_blob")
            .bind(now).fetch_all(&mut *tx).await.map_err(orch_db::DbError::from)?;
        let hashes = expired_ends
            .iter()
            .map(|bytes| hash(bytes))
            .collect::<Result<Vec<_>, _>>()?;
        // Each turn owns one pin, so equal hashes remain repeated in this list intentionally.
        for hash in hashes {
            storage::unpin_blobs(&mut tx, &[hash], now).await?;
        }
        let plans: Vec<Json> = sqlx::query_scalar("DELETE FROM preflights WHERE kind = 'workspace_restore' AND expires_at <= $1 RETURNING plan")
            .bind(now).fetch_all(&mut *tx).await.map_err(orch_db::DbError::from)?;
        for plan in plans {
            if let Some(blob) = plan.get("plan_blob") {
                let hash: Hash = serde_json::from_value(blob.clone())?;
                storage::unpin_blobs(&mut tx, &[hash], now).await?;
            }
        }
        tx.commit().await.map_err(orch_db::DbError::from)?;
        // Native metadata also finds expired roots whose old thread rows were purged, or whose
        // home was explicitly carried with a fork. Those ids need not survive in PostgreSQL.
        let mut conn = self.db().acquire().await?;
        let live: Vec<Id> = sqlx::query_scalar("SELECT id FROM environments WHERE status = 'running' AND deleted_at IS NULL ORDER BY id")
            .fetch_all(&mut *conn).await.map_err(orch_db::DbError::from)?;
        drop(conn);
        for environment in live {
            pruning.entry(environment).or_default();
        }
        for (environment, snapshots) in pruning {
            if self.is_stopped() || !lease.hold(self.db(), self.inner.clock.now()).await? {
                break;
            }
            let mut conn = self.db().acquire().await?;
            let row =
                orch_db::queries::environments::thread_engine::environment(&mut conn, environment)
                    .await?;
            let Some(machine) = row
                .filter(|row| row.status == "running")
                .and_then(|row| row.machine_id)
            else {
                continue;
            };
            drop(conn);
            let batches = if snapshots.is_empty() {
                vec![Vec::new()]
            } else {
                snapshots
                    .chunks(PAGE as usize)
                    .map(<[Id]>::to_vec)
                    .collect::<Vec<_>>()
            };
            for ids in batches {
                let command = Outgoing::new(
                    new_id(),
                    machine,
                    SerialKey::Environment(environment),
                    &FsRecordPrune {
                        environment_id: environment,
                        expired_snapshot_ids: ids,
                    },
                )?
                .by(Time::new(now + chrono::Duration::minutes(PRUNE_MINUTES)));
                if let Err(error) = self.ask_bounded(command).await {
                    tracing::warn!(%environment, %error, "expired recorded objects await their next native sweep");
                }
            }
        }
        Ok(())
    }
}
