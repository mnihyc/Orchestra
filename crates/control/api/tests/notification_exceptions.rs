//! Current personal exceptions, their true inherited level, one node mutation and privacy.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]
mod places;
mod support;
use orch_api_contract::common::{ErrorCode, PageQuery, SubscriptionLevel as Level};
use orch_api_contract::nodes::{Node, NodeSubscriptionSet, NodeSubscriptionSetBody};
use orch_api_contract::notifications::{
    MeNotificationExceptionList, MeNotificationExceptionSet, NotificationExceptionSetBody,
};
use orch_types::Id;
use places::{Who, make_org, make_project, make_topic, person};
use support::World;

async fn set(world: &World, who: &Who, node: Id, level: Level) {
    world
        .app
        .call(NodeSubscriptionSet, &[&node])
        .token(&who.token)
        .body(&NodeSubscriptionSetBody { level })
        .send()
        .await
        .ok();
}

#[tokio::test]
async fn exceptions_name_only_the_callers_different_manual_levels_and_use_the_parent_without_the_own_row()
 {
    let world = World::new().await;
    let ada = person(&world, "ada").await;
    let bob = person(&world, "bob").await;
    let root = make_org(&world, &ada, "Acme", "acme").await;
    let project = make_project(&world, &ada, root.base.id, "Work").await;
    let topic = make_topic(&world, &ada, project.base.id, "Login").await;
    let same = make_project(&world, &ada, root.base.id, "Uses the default").await;
    let automatic = make_project(&world, &ada, root.base.id, "Automatic participation").await;
    set(&world, &ada, root.base.id, Level::Important).await;
    set(&world, &ada, project.base.id, Level::All).await;
    set(&world, &ada, topic.base.id, Level::Off).await;
    set(&world, &ada, same.base.id, Level::Important).await;
    let private = make_org(&world, &bob, "Private name never disclosed", "private-bob").await;
    let mut conn = world.conn().await;
    // A historical row may remain after View is lost. It must disclose neither node nor name.
    orch_db::queries::hierarchy::put_subscription(
        &mut conn,
        ada.id(),
        private.base.id,
        "all",
        "manual",
        world.now(),
    )
    .await
    .unwrap();
    orch_db::queries::hierarchy::put_subscription(
        &mut conn,
        ada.id(),
        automatic.base.id,
        "all",
        "auto",
        world.now(),
    )
    .await
    .unwrap();
    drop(conn);
    let page = world
        .app
        .call(MeNotificationExceptionList, &[])
        .token(&ada.token)
        .query(&PageQuery::default())
        .send()
        .await
        .ok();
    assert_eq!(page.items.len(), 3);
    let project_row = page
        .items
        .iter()
        .find(|row| row.node.id == project.base.id)
        .unwrap();
    assert_eq!(project_row.level, Level::All);
    assert_eq!(project_row.default_level, Level::Important);
    assert_eq!(
        project_row.default_from.as_ref().map(|node| node.id),
        Some(root.base.id)
    );
    let topic_row = page
        .items
        .iter()
        .find(|row| row.node.id == topic.base.id)
        .unwrap();
    assert_eq!(
        topic_row.default_level,
        Level::All,
        "the default excludes this topic's own Off row"
    );
    assert_eq!(topic_row.node.name.as_deref(), Some("Login"));
    let read = places::read(&world, &ada, topic.base.id).await;
    let Node::Topic(read) = read else {
        panic!("expected the real topic reader");
    };
    assert_eq!(read.base.access.subscription, Some(Level::Off));
    assert_eq!(read.base.access.effective_subscription, Level::Off);
    assert_eq!(
        read.base.access.inherited_subscription,
        Some(Level::All),
        "the bell's Default is supplied strictly above its own Off row"
    );
    let text = serde_json::to_string(&page).unwrap();
    assert!(!text.contains("Private name never disclosed"));
    assert!(!text.contains(&private.base.id.to_string()));
    let others = world
        .app
        .call(MeNotificationExceptionList, &[])
        .token(&bob.token)
        .query(&PageQuery::default())
        .send()
        .await
        .ok();
    assert!(
        others.items.is_empty(),
        "another person's choices never appear"
    );
}

#[tokio::test]
async fn a_change_returns_the_recomputed_page_and_reset_is_the_same_subscription_the_bell_uses() {
    let world = World::new().await;
    let ada = person(&world, "ada").await;
    let root = make_org(&world, &ada, "Acme", "acme").await;
    let project = make_project(&world, &ada, root.base.id, "Work").await;
    set(&world, &ada, root.base.id, Level::Important).await;
    set(&world, &ada, project.base.id, Level::All).await;
    let changed = world
        .app
        .call(MeNotificationExceptionSet, &[&root.base.id])
        .token(&ada.token)
        .query(&PageQuery::default())
        .body(&NotificationExceptionSetBody {
            level: Some(Level::All),
        })
        .send()
        .await
        .ok();
    assert_eq!(changed.access.subscription, Some(Level::All));
    assert!(
        !changed
            .exceptions
            .items
            .iter()
            .any(|row| row.node.id == project.base.id),
        "equal levels stop being exceptions in the mutation reply"
    );
    let reset = world
        .app
        .call(MeNotificationExceptionSet, &[&project.base.id])
        .token(&ada.token)
        .query(&PageQuery::default())
        .body(&NotificationExceptionSetBody { level: None })
        .send()
        .await
        .ok();
    assert_eq!(reset.access.subscription, None);
    assert_eq!(reset.access.effective_subscription, Level::All);
    assert_eq!(reset.access.inherited_subscription, Some(Level::All));
    let own = orch_db::queries::hierarchy::subscriptions_on(
        &mut *world.conn().await,
        ada.id(),
        &[project.base.id],
    )
    .await
    .unwrap();
    assert!(
        own.is_empty(),
        "Default removes the one ordinary subscription row"
    );
    // The same refusal as the bell's (`PUT /nodes/{id}/subscription`): somebody who does not view the node is
    // told what is missing there.
    let bob = person(&world, "bob").await;
    assert_eq!(
        world
            .app
            .call(MeNotificationExceptionSet, &[&project.base.id])
            .token(&bob.token)
            .query(&PageQuery::default())
            .body(&NotificationExceptionSetBody {
                level: Some(Level::All)
            })
            .send()
            .await
            .problem()
            .code,
        ErrorCode::AuthzDenied
    );
}

#[tokio::test]
async fn exception_pages_are_bounded_do_not_repeat_and_refuse_an_unrelated_cursor() {
    let world = World::new().await;
    let ada = person(&world, "ada").await;
    let root = make_org(&world, &ada, "Acme", "acme").await;
    let mut expected = vec![root.base.id];
    set(&world, &ada, root.base.id, Level::Important).await;
    for name in ["First", "Second", "Third"] {
        let project = make_project(&world, &ada, root.base.id, name).await;
        set(&world, &ada, project.base.id, Level::All).await;
        expected.push(project.base.id);
    }
    let mut cursor = None;
    let mut found = Vec::new();
    for _ in 0..8 {
        let page = world
            .app
            .call(MeNotificationExceptionList, &[])
            .token(&ada.token)
            .query(&PageQuery {
                limit: Some(1),
                cursor: cursor.clone(),
            })
            .send()
            .await
            .ok();
        assert!(page.items.len() <= 1);
        found.extend(page.items.into_iter().map(|row| row.node.id));
        cursor = page.next_cursor;
        if cursor.is_none() {
            break;
        }
    }
    assert!(cursor.is_none());
    found.sort();
    expected.sort();
    assert_eq!(found, expected);
    assert_eq!(
        world
            .app
            .call(MeNotificationExceptionList, &[])
            .token(&ada.token)
            .query(&PageQuery {
                limit: Some(1),
                cursor: Some("not-a-page".into())
            })
            .send()
            .await
            .problem()
            .code,
        ErrorCode::RequestInvalid
    );

    let first = world
        .app
        .call(MeNotificationExceptionList, &[])
        .token(&ada.token)
        .query(&PageQuery {
            limit: Some(1),
            cursor: None,
        })
        .send()
        .await
        .ok();
    let target = first.items.first().unwrap().node.id;
    let cursor = first
        .next_cursor
        .expect("there are further personal exceptions");
    let query = PageQuery {
        limit: Some(1),
        cursor: Some(cursor.clone()),
    };
    let changed = world
        .app
        .call(MeNotificationExceptionSet, &[&target])
        .token(&ada.token)
        .query(&query)
        .body(&NotificationExceptionSetBody { level: None })
        .send()
        .await
        .ok();
    assert_eq!(changed.page_cursor, Some(cursor));
    let current = world
        .app
        .call(MeNotificationExceptionList, &[])
        .token(&ada.token)
        .query(&query)
        .send()
        .await
        .ok();
    assert_eq!(
        changed.exceptions, current,
        "the response supplies exactly the requested current page"
    );

    let before = world
        .app
        .call(MeNotificationExceptionList, &[])
        .token(&ada.token)
        .query(&PageQuery::default())
        .send()
        .await
        .ok();
    let different = if changed.access.effective_subscription == Level::All {
        Level::Off
    } else {
        Level::All
    };
    assert_eq!(
        world
            .app
            .call(MeNotificationExceptionSet, &[&target])
            .token(&ada.token)
            .query(&PageQuery {
                limit: None,
                cursor: Some("not-a-page".into())
            })
            .body(&NotificationExceptionSetBody {
                level: Some(different)
            })
            .send()
            .await
            .problem()
            .code,
        ErrorCode::RequestInvalid
    );
    let after = world
        .app
        .call(MeNotificationExceptionList, &[])
        .token(&ada.token)
        .query(&PageQuery::default())
        .send()
        .await
        .ok();
    assert_eq!(
        before, after,
        "an invalid page cannot leave behind a changed subscription"
    );
}
