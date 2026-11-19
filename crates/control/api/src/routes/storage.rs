//! One authorized storage report from existing retention records, content metadata and node inventories.

use std::collections::{BTreeSet, HashMap};

use chrono::{DateTime, Utc};
use orch_access_contract::decisions::ObjectRef;
use orch_access_contract::permissions::{NodePermission, ObjectKind, PermissionName};
use orch_api_contract::nodes::NodeRef;
use orch_api_contract::resources::HostRef;
use orch_api_contract::storage::{
    self as api, KeptRecord, KeptWhy, StorageAvailability, StorageKind, StorageSourceIssue,
};
use orch_authz::{Act, Principal};
use orch_db::{Conn, DbError};
use orch_driver_contract::resources::ResourceInventory;
use orch_engine::{resource_facts, storage as model};
use orch_types::{Id, Json, Time};

use crate::middleware::authorization::{self, refusals};
use crate::middleware::requests::{ApiError, ApiResult};
use crate::registry::{Ctx, Registry};

mod cleanup;

/// Binds the single storage report.
pub fn mount(registry: &mut Registry) {
    registry.on(api::StorageGet, get_report);
    cleanup::mount(registry);
}

#[derive(Clone, Copy)]
enum Scope {
    Me,
    Node(Id),
    Host(Id),
    System,
    Resource(ObjectRef),
}

impl Scope {
    fn parse(query: &api::StorageQuery) -> ApiResult<Self> {
        if let Some(id) = query.resource_id {
            let kind = query.resource_kind.ok_or_else(|| {
                ApiError::invalid("resource_kind", "is required with resource_id")
            })?;
            return Ok(Self::Resource(ObjectRef { kind, id }));
        }
        let scope = query.scope.as_deref().unwrap_or("me");
        if scope == "me" {
            return Ok(Self::Me);
        }
        if scope == "system" {
            return Ok(Self::System);
        }
        if let Some(id) = scope.strip_prefix("node:").and_then(|id| id.parse().ok()) {
            return Ok(Self::Node(id));
        }
        if let Some(id) = scope.strip_prefix("host:").and_then(|id| id.parse().ok()) {
            return Ok(Self::Host(id));
        }
        Err(ApiError::invalid(
            "scope",
            "is me, system, node:<id> or host:<id>",
        ))
    }
    fn wire(self) -> String {
        match self {
            Self::Me => "me".into(),
            Self::System => "system".into(),
            Self::Host(id) => format!("host:{id}"),
            Self::Node(id) => format!("node:{id}"),
            Self::Resource(object) => format!("resource:{}:{}", object.kind, object.id),
        }
    }
}

#[derive(sqlx::FromRow)]
struct HostRow {
    id: Id,
    machine_id: Id,
    name: String,
    home_id: Option<Id>,
    owner_id: Option<Id>,
    online: bool,
    resource_inventory: Option<Json>,
}
#[derive(sqlx::FromRow)]
struct RuntimeOwner {
    id: Id,
    host_id: Id,
    environment_id: Option<Id>,
    provider_id: Option<Id>,
    template_id: Id,
    home_id: Option<Id>,
    owner_id: Option<Id>,
}
impl RuntimeOwner {
    fn object(&self) -> Option<ObjectRef> {
        self.environment_id
            .map(|id| ObjectRef {
                kind: ObjectKind::Environment,
                id,
            })
            .or_else(|| {
                self.provider_id.map(|id| ObjectRef {
                    kind: ObjectKind::Provider,
                    id,
                })
            })
    }
}
#[derive(sqlx::FromRow)]
struct KeptEnvironment {
    id: Id,
    name: String,
    owner_id: Id,
    home_node_id: Id,
    retained_bytes: Option<i64>,
    retained_size_at: Option<DateTime<Utc>>,
    deleted_at: Option<DateTime<Utc>>,
    volumes_purge_at: Option<DateTime<Utc>>,
    retained_volumes: Json,
}
#[derive(sqlx::FromRow)]
struct KeptMount {
    id: Id,
    mount_id: Id,
    machine_id: Id,
    name: String,
    home_id: Option<Id>,
    owner_id: Option<Id>,
    kind: String,
    total_bytes: Option<i64>,
    kept_at: DateTime<Utc>,
}
#[derive(sqlx::FromRow)]
struct SkillVersion {
    id: Id,
    skill_id: Id,
    name: String,
    number: i32,
    archive_hash: String,
    owner_id: Id,
    home_id: Id,
    archive_size: i64,
    created_at: DateTime<Utc>,
    latest: bool,
    retained: bool,
}

/// Reads sources once; all permission frames and all dependent names are batched.
pub(crate) async fn records(
    ctx: &Ctx,
    conn: &mut Conn,
    principal: &Principal,
    query: &api::StorageQuery,
) -> ApiResult<(String, Vec<KeptRecord>, Vec<StorageSourceIssue>)> {
    let scope = Scope::parse(query)?;
    if matches!(scope, Scope::System) && !principal.is_system_admin() {
        return Err(ApiError::not_found());
    }
    if let Scope::Node(id) = scope {
        let seen = orch_authz::nodes::holding(
            conn,
            principal,
            &[id],
            PermissionName::OfNode(NodePermission::View),
            Act::Use,
            Default::default(),
        )
        .await
        .map_err(ApiError::internal)?;
        if seen.is_empty() {
            return Err(ApiError::not_found());
        }
    }
    let hosts:Vec<HostRow>=sqlx::query_as("SELECT h.id,h.machine_id,h.name,h.home_id,h.owner_id,(m.online AND m.status='active' AND m.deleted_at IS NULL) AS online,h.resource_inventory FROM hosts h JOIN machines m ON m.id=h.machine_id")
        .fetch_all(&mut *conn).await.map_err(DbError::from)?;
    let environments:Vec<KeptEnvironment>=sqlx::query_as("SELECT id,name,owner_id,home_node_id,retained_bytes,retained_size_at,deleted_at,volumes_purge_at,retained_volumes FROM environments WHERE removal_phase IN ('removing','kept','restoring','purging') AND purged_at IS NULL")
        .fetch_all(&mut *conn).await.map_err(DbError::from)?;
    let mounts:Vec<KeptMount>=sqlx::query_as("SELECT k.id,k.mount_id,k.machine_id,m.name,m.home_id,m.owner_id,k.kind,k.total_bytes,k.kept_at FROM mount_kept_data k JOIN mounts m ON m.id=k.mount_id")
        .fetch_all(&mut *conn).await.map_err(DbError::from)?;
    let skills:Vec<SkillVersion>=sqlx::query_as("SELECT v.id,v.skill_id,v.name,v.number,encode(v.archive,'hex') AS archive_hash,s.owner_id,s.home_id,v.archive_size,v.created_at,(COALESCE(s.latest_version_id=v.id,false) AND s.deleted_at IS NULL) AS latest,EXISTS(SELECT 1 FROM turns t WHERE t.skill_versions @> ARRAY[v.id]) AS retained FROM skill_versions v JOIN skills s ON s.id=v.skill_id")
        .fetch_all(&mut *conn).await.map_err(DbError::from)?;
    let runtimes:Vec<RuntimeOwner>=sqlx::query_as("SELECT r.id,r.host_id,r.environment_id,r.provider_id,r.template_id,COALESCE(e.home_node_id,p.home_id) AS home_id,COALESCE(e.owner_id,p.owner_id) AS owner_id FROM runtimes r LEFT JOIN environments e ON e.id=r.environment_id LEFT JOIN providers p ON p.id=r.provider_id WHERE r.host_id=ANY($1)")
        .bind(hosts.iter().map(|host|host.id).collect::<Vec<_>>()).fetch_all(&mut *conn).await.map_err(DbError::from)?;
    let ids: Vec<_> = hosts
        .iter()
        .map(|row| row.id)
        .chain(environments.iter().map(|row| row.id))
        .chain(mounts.iter().map(|row| row.mount_id))
        .chain(skills.iter().map(|row| row.skill_id))
        .chain(
            runtimes
                .iter()
                .flat_map(|row| [row.environment_id, row.provider_id, Some(row.template_id)])
                .flatten(),
        )
        .chain(hosts.iter().map(|row| row.machine_id))
        .chain(if let Scope::Resource(object) = scope {
            Some(object.id)
        } else {
            None
        })
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect();
    let rights = resource_facts::authorities(conn, principal, &ids)
        .await
        .map_err(ApiError::internal)?;
    let rights: HashMap<_, _> = rights
        .into_iter()
        .map(|row| (row.resource.object(), row))
        .collect();
    let manages = |kind, id| {
        rights
            .get(&ObjectRef { kind, id })
            .is_some_and(|row| row.manageable)
    };
    match scope {
        Scope::Host(id) if !manages(ObjectKind::Host, id) => return Err(ApiError::not_found()),
        Scope::Resource(object) if !rights.get(&object).is_some_and(|row| row.manageable) => {
            return Err(ApiError::not_found());
        }
        _ => {}
    }
    let homes: Vec<_> = hosts
        .iter()
        .filter_map(|row| row.home_id)
        .chain(environments.iter().map(|row| row.home_node_id))
        .chain(mounts.iter().filter_map(|row| row.home_id))
        .chain(skills.iter().map(|row| row.home_id))
        .chain(runtimes.iter().filter_map(|row| row.home_id))
        .collect();
    let nodes = orch_db::queries::hierarchy::with_ancestors(conn, &homes).await?;
    let seen_nodes = orch_authz::nodes::holding(
        conn,
        principal,
        &homes,
        PermissionName::OfNode(NodePermission::View),
        Act::Use,
        Default::default(),
    )
    .await
    .map_err(ApiError::internal)?;
    let seen_nodes: HashMap<_, _> = seen_nodes.into_iter().map(|row| (row.id, row)).collect();
    let in_scope = |owner: Option<Id>,
                    home: Option<Id>,
                    host: Option<Id>,
                    resource: Option<ObjectRef>| match scope {
        Scope::Me => owner == Some(principal.user()),
        Scope::System => true,
        Scope::Host(id) => host == Some(id),
        Scope::Node(id) => home
            .and_then(|home| nodes.iter().find(|node| node.id == home))
            .is_some_and(|node| node.path.contains(&id)),
        Scope::Resource(object) => {
            resource == Some(object) || (object.kind == ObjectKind::Host && host == Some(object.id))
        }
    };
    let people = refusals::user_refs(
        conn,
        &hosts
            .iter()
            .filter_map(|row| row.owner_id)
            .chain(environments.iter().map(|row| row.owner_id))
            .chain(mounts.iter().filter_map(|row| row.owner_id))
            .chain(skills.iter().map(|row| row.owner_id))
            .chain(runtimes.iter().filter_map(|row| row.owner_id))
            .collect::<Vec<_>>(),
    )
    .await?;
    let at = Time::new(ctx.now);
    let mut result = Vec::new();
    let mut unavailable = Vec::new();
    let policies = orch_settings::cleanup_at_hosts(
        conn,
        &hosts.iter().map(|host| host.id).collect::<Vec<_>>(),
    )
    .await
    .map_err(ApiError::internal)?;
    let addresses: HashMap<_, _> = orch_authz::resources::addresses(
        conn,
        &hosts
            .iter()
            .filter(|host| {
                rights
                    .get(&ObjectRef {
                        kind: ObjectKind::Host,
                        id: host.id,
                    })
                    .is_some_and(|row| row.readable)
            })
            .map(|host| host.id)
            .collect::<Vec<_>>(),
    )
    .await
    .map_err(ApiError::internal)?
    .into_iter()
    .collect();
    let mut sources = Vec::new();
    let mut related = HashMap::<Id, std::collections::HashSet<ObjectRef>>::new();
    for runtime in &runtimes {
        let resources = related.entry(runtime.host_id).or_default();
        if let Some(object) = runtime.object() {
            resources.insert(object);
        }
        resources.insert(ObjectRef {
            kind: ObjectKind::Template,
            id: runtime.template_id,
        });
    }
    for host in &hosts {
        let scoped = match scope {
            Scope::Resource(object) => {
                related
                    .get(&host.id)
                    .is_some_and(|resources| resources.contains(&object))
                    || object.kind == ObjectKind::Machine && host.machine_id == object.id
            }
            _ => false,
        };
        if !(manages(ObjectKind::Host, host.id) || scoped)
            || matches!(scope, Scope::Host(_))
                && !in_scope(
                    host.owner_id,
                    host.home_id,
                    Some(host.id),
                    Some(ObjectRef {
                        kind: ObjectKind::Host,
                        id: host.id,
                    }),
                )
        {
            continue;
        }
        if let Scope::Resource(object) = scope
            && !scoped
            && !(object.kind == ObjectKind::Host && host.id == object.id)
        {
            continue;
        }
        let reference = HostRef {
            id: host.id,
            name: host.name.clone(),
            address: addresses.get(&host.id).cloned().unwrap_or_default(),
        };
        let Some(inventory) = host
            .resource_inventory
            .as_ref()
            .and_then(|value| serde_json::from_value::<ResourceInventory>(value.clone()).ok())
        else {
            unavailable.push(StorageSourceIssue {
                host: Some(reference),
                reason: if host.online {
                    "inventory_unrecorded"
                } else {
                    "node_away"
                }
                .into(),
            });
            continue;
        };
        sources.push((host, reference, inventory));
    }
    let subjects: Vec<_> = sources
        .iter()
        .flat_map(|(host, _, inventory)| {
            inventory.resources.iter().map(move |item| {
                orch_api_contract::resource_facts::FactSubject::Artifact {
                    host_id: host.id,
                    artifact_kind: serde_json::to_value(item.resource.kind)
                        .ok()
                        .and_then(|value| value.as_str().map(str::to_owned))
                        .unwrap_or_default(),
                    id: item.resource.id.clone(),
                }
            })
        })
        .collect();
    let raw = resource_facts::read(conn, &subjects, at)
        .await
        .map_err(ApiError::internal)?;
    let facts = resource_facts::project(conn, principal, Default::default(), &raw)
        .await
        .map_err(ApiError::internal)?;
    let mut facts: HashMap<_, _> = facts
        .into_iter()
        .filter_map(|facts| match &facts.subject {
            orch_api_contract::resource_facts::FactSubject::Artifact { host_id, id, .. } => {
                Some(((*host_id, id.clone()), facts))
            }
            _ => None,
        })
        .collect();
    let runtime_ids: HashMap<_, _> = runtimes.iter().map(|row| (row.id, row)).collect();
    let mut retained_members = HashMap::new();
    for environment in &environments {
        if let Ok(volumes) = serde_json::from_value::<
            Vec<orch_db::queries::environments::retained::Volume>,
        >(environment.retained_volumes.clone())
        {
            for volume in volumes
                .into_iter()
                .filter(|volume| volume.environment_id == environment.id)
            {
                retained_members.insert((volume.host_id, volume.name), environment.id);
            }
        }
    }
    for (host, reference, inventory) in sources {
        let days = *policies
            .get(&host.id)
            .ok_or_else(|| ApiError::internal("the cleanup policy is missing"))?;
        let mut items = model::node_records(&reference, &inventory, days, host.online);
        for (item, native) in items.iter_mut().zip(&inventory.resources) {
            item.resource = Some(ObjectRef {
                kind: ObjectKind::Host,
                id: host.id,
            });
            item.place = host
                .home_id
                .and_then(|home| seen_nodes.get(&home))
                .map(|node| NodeRef {
                    thread_display: None,
                    id: node.id,
                    kind: refusals::node_kind(node.kind.as_str())
                        .unwrap_or(orch_api_contract::nodes::NodeKind::Org),
                    name: Some(node.name.clone()),
                    redacted: false,
                });
            item.facts = facts.remove(&(host.id, native.resource.id.clone()));
            if let Some(facts) = &item.facts {
                use orch_api_contract::resource_facts::DependentRelation;
                if facts.dependents.iter().any(|group| {
                    matches!(
                        group.relation,
                        DependentRelation::Restore | DependentRelation::Attachment
                    ) && group.count > 0
                }) {
                    item.why = if facts.dependents.iter().any(|group| {
                        group.relation == DependentRelation::Attachment && group.count > 0
                    }) {
                        KeptWhy::InUse
                    } else {
                        KeptWhy::Restore
                    };
                    item.until = facts
                        .dependents
                        .iter()
                        .find(|group| group.relation == DependentRelation::Restore)
                        .and_then(|group| group.until);
                    item.blocker = Some("use_resource_cleanup".into());
                } else if !facts.dependents_complete && item.blocker.is_none() {
                    item.why = KeptWhy::Unknown;
                    item.blocker = Some("dependencies_unrecorded".into());
                } else if facts.dependents.iter().any(|group| {
                    matches!(
                        group.relation,
                        DependentRelation::NativeDependency | DependentRelation::Operation
                    )
                }) && item.blocker.is_none()
                {
                    item.blocker = Some("in_use".into());
                }
            }
            let runtime = native
                .labels
                .get(orch_driver_contract::runtime::LABEL_RUNTIME_ID)
                .and_then(|id| id.parse::<Id>().ok())
                .and_then(|id| runtime_ids.get(&id));
            let owner = runtime
                .and_then(|runtime| runtime.object())
                .and_then(|object| rights.get(&object).filter(|row| row.readable));
            item.owner = owner
                .and_then(|owner| owner.resource.first_owner)
                .or_else(|| runtime.is_none().then_some(host.owner_id).flatten())
                .and_then(|id| people.get(&id).cloned());
            if let Some(owner) = owner {
                item.resource = Some(owner.resource.object());
                item.place = owner
                    .resource
                    .home
                    .and_then(|home| seen_nodes.get(&home))
                    .map(|node| NodeRef {
                        thread_display: None,
                        id: node.id,
                        kind: refusals::node_kind(node.kind.as_str())
                            .unwrap_or(orch_api_contract::nodes::NodeKind::Topic),
                        name: Some(node.name.clone()),
                        redacted: false,
                    });
            }
            if native.resource.kind == orch_driver_contract::resources::ResourceKind::Volume
                && let Some(environment) = retained_members.get(&(host.id, native.name.clone()))
            {
                item.included_in = Some(format!("environment:{environment}"));
            }
            if !rights
                .get(&ObjectRef {
                    kind: ObjectKind::Host,
                    id: host.id,
                })
                .is_some_and(|row| row.readable)
            {
                item.host = None;
            }
        }
        if !inventory.unavailable.is_empty() {
            unavailable.push(StorageSourceIssue {
                host: Some(reference),
                reason: "inventory_incomplete".into(),
            });
        }
        items.retain(|row| match scope {
            Scope::Me => row
                .owner
                .as_ref()
                .is_some_and(|owner| owner.id == principal.user()),
            Scope::Node(id) => row
                .place
                .as_ref()
                .and_then(|place| nodes.iter().find(|node| node.id == place.id))
                .is_some_and(|node| node.path.contains(&id)),
            Scope::Resource(object) => {
                object.kind == ObjectKind::Machine && host.machine_id == object.id
                    || row.resource == Some(object)
                    || object.kind == ObjectKind::Host && host.id == object.id
                    || row.facts.as_ref().is_some_and(|facts| {
                        facts
                            .dependents
                            .iter()
                            .flat_map(|group| &group.visible)
                            .any(|edge| edge.object == Some(object))
                    })
            }
            _ => true,
        });
        result.extend(items);
    }
    for environment in environments {
        if !manages(ObjectKind::Environment, environment.id) {
            continue;
        }
        let refs = serde_json::from_value::<Vec<orch_db::queries::environments::retained::Volume>>(
            environment.retained_volumes,
        )
        .unwrap_or_default();
        let host_ids: BTreeSet<_> = refs.iter().map(|volume| volume.host_id).collect();
        let host = if host_ids.len() == 1 {
            host_ids.first().copied()
        } else {
            None
        };
        let object = ObjectRef {
            kind: ObjectKind::Environment,
            id: environment.id,
        };
        if !in_scope(
            Some(environment.owner_id),
            Some(environment.home_node_id),
            host,
            Some(object),
        ) {
            continue;
        }
        let mut row = model::record(
            format!("environment:{}", environment.id),
            StorageKind::EnvironmentFiles,
            environment.name,
            at,
        );
        row.resource = Some(object);
        row.bytes = environment.retained_bytes;
        row.why = KeptWhy::Restore;
        row.since = environment.deleted_at.map(Time::new);
        row.until = environment.volumes_purge_at.map(Time::new);
        row.owner = people.get(&environment.owner_id).cloned();
        row.observed_at = environment.retained_size_at.map_or(at, Time::new);
        row.can_clean = true;
        row.availability = StorageAvailability::Ready;
        row.host = host
            .and_then(|id| {
                hosts
                    .iter()
                    .find(|host| host.id == id && manages(ObjectKind::Host, id))
            })
            .map(|host| HostRef {
                id: host.id,
                name: host.name.clone(),
                address: String::new(),
            });
        row.place = seen_nodes
            .get(&environment.home_node_id)
            .map(|node| NodeRef {
                thread_display: None,
                id: node.id,
                kind: refusals::node_kind(node.kind.as_str())
                    .unwrap_or(orch_api_contract::nodes::NodeKind::Topic),
                name: Some(node.name.clone()),
                redacted: false,
            });
        if row.until.is_some_and(|until| until > at)
            && !rights
                .get(&object)
                .is_some_and(|authority| authority.administers)
        {
            row.blocker = Some("restore_window".into());
        }
        result.push(row);
    }
    for kept in mounts {
        let object = ObjectRef {
            kind: ObjectKind::Mount,
            id: kept.mount_id,
        };
        let host = hosts.iter().find(|host| host.machine_id == kept.machine_id);
        let host_id = host.map(|host| host.id);
        if !manages(ObjectKind::Mount, kept.mount_id)
            || !in_scope(kept.owner_id, kept.home_id, host_id, Some(object))
        {
            continue;
        }
        let mut row = model::record(
            format!("mount:{}", kept.id),
            StorageKind::MountData,
            kept.name,
            Time::new(kept.kept_at),
        );
        row.resource = Some(object);
        row.owner = kept.owner_id.and_then(|id| people.get(&id).cloned());
        row.bytes = kept.total_bytes;
        row.why = KeptWhy::Restore;
        row.since = Some(Time::new(kept.kept_at));
        row.availability = StorageAvailability::Ready;
        row.can_clean = true;
        if kept.kind == "path" {
            row.blocker = Some("borrowed_path".into());
        }
        result.push(row);
    }
    for version in skills {
        let object = ObjectRef {
            kind: ObjectKind::Skill,
            id: version.skill_id,
        };
        if !manages(ObjectKind::Skill, version.skill_id)
            || !in_scope(
                Some(version.owner_id),
                Some(version.home_id),
                None,
                Some(object),
            )
        {
            continue;
        }
        let mut row = model::record(
            format!("skill-version:{}", version.id),
            StorageKind::SkillVersion,
            format!("{} · {}", version.name, version.number),
            Time::new(version.created_at),
        );
        row.accounting_key = format!("blob:{}", version.archive_hash);
        row.resource = Some(object);
        row.owner = people.get(&version.owner_id).cloned();
        row.bytes = Some(version.archive_size);
        row.why = if version.latest || version.retained {
            KeptWhy::InUse
        } else {
            KeptWhy::Cache
        };
        row.since = Some(Time::new(version.created_at));
        row.availability = StorageAvailability::Ready;
        row.can_clean = true;
        if version.latest {
            row.blocker = Some("latest_version".into())
        } else if version.retained {
            row.blocker = Some("in_use".into())
        }
        result.push(row);
    }
    if matches!(scope, Scope::System) {
        server_records(conn, at, &mut result).await?;
        operator_records(ctx, conn, principal, at, &mut result, &mut unavailable).await?;
    }
    Ok((scope.wire(), result, unavailable))
}

async fn operator_records(
    ctx: &Ctx,
    conn: &mut Conn,
    principal: &orch_authz::Principal,
    at: Time,
    records: &mut Vec<KeptRecord>,
    unavailable: &mut Vec<StorageSourceIssue>,
) -> ApiResult<()> {
    let directory = ctx.app.config.installation.records_dir.clone();
    let system = ctx
        .app
        .link
        .certificates()
        .authority_certificate()
        .fingerprint();
    let read = tokio::task::spawn_blocking(move || {
        orch_worker_core::backups::storage::inventory(&directory, &system)
    })
    .await
    .map_err(ApiError::internal)?;
    let report = match read {
        Ok(Some(report)) => report,
        Ok(None) => {
            unavailable.push(StorageSourceIssue {
                host: None,
                reason: "operator_metadata_unrecorded".into(),
            });
            return Ok(());
        }
        Err(_) => {
            unavailable.push(StorageSourceIssue {
                host: None,
                reason: "operator_metadata_unavailable".into(),
            });
            return Ok(());
        }
    };
    let raw = report
        .items
        .iter()
        .map(|item| resource_facts::operator_item(item, at))
        .collect::<Result<Vec<_>, _>>()
        .map_err(ApiError::internal)?;
    if !report.issues.is_empty() {
        unavailable.push(StorageSourceIssue {
            host: None,
            reason: "operator_metadata_unavailable".into(),
        });
    }
    let facts = resource_facts::project(conn, principal, Default::default(), &raw)
        .await
        .map_err(ApiError::internal)?;
    for (item, facts) in report.items.into_iter().zip(facts) {
        let mut row = item.record;
        row.facts = Some(facts);
        records.push(row);
    }
    Ok(())
}

async fn server_records(conn: &mut Conn, at: Time, records: &mut Vec<KeptRecord>) -> ApiResult<()> {
    let bytes: i64 = sqlx::query_scalar("SELECT pg_database_size(current_database())")
        .fetch_one(&mut *conn)
        .await
        .map_err(DbError::from)?;
    let mut row = model::record(
        "server:database".into(),
        StorageKind::Database,
        String::new(),
        at,
    );
    row.bytes = Some(bytes);
    row.why = KeptWhy::InUse;
    row.availability = StorageAvailability::Ready;
    row.blocker = Some("database_in_use".into());
    records.push(row);
    let blobs: Vec<(String, i64, i32, DateTime<Utc>)> =
        sqlx::query_as("SELECT encode(hash,'hex'),size,pin_count,last_access_at FROM blobs")
            .fetch_all(&mut *conn)
            .await
            .map_err(DbError::from)?;
    for (hash, bytes, pins, last) in blobs {
        let mut row = model::record(format!("blob:{hash}"), StorageKind::Content, hash, at);
        row.bytes = Some(bytes);
        row.why = if pins > 0 {
            KeptWhy::InUse
        } else {
            KeptWhy::Cache
        };
        row.since = Some(Time::new(last));
        row.availability = StorageAvailability::Ready;
        row.can_clean = true;
        if pins > 0 {
            row.blocker = Some("in_use".into())
        }
        records.push(row);
    }
    type UploadStorageRow = (Id, String, i64, DateTime<Utc>, DateTime<Utc>);
    let uploads: Vec<UploadStorageRow> =
        sqlx::query_as("SELECT id,name,received,created_at,expires_at FROM upload_sessions")
            .fetch_all(&mut *conn)
            .await
            .map_err(DbError::from)?;
    for (id, name, bytes, started, until) in uploads {
        let mut row = model::record(format!("upload:{id}"), StorageKind::Upload, name, at);
        row.bytes = Some(bytes);
        row.why = KeptWhy::InUse;
        row.since = Some(Time::new(started));
        row.until = Some(Time::new(until));
        row.availability = StorageAvailability::Ready;
        row.can_clean = until <= at.get();
        row.blocker = (until > at.get()).then(|| "upload_active".into());
        records.push(row);
    }
    type ExportStorageRow = (
        Id,
        Option<i64>,
        DateTime<Utc>,
        Option<DateTime<Utc>>,
        String,
    );
    let exports: Vec<ExportStorageRow> = sqlx::query_as(
        "SELECT id,bytes,created_at,expires_at,state FROM exports WHERE state<>'expired'",
    )
    .fetch_all(&mut *conn)
    .await
    .map_err(DbError::from)?;
    for (id, bytes, created, until, state) in exports {
        let mut row = model::record(
            format!("export:{id}"),
            StorageKind::Export,
            id.to_string(),
            at,
        );
        row.bytes = bytes;
        row.why = KeptWhy::Restore;
        row.since = Some(Time::new(created));
        row.until = until.map(Time::new);
        row.availability = StorageAvailability::Ready;
        row.can_clean = state != "pending";
        row.blocker = (state == "pending").then(|| "export_running".into());
        records.push(row);
    }
    Ok(())
}

async fn get_report(ctx: Ctx, query: api::StorageQuery, (): ()) -> ApiResult<api::StorageReport> {
    let mut tx = ctx.app.db.begin().await?;
    sqlx::query("SET TRANSACTION ISOLATION LEVEL REPEATABLE READ")
        .execute(&mut *tx)
        .await
        .map_err(DbError::from)?;
    let principal = authorization::principal(&ctx, &mut tx).await?;
    let (scope, mut records, unavailable) = records(&ctx, &mut tx, &principal, &query).await?;
    let at = Time::new(ctx.now);
    records.retain(|row| model::matches(row, &query, at));
    records.sort_by(|a, b| a.id.cmp(&b.id));
    let totals = model::totals(&records);
    let breakdowns = model::breakdowns(&records, at);
    let after = query.page.cursor.as_deref().unwrap_or("");
    let mut items: Vec<_> = records
        .into_iter()
        .filter(|row| row.id.as_str() > after)
        .take(query.page.limit() as usize + 1)
        .collect();
    let next_cursor = if items.len() > query.page.limit() as usize {
        items.pop();
        items.last().map(|row| row.id.clone())
    } else {
        None
    };
    let name = match Scope::parse(&query)? {
        Scope::Host(id) => sqlx::query_scalar("SELECT name FROM hosts WHERE id=$1")
            .bind(id)
            .fetch_optional(&mut *tx)
            .await
            .map_err(DbError::from)?
            .unwrap_or_default(),
        Scope::Node(id) => sqlx::query_scalar("SELECT name FROM nodes WHERE id=$1")
            .bind(id)
            .fetch_optional(&mut *tx)
            .await
            .map_err(DbError::from)?
            .unwrap_or_default(),
        Scope::Resource(object) => resource_facts::authorities(&mut tx, &principal, &[object.id])
            .await
            .map_err(ApiError::internal)?
            .into_iter()
            .find(|row| row.resource.object() == object && row.readable)
            .map(|row| row.resource.name)
            .unwrap_or_default(),
        Scope::Me | Scope::System => String::new(),
    };
    Ok(api::StorageReport {
        scope,
        name,
        at,
        totals,
        breakdowns,
        items,
        next_cursor,
        unavailable,
    })
}
