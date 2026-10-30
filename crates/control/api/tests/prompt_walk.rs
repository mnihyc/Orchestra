//! A person's announcement is stored verbatim, visible to another reader, and refused before any effect above its body limit.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]

mod places;
mod rig;
mod support;

use orch_api_contract::common::ErrorCode;
use orch_api_contract::topics::{
    AnnouncementCreateBody, NodeAnnouncementCreate, NodeAnnouncementList, NodeAnnouncementListQuery,
};
use rig::Scene;

#[tokio::test]
async fn announce_bound_person_walk_accepts_1500_and_refuses_1501_without_a_second_effect() {
    let s = Scene::new().await;
    s.thread_by(&s.ada, "Build").await;
    s.thread_by(&s.ada, "Check").await;
    let original = format!("<orch>{}", "界".repeat(1494));
    assert_eq!(original.chars().count(), 1500);
    let body = AnnouncementCreateBody {
        title: "A finding".to_owned(),
        body: original.clone(),
        category: None,
        severity: None,
        targets: None,
        delivery: None,
        fallback: None,
        wake_idle: None,
    };
    let made = s
        .rig
        .world
        .app
        .call(NodeAnnouncementCreate, &[&s.tree.topic.base.id])
        .token(&s.ada.token)
        .body(&body)
        .send()
        .await
        .ok();
    let made = made.announcements.into_iter().next().unwrap();
    assert_eq!(made.body, original);
    assert_eq!(made.delivered.deferred, 2);
    let listed = s
        .rig
        .world
        .app
        .call(NodeAnnouncementList, &[&s.tree.topic.base.id])
        .token(&s.ada.token)
        .query(&NodeAnnouncementListQuery::default())
        .send()
        .await
        .ok();
    assert_eq!(listed.items[0].body, original);
    let before: (i64, i64) = sqlx::query_as(
        "SELECT (SELECT count(*) FROM announcements),(SELECT count(*) FROM thread_inputs)",
    )
    .fetch_one(s.rig.db.pool())
    .await
    .unwrap();
    let mut longer = body;
    longer.body.push('界');
    let refused = s
        .rig
        .world
        .app
        .call(NodeAnnouncementCreate, &[&s.tree.topic.base.id])
        .token(&s.ada.token)
        .body(&longer)
        .send()
        .await
        .problem();
    assert_eq!(refused.code, ErrorCode::RequestInvalid);
    let after: (i64, i64) = sqlx::query_as(
        "SELECT (SELECT count(*) FROM announcements),(SELECT count(*) FROM thread_inputs)",
    )
    .fetch_one(s.rig.db.pool())
    .await
    .unwrap();
    assert_eq!(after, before);
    let read_again = s
        .rig
        .world
        .app
        .call(NodeAnnouncementList, &[&s.tree.topic.base.id])
        .token(&s.cai.token)
        .query(&NodeAnnouncementListQuery::default())
        .send()
        .await
        .ok();
    assert_eq!(read_again.items.len(), 1);
    assert_eq!(read_again.items[0].id, made.id);
}
