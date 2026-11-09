//! Native Node header sources selected before its protected references are formatted.

use super::view;
use crate::middleware::requests::{ApiError, ApiResult};
use crate::registry::Ctx;
use crate::routes::documents::build::{ProjectedDocumentRef, WrittenLabelSource};
use crate::routes::environments::reading::{self, TopicSummarySources};
use orch_access_contract::permissions::{ObjectKind, Permission};
use orch_api_contract::nodes::Node;
use orch_authz::engine::Graph;
use orch_authz::{Policy, Principal};
use orch_db::Conn;
use orch_db::queries::{documents, hierarchy};
use orch_types::Id;
use std::collections::{BTreeMap, BTreeSet, HashMap};

/// Factual source identities and the exact native associations of a successful header.
pub(crate) struct Prepared {
    pub objects: BTreeSet<(ObjectKind, Id)>,
    path: Vec<Id>,
    kind: String,
    readmes: Vec<(Id, Id, Option<Id>)>,
    names: BTreeSet<Id>,
    displays: BTreeSet<Id>,
    authority: view::ReferenceAuthority,
    thread: Option<orch_engine::views::ThreadMetadataSources>,
    topic_bindings: Vec<(Id, Id)>,
    topic_defaults: BTreeMap<Id, Option<Id>>,
    pub topic: Option<TopicSummarySources>,
}

impl Prepared {
    pub(crate) fn preserves(&self, fresh: &Self) -> bool {
        self.path == fresh.path
            && self.kind == fresh.kind
            && self.readmes == fresh.readmes
            && self.thread == fresh.thread
            && self.topic_bindings == fresh.topic_bindings
            && self.topic_defaults == fresh.topic_defaults
            && self.objects.is_subset(&fresh.objects)
            && self.names.is_subset(&fresh.names)
            && self.displays.is_subset(&fresh.displays)
            && self.authority.preserves(&fresh.authority)
    }
}

/// Reuses the original graph, README, topic-environment and native binding selectors.
/// No returned Node or Resource DTO, submitted ancestry or JSON UUID scan supplies a source.
pub(crate) async fn prepare(conn: &mut Conn, principal: &Principal, id: Id) -> ApiResult<Prepared> {
    let root_graph = Graph::load(conn, &[id]).await.map_err(ApiError::internal)?;
    let node = hierarchy::with_ancestors(conn, &[id])
        .await?
        .into_iter()
        .find(|row| row.id == id && row.deleted_at.is_none())
        .ok_or_else(ApiError::not_found)?;
    let mut candidates = root_graph
        .nodes()
        .map(|node| node.id)
        .collect::<BTreeSet<_>>();
    let rows = documents::readmes_of(conn, &[id]).await?;
    let ids = rows.iter().map(|row| row.id).collect::<Vec<_>>();
    let listed = orch_authz::documents::listed(conn, principal, &ids, Policy::default())
        .await
        .map_err(ApiError::internal)?
        .into_iter()
        .map(|listed| (listed.document.id, listed))
        .collect::<HashMap<_, _>>();
    let mut objects = BTreeSet::new();
    let mut readmes = Vec::new();
    for row in rows {
        let Some(listed) = listed
            .get(&row.id)
            .filter(|listed| listed.holds(Permission::DocView))
        else {
            continue;
        };
        objects.insert((ObjectKind::Document, row.id));
        readmes.push((row.node_id, row.id, row.created_by_thread));
        for source in ProjectedDocumentRef::candidates(conn, &row, listed).await? {
            if let WrittenLabelSource::Node(id) = source {
                candidates.insert(id);
            } else {
                return Err(ApiError::internal(
                    "a native README header has a non-node label source",
                ));
            }
        }
    }
    readmes.sort_unstable();
    let topic = if node.kind == "topic" {
        Some(reading::topic_summary_sources(conn, principal, &[id]).await?)
    } else {
        None
    };
    let mut topic_bindings = topic
        .as_ref()
        .map(|topic| topic.bindings.clone())
        .unwrap_or_default();
    topic_bindings.sort_unstable();
    for &(_, environment) in &topic_bindings {
        objects.insert((ObjectKind::Environment, environment));
    }
    let topic_defaults = topic
        .as_ref()
        .map(|topic| {
            topic
                .defaults
                .iter()
                .map(|(&id, &value)| (id, value))
                .collect()
        })
        .unwrap_or_default();
    let thread = if node.kind == "thread" {
        orch_engine::views::thread_metadata_sources(conn, id)
            .await
            .map_err(ApiError::internal)?
    } else {
        None
    };
    let mut forced_names = BTreeSet::new();
    if let Some(thread) = &thread {
        if let Some(environment) = thread.environment {
            objects.insert((ObjectKind::Environment, environment));
        }
        if let Some((_, agent, provider, machine)) = thread.binding {
            objects.insert((ObjectKind::Agent, agent));
            objects.insert((ObjectKind::Provider, provider));
            if let Some(machine) = machine {
                objects.insert((ObjectKind::Machine, machine));
            }
        }
        if let Some(machine) = thread.sandbox_machine {
            objects.insert((ObjectKind::Machine, machine));
        }
        if let Some(parent) = thread.side_of {
            // The original side header names its actual parent directly, even without a parent View gate.
            candidates.insert(parent);
            forced_names.insert(parent);
        }
    }
    let ids = candidates.into_iter().collect::<Vec<_>>();
    let mut names = view::reference_visibility(conn, principal, &ids, Policy::default()).await?;
    names.extend(forced_names);
    let displays =
        view::reference_thread_visibility(conn, principal, &ids, Policy::default()).await?;
    let graph = Graph::load(conn, &names.iter().copied().collect::<Vec<_>>())
        .await
        .map_err(ApiError::internal)?;
    for name in &names {
        let target = graph.get(*name).ok_or_else(ApiError::not_found)?;
        objects.insert((ObjectKind::from(target.kind), *name));
    }
    objects.remove(&(
        node.kind
            .parse::<ObjectKind>()
            .map_err(ApiError::internal)?,
        id,
    ));
    let authority = view::reference_authority(conn, principal, id, Policy::default()).await?;
    Ok(Prepared {
        objects,
        path: node.path,
        kind: node.kind,
        readmes,
        names,
        displays,
        authority,
        thread,
        topic_bindings,
        topic_defaults,
        topic,
    })
}

/// The original native builder, after the caller's actual prepared source roles pass Read.
pub(crate) async fn render(
    ctx: &Ctx,
    conn: &mut Conn,
    principal: &Principal,
    id: Id,
    prepared: &Prepared,
) -> ApiResult<Node> {
    if !crate::key_operations::native::prepared_read(ctx, principal.caller()) {
        return super::shown(ctx, conn, principal, id).await;
    }
    view::one_with_name_sources(
        conn,
        principal,
        id,
        ctx.now,
        Policy::default(),
        &prepared.names,
        prepared.topic.as_ref(),
    )
    .await?
    .ok_or_else(ApiError::not_found)
}
