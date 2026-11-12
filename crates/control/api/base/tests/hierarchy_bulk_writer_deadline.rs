//! A source-root lock cannot fence the native move UPDATE's descendant row wait.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "tests fail by panicking"
)]
mod places;
mod support;
use chrono::Duration;
use http::StatusCode;
use orch_api_contract::nodes::TopicMove;
use orch_api_core::middleware::authentication::{API_TOKEN_PREFIX, new_token, token_hash};
use orch_db::queries::{hierarchy, identity, threads};
use orch_types::new_id;
use places::{make_org, make_project, make_topic, person};
use serde_json::json;
use support::World;

#[tokio::test]
async fn move_rechecks_before_bulk_hierarchy_effect_after_actual_descendant_wait() {
    let world = World::new().await;
    let owner = person(&world, "bulk-move-owner").await;
    let org = make_org(&world, &owner, "Bulk office", "bulk-office")
        .await
        .base
        .id;
    let source = make_project(&world, &owner, org, "Native source")
        .await
        .base
        .id;
    let target = make_project(&world, &owner, org, "Native target")
        .await
        .base
        .id;
    let mirror = make_topic(&world, &owner, source, "Native ordinary mirror").await;
    let ordinary = world
        .app
        .call(TopicMove, &[&mirror.base.id])
        .token(&owner.token)
        .raw_body(&json!({"parent_id":target,"expected_revision":mirror.base.revision}))
        .send_bytes()
        .await;
    assert_eq!(
        ordinary.status,
        StatusCode::OK,
        "{}",
        String::from_utf8_lossy(&ordinary.raw)
    );
    let topic = make_topic(&world, &owner, source, "Native bulk source").await;
    let child = new_id();
    assert!(
        threads::insert_thread(
            &mut *world.conn().await,
            child,
            topic.base.id,
            "Native descendant",
            owner.id(),
            world.now()
        )
        .await
        .unwrap()
    );
    let before = hierarchy::with_ancestors(&mut *world.conn().await, &[child])
        .await
        .unwrap()
        .into_iter()
        .find(|row| row.id == child)
        .unwrap()
        .path;
    let secret = new_token(API_TOKEN_PREFIX);
    identity::create_api_token(
        &mut *world.conn().await,
        &identity::NewApiToken {
            id: new_id(),
            user_id: owner.id(),
            name: "Bulk native writer deadline",
            token_hash: &token_hash(&secret),
            scopes: &["*".to_owned()],
            node_id: None,
            at: world.now(),
            expires_at: Some(world.now() + Duration::seconds(1)),
        },
    )
    .await
    .unwrap();
    let mut locked = world.db.pool.begin().await.unwrap();
    sqlx::query("SELECT id FROM nodes WHERE id=$1 FOR UPDATE")
        .bind(child)
        .fetch_one(&mut *locked)
        .await
        .unwrap();
    let pid: i32 = sqlx::query_scalar("SELECT pg_backend_pid()")
        .fetch_one(&mut *locked)
        .await
        .unwrap();
    let app = world.app.clone();
    let id = topic.base.id;
    let revision = topic.base.revision;
    let pending = tokio::spawn(async move {
        let path = id.to_string();
        let request = app
            .call(TopicMove, &[&path])
            .token(&secret)
            .raw_body(&json!({"parent_id":target,"expected_revision":revision}));
        request.send_bytes().await
    });
    let arrived=tokio::time::timeout(std::time::Duration::from_secs(30),async {
        loop {
            let blocked:bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM pg_stat_activity
                WHERE datname=current_database() AND pg_blocking_pids(pid) @> ARRAY[$1]::integer[]
                  AND wait_event_type='Lock' AND (lower(query) LIKE '%update nodes set path%'
                       OR (lower(query) LIKE '%from nodes%' AND lower(query) LIKE '%for update%')))")
                .bind(pid).fetch_one(&world.db.pool).await.unwrap();
            if blocked{return true;}
            if pending.is_finished(){return false;}
            tokio::task::yield_now().await;
        }
    }).await.unwrap_or(false);
    if !arrived {
        pending.abort();
        let result = pending.await;
        locked.rollback().await.unwrap();
        panic!("real native descendant wait was not reached: {result:?}");
    }
    world.advance(Duration::seconds(1));
    locked.rollback().await.unwrap();
    let reply = tokio::time::timeout(std::time::Duration::from_secs(30), pending)
        .await
        .unwrap()
        .unwrap();
    let current = hierarchy::with_ancestors(&mut *world.conn().await, &[id])
        .await
        .unwrap()
        .into_iter()
        .find(|row| row.id == id)
        .unwrap();
    let descendant = hierarchy::with_ancestors(&mut *world.conn().await, &[child])
        .await
        .unwrap()
        .into_iter()
        .find(|row| row.id == child)
        .unwrap();
    assert_eq!(
        current.parent_id,
        Some(source),
        "expired action moved real root; HTTP{}",
        reply.status
    );
    assert_eq!(
        descendant.path, before,
        "expired bulk action rewrote descendant ancestry"
    );
    assert_eq!(reply.status, StatusCode::UNAUTHORIZED);
}
