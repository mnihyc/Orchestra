//! Only a current reader and sharer may inspect sharing effects; existing rights use the common holder graph.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]

mod support;

use orch_access_contract::pages::{Holder, PageChange};
use orch_access_contract::permissions::Permission;
use orch_access_contract::roles::Role;
use orch_authz::pages::Target;
use orch_authz::previews::{already_held, may_share, of_page};
use orch_authz::things::Thing;
use orch_authz::usable::ResourcePolicy;
use orch_authz::{Caller, Policy};
use serde_json::json;
use support::World;

#[tokio::test]
async fn a_person_kept_from_sharing_or_from_the_content_gets_no_recipient_preview() {
    let w = World::new().await;
    let viewer = w.member("viewer").await;
    w.give(w.root, viewer, Role::Viewer).await;
    let caller = w.principal(Caller::session(viewer)).await;
    let closed = Policy {
        holders_share: false,
        ..Policy::default()
    };
    let mut conn = w.conn().await;
    assert!(
        !may_share(&mut conn, &caller, Thing::Node(w.topic), closed)
            .await
            .unwrap()
    );
    assert!(
        already_held(
            &mut conn,
            &caller,
            Thing::Node(w.topic),
            closed,
            &[Holder::User { id: w.boss }],
            &[Permission::TopicView]
        )
        .await
        .is_err()
    );
    drop(conn);
    let outside = w.user("outside").await;
    let stranger = w.principal(Caller::session(outside)).await;
    let mut conn = w.conn().await;
    assert!(
        !may_share(
            &mut conn,
            &stranger,
            Thing::Node(w.topic),
            Policy::default()
        )
        .await
        .unwrap()
    );
    drop(conn);
    w.change(w.boss, Target::Live(w.topic), json!({"whole":"private"}))
        .await
        .unwrap();
    let mut conn = w.conn().await;
    assert!(
        !may_share(&mut conn, &caller, Thing::Node(w.topic), Policy::default())
            .await
            .unwrap()
    );
    assert!(
        already_held(
            &mut conn,
            &caller,
            Thing::Node(w.topic),
            Policy::default(),
            &[Holder::User { id: w.boss }],
            &[Permission::TopicView]
        )
        .await
        .is_err()
    );
}

#[tokio::test]
async fn existing_rights_follow_a_real_group_and_a_preview_never_changes_them() {
    let w = World::new().await;
    let reader = w.member("reader").await;
    let receiver = w.member("receiver").await;
    w.give(w.root, reader, Role::Viewer).await;
    let group = w.group(w.root, "Crew", &[reader, receiver]).await;
    let principal = w.principal(Caller::session(w.boss)).await;
    let before = (w.epoch().await, w.audit().await.len());
    let mut conn = w.conn().await;
    let (already, more) = already_held(
        &mut conn,
        &principal,
        Thing::Node(w.topic),
        Policy::default(),
        &[Holder::Group { id: group }],
        &[Permission::TopicView, Permission::TopicInteract],
    )
    .await
    .unwrap();
    drop(conn);
    assert_eq!(more, 0);
    let row = already.iter().find(|person| person.user == reader).unwrap();
    assert!(row.permissions.contains(&Permission::TopicView));
    assert!(!row.permissions.contains(&Permission::TopicInteract));
    assert!(!already.iter().any(|person| person.user == receiver));
    let change:PageChange=serde_json::from_value(json!({"rows":[{"permission":"topic.view","add":[{"holder":{"kind":"group","id":group}}]}]})).unwrap();
    let mut tx = w.db.pool.begin().await.unwrap();
    let effect = of_page(
        &mut tx,
        &principal,
        Thing::Node(w.topic),
        &change,
        &ResourcePolicy {
            owners_name_outsiders: true,
            shared_subscriptions: None,
        },
        &Default::default(),
    )
    .await
    .unwrap();
    assert!(effect.gaining.iter().any(
        |person| person.user == receiver && person.permissions.contains(&Permission::TopicView)
    ));
    assert!(!effect.gaining.iter().any(|person| person.user == reader));
    tx.rollback().await.unwrap();
    assert_eq!((w.epoch().await, w.audit().await.len()), before);
}

#[tokio::test]
async fn everyone_on_a_system_resource_counts_unknown_people_without_listing_their_names() {
    let w = World::new().await;
    let outside = w.user("outside").await;
    let agent = w.system_thing("agent", "Shared agent profile").await;
    w.grant(
        "agent",
        agent,
        Permission::AccessManage,
        Holder::User { id: w.boss },
    )
    .await;
    w.grant("agent", agent, Permission::AgentUse, Holder::Everyone)
        .await;
    let thing = Thing::Other(support::things::object("agent", agent));
    let principal = w.principal(Caller::session(w.boss)).await;
    let mut conn = w.conn().await;
    let (already, more) = already_held(
        &mut conn,
        &principal,
        thing,
        Policy::default(),
        &[Holder::Everyone],
        &[Permission::AgentUse],
    )
    .await
    .unwrap();
    assert!(already.iter().any(|person| person.user == w.boss));
    assert!(!already.iter().any(|person| person.user == outside));
    assert_eq!(
        more, 1,
        "an unnamed outsider is counted, never supplied as a user reference"
    );
}
