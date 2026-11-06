//! The native person-announcement producer retains its real audience through preparation.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a deciding test fails by panicking"
)]

mod places;
mod rig;
mod support;

use orch_access_contract::permissions::ObjectKind;
use orch_api_contract::topics::{AnnouncementCreateBody, NodeAnnouncementCreate};
use orch_api_core::middleware::authentication::{API_TOKEN_PREFIX, new_token, token_hash};
use orch_authz::{Caller, Principal};
use orch_db::queries::{identity, transaction_guard};
use orch_engine::tools::announce::{AnnounceError, PersonAnnouncement};
use orch_types::{Id, Time, new_id};
use rig::Scene;
use std::collections::HashSet;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

const WAIT_NATIVE: Duration = Duration::from_secs(30);

fn request() -> PersonAnnouncement {
    PersonAnnouncement {
        title: "A prepared native announcement".into(),
        body: "The native audience receives these supplied words.".into(),
        category: None,
        severity: None,
        targets: None,
        delivery: None,
        fallback: None,
        wake_idle: Some(false),
    }
}

async fn counts(s: &Scene, thread: Id) -> (i64, i64, i64, i64, i64) {
    sqlx::query_as(
        "SELECT (SELECT count(*) FROM announcements WHERE topic_id=t.topic_id),
                (SELECT count(*) FROM items WHERE thread_id=t.node_id),
                (SELECT count(*) FROM thread_inputs WHERE thread_id=t.node_id),
                (SELECT count(*) FROM thread_pending_notices WHERE thread_id=t.node_id),
                t.next_seq FROM threads t WHERE t.node_id=$1",
    )
    .bind(thread)
    .fetch_one(&s.rig.world.db.pool)
    .await
    .unwrap()
}

#[tokio::test]
async fn person_announcement_preparation_keeps_the_native_audience_without_delivering() {
    let s = Scene::new().await;
    let live = s.thread_by(&s.ada, "Selected recipient").await.base.id;
    let archived = s.thread_by(&s.ada, "Archived recipient").await.base.id;
    let side = s.thread_by(&s.ada, "Aside").await.base.id;
    sqlx::query("UPDATE nodes SET archived_at=$2 WHERE id=$1")
        .bind(archived)
        .bind(s.rig.world.now())
        .execute(&s.rig.world.db.pool)
        .await
        .unwrap();
    sqlx::query("UPDATE threads SET side_of=$2 WHERE node_id=$1")
        .bind(side)
        .bind(live)
        .execute(&s.rig.world.db.pool)
        .await
        .unwrap();
    let empty = places::make_topic(
        &s.rig.world,
        &s.ada,
        s.tree.project.base.id,
        "Open but empty",
    )
    .await
    .base
    .id;
    let done = places::make_topic(&s.rig.world, &s.ada, s.tree.project.base.id, "Already done")
        .await
        .base
        .id;
    sqlx::query("UPDATE topics SET status='done' WHERE node_id=$1")
        .bind(done)
        .execute(&s.rig.world.db.pool)
        .await
        .unwrap();
    let before = [
        counts(&s, live).await,
        counts(&s, archived).await,
        counts(&s, side).await,
    ];
    let mut tx = s.rig.db.begin().await.unwrap();
    transaction_guard::lock_action(&mut tx).await.unwrap();
    let principal = Principal::load(
        &mut tx,
        Caller::session(s.ada.id()),
        Time::new(s.rig.world.now()),
    )
    .await
    .unwrap();
    let engine = &s.rig.world.app.app.engine;
    let prepared = engine
        .prepare_person_announcement_in(&mut tx, &principal, s.tree.project.base.id, &request())
        .await
        .unwrap();
    assert_eq!(prepared.place(), s.tree.project.base.id);
    let actual = prepared.destinations().collect::<HashSet<_>>();
    assert_eq!(
        actual,
        HashSet::from([
            (ObjectKind::Topic, s.tree.topic.base.id),
            (ObjectKind::Topic, empty),
            (ObjectKind::Thread, live),
        ])
    );
    let mut active = request();
    active.targets = Some(orch_api_contract::topics::AnnouncementSendAudience::Active);
    let active = engine
        .prepare_person_announcement_in(&mut tx, &principal, s.tree.project.base.id, &active)
        .await
        .unwrap();
    assert_eq!(
        active.destinations().collect::<HashSet<_>>(),
        HashSet::from([
            (ObjectKind::Topic, s.tree.topic.base.id),
            (ObjectKind::Topic, empty)
        ]),
        "active-only selection has no fabricated recipient when all real threads are idle"
    );
    assert_eq!(
        before,
        [
            counts(&s, live).await,
            counts(&s, archived).await,
            counts(&s, side).await
        ]
    );
    let checks = Arc::new(AtomicUsize::new(0));
    let checked = Arc::clone(&checks);
    let sent = engine
        .announce_prepared_place_guarded(tx, principal, &prepared, move || {
            checked.fetch_add(1, Ordering::SeqCst);
            Ok(())
        })
        .await
        .unwrap();
    assert_eq!(
        checks.load(Ordering::SeqCst),
        1,
        "one batch has one first-effect cut"
    );
    assert_eq!(
        sent.sent.len(),
        2,
        "an empty open topic still receives its announcement"
    );
    assert_eq!(sent.skipped, 1);
    assert_eq!(
        sent.sent
            .iter()
            .map(|row| row.delivered.deferred)
            .sum::<i64>(),
        1
    );
    assert_eq!(counts(&s, archived).await.1, before[1].1);
    assert_eq!(counts(&s, side).await.1, before[2].1);
}

#[tokio::test]
async fn person_announcement_checks_the_http_key_after_its_real_recipient_row_wait() {
    for expire in [false, true] {
        let s = Scene::new().await;
        let thread = s.thread_by(&s.ada, "The locked recipient").await.base.id;
        let secret = new_token(API_TOKEN_PREFIX);
        identity::create_api_token(
            &mut *s.rig.world.conn().await,
            &identity::NewApiToken {
                id: new_id(),
                user_id: s.ada.id(),
                name: "Native announcement wait",
                token_hash: &token_hash(&secret),
                scopes: &["*".into()],
                node_id: None,
                at: s.rig.world.now(),
                expires_at: Some(s.rig.world.now() + chrono::Duration::hours(1)),
            },
        )
        .await
        .unwrap();
        let before = counts(&s, thread).await;
        let mut blocker = s.rig.world.db.pool.begin().await.unwrap();
        let blocker_pid: i32 = sqlx::query_scalar("SELECT pg_backend_pid()")
            .fetch_one(&mut *blocker)
            .await
            .unwrap();
        sqlx::query("SELECT node_id FROM threads WHERE node_id=$1 FOR UPDATE")
            .bind(thread)
            .fetch_one(&mut *blocker)
            .await
            .unwrap();
        let app = s.rig.world.app.clone();
        let topic = s.tree.topic.base.id;
        let body = request();
        let mut action = tokio::spawn(async move {
            app.call(NodeAnnouncementCreate, &[&topic])
                .token(&secret)
                .body(&AnnouncementCreateBody {
                    title: body.title,
                    body: body.body,
                    category: body.category,
                    severity: body.severity,
                    targets: body.targets,
                    delivery: body.delivery,
                    fallback: body.fallback,
                    wake_idle: body.wake_idle,
                })
                .send_bytes()
                .await
        });
        let reached = tokio::time::timeout(WAIT_NATIVE, async {
            loop {
                if action.is_finished() { break false; }
                let blocked: bool = sqlx::query_scalar(
                    "SELECT EXISTS(SELECT 1 FROM pg_stat_activity WHERE datname=current_database()
                     AND wait_event_type='Lock' AND $1=ANY(pg_blocking_pids(pid))
                     AND query LIKE '%SELECT node_id FROM threads WHERE node_id = ANY($1) ORDER BY node_id FOR UPDATE%')",
                ).bind(blocker_pid).fetch_one(&s.rig.world.db.pool).await.unwrap();
                if blocked { break true; }
                tokio::task::yield_now().await;
            }
        }).await.unwrap_or(false);
        let during = counts(&s, thread).await;
        if reached && expire {
            s.rig.world.advance(chrono::Duration::hours(2));
        }
        blocker.rollback().await.unwrap();
        let completed = tokio::time::timeout(WAIT_NATIVE, &mut action).await;
        if completed.is_err() {
            action.abort();
            let _ = action.await;
        }
        let reply = completed
            .expect("the native announcement must drain after releasing its recipient")
            .unwrap();
        assert!(
            reached,
            "the native recipient row lock must actually be reached"
        );
        assert_eq!(
            during, before,
            "nothing is delivered while preparation waits"
        );
        let after = counts(&s, thread).await;
        if expire {
            assert!(
                matches!(reply.status.as_u16(), 401 | 403),
                "{}",
                String::from_utf8_lossy(&reply.raw)
            );
            assert_eq!(
                after, before,
                "the first native INSERT must retain the original key boundary"
            );
        } else {
            assert_eq!(reply.status, 201, "{}", String::from_utf8_lossy(&reply.raw));
            assert_eq!(after.0, before.0 + 1);
            assert_eq!(after.1, before.1 + 1);
            assert_eq!(after.2, before.2);
            assert_eq!(after.3, before.3 + 1);
        }
    }
}

#[tokio::test]
async fn person_announcement_operands_cannot_cross_their_native_transaction() {
    let s = Scene::new().await;
    let thread = s.thread().await.base.id;
    let before = counts(&s, thread).await;
    let mut preparing = s.rig.db.begin().await.unwrap();
    transaction_guard::lock_action(&mut preparing)
        .await
        .unwrap();
    let principal = Principal::load(
        &mut preparing,
        Caller::session(s.ada.id()),
        Time::new(s.rig.world.now()),
    )
    .await
    .unwrap();
    let engine = &s.rig.world.app.app.engine;
    let prepared = engine
        .prepare_person_announcement_in(
            &mut preparing,
            &principal,
            s.tree.topic.base.id,
            &request(),
        )
        .await
        .unwrap();
    let applying = s.rig.db.begin().await.unwrap();
    let checks = Arc::new(AtomicUsize::new(0));
    let checked = Arc::clone(&checks);
    let result = engine
        .announce_prepared_place_guarded(applying, principal, &prepared, move || {
            checked.fetch_add(1, Ordering::SeqCst);
            Ok(())
        })
        .await;
    preparing.rollback().await.unwrap();
    assert!(matches!(
        result,
        Err(AnnounceError::Internal(
            orch_engine::EngineError::Unreadable(_)
        ))
    ));
    assert_eq!(checks.load(Ordering::SeqCst), 0);
    assert_eq!(counts(&s, thread).await, before);
}
