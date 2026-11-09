//! Native home/side deletion footprint, shared with direct thread deletion and its scope supplier.

use super::RemovalScheduleError;
use crate::{Engine, EngineError};
use orch_db::queries::{hierarchy, lifecycle, resources, threads};
use orch_db::{Conn, DbError};
use orch_types::Id;
use std::collections::BTreeSet;

/// The actual native deletion selection, not a permission-bearing review or a rendered response.
#[derive(Debug)]
pub struct Prepared {
    node: hierarchy::NodeRow,
    set: Vec<(String, Id, i64)>,
    resources: Vec<resources::AccessFacts>,
    environments: Vec<Id>,
    side_parent: Option<Id>,
    member_destination: Option<Id>,
}
impl Prepared {
    pub fn node(&self) -> &hierarchy::NodeRow {
        &self.node
    }
    pub fn set(&self) -> &[(String, Id, i64)] {
        &self.set
    }
    /// The original audit/deletion operand order keeps the selected root before its children.
    pub fn node_ids(&self) -> Vec<Id> {
        std::iter::once(self.node.id)
            .chain(
                self.set
                    .iter()
                    .filter(|(kind, id, _)| kind != "document" && *id != self.node.id)
                    .map(|(_, id, _)| *id),
            )
            .collect()
    }
    pub fn resources(&self) -> &[resources::AccessFacts] {
        &self.resources
    }
    pub fn environments(&self) -> &[Id] {
        &self.environments
    }
    pub fn side_parent(&self) -> Option<Id> {
        self.side_parent
    }
    pub fn member_destination(&self) -> Option<Id> {
        self.member_destination
    }
}

#[derive(Debug, PartialEq, Eq, sqlx::FromRow)]
struct Custody {
    runtime: Id,
    host: Id,
    machine: Id,
    connector: Option<Id>,
}

struct Selection {
    node: hierarchy::NodeRow,
    set: Vec<(String, Id, i64)>,
    resources: Vec<resources::AccessFacts>,
    environments: Vec<Id>,
    threads: Vec<Id>,
    binding_resources: Vec<resources::AccessFacts>,
    custody: Vec<Custody>,
    side_parent: Option<Id>,
    member_destination: Option<Id>,
}
impl Selection {
    fn same(&self, fresh: &Self) -> bool {
        self.node == fresh.node
            && self.set == fresh.set
            && self.resources == fresh.resources
            && self.environments == fresh.environments
            && self.threads == fresh.threads
            && self.binding_resources == fresh.binding_resources
            && self.custody == fresh.custody
            && self.side_parent == fresh.side_parent
            && self.member_destination == fresh.member_destination
    }
    fn into_prepared(self) -> Prepared {
        Prepared {
            node: self.node,
            set: self.set,
            resources: self.resources,
            environments: self.environments,
            side_parent: self.side_parent,
            member_destination: self.member_destination,
        }
    }
}

async fn select(
    conn: &mut Conn,
    node: Id,
    keep_at: Option<Id>,
) -> Result<Selection, RemovalScheduleError> {
    let root = hierarchy::with_ancestors(conn, &[node])
        .await?
        .into_iter()
        .find(|row| row.id == node && row.deleted_at.is_none())
        .ok_or(RemovalScheduleError::Gone)?;
    let side_parent = if root.kind == "thread" {
        threads::forking::side_parent(conn, node).await?
    } else {
        None
    };
    let set = hierarchy::deletion_set(conn, node, false).await?;
    if side_parent.is_some() {
        return Ok(Selection {
            node: root,
            set,
            resources: Vec::new(),
            environments: Vec::new(),
            threads: vec![node],
            binding_resources: binding_resources(conn, &[node]).await?,
            custody: custody(conn, &[], &[node]).await?,
            side_parent,
            member_destination: None,
        });
    }
    let environments = lifecycle::environments_under(conn, node).await?;
    let resources = if root.kind == "org" {
        resources::resources_under_home(conn, node).await?
    } else {
        resources::resource_facts(conn, &environments)
            .await?
            .into_iter()
            .filter(|row| row.kind == "environment" && !row.deleted)
            .collect()
    };
    let mut affected = lifecycle::threads_under(conn, node).await?;
    for environment in &environments {
        affected.extend(lifecycle::bindings(conn, *environment).await?);
    }
    if keep_at.is_none() {
        for resource in &resources {
            affected.extend(resources::ending_threads(conn, resource.id).await?);
        }
    }
    affected.sort_unstable();
    affected.dedup();
    let member_destination =
        if root.kind == "org" && hierarchy::members_inside(conn, node).await? > 0 {
            root.parent_id
        } else {
            None
        };
    let binding_resources = binding_resources(conn, &affected).await?;
    let custody = custody(
        conn,
        &resources.iter().map(|row| row.id).collect::<Vec<_>>(),
        &affected,
    )
    .await?;
    Ok(Selection {
        node: root,
        set,
        resources,
        environments,
        binding_resources,
        custody,
        threads: affected,
        side_parent,
        member_destination,
    })
}

async fn binding_resources(
    conn: &mut Conn,
    threads: &[Id],
) -> Result<Vec<resources::AccessFacts>, RemovalScheduleError> {
    let mut ids = BTreeSet::new();
    for id in threads {
        if let Some(thread) = orch_db::queries::threads::thread(conn, *id).await? {
            ids.extend(thread.environment_id.map(|id| ("environment", id)));
            ids.extend(thread.provider_id.map(|id| ("provider", id)));
            ids.extend(thread.agent_id.map(|id| ("agent", id)));
        }
    }
    let rows =
        resources::resource_facts(conn, &ids.iter().map(|(_, id)| *id).collect::<Vec<_>>()).await?;
    Ok(rows
        .into_iter()
        .filter(|row| ids.contains(&(row.kind.as_str(), row.id)))
        .collect())
}

async fn custody(
    conn: &mut Conn,
    resources: &[Id],
    threads: &[Id],
) -> Result<Vec<Custody>, RemovalScheduleError> {
    Ok(sqlx::query_as("SELECT r.id AS runtime,r.host_id AS host,h.machine_id AS machine,r.machine_id AS connector FROM runtimes r JOIN hosts h ON h.id=r.host_id WHERE r.state NOT IN ('retired','removed') AND (r.environment_id=ANY($1) OR r.provider_id=ANY($1) OR r.host_id=ANY($1) OR EXISTS(SELECT 1 FROM threads t WHERE t.node_id=ANY($2) AND (t.environment_id=r.environment_id OR t.provider_id=r.provider_id))) ORDER BY r.id")
        .bind(resources).bind(threads).fetch_all(conn).await.map_err(DbError::from)?)
}

/// The same native deletion selector, without writer locks, usable by a typed source supplier.
pub async fn inspect(conn: &mut Conn, node: Id) -> Result<Prepared, RemovalScheduleError> {
    Ok(select(conn, node, None).await?.into_prepared())
}

impl Engine {
    /// Inspect the same native deletion operands without acquiring the later writer's locks.
    /// Queue preparation supplies these facts; execution still uses prepare_home_deletion_in.
    pub async fn inspect_home_deletion_in(
        &self,
        conn: &mut Conn,
        node: Id,
    ) -> Result<Prepared, RemovalScheduleError> {
        inspect(conn, node).await
    }

    /// Hold the actual thread/node/document/resource/member serialization footprint, then refresh it.
    /// A stable selection is reused by the writer; no trial mutation or DTO supplies scope sources.
    pub async fn prepare_home_deletion_in(
        &self,
        conn: &mut Conn,
        node: Id,
        keep_at: Option<Id>,
    ) -> Result<Prepared, RemovalScheduleError> {
        let mut selected = select(conn, node, keep_at).await?;
        loop {
            threads::tools::lock_threads(conn, &selected.threads).await?;
            let anchors = selected
                .set
                .iter()
                .filter(|(kind, _, _)| kind != "document")
                .map(|(_, id, _)| *id)
                .chain(selected.threads.iter().copied())
                .chain(selected.node.parent_id)
                .collect::<Vec<_>>();
            let nodes = hierarchy::with_ancestors(conn, &anchors).await?;
            let nodes = nodes.into_iter().map(|row| row.id).collect::<BTreeSet<_>>();
            for id in nodes {
                hierarchy::lock(conn, id).await?;
            }
            let _ = hierarchy::deletion_set(conn, node, true).await?;
            let mut resources_to_hold = selected
                .resources
                .iter()
                .chain(&selected.binding_resources)
                .map(|row| (row.kind.as_str(), row.id))
                .collect::<BTreeSet<_>>();
            resources_to_hold.extend(selected.custody.iter().map(|row| ("host", row.host)));
            for (kind, id) in resources_to_hold {
                resources::lock_resource(conn, kind, id).await?;
            }
            let runtimes = selected
                .custody
                .iter()
                .map(|row| row.runtime)
                .collect::<Vec<_>>();
            sqlx::query("SELECT id FROM runtimes WHERE id=ANY($1) ORDER BY id FOR UPDATE")
                .bind(&runtimes)
                .fetch_all(&mut *conn)
                .await
                .map_err(DbError::from)?;
            let machines = selected
                .custody
                .iter()
                .flat_map(|row| std::iter::once(row.machine).chain(row.connector))
                .chain(
                    selected
                        .binding_resources
                        .iter()
                        .chain(&selected.resources)
                        .filter_map(|row| row.machine_id),
                )
                .collect::<BTreeSet<_>>()
                .into_iter()
                .collect::<Vec<_>>();
            sqlx::query("SELECT id FROM machines WHERE id=ANY($1) ORDER BY id FOR UPDATE")
                .bind(&machines)
                .fetch_all(&mut *conn)
                .await
                .map_err(DbError::from)?;
            if selected.node.kind == "org" {
                sqlx::query("SELECT m.user_id FROM org_members m JOIN nodes n ON n.id=m.domain_id WHERE $1=ANY(n.path) ORDER BY m.user_id FOR UPDATE OF m")
                    .bind(node).fetch_all(&mut *conn).await.map_err(DbError::from)?;
                if let Some(parent) = selected.member_destination {
                    sqlx::query("SELECT id FROM node_roles WHERE node_id=$1 AND removed_at IS NULL ORDER BY id FOR UPDATE")
                        .bind(parent).fetch_all(&mut *conn).await.map_err(DbError::from)?;
                }
            }
            let resource_ids = selected
                .resources
                .iter()
                .map(|row| row.id)
                .collect::<Vec<_>>();
            sqlx::query("SELECT id FROM jobs WHERE node_id=ANY($1) OR environment_id=ANY($2) OR provider_id=ANY($2) ORDER BY id FOR UPDATE")
                .bind(&selected.threads).bind(&resource_ids).fetch_all(&mut *conn).await.map_err(DbError::from)?;
            for thread in &selected.threads {
                if let Some(hold) = crate::locks::lock_of(conn, *thread).await? {
                    let _ = orch_db::queries::jobs::lock(conn, hold.job.id).await?;
                }
                if crate::background_wake::running(conn, Some(*thread), None, None).await? {
                    return Err(EngineError::BackgroundRunning(*thread).into());
                }
            }
            let fresh = select(conn, node, keep_at).await?;
            if selected.same(&fresh) {
                return Ok(fresh.into_prepared());
            }
            selected = fresh;
        }
    }

    /// The native first SQL of a prepared hierarchy deletion; later settlement is owned cleanup.
    pub async fn begin_home_deletion_before<E: From<DbError>>(
        &self,
        conn: &mut Conn,
        prepared: &Prepared,
        before: impl FnOnce() -> Result<(), E>,
    ) -> Result<(), E> {
        before()?;
        hierarchy::keep_archive_states(conn, prepared.node.id).await?;
        Ok(())
    }
}
