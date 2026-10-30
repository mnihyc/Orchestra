//! One batched source of current dependents, reported use and privacy projections for plans and lists.

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use orch_access_contract::decisions::ObjectRef;
use orch_access_contract::permissions::{NodePermission, ObjectKind, Permission, PermissionName};
use orch_api_contract::resource_facts::{
    DependentGroup, DependentRelation, FactSubject, ResourceFacts, ResourceUseKind, ResourceUses,
    UseCoverage, VisibleDependent,
};
use orch_authz::{Act, Policy, Principal};
use orch_db::usage::rollup::{self, ResourceTurnDimension};
use orch_db::{Conn, DbError};
use orch_driver_contract::resources::{Resource, ResourceInventory, ResourceKind, ResourceRef};
use orch_types::{Id, Json, Time};
use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};

use crate::EngineError;

pub mod activity;
pub mod custody;
pub mod library;

/// Current host dependents that explain a deletion's wait; no route maintains another counter.
#[derive(Debug)]
pub struct HostWaiting {
    /// Live environments still needing their own managers.
    pub environments: i64,
    /// Recovery promises still keeping data on the host.
    pub kept: i64,
    /// Current runtime cleanup records, including retired generations still awaiting removal.
    pub runtimes: i64,
    /// The next future recovery or retired-generation deadline.
    pub until: Option<Time>,
    /// The host's current physical availability.
    pub online: bool,
    /// Its authorized page name, projected only after Host View or Manage.
    pub name: String,
}

#[derive(sqlx::FromRow)]
struct HostWaitingRow {
    environments: i64,
    kept: i64,
    runtimes: i64,
    until: Option<DateTime<Utc>>,
    online: bool,
    name: String,
}

/// Summarizes the same current host memberships without loading cascade or history rows.
pub async fn host_waiting(
    conn: &mut Conn,
    host: Id,
    at: Time,
) -> Result<Option<HostWaiting>, EngineError> {
    let row: Option<HostWaitingRow> =
        sqlx::query_as(include_str!("resource_facts/host_waiting.sql"))
            .bind(host)
            .bind(at.get())
            .fetch_optional(conn)
            .await
            .map_err(DbError::from)?;
    Ok(row.map(|row| HostWaiting {
        environments: row.environments,
        kept: row.kept,
        runtimes: row.runtimes,
        until: row.until.map(Time::new),
        online: row.online,
        name: row.name,
    }))
}

/// Existing runtime records retain exact artifact references after native units disappear.
#[derive(Debug, Default)]
pub struct ArtifactHolds {
    /// Exact recorded references, grouped by their source host.
    pub known: HashMap<Id, HashSet<ResourceRef>>,
    /// Hosts whose current or retained work has no trustworthy reference inventory yet.
    pub unknown: HashSet<Id>,
    /// Logical owner and template memberships of current and retained runtimes, before privacy projection.
    pub runtimes: HashMap<(Id, Id), Vec<Dependent>>,
}

#[derive(sqlx::FromRow)]
struct RuntimeHoldRow {
    id: Id,
    host_id: Id,
    resources_used: Option<Json>,
    owner: Id,
    owner_kind: String,
    name: String,
    retained: bool,
    until: Option<DateTime<Utc>>,
    template_id: Option<Id>,
    template_name: Option<String>,
}

/// Current generations and every kept recovery generation use the same exact hold rule.
const RUNTIME_HOLD: &str = "(r.lost_at IS NULL AND ((e.lost_at IS NULL AND e.deleted_at IS NULL AND r.environment_id IS NOT NULL AND r.generation=e.generation AND r.successor_of IS NULL AND r.state<>'retired') OR (e.lost_at IS NULL AND e.purged_at IS NULL AND e.removal_phase IN ('removing','kept','restoring','purging')) OR (r.provider_id IS NOT NULL AND p.deleted_at IS NULL AND r.successor_of IS NULL AND r.state<>'retired') OR EXISTS(SELECT 1 FROM jobs lifecycle WHERE lifecycle.state IN ('queued','running') AND (lifecycle.environment_id=r.environment_id OR lifecycle.provider_id=r.provider_id))))";

/// The one retained/current hold reader used by both cleanup paths and dependency projections.
pub async fn artifact_holds(conn: &mut Conn, hosts: &[Id]) -> Result<ArtifactHolds, EngineError> {
    let rows:Vec<RuntimeHoldRow>=sqlx::query_as(sqlx::AssertSqlSafe(format!("SELECT r.id,r.host_id,r.resources_used,COALESCE(r.environment_id,r.provider_id) AS owner,CASE WHEN r.environment_id IS NOT NULL THEN 'environment' ELSE 'provider' END AS owner_kind,COALESCE(e.name,p.name) AS name,(e.deleted_at IS NOT NULL) AS retained,e.volumes_purge_at AS until,r.template_id,t.name AS template_name FROM runtimes r LEFT JOIN environments e ON e.id=r.environment_id LEFT JOIN providers p ON p.id=r.provider_id LEFT JOIN env_templates t ON t.id=r.template_id WHERE r.host_id=ANY($1) AND {RUNTIME_HOLD}")))
        .bind(hosts).fetch_all(conn).await.map_err(DbError::from)?;
    let mut result = ArtifactHolds::default();
    for row in rows {
        let owner = ObjectRef {
            kind: row.owner_kind.parse().map_err(EngineError::unreadable)?,
            id: row.owner,
        };
        let mut memberships = vec![Dependent {
            key: format!("owner:{}:{}", owner.kind, owner.id),
            visibility: Some(owner),
            object: Some(owner),
            name: row.name,
            relation: if row.retained {
                DependentRelation::Restore
            } else {
                DependentRelation::Attachment
            },
            until: row.until.map(Time::new),
        }];
        if let Some((id, name)) = row.template_id.zip(row.template_name) {
            let template = ObjectRef {
                kind: ObjectKind::Template,
                id,
            };
            memberships.push(Dependent {
                key: format!("template:{id}"),
                visibility: Some(template),
                object: Some(template),
                name,
                relation: DependentRelation::Serves,
                until: None,
            });
        }
        result.runtimes.insert((row.host_id, row.id), memberships);
        match row
            .resources_used
            .and_then(|value| serde_json::from_value::<Vec<ResourceRef>>(value).ok())
        {
            Some(uses) => result.known.entry(row.host_id).or_default().extend(uses),
            None => {
                result.unknown.insert(row.host_id);
            }
        }
    }
    Ok(result)
}

/// The same native reverse edges used by reports, destructive plans and cache admission.
#[must_use]
pub fn native_dependents<'a>(
    inventory: &'a ResourceInventory,
    subject: &ResourceRef,
) -> Vec<&'a Resource> {
    inventory
        .resources
        .iter()
        .filter(|item| item.depends_on.contains(subject))
        .collect()
}

/// Exact cache admission, using the node's occupancy fence rather than a use count or rollup.
#[must_use]
pub fn unused_caches(
    inventory: &ResourceInventory,
    days: i64,
    at: DateTime<Utc>,
) -> Vec<&Resource> {
    if days <= 0 || !inventory.unavailable.is_empty() {
        return Vec::new();
    }
    let held: HashSet<_> = inventory
        .resources
        .iter()
        .flat_map(|row| &row.depends_on)
        .collect();
    inventory
        .resources
        .iter()
        .filter(|row| {
            matches!(
                row.resource.kind,
                ResourceKind::Image | ResourceKind::Build | ResourceKind::ToolVersion
            ) && !row.busy
                && !held.contains(&row.resource)
                && row
                    .last_used_at
                    .is_some_and(|used| at.signed_duration_since(used.get()).num_days() >= days)
        })
        .collect()
}

/// One requested reported-use unit. Producers retain stable execution and captured actor identities.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UseRequest {
    /// The resource being measured.
    pub object: ObjectRef,
    /// One immutable skill version, or none for the resource's aggregate.
    pub version_id: Option<Id>,
    /// The applicable unit, never inferred from availability or attachment.
    pub kind: ResourceUseKind,
}

/// Additional durable use sources join the same service in a single batch. Sources must deduplicate
/// native operation identities, preserve captured people, state coverage and use UTC daily buckets.
#[async_trait]
pub trait ReportedUses: Send + Sync {
    /// Reads the requested subjects at one instant; absent results mean not recorded, never zero.
    async fn read(
        &self,
        conn: &mut Conn,
        requested: &[UseRequest],
        at: Time,
    ) -> Result<Vec<(UseRequest, ResourceUses)>, EngineError>;
}

/// Reads additional units, including exact skill versions, through the same batch and coverage rules.
/// Callers authorize the parent resource and validate a version's membership before requesting it.
pub async fn reported_uses(
    conn: &mut Conn,
    requested: &[UseRequest],
    at: Time,
    source: Option<&dyn ReportedUses>,
) -> Result<Vec<(UseRequest, ResourceUses)>, EngineError> {
    let wanted: HashSet<_> = requested
        .iter()
        .map(|request| (request.object, request.version_id, request.kind))
        .collect();
    let mut values = HashMap::new();
    if let Some(source) = source {
        for (request, value) in source.read(conn, requested, at).await? {
            let key = (request.object, request.version_id, request.kind);
            if wanted.contains(&key) && request.kind == value.kind {
                values.insert(key, value);
            }
        }
    }
    Ok(requested
        .iter()
        .map(|request| {
            let value = values
                .get(&(request.object, request.version_id, request.kind))
                .cloned()
                .unwrap_or(ResourceUses {
                    kind: request.kind,
                    coverage: UseCoverage::Unrecorded,
                    last_used_at: None,
                    all_time: None,
                    last_30_days: None,
                    people: None,
                    people_30_days: None,
                });
            (request.clone(), value)
        })
        .collect())
}

/// A complete internal edge. Its visibility identity is checked before any name reaches a client.
#[derive(Debug, Clone)]
pub struct Dependent {
    /// Stable identity within its relationship, for deduplication.
    pub key: String,
    /// The object whose visibility licenses this name; none means system settings.
    pub visibility: Option<ObjectRef>,
    /// The object to follow, where the dependent has its own page.
    pub object: Option<ObjectRef>,
    /// Its current name, kept internal until projected.
    pub name: String,
    /// Why it depends on the subject.
    pub relation: DependentRelation,
    /// The frozen restore deadline, if any.
    pub until: Option<Time>,
}

/// Internal complete facts. Callers authorize the subject itself, then project all dependent names
/// through the ordinary access engine. A plan uses these same edges, including hidden counts.
#[derive(Debug, Clone)]
pub struct RawFacts {
    /// The requested subject.
    pub subject: FactSubject,
    /// The instant shared by the entire read.
    pub as_of: Time,
    /// Every known current edge, before permission projection.
    pub dependents: Vec<Dependent>,
    /// Whether every source needed to decide on this subject could be read.
    pub dependents_complete: bool,
    /// Reported use, absent for artifacts and resources without a meaningful use unit.
    pub uses: Option<ResourceUses>,
}

fn view(kind: ObjectKind) -> Option<Permission> {
    Some(match kind {
        ObjectKind::Provider => Permission::ProviderView,
        ObjectKind::Agent => Permission::AgentView,
        ObjectKind::Environment => Permission::EnvironmentView,
        ObjectKind::Host => Permission::HostView,
        ObjectKind::Machine => Permission::MachineView,
        ObjectKind::Template => Permission::TemplateView,
        ObjectKind::McpServer => Permission::McpView,
        ObjectKind::Mount => Permission::MountView,
        ObjectKind::Skill => Permission::SkillView,
        _ => return None,
    })
}

fn manage(kind: ObjectKind) -> Option<Permission> {
    Some(match kind {
        ObjectKind::Provider => Permission::ProviderManage,
        ObjectKind::Agent => Permission::AgentManage,
        ObjectKind::Environment => Permission::EnvironmentManage,
        ObjectKind::Host => Permission::HostManage,
        ObjectKind::Machine => Permission::MachineManage,
        ObjectKind::Template => Permission::TemplateManage,
        ObjectKind::McpServer => Permission::McpManage,
        ObjectKind::Mount => Permission::MountManage,
        ObjectKind::Skill => Permission::SkillManage,
        _ => return None,
    })
}

/// Ordinary authority on one current or retained resource, read in the same native access frame.
#[derive(Debug, Clone)]
pub struct Authority {
    /// The source resource, kept internal until its rights are checked.
    pub resource: orch_authz::resources::Resource,
    /// Its name and stored content may be read.
    pub readable: bool,
    /// The current caller, including its token boundary, holds Manage.
    pub manageable: bool,
    /// Manage is held as a current home administrator, which may shorten a recovery promise.
    pub administers: bool,
}

/// Reads resource and retained-file authority in one frame. Retained resources keep the same page;
/// this does not authorize reading deleted place/document bodies or bypass a restoration guard.
pub async fn authorities(
    conn: &mut Conn,
    principal: &Principal,
    ids: &[Id],
) -> Result<Vec<Authority>, EngineError> {
    let mut graph = orch_authz::engine::Graph::default();
    let resources = orch_authz::resources::read_into(conn, &mut graph, ids).await?;
    let sites: Vec<_> = resources
        .iter()
        .filter_map(|resource| orch_authz::resources::site_of(&graph, resource))
        .collect();
    let wanted: Vec<_> = sites
        .iter()
        .flat_map(|site| view(site.kind).into_iter().chain(manage(site.kind)))
        .collect();
    let frame =
        orch_authz::Frame::load(conn, principal, Policy::default(), graph, &sites, &wanted).await?;
    Ok(resources
        .into_iter()
        .filter_map(|resource| {
            let site = sites
                .iter()
                .find(|site| site.id == resource.id && site.kind == resource.object().kind)?;
            let active = !principal.suspension().applies(&site.orgs);
            let readable = active
                && view(site.kind)
                    .is_some_and(|permission| !frame.ways(site, permission).is_empty());
            let manageable = active
                && manage(site.kind)
                    .is_some_and(|permission| !frame.ways(site, permission).is_empty());
            Some(Authority {
                resource,
                readable,
                manageable,
                administers: manageable && frame.administers(site),
            })
        })
        .collect())
}

/// Projects an entire facts batch together: statements grow with kinds, never with rows. Both
/// people-facing lists and destructive plans receive complete hidden counts from this projection.
pub async fn project(
    conn: &mut Conn,
    principal: &Principal,
    policy: Policy,
    facts: &[RawFacts],
) -> Result<Vec<ResourceFacts>, EngineError> {
    let mut by_kind = BTreeMap::<ObjectKind, BTreeSet<Id>>::new();
    for edge in facts.iter().flat_map(|row| &row.dependents) {
        if let Some(object) = edge.visibility {
            by_kind.entry(object.kind).or_default().insert(object.id);
        }
    }
    let mut visible = HashSet::new();
    for (kind, ids) in by_kind {
        let ids: Vec<_> = ids.into_iter().collect();
        if let Some(permission) = view(kind) {
            for row in
                orch_authz::resources::holding(conn, principal, &ids, permission, policy).await?
            {
                visible.insert((kind, row.id));
            }
        } else if kind.node().is_some() {
            for row in orch_authz::nodes::holding(
                conn,
                principal,
                &ids,
                PermissionName::OfNode(NodePermission::View),
                Act::Use,
                policy,
            )
            .await?
            {
                visible.insert((kind, row.id));
            }
        }
    }
    Ok(facts
        .iter()
        .map(|raw| {
            let mut row = ResourceFacts {
                subject: raw.subject.clone(),
                as_of: raw.as_of,
                dependents: Vec::new(),
                dependents_complete: raw.dependents_complete,
                uses: raw.uses.clone(),
            };
            let mut groups = BTreeMap::<DependentRelation, DependentGroup>::new();
            for edge in &raw.dependents {
                let group = groups
                    .entry(edge.relation)
                    .or_insert_with(|| DependentGroup {
                        relation: edge.relation,
                        count: 0,
                        visible: Vec::new(),
                        hidden: 0,
                        until: None,
                    });
                group.until = if group.count == 0 {
                    edge.until
                } else {
                    group
                        .until
                        .zip(edge.until)
                        .map(|(left, right)| left.max(right))
                };
                group.count += 1;
                let allowed = edge.visibility.map_or_else(
                    || principal.is_system_admin(),
                    |object| visible.contains(&(object.kind, object.id)),
                );
                if allowed {
                    group.visible.push(VisibleDependent {
                        object: edge.object,
                        name: edge.name.clone(),
                        relation: edge.relation,
                        until: edge.until,
                    });
                } else {
                    group.hidden += 1;
                }
            }
            row.dependents = groups.into_values().collect();
            row
        })
        .collect())
}

/// Reads existing fact sources without an additional use producer.
pub async fn read(
    conn: &mut Conn,
    subjects: &[FactSubject],
    at: Time,
) -> Result<Vec<RawFacts>, EngineError> {
    read_with(conn, subjects, at, None).await
}

/// Reads resources and artifacts in a batch, optionally adding the library's reported-use sources.
/// The caller must check the subjects' own read/manage authority before returning this data.
pub async fn read_with(
    conn: &mut Conn,
    subjects: &[FactSubject],
    at: Time,
    reported: Option<&dyn ReportedUses>,
) -> Result<Vec<RawFacts>, EngineError> {
    read_inner(conn, subjects, at, reported, true).await
}

/// Reads the same current edges without scanning historical use when a selector needs only bindings.
pub async fn dependents(
    conn: &mut Conn,
    subjects: &[FactSubject],
    at: Time,
) -> Result<Vec<RawFacts>, EngineError> {
    read_inner(conn, subjects, at, None, false).await
}

/// A model's current bindings are qualified by its immutable routing name within the provider.
pub async fn target_dependents(
    conn: &mut Conn,
    target: &orch_api_contract::resource_facts::FactTarget,
    at: Time,
) -> Result<RawFacts, EngineError> {
    let mut row = dependents(
        conn,
        &[FactSubject::Resource {
            object: target.object,
        }],
        at,
    )
    .await?
    .into_iter()
    .next()
    .ok_or_else(|| EngineError::unreadable("the impact subject is missing"))?;
    if let Some(model) = target.model_id {
        let name: Option<String> = sqlx::query_scalar(
            "SELECT model_name FROM provider_models WHERE provider_id=$1 AND id=$2",
        )
        .bind(target.object.id)
        .bind(model)
        .fetch_optional(&mut *conn)
        .await
        .map_err(DbError::from)?;
        let threads: Vec<_> = row
            .dependents
            .iter()
            .filter(|edge| edge.relation == DependentRelation::Binding)
            .filter_map(|edge| {
                edge.object
                    .filter(|object| object.kind == ObjectKind::Thread)
                    .map(|object| object.id)
            })
            .collect();
        let models = orch_settings::resolver::get_at_nodes::<String>(
            conn,
            &threads,
            orch_settings_contract::registry::keys::AGENT_MODEL,
        )
        .await
        .map_err(EngineError::from)?;
        row.dependents.retain(|edge| {
            edge.relation == DependentRelation::Binding
                && edge.object.is_some_and(|object| {
                    name.is_some()
                        && models.get(&object.id).and_then(Option::as_ref) == name.as_ref()
                })
        });
        row.uses = None;
    }
    Ok(row)
}

async fn read_inner(
    conn: &mut Conn,
    subjects: &[FactSubject],
    at: Time,
    reported: Option<&dyn ReportedUses>,
    include_uses: bool,
) -> Result<Vec<RawFacts>, EngineError> {
    let objects: Vec<_> = subjects
        .iter()
        .filter_map(|subject| match subject {
            FactSubject::Resource { object } => Some(*object),
            FactSubject::Artifact { .. } | FactSubject::ServerArtifact { .. } => None,
        })
        .collect();
    let ids: Vec<_> = objects.iter().map(|object| object.id).collect();
    let mut edges = HashMap::<(String, Id), Vec<Dependent>>::new();
    for edge in references(conn, &ids).await? {
        edges
            .entry((edge.subject_kind.clone(), edge.subject_id))
            .or_default()
            .push(edge.dependent()?);
    }
    let mut uses = HashMap::<(ObjectKind, Id), ResourceUses>::new();
    if include_uses {
        for (kind, dimension) in [
            (ObjectKind::Provider, ResourceTurnDimension::Provider),
            (ObjectKind::Agent, ResourceTurnDimension::Agent),
            (ObjectKind::Environment, ResourceTurnDimension::Environment),
        ] {
            let wanted: Vec<_> = objects
                .iter()
                .filter(|object| object.kind == kind)
                .map(|object| object.id)
                .collect();
            if wanted.is_empty() {
                continue;
            }
            for id in &wanted {
                uses.insert(
                    (kind, *id),
                    ResourceUses {
                        kind: ResourceUseKind::Turn,
                        coverage: UseCoverage::Complete,
                        last_used_at: None,
                        all_time: Some(0),
                        last_30_days: Some(0),
                        people: Some(0),
                        people_30_days: Some(0),
                    },
                );
            }
            for row in rollup::resource_turn_counts(conn, &wanted, dimension, at.get()).await? {
                uses.insert(
                    (kind, row.id),
                    ResourceUses {
                        kind: ResourceUseKind::Turn,
                        coverage: UseCoverage::Complete,
                        last_used_at: row.last_at.map(Time::new),
                        all_time: Some(row.all_time),
                        last_30_days: Some(row.last_30_days),
                        people: row.people,
                        people_30_days: row.people_30_days,
                    },
                );
            }
        }
        let templates: Vec<(Id, String)> =
            sqlx::query_as("SELECT id,kind FROM env_templates WHERE id=ANY($1)")
                .bind(&ids)
                .fetch_all(&mut *conn)
                .await
                .map_err(DbError::from)?;
        let templates: HashMap<_, _> = templates.into_iter().collect();
        let mut wanted = Vec::new();
        for object in &objects {
            let kind = match object.kind {
                ObjectKind::McpServer => Some(ResourceUseKind::McpCall),
                ObjectKind::Skill => Some(ResourceUseKind::SkillInvocation),
                ObjectKind::Host => Some(ResourceUseKind::EnvironmentMade),
                ObjectKind::Template => templates.get(&object.id).map(|kind| {
                    if kind == "provider" {
                        ResourceUseKind::ProviderMade
                    } else {
                        ResourceUseKind::EnvironmentMade
                    }
                }),
                _ => None,
            };
            if let Some(kind) = kind {
                uses.insert(
                    (object.kind, object.id),
                    ResourceUses {
                        kind,
                        coverage: UseCoverage::Unrecorded,
                        last_used_at: None,
                        all_time: None,
                        last_30_days: None,
                        people: None,
                        people_30_days: None,
                    },
                );
                wanted.push(UseRequest {
                    object: *object,
                    version_id: None,
                    kind,
                });
            }
        }
        for (request, value) in reported_uses(conn, &wanted, at, reported).await? {
            uses.insert((request.object.kind, request.object.id), value);
        }
    }
    let hosts: Vec<_> = subjects
        .iter()
        .filter_map(|subject| match subject {
            FactSubject::Artifact { host_id, .. } => Some(*host_id),
            _ => None,
        })
        .collect();
    let inventories: Vec<(Id, Option<serde_json::Value>, bool)> = sqlx::query_as("SELECT h.id,h.resource_inventory,(m.online AND m.status='active' AND m.deleted_at IS NULL) FROM hosts h JOIN machines m ON m.id=h.machine_id WHERE h.id=ANY($1)")
        .bind(&hosts).fetch_all(&mut *conn).await.map_err(DbError::from)?;
    let holds = artifact_holds(conn, &hosts).await?;
    let artifacts: Vec<_> = subjects
        .iter()
        .filter_map(|subject| match subject {
            FactSubject::Artifact {
                host_id,
                artifact_kind,
                id,
            } => Some(serde_json::json!({"host":host_id,"kind":artifact_kind,"id":id})),
            _ => None,
        })
        .collect();
    let artifact_refs: Vec<ArtifactEdge> = sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "SELECT x.host,x.id AS artifact,r.template_id,t.name AS template_name,
          COALESCE(r.environment_id,r.provider_id) AS owner,
          CASE WHEN r.environment_id IS NULL THEN 'provider' ELSE 'environment' END AS owner_kind,
          COALESCE(e.name,p.name) AS name,e.volumes_purge_at AS until,(e.deleted_at IS NOT NULL) AS retained
         FROM jsonb_to_recordset($1) x(host uuid,kind text,id text)
         JOIN runtimes r ON r.host_id=x.host AND r.resources_used @> jsonb_build_array(jsonb_build_object('kind',x.kind,'id',x.id))
         JOIN env_templates t ON t.id=r.template_id
         LEFT JOIN environments e ON e.id=r.environment_id LEFT JOIN providers p ON p.id=r.provider_id
         WHERE {RUNTIME_HOLD}")))
        .bind(serde_json::json!(artifacts)).fetch_all(&mut *conn).await.map_err(DbError::from)?;
    let inventories: HashMap<_, _> = inventories
        .into_iter()
        .filter_map(|(host, value, online)| {
            value
                .and_then(|value| serde_json::from_value::<ResourceInventory>(value).ok())
                .map(|inventory| (host, (inventory, online)))
        })
        .collect();
    let mut native_edges = HashMap::<(Id, ResourceRef), Vec<Dependent>>::new();
    let mut native_objects = HashMap::new();
    for (host, (inventory, _)) in &inventories {
        for item in &inventory.resources {
            native_objects.insert((*host, item.resource.clone()), item);
            for dependency in &item.depends_on {
                native_edges
                    .entry((*host, dependency.clone()))
                    .or_default()
                    .push(Dependent {
                        key: item.resource.id.clone(),
                        visibility: Some(ObjectRef {
                            kind: ObjectKind::Host,
                            id: *host,
                        }),
                        object: None,
                        name: item.name.clone(),
                        relation: DependentRelation::NativeDependency,
                        until: None,
                    });
            }
        }
    }
    let mut held_edges = HashMap::<(Id, String), Vec<Dependent>>::new();
    let mount_refs:Vec<MountArtifactEdge>=sqlx::query_as("SELECT h.id AS host,l.mount_id AS mount,m.name,l.source,false AS retained,('location:'||l.mount_id::text||':'||l.machine_id::text) AS membership FROM mount_locations l JOIN hosts h ON h.machine_id=l.machine_id JOIN mounts m ON m.id=l.mount_id WHERE h.id=ANY($1) AND l.kind='volume' UNION ALL SELECT h.id,k.mount_id,m.name,k.source,true,('kept:'||k.id::text) FROM mount_kept_data k JOIN hosts h ON h.machine_id=k.machine_id JOIN mounts m ON m.id=k.mount_id WHERE h.id=ANY($1) AND k.kind='volume' AND k.lost_at IS NULL")
        .bind(&hosts).fetch_all(&mut *conn).await.map_err(DbError::from)?;
    let mut volumes_by_name = HashMap::new();
    for ((host, reference), native) in &native_objects {
        if reference.kind == ResourceKind::Volume {
            volumes_by_name
                .entry((*host, native.name.as_str()))
                .or_insert_with(Vec::new)
                .push(reference.id.as_str());
        }
    }
    for held in mount_refs {
        for id in volumes_by_name
            .get(&(held.host, held.source.as_str()))
            .into_iter()
            .flatten()
        {
            let object = ObjectRef {
                kind: ObjectKind::Mount,
                id: held.mount,
            };
            held_edges
                .entry((held.host, (*id).to_owned()))
                .or_default()
                .push(Dependent {
                    key: held.membership.clone(),
                    visibility: Some(object),
                    object: Some(object),
                    name: held.name.clone(),
                    relation: if held.retained {
                        DependentRelation::Restore
                    } else {
                        DependentRelation::Attachment
                    },
                    until: None,
                });
        }
    }
    for retained in artifact_refs {
        let owner = ObjectRef {
            kind: retained
                .owner_kind
                .parse()
                .map_err(EngineError::unreadable)?,
            id: retained.owner,
        };
        let template = ObjectRef {
            kind: ObjectKind::Template,
            id: retained.template_id,
        };
        let rows = held_edges
            .entry((retained.host, retained.artifact))
            .or_default();
        rows.push(Dependent {
            key: format!("owner:{}:{}", owner.kind, owner.id),
            visibility: Some(owner),
            object: Some(owner),
            name: retained.name,
            relation: if retained.retained {
                DependentRelation::Restore
            } else {
                DependentRelation::Attachment
            },
            until: retained.until.map(Time::new),
        });
        rows.push(Dependent {
            key: format!("template:{}", retained.template_id),
            visibility: Some(template),
            object: Some(template),
            name: retained.template_name,
            relation: DependentRelation::Serves,
            until: None,
        });
    }
    let mut result = Vec::with_capacity(subjects.len());
    let unrecorded_delivery: (bool, bool) = if objects
        .iter()
        .any(|object| matches!(object.kind, ObjectKind::McpServer | ObjectKind::Skill))
    {
        sqlx::query_as("SELECT EXISTS(SELECT 1 FROM threads h JOIN segments s ON s.id=h.current_segment_id JOIN nodes n ON n.id=h.node_id WHERE n.deleted_at IS NULL AND s.native_session_ref IS NOT NULL AND s.delivered_resources->>'state' IS DISTINCT FROM 'closed' AND COALESCE(jsonb_typeof(s.delivered_resources->'mcp'),'null')<>'array') OR EXISTS(SELECT 1 FROM environments e CROSS JOIN LATERAL jsonb_array_elements(CASE WHEN jsonb_typeof(e.facts->'mcp_servers')='array' THEN e.facts->'mcp_servers' ELSE '[]'::jsonb END) server WHERE e.deleted_at IS NULL AND server->>'server_id' IS NULL),EXISTS(SELECT 1 FROM threads h JOIN segments s ON s.id=h.current_segment_id JOIN nodes n ON n.id=h.node_id WHERE n.deleted_at IS NULL AND s.native_session_ref IS NOT NULL AND s.delivered_resources->>'state' IS DISTINCT FROM 'closed' AND COALESCE(jsonb_typeof(s.delivered_resources->'skills'),'null')<>'array')")
        .fetch_one(&mut *conn).await.map_err(DbError::from)?
    } else {
        (false, false)
    };
    for subject in subjects {
        let mut row = RawFacts {
            subject: subject.clone(),
            as_of: at,
            dependents: Vec::new(),
            dependents_complete: true,
            uses: None,
        };
        match subject {
            FactSubject::ServerArtifact { .. } => {
                row.dependents_complete = false;
            }
            FactSubject::Resource { object } => {
                row.dependents_complete = object.kind.resource().is_some();
                if object.kind == ObjectKind::McpServer && unrecorded_delivery.0
                    || object.kind == ObjectKind::Skill && unrecorded_delivery.1
                {
                    row.dependents_complete = false;
                }
                row.uses = uses.get(&(object.kind, object.id)).cloned();
                row.dependents = edges
                    .get(&(object.kind.as_str().to_owned(), object.id))
                    .cloned()
                    .unwrap_or_default();
            }
            FactSubject::Artifact {
                host_id,
                artifact_kind,
                id,
            } => {
                let kind =
                    serde_json::from_value::<ResourceKind>(serde_json::json!(artifact_kind)).ok();
                let reference = kind.map(|kind| ResourceRef {
                    kind,
                    id: id.clone(),
                });
                let saved = inventories.get(host_id);
                row.dependents_complete = saved
                    .is_some_and(|(inventory, online)| *online && inventory.unavailable.is_empty())
                    && !holds.unknown.contains(host_id);
                if let Some(reference) = reference {
                    row.dependents = native_edges
                        .get(&(*host_id, reference.clone()))
                        .cloned()
                        .unwrap_or_default();
                    if let Some(native) = native_objects.get(&(*host_id, reference)) {
                        if matches!(
                            native.resource.kind,
                            ResourceKind::Unit | ResourceKind::Volume | ResourceKind::KeptLayer
                        ) && let Some(memberships) = native
                            .labels
                            .get(orch_driver_contract::runtime::LABEL_RUNTIME_ID)
                            .and_then(|id| id.parse::<Id>().ok())
                            .and_then(|id| holds.runtimes.get(&(*host_id, id)))
                        {
                            row.dependents.extend(memberships.iter().cloned());
                        }
                        if native.busy
                            && native.resource.kind != ResourceKind::Unit
                            && row.dependents.is_empty()
                        {
                            row.dependents_complete = false;
                        }
                    } else {
                        row.dependents_complete = false;
                    }
                } else {
                    row.dependents_complete = false;
                }
                row.dependents.extend(
                    held_edges
                        .get(&(*host_id, id.clone()))
                        .cloned()
                        .unwrap_or_default(),
                );
            }
        }
        let mut seen = HashSet::new();
        row.dependents
            .retain(|edge| seen.insert((edge.key.clone(), edge.relation)));
        result.push(row);
    }
    Ok(result)
}

/// The operator supplies references without counts; the ordinary privacy projection groups them once.
pub fn operator_item(
    item: &orch_api_contract::storage::OperatorStorageItem,
    at: Time,
) -> Result<RawFacts, EngineError> {
    use orch_api_contract::storage::StorageKind;
    let kind = match item.record.kind {
        StorageKind::Release => "release",
        StorageKind::Dump => "dump",
        _ => {
            return Err(EngineError::unreadable(
                "the operator artifact kind is invalid",
            ));
        }
    };
    let mut seen = HashSet::new();
    let dependents = item
        .dependencies
        .iter()
        .filter(|edge| seen.insert((edge.key.clone(), edge.relation)))
        .map(|edge| Dependent {
            key: edge.key.clone(),
            visibility: None,
            object: None,
            name: edge.name.clone(),
            relation: edge.relation,
            until: None,
        })
        .collect();
    Ok(RawFacts {
        subject: FactSubject::ServerArtifact {
            artifact_kind: kind.into(),
            id: item.record.id.clone(),
        },
        as_of: at,
        dependents,
        dependents_complete: true,
        uses: None,
    })
}

#[derive(sqlx::FromRow)]
struct ArtifactEdge {
    host: Id,
    artifact: String,
    template_id: Id,
    template_name: String,
    owner: Id,
    owner_kind: String,
    name: String,
    until: Option<DateTime<Utc>>,
    retained: bool,
}

#[derive(sqlx::FromRow)]
struct MountArtifactEdge {
    host: Id,
    mount: Id,
    name: String,
    source: String,
    retained: bool,
    membership: String,
}

#[derive(sqlx::FromRow)]
struct EdgeRow {
    subject_kind: String,
    subject_id: Id,
    kind: Option<String>,
    id: Option<Id>,
    name: String,
    relation: String,
    until: Option<DateTime<Utc>>,
    #[sqlx(default)]
    reference_key: Option<String>,
}

impl EdgeRow {
    fn dependent(&self) -> Result<Dependent, EngineError> {
        let object = self
            .kind
            .as_deref()
            .zip(self.id)
            .map(|(kind, id)| {
                kind.parse()
                    .map(|kind| ObjectRef { kind, id })
                    .map_err(EngineError::unreadable)
            })
            .transpose()?;
        let relation = match self.relation.as_str() {
            "binding" => DependentRelation::Binding,
            "explicit" => DependentRelation::ExplicitGiving,
            "restore" => DependentRelation::Restore,
            "operation" => DependentRelation::Operation,
            "serves" => DependentRelation::Serves,
            _ => DependentRelation::Attachment,
        };
        Ok(Dependent {
            key: self.reference_key.clone().unwrap_or_else(|| {
                object.map_or_else(
                    || "system".into(),
                    |object| format!("{}:{}", object.kind, object.id),
                )
            }),
            visibility: object,
            object,
            name: self.name.clone(),
            relation,
            until: self.until.map(Time::new),
        })
    }
}

async fn references(conn: &mut Conn, ids: &[Id]) -> Result<Vec<EdgeRow>, EngineError> {
    let mut rows: Vec<EdgeRow> = sqlx::query_as(include_str!("resource_facts.sql"))
        .bind(ids)
        .fetch_all(&mut *conn)
        .await
        .map_err(DbError::from)?;
    rows.extend(
        sqlx::query_as::<_, EdgeRow>(include_str!("resource_job_facts.sql"))
            .bind(ids)
            .fetch_all(conn)
            .await
            .map_err(DbError::from)?,
    );
    Ok(rows)
}
