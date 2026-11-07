//! Raw attachment admission precedes bytes, keeps native custody and never replays its supplier.
use super::*;

const OP: &str = "ChatAttachmentCreate";
fn write(chat: Id) -> Json {
    rule(OP, "chat", chat, "chat", "write")
}
async fn retained(room: &Room, user: Id, hash: Hash) -> bool {
    orch_db::queries::uploads::owns(&mut *room.world.conn().await, user, &hash)
        .await
        .unwrap()
}
async fn direct(room: &Room, token: &str, chat: Id, bytes: &[u8]) -> RawReply {
    let call = room.world.app.call(ChatAttachmentCreate, &[&chat]);
    call.token(token)
        .header(FILENAME_HEADER, "request.pdf")
        .bytes(bytes)
        .send_bytes()
        .await
}
async fn referenced(room: &Room, token: &str, chat: Id, hash: Hash) -> RawReply {
    let call = room.world.app.call(ChatAttachmentCreate, &[&chat]);
    call.token(token)
        .raw_body(&json!({"blob":hash,"name":"copied.pdf"}))
        .send_bytes()
        .await
}
fn accepted(reply: &RawReply) -> ChatAttachmentCreateReply {
    assert_eq!(
        reply.status,
        StatusCode::OK,
        "{}",
        String::from_utf8_lossy(&reply.raw)
    );
    serde_json::from_slice(&reply.raw).unwrap()
}

#[tokio::test]
async fn direct_attachment_uses_actual_chat_write_without_read_and_keeps_the_native_generated_reply()
 {
    let room = Room::new().await;
    let other = room.direct(&room.cai, &room.bea).await.base.id;
    for (index, rules) in [
        vec![rule(OP, "chat", room.chat, "chat", "read")],
        vec![write(other)],
        vec![rule(
            "ChatMessageCreate",
            "chat",
            room.chat,
            "chat",
            "write",
        )],
        vec![rule(OP, "chat", room.chat, "thread", "write")],
    ]
    .into_iter()
    .enumerate()
    {
        let token = key(&room, &room.bea, rules).await;
        let bytes = format!("never store refused attachment {index}").into_bytes();
        assert!(
            !direct(&room, &token, room.chat, &bytes)
                .await
                .status
                .is_success()
        );
        assert!(
            !room
                .world
                .app
                .app
                .blobs
                .store()
                .exists(&Hash::of(&bytes))
                .await
                .unwrap()
        );
        assert!(!retained(&room, room.bea.id(), Hash::of(&bytes)).await);
    }
    let bytes = pdf(64);
    let native = upload(&room, &room.bea, room.chat, "request.pdf", &bytes)
        .await
        .ok();
    let token = key(&room, &room.bea, vec![write(room.chat)]).await;
    let scoped = accepted(&direct(&room, &token, room.chat, &bytes).await);
    assert_eq!(json!(scoped), json!(native));
    assert!(retained(&room, room.bea.id(), scoped.blob).await);
    let outsider = key(&room, &room.eve, vec![write(room.chat)]).await;
    assert!(
        !direct(&room, &outsider, room.chat, b"not a member")
            .await
            .status
            .is_success()
    );
}

#[tokio::test]
async fn referenced_attachment_requires_its_own_operation_read_on_the_selected_native_source() {
    let room = Room::new().await;
    let file = upload(&room, &room.bea, room.chat, "source.pdf", &pdf(128))
        .await
        .ok();
    send_file(&room, &room.bea, room.chat, &file).await;
    let target = room.direct(&room.cai, &room.bea).await.base.id;
    assert!(!retained(&room, room.cai.id(), file.blob).await);
    for extra in [
        None,
        Some(rule("ChatBlobGet", "chat", room.chat, "chat", "read")),
        Some(rule(OP, "chat", target, "chat", "read")),
        Some(rule(OP, "chat", room.chat, "thread", "read")),
    ] {
        let mut rules = vec![write(target)];
        rules.extend(extra);
        let token = key(&room, &room.cai, rules).await;
        assert!(
            !referenced(&room, &token, target, file.blob)
                .await
                .status
                .is_success()
        );
        assert!(!retained(&room, room.cai.id(), file.blob).await);
    }
    let token = key(
        &room,
        &room.cai,
        vec![write(target), rule(OP, "chat", room.chat, "chat", "read")],
    )
    .await;
    let copied = accepted(&referenced(&room, &token, target, file.blob).await);
    assert_eq!(
        (copied.blob, copied.size, copied.media_type.as_str()),
        (file.blob, file.size, "application/pdf")
    );
    assert!(retained(&room, room.cai.id(), file.blob).await);

    // The original uploader's native custody is an Account source, never org membership.
    for extra in [
        rule(OP, "account", room.cai.id(), "upload", "read"),
        rule(OP, "org", room.root, "upload", "read"),
    ] {
        let token = key(&room, &room.bea, vec![write(target), extra]).await;
        assert!(
            !referenced(&room, &token, target, file.blob)
                .await
                .status
                .is_success()
        );
    }
    let own = key(
        &room,
        &room.bea,
        vec![
            write(target),
            rule(OP, "account", room.bea.id(), "upload", "read"),
        ],
    )
    .await;
    assert_eq!(
        accepted(&referenced(&room, &own, target, file.blob).await).blob,
        file.blob
    );
}

#[tokio::test]
async fn attachment_rechecks_the_actual_key_after_the_chat_parent_wait_before_custody() {
    for expire in [false, true] {
        let room = Room::new().await;
        let token = key(&room, &room.bea, vec![write(room.chat)]).await;
        let bytes = pdf(44);
        let hash = Hash::of(&bytes);
        let mut lock = room.world.db.pool.begin().await.unwrap();
        sqlx::query("SELECT id FROM chats WHERE id=$1 FOR UPDATE")
            .bind(room.chat)
            .fetch_one(&mut *lock)
            .await
            .unwrap();
        let pid: i32 = sqlx::query_scalar("SELECT pg_backend_pid()")
            .fetch_one(&mut *lock)
            .await
            .unwrap();
        let request = direct(&room, &token, room.chat, &bytes);
        let release = async {
            tokio::time::timeout(WAIT, async {
                loop {
                    let waiting: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM pg_stat_activity WHERE datname=current_database() AND wait_event_type='Lock' AND $1=ANY(pg_blocking_pids(pid)))")
                        .bind(pid).fetch_one(&room.world.db.pool).await.unwrap();
                    if waiting { break; }
                    tokio::task::yield_now().await;
                }
            }).await.expect("the actual parent fence is reached");
            assert!(!retained(&room, room.bea.id(), hash).await);
            if expire {
                room.world.advance(Duration::hours(2));
            }
            lock.commit().await.unwrap();
        };
        let (reply, ()) = tokio::time::timeout(WAIT, async { tokio::join!(request, release) })
            .await
            .unwrap();
        if expire {
            assert!(matches!(
                reply.status,
                StatusCode::UNAUTHORIZED | StatusCode::FORBIDDEN
            ));
            assert!(!retained(&room, room.bea.id(), hash).await);
        } else {
            assert_eq!(accepted(&reply).blob, hash);
        }
    }
}

#[tokio::test]
async fn attachment_supplier_wait_retains_original_key_and_selected_source_without_an_alternative()
{
    for change in ["none", "expire", "source"] {
        let mut room = Room::new().await;
        let file = upload(&room, &room.bea, room.chat, "old-source.pdf", &pdf(66))
            .await
            .ok();
        let original = send_file(&room, &room.bea, room.chat, &file).await;
        let target = room.direct(&room.cai, &room.bea).await.base.id;
        let alternative = room
            .group(
                &room.cai,
                &[&room.bea, &room.dee],
                Some("Alternative source"),
            )
            .await
            .base
            .id;
        let token = key(
            &room,
            &room.cai,
            vec![
                write(target),
                rule(OP, "chat", room.chat, "chat", "read"),
                rule(OP, "chat", alternative, "chat", "read"),
            ],
        )
        .await;
        let gate = gate_open(&mut room, file.blob);
        let request = referenced(&room, &token, target, file.blob);
        let alter = async {
            tokio::time::timeout(WAIT, gate.reached.notified())
                .await
                .expect("the once-only native sniff actually opens");
            assert!(!retained(&room, room.cai.id(), file.blob).await);
            if change == "expire" {
                room.world.advance(Duration::hours(2));
            }
            if change == "source" {
                send_file(&room, &room.bea, alternative, &file).await;
                room.world
                    .app
                    .call(ChatMessageDelete, &[&room.chat, &original.id])
                    .token(&room.bea.token)
                    .send()
                    .await
                    .ok();
            }
            gate.release.notify_one();
        };
        let (reply, ()) = tokio::time::timeout(WAIT, async { tokio::join!(request, alter) })
            .await
            .unwrap();
        if change == "none" {
            assert_eq!(accepted(&reply).blob, file.blob);
        } else {
            assert!(!reply.status.is_success());
            assert!(!retained(&room, room.cai.id(), file.blob).await);
        }
        if change != "source" {
            assert_eq!(
                gate.opens.load(Ordering::SeqCst),
                1,
                "guarded preparation never repeats the native supplier"
            );
        }
        if change == "source" {
            assert_eq!(
                accepted(&referenced(&room, &token, target, file.blob).await).blob,
                file.blob
            );
        }
    }
}

#[tokio::test]
async fn an_accepted_attachment_keeps_its_generated_receipt_after_late_scope_loss() {
    let room = Room::new().await;
    let token = key(&room, &room.bea, vec![write(room.chat)]).await;
    let bytes = pdf(25);
    let hash = Hash::of(&bytes);
    let mut lock = room.world.db.pool.begin().await.unwrap();
    sqlx::query("LOCK TABLE api_token_activity IN SHARE MODE")
        .execute(&mut *lock)
        .await
        .unwrap();
    let pid: i32 = sqlx::query_scalar("SELECT pg_backend_pid()")
        .fetch_one(&mut *lock)
        .await
        .unwrap();
    let request = direct(&room, &token, room.chat, &bytes);
    let alter = async {
        tokio::time::timeout(WAIT, async {
            loop {
                let waiting: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM pg_stat_activity WHERE datname=current_database() AND wait_event_type='Lock' AND $1=ANY(pg_blocking_pids(pid)) AND query LIKE '%INSERT INTO api_token_activity%')")
                    .bind(pid).fetch_one(&room.world.db.pool).await.unwrap();
                if waiting { break; }
                tokio::task::yield_now().await;
            }
        }).await.expect("the actual post-commit activity boundary");
        assert!(retained(&room, room.bea.id(), hash).await);
        sqlx::query("UPDATE api_tokens SET operation_scope=$2 WHERE token_hash=$1")
            .bind(token_hash(&token))
            .bind(json!({"rules":[]}))
            .execute(&room.world.db.pool)
            .await
            .unwrap();
        lock.commit().await.unwrap();
    };
    let (reply, ()) = tokio::time::timeout(WAIT, async { tokio::join!(request, alter) })
        .await
        .unwrap();
    assert_eq!(accepted(&reply).blob, hash);
    assert!(retained(&room, room.bea.id(), hash).await);
}
