//! Calendar scope cuts precede amounts and ranking, while the native audience remains independent.

use super::{checked, key};
use crate::{places, support::World, usage_world};
use chrono::Duration;
use http::StatusCode;
use orch_api_contract::users::{
    MeProfileUpdate, MeProfileUpdateBody, ProfileActivityGet, ProfileAudience,
};
use orch_types::{Id, Json, new_id};
use serde_json::json;
use usage_world::{Use, another_thread, spend_in, team};

fn selected(operation: &str, kind: &str, id: Id, action: &str) -> Json {
    json!({"target":{"kind":kind,"id":id,"reach":"self"},"resources":[kind],
        "allow":[{"operation":operation,"actions":[action]}]})
}
fn activity_rule(kind: &str, id: Id) -> Json {
    selected("ProfileActivityGet", kind, id, "read")
}

#[tokio::test]
async fn activity_scope_filters_threads_before_days_and_keeps_native_full_parity() {
    let world = World::new().await;
    let t = team(&world).await;
    let second = another_thread(&world, &t, "Other visible work").await;
    for thread in [t.thread, second] {
        spend_in(&world, &t, thread, &Use::of(&world, t.boss.id(), 100)).await;
    }
    let username = t.boss.person.username.as_str();
    let token = key(
        &world,
        t.viewer.id(),
        vec![
            activity_rule("account", t.boss.id()),
            activity_rule("thread", t.thread),
        ],
    )
    .await;
    let cut = checked(
        world
            .app
            .call(ProfileActivityGet, &[&username])
            .token(&token)
            .send_bytes()
            .await,
        StatusCode::OK,
    );
    assert_eq!(cut["total"], 1);
    assert_eq!(cut["days"].as_array().unwrap().len(), 1);
    assert_eq!(cut["days"][0]["turns"], 1);
    assert_eq!(cut["recent"], json!([]));
    let unrelated = key(
        &world,
        t.viewer.id(),
        vec![
            activity_rule("account", t.boss.id()),
            activity_rule("thread", t.thread),
            selected("NodeGet.read", "topic", t.topic, "read"),
        ],
    )
    .await;
    let unborrowed = checked(
        world
            .app
            .call(ProfileActivityGet, &[&username])
            .token(&unrelated)
            .send_bytes()
            .await,
        StatusCode::OK,
    );
    assert_eq!(unborrowed["total"], 1);
    assert_eq!(unborrowed["recent"], json!([]));
    let token = key(
        &world,
        t.viewer.id(),
        vec![
            activity_rule("account", t.boss.id()),
            activity_rule("thread", t.thread),
            activity_rule("thread", second),
            activity_rule("topic", t.topic),
        ],
    )
    .await;
    let full = world
        .app
        .call(ProfileActivityGet, &[&username])
        .token(&t.viewer.token)
        .send()
        .await
        .ok();
    let scoped = checked(
        world
            .app
            .call(ProfileActivityGet, &[&username])
            .token(&token)
            .send_bytes()
            .await,
        StatusCode::OK,
    );
    assert_eq!(scoped, serde_json::to_value(full).unwrap());
    assert_eq!(scoped["total"], 2);
    assert_eq!(scoped["recent"][0]["node"]["id"], json!(t.topic));
}

#[tokio::test]
async fn activity_scope_keeps_operation_action_account_and_work_targets_independent() {
    let world = World::new().await;
    let t = team(&world).await;
    spend_in(&world, &t, t.thread, &Use::of(&world, t.boss.id(), 100)).await;
    let username = t.boss.person.username.as_str();
    for root in [
        activity_rule("account", t.viewer.id()),
        selected("ProfileGet", "account", t.boss.id(), "read"),
        selected("ProfileActivityGet", "account", t.boss.id(), "manage"),
        json!({"target":{"kind":"org","id":t.acme,"reach":"subtree"},"resources":["account"],
            "allow":[{"operation":"ProfileActivityGet","actions":["read"]}]}),
    ] {
        let token = key(
            &world,
            t.viewer.id(),
            vec![
                root,
                activity_rule("thread", t.thread),
                activity_rule("topic", t.topic),
            ],
        )
        .await;
        checked(
            world
                .app
                .call(ProfileActivityGet, &[&username])
                .token(&token)
                .send_bytes()
                .await,
            StatusCode::FORBIDDEN,
        );
    }
    let unused = another_thread(&world, &t, "No counted work").await;
    for sources in [
        vec![],
        vec![activity_rule("thread", unused)],
        vec![activity_rule("topic", t.topic)],
    ] {
        let token = key(
            &world,
            t.viewer.id(),
            std::iter::once(activity_rule("account", t.boss.id()))
                .chain(sources)
                .collect(),
        )
        .await;
        let empty = checked(
            world
                .app
                .call(ProfileActivityGet, &[&username])
                .token(&token)
                .send_bytes()
                .await,
            StatusCode::OK,
        );
        assert_eq!(empty["total"], 0);
        assert_eq!(empty["days"], json!([]));
        assert_eq!(empty["recent"], json!([]));
    }
}

#[tokio::test]
async fn activity_scope_cannot_supply_profile_audience_or_native_thread_view() {
    let world = World::new().await;
    let t = team(&world).await;
    spend_in(&world, &t, t.thread, &Use::of(&world, t.boss.id(), 100)).await;
    let username = t.boss.person.username.as_str();
    let rules = vec![
        activity_rule("account", t.boss.id()),
        activity_rule("thread", t.thread),
        activity_rule("topic", t.topic),
    ];
    let token = key(&world, t.stranger.id(), rules.clone()).await;
    let empty = checked(
        world
            .app
            .call(ProfileActivityGet, &[&username])
            .token(&token)
            .send_bytes()
            .await,
        StatusCode::OK,
    );
    assert_eq!(empty["total"], 0);
    world
        .app
        .call(MeProfileUpdate, &[])
        .token(&t.boss.token)
        .body(&MeProfileUpdateBody {
            audience: Some(ProfileAudience::Private),
            ..Default::default()
        })
        .send()
        .await
        .ok();
    let token = key(&world, t.viewer.id(), rules.clone()).await;
    checked(
        world
            .app
            .call(ProfileActivityGet, &[&username])
            .token(&token)
            .send_bytes()
            .await,
        StatusCode::NOT_FOUND,
    );
    // A key never borrows an administrator's open browser inspection mode.
    places::admin_mode(&world, &t.admin, true).await;
    let token = key(&world, t.admin.id(), rules.clone()).await;
    checked(
        world
            .app
            .call(ProfileActivityGet, &[&username])
            .token(&token)
            .send_bytes()
            .await,
        StatusCode::NOT_FOUND,
    );
    let token = key(&world, t.boss.id(), rules).await;
    assert_eq!(
        checked(
            world
                .app
                .call(ProfileActivityGet, &[&username])
                .token(&token)
                .send_bytes()
                .await,
            StatusCode::OK
        )["total"],
        1
    );
}

#[tokio::test]
async fn activity_scope_filters_before_the_native_eight_topic_ranking() {
    let world = World::new().await;
    let mut t = team(&world).await;
    let wanted_topic = t.topic;
    spend_in(
        &world,
        &t,
        t.thread,
        &Use::of(&world, t.boss.id(), 100).at(world.now() - Duration::minutes(10)),
    )
    .await;
    // Eight newer native topics would consume the original recent window before a post-limit filter.
    for index in 0..8 {
        let topic = places::make_topic(&world, &t.boss, t.project, &format!("Newer work {index}"))
            .await
            .base
            .id;
        let thread = new_id();
        orch_db::queries::hierarchy::testing::insert_node(
            &mut *world.conn().await,
            thread,
            "thread",
            Some(topic),
            "Newer work",
            t.boss.id(),
        )
        .await
        .unwrap();
        t.topic = topic;
        spend_in(
            &world,
            &t,
            thread,
            &Use::of(&world, t.boss.id(), 100).at(world.now() - Duration::minutes(1)),
        )
        .await;
    }
    let username = t.boss.person.username.as_str();
    let ordinary = world
        .app
        .call(ProfileActivityGet, &[&username])
        .token(&t.viewer.token)
        .send()
        .await
        .ok();
    assert_eq!(ordinary.total, 9);
    assert_eq!(ordinary.recent.len(), 8);
    assert!(
        !ordinary
            .recent
            .iter()
            .any(|recent| recent.node.id == wanted_topic)
    );
    let token = key(
        &world,
        t.viewer.id(),
        vec![
            activity_rule("account", t.boss.id()),
            activity_rule("thread", t.thread),
            activity_rule("topic", wanted_topic),
        ],
    )
    .await;
    let scoped = checked(
        world
            .app
            .call(ProfileActivityGet, &[&username])
            .token(&token)
            .send_bytes()
            .await,
        StatusCode::OK,
    );
    assert_eq!(scoped["total"], 1);
    assert_eq!(scoped["recent"].as_array().unwrap().len(), 1);
    assert_eq!(scoped["recent"][0]["node"]["id"], json!(wanted_topic));
}

#[tokio::test]
async fn activity_publication_rechecks_native_visibility_after_the_actual_activity_wait() {
    let world = World::new().await;
    let t = team(&world).await;
    spend_in(&world, &t, t.thread, &Use::of(&world, t.boss.id(), 100)).await;
    let username = t.boss.person.username.clone();
    let token = key(
        &world,
        t.viewer.id(),
        vec![
            activity_rule("account", t.boss.id()),
            activity_rule("thread", t.thread),
            activity_rule("topic", t.topic),
        ],
    )
    .await;
    assert_eq!(
        checked(
            world
                .app
                .call(ProfileActivityGet, &[&username.as_str()])
                .token(&token)
                .send_bytes()
                .await,
            StatusCode::OK
        )["total"],
        1
    );
    let mut blocker = world.db.pool.begin().await.unwrap();
    sqlx::query("LOCK TABLE api_token_activity IN SHARE MODE")
        .execute(&mut *blocker)
        .await
        .unwrap();
    let pid: i32 = sqlx::query_scalar("SELECT pg_backend_pid()")
        .fetch_one(&mut *blocker)
        .await
        .unwrap();
    let app = world.app.clone();
    let request = tokio::spawn(async move {
        let call = app
            .call(ProfileActivityGet, &[&username.as_str()])
            .token(&token);
        call.send_bytes().await
    });
    let wait = std::time::Duration::from_secs(30);
    tokio::time::timeout(wait, async {
        loop {
            assert!(!request.is_finished(), "the actual activity append must be blocked");
            let blocked: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM pg_stat_activity WHERE datname=current_database() AND wait_event_type='Lock' AND $1=ANY(pg_blocking_pids(pid)) AND query LIKE '%INSERT INTO api_token_activity%')")
                .bind(pid).fetch_one(&world.db.pool).await.unwrap();
            if blocked { break; }
            tokio::task::yield_now().await;
        }
    }).await.expect("the native publication activity wait must be observed");
    world
        .app
        .call(MeProfileUpdate, &[])
        .token(&t.boss.token)
        .body(&MeProfileUpdateBody {
            show_activity: Some(false),
            ..Default::default()
        })
        .send()
        .await
        .ok();
    blocker.commit().await.unwrap();
    let reply = tokio::time::timeout(wait, request).await.unwrap().unwrap();
    assert_eq!(reply.status, StatusCode::NOT_FOUND);
    assert!(!String::from_utf8_lossy(&reply.raw).contains("\"total\":1"));
}
