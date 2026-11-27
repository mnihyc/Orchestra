//! Machine-dependent expiry, idle, retention and summary schedules, the start of a stopped provider's runtime to renew its sign-in, the shells that nobody came back to, the samples of the API's, the gateway's and the hosts' metrics, and the reaper's tick.
//!
//! Retention is an engine action because putting work away must commit its native controls in the
//! same transaction. Each node is rechecked under the authority gate and committed separately.

use crate::engine::{Engine, lock};
use crate::error::EngineError;
use orch_access_contract::audit::AuditAction;
use orch_db::audit::{self, Actor, Entry, Origin};
use orch_db::queries::{hierarchy, job_leases, lifecycle, transaction_guard};
use orch_settings::{Place, Snapshot, Wanted};
use orch_settings_contract::registry::{self, keys};
use orch_types::{Id, Time};
use serde_json::json;
use tokio::time::Instant;

/// The ordinary sweep budget bounds each candidate read without truncating a complete run.
const BATCH: i64 = 200;

/// How long a run's lease on a paged job holds before the run renews it between pages: ten minutes, far
/// longer than one page takes, and how soon a run that died leaves the job to another process.
pub(crate) const RUN_LEASE: chrono::Duration = chrono::Duration::minutes(10);

/// Changes committed by one retention run.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct Retained {
    /// Topics put away because nobody was working in them.
    pub archived: u64,
    /// Archived nodes soft-deleted with their recoverable descendants.
    pub deleted: u64,
}

fn old_enough(
    at: chrono::DateTime<chrono::Utc>,
    now: chrono::DateTime<chrono::Utc>,
    days: i64,
) -> bool {
    days > 0
        && chrono::Duration::try_days(days)
            .and_then(|age| now.checked_sub_signed(age))
            .is_some_and(|cutoff| at <= cutoff)
}

impl Engine {
    /// The supervisor's paced retention run, including its first startup sweep.
    pub(crate) async fn keep_retention(&self) {
        let due = {
            let mut last = lock(&self.inner.retained);
            if last.is_some_and(|at| at.elapsed() < self.inner.config.retention_every) {
                false
            } else {
                *last = Some(Instant::now());
                true
            }
        };
        if due && let Err(error) = self.run_retention().await {
            tracing::warn!(%error,"node retention will be examined again at its next interval");
        }
    }

    /// Runs the explicit retention policy once. Another process holding its lease makes this a no-op.
    /// Zero-valued policies never act; orgs and side lanes are not independent retention targets.
    pub async fn run_retention(&self) -> Result<Retained, EngineError> {
        // A lease, not a lock kept in an open transaction: each node commits on its own.
        let lease = job_leases::Lease::for_run("retention", RUN_LEASE);
        if !lease.hold(&self.inner.db, self.inner.clock.now()).await? {
            return Ok(Retained::default());
        }
        let retained = self.retain_held(&lease).await;
        lease.release(&self.inner.db).await?;
        let result = retained?;
        self.keep_file_states().await?;
        self.keep_pool_choice_details().await?;
        self.schedule_agent_history(true);
        if result != Retained::default() {
            self.kick();
        }
        Ok(result)
    }

    /// Pages through the candidates, renewing `lease` between pages; stops where another process took it.
    async fn retain_held(&self, lease: &job_leases::Lease) -> Result<Retained, EngineError> {
        let mut after = None;
        let mut result = Retained::default();
        loop {
            let ids = {
                let mut conn = self.inner.db.acquire().await?;
                hierarchy::retention_page(&mut conn, after, BATCH).await?
            };
            if ids.is_empty() {
                break;
            }
            for id in &ids {
                // A busy independent home must not prevent other candidates from being retained.
                match self.retain_node(*id).await {
                    Ok(changed) => {
                        result.archived += changed.archived;
                        result.deleted += changed.deleted;
                    }
                    Err(error) => {
                        tracing::warn!(node=%id,%error,"this node's retention could not be committed")
                    }
                }
            }
            after = ids.last().copied();
            if !lease.hold(&self.inner.db, self.inner.clock.now()).await? {
                break;
            }
        }
        Ok(result)
    }

    async fn retain_node(&self, id: Id) -> Result<Retained, EngineError> {
        let mut tx = transaction_guard::scope(true, self.inner.db.begin()).await?;
        let rows = hierarchy::with_ancestors(&mut tx, &[id]).await?;
        let Some(node) = rows
            .iter()
            .find(|row| row.id == id && row.deleted_at.is_none() && row.kind != "org")
            .cloned()
        else {
            return Ok(Retained::default());
        };
        if rows.iter().any(|row| row.deleted_at.is_some()) {
            return Ok(Retained::default());
        }
        hierarchy::lock_tree_for_moves(&mut tx, node.org_id).await?;
        let now = self.inner.clock.now();
        let assessment = retention_assessment(&mut tx, &node, &rows, now).await?;
        let archive = assessment.archive;
        let delete = assessment.delete;
        if !archive && !delete {
            return Ok(Retained::default());
        }
        self.lock_home_in(&mut tx, id).await?;
        hierarchy::lock(&mut tx, id).await?;
        let deleted_nodes = if delete {
            std::iter::once(id)
                .chain(
                    hierarchy::descendants(&mut tx, id)
                        .await?
                        .into_iter()
                        .map(|row| row.id),
                )
                .collect::<Vec<_>>()
        } else {
            vec![]
        };
        if delete {
            hierarchy::keep_archive_states(&mut tx, id).await?;
        }
        let marked = hierarchy::archive_by(&mut tx, id, None, now).await?;
        let settled = if marked || lifecycle::archive_work_pending(&mut tx, id).await? {
            self.archive_as_in(&mut tx, id, None, Time::new(now))
                .await?
        } else {
            crate::archive::Settled::default()
        };
        if delete {
            self.remove_home_as_in(&mut tx, id, None, Time::new(now), None)
                .await
                .map_err(|error| EngineError::unreadable(error.to_string()))?;
            hierarchy::delete_subtree_by(&mut tx, id, None, now).await?;
            hierarchy::include_settled_in_deletion(&mut tx, id, &deleted_nodes).await?;
        }
        orch_authz::epoch::bump(&mut tx)
            .await
            .map_err(|error| EngineError::unreadable(error.to_string()))?;
        hierarchy::stamp_subtree(&mut tx, id).await?;
        let action = if delete {
            orch_db::audit::action!(AuditAction::NodeDeleted)
        } else {
            orch_db::audit::action!(AuditAction::NodeArchived)
        };
        let summary = if delete {
            format!("The retention policy deleted {}", node.name)
        } else {
            format!("The retention policy archived {}", node.name)
        };
        let mut entry = Entry::new(
            Actor::System,
            action.as_str(),
            &node.kind,
            Some(id),
            &summary,
        );
        entry.history = true;
        entry.node_id = Some(id);
        entry.org_id = Some(node.org_id);
        let days = orch_settings::retention::deleted_keep_days(&mut tx).await?;
        let purge_at = orch_settings::retention::purge_at(now, days);
        entry.details = json!({"retention":true,"archived_first":marked,"threads":settled.threads,"turns":settled.turns,
            "inputs":settled.inputs,"approvals":settled.approvals,"deleted_nodes":deleted_nodes,"purge_at":purge_at});
        audit::append(&mut tx, &entry, &Origin::default(), now).await?;
        tx.commit().await.map_err(orch_db::DbError::from)?;
        Ok(if delete {
            Retained {
                deleted: 1,
                archived: 0,
            }
        } else {
            Retained {
                archived: 1,
                deleted: 0,
            }
        })
    }
}

struct RetentionAssessment {
    facts: orch_api_contract::nodes::NodeRetention,
    archive: bool,
    delete: bool,
}

/// The native scheduler's present policy and conditions for one authorized node review.
pub async fn node_retention(
    conn: &mut orch_db::Conn,
    id: Id,
    now: chrono::DateTime<chrono::Utc>,
) -> Result<Option<orch_api_contract::nodes::NodeRetention>, EngineError> {
    let rows = hierarchy::with_ancestors(conn, &[id]).await?;
    let Some(node) = rows
        .iter()
        .find(|row| row.id == id && row.deleted_at.is_none())
    else {
        return Ok(None);
    };
    if rows.iter().any(|row| row.deleted_at.is_some()) {
        return Ok(None);
    }
    Ok(Some(
        retention_assessment(conn, node, &rows, now).await?.facts,
    ))
}

async fn retention_assessment(
    conn: &mut orch_db::Conn,
    node: &hierarchy::NodeRow,
    ancestors: &[hierarchy::NodeRow],
    now: chrono::DateTime<chrono::Utc>,
) -> Result<RetentionAssessment, EngineError> {
    use orch_api_contract::nodes::{NodeRetention, NodeRetentionHold, NodeRetentionPolicy};
    use orch_api_contract::workspace::JobWaitTime;
    let archive_def = registry::get(keys::RETENTION_ARCHIVE_IDLE_DAYS)
        .ok_or_else(|| EngineError::unreadable("the archive retention setting is absent"))?;
    let delete_def = registry::get(keys::RETENTION_DELETE_ARCHIVED_DAYS)
        .ok_or_else(|| EngineError::unreadable("the deletion retention setting is absent"))?;
    let defs = if node.kind == "topic" {
        vec![archive_def, delete_def]
    } else {
        vec![delete_def]
    };
    let settings = Snapshot::load(conn, Place::Node(node.id), Wanted::Keys(&defs))
        .await
        .map_err(|error| EngineError::unreadable(error.to_string()))?;
    let boundary = |def,
                    applies: bool,
                    from: Option<chrono::DateTime<chrono::Utc>>,
                    hold|
     -> Result<NodeRetentionPolicy, EngineError> {
        if !applies {
            return Ok(NodeRetentionPolicy {
                days: None,
                source: None,
                eligible_at: JobWaitTime::NotApplicable,
                hold_reason: None,
            });
        }
        let resolved = settings.resolve(def);
        let days = resolved.value.and_then(|value| value.as_i64()).unwrap_or(0);
        let eligible_at = if days == 0 || from.is_none() {
            JobWaitTime::NotApplicable
        } else {
            from.and_then(|at| {
                chrono::Duration::try_days(days).and_then(|age| at.checked_add_signed(age))
            })
            .map_or(JobWaitTime::Unreported, |at| JobWaitTime::At {
                at: Time::new(at),
            })
        };
        Ok(NodeRetentionPolicy {
            days: Some(days),
            source: Some(serde_json::from_value(serde_json::to_value(
                resolved.source.scope,
            )?)?),
            eligible_at,
            hold_reason: if days == 0 { None } else { hold },
        })
    };
    let (activity, active) =
        if node.kind == "topic" && ancestors.iter().all(|row| row.archived_at.is_none()) {
            let (at, active) = hierarchy::retention_activity(conn, node.id).await?;
            (Some(at), active)
        } else {
            (None, false)
        };
    let archive_hold = if node.archived_at.is_some() {
        Some(NodeRetentionHold::AlreadyArchived)
    } else if ancestors.iter().any(|row| row.archived_at.is_some()) {
        Some(NodeRetentionHold::AncestorArchived)
    } else if active {
        Some(NodeRetentionHold::ActiveWork)
    } else {
        None
    };
    let archive = boundary(archive_def, node.kind == "topic", activity, archive_hold)?;
    let delete = boundary(
        delete_def,
        node.kind != "org",
        node.archived_at,
        node.archived_at
            .is_none()
            .then_some(NodeRetentionHold::NotArchived),
    )?;
    let deleting = node
        .archived_at
        .is_some_and(|at| old_enough(at, now, delete.days.unwrap_or(0)));
    let archiving = !deleting
        && archive.hold_reason.is_none()
        && activity.is_some_and(|at| old_enough(at, now, archive.days.unwrap_or(0)));
    Ok(RetentionAssessment {
        facts: NodeRetention {
            archive,
            delete,
            measured_at: Time::new(now),
        },
        archive: archiving,
        delete: deleting,
    })
}
