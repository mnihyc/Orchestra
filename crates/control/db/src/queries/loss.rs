//! Locked inventories and terminal records for explicitly confirmed permanent machine or environment loss.

use crate::{Conn, DbError};
use chrono::{DateTime, Utc};
use orch_types::{Hash, Id, Json};
use serde::{Deserialize, Serialize};

/// The target's name, physical machine, host, availability and declared loss.
type Subject = (String, Option<Id>, Option<Id>, bool, bool);

/// One affected environment, including a closed retention promise.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, sqlx::FromRow)]
pub struct Environment {
    /// Its identity.
    pub id: Id,
    /// Its safe display name before permission filtering.
    pub name: String,
    /// Its owner.
    pub owner_id: Id,
    /// Its home.
    pub home_node_id: Id,
    /// Its root organization.
    pub org_id: Id,
    /// The configuration revision reviewed by the person.
    pub revision: i64,
    /// Its current lifecycle state.
    pub status: String,
    /// Measured retained bytes, if any.
    pub retained_bytes: Option<i64>,
    /// Whether a retained-data promise exists.
    pub retained: bool,
}

/// A deterministic inventory frozen into a loss preflight.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Inventory {
    /// Exact resource name for typed confirmation.
    pub name: String,
    /// The physical machine, absent for an environment that never had one.
    pub machine: Option<Id>,
    /// Affected host facade, where present.
    pub host: Option<Id>,
    /// Whether the resource currently answers.
    pub online: bool,
    /// Already declared lost.
    pub lost: bool,
    /// The affected environments.
    pub environments: Vec<Environment>,
    /// Exact runtime identities and reviewed states.
    pub runtimes: Vec<(Id, String)>,
    /// Connector identities revoked with their lost units.
    pub machines: Vec<Id>,
    /// Provider runtimes that cannot be used.
    pub providers: Vec<Id>,
    /// Open work ending as lost.
    pub jobs: Vec<Id>,
    /// Mount locations forgotten with a physical machine.
    pub mounts: Vec<Id>,
    /// The data displaced by a mount copy that was still kept there.
    pub mount_kept: Vec<Id>,
    /// Bytes of displaced mount data, null when any is unmeasured.
    pub mount_bytes: Option<i64>,
    /// Threads whose execution is held on these environments.
    pub threads: Vec<Id>,
    /// Work elsewhere whose provider runtime was on the physical source.
    #[serde(default)]
    pub provider_threads: Vec<Id>,
}

impl Inventory {
    /// The exact dependency and revision cut a confirmation must still match.
    pub fn fingerprint(&self) -> Result<Hash, DbError> {
        Ok(Hash::of(&serde_json::to_vec(self).map_err(|error| {
            sqlx::Error::Protocol(error.to_string())
        })?))
    }
}

/// Reads current dependencies; the caller first locks the resource when confirming.
pub async fn inventory(conn: &mut Conn, kind: &str, id: Id) -> Result<Option<Inventory>, DbError> {
    let subject: Option<Subject> = match kind {
        "host" => sqlx::query_as("SELECT h.name, m.id, h.id, m.online, m.lost_at IS NOT NULL FROM hosts h JOIN machines m ON m.id=h.machine_id WHERE h.id=$1 AND m.deleted_at IS NULL").bind(id).fetch_optional(&mut *conn).await?,
        "machine" => sqlx::query_as("SELECT m.name, m.id, h.id, m.online, m.lost_at IS NOT NULL FROM machines m LEFT JOIN hosts h ON h.machine_id=m.id WHERE m.id=$1 AND m.deleted_at IS NULL").bind(id).fetch_optional(&mut *conn).await?,
        "environment" => sqlx::query_as("SELECT e.name, COALESCE(h.machine_id,e.machine_id),r.host_id, (NOT (e.loss_missing OR COALESCE(r.loss_missing,false)) AND (COALESCE(m.online,false) OR COALESCE(hm.online,false))), e.lost_at IS NOT NULL FROM environments e LEFT JOIN runtimes r ON r.environment_id=e.id AND r.successor_of IS NULL AND r.state NOT IN ('retired','removed') LEFT JOIN hosts h ON h.id=r.host_id LEFT JOIN machines hm ON hm.id=h.machine_id LEFT JOIN machines m ON m.id=e.machine_id WHERE e.id=$1 AND e.purged_at IS NULL").bind(id).fetch_optional(&mut *conn).await?,
        _ => return Ok(None),
    };
    let Some((name, machine, host, online, lost)) = subject else {
        return Ok(None);
    };
    let physical = kind != "environment";
    let environments: Vec<Environment> = sqlx::query_as("SELECT e.id,e.name,e.owner_id,e.home_node_id,e.org_id,e.revision,e.status,e.retained_bytes,e.removal_phase IS NOT NULL AS retained FROM environments e WHERE e.purged_at IS NULL AND (($1 AND (e.machine_id=$2 OR EXISTS(SELECT 1 FROM runtimes r WHERE r.environment_id=e.id AND r.host_id=$3 AND r.successor_of IS NULL AND ((r.lost_at IS NULL AND r.state NOT IN ('retired','removed')) OR (r.lost_at IS NOT NULL AND e.status='lost'))) OR (e.removal_phase IN ('kept','restoring','purging','lost') AND EXISTS(SELECT 1 FROM jsonb_array_elements(e.retained_volumes) v WHERE v->>'host_id'=$3::text)))) OR (NOT $1 AND e.id=$4)) ORDER BY e.id")
        .bind(physical).bind(machine).bind(host).bind(id).fetch_all(&mut *conn).await?;
    let env_ids: Vec<Id> = environments.iter().map(|env| env.id).collect();
    let runtimes: Vec<(Id,String)> = sqlx::query_as("SELECT id,state FROM runtimes WHERE lost_at IS NULL AND state<>'removed' AND (($1 AND host_id=$2) OR (NOT $1 AND environment_id=ANY($3) AND successor_of IS NULL AND state NOT IN ('retired','removed'))) ORDER BY id")
        .bind(physical).bind(host).bind(&env_ids).fetch_all(&mut *conn).await?;
    let runtime_ids: Vec<Id> = runtimes.iter().map(|row| row.0).collect();
    let mut machines: Vec<Id> = sqlx::query_scalar("SELECT id FROM machines WHERE runtime_id=ANY($1) OR (runtime_id IS NULL AND id IN(SELECT machine_id FROM runtimes WHERE id=ANY($1))) ORDER BY id").bind(&runtime_ids).fetch_all(&mut *conn).await?;
    if physical && let Some(machine) = machine {
        machines.push(machine);
    }
    if !physical {
        machines.extend(sqlx::query_scalar::<_,Id>("SELECT e.machine_id FROM environments e JOIN machines m ON m.id=e.machine_id WHERE e.id=$1 AND e.placement='managed' AND m.placement='managed'").bind(id).fetch_all(&mut *conn).await?);
    }
    machines.sort_unstable();
    machines.dedup();
    let providers: Vec<Id> = sqlx::query_scalar("SELECT DISTINCT id FROM providers WHERE deleted_at IS NULL AND (machine_id=ANY($1) OR id IN(SELECT provider_id FROM runtimes WHERE id=ANY($2) AND successor_of IS NULL AND state NOT IN ('retired','removed')) OR held_by_environment_id=ANY($3) OR ($4 AND runtime_unavailable IS NOT NULL AND EXISTS(SELECT 1 FROM runtimes r WHERE r.provider_id=providers.id AND r.host_id=$5 AND r.lost_at IS NOT NULL AND (providers.machine_id=r.machine_id OR (providers.machine_id IS NULL AND NOT EXISTS(SELECT 1 FROM runtimes live WHERE live.provider_id=providers.id AND live.successor_of IS NULL AND live.state NOT IN ('retired','removed'))))))) ORDER BY id")
        .bind(&machines).bind(&runtime_ids).bind(&env_ids).bind(physical).bind(host).fetch_all(&mut *conn).await?;
    let provider_threads:Vec<Id>=sqlx::query_scalar("SELECT t.node_id FROM threads t JOIN nodes n ON n.id=t.node_id WHERE t.provider_id=ANY($1) AND n.deleted_at IS NULL AND NOT COALESCE(t.environment_id=ANY($2),false) ORDER BY t.node_id").bind(&providers).bind(&env_ids).fetch_all(&mut *conn).await?;
    let jobs: Vec<Id> = sqlx::query_scalar("SELECT id FROM jobs WHERE state IN ('queued','running') AND (environment_id=ANY($1) OR provider_id=ANY($2) OR plan->>'runtime'=ANY($3::text[]) OR plan->>'source_machine'=ANY($4::text[]) OR plan#>>'{cascade,machine}'=ANY($4::text[]) OR (kind='measurement' AND (plan->>'machine'=ANY($4::text[]) OR plan#>>'{body,target_machine}'=ANY($4::text[]) OR plan#>>'{subject,environment_id}'=ANY($1::text[])))) ORDER BY id")
        .bind(&env_ids).bind(&providers).bind(runtime_ids.iter().map(Id::to_string).collect::<Vec<_>>()).bind(machines.iter().map(Id::to_string).collect::<Vec<_>>()).fetch_all(&mut *conn).await?;
    let mounts: Vec<Id> = if physical {
        sqlx::query_scalar(
            "SELECT mount_id FROM mount_locations WHERE machine_id=$1 ORDER BY mount_id",
        )
        .bind(machine)
        .fetch_all(&mut *conn)
        .await?
    } else {
        Vec::new()
    };
    let kept: Vec<(Id, Option<i64>)> = if physical {
        sqlx::query_as("SELECT id,total_bytes FROM mount_kept_data WHERE machine_id=$1 AND lost_at IS NULL ORDER BY id").bind(machine).fetch_all(&mut *conn).await?
    } else {
        Vec::new()
    };
    let mount_bytes = kept.iter().try_fold(0_i64, |sum, row| {
        row.1.map(|bytes| sum.saturating_add(bytes))
    });
    let threads=sqlx::query_scalar("SELECT t.node_id FROM threads t JOIN nodes n ON n.id=t.node_id WHERE t.environment_id=ANY($1) AND n.deleted_at IS NULL ORDER BY t.node_id").bind(&env_ids).fetch_all(conn).await?;
    Ok(Some(Inventory {
        name,
        machine,
        host,
        online,
        lost,
        environments,
        runtimes,
        machines,
        providers,
        provider_threads,
        jobs,
        mounts,
        mount_kept: kept.into_iter().map(|row| row.0).collect(),
        mount_bytes,
        threads,
    }))
}

/// Closes the physical records in the declaration transaction, without issuing a command.
pub async fn close(
    conn: &mut Conn,
    inventory: &Inventory,
    by: Id,
    now: DateTime<Utc>,
) -> Result<(), DbError> {
    let envs: Vec<Id> = inventory.environments.iter().map(|env| env.id).collect();
    let runtimes: Vec<Id> = inventory.runtimes.iter().map(|row| row.0).collect();
    for machine in &inventory.machines {
        super::resources::revoke_machine(conn, *machine, now).await?;
        super::link::cancel_open_of(conn, *machine, now).await?;
    }
    sqlx::query("UPDATE machines SET lost_at=$2,lost_by=$3,current_session_id=NULL,session_started_at=NULL,cert_next_fingerprint=NULL,cert_next_public_key=NULL,cert_next_serial=NULL,cert_next_not_after=NULL,cert_next_again=NULL,revision=revision+1 WHERE id=ANY($1)")
        .bind(&inventory.machines).bind(now).bind(by).execute(&mut *conn).await?;
    sqlx::query("UPDATE runtimes SET state='retired',state_detail='Declared lost; physical cleanup is unavailable',lost_at=$2,remove_at=NULL,updated_at=$2 WHERE id=ANY($1)")
        .bind(&runtimes).bind(now).execute(&mut *conn).await?;
    sqlx::query("UPDATE environments SET status='lost',status_detail='Its environment was declared lost',lost_at=$2,lost_by=$3,remove_at=NULL,volumes_purge_at=NULL,removal_phase=CASE WHEN removal_phase IS NULL THEN NULL ELSE 'lost' END,removal_revision=removal_revision+1,revision=revision+1,updated_at=$2 WHERE id=ANY($1)")
        .bind(&envs).bind(now).bind(by).execute(&mut *conn).await?;
    sqlx::query("UPDATE providers SET runtime_unavailable='Its runtime was declared lost',status=CASE WHEN held_by_environment_id IS NULL THEN 'offline' ELSE 'needs_setup' END,status_detail='Its runtime was declared lost',status_at=$2,held_by_environment_id=NULL,revision=revision+1,updated_at=$2 WHERE id=ANY($1)")
        .bind(&inventory.providers).bind(now).execute(&mut *conn).await?;
    sqlx::query("UPDATE leases SET closed_at=$2,close_reason='interrupted' WHERE closed_at IS NULL AND (thread_id=ANY($1) OR provider_id=ANY($3))")
        .bind(&inventory.threads).bind(now).bind(&inventory.providers).execute(&mut *conn).await?;
    let mut affected_threads = inventory.threads.clone();
    affected_threads.extend(&inventory.provider_threads);
    sqlx::query("UPDATE turns SET state='interrupted',interrupt_cause='environment_change',error=jsonb_build_object('code',CASE WHEN thread_id=ANY($3) THEN 'environment_lost' ELSE 'provider_runtime_lost' END,'message',CASE WHEN thread_id=ANY($3) THEN 'Its environment was declared lost' ELSE 'Its provider runtime was declared lost' END,'retryable',false),completed_at=$2,files_end_due=false WHERE thread_id=ANY($1) AND state IN ('starting','running')")
        .bind(&affected_threads).bind(now).bind(&inventory.threads).execute(&mut *conn).await?;
    sqlx::query("UPDATE terminal_sessions SET state='closed',closed_at=$2 WHERE environment_id=ANY($1) AND state<>'closed'")
        .bind(&envs).bind(now).execute(&mut *conn).await?;
    for job in &inventory.jobs {
        if let Some(row) = super::jobs::job(conn, *job).await? {
            super::jobs::finish(conn,*job,"lost",&row.progress,None,Some(&serde_json::json!({"code":"environment_lost","message":"The machine needed by this work was declared lost","phase":row.progress.get("phase").and_then(Json::as_str).unwrap_or_default(),"undone":false})),now).await?;
        }
    }
    if let Some(machine) = inventory
        .machine
        .filter(|id| inventory.machines.contains(id))
    {
        sqlx::query("UPDATE hosts SET schedulable=false,revision=revision+1,updated_at=$2 WHERE machine_id=$1").bind(machine).bind(now).execute(&mut *conn).await?;
        sqlx::query("DELETE FROM mount_locations WHERE machine_id=$1")
            .bind(machine)
            .execute(&mut *conn)
            .await?;
        sqlx::query(
            "UPDATE mount_kept_data SET lost_at=$2 WHERE machine_id=$1 AND lost_at IS NULL",
        )
        .bind(machine)
        .bind(now)
        .execute(&mut *conn)
        .await?;
    }
    for environment in envs {
        super::jobs::stamp_environment(conn, environment, now).await?;
    }
    Ok(())
}

/// Whether a resource identity has been permanently revoked by a loss declaration.
pub async fn machine_lost(conn: &mut Conn, machine: Id) -> Result<bool, DbError> {
    Ok(sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM machines WHERE id=$1 AND lost_at IS NOT NULL)",
    )
    .bind(machine)
    .fetch_one(conn)
    .await?)
}

/// Live defaults are skipped in both creation and binding previews.
pub async fn accepts_threads(conn: &mut Conn, environment: Id) -> Result<bool, DbError> {
    Ok(sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM environments WHERE id=$1 AND deleted_at IS NULL AND remove_at IS NULL AND lost_at IS NULL AND status NOT IN ('lost','removed'))").bind(environment).fetch_one(conn).await?)
}

/// A healthy stopped unit is reachable through its host even while its connector is stopped.
/// Only an unreachable physical source or a node's explicit absent-unit report exposes permanent loss.
pub async fn eligible_environments(conn: &mut Conn, ids: &[Id]) -> Result<Vec<Id>, DbError> {
    Ok(sqlx::query_scalar("SELECT e.id FROM environments e LEFT JOIN runtimes r ON r.environment_id=e.id AND r.successor_of IS NULL AND r.state NOT IN ('retired','removed') LEFT JOIN hosts h ON h.id=r.host_id LEFT JOIN machines hm ON hm.id=h.machine_id LEFT JOIN machines m ON m.id=e.machine_id
      WHERE e.id=ANY($1) AND e.deleted_at IS NULL AND e.status NOT IN ('lost','removed') AND (e.generation>0 OR e.placement='connected')
        AND NOT (NOT (e.loss_missing OR COALESCE(r.loss_missing,false)) AND (COALESCE(m.online,false) OR COALESCE(hm.online,false))) ORDER BY e.id")
      .bind(ids).fetch_all(conn).await?)
}

/// Topics actually using these shared defaults, including inherited values and the first ancestor lock.
pub async fn default_topics(
    conn: &mut Conn,
    environments: &[Id],
) -> Result<Vec<(Id, Id)>, DbError> {
    if environments.is_empty() {
        return Ok(Vec::new());
    }
    Ok(sqlx::query_as("SELECT (chosen.value #>> '{}')::uuid,n.id FROM nodes n
      CROSS JOIN LATERAL (SELECT s.value FROM settings s WHERE s.key='env.default_id' AND (s.node_id IS NULL OR s.node_id=ANY(n.path))
        ORDER BY CASE WHEN s.locked THEN 0 ELSE 1 END,CASE WHEN s.locked THEN COALESCE(array_position(n.path,s.node_id),0) ELSE -COALESCE(array_position(n.path,s.node_id),0) END LIMIT 1) chosen
      LEFT JOIN LATERAL (SELECT s.value FROM settings s WHERE s.key='env.thread_default' AND (s.node_id IS NULL OR s.node_id=ANY(n.path))
        ORDER BY CASE WHEN s.locked THEN 0 ELSE 1 END,CASE WHEN s.locked THEN COALESCE(array_position(n.path,s.node_id),0) ELSE -COALESCE(array_position(n.path,s.node_id),0) END LIMIT 1) mode ON true
      WHERE n.kind='topic' AND n.deleted_at IS NULL AND NOT EXISTS(SELECT 1 FROM nodes a WHERE a.id=ANY(n.path) AND a.deleted_at IS NOT NULL)
        AND chosen.value #>> '{}' = ANY(SELECT id::text FROM unnest($1::uuid[]) id) AND COALESCE(mode.value #>> '{}','shared')='shared' ORDER BY n.id")
      .bind(environments).fetch_all(conn).await?)
}
