//! Host departure reads its current dependents and retained data once from their existing records.

use super::EngineError;
use orch_db::{
    Conn, DbError,
    queries::{environments::retained, loss, resources::mounts},
};
use orch_driver_contract::resources::ResourceInventory;
use orch_types::{Id, Json};

/// One host's exact current sources, before the usual name and authority projection.
pub struct HostCustody {
    /// The canonical loss and recovery inventory.
    pub inventory: loss::Inventory,
    /// Current mount locations on this machine.
    pub locations: Vec<mounts::LocationRow>,
    /// Earlier displaced mount data on this machine.
    pub mount_kept: Vec<mounts::KeptRow>,
    /// Frozen environment recovery promises on this host.
    pub kept: Vec<retained::Removal>,
    /// Latest complete or partial physical observation; absent is unknown.
    pub native: Option<ResourceInventory>,
    /// Counts for the plan, derived from these memberships and the current offering records.
    pub counts: Vec<(String, i64)>,
    /// An exact standalone node command, only where the installation reported one.
    pub uninstall_command: Option<String>,
}

impl HostCustody {
    /// Counts the same live memberships after the caller's reviewed manageable set is projected.
    pub fn pending_managers(&self, managed: &std::collections::BTreeSet<Id>) -> i64 {
        i64::try_from(
            self.inventory
                .environments
                .iter()
                .filter(|row| {
                    !matches!(row.status.as_str(), "removed" | "lost") && !managed.contains(&row.id)
                })
                .count(),
        )
        .unwrap_or(i64::MAX)
    }
}

/// Reads batched indexed memberships without counting separately in each destructive route.
pub async fn host(conn: &mut Conn, host: Id) -> Result<Option<HostCustody>, EngineError> {
    let Some(inventory) = loss::inventory(conn, "host", host).await? else {
        return Ok(None);
    };
    let Some(machine) = inventory.machine else {
        return Ok(None);
    };
    let locations = mounts::locations(conn, &inventory.mounts)
        .await?
        .into_iter()
        .filter(|row| row.machine_id == machine)
        .collect::<Vec<_>>();
    let mount_kept: Vec<mounts::KeptRow> = sqlx::query_as("SELECT id,mount_id,machine_id,kind,source,storage,data_kind,file_count,total_bytes,job_id,kept_by,kept_at FROM mount_kept_data WHERE id=ANY($1) AND lost_at IS NULL ORDER BY id")
        .bind(&inventory.mount_kept).fetch_all(&mut *conn).await.map_err(DbError::from)?;
    let kept: Vec<retained::Removal> = sqlx::query_as("SELECT id,deleted_by,deleted_at,removed_keep_days,volumes_purge_at,retained_volumes,retained_bytes,retained_size_at,removal_phase,removal_revision,purged_at FROM environments WHERE removal_phase IN ('kept','restoring','purging') AND purged_at IS NULL AND EXISTS(SELECT 1 FROM jsonb_array_elements(retained_volumes) v WHERE v->>'host_id'=$1::uuid::text) ORDER BY id")
        .bind(host).fetch_all(&mut *conn).await.map_err(DbError::from)?;
    let offered: i64 = sqlx::query_scalar("SELECT count(DISTINCT o.server_id) FROM mcp_server_offerings o JOIN mcp_servers s ON s.id=o.server_id WHERE o.machine_id=$1 AND o.state<>'lost' AND s.deleted_at IS NULL")
        .bind(machine).fetch_one(&mut *conn).await.map_err(DbError::from)?;
    let raw: Option<Json> = sqlx::query_scalar("SELECT resource_inventory FROM hosts WHERE id=$1")
        .bind(host)
        .fetch_one(&mut *conn)
        .await
        .map_err(DbError::from)?;
    let native = raw.map(serde_json::from_value).transpose()?;
    let installation: Option<Json> =
        sqlx::query_scalar("SELECT facts->'installation' FROM machines WHERE id=$1")
            .bind(machine)
            .fetch_one(&mut *conn)
            .await
            .map_err(DbError::from)?;
    let uninstall_command = installation
        .and_then(|value| {
            serde_json::from_value::<orch_api_contract::machine_programs::MachineInstallation>(
                value,
            )
            .ok()
        })
        .filter(|installation| installation.program == "orch-node")
        .and_then(|installation| installation.commands.uninstall)
        .filter(|command| command.ends_with(" uninstall") && command.contains(" --config "))
        .map(|command| format!("{command} --purge"));
    let count = |length: usize| i64::try_from(length).unwrap_or(i64::MAX);
    let counts = vec![
        (
            "environments".into(),
            count(
                inventory
                    .environments
                    .iter()
                    .filter(|row| row.status != "removed")
                    .count(),
            ),
        ),
        ("mount_locations".into(), count(locations.len())),
        ("mcp_servers".into(), offered),
        ("provider_runtimes".into(), count(inventory.providers.len())),
    ];
    Ok(Some(HostCustody {
        inventory,
        locations,
        mount_kept,
        kept,
        native,
        counts,
        uninstall_command,
    }))
}

impl crate::Engine {
    /// Refreshes one authorized host's physical facts through the existing observed background job.
    pub async fn host_custody_inventory(
        &self,
        host: Id,
        machine: Id,
        by: Id,
    ) -> Result<
        (
            Option<ResourceInventory>,
            orch_api_contract::forks::Measurement,
        ),
        EngineError,
    > {
        let count = self
            .measure_in_background(
                machine,
                orch_link_contract::commands::CommandKind::NodeResourcesList,
                serde_json::json!({}),
                serde_json::json!({"host_id":host}),
                by,
            )
            .await?;
        let inventory: Option<ResourceInventory> =
            count.result.map(serde_json::from_value).transpose()?;
        if let Some(inventory) = &inventory {
            sqlx::query("UPDATE hosts SET resource_inventory=$2 WHERE id=$1 AND machine_id=$3")
                .bind(host)
                .bind(serde_json::to_value(inventory)?)
                .bind(machine)
                .execute(self.db().pool())
                .await
                .map_err(DbError::from)?;
        }
        Ok((inventory, count.reference))
    }
}
