//! Batched, visibility-filtered setting reach using the resolver's merge and lock provenance.

use super::scope_of;
use crate::middleware::{
    authorization,
    requests::{ApiError, ApiResult},
};
use crate::registry::Ctx;
use orch_access_contract::permissions::{NodePermission, PermissionName};
use orch_api_contract::common::{Page, PageQuery};
use orch_api_contract::nodes::{NodeKind, NodeRef};
use orch_api_contract::settings::{SettingReach, SettingReachCounts};
use orch_authz::{Act, Policy, nodes};
use orch_db::queries::{hierarchy::NodeRow, settings};
use orch_settings::merge::{Link, Rows, Stack, resolve_key};
use orch_settings_contract::provenance::Resolved;
use orch_settings_contract::registry::{self, Scope, SettingDef};
use orch_types::Id;
use std::collections::{HashMap, HashSet};

/// The same node-view permission as the settings list; callers never need permission to change a setting to inspect it.
pub(super) async fn at_node(ctx: Ctx, query: PageQuery, (): ()) -> ApiResult<SettingReach> {
    let id = ctx.id("id")?;
    read(&ctx, Some(id), &query).await
}

/// Shared by the ordinary place read and the separately gated system-settings read.
pub(crate) async fn read(
    ctx: &Ctx,
    place: Option<Id>,
    query: &PageQuery,
) -> ApiResult<SettingReach> {
    let key = ctx.param("key")?;
    let def = registry::get(key).ok_or_else(ApiError::not_found)?;
    let after = query
        .cursor
        .as_deref()
        .map(|cursor| {
            cursor
                .parse::<Id>()
                .map_err(|_| ApiError::invalid("cursor", "the settings reach cursor is not valid"))
        })
        .transpose()?;
    let mut conn = ctx.app.db.acquire().await?;
    let principal = authorization::principal(ctx, &mut conn).await?;
    let all = settings::reach_nodes(&mut conn, place).await?;
    let by_id: HashMap<_, _> = all.iter().map(|node| (node.id, node)).collect();
    let here = match place {
        Some(id) => chain(
            by_id.get(&id).copied().ok_or_else(ApiError::not_found)?,
            &by_id,
        )?,
        None => Vec::new(),
    };
    let scope = here.last().map_or(Scope::System, |link| link.scope);
    if !orch_settings::validation::listed(def, scope) {
        return Err(ApiError::not_found());
    }
    let live = |node: &&NodeRow| {
        node.id != place.unwrap_or_else(Id::nil)
            && node.path.iter().all(|id| {
                by_id
                    .get(id)
                    .is_some_and(|parent| parent.deleted_at.is_none())
            })
            && place.is_none_or(|id| node.path.contains(&id))
    };
    let candidates: Vec<_> = all.iter().filter(live).collect();
    let ids: Vec<_> = candidates.iter().map(|node| node.id).collect();
    // One authorization frame for the whole set; private descendants contribute neither names nor counts.
    let visible = nodes::holding(
        &mut conn,
        &principal,
        &ids,
        PermissionName::OfNode(NodePermission::View),
        Act::Use,
        Policy::default(),
    )
    .await
    .map_err(ApiError::internal)?;
    let visible: HashSet<_> = visible.into_iter().map(|node| node.id).collect();
    let all_ids: Vec<_> = all.iter().map(|node| node.id).collect();
    let rows =
        Rows::new(settings::rows_on_chain(&mut conn, &all_ids, Some(&[key.to_owned()])).await?);
    let baseline = resolved(def, &rows, &here);
    let mut applies = SettingReachCounts::default();
    let mut overrides = Vec::new();
    for node in candidates
        .into_iter()
        .filter(|node| visible.contains(&node.id))
    {
        let chain = chain(node, &by_id)?;
        let value = resolved(def, &rows, &chain);
        if contributes(&baseline, &value) {
            match chain.last().map(|link| link.scope) {
                Some(Scope::Org) => applies.orgs += 1,
                Some(Scope::Project) => applies.projects += 1,
                Some(Scope::Topic) => applies.topics += 1,
                Some(Scope::Thread) => applies.threads += 1,
                _ => {}
            }
        } else if value
            .parts
            .iter()
            .any(|part| part.source.node == Some(node.id))
        {
            overrides.push(NodeRef {
                thread_display: None,
                id: node.id,
                kind: authorization::refusals::node_kind(&node.kind)
                    .ok_or_else(|| ApiError::internal("a setting place has no node kind"))?,
                name: Some(node.name.clone()),
                redacted: false,
            });
        }
    }
    if let Some(after) = after {
        overrides.retain(|node| node.id > after);
    }
    let limit = query.limit() as usize;
    let more = overrides.len() > limit;
    overrides.truncate(limit);
    let next_cursor = if more {
        overrides.last().map(|node| node.id.to_string())
    } else {
        None
    };
    let ids: Vec<_> = overrides.iter().map(|node| node.id).collect();
    let names =
        crate::routes::nodes::view::qualified_refs(&mut conn, &principal, &ids, Policy::default())
            .await?;
    for node in &mut overrides {
        if let Some(qualified) = names.get(&node.id) {
            *node = qualified.clone();
        }
    }
    Ok(SettingReach {
        applies,
        overrides: Page {
            items: overrides,
            next_cursor,
        },
    })
}

fn chain(node: &NodeRow, nodes: &HashMap<Id, &NodeRow>) -> ApiResult<Vec<Link>> {
    node.path
        .iter()
        .map(|id| {
            let row = nodes.get(id).ok_or_else(ApiError::not_found)?;
            if row.deleted_at.is_some() {
                return Err(ApiError::not_found());
            }
            let kind: NodeKind = authorization::refusals::node_kind(&row.kind)
                .ok_or_else(|| ApiError::internal("a setting place has no node kind"))?;
            Ok(Link {
                id: *id,
                scope: scope_of(kind),
            })
        })
        .collect()
}

fn resolved(def: &SettingDef, rows: &Rows, chain: &[Link]) -> Resolved<Id> {
    // Supplied defaults have registry provenance; there is no per-child machine/provider read.
    resolve_key(
        def,
        &Stack {
            rows,
            chain,
            own: None,
        },
        None,
    )
}

fn contributes(baseline: &Resolved<Id>, current: &Resolved<Id>) -> bool {
    if baseline.parts.is_empty() {
        return current.parts.is_empty();
    }
    baseline
        .parts
        .iter()
        .any(|part| current.parts.iter().any(|now| now.source == part.source))
}
