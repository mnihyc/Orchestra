//! The graph around a document: only readable documents, chats and threads, joined by the references among them.

use super::placed::{kind_of, placed, store_kind};
use super::{Limits, live_document};
use crate::middleware::authorization;
use crate::middleware::requests::{ApiError, ApiResult};
use crate::registry::Ctx;
use crate::routes::nodes::invalid;
use orch_api_contract::links::{
    DocumentGraphGetQuery, GraphEdge, GraphEdgeKind, GraphNode, LinkGraph,
};
use orch_api_contract::search::{RelatedKind, RelatedMeaning};
use orch_db::DbError;
use orch_db::queries::links as rows;
use orch_engine::links::{GraphAsk, Person};
use orch_types::Id;
use std::collections::HashSet;

/// The deepest a graph reaches from its centre: beyond it a graph is a map of everything.
const DEPTH_MAX: u32 = 3;

pub(super) async fn get(ctx: Ctx, query: DocumentGraphGetQuery, (): ()) -> ApiResult<LinkGraph> {
    let mut tx = ctx.app.db.begin_snapshot().await?;
    let mut cancellation = orch_db::queries::search::cancel_on_drop(&ctx.app.db, &mut tx).await?;
    let principal = authorization::principal(&ctx, &mut tx).await?;
    let document = live_document(&mut tx, ctx.id("id")?).await?;
    let limits = Limits::read(&mut tx, document.node_id).await?;
    let depth = query.depth.unwrap_or(1);
    if depth == 0 || depth > DEPTH_MAX {
        return Err(invalid("depth", "is from 1 to 3"));
    }
    let kinds = kinds_of(query.kinds.as_deref())?;
    let center = match query.center.as_deref() {
        None => (RelatedKind::Document, document.id),
        Some(text) => {
            let bad = || invalid("center", "is not a node, as kind:id");
            let (kind, id) = text.split_once(':').ok_or_else(bad)?;
            let kind = match kind {
                "document" => RelatedKind::Document,
                "chat" => RelatedKind::Chat,
                "thread" => RelatedKind::Thread,
                _ => return Err(bad()),
            };
            (kind, id.parse().map_err(|_| bad())?)
        }
    };
    if center.1 != document.id {
        let source = rows::LiveSource {
            kind: store_kind(center.0).to_owned(),
            source_id: center.1,
        };
        let readable = orch_engine::links::readable_by_person(
            &mut tx,
            &principal,
            std::slice::from_ref(&source),
        )
        .await
        .map_err(ApiError::internal)?;
        if !readable.contains(&center.1) {
            return Err(ApiError::not_found());
        }
    }
    let scopes =
        orch_authz::everything::project_search(&mut tx, &principal, None, &[document.id], &[], &[])
            .await
            .map_err(ApiError::internal)?
            .search_candidates(&principal);
    let kind_words: Vec<&str> = kinds.iter().map(|kind| store_kind(*kind)).collect();
    let mut drawn = orch_engine::links::graph(
        &mut tx,
        &Person(&principal),
        GraphAsk {
            center: (store_kind(center.0), center.1),
            scopes: &scopes,
            depth,
            kinds: &kind_words,
            limit: limits.graph_nodes,
            checks: limits.access_checks,
        },
    )
    .await
    .map_err(ApiError::internal)?;
    // What is alike in meaning to a document is read from the document's own related list, which the reader may
    // read in full already; each such item is added as a dashed edge from the centre.
    let mut meaning = RelatedMeaning::Off;
    let mut likeness: Vec<Id> = Vec::new();
    if query.meaning == Some(true) && center.0 == RelatedKind::Document {
        let (near, state) = ctx
            .app
            .engine
            .meaning_neighbours(&mut tx, &principal, center.1, limits.graph_nodes)
            .await
            .map_err(ApiError::internal)?;
        meaning = state;
        let known: HashSet<Id> = drawn.members.iter().map(|member| member.id).collect();
        for (kind, id) in near {
            if !kinds.contains(&kind_of(&kind)) || id == center.1 {
                continue;
            }
            likeness.push(id);
            if known.contains(&id) {
                continue;
            }
            if drawn.members.len() >= limits.graph_nodes {
                drawn.truncated = true;
                continue;
            }
            drawn.members.push(orch_engine::links::GraphMember {
                kind,
                id,
                distance: 1,
            });
        }
    }
    let ids: Vec<Id> = drawn.members.iter().map(|member| member.id).collect();
    let named = placed(&mut tx, &principal, &ids, ctx.now).await?;
    let nodes: Vec<GraphNode> = drawn
        .members
        .iter()
        .filter_map(|member| {
            let place = named.get(&member.id)?;
            Some(GraphNode {
                kind: place.kind,
                id: member.id,
                name: place.name.clone(),
                context: place.context.clone(),
                distance: member.distance,
                current: member.id == document.id,
            })
        })
        .collect();
    let shown: HashSet<Id> = nodes.iter().map(|node| node.id).collect();
    let mut edges: Vec<GraphEdge> = drawn
        .links
        .iter()
        .filter(|link| shown.contains(&link.from_id) && shown.contains(&link.to_id))
        .map(|link| GraphEdge {
            from: link.from_id,
            to: link.to_id,
            kind: GraphEdgeKind::Reference,
        })
        .collect();
    for id in likeness {
        let referenced = edges.iter().any(|edge| {
            (edge.from == center.1 && edge.to == id) || (edge.from == id && edge.to == center.1)
        });
        if shown.contains(&id) && !referenced {
            edges.push(GraphEdge {
                from: center.1,
                to: id,
                kind: GraphEdgeKind::Meaning,
            });
        }
    }
    cancellation.disarm();
    tx.commit().await.map_err(DbError::from)?;
    Ok(LinkGraph {
        center: center.1,
        nodes,
        edges,
        truncated: drawn.truncated,
        meaning,
    })
}

/// The kinds a graph draws: the words of the query, or all three.
fn kinds_of(text: Option<&str>) -> ApiResult<Vec<RelatedKind>> {
    let mut kinds = Vec::new();
    for word in text
        .unwrap_or_default()
        .split(',')
        .map(str::trim)
        .filter(|word| !word.is_empty())
    {
        let kind = match word {
            "document" => RelatedKind::Document,
            "chat" => RelatedKind::Chat,
            "thread" => RelatedKind::Thread,
            _ => return Err(invalid("kinds", "is made of document, chat and thread")),
        };
        if !kinds.contains(&kind) {
            kinds.push(kind);
        }
    }
    if kinds.is_empty() {
        kinds = vec![
            RelatedKind::Document,
            RelatedKind::Chat,
            RelatedKind::Thread,
        ];
    }
    Ok(kinds)
}
