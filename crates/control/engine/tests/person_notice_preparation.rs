//! The native explicit-notice writer exposes its real first Inbox effect without replay.
#![allow(clippy::unwrap_used, reason = "a control fails by panicking")]

mod common;

use common::World;
use orch_db::queries::notifications::people::NewNotification;
use orch_engine::{EngineError, notifications::tell_person_notice_before};
use orch_types::new_id;
use serde_json::json;
use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering},
};

#[tokio::test]
async fn native_person_notice_refusal_and_replay_consume_no_inbox_effect() {
    let world = World::new().await;
    let receiver = world.person("native-notice-recipient").await;
    let key = format!("person.notify:{}", new_id());
    let data = json!({"target_node_id":world.topic});
    let notice = NewNotification {
        user_id: receiver,
        kind: "person.notify",
        node_id: Some(world.topic),
        thread_id: None,
        approval_id: None,
        chat_id: None,
        title: "Supplied native notice",
        body: "Supplied words",
        data: &data,
        dedupe_key: &key,
        at: world.clock.now(),
    };
    let initial: i64 = sqlx::query_scalar("SELECT inbox_seq FROM users WHERE id=$1")
        .bind(receiver)
        .fetch_one(world.pool.pool())
        .await
        .unwrap();
    let calls = Arc::new(AtomicUsize::new(0));
    let refused = Arc::clone(&calls);
    let mut tx = world.pool.begin().await.unwrap();
    let result = tell_person_notice_before(
        &mut tx,
        Some(world.root),
        &notice,
        Some(move || {
            refused.fetch_add(1, Ordering::SeqCst);
            Err(EngineError::unreadable(
                "native before-effect fixture refusal",
            ))
        }),
    )
    .await;
    assert!(result.is_err());
    let counts:(i64,i64)=sqlx::query_as("SELECT inbox_seq,(SELECT count(*) FROM notifications WHERE user_id=$1) FROM users WHERE id=$1")
        .bind(receiver).fetch_one(&mut *tx).await.unwrap();
    assert_eq!(counts, (initial, 0));
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    tx.rollback().await.unwrap();

    let accepted = Arc::clone(&calls);
    let mut tx = world.pool.begin().await.unwrap();
    let actual = tell_person_notice_before(
        &mut tx,
        Some(world.root),
        &notice,
        Some(move || {
            accepted.fetch_add(1, Ordering::SeqCst);
            Ok(())
        }),
    )
    .await
    .unwrap()
    .unwrap();
    tx.commit().await.unwrap();
    let stored: (orch_types::Id, i64) =
        sqlx::query_as("SELECT id,inbox_seq FROM notifications WHERE user_id=$1 AND dedupe_key=$2")
            .bind(receiver)
            .bind(&key)
            .fetch_one(world.pool.pool())
            .await
            .unwrap();
    assert_eq!(stored, (actual.id, actual.inbox_seq));
    assert_eq!(actual.inbox_seq, initial + 1);
    assert_eq!(calls.load(Ordering::SeqCst), 2);

    let duplicate = Arc::clone(&calls);
    let mut tx = world.pool.begin().await.unwrap();
    assert!(
        tell_person_notice_before(
            &mut tx,
            Some(world.root),
            &notice,
            Some(move || {
                duplicate.fetch_add(1, Ordering::SeqCst);
                Ok(())
            })
        )
        .await
        .unwrap()
        .is_none()
    );
    tx.commit().await.unwrap();
    assert_eq!(calls.load(Ordering::SeqCst), 2);
    let final_seq: i64 = sqlx::query_scalar("SELECT inbox_seq FROM users WHERE id=$1")
        .bind(receiver)
        .fetch_one(world.pool.pool())
        .await
        .unwrap();
    assert_eq!(final_seq, initial + 1);
}

#[tokio::test]
async fn native_inbox_first_effect_coexists_with_the_senders_real_identity_key_share_lock() {
    let world = World::new().await;
    let mut identity = world.pool.begin().await.unwrap();
    let initial: i64 = sqlx::query_scalar("SELECT inbox_seq FROM users WHERE id=$1")
        .bind(world.ada)
        .fetch_one(&mut *identity)
        .await
        .unwrap();
    sqlx::query("SELECT id FROM users WHERE id=$1 FOR KEY SHARE")
        .bind(world.ada)
        .execute(&mut *identity)
        .await
        .unwrap();
    let key = format!("person.notify:{}", new_id());
    let data = json!({"target_node_id":world.topic});
    let notice = NewNotification {
        user_id: world.ada,
        kind: "person.notify",
        node_id: Some(world.topic),
        thread_id: None,
        approval_id: None,
        chat_id: None,
        title: "Actual identity lock",
        body: "A self recipient",
        data: &data,
        dedupe_key: &key,
        at: world.clock.now(),
    };
    let mut tx = world.pool.begin().await.unwrap();
    let actual = tokio::time::timeout(
        std::time::Duration::from_secs(30),
        tell_person_notice_before(&mut tx, Some(world.root), &notice, Some(|| Ok(()))),
    )
    .await
    .expect("the Inbox non-key write must not wait on an authenticated sender's key-share identity")
    .unwrap()
    .unwrap();
    assert_eq!(actual.inbox_seq, initial + 1);
    tx.commit().await.unwrap();
    identity.rollback().await.unwrap();
}
