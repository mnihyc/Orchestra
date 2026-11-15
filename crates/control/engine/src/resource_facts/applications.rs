//! Native deliveries and running attachments determine when a resource change reaches existing work.

use super::{PreparedVisibility, RawFacts};
use crate::EngineError;
use orch_access_contract::{decisions::ObjectRef, permissions::ObjectKind};
use orch_api_contract::resource_facts::{
    DependentRelation, ResourceApplicationGroup, ResourceApplicationWhen,
};
use orch_db::{Conn, DbError};
use orch_types::Id;
use std::collections::{BTreeMap, BTreeSet, HashSet};

/// A private native group, projected through the same name sources as the ordinary facts.
pub struct Application {
    /// Its application boundary.
    pub when: ResourceApplicationWhen,
    /// Its count's unit.
    pub object_kind: ObjectKind,
    facts: RawFacts,
}

impl Application {
    /// The exact memberships used to recheck an opened native snapshot.
    pub fn members(&self) -> impl Iterator<Item = (ResourceApplicationWhen, ObjectRef)> + '_ {
        self.facts
            .dependents
            .iter()
            .filter_map(move |edge| edge.object.map(|object| (self.when, object)))
    }

    /// Counts stay complete while independently unreadable names remain hidden.
    pub fn project(&self, visibility: &PreparedVisibility) -> Option<ResourceApplicationGroup> {
        let facts = visibility
            .project(std::slice::from_ref(&self.facts))
            .pop()?;
        let group = facts
            .dependents
            .into_iter()
            .find(|group| group.relation == DependentRelation::Attachment)?;
        Some(ResourceApplicationGroup {
            when: self.when,
            object_kind: self.object_kind,
            count: group.count,
            visible: group.visible,
            hidden: group.hidden,
        })
    }
}

/// Uses the shared current membership reader; historical provenance and explicit giving are not live use.
pub async fn read(
    conn: &mut Conn,
    kind: ObjectKind,
    facts: &RawFacts,
) -> Result<Vec<Application>, EngineError> {
    use ResourceApplicationWhen::{BeforeNextTurn, NextStart, Now};
    let boundaries: &[(ResourceApplicationWhen, ObjectKind)] = match kind {
        ObjectKind::McpServer => &[
            (Now, ObjectKind::Thread),
            (BeforeNextTurn, ObjectKind::Thread),
        ],
        ObjectKind::Skill => &[(BeforeNextTurn, ObjectKind::Thread)],
        ObjectKind::Mount | ObjectKind::Template => &[
            (NextStart, ObjectKind::Environment),
            (NextStart, ObjectKind::Provider),
        ],
        _ => return Ok(Vec::new()),
    };
    let running = if matches!(kind, ObjectKind::Mount | ObjectKind::Template) {
        let ids = |kind| {
            facts
                .dependents
                .iter()
                .filter(|edge| edge.relation == DependentRelation::Attachment)
                .filter_map(|edge| {
                    edge.object
                        .filter(|object| object.kind == kind)
                        .map(|object| object.id)
                })
                .collect::<BTreeSet<_>>()
                .into_iter()
                .collect::<Vec<_>>()
        };
        let environments = ids(ObjectKind::Environment);
        let providers = ids(ObjectKind::Provider);
        let rows: Vec<(String, Id)> = sqlx::query_as("SELECT 'environment'::text,e.id FROM environments e WHERE e.id=ANY($1) AND e.status='running' AND e.deleted_at IS NULL AND e.lost_at IS NULL UNION SELECT 'provider',p.id FROM providers p JOIN runtimes r ON r.provider_id=p.id WHERE p.id=ANY($2) AND p.deleted_at IS NULL AND r.state='running' AND r.lost_at IS NULL")
            .bind(&environments).bind(&providers).fetch_all(conn).await.map_err(DbError::from)?;
        rows.into_iter().collect::<HashSet<_>>()
    } else {
        HashSet::new()
    };
    let mut result = Vec::new();
    for &(when, object_kind) in boundaries {
        let mut members = BTreeMap::new();
        for edge in &facts.dependents {
            if edge.relation != DependentRelation::Attachment {
                continue;
            }
            let Some(object) = edge.object.filter(|object| object.kind == object_kind) else {
                continue;
            };
            if when == NextStart && !running.contains(&(object_kind.as_str().to_owned(), object.id))
            {
                continue;
            }
            members.entry(object.id).or_insert_with(|| edge.clone());
        }
        if !members.is_empty() {
            result.push(Application {
                when,
                object_kind,
                facts: RawFacts {
                    subject: facts.subject.clone(),
                    as_of: facts.as_of,
                    dependents: members.into_values().collect(),
                    dependents_complete: facts.dependents_complete,
                    uses: None,
                },
            });
        }
    }
    Ok(result)
}
