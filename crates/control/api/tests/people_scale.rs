//! People pages and discussion jobs keep bounded batches, exact replay and current reading at scale.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]
mod desk;
mod places;
mod support;

use desk::Desk;
use orch_api_contract::access::{
    AccessChoice, HolderInput, PageChange, PageChangeName, PermissionPageUpdate,
};
use orch_api_contract::chats::ChatMentionListQuery;
use orch_api_contract::documents::{
    DocumentCommentCreate, DocumentCommentCreateBody, DocumentMentionList,
};
use orch_api_contract::notifications::{
    DocumentNoticeJobList, PERSON_NOTIFY_MAX_PEOPLE, PersonNotifyBody, PersonNotifyCreate,
};
use orch_api_contract::workspace::{JobCancel, JobGet};
use orch_db::queries::{documents, identity, jobs};
use orch_engine::jobs::document_notices::{self, BATCH, Plan};
use orch_types::{Id, new_id};
use serde_json::json;

#[tokio::test]
async fn a_disabled_hourly_policy_still_keeps_each_synchronous_act_bounded() {
    let desk = Desk::new().await;
    let reader = desk.member("reader", "viewer").await;
    let doc = desk.make(&desk.ada, "Guide").await;
    desk.world
        .set("notifications.person_rate_per_hour", json!(0))
        .await;
    let refused = desk
        .world
        .app
        .call(PersonNotifyCreate, &[&"documents", &doc.base.id])
        .token(&desk.ada.token)
        .body(&PersonNotifyBody {
            user_ids: vec![reader.id(); PERSON_NOTIFY_MAX_PEOPLE + 1],
            anchor: None,
            note: None,
        })
        .send()
        .await;
    assert_eq!(refused.status.as_u16(), 400);
    let count: i64 =
        sqlx::query_scalar("SELECT count(*) FROM notifications WHERE kind='person.notify'")
            .fetch_one(&desk.world.db.pool)
            .await
            .unwrap();
    assert_eq!(count, 0);
}

#[tokio::test]
async fn a_large_discussion_job_resumes_after_one_committed_page_without_replaying_people() {
    let desk = Desk::new().await;
    let doc = desk.make(&desk.ada, "Scale guide").await;
    let discussion = new_id();
    let now = desk.world.app.app.clock.now();
    let mut conn = desk.world.db.pool.acquire().await.unwrap();
    sqlx::query("INSERT INTO users(id,username,display_name,avatar_color,created_at) SELECT md5('people-job:'||g)::uuid,'jobperson'||g,'Job person '||g,0,$1 FROM generate_series(1,205)g")
        .bind(now).execute(&mut *conn).await.unwrap();
    sqlx::query("INSERT INTO org_members(org_id,user_id,domain_id) SELECT $1,md5('people-job:'||g)::uuid,$1 FROM generate_series(1,205)g")
        .bind(desk.org).execute(&mut *conn).await.unwrap();
    desk.world
        .app
        .call(PermissionPageUpdate, &[&"documents", &doc.base.id])
        .token(&desk.ada.token)
        .body(&PageChange {
            name: Some(vec![PageChangeName {
                holders: vec![HolderInput::Members { id: desk.org }],
                choice: AccessChoice::FullView,
                export: Some(false),
                options: None,
                expires_at: None,
                note: None,
            }]),
            ..PageChange::default()
        })
        .send()
        .await
        .ok();
    sqlx::query("INSERT INTO document_states(user_id,document_id,discussion_level) SELECT md5('people-job:'||g)::uuid,$1,'all' FROM generate_series(1,205)g")
        .bind(doc.base.id).execute(&mut *conn).await.unwrap();
    let legacy:Id=sqlx::query_scalar("UPDATE document_states SET discussion_subscription_seq=NULL WHERE document_id=$1 AND user_id=md5('people-job:1')::uuid RETURNING user_id")
        .bind(doc.base.id).fetch_one(&mut *conn).await.unwrap();
    assert_eq!(
        documents::people::discussion_audience_page(
            &mut conn,
            doc.base.id,
            discussion,
            None,
            0,
            0,
            true,
            10
        )
        .await
        .unwrap(),
        [legacy],
        "pre-migration subscribers remain in generation zero without a table rewrite"
    );
    sqlx::query("INSERT INTO document_discussions(id,document_id,created_by,seq_updated,created_at) VALUES($1,$2,$3,0,$4)")
        .bind(discussion).bind(doc.base.id).bind(desk.ada.id()).bind(now).execute(&mut *conn).await.unwrap();
    desk.world
        .app
        .call(DocumentCommentCreate, &[&doc.base.id, &discussion])
        .token(&desk.ada.token)
        .body(&DocumentCommentCreateBody {
            content: desk::text("Scale reply"),
            client_id: new_id().to_string(),
        })
        .send()
        .await
        .ok();
    let id: Id = sqlx::query_scalar("SELECT id FROM jobs WHERE kind='document_notices'")
        .fetch_one(&mut *conn)
        .await
        .unwrap();
    let queued = jobs::job(&mut conn, id).await.unwrap().unwrap();
    assert_eq!(queued.state, "queued");
    let outsider = desk.stranger("outsider").await;
    assert!(
        !desk
            .world
            .app
            .call(JobCancel, &[&id])
            .token(&outsider.token)
            .send()
            .await
            .status
            .is_success()
    );
    let plan: Plan = serde_json::from_value(queued.plan["notice"].clone()).unwrap();
    let page = document_notices::audience(&mut conn, &plan, None, now)
        .await
        .unwrap();
    assert_eq!(page.people.len(), BATCH);
    assert!(page.more);
    let first = page.people[0];
    // The receipt and its cursor share a transaction, as the runner does before a restart.
    let mut tx = desk.world.db.pool.begin().await.unwrap();
    orch_db::queries::transaction_guard::lock_action(&mut tx)
        .await
        .unwrap();
    document_notices::deliver(&mut tx, &plan, &page.people, now)
        .await
        .unwrap();
    jobs::merge_plan(
        &mut tx,
        id,
        &json!({"after":page.people.last(),"processed":BATCH}),
        now,
    )
    .await
    .unwrap();
    tx.commit().await.unwrap();
    let seq: i64 = sqlx::query_scalar(
        "SELECT inbox_seq FROM notifications WHERE user_id=$1 AND kind='doc.comment'",
    )
    .bind(first)
    .fetch_one(&mut *conn)
    .await
    .unwrap();
    let cancelled = document_notices::enqueue(&mut conn, &plan).await.unwrap();
    desk.world
        .app
        .call(JobCancel, &[&cancelled])
        .token(&desk.ada.token)
        .send()
        .await
        .ok();
    let listed = desk
        .world
        .app
        .call(DocumentNoticeJobList, &[&doc.base.id])
        .token(&desk.ada.token)
        .query(&orch_api_contract::common::PageQuery {
            limit: Some(1),
            cursor: None,
        })
        .send()
        .await
        .ok();
    assert_eq!(listed.items.len(), 1);
    assert!(listed.next_cursor.is_some());
    let next = desk
        .world
        .app
        .call(DocumentNoticeJobList, &[&doc.base.id])
        .token(&desk.ada.token)
        .query(&orch_api_contract::common::PageQuery {
            limit: Some(1),
            cursor: listed.next_cursor,
        })
        .send()
        .await
        .ok();
    assert_eq!(next.items.len(), 1);
    assert_ne!(listed.items[0].id, next.items[0].id);
    drop(conn);
    desk.world.app.app.engine.start();
    tokio::time::timeout(std::time::Duration::from_secs(25), async {
        loop {
            let state: String = sqlx::query_scalar("SELECT state FROM jobs WHERE id=$1")
                .bind(id)
                .fetch_one(&desk.world.db.pool)
                .await
                .unwrap();
            let cancelled_state: String = sqlx::query_scalar("SELECT state FROM jobs WHERE id=$1")
                .bind(cancelled)
                .fetch_one(&desk.world.db.pool)
                .await
                .unwrap();
            if state == "succeeded" && cancelled_state == "cancelled" {
                break;
            }
            assert_ne!(state, "failed");
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        }
    })
    .await
    .expect("the remaining bounded page finishes");
    let count: i64 =
        sqlx::query_scalar("SELECT count(*) FROM notifications WHERE kind='doc.comment'")
            .fetch_one(&desk.world.db.pool)
            .await
            .unwrap();
    assert_eq!(count, 205);
    let after: i64 = sqlx::query_scalar(
        "SELECT inbox_seq FROM notifications WHERE user_id=$1 AND kind='doc.comment'",
    )
    .bind(first)
    .fetch_one(&desk.world.db.pool)
    .await
    .unwrap();
    assert_eq!(
        after, seq,
        "a committed recipient is not renewed by recovery"
    );
    desk.world
        .app
        .call(JobGet, &[&id])
        .token(&desk.ada.token)
        .send()
        .await
        .ok();
    assert!(
        !desk
            .world
            .app
            .call(JobGet, &[&id])
            .token(&outsider.token)
            .send()
            .await
            .status
            .is_success()
    );
    let mut conn = desk.world.db.pool.acquire().await.unwrap();
    let newer = Plan {
        sequence: plan.sequence + 1,
        quote: "Newer reply".to_owned(),
        ..plan.clone()
    };
    let mut ordering = desk.world.db.pool.begin().await.unwrap();
    orch_db::queries::transaction_guard::lock_action(&mut ordering)
        .await
        .unwrap();
    document_notices::deliver(&mut ordering, &newer, &[first], now)
        .await
        .unwrap();
    document_notices::deliver(&mut ordering, &plan, &[first], now)
        .await
        .unwrap();
    ordering.commit().await.unwrap();
    let body: String = sqlx::query_scalar(
        "SELECT body FROM notifications WHERE user_id=$1 AND kind='doc.comment'",
    )
    .bind(first)
    .fetch_one(&mut *conn)
    .await
    .unwrap();
    assert_eq!(
        body, "Newer reply",
        "late older work cannot overwrite a later reply"
    );
}

#[tokio::test]
async fn mention_continuations_hold_one_page_and_keep_all_readers_before_nonreaders() {
    let desk = Desk::new().await;
    let mut body = desk::body("Private");
    body.access = Some(orch_api_contract::documents::NewDocumentAccess::Private);
    let doc = desk.make_body(&desk.ada, body).await.ok();
    let reader = desk.member("readable", "viewer").await;
    let waiting = desk.member("waiting", "viewer").await;
    desk.world
        .app
        .call(PermissionPageUpdate, &[&"documents", &doc.base.id])
        .token(&desk.ada.token)
        .body(&PageChange {
            name: Some(vec![PageChangeName {
                holders: vec![HolderInput::User { id: reader.id() }],
                choice: AccessChoice::FullView,
                export: Some(false),
                options: None,
                expires_at: None,
                note: None,
            }]),
            ..PageChange::default()
        })
        .send()
        .await
        .ok();
    let mut cursor = None;
    let mut seen = Vec::new();
    for _ in 0..8 {
        let page = desk
            .world
            .app
            .call(DocumentMentionList, &[&doc.base.id])
            .token(&desk.ada.token)
            .query(&ChatMentionListQuery {
                q: None,
                limit: Some(1),
                cursor,
            })
            .send()
            .await
            .ok();
        assert!(page.items.len() <= 1);
        seen.extend(
            page.items
                .into_iter()
                .map(|item| (item.user.id, item.can_view)),
        );
        cursor = page.next_cursor;
        if cursor.is_none() {
            break;
        }
    }
    assert!(cursor.is_none(), "the finite fixture is completely paged");
    assert!(seen.iter().any(|(id, view)| *id == waiting.id() && !*view));
    let first_nonreader = seen.iter().position(|(_, view)| !*view).unwrap();
    assert!(seen.iter().skip(first_nonreader).all(|(_, view)| !*view));
    assert!(seen.iter().any(|(id, view)| *id == reader.id() && *view));
    let mut conn = desk.world.db.pool.acquire().await.unwrap();
    let found = identity::mention_directory_page(
        &mut conn,
        desk.ada.id(),
        None,
        "%",
        2,
        desk.world.app.app.clock.now(),
    )
    .await
    .unwrap();
    assert!(found.is_empty(), "a wildcard in typed text is literal");
    assert!(
        documents::get(&mut conn, doc.base.id)
            .await
            .unwrap()
            .is_some()
    );
}

#[tokio::test]
async fn discussion_fanout_admission_never_exceeds_four_live_jobs_across_sweeps() {
    let desk = Desk::new().await;
    let doc = desk.make(&desk.ada, "Admission").await;
    let now = desk.world.app.app.clock.now();
    let plan = Plan {
        document: doc.base.id,
        discussion: new_id(),
        block: None,
        comment: None,
        by: desk.ada.id(),
        by_name: "Ada".to_owned(),
        quote: "Bounded".to_owned(),
        started: false,
        sequence: doc.base.seq,
        subscriptions: 0,
        at: now,
        owners: Default::default(),
    };
    let mut conn = desk.world.db.pool.acquire().await.unwrap();
    for _ in 0..7 {
        document_notices::enqueue(&mut conn, &plan).await.unwrap();
    }
    let mut first = desk.world.db.pool.begin().await.unwrap();
    let taken = jobs::claim_document_notices(&mut first, now, now + chrono::Duration::minutes(5))
        .await
        .unwrap();
    assert_eq!(taken.len(), 4);
    first.commit().await.unwrap();
    let mut second = desk.world.db.pool.begin().await.unwrap();
    assert!(
        jobs::claim_document_notices(&mut second, now, now + chrono::Duration::minutes(5))
            .await
            .unwrap()
            .is_empty()
    );
    second.commit().await.unwrap();
    let row = jobs::job(&mut conn, taken[0].0).await.unwrap().unwrap();
    jobs::finish(
        &mut conn,
        row.id,
        "succeeded",
        &row.progress,
        None,
        None,
        now,
    )
    .await
    .unwrap();
    let mut next = desk.world.db.pool.begin().await.unwrap();
    assert_eq!(
        jobs::claim_document_notices(&mut next, now, now + chrono::Duration::minutes(5))
            .await
            .unwrap()
            .len(),
        1
    );
    next.commit().await.unwrap();
    let later = now + chrono::Duration::minutes(6);
    let mut recovered = desk.world.db.pool.begin().await.unwrap();
    let renewed =
        jobs::claim_document_notices(&mut recovered, later, later + chrono::Duration::minutes(5))
            .await
            .unwrap();
    assert_eq!(
        renewed.len(),
        4,
        "expired leases resume through the same four slots"
    );
    assert_eq!(
        renewed
            .iter()
            .map(|(id, _)| *id)
            .collect::<std::collections::BTreeSet<_>>()
            .len(),
        4
    );
    recovered.commit().await.unwrap();
}
