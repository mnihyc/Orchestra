//! Server-selected unused cache cleanup with fresh node use guards and durable per-object results.

use crate::commands::Outgoing;
use crate::jobs::runner::{Job, Stop};
use crate::{Engine, EngineError, ids};
use chrono::{DateTime, Utc};
use orch_db::DbError;
use orch_db::audit::{self, Actor, Entry, Origin};
use orch_driver_contract::resources::{Resource, ResourceInventory, ResourceKind};
use orch_link_contract::commands::{NodeResourcesList, NodeResourcesRemove, SerialKey};
use orch_settings::{Place, get};
use orch_settings_contract::registry::keys;
use orch_types::{Id, Json};
use serde_json::json;
use std::collections::{BTreeMap, BTreeSet, HashSet};

fn eligible(inventory: &ResourceInventory, days: i64, now: DateTime<Utc>) -> Vec<&Resource> {
    crate::resource_facts::unused_caches(inventory, days, now)
}

impl Engine {
    /// Refreshes one host's facts and removes only caches admitted by its current resolved setting.
    pub(crate) async fn cleanup_unused_caches(&self, job: &Job, host: Id) -> Result<(), Stop> {
        let host_row = {
            let mut conn = self.db().acquire().await.map_err(EngineError::from)?;
            orch_db::queries::resources::hosting::host(&mut conn, host)
                .await?
                .ok_or_else(|| EngineError::unreadable("the cleanup host is gone"))?
        };
        let command = Outgoing::new(
            ids::derived("node.resources.inventory", &[job.id().as_bytes()]),
            host_row.machine_id,
            SerialKey::Machine,
            &NodeResourcesList {},
        )
        .map_err(EngineError::from)?;
        let inventory: ResourceInventory =
            serde_json::from_value(job.command(command).await?).map_err(EngineError::from)?;
        let mut runtime_uses =
            BTreeMap::<Id, BTreeSet<orch_driver_contract::resources::ResourceRef>>::new();
        if inventory.unavailable.is_empty() {
            for item in inventory
                .resources
                .iter()
                .filter(|item| item.resource.kind == ResourceKind::Unit)
            {
                if let Some(runtime) = item
                    .labels
                    .get(orch_driver_contract::runtime::LABEL_RUNTIME_ID)
                    .and_then(|id| id.parse().ok())
                {
                    runtime_uses
                        .entry(runtime)
                        .or_default()
                        .extend(item.depends_on.iter().cloned());
                }
            }
        }
        let mut held = HashSet::new();
        let unknown_use;
        let days = {
            let mut conn = self.db().acquire().await.map_err(EngineError::from)?;
            sqlx::query("UPDATE hosts SET resource_inventory=$2 WHERE id=$1")
                .bind(host)
                .bind(json!(inventory))
                .execute(&mut *conn)
                .await
                .map_err(DbError::from)?;
            let uses: Vec<_> = runtime_uses
                .into_iter()
                .map(|(id, uses)| json!({"id":id,"uses":uses}))
                .collect();
            sqlx::query("UPDATE runtimes r SET resources_used=x.uses FROM jsonb_to_recordset($2::jsonb) AS x(id uuid, uses jsonb) WHERE r.id=x.id AND r.host_id=$1 AND r.resources_used IS DISTINCT FROM x.uses")
                .bind(host).bind(json!(uses)).execute(&mut *conn).await.map_err(DbError::from)?;
            let holds = crate::resource_facts::artifact_holds(&mut conn, &[host]).await?;
            held.extend(holds.known.into_values().flatten());
            unknown_use = holds.unknown.contains(&host);
            get::<i64>(&mut conn, Place::Host(host), keys::CLEANUP_UNUSED_KEEP_DAYS)
                .await
                .map_err(EngineError::from)?
                .ok_or_else(|| EngineError::unreadable("the cleanup setting is unavailable"))?
        };
        if unknown_use {
            job.remember(&json!({"cleanup_stopped":"unknown_restore_dependencies"}))
                .await?;
            return Ok(());
        }
        // Occupancy stamps and this observation come from the same node clock. A controller
        // clock offset must never make an artifact seven days old before its host says so.
        let candidates: Vec<_> = eligible(&inventory, days, inventory.observed_at.get())
            .into_iter()
            .filter(|item| !held.contains(&item.resource))
            .collect();
        job.remember(&json!({"cleanup_plan":candidates,"cleanup_keep_days":days}))
            .await?;
        let mut results = Vec::<Json>::new();
        for (index, item) in candidates.iter().enumerate() {
            job.check()?;
            let mut conn = self.db().acquire().await.map_err(EngineError::from)?;
            let current_days =
                get::<i64>(&mut conn, Place::Host(host), keys::CLEANUP_UNUSED_KEEP_DAYS)
                    .await
                    .map_err(EngineError::from)?
                    .ok_or_else(|| EngineError::unreadable("the cleanup setting is unavailable"))?;
            if current_days != days {
                job.remember(&json!({"cleanup_stopped":"setting_changed"}))
                    .await?;
                break;
            }
            // Setup jobs can need a cache between ensure and unit creation. The node also serializes
            // its local ensure/check/start operations with removal and compares the observed use.
            let preparing: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM jobs j WHERE j.id<>$1 AND j.state IN ('queued','running') AND (j.plan->>'host'=$2::text OR EXISTS(SELECT 1 FROM runtimes r WHERE r.host_id=$2 AND r.environment_id=j.environment_id AND r.state NOT IN ('removed','retired')))) OR EXISTS(SELECT 1 FROM commands WHERE machine_id=$3 AND state IN ('queued','sent','accepted'))")
                .bind(job.id()).bind(host).bind(host_row.machine_id).fetch_one(&mut *conn).await.map_err(DbError::from)?;
            drop(conn);
            if preparing {
                job.remember(&json!({"cleanup_stopped":"work_in_progress"}))
                    .await?;
                break;
            }
            if index == 0 {
                let mut tx = self.db().begin().await.map_err(EngineError::from)?;
                orch_db::queries::transaction_guard::lock_authority(&mut tx).await?;
                let exists: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM audit_log WHERE object_kind='host' AND object_id=$1 AND details->>'job'=$2::text AND details->>'operation'='cleanup')").bind(host).bind(job.id()).fetch_one(&mut *tx).await.map_err(DbError::from)?;
                if !exists {
                    let summary = format!("Automatic cleanup on {}", host_row.name);
                    let mut entry = Entry::new(
                        Actor::System,
                        orch_access_contract::audit::AuditAction::HostUpdated.as_str(),
                        "host",
                        Some(host),
                        &summary,
                    );
                    entry.history = true;
                    entry.node_id = host_row.home_id;
                    entry.org_id = host_row.org_id;
                    entry.details = json!({"operation":"cleanup","host":host,"name":host_row.name,"job":job.id(),"keep_days":current_days,"cleanup":candidates});
                    audit::append(&mut tx, &entry, &Origin::default(), job.now()).await?;
                }
                tx.commit().await.map_err(DbError::from)?;
            }
            let body = NodeResourcesRemove {
                kind: item.resource.kind,
                id: item.resource.id.clone(),
                stop_first: false,
                expected_last_used_at: item.last_used_at,
            };
            let command = Outgoing::new(
                ids::derived(
                    "node.resources.cleanup",
                    &[job.id().as_bytes(), item.resource.id.as_bytes()],
                ),
                host_row.machine_id,
                SerialKey::Machine,
                &body,
            )
            .map_err(EngineError::from)?;
            let (outcome, reason) = match job.command_irreversible(command).await {
                Ok(_) => ("removed", None),
                Err(Stop::Failed(failure)) => ("failed", Some(failure.message)),
                Err(other) => return Err(other),
            };
            results.push(json!({"resource":item.resource,"name":item.name,"size_bytes":item.size_bytes,"outcome":outcome,"reason":reason}));
            job.remember(&json!({"cleanup_results":results})).await?;
            if outcome == "removed" {
                let mut conn = self.db().acquire().await.map_err(EngineError::from)?;
                sqlx::query("UPDATE hosts SET resource_inventory=jsonb_set(resource_inventory,'{resources}',COALESCE((SELECT jsonb_agg(item) FROM jsonb_array_elements(resource_inventory->'resources') item WHERE item->'resource'<>$2::jsonb),'[]'::jsonb)) WHERE id=$1 AND resource_inventory->>'observed_at'=$3")
                    .bind(host).bind(json!(item.resource)).bind(inventory.observed_at.to_string()).execute(&mut *conn).await.map_err(DbError::from)?;
            }
            job.progress((index + 1) as f64 / candidates.len() as f64, None)
                .await?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use orch_types::Time;

    fn inventory() -> ResourceInventory {
        serde_json::from_value(json!({"observed_at":"2026-10-01T00:00:00.000Z","unavailable":[],"resources":[
            {"resource":{"kind":"tool_version","id":"old"},"name":"Old tool","size_bytes":null,"labels":{},"depends_on":[],"last_used_at":"2026-10-01T00:00:00.000Z","busy":false},
            {"resource":{"kind":"volume","id":"data"},"name":"Kept data","size_bytes":128,"labels":{},"depends_on":[],"last_used_at":"2026-10-01T00:00:00.000Z","busy":false}
        ]})).unwrap()
    }

    #[test]
    fn seven_days_admits_only_unused_caches_and_zero_disables_automatic_removal() {
        let inventory = inventory();
        let before: Time = "2026-10-07T23:59:59.999Z".parse().unwrap();
        let due: Time = "2026-10-08T00:00:00.000Z".parse().unwrap();
        assert!(eligible(&inventory, 7, before.get()).is_empty());
        assert_eq!(
            eligible(&inventory, 7, due.get())
                .iter()
                .map(|item| item.resource.id.as_str())
                .collect::<Vec<_>>(),
            ["old"]
        );
        assert!(eligible(&inventory, 0, due.get()).is_empty());
        assert!(eligible(&inventory, 14, due.get()).is_empty());
    }

    #[test]
    fn an_unreadable_inventory_or_a_new_dependency_never_proves_a_cache_unused() {
        let mut inventory = inventory();
        let due: Time = "2026-11-01T00:00:00.000Z".parse().unwrap();
        let cache = inventory.resources[0].resource.clone();
        inventory.resources[1].depends_on.push(cache);
        assert!(eligible(&inventory, 7, due.get()).is_empty());
        inventory.resources[1].depends_on.clear();
        inventory
            .unavailable
            .push(orch_driver_contract::resources::ResourceUnavailable {
                driver: "unreachable".into(),
                reason: "offline".into(),
            });
        assert!(eligible(&inventory, 7, due.get()).is_empty());
    }
}
