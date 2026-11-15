//! Query-owned serving facts, projected on intent with current content scope and safe names.

use super::super::meaning::{self, Position};
use crate::middleware::authorization::refusals::user_refs;
use crate::middleware::requests::{ApiError, ApiResult};
use crate::registry::Ctx;
use crate::routes::nodes::view;
use orch_api_contract::search::{
    SearchListQuery, SearchMeaningModel, SearchMeaningSource, SearchMeaningSourcePage,
};
use orch_authz::{Policy, Principal};
use orch_db::Conn;
use orch_engine::semantic::query::ServingReceipt;
use orch_types::Id;
use std::collections::{BTreeMap, HashSet};

pub(super) fn model(receipt: &ServingReceipt) -> SearchMeaningModel {
    SearchMeaningModel {
        provider_id: receipt.provider_id,
        model_name: receipt.model_name.clone(),
        display_name: receipt.model_display.clone(),
    }
}

pub(super) fn unreported(positions: &BTreeMap<Id, Position>) -> bool {
    positions
        .values()
        .any(|position| position.generation.is_some() && position.receipt.is_none())
}

#[allow(
    clippy::too_many_arguments,
    reason = "one captured query, its current native scope and explicit page selection travel together"
)]
pub(super) async fn page(
    ctx: &Ctx,
    conn: &mut Conn,
    principal: &Principal,
    query: &SearchListQuery,
    native_scopes: &orch_types::Json,
    exhausted: bool,
    positions: &BTreeMap<Id, Position>,
    offset: u32,
    limit: u32,
    filter: &str,
) -> ApiResult<SearchMeaningSourcePage> {
    if exhausted {
        return Ok(SearchMeaningSourcePage {
            sources: Vec::new(),
            next_offset: None,
            total: 0,
            unreported: false,
        });
    }
    let roots: Vec<_> = positions
        .iter()
        .filter_map(|(root, position)| position.receipt.as_ref().map(|_| *root))
        .collect();
    let eligible = meaning::prepare_scope(ctx, conn, principal, query, roots, Some(native_scopes))
        .await?
        .orgs
        .into_iter()
        .collect::<HashSet<_>>();
    let eligible = if native_scopes
        .get("unrestricted")
        .and_then(orch_types::Json::as_bool)
        == Some(true)
    {
        eligible
    } else {
        let ids = |keys: &[&str]| {
            keys.iter()
                .flat_map(|key| {
                    native_scopes
                        .get(*key)
                        .and_then(orch_types::Json::as_array)
                        .into_iter()
                        .flatten()
                })
                .filter_map(orch_types::Json::as_str)
                .filter_map(|id| id.parse::<Id>().ok())
                .collect::<Vec<_>>()
        };
        orch_db::queries::semantic::source_roots(
            conn,
            &eligible.into_iter().collect::<Vec<_>>(),
            &ids(&[
                "scopes",
                "doc_scopes",
                "thread_scopes",
                "chat_scopes",
                "chat_nodes",
                "names",
            ]),
            &ids(&["documents", "chats"]),
        )
        .await?
        .into_iter()
        .collect()
    };
    let roots: Vec<_> = positions
        .keys()
        .filter(|root| eligible.contains(*root))
        .copied()
        .collect();
    // Captured model labels are admitted query facts. Live external names must not broaden an operation-scoped key.
    let bounded = principal
        .caller()
        .grant()
        .is_some_and(|grant| grant.operation_scope().is_some());
    let refs = if bounded {
        std::collections::HashMap::new()
    } else {
        view::refs(conn, principal, &roots, Policy::default()).await?
    };
    let users: Vec<_> = if bounded {
        Vec::new()
    } else {
        positions
            .iter()
            .filter(|(root, _)| eligible.contains(*root))
            .filter_map(|(_, position)| position.receipt.as_ref()?.usage_owner)
            .collect()
    };
    let people = user_refs(conn, &users).await?;
    let needle = filter.to_lowercase();
    let mut sources = Vec::new();
    for (ordinal, (root, position)) in positions.iter().enumerate() {
        if !eligible.contains(root) {
            continue;
        }
        let Some(receipt) = &position.receipt else {
            continue;
        };
        let root = if bounded {
            orch_api_contract::nodes::NodeRef {
                id: *root,
                kind: orch_api_contract::nodes::NodeKind::Org,
                name: None,
                redacted: true,
                thread_display: None,
            }
        } else {
            let Some(root) = refs.get(root).cloned() else {
                continue;
            };
            root
        };
        if !needle.is_empty()
            && !root
                .name
                .as_deref()
                .unwrap_or_default()
                .to_lowercase()
                .contains(&needle)
            && !receipt.model_name.to_lowercase().contains(&needle)
            && !receipt.model_display.to_lowercase().contains(&needle)
        {
            continue;
        }
        let usage_owner = receipt
            .usage_owner
            .and_then(|writer| people.get(&writer).cloned());
        sources.push((
            ordinal,
            SearchMeaningSource {
                root,
                model: model(receipt),
                usage_owner_withheld: receipt.usage_owner.is_some() && usage_owner.is_none(),
                usage_owner,
                cached: receipt.cached,
                observed_at: receipt.observed_at,
            },
        ));
    }
    let total = u32::try_from(sources.len()).map_err(ApiError::internal)?;
    let start = usize::try_from(offset).unwrap_or(usize::MAX);
    let take = usize::try_from(limit.max(1).min(orch_api_contract::common::MAX_LIMIT))
        .unwrap_or(usize::MAX);
    let mut after = sources.into_iter().filter(|(ordinal, _)| *ordinal >= start);
    let shown: Vec<_> = after.by_ref().take(take).collect();
    let next_offset = after.next().and_then(|_| {
        shown
            .last()
            .map(|(ordinal, _)| u32::try_from(ordinal.saturating_add(1)).unwrap_or(u32::MAX))
    });
    Ok(SearchMeaningSourcePage {
        sources: shown.into_iter().map(|(_, source)| source).collect(),
        next_offset,
        total,
        unreported: unreported(positions),
    })
}
