//! What a change of a page or of the defaults of a place would do, said before it is made, on a real database: nothing is left behind, the pages it reaches are those that follow, and it is refused as the change is.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]

mod support;

use orch_access_contract::pages::PageChange;
use orch_access_contract::permissions::Permission;
use orch_access_contract::roles::{PlaceRef, Role};
use orch_authz::pages::{ChangeError, Target};
use orch_authz::previews::{Preview, of_defaults, of_page};
use orch_authz::things::Thing;
use orch_authz::usable::ResourcePolicy;
use orch_authz::{Caller, Policy};
use orch_db::audit::Origin;
use orch_types::Id;
use serde_json::{Value, json};
use support::{Statements, World, role_holder, user_holder};

fn policy() -> ResourcePolicy {
    ResourcePolicy {
        owners_name_outsiders: true,
        shared_subscriptions: None,
    }
}

async fn preview_page(w: &World, who: Id, node: Id, change: Value) -> Result<Preview, ChangeError> {
    let principal = w.principal(Caller::session(who)).await;
    let change: PageChange = serde_json::from_value(change).unwrap();
    let mut tx = w.db.pool.begin().await.unwrap();
    of_page(
        &mut tx,
        &principal,
        Thing::Node(node),
        &change,
        &policy(),
        &Origin::default(),
    )
    .await
}

async fn preview_defaults(
    w: &World,
    who: Id,
    place: Id,
    change: Value,
) -> Result<Preview, ChangeError> {
    let principal = w.principal(Caller::session(who)).await;
    let change: PageChange = serde_json::from_value(change).unwrap();
    let mut tx = w.db.pool.begin().await.unwrap();
    of_defaults(
        &mut tx,
        &principal,
        PlaceRef::Node(place),
        &change,
        Policy::default(),
        &Origin::default(),
    )
    .await
}

fn adding(permission: &str, holder: Value) -> Value {
    json!({ "rows": [{ "permission": permission, "add": [{ "holder": holder }] }] })
}

#[tokio::test]
async fn a_preview_leaves_nothing_behind_whatever_the_change_is() {
    let w = World::new().await;
    let reader = w.member("rae").await;
    let other = w.member("ben").await;
    w.give(w.root, reader, Role::Viewer).await;
    let changes = [
        adding("topic.interact", user_holder(reader)),
        json!({ "name": [{ "holders": [user_holder(other)], "choice": "full_manage" }] }),
        json!({ "rows": [{ "permission": "topic.view", "inherit": false }] }),
        json!({ "whole": "private" }),
        json!({ "whole": "reset" }),
        json!({ "owners": { "add": [user_holder(other)], "first": other } }),
    ];
    let before = (
        w.epoch().await,
        w.audit().await.len(),
        w.read(w.boss, Target::Live(w.topic)).await,
    );
    for change in &changes {
        preview_page(&w, w.boss, w.topic, change.clone())
            .await
            .unwrap();
        let after = (
            w.epoch().await,
            w.audit().await.len(),
            w.read(w.boss, Target::Live(w.topic)).await,
        );
        assert_eq!(after, before, "{change}");
    }
    // The same changes, made, do move things: the loop above was not looking at nothing.
    w.change(w.boss, Target::Live(w.topic), changes[1].clone())
        .await
        .unwrap();
    assert!(w.epoch().await > before.0);
}

#[tokio::test]
async fn a_preview_of_a_row_cut_loose_names_those_who_hold_it_only_through_the_defaults() {
    let w = World::new().await;
    let viewer = w.member("rae").await;
    let named = w.member("ben").await;
    w.give(w.root, viewer, Role::Viewer).await;
    w.name(
        w.topic,
        Permission::TopicView,
        orch_access_contract::pages::Holder::User { id: named },
    )
    .await;
    let preview = preview_page(
        &w,
        w.boss,
        w.topic,
        json!({ "rows": [{ "permission": "topic.view", "inherit": false }] }),
    )
    .await
    .unwrap();
    let losing: Vec<Id> = preview.losing.iter().map(|moved| moved.user).collect();
    assert!(
        losing.contains(&viewer),
        "a viewer holds it through the defaults"
    );
    assert!(!losing.contains(&named), "named in the row, they keep it");
    assert!(!losing.contains(&w.boss), "its owner keeps it");
    assert!(preview.gaining.is_empty());
    assert_eq!(preview.pages, 1);
}

#[tokio::test]
async fn a_preview_of_defaults_reaches_the_pages_that_follow_the_row_and_no_other() {
    let w = World::new().await;
    let reader = w.member("rae").await;
    w.give(w.root, reader, Role::Viewer).await;
    let following_one = w.node("topic", w.project, "One").await;
    let cut_loose = w.node("topic", w.project, "Two").await;
    let following_two = w.node("topic", w.project, "Three").await;
    w.row(cut_loose, Permission::TopicManage, false).await;
    let change = adding("topic.manage", role_holder(Role::Viewer));
    let preview = preview_defaults(&w, w.boss, w.project, change.clone())
        .await
        .unwrap();
    // The topic that already existed in the world follows as well.
    let following = [w.topic, following_one, following_two];
    assert_eq!(
        preview.pages,
        i64::try_from(following.len()).unwrap(),
        "{preview:?}"
    );
    let moved = preview
        .gaining
        .iter()
        .find(|moved| moved.user == reader)
        .expect("a viewer would gain it");
    assert_eq!(moved.pages, 3);
    assert!(moved.permissions.contains(&Permission::TopicManage));
    assert!(
        !moved.permissions.contains(&Permission::TopicView),
        "she views it already: {moved:?}"
    );
    // Made for real, it is what was said.
    w.change_defaults(w.boss, w.project, change).await.unwrap();
    for topic in following {
        assert!(w.can(reader, topic, Permission::TopicManage).await);
    }
    assert!(!w.can(reader, cut_loose, Permission::TopicManage).await);
}

#[tokio::test]
async fn a_preview_is_refused_as_the_change_is() {
    let w = World::new().await;
    let viewer = w.member("rae").await;
    w.give(w.root, viewer, Role::Viewer).await;
    let by_viewer = preview_page(
        &w,
        viewer,
        w.topic,
        adding("topic.view", user_holder(viewer)),
    )
    .await;
    assert!(
        matches!(by_viewer, Err(ChangeError::Denied(_))),
        "{by_viewer:?}"
    );
    let stale = preview_page(
        &w,
        w.boss,
        w.topic,
        json!({ "expected_revision": 99, "whole": "private" }),
    )
    .await;
    assert!(matches!(stale, Err(ChangeError::Stale { .. })), "{stale:?}");
    // A lock above decides the row.
    w.change_defaults(
        w.boss,
        w.root,
        json!({ "rows": [{ "permission": "topic.interact", "locked": true }] }),
    )
    .await
    .unwrap();
    let locked = preview_page(
        &w,
        w.boss,
        w.topic,
        adding("topic.interact", user_holder(viewer)),
    )
    .await;
    assert!(
        matches!(locked, Err(ChangeError::Locked { .. })),
        "{locked:?}"
    );
    let unknown = preview_page(
        &w,
        w.boss,
        w.topic,
        json!({ "owners": { "remove": [Id::nil()] } }),
    )
    .await;
    assert!(
        matches!(unknown, Err(ChangeError::Invalid(_))),
        "{unknown:?}"
    );
}

#[tokio::test]
async fn the_statements_of_a_defaults_preview_do_not_grow_with_the_number_of_pages() {
    let w = World::new().await;
    let reader = w.member("rae").await;
    w.give(w.root, reader, Role::Viewer).await;
    let change = adding("topic.manage", role_holder(Role::Viewer));
    let (few_statements, few_pages) = counted(&w, &change).await;
    for at in 0..40 {
        w.node("topic", w.project, &format!("Topic {at}")).await;
    }
    let (many_statements, many_pages) = counted(&w, &change).await;
    assert!(many_pages > few_pages + 30);
    assert!(
        many_statements <= few_statements + few_statements / 4 + 4,
        "{few_statements} statements for {few_pages} pages, {many_statements} for {many_pages}"
    );
}

/// The statements a preview of the defaults of the project takes, and the pages it reaches.
async fn counted(w: &World, change: &Value) -> (usize, i64) {
    let before = Statements::now();
    let preview = preview_defaults(w, w.boss, w.project, change.clone())
        .await
        .unwrap();
    (Statements::now() - before, preview.pages)
}
