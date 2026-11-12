//! Actual group Move consumes the completed-ending notice hook and native copied media.
use super::*;
use orch_api_contract::{
    chats::{ChatGet, ChatManagerSet, ChatManagerSetBody},
    duplicates::{
        ChatMoveOut, ChatMoveOutBody, ChatMoveOutPlan, ChatMoveOutPreview, MoveOutConfirmation,
        MoveOutDeleteEffect,
    },
};
async fn preview(
    r: &Room,
    source: Id,
    token: &str,
    copy: &DuplicateBody,
) -> Proof<ChatMoveOutPlan> {
    let reply = r
        .world
        .app
        .call(ChatMoveOutPreview, &[&source])
        .token(token)
        .body(copy)
        .send_bytes()
        .await;
    Ok(serde_json::from_value(ok(&reply)?)?)
}
fn body(copy: DuplicateBody, plan: ChatMoveOutPlan, notify: bool) -> ChatMoveOutBody {
    ChatMoveOutBody {
        copy,
        delete_plan: MoveOutConfirmation {
            revision: plan.delete_plan.revision,
            stamp: plan.delete_plan.stamp,
        },
        notify: Some(notify),
        notify_preview_id: plan.notification.preview_id,
    }
}
async fn apply(r: &Room, source: Id, token: &str, body: &ChatMoveOutBody) -> RawReply {
    r.world
        .app
        .call(ChatMoveOut, &[&source])
        .token(token)
        .body(body)
        .send_bytes()
        .await
}
async fn manager(r: &Room, source: Id, by: &places::Who, person: &places::Who, value: bool) {
    r.world
        .app
        .call(ChatManagerSet, &[&source, &person.id()])
        .token(&by.token)
        .body(&ChatManagerSetBody { manager: value })
        .send()
        .await
        .ok();
}
#[tokio::test]
async fn an_active_noncreator_manager_moves_all_native_pages_and_media_with_a_fresh_sole_roster()
-> Proof {
    let r = Room::new().await;
    let source = r
        .group(&r.ada, &[&r.bea, &r.cai], Some("Original movable group"))
        .await
        .base
        .id;
    manager(&r, source, &r.ada, &r.bea, true).await;
    manager(&r, source, &r.bea, &r.ada, false).await;
    for number in 0..43 {
        r.say(&r.ada, source, &format!("Original copied message {number}"))
            .await;
    }
    let file = r
        .world
        .app
        .call(ChatAttachmentCreate, &[&source])
        .token(&r.bea.token)
        .header(FILENAME_HEADER, "moved.txt")
        .bytes(b"Real moved group attachment\n")
        .send()
        .await
        .ok();
    r.send(
        &r.bea,
        source,
        vec![ChatPart::Attachment {
            blob: file.blob,
            name: file.name.clone(),
            size: file.size,
            media_type: file.media_type.clone(),
        }],
        &new_id().to_string(),
    )
    .await
    .ok();
    let target = places::make_org(&r.world, &r.bea, "Move target", "move-target")
        .await
        .base
        .id;
    let copy = DuplicateBody {
        destination: Some(DuplicateDestination::Node { id: target }),
        ..Default::default()
    };
    let plan = preview(&r, source, &r.bea.token, &copy).await?;
    assert_eq!(
        plan.delete_plan.effects,
        vec![MoveOutDeleteEffect::EndGroupAndHistory]
    );
    assert!(
        rows::chat(&mut *r.world.conn().await, source)
            .await?
            .is_some()
    );
    let actor=key(&r.world,r.bea.id(),vec![json!({"target":{"kind":"chat","id":source,"reach":"self"},"resources":["chat"],"allow":[{"operation":"ChatMoveOut","actions":["manage","read"]}]}),rule("ChatMoveOut","org",target,"self","write")]).await?;
    let moved = id(&apply(&r, source, &actor.secret, &body(copy, plan, false)).await)?;
    assert!(
        rows::chat(&mut *r.world.conn().await, source)
            .await?
            .is_none()
    );
    let row = rows::chat(&mut *r.world.conn().await, moved)
        .await?
        .ok_or("new native group missing")?;
    assert_eq!((row.created_by, row.org_id), (Some(r.bea.id()), target));
    let roster:Vec<(Id,bool)>=sqlx::query_as("SELECT user_id,manager FROM chat_members WHERE chat_id=$1 AND left_at IS NULL ORDER BY user_id").bind(moved).fetch_all(&r.world.db.pool).await?;
    assert_eq!(roster, vec![(r.bea.id(), true)]);
    let messages: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM chat_messages WHERE chat_id=$1 AND kind='message'",
    )
    .bind(moved)
    .fetch_one(&r.world.db.pool)
    .await?;
    assert_eq!(messages, 44);
    let bytes = r
        .world
        .app
        .call(ChatBlobGet, &[&moved, &file.blob])
        .token(&r.bea.token)
        .query(&ChatBlobGetQuery::default())
        .send_bytes()
        .await;
    assert_eq!(bytes.status.as_u16(), 200);
    assert_eq!(&bytes.raw[..], b"Real moved group attachment\n");
    for actor in [&r.ada, &r.cai] {
        assert!(
            !r.world
                .app
                .call(ChatGet, &[&moved])
                .token(&actor.token)
                .send_bytes()
                .await
                .status
                .is_success()
        );
    }
    let actions:Vec<String>=sqlx::query_scalar("SELECT action FROM audit_log WHERE object_id=$1 AND action IN ('content.copied_out','chat.ended') ORDER BY action").bind(source).fetch_all(&r.world.db.pool).await?;
    assert_eq!(actions, vec!["chat.ended", "content.copied_out"]);
    Ok(())
}
#[tokio::test]
async fn original_members_including_nonauthors_get_only_the_reviewed_move_receipt_when_notify_is_selected()
-> Proof {
    for notify in [false, true] {
        let r = Room::new().await;
        let source = r
            .group(&r.ada, &[&r.bea, &r.cai], Some("Ended original group"))
            .await
            .base
            .id;
        r.say(&r.bea, source, "Bea contributes").await;
        // Cai never posts, and still loses this source history when it ends.
        let target = places::make_org(
            &r.world,
            &r.ada,
            "Move notice recipient",
            "move-notice-recipient",
        )
        .await
        .base
        .id;
        let copy = DuplicateBody {
            destination: Some(DuplicateDestination::Node { id: target }),
            ..Default::default()
        };
        let plan = preview(&r, source, &r.ada.token, &copy).await?;
        assert_eq!(plan.notification.people, 2);
        assert!(plan.notification.preview_id.is_some());
        let moved = id(&apply(&r, source, &r.ada.token, &body(copy, plan, notify)).await)?;
        let notices:Vec<(Id,Option<Id>,Json)>=sqlx::query_as("SELECT user_id,chat_id,data FROM notifications WHERE kind='person.notify' AND data#>>'{completed_chat_move,source}'=$1 ORDER BY user_id").bind(source.to_string()).fetch_all(&r.world.db.pool).await?;
        assert_eq!(notices.len(), if notify { 2 } else { 0 });
        for (user, source_fk, data) in notices {
            assert!([r.bea.id(), r.cai.id()].contains(&user));
            assert_eq!(source_fk, None);
            assert_eq!(data["path"], json!(format!("/chat/{moved}")));
            assert_eq!(data["completed_chat_move"]["created"], json!(moved));
        }
        assert!(
            rows::chat(&mut *r.world.conn().await, source)
                .await?
                .is_none()
        );
        for actor in [&r.bea, &r.cai] {
            assert!(
                !r.world
                    .app
                    .call(ChatGet, &[&moved])
                    .token(&actor.token)
                    .send_bytes()
                    .await
                    .status
                    .is_success()
            );
        }
    }
    Ok(())
}
#[tokio::test]
async fn historical_creators_and_source_inspection_admins_are_not_active_manager_owners() -> Proof {
    let r = Room::new().await;
    let source = r
        .group(&r.ada, &[&r.bea, &r.cai], Some("Manager owned group"))
        .await
        .base
        .id;
    manager(&r, source, &r.ada, &r.bea, true).await;
    manager(&r, source, &r.bea, &r.ada, false).await;
    let target = places::make_org(
        &r.world,
        &r.ada,
        "Refused move target",
        "refused-move-target",
    )
    .await
    .base
    .id;
    for actor in [&r.ada, &r.dee, &r.cai] {
        if actor.id() == r.dee.id() {
            places::admin_mode(&r.world, actor, true).await;
        }
        let reply = r
            .world
            .app
            .call(ChatMoveOutPreview, &[&source])
            .token(&actor.token)
            .body(&DuplicateBody {
                destination: Some(DuplicateDestination::Node { id: target }),
                ..Default::default()
            })
            .send_bytes()
            .await;
        assert_eq!(reply.status.as_u16(), 403, "{:?}", reply);
    }
    assert!(
        rows::chat(&mut *r.world.conn().await, source)
            .await?
            .is_some()
    );
    Ok(())
}
#[tokio::test]
async fn changed_history_and_roster_refuse_the_whole_move_and_notify_audience_changes_need_new_review()
-> Proof {
    let r = Room::new().await;
    let source = r
        .group(&r.ada, &[&r.bea, &r.cai], Some("Reviewed group"))
        .await
        .base
        .id;
    let target = places::make_org(
        &r.world,
        &r.ada,
        "Reviewed destination",
        "reviewed-destination",
    )
    .await
    .base
    .id;
    let copy = DuplicateBody {
        destination: Some(DuplicateDestination::Node { id: target }),
        ..Default::default()
    };
    let old = preview(&r, source, &r.ada.token, &copy).await?;
    r.say(&r.bea, source, "Changed after original ending review")
        .await;
    let before = count(&r).await?;
    let stale = apply(&r, source, &r.ada.token, &body(copy.clone(), old, false)).await;
    assert_eq!(
        stale.problem().code,
        orch_api_contract::common::ErrorCode::ConflictStaleRevision
    );
    assert_eq!(count(&r).await?, before);
    let plan = preview(&r, source, &r.ada.token, &copy).await?;
    sqlx::query("UPDATE chat_members SET expires_at=$3 WHERE chat_id=$1 AND user_id=$2")
        .bind(source)
        .bind(r.cai.id())
        .bind(r.world.now())
        .execute(&r.world.db.pool)
        .await?;
    let stale = apply(&r, source, &r.ada.token, &body(copy, plan, true)).await;
    assert_eq!(
        stale.problem().code,
        orch_api_contract::common::ErrorCode::ConflictStaleRevision
    );
    assert_eq!(count(&r).await?, before);
    let notices: i64 =
        sqlx::query_scalar("SELECT count(*) FROM notifications WHERE kind='person.notify'")
            .fetch_one(&r.world.db.pool)
            .await?;
    assert_eq!(notices, 0);
    Ok(())
}
#[tokio::test]
async fn late_native_group_ending_failure_rolls_back_the_copy_notices_blob_custody_and_audits()
-> Proof {
    let r = Room::new().await;
    let source = r
        .group(&r.ada, &[&r.bea, &r.cai], Some("Held ending group"))
        .await
        .base
        .id;
    r.say(&r.bea, source, "Original history stays").await;
    let target = places::make_org(
        &r.world,
        &r.ada,
        "Held ending destination",
        "held-ending-destination",
    )
    .await
    .base
    .id;
    let copy = DuplicateBody {
        destination: Some(DuplicateDestination::Node { id: target }),
        ..Default::default()
    };
    let plan = preview(&r, source, &r.ada.token, &copy).await?;
    sqlx::query("CREATE FUNCTION hold_group_move_end() RETURNS trigger LANGUAGE plpgsql AS $$ BEGIN IF OLD.kind='group' THEN RAISE EXCEPTION 'held native group ending'; END IF; RETURN OLD; END $$").execute(&r.world.db.pool).await?;
    sqlx::query("CREATE TRIGGER hold_group_move_end BEFORE DELETE ON chats FOR EACH ROW EXECUTE FUNCTION hold_group_move_end()").execute(&r.world.db.pool).await?;
    let before = count(&r).await?;
    let failed = apply(&r, source, &r.ada.token, &body(copy, plan, false)).await;
    assert_eq!(failed.status.as_u16(), 500);
    assert_eq!(count(&r).await?, before);
    let messages: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM chat_messages WHERE chat_id=$1 AND kind='message'",
    )
    .bind(source)
    .fetch_one(&r.world.db.pool)
    .await?;
    assert_eq!(messages, 1);
    let audits:i64=sqlx::query_scalar("SELECT count(*) FROM audit_log WHERE object_id=$1 AND action IN ('chat.ended','content.copied_out')").bind(source).fetch_one(&r.world.db.pool).await?;
    assert_eq!(audits, 0);
    Ok(())
}
