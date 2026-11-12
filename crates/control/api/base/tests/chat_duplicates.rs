//! Native group conversation copying keeps its real source, fresh roster, projected names and retained media.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "tests fail by panicking"
)]
mod chat_world;
mod places;
mod support;
mod thread_history_scope;
use chat_world::Room;
use orch_api_contract::{
    chats::{
        ChatAttachmentCreate, ChatBlobGet, ChatBlobGetQuery, ChatPart, OrgChatCreate,
        OrgChatCreateBody,
    },
    duplicates::{ChatDuplicate, ChatDuplicateBody, DuplicateBody, DuplicateDestination},
    threads::FILENAME_HEADER,
};
use orch_api_core::testing::RawReply;
use orch_db::queries::chats as rows;
use orch_types::{Id, Json, new_id};
use serde_json::json;
use thread_history_scope::{Proof, key, ok, rule};
fn id(reply: &RawReply) -> Proof<Id> {
    let value = ok(reply)?;
    let id = serde_json::from_value(value["id"].clone())?;
    assert_eq!(value, json!({"projection":"receipt","kind":"chat","id":id}));
    Ok(id)
}
async fn copy(room: &Room, source: Id, token: &str, body: &DuplicateBody) -> RawReply {
    room.world
        .app
        .call(ChatDuplicate, &[&source])
        .token(token)
        .body(&ChatDuplicateBody {
            copy: body.clone(),
            ..Default::default()
        })
        .send_bytes()
        .await
}
async fn count(room: &Room) -> Proof<i64> {
    Ok(
        sqlx::query_scalar("SELECT count(*) FROM chats WHERE kind='group'")
            .fetch_one(&room.world.db.pool)
            .await?,
    )
}
#[tokio::test]
async fn group_copy_drains_every_native_page_and_copies_real_media_with_a_fresh_creator_only_roster()
-> Proof {
    let r = Room::new().await;
    let source = r
        .group(&r.ada, &[&r.bea, &r.cai], Some("Original group"))
        .await
        .base
        .id;
    for number in 0..43 {
        let by = if number % 2 == 0 { &r.ada } else { &r.cai };
        r.say(by, source, &format!("Original message {number}"))
            .await;
    }
    let file = r
        .world
        .app
        .call(ChatAttachmentCreate, &[&source])
        .token(&r.ada.token)
        .header(FILENAME_HEADER, "copied.txt")
        .bytes(b"Actual copied attachment\n")
        .send()
        .await
        .ok();
    r.send(
        &r.ada,
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
    let department = places::make_department(
        &r.world,
        &r.ada,
        r.root,
        "Copied group home",
        "copied-group-home",
    )
    .await
    .base
    .id;

    for home in [None, Some(department)] {
        let made = id(&copy(
            &r,
            source,
            &r.bea.token,
            &DuplicateBody {
                destination: home.map(|id| DuplicateDestination::Node { id }),
                ..Default::default()
            },
        )
        .await)?;
        let row = rows::chat(&mut *r.world.conn().await, made)
            .await?
            .ok_or("new chat missing")?;
        assert_eq!(row.kind, "group");
        assert_eq!(row.created_by, Some(r.bea.id()));
        assert_eq!(row.name.as_deref(), Some("Original group copy"));
        assert_eq!(row.home(), home.unwrap_or(r.root));
        let members:Vec<Id>=sqlx::query_scalar("SELECT user_id FROM chat_members WHERE chat_id=$1 AND left_at IS NULL ORDER BY user_id").bind(made).fetch_all(&r.world.db.pool).await?;
        assert_eq!(members, vec![r.bea.id()]);
        let owners:Vec<Id>=sqlx::query_scalar("SELECT holder_id FROM permission_entries WHERE object_kind='chat' AND object_id=$1 AND permission='access.own' AND removed_at IS NULL").bind(made).fetch_all(&r.world.db.pool).await?;
        assert_eq!(owners, vec![r.bea.id()]);
        let messages:Vec<Json>=sqlx::query_scalar("SELECT content FROM chat_messages WHERE chat_id=$1 AND kind='message' ORDER BY seq_created").bind(made).fetch_all(&r.world.db.pool).await?;
        assert_eq!(messages.len(), 44);
        let content = json!(messages).to_string();
        assert!(content.contains("Original message 0"));
        assert!(content.contains("Original message 42"));
        assert!(content.contains(&file.blob.to_string()));
        let bytes = r
            .world
            .app
            .call(ChatBlobGet, &[&made, &file.blob])
            .token(&r.bea.token)
            .query(&ChatBlobGetQuery::default())
            .send_bytes()
            .await;
        assert_eq!(bytes.status.as_u16(), 200);
        assert_eq!(&bytes.raw[..], b"Actual copied attachment\n");
        assert!(
            !r.world
                .app
                .call(orch_api_contract::chats::ChatGet, &[&made])
                .token(&r.cai.token)
                .send_bytes()
                .await
                .status
                .is_success(),
            "the source roster cannot read the private new copy"
        );
    }
    assert_eq!(count(&r).await?, 3);
    Ok(())
}
#[tokio::test]
async fn group_copy_requires_its_own_complete_source_and_destination_and_never_converts_direct_or_node_chats()
-> Proof {
    let r = Room::new().await;
    let source = r
        .group(&r.ada, &[&r.bea], Some("Scoped group"))
        .await
        .base
        .id;
    r.say(&r.ada, source, "Copied current content").await;
    let read = || rule("ChatDuplicate", "chat", source, "self", "read");
    let write = || rule("ChatDuplicate", "org", r.root, "self", "write");
    for rules in [
        vec![read()],
        vec![write()],
        vec![
            rule("ForwardCreate", "chat", source, "self", "read"),
            write(),
        ],
    ] {
        let actor = key(&r.world, r.ada.id(), rules).await?;
        assert_eq!(
            copy(&r, source, &actor.secret, &DuplicateBody::default())
                .await
                .status
                .as_u16(),
            403
        );
    }
    let actor = key(&r.world, r.bea.id(), vec![read(), write()]).await?;
    let made = id(&copy(&r, source, &actor.secret, &DuplicateBody::default()).await)?;
    assert_ne!(made, source);
    let outsider = key(&r.world, r.gus.id(), vec![read(), write()]).await?;
    assert!(
        !copy(&r, source, &outsider.secret, &DuplicateBody::default())
            .await
            .status
            .is_success()
    );
    let direct = r.direct(&r.ada, &r.bea).await.base.id;
    assert!(
        !copy(&r, direct, &r.ada.token, &DuplicateBody::default())
            .await
            .status
            .is_success()
    );
    assert!(
        !copy(&r, r.chat, &r.ada.token, &DuplicateBody::default())
            .await
            .status
            .is_success()
    );
    let stale = copy(
        &r,
        source,
        &r.ada.token,
        &DuplicateBody {
            expected_revision: Some(0),
            ..Default::default()
        },
    )
    .await;
    assert_eq!(
        stale.problem().code,
        orch_api_contract::common::ErrorCode::ConflictStaleRevision
    );
    // Ordinary opening still requires another person; the native source-qualified copy is the sole exception.
    assert!(
        !r.world
            .app
            .call(OrgChatCreate, &[&r.root])
            .token(&r.ada.token)
            .body(&OrgChatCreateBody {
                visibility: None,
                home_id: None,
                user_ids: vec![],
                name: Some("No source".into())
            })
            .send_bytes()
            .await
            .status
            .is_success()
    );
    assert_eq!(count(&r).await?, 2);
    Ok(())
}
#[tokio::test]
async fn group_copy_keeps_one_accepted_identity_on_retry_and_expiry_closes_the_real_source_row_wait()
-> Proof {
    let r = Room::new().await;
    let source = r
        .group(&r.ada, &[&r.bea], Some("Stable group"))
        .await
        .base
        .id;
    r.say(&r.ada, source, "Stable content").await;
    let retry = new_id().to_string();
    let body = DuplicateBody::default();
    let first = r
        .world
        .app
        .call(ChatDuplicate, &[&source])
        .token(&r.ada.token)
        .header("Idempotency-Key", &retry)
        .body(&ChatDuplicateBody {
            copy: body.clone(),
            ..Default::default()
        })
        .send_bytes()
        .await;
    let made = id(&first)?;
    let reader = key(
        &r.world,
        r.ada.id(),
        vec![rule("ChatGet", "chat", source, "self", "read")],
    )
    .await?;
    let again = r
        .world
        .app
        .call(ChatDuplicate, &[&source])
        .token(&reader.secret)
        .header("Idempotency-Key", &retry)
        .body(&ChatDuplicateBody {
            copy: body.clone(),
            ..Default::default()
        })
        .send_bytes()
        .await;
    assert_eq!(id(&again)?, made);
    assert_eq!(count(&r).await?, 2);
    for expire in [false, true] {
        let actor = key(
            &r.world,
            r.ada.id(),
            vec![
                rule("ChatDuplicate", "chat", source, "self", "read"),
                rule("ChatDuplicate", "org", r.root, "self", "write"),
            ],
        )
        .await?;
        let mut blocker = r.world.db.pool.begin().await?;
        rows::lock(&mut blocker, source)
            .await?
            .ok_or("source lock missing")?;
        let pid: i32 = sqlx::query_scalar("SELECT pg_backend_pid()")
            .fetch_one(&mut *blocker)
            .await?;
        let app = r.world.app.clone();
        let token = actor.secret.clone();
        let request = tokio::spawn(async move {
            let request = app
                .call(ChatDuplicate, &[&source])
                .token(&token)
                .body(&ChatDuplicateBody::default());
            request.send_bytes().await
        });
        tokio::time::timeout(std::time::Duration::from_secs(30),async{loop{assert!(!request.is_finished());let waiting:bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM pg_stat_activity WHERE datname=current_database() AND wait_event_type='Lock' AND $1=ANY(pg_blocking_pids(pid)))").bind(pid).fetch_one(&r.world.db.pool).await.unwrap();if waiting{break;}tokio::task::yield_now().await;}}).await?;
        let before = count(&r).await?;
        if expire {
            r.world.advance(chrono::Duration::hours(2));
        }
        blocker.commit().await?;
        let reply = tokio::time::timeout(std::time::Duration::from_secs(30), request).await??;
        if expire {
            assert!(!reply.status.is_success());
            assert_eq!(count(&r).await?, before);
        } else {
            id(&reply)?;
            assert_eq!(count(&r).await?, before + 1);
        }
    }
    Ok(())
}

#[tokio::test]
async fn copying_a_group_can_notify_exactly_the_reviewed_original_contributors_without_regranting_membership()
-> Proof {
    use orch_api_contract::duplicates::ChatDuplicatePreview;
    let r = Room::new().await;
    let source = r
        .group(&r.ada, &[&r.bea, &r.cai], Some("Original contributors"))
        .await
        .base
        .id;
    for (by, text) in [
        (&r.ada, "Ada first"),
        (&r.ada, "Ada again"),
        (&r.cai, "Cai work"),
        (&r.bea, "Copier own work"),
    ] {
        r.say(by, source, text).await;
    }
    let preview = r
        .world
        .app
        .call(ChatDuplicatePreview, &[&source])
        .token(&r.bea.token)
        .send()
        .await
        .ok();
    assert_eq!(preview.people, 2);
    assert!(preview.preview_id.is_some());
    let retry = new_id().to_string();
    let body = ChatDuplicateBody {
        copy: DuplicateBody::default(),
        notify: Some(true),
        notify_preview_id: preview.preview_id,
    };
    let first = r
        .world
        .app
        .call(ChatDuplicate, &[&source])
        .token(&r.bea.token)
        .header("Idempotency-Key", &retry)
        .body(&body)
        .send_bytes()
        .await;
    let made = id(&first)?;
    let notices:Vec<(Id,Json)>=sqlx::query_as("SELECT user_id,data FROM notifications WHERE chat_id=$1 AND kind='person.notify' ORDER BY user_id").bind(source).fetch_all(&r.world.db.pool).await?;
    let mut people = vec![r.ada.id(), r.cai.id()];
    people.sort_unstable();
    assert_eq!(notices.iter().map(|row| row.0).collect::<Vec<_>>(), people);
    for (_, data) in &notices {
        assert!(data["operation_id"].is_string());
        assert!(!data.to_string().contains(&made.to_string()));
        assert!(data.get("raw").is_none());
    }
    let records:Vec<Json>=sqlx::query_scalar("SELECT o.data FROM notification_operations o WHERE id IN(SELECT (data->>'operation_id')::uuid FROM notifications WHERE chat_id=$1 AND kind='person.notify')").bind(source).fetch_all(&r.world.db.pool).await?;
    assert_eq!(records.len(), 1);
    assert_eq!(records[0]["origin"]["operation"], "ChatDuplicate");
    assert_eq!(records[0]["raw"]["origin"]["operation"], "ChatDuplicate");
    assert_eq!(records[0]["raw"]["credential"]["kind"], "session");
    assert_eq!(records[0]["accepted_raw"], records[0]["raw"]);
    let again = r
        .world
        .app
        .call(ChatDuplicate, &[&source])
        .token(&r.bea.token)
        .header("Idempotency-Key", &retry)
        .body(&body)
        .send_bytes()
        .await;
    assert_eq!(id(&again)?, made);
    let count: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM notifications WHERE chat_id=$1 AND kind='person.notify'",
    )
    .bind(source)
    .fetch_one(&r.world.db.pool)
    .await?;
    assert_eq!(count, 2);
    assert!(
        !r.world
            .app
            .call(orch_api_contract::chats::ChatGet, &[&made])
            .token(&r.cai.token)
            .send_bytes()
            .await
            .status
            .is_success()
    );
    Ok(())
}
#[tokio::test]
async fn contributor_previews_keep_opt_out_and_current_recipient_changes_and_an_explicit_no_notice_copy()
-> Proof {
    use orch_api_contract::duplicates::ChatDuplicatePreview;
    let r = Room::new().await;
    let source = r
        .group(&r.ada, &[&r.bea, &r.cai], Some("Reviewed audience"))
        .await
        .base
        .id;
    r.say(&r.ada, source, "First contributor").await;
    let reviewed = r
        .world
        .app
        .call(ChatDuplicatePreview, &[&source])
        .token(&r.bea.token)
        .send()
        .await
        .ok();
    assert_eq!(reviewed.people, 1);
    r.say(&r.cai, source, "A later contributor").await;
    let stale = r
        .world
        .app
        .call(ChatDuplicate, &[&source])
        .token(&r.bea.token)
        .body(&ChatDuplicateBody {
            copy: DuplicateBody::default(),
            notify: Some(true),
            notify_preview_id: reviewed.preview_id,
        })
        .send_bytes()
        .await;
    assert_eq!(
        stale.problem().code,
        orch_api_contract::common::ErrorCode::ConflictStaleRevision
    );
    assert_eq!(count(&r).await?, 1);
    let current = r
        .world
        .app
        .call(ChatDuplicatePreview, &[&source])
        .token(&r.bea.token)
        .send()
        .await
        .ok();
    assert_eq!(current.people, 2);
    sqlx::query("INSERT INTO notification_prefs(user_id,disabled_kinds) VALUES($1,'[\"person.notify\"]') ON CONFLICT(user_id) DO UPDATE SET disabled_kinds=EXCLUDED.disabled_kinds").bind(r.cai.id()).execute(&r.world.db.pool).await?;
    let opted = r
        .world
        .app
        .call(ChatDuplicatePreview, &[&source])
        .token(&r.bea.token)
        .send()
        .await
        .ok();
    assert_eq!(opted.people, 1);
    let silent = r
        .world
        .app
        .call(ChatDuplicate, &[&source])
        .token(&r.bea.token)
        .body(&ChatDuplicateBody {
            copy: DuplicateBody::default(),
            notify: Some(false),
            notify_preview_id: None,
        })
        .send_bytes()
        .await;
    id(&silent)?;
    let notices: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM notifications WHERE chat_id=$1 AND kind='person.notify'",
    )
    .bind(source)
    .fetch_one(&r.world.db.pool)
    .await?;
    assert_eq!(notices, 0);
    Ok(())
}

#[path = "chat_duplicates/move_notices.rs"]
mod move_notices;
#[path = "chat_duplicates/moving_routes.rs"]
mod moving_routes;
#[path = "chat_duplicates/notices.rs"]
mod notices;

#[tokio::test]
async fn a_contributor_preview_reads_and_publishes_only_its_own_actual_chat_source() -> Proof {
    use orch_api_contract::duplicates::ChatDuplicatePreview;
    let r = Room::new().await;
    let source = r
        .group(&r.ada, &[&r.bea], Some("Scoped reviewed source"))
        .await
        .base
        .id;
    r.say(&r.ada, source, "A real contributor").await;
    for (operation, action, ok) in [
        ("ChatDuplicatePreview", "read", true),
        ("ChatGet", "read", false),
        ("ChatDuplicatePreview", "write", false),
    ] {
        let token = key(
            &r.world,
            r.bea.id(),
            vec![rule(operation, "chat", source, "self", action)],
        )
        .await?;
        let reply = r
            .world
            .app
            .call(ChatDuplicatePreview, &[&source])
            .token(&token.secret)
            .send()
            .await;
        if ok {
            let value = reply.ok();
            assert_eq!(value.people, 1);
            assert!(value.preview_id.is_some());
        } else {
            assert_eq!(reply.status.as_u16(), 403);
        }
    }
    Ok(())
}

#[tokio::test]
async fn move_destination_ownership_follows_active_managers_and_never_creator_attribution() -> Proof
{
    use orch_api_contract::{
        chats::{ChatManagerSet, ChatManagerSetBody},
        duplicates::{
            CopyDestinationCause, CopyDestinationOutcome, CopyDestinationsJudge,
            CopyDestinationsRequest, CopyIntent, CopySubject, CopySubjectKind,
        },
    };
    let r = Room::new().await;
    let source = r
        .group(&r.ada, &[&r.bea, &r.cai], Some("Move ownership"))
        .await
        .base
        .id;
    let destination = places::make_org(&r.world, &r.ada, "Move recipient", "move-recipient")
        .await
        .base
        .id;
    r.world
        .app
        .call(ChatManagerSet, &[&source, &r.bea.id()])
        .token(&r.ada.token)
        .body(&ChatManagerSetBody { manager: true })
        .send()
        .await
        .ok();
    r.world
        .app
        .call(ChatManagerSet, &[&source, &r.ada.id()])
        .token(&r.bea.token)
        .body(&ChatManagerSetBody { manager: false })
        .send()
        .await
        .ok();
    let stored = rows::chat(&mut *r.world.conn().await, source)
        .await?
        .ok_or("missing actual group")?;
    assert_eq!(stored.created_by, Some(r.ada.id()));
    assert!(
        !rows::is_manager(
            &mut *r.world.conn().await,
            source,
            r.ada.id(),
            r.world.now()
        )
        .await?
    );
    assert!(
        rows::is_manager(
            &mut *r.world.conn().await,
            source,
            r.bea.id(),
            r.world.now()
        )
        .await?
    );
    let request = CopyDestinationsRequest {
        subject: CopySubject {
            kind: CopySubjectKind::Chat,
            id: source,
        },
        intent: CopyIntent::Move,
        destinations: vec![DuplicateDestination::Node { id: destination }],
    };
    for person in [&r.bea, &r.cai] {
        places::join(&r.world, destination, destination, person, "viewer", &r.ada).await;
    }
    for token in [&r.ada.token, &r.cai.token] {
        let reply = r
            .world
            .app
            .call(CopyDestinationsJudge, &[])
            .token(token)
            .body(&request)
            .send()
            .await
            .ok();
        assert!(matches!(
            reply.destinations[0].outcome,
            CopyDestinationOutcome::Blocked {
                cause: CopyDestinationCause::OwnersOnly,
                ..
            }
        ));
    }
    let active = r
        .world
        .app
        .call(CopyDestinationsJudge, &[])
        .token(&r.bea.token)
        .body(&request)
        .send()
        .await
        .ok();
    assert!(active.source_owned);
    let creator = r
        .world
        .app
        .call(CopyDestinationsJudge, &[])
        .token(&r.ada.token)
        .body(&request)
        .send()
        .await
        .ok();
    assert!(!creator.source_owned);
    sqlx::query("UPDATE chat_members SET expires_at=$3 WHERE chat_id=$1 AND user_id=$2")
        .bind(source)
        .bind(r.bea.id())
        .bind(r.world.now() - chrono::Duration::seconds(1))
        .execute(&r.world.db.pool)
        .await?;
    let expired = r
        .world
        .app
        .call(CopyDestinationsJudge, &[])
        .token(&r.bea.token)
        .body(&request)
        .send_bytes()
        .await;
    assert_eq!(
        expired.status.as_u16(),
        404,
        "an expired manager cannot read or move the private original"
    );
    Ok(())
}

#[tokio::test]
async fn cross_org_group_copy_keeps_messages_but_recreates_its_roster_and_source_audit() -> Proof {
    let r = Room::new().await;
    let source = r
        .group(&r.ada, &[&r.bea], Some("Governed group"))
        .await
        .base
        .id;
    r.say(&r.bea, source, "Actual message taken into a fresh group")
        .await;
    let destination = places::make_org(&r.world, &r.ada, "Chat recipient", "chat-recipient")
        .await
        .base
        .id;
    let token = key(
        &r.world,
        r.ada.id(),
        vec![
            rule("ChatDuplicate", "chat", source, "self", "read"),
            rule("ChatDuplicate", "org", destination, "self", "write"),
        ],
    )
    .await?;
    let made = id(&copy(
        &r,
        source,
        &token.secret,
        &DuplicateBody {
            destination: Some(DuplicateDestination::Node { id: destination }),
            ..Default::default()
        },
    )
    .await)?;
    let row = rows::chat(&mut *r.world.conn().await, made)
        .await?
        .ok_or("missing copied group")?;
    assert_eq!(
        (row.org_id, row.home(), row.created_by),
        (destination, destination, Some(r.ada.id()))
    );
    let members = rows::members(&mut *r.world.conn().await, made).await?;
    assert_eq!(members.len(), 1);
    assert_eq!(members[0].user_id, r.ada.id());
    assert!(members[0].manager);
    let content: Vec<Json> =
        sqlx::query_scalar("SELECT content FROM chat_messages WHERE chat_id=$1 AND kind='message'")
            .bind(made)
            .fetch_all(&r.world.db.pool)
            .await?;
    assert_eq!(content.len(), 1);
    assert!(
        json!(content)
            .to_string()
            .contains("Actual message taken into a fresh group")
    );
    let audit: (Option<Id>, Json) = sqlx::query_as(
        "SELECT org_id,details FROM audit_log WHERE action='content.copied_out' AND object_id=$1",
    )
    .bind(source)
    .fetch_one(&r.world.db.pool)
    .await?;
    assert_eq!(audit.0, Some(r.root));
    assert_eq!(audit.1["destination_org_id"], json!(destination));
    assert_eq!(audit.1["created_id"], json!(made));
    Ok(())
}
