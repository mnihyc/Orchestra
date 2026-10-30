//! UA-16: a notification page names current readable threads without needing any browser tree.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]

mod desk;
mod places;
mod search_world;
mod support;

use desk::Desk;
use orch_api_contract::access::{PageChange, PermissionPageGet, PermissionPageUpdate};
use orch_api_contract::nodes::{Node, NodeUpdate};
use orch_api_contract::notifications::{
    Notification, NotificationGrouping, NotificationList, NotificationListQuery,
};
use orch_db::queries::notifications::people::{NewNotification, announce};
use orch_types::{Id, new_id};
use places::{Who, join, person};
use serde_json::json;

async fn tell(desk: &Desk, to: &Who, thread: Id) {
    let mut conn = desk.world.conn().await;
    announce(
        &mut conn,
        &NewNotification {
            user_id: to.id(),
            kind: "turn.completed",
            node_id: Some(desk.topic),
            thread_id: Some(thread),
            approval_id: None,
            chat_id: None,
            title: "Turn completed",
            body: "Turn finished",
            data: &json!({}),
            dedupe_key: &new_id().to_string(),
            at: desk.world.now(),
        },
    )
    .await
    .unwrap()
    .expect("the notification is written");
}

async fn list(desk: &Desk, by: &Who, grouping: Option<NotificationGrouping>) -> Vec<Notification> {
    desk.world
        .app
        .call(NotificationList, &[])
        .token(&by.token)
        .query(&NotificationListQuery {
            group_by: grouping,
            ..NotificationListQuery::default()
        })
        .send()
        .await
        .ok()
        .items
}

#[tokio::test]
async fn threads_in_one_topic_have_distinct_current_titles_in_day_and_place_pages_and_private_names_stay_private()
 {
    let desk = Desk::new().await;
    let vera = person(&desk.world, "vera").await;
    join(&desk.world, desk.org, desk.org, &vera, "viewer", &desk.ada).await;
    let (first, _) = search_world::thread(&desk, &desk.ada, desk.topic, "Fix the redirect").await;
    let (second, _) =
        search_world::thread(&desk, &desk.ada, desk.topic, "Check the rate limit").await;
    tell(&desk, &vera, first).await;
    tell(&desk, &vera, second).await;
    for grouping in [None, Some(NotificationGrouping::Place)] {
        let page = list(&desk, &vera, grouping).await;
        for (id, name) in [
            (first, "Fix the redirect"),
            (second, "Check the rate limit"),
        ] {
            let row = page
                .iter()
                .find(|row| row.thread_id == Some(id))
                .expect("the existing row is listed");
            assert_eq!(
                row.thread.as_ref().and_then(|node| node.name.as_deref()),
                Some(name)
            );
            assert_eq!(row.node.as_ref().map(|node| node.id), Some(desk.topic));
            assert_eq!(row.path.last().map(|node| node.id), Some(id));
            assert!(
                row.path
                    .iter()
                    .any(|node| node.id == desk.topic && !node.redacted)
            );
            assert_eq!(
                row.title, "Turn completed",
                "stored fallback text is not repurposed into a name"
            );
        }
    }
    let Node::Thread(before) = places::read(&desk.world, &desk.ada, first).await else {
        panic!("a thread")
    };
    desk.world
        .app
        .call(NodeUpdate, &[&first])
        .token(&desk.ada.token)
        .raw_body(
            &json!({"name":"Redirect ready for review","expected_revision":before.base.revision}),
        )
        .send()
        .await
        .ok();
    let renamed = list(&desk, &vera, None).await;
    assert_eq!(
        renamed
            .iter()
            .find(|row| row.thread_id == Some(first))
            .and_then(|row| row.thread.as_ref())
            .and_then(|node| node.name.as_deref()),
        Some("Redirect ready for review")
    );

    let page = desk
        .world
        .app
        .call(PermissionPageGet, &[&"nodes", &first])
        .token(&desk.ada.token)
        .send()
        .await
        .ok();
    let change: PageChange =
        serde_json::from_value(json!({"whole":"private","expected_revision":page.revision}))
            .unwrap();
    desk.world
        .app
        .call(PermissionPageUpdate, &[&"nodes", &first])
        .token(&desk.ada.token)
        .body(&change)
        .send()
        .await
        .ok();
    for grouping in [None, Some(NotificationGrouping::Place)] {
        let page = list(&desk, &vera, grouping).await;
        let row = page
            .iter()
            .find(|row| row.thread_id == Some(first))
            .expect("its historical notice stays in the Inbox");
        assert!(
            row.thread
                .as_ref()
                .is_none_or(|node| node.redacted && node.name.is_none())
        );
        assert!(
            row.path
                .iter()
                .filter(|node| node.id == first)
                .all(|node| node.redacted && node.name.is_none())
        );
        assert!(
            !row.path
                .iter()
                .any(|node| node.name.as_deref() == Some("Redirect ready for review"))
        );
    }
}
