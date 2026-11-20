//! Governing navigation preferences and safe relative orders of already admitted identities.
use crate::middleware::requests::{ApiError, ApiResult};
use orch_api_contract::common::Listed;
use orch_api_contract::nodes::{
    HierarchyLocale, HierarchyOrderBy, HierarchyOrdering, HierarchySort, OrgTreeGetQuery, TreeNode,
};
use orch_db::{
    Conn,
    queries::{hierarchy, identity},
};
use orch_types::{Hash, Id, Json};
use std::collections::{BTreeMap, BTreeSet};

pub(super) struct Resolved {
    pub native: hierarchy::order::Order,
    pub by: Option<HierarchyOrderBy>,
    pub revision: Hash,
}
fn by(value: &Json) -> Option<HierarchyOrderBy> {
    serde_json::from_value(value.get("by")?.clone()).ok()
}

pub(super) async fn resolve(
    conn: &mut Conn,
    user: Id,
    root: Id,
    query: &OrgTreeGetQuery,
) -> ApiResult<Resolved> {
    let locale = match query.locale.unwrap_or_default() {
        HierarchyLocale::En => hierarchy::order::Locale::En,
        HierarchyLocale::ZhCn => hierarchy::order::Locale::ZhCn,
    };
    let Some(sort) = query.sort else {
        return Ok(Resolved {
            native: hierarchy::order::Order::default(),
            by: None,
            revision: Hash::of(b"native identity order"),
        });
    };
    let sidebar = identity::interface_value(conn, user, "sidebar")
        .await?
        .unwrap_or(Json::Null);
    let root_key = root.to_string();
    let own = sidebar.get("order").and_then(|rows| rows.get(&root_key));
    let default = orch_settings::get::<Json>(
        conn,
        orch_settings::Place::Node(root),
        orch_settings_contract::registry::keys::NAVIGATION_ORDER,
    )
    .await
    .map_err(ApiError::internal)?
    .unwrap_or(serde_json::json!({"by":"changed"}));
    let chosen = own.filter(|value| by(value).is_some()).unwrap_or(&default);
    let by = match sort {
        HierarchySort::Navigation => by(chosen).unwrap_or(HierarchyOrderBy::Changed),
        HierarchySort::Newest => HierarchyOrderBy::Newest,
        HierarchySort::Changed => HierarchyOrderBy::Changed,
        HierarchySort::Name => HierarchyOrderBy::Name,
        HierarchySort::Custom => HierarchyOrderBy::Custom,
    };
    let mut custom = BTreeMap::<Id, Vec<Id>>::new();
    if by == HierarchyOrderBy::Custom
        && let Some(parents) = chosen.get("custom").and_then(Json::as_object)
    {
        for (parent, ids) in parents {
            if let Ok(parent) = parent.parse::<Id>()
                && let Some(ids) = ids.as_array()
            {
                let mut seen = BTreeSet::new();
                custom.insert(
                    parent,
                    ids.iter()
                        .filter_map(Json::as_str)
                        .filter_map(|id| id.parse::<Id>().ok())
                        .filter(|id| seen.insert(*id))
                        .collect(),
                );
            }
        }
    }
    let custom = serde_json::to_value(custom).map_err(ApiError::internal)?;
    let revision = Hash::of(
        &serde_json::to_vec(&(by, locale.collation(), &custom)).map_err(ApiError::internal)?,
    );
    let thread_activity = match query.parent_id {
        None => true,
        Some(parent) => hierarchy::node(conn, parent)
            .await?
            .is_some_and(|node| node.kind == "topic"),
    };
    Ok(Resolved {
        native: hierarchy::order::Order {
            by: match by {
                HierarchyOrderBy::Newest => hierarchy::order::By::Newest,
                HierarchyOrderBy::Changed => hierarchy::order::By::Changed,
                HierarchyOrderBy::Name => hierarchy::order::By::Name,
                HierarchyOrderBy::Custom => hierarchy::order::By::Custom,
            },
            locale,
            custom,
            thread_activity,
        },
        by: Some(by),
        revision,
    })
}
impl Resolved {
    pub fn metadata(
        &self,
        nodes: &[Listed<TreeNode>],
        ancestors: &[Listed<TreeNode>],
    ) -> Option<HierarchyOrdering> {
        let by = self.by?;
        let mut known_order = BTreeMap::<Id, Vec<Id>>::new();
        if by == HierarchyOrderBy::Custom {
            for row in nodes.iter().chain(ancestors) {
                let (id, parent) = match row {
                    Listed::Visible(node) => (node.content.id, node.content.parent_id),
                    Listed::Hidden(node) => (node.id, node.parent_id),
                };
                if let Some(parent) = parent {
                    known_order.entry(parent).or_default().push(id);
                }
            }
            for (parent, ids) in &mut known_order {
                ids.sort_by(|one, other| {
                    self.native
                        .rank(Some(*parent), *one)
                        .cmp(&self.native.rank(Some(*parent), *other))
                        .then_with(|| other.cmp(one))
                });
                ids.dedup();
            }
        }
        Some(HierarchyOrdering {
            by,
            revision: self.revision,
            known_order,
        })
    }
}
