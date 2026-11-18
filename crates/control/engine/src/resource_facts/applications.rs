//! Current deliveries and confirmed runtime inputs determine when a resource change still needs application.

use super::{Dependent, PreparedVisibility, RawFacts};
use crate::{Engine, EngineError};
use orch_access_contract::{decisions::ObjectRef, permissions::ObjectKind};
use orch_api_contract::resource_facts::{
    DependentRelation, FactSubject, ResourceApplicationAction, ResourceApplicationGroup,
    ResourceApplicationWhen,
};
use orch_db::{Conn, DbError};
use orch_types::{Id, Json};
use std::collections::{BTreeMap, BTreeSet, HashSet};

pub(crate) mod inputs;

/// Private native facts; incompleteness never means that no configuration still needs applying.
pub struct Applications {
    /// Nonempty groups of current delivery or pending configuration.
    pub groups: Vec<Application>,
    /// Every relevant installed and desired input was reported.
    pub complete: bool,
}

/// A private group projected through the same name sources as ordinary dependency facts.
pub struct Application {
    /// When the change is applied.
    pub when: ResourceApplicationWhen,
    /// The unit counted by this group.
    pub object_kind: ObjectKind,
    facts: RawFacts,
}

impl Application {
    /// Exact memberships rechecked when a native read is published.
    pub fn members(&self) -> impl Iterator<Item = (ResourceApplicationWhen, ObjectRef)> + '_ {
        self.facts
            .dependents
            .iter()
            .filter_map(move |edge| edge.object.map(|object| (self.when, object)))
    }

    /// Native names supplied to the consuming operation's ordinary disclosure frame.
    pub fn facts(&self) -> &RawFacts {
        &self.facts
    }

    /// Keeps private names and unavailable acts outside the public result.
    pub fn project(
        &self,
        visibility: &PreparedVisibility,
        manageable: &HashSet<ObjectRef>,
    ) -> Option<ResourceApplicationGroup> {
        let facts = visibility
            .project(std::slice::from_ref(&self.facts))
            .pop()?;
        let group = facts
            .dependents
            .into_iter()
            .find(|group| group.relation == DependentRelation::Attachment)?;
        let apply = (self.when == ResourceApplicationWhen::Updated)
            .then_some(ResourceApplicationAction::Update);
        let apply_targets = if apply.is_some() {
            group
                .visible
                .iter()
                .filter_map(|edge| edge.object.filter(|object| manageable.contains(object)))
                .collect()
        } else {
            Vec::new()
        };
        Some(ResourceApplicationGroup {
            when: self.when,
            object_kind: self.object_kind,
            count: group.count,
            visible: group.visible,
            hidden: group.hidden,
            apply,
            apply_targets,
        })
    }
}

#[derive(sqlx::FromRow)]
struct Running {
    id: Id,
    target: Id,
    kind: String,
    name: String,
    template_id: Id,
    template_fingerprint: Option<String>,
    mount_fingerprints: Option<Json>,
}

fn make_group(
    facts: &RawFacts,
    when: ResourceApplicationWhen,
    object_kind: ObjectKind,
    members: BTreeMap<Id, Dependent>,
) -> Option<Application> {
    (!members.is_empty()).then(|| Application {
        when,
        object_kind,
        facts: RawFacts {
            subject: facts.subject.clone(),
            as_of: facts.as_of,
            dependents: members.into_values().collect(),
            dependents_complete: facts.dependents_complete,
            uses: None,
        },
    })
}

/// Uses actual delivery and the ordinary batched update observer, never a same-unit restart prediction.
pub async fn read(
    engine: &Engine,
    conn: &mut Conn,
    kind: ObjectKind,
    facts: &RawFacts,
) -> Result<Applications, EngineError> {
    read_for_change(engine, conn, kind, facts, false).await
}

/// A publication reaches current Latest followers; resource-wide changes retain every delivery.
pub async fn read_for_change(
    engine: &Engine,
    conn: &mut Conn,
    kind: ObjectKind,
    facts: &RawFacts,
    skill_publication: bool,
) -> Result<Applications, EngineError> {
    use ResourceApplicationWhen::{BeforeNextTurn, Now, Updated};
    let mut result = Applications {
        groups: Vec::new(),
        complete: facts.dependents_complete,
    };
    if matches!(kind, ObjectKind::McpServer | ObjectKind::Skill) {
        let mut members = facts
            .dependents
            .iter()
            .filter(|edge| edge.relation == DependentRelation::Attachment)
            .filter_map(|edge| {
                edge.object
                    .filter(|object| object.kind == ObjectKind::Thread)
                    .map(|object| (object.id, edge.clone()))
            })
            .collect::<BTreeMap<_, _>>();
        if kind == ObjectKind::Skill && skill_publication {
            let FactSubject::Resource { object } = &facts.subject else {
                return Err(EngineError::unreadable(
                    "a skill publication requires its resource",
                ));
            };
            let threads = members.keys().copied().collect::<Vec<_>>();
            let latest =
                crate::skills::resolve::latest_followers(conn, object.id, &threads).await?;
            members.retain(|thread, _| latest.contains(thread));
        }
        if kind == ObjectKind::McpServer {
            result
                .groups
                .extend(make_group(facts, Now, ObjectKind::Thread, members.clone()));
        }
        result.groups.extend(make_group(
            facts,
            BeforeNextTurn,
            ObjectKind::Thread,
            members,
        ));
        return Ok(result);
    }
    if !matches!(kind, ObjectKind::Mount | ObjectKind::Template) {
        return Ok(result);
    }
    let FactSubject::Resource { object: subject } = &facts.subject else {
        return Err(EngineError::unreadable(
            "an application target must be a resource",
        ));
    };
    let member = subject.id.to_string();
    let selected = |kind| {
        facts
            .dependents
            .iter()
            .filter(|edge| {
                matches!(
                    edge.relation,
                    DependentRelation::Attachment | DependentRelation::ExplicitGiving
                )
            })
            .filter_map(|edge| {
                edge.object
                    .filter(|object| object.kind == kind)
                    .map(|object| object.id)
            })
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect::<Vec<_>>()
    };
    let environments = selected(ObjectKind::Environment);
    let providers = selected(ObjectKind::Provider);
    let scopes = facts
        .dependents
        .iter()
        .filter(|edge| edge.relation == DependentRelation::ExplicitGiving)
        .filter_map(|edge| {
            edge.object
                .filter(|object| {
                    matches!(
                        object.kind,
                        ObjectKind::Org | ObjectKind::Project | ObjectKind::Topic
                    )
                })
                .map(|object| object.id)
        })
        .collect::<Vec<_>>();
    let system = facts
        .dependents
        .iter()
        .any(|edge| edge.relation == DependentRelation::ExplicitGiving && edge.object.is_none());
    if kind == ObjectKind::Mount {
        let unknown: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM runtimes r LEFT JOIN runtime_resource_inputs i ON i.runtime_id=r.id WHERE r.state='running' AND r.lost_at IS NULL AND r.environment_id IS NOT NULL AND i.mount_fingerprints IS NULL)")
            .fetch_one(&mut *conn).await.map_err(DbError::from)?;
        result.complete &= !unknown;
    }
    let running: Vec<Running> = sqlx::query_as("SELECT r.id,COALESCE(e.id,p.id) AS target,r.kind,COALESCE(e.name,p.name) AS name,r.template_id,i.template_fingerprint,i.mount_fingerprints FROM runtimes r LEFT JOIN runtime_resource_inputs i ON i.runtime_id=r.id LEFT JOIN environments e ON e.id=r.environment_id AND e.deleted_at IS NULL LEFT JOIN providers p ON p.id=r.provider_id AND p.deleted_at IS NULL WHERE r.state='running' AND r.lost_at IS NULL AND (e.id IS NOT NULL OR p.id IS NOT NULL) AND (e.id=ANY($1) OR p.id=ANY($2) OR $4 OR EXISTS(SELECT 1 FROM nodes home WHERE home.id=COALESCE(e.home_node_id,p.home_id) AND home.path && $3))")
        .bind(environments).bind(providers).bind(scopes).bind(system).fetch_all(&mut *conn).await.map_err(DbError::from)?;
    let ids = running.iter().map(|runtime| runtime.id).collect::<Vec<_>>();
    let desired = engine.runtime_resource_observations(conn, &ids).await?;
    let mut groups = BTreeMap::<ObjectKind, BTreeMap<Id, Dependent>>::new();
    for runtime in running {
        let Some(wanted) = desired.get(&runtime.id) else {
            result.complete = false;
            continue;
        };
        let changed = if kind == ObjectKind::Mount {
            match (&runtime.mount_fingerprints, &wanted.mount_fingerprints) {
                (Some(held), Some(wanted)) => {
                    held.get(member.as_str()) != wanted.get(member.as_str())
                }
                _ => {
                    result.complete = false;
                    continue;
                }
            }
        } else {
            if runtime.template_id != subject.id && wanted.template_id != Some(subject.id) {
                continue;
            }
            match (&runtime.template_fingerprint, &wanted.template_fingerprint) {
                (Some(held), Some(wanted_fingerprint)) => {
                    runtime.template_id != wanted.template_id.unwrap_or(runtime.template_id)
                        || held != wanted_fingerprint
                }
                _ => {
                    result.complete = false;
                    continue;
                }
            }
        };
        if !changed {
            continue;
        }
        let object_kind = match runtime.kind.as_str() {
            "environment" => ObjectKind::Environment,
            "provider" => ObjectKind::Provider,
            _ => {
                return Err(EngineError::unreadable(
                    "the runtime has an unknown resource kind",
                ));
            }
        };
        let object = ObjectRef {
            kind: object_kind,
            id: runtime.target,
        };
        groups
            .entry(object_kind)
            .or_default()
            .entry(runtime.target)
            .or_insert(Dependent {
                key: format!("application:{object_kind}:{}", runtime.target),
                visibility: Some(object),
                object: Some(object),
                name: runtime.name,
                relation: DependentRelation::Attachment,
                until: None,
            });
    }
    for (kind, members) in groups {
        result
            .groups
            .extend(make_group(facts, Updated, kind, members));
    }
    Ok(result)
}
