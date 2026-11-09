//! The completed-ending capture's native database and standalone delivery controls.
use super::*;
use orch_access_contract::key_scope::{KeyAction, KeyFacet, KeyIdentityKind, KeyTargetKind};
use orch_api_contract::duplicates::{DuplicateKind, DuplicateReceipt};
use orch_authz::{
    Caller, CapturedOperation, ResolvedFacetRole,
    http_origin::{Credential, HttpOrigin},
};
use orch_db::queries::notifications::{operations::ending, people::NewNotification};
use orch_worker_core::person_notices::Capture;

struct Move {
    source: Id,
    receipt: DuplicateReceipt,
}

#[tokio::test]
async fn a_native_thread_move_without_notify_creates_no_capture_inbox_or_outbox() -> Proof {
    let r = Room::new().await;
    let before:(i64,i64,i64)=sqlx::query_as("SELECT (SELECT count(*) FROM notification_operations),(SELECT count(*) FROM notifications),(SELECT count(*) FROM outbox)").fetch_one(&r.world.db.pool).await?;
    for (notify, people) in [(false, vec![r.bea.id(), r.cai.id()]), (true, Vec::new())] {
        let db = orch_db::Db::from_pool(r.world.db.pool.clone());
        let mut tx = db.begin().await?;
        r.world
            .app
            .app
            .engine
            .tell_thread_move(
                &mut tx,
                &orch_engine::notifications::ThreadMoveNotice {
                    job: new_id(),
                    source: new_id(),
                    created: new_id(),
                    notify,
                    people: &people,
                },
            )
            .await?;
        tx.commit().await?;
    }
    let after:(i64,i64,i64)=sqlx::query_as("SELECT (SELECT count(*) FROM notification_operations),(SELECT count(*) FROM notifications),(SELECT count(*) FROM outbox)").fetch_one(&r.world.db.pool).await?;
    assert_eq!(before, after);
    Ok(())
}

async fn groups(r: &Room) -> Move {
    let source = r
        .group(&r.ada, &[&r.bea, &r.cai], Some("Moved original group"))
        .await
        .base
        .id;
    // Cai never writes a message: the ending receipt belongs to members, not just authors.
    let org = places::make_org(&r.world, &r.ada, "Destination", "destination").await;
    let created = r
        .world
        .app
        .call(OrgChatCreate, &[&org.base.id])
        .token(&r.ada.token)
        .body(&OrgChatCreateBody {
            visibility: None,
            home_id: None,
            user_ids: Vec::new(),
            name: Some("Destination group".into()),
        })
        .send()
        .await
        .ok()
        .into_full()
        .unwrap()
        .base
        .id;
    Move {
        source,
        receipt: DuplicateReceipt {
            projection: orch_api_contract::common::ReceiptProjection::Receipt,
            kind: DuplicateKind::Chat,
            id: created,
        },
    }
}

async fn accepted(r: &Room, conn: &mut orch_db::Conn, moved: &Move) -> Proof<(Caller, HttpOrigin)> {
    let source = orch_authz::key_targets::chat(conn, moved.source).await?;
    let destination = rows::chat(conn, moved.receipt.id)
        .await?
        .ok_or("the native destination group")?;
    let target = orch_authz::key_targets::node(conn, destination.home()).await?;
    // This is a source-hook test, independent of the route's separately owned move/Delete gate.
    // It passes genuine materialized facets through the existing admission/capture mechanism.
    let facets = [
        KeyFacet {
            operation: "ChatMoveOut".into(),
            action: KeyAction::Manage,
            resource_kind: KeyTargetKind::Identity(KeyIdentityKind::Chat),
            target: source,
        },
        KeyFacet {
            operation: "ChatMoveOut".into(),
            action: KeyAction::Write,
            resource_kind: KeyTargetKind::Identity(KeyIdentityKind::Chat),
            target,
        },
    ];
    let roles = facets
        .iter()
        .map(|facet| ResolvedFacetRole {
            action: facet.action,
            resources: vec![facet.resource_kind],
            facets: vec![facet.clone()],
        })
        .collect::<Vec<_>>();
    let caller = Caller::session(r.ada.id())
        .admit_resolved_operation("ChatMoveOut", &roles, &facets)?
        .into_caller();
    let hash = orch_api_core::middleware::authentication::token_hash(&r.ada.token);
    let id = sqlx::query_scalar("SELECT session_id FROM access_tokens WHERE token_hash=$1")
        .bind(&hash)
        .fetch_one(&mut *conn)
        .await?;
    let raw = HttpOrigin::capture(
        CapturedOperation::capture(&caller)?,
        Credential::Session { id, hash },
        false,
        false,
        None,
    )?;
    Ok((caller, raw))
}

async fn capture(r: &Room, conn: &mut orch_db::Conn, moved: &Move) -> Proof<(Id, Json)> {
    let (caller, raw) = accepted(r, conn, moved).await?;
    let act = new_id();
    let mut capture = Capture::prepare_chat_move(
        conn,
        r.world.app.app.clock.as_ref(),
        &caller,
        raw,
        moved.source,
        &moved.receipt,
        act,
    )
    .await?;
    let data = json!({"operation_id":act,"by":r.ada.id(),"subject":"Moved original group",
        "path":format!("/chat/{}",moved.receipt.id),"completed_chat_move":{"source":moved.source,"created":moved.receipt.id}});
    let key = format!("person.notify:{act}");
    for user in [r.bea.id(), r.cai.id()] {
        let notice = NewNotification {
            user_id: user,
            kind: "person.notify",
            node_id: None,
            thread_id: None,
            chat_id: None,
            approval_id: None,
            title: "Group moved",
            body: "",
            data: &data,
            dedupe_key: &key,
            at: r.world.now(),
        };
        let actual = orch_engine::notifications::tell_person_notice_before(
            conn,
            Some(r.root),
            &notice,
            None::<fn() -> Result<(), orch_engine::EngineError>>,
        )
        .await?
        .ok_or("the actual native Inbox identity")?;
        capture.record(&notice, &actual)?;
    }
    capture.save(conn).await?;
    Ok((act, data))
}

async fn inbox(r: &Room) -> Proof<Vec<(Id, Id, Json)>> {
    Ok(sqlx::query_as(
        "SELECT id,user_id,data FROM notifications WHERE kind='person.notify' ORDER BY user_id",
    )
    .fetch_all(&r.world.db.pool)
    .await?)
}

async fn readable_inbox(r: &Room, moved: &Move, who: &places::Who) -> Proof {
    use orch_api_contract::notifications::{
        NotificationGrouping, NotificationList, NotificationListQuery,
    };
    for grouping in [NotificationGrouping::Day, NotificationGrouping::Place] {
        let call = r
            .world
            .app
            .call(NotificationList, &[])
            .token(&who.token)
            .query(&NotificationListQuery {
                kind: Some("person.notify".into()),
                group_by: Some(grouping),
                ..Default::default()
            });
        let shown = call.send().await.ok();
        assert_eq!(shown.items.len(), 1);
        assert_eq!(shown.items[0].url, format!("/chat/{}", moved.receipt.id));
        assert_eq!(shown.items[0].chat_id, None);
        assert_eq!(shown.items[0].node, None);
    }
    Ok(())
}

#[tokio::test]
async fn accepted_group_move_keeps_each_members_inbox_and_delivers_once_after_source_ends() -> Proof
{
    let r = Room::new().await;
    let moved = groups(&r).await;
    let (worker, sink) = super::notices::worker(&r, r.bea.id()).await?;
    let (other_worker, other_sink) = super::notices::worker(&r, r.cai.id()).await?;
    let db = orch_db::Db::from_pool(r.world.db.pool.clone());
    let mut tx = db.begin().await?;
    let (_, data) = capture(&r, &mut tx, &moved).await?;
    assert!(
        !ending::chat_move_readable(&mut tx, r.bea.id(), "person.notify", &data).await?,
        "pending ending is not a completed receipt"
    );
    sqlx::query("DELETE FROM chats WHERE id=$1")
        .bind(moved.source)
        .execute(&mut *tx)
        .await?;
    tx.commit().await?;
    let before = inbox(&r).await?;
    assert_eq!(before.len(), 2);
    assert_eq!(
        before
            .iter()
            .map(|row| row.1)
            .collect::<std::collections::BTreeSet<_>>(),
        [r.bea.id(), r.cai.id()].into_iter().collect()
    );
    for (_, user, data) in &before {
        let mut conn = r.world.conn().await;
        assert!(ending::chat_move_readable(&mut conn, *user, "person.notify", data).await?);
        let principal = orch_authz::Principal::load(
            &mut conn,
            Caller::session(*user),
            orch_types::Time::new(r.world.now()),
        )
        .await?;
        assert!(
            orch_authz::documents::notice_readable(
                &mut conn,
                &principal,
                "person.notify",
                data,
                None,
                None
            )
            .await?
        );
        assert_eq!(data["path"], format!("/chat/{}", moved.receipt.id));
        assert!(
            !orch_worker_core::person_notices::reads(
                &mut conn,
                &principal,
                orch_worker_core::person_notices::Subject::Chat(moved.receipt.id)
            )
            .await?,
            "a moved receipt does not transfer membership"
        );
    }
    // A completed personal receipt uses no new sender authority after the ending.
    sqlx::query("UPDATE sessions SET revoked_at=$2 WHERE id=(SELECT session_id FROM access_tokens WHERE token_hash=$1)")
        .bind(orch_api_core::middleware::authentication::token_hash(&r.ada.token)).bind(r.world.now()).execute(&r.world.db.pool).await?;
    r.world.advance(chrono::Duration::minutes(5));
    worker.run().await?;
    other_worker.run().await?;
    worker.run().await?;
    other_worker.run().await?;
    assert_eq!(sink.0.lock().unwrap().len(), 1);
    assert_eq!(other_sink.0.lock().unwrap().len(), 1);
    readable_inbox(&r, &moved, &r.bea).await?;
    readable_inbox(&r, &moved, &r.cai).await?;
    assert_eq!(
        inbox(&r).await?,
        before,
        "hard ending and delivery keep the exact Inbox items"
    );
    Ok(())
}

#[tokio::test]
async fn a_group_ending_without_the_notify_choice_adds_no_inbox_or_delivery() -> Proof {
    let r = Room::new().await;
    let moved = groups(&r).await;
    let (worker, sink) = super::notices::worker(&r, r.bea.id()).await?;
    let db = orch_db::Db::from_pool(r.world.db.pool.clone());
    let mut tx = db.begin().await?;
    sqlx::query("DELETE FROM chats WHERE id=$1")
        .bind(moved.source)
        .execute(&mut *tx)
        .await?;
    tx.commit().await?;
    r.world.advance(chrono::Duration::minutes(5));
    worker.run().await?;
    assert!(inbox(&r).await?.is_empty());
    assert!(sink.0.lock().unwrap().is_empty());
    Ok(())
}

#[tokio::test]
async fn a_move_receipt_cannot_be_borrowed_by_another_recipient_or_changed_payload() -> Proof {
    let r = Room::new().await;
    let moved = groups(&r).await;
    let db = orch_db::Db::from_pool(r.world.db.pool.clone());
    let mut tx = db.begin().await?;
    let (_, mut data) = capture(&r, &mut tx, &moved).await?;
    sqlx::query("DELETE FROM chats WHERE id=$1")
        .bind(moved.source)
        .execute(&mut *tx)
        .await?;
    tx.commit().await?;
    let mut conn = r.world.conn().await;
    assert!(!ending::chat_move_readable(&mut conn, r.eve.id(), "person.notify", &data).await?);
    data["subject"] = json!("Borrowed content");
    assert!(!ending::chat_move_readable(&mut conn, r.bea.id(), "person.notify", &data).await?);
    Ok(())
}

#[tokio::test]
async fn a_move_notice_still_requires_its_live_original_identity_before_capture() -> Proof {
    let r = Room::new().await;
    let moved = groups(&r).await;
    let db = orch_db::Db::from_pool(r.world.db.pool.clone());
    let mut tx = db.begin().await?;
    let (caller, raw) = accepted(&r, &mut tx, &moved).await?;
    sqlx::query("UPDATE sessions SET revoked_at=$2 WHERE id=(SELECT session_id FROM access_tokens WHERE token_hash=$1)")
        .bind(orch_api_core::middleware::authentication::token_hash(&r.ada.token)).bind(r.world.now()).execute(&mut *tx).await?;
    assert!(
        Capture::prepare_chat_move(
            &mut tx,
            r.world.app.app.clock.as_ref(),
            &caller,
            raw,
            moved.source,
            &moved.receipt,
            new_id()
        )
        .await
        .is_err()
    );
    tx.rollback().await?;
    assert!(inbox(&r).await?.is_empty());
    assert!(
        rows::chat(&mut *r.world.conn().await, moved.source)
            .await?
            .is_some()
    );
    Ok(())
}
