//! A bounded overview shares native windows and never lends ordinary authority or another operation.
#![allow(clippy::unwrap_used, clippy::expect_used, reason = "test assertions")]

mod places;
mod support;

use orch_access_contract::key_scope::KeyScope;
use orch_api_contract::common::ErrorCode;
use orch_api_contract::nodes::{
    NodeChildList, NodeChildListQuery, OrgOverview, OrgOverviewGet, OrgOverviewGetQuery,
    OrgOverviewResponse, OverviewPart,
};
use orch_api_contract::routes::Endpoint;
use orch_api_core::middleware::authentication::{API_TOKEN_PREFIX, new_token, token_hash};
use orch_db::queries::identity::{self, NewApiToken};
use orch_types::{Id, new_id};
use serde_json::json;
use support::World;

#[test]
fn observed_delivery_keeps_the_native_notification_read_family() {
    let native = orch_api_contract::operations::for_route(
        &orch_api_contract::notifications::NotificationSummaryGet::ROUTE,
    )
    .unwrap();
    let overview = orch_api_contract::operations::for_route(&OrgOverviewGet::ROUTE).unwrap();
    let delivery = overview.variants[0]
        .facets
        .iter()
        .find(|facet| {
            matches!(
                facet.target,
                orch_api_contract::operations::TargetResolver::ResolvedCollection(
                    "overview_delivery"
                )
            )
        })
        .unwrap();
    assert_eq!(delivery.action, native.variants[0].facets[0].action);
    assert_eq!(delivery.resources, native.variants[0].facets[0].resources);
}

trait FullOverview {
    fn overview(self) -> OrgOverview;
}
impl FullOverview for OrgOverviewResponse {
    fn overview(self) -> OrgOverview {
        match self {
            Self::Overview { data } => *data,
            Self::Places { .. } => panic!("the native full overview was requested"),
        }
    }
}

async fn key(world: &World, owner: Id, root: Id, child: Option<Id>) -> String {
    let rule = |kind: &str, id: Id| json!({"target":{"kind":kind,"id":id,"reach":"self"},"resources":["org","project"],"allow":[{"operation":"OrgOverviewGet","actions":["read"]}]});
    let mut rules = vec![rule("org", root)];
    if let Some(child) = child {
        rules.push(rule("project", child));
    }
    let scope: KeyScope = serde_json::from_value(json!({"rules":rules})).unwrap();
    let secret = new_token(API_TOKEN_PREFIX);
    identity::create_api_token_scoped(
        &mut *world.conn().await,
        &NewApiToken {
            id: new_id(),
            user_id: owner,
            name: "Overview reader",
            token_hash: &token_hash(&secret),
            scopes: &[],
            node_id: None,
            at: world.now(),
            expires_at: None,
        },
        Some(&serde_json::to_value(scope).unwrap()),
    )
    .await
    .unwrap();
    secret
}

#[tokio::test]
async fn overview_keeps_the_native_first_page_and_all_applicable_sections() {
    let world = World::new().await;
    let owner = places::person(&world, "overview-owner").await;
    let org = places::make_org(&world, &owner, "Overview", "overview").await;
    // One more than the existing native first page proves this is neither an
    // unbounded inventory nor a smaller page invented for the opening.
    for at in 0..51 {
        places::make_project(&world, &owner, org.base.id, &format!("Project {at}")).await;
    }
    let opened = world
        .app
        .call(OrgOverviewGet, &[&org.base.id])
        .token(&owner.token)
        .query(&OrgOverviewGetQuery::default())
        .send()
        .await
        .ok()
        .overview();
    let native = world
        .app
        .call(NodeChildList, &[&org.base.id])
        .token(&owner.token)
        .query(&NodeChildListQuery::default())
        .send()
        .await
        .ok();
    assert_eq!(opened.children, native);
    assert_eq!(opened.children.items.len(), 50);
    assert!(opened.children.next_cursor.is_some());
    assert!(matches!(opened.first_steps, OverviewPart::Ready { .. }));
    assert!(matches!(opened.usage, OverviewPart::Ready { .. }));
    assert!(matches!(opened.activity, OverviewPart::Ready { .. }));
    assert!(matches!(
        opened.outside_openings,
        OverviewPart::Ready { .. }
    ));
    assert!(opened.recent.items.len() <= 50);
}

#[tokio::test]
async fn scoped_overview_selects_before_the_window_without_borrowing_account_manage_or_other_operations()
 {
    let world = World::new().await;
    let owner = places::person(&world, "overview-owner").await;
    let org = places::make_org(&world, &owner, "Overview", "overview").await;
    let selected = places::make_project(&world, &owner, org.base.id, "Selected").await;
    places::make_project(&world, &owner, org.base.id, "Outside the credential").await;
    let secret = key(&world, owner.id(), org.base.id, Some(selected.base.id)).await;
    let opened = world
        .app
        .call(OrgOverviewGet, &[&org.base.id])
        .token(&secret)
        .query(&OrgOverviewGetQuery::default())
        .send()
        .await
        .ok()
        .overview();
    assert_eq!(opened.children.items.len(), 1);
    let serialized = serde_json::to_string(&opened).unwrap();
    assert!(serialized.contains("Selected"));
    assert!(!serialized.contains("Outside the credential"));
    assert!(matches!(opened.first_steps, OverviewPart::Omitted));
    assert!(matches!(opened.outside_openings, OverviewPart::Omitted));
    let other_operation = world
        .app
        .call(NodeChildList, &[&org.base.id])
        .token(&secret)
        .query(&NodeChildListQuery::default())
        .send()
        .await
        .problem();
    assert_eq!(other_operation.code, ErrorCode::AuthzDenied);
    let empty_key = key(&world, owner.id(), org.base.id, None).await;
    let empty = world
        .app
        .call(OrgOverviewGet, &[&org.base.id])
        .token(&empty_key)
        .query(&OrgOverviewGetQuery::default())
        .send()
        .await
        .ok()
        .overview();
    assert!(empty.children.items.is_empty());
    assert!(empty.children.next_cursor.is_none());
    assert!(matches!(empty.usage, OverviewPart::Omitted));
    let stranger = places::person(&world, "overview-stranger").await;
    let stranger_key = key(&world, stranger.id(), org.base.id, Some(selected.base.id)).await;
    let refused = world
        .app
        .call(OrgOverviewGet, &[&org.base.id])
        .token(&stranger_key)
        .query(&OrgOverviewGetQuery::default())
        .send()
        .await
        .problem();
    assert_eq!(refused.code, ErrorCode::AuthzDenied);
}

#[tokio::test]
async fn place_resolution_uses_the_same_current_scoped_native_node_window() {
    let world = World::new().await;
    let owner = places::person(&world, "overview-owner").await;
    let org = places::make_org(&world, &owner, "Overview", "overview").await;
    let selected = places::make_project(&world, &owner, org.base.id, "Selected").await;
    let other = places::make_project(&world, &owner, org.base.id, "Outside the credential").await;
    let secret = key(&world, owner.id(), org.base.id, Some(selected.base.id)).await;
    let answer = world
        .app
        .call(OrgOverviewGet, &[&org.base.id])
        .token(&secret)
        .query(&OrgOverviewGetQuery {
            resolve_ids: Some(format!(
                "{},{},{},{}",
                selected.base.id,
                other.base.id,
                new_id(),
                selected.base.id
            )),
            ..Default::default()
        })
        .send()
        .await
        .ok();
    let OrgOverviewResponse::Places { nodes } = answer else {
        panic!("place resolution must not load overview sections")
    };
    assert_eq!(nodes.len(), 1);
    assert_eq!(
        serde_json::to_value(&nodes[0]).unwrap()["id"],
        selected.base.id.to_string()
    );
    let oversized = std::iter::repeat_n(selected.base.id.to_string(), 51)
        .collect::<Vec<_>>()
        .join(",");
    let denied = world
        .app
        .call(OrgOverviewGet, &[&org.base.id])
        .token(&secret)
        .query(&OrgOverviewGetQuery {
            resolve_ids: Some(oversized),
            ..Default::default()
        })
        .send()
        .await
        .problem();
    assert_eq!(denied.code, ErrorCode::RequestInvalid);
}
