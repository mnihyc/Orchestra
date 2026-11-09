//! Message edits keep their native no-op/CAS rules and require only actual consumed byte sources.
//! A key-scoped quote Thread Read/no-Thread-Write case awaits the shared chats_threads fixture;
//! the existing native quote and lock-order controls remain there.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]
mod chat_world;
mod places;
mod support;

use chat_world::{Room, text};
use orch_api_contract::access::Role;
use orch_api_contract::chats::{
    ChatMentionTarget, ChatMessageDelete, ChatMessageUpdate, ChatMessageUpdateBody, ChatPart,
};
use orch_api_contract::directory::{GroupMemberAdd, OrgGroupCreate, OrgGroupCreateBody};
use orch_db::Conn;
use orch_types::{Hash, Id, new_id};
use serde_json::{Value, json};

const OP: &str = "ChatMessageUpdate";
fn rule(op: &str, kind: &str, id: Id, actions: &[&str]) -> Value {
    json!({"target":{"kind":kind,"id":id,"reach":"self"},"allow":[{"operation":op,"actions":actions}]})
}
async fn key(room: &Room, user: Id, rules: Option<Vec<Value>>, life: chrono::Duration) -> String {
    use orch_access_contract::key_scope::KeyScope;
    use orch_api_core::middleware::authentication::{API_TOKEN_PREFIX, new_token, token_hash};
    use orch_db::queries::identity::{NewApiToken, create_api_token_scoped};
    let scope = rules.map(|rules| {
        let scope: KeyScope = serde_json::from_value(json!({"rules":rules})).unwrap();
        assert!(scope.validate().is_empty());
        serde_json::to_value(scope).unwrap()
    });
    let legacy = vec!["*".to_owned()];
    let secret = new_token(API_TOKEN_PREFIX);
    create_api_token_scoped(
        &mut *room.world.conn().await,
        &NewApiToken {
            id: new_id(),
            user_id: user,
            name: "Message edit scope control",
            token_hash: &token_hash(&secret),
            scopes: if scope.is_some() { &[] } else { &legacy },
            node_id: None,
            at: room.world.now(),
            expires_at: Some(room.world.now() + life),
        },
        scope.as_ref(),
    )
    .await
    .unwrap();
    secret
}
async fn scoped(room: &Room, user: Id, rules: Vec<Value>) -> String {
    // Finite requests run on the fixed fixture clock within the established one-hour key life.
    key(room, user, Some(rules), chrono::Duration::hours(1)).await
}
fn result(reply: orch_api_core::testing::RawReply) -> (u16, Value) {
    (
        reply.status.as_u16(),
        serde_json::from_slice(&reply.raw).unwrap(),
    )
}
async fn edit(room: &Room, token: &str, chat: Id, message: Id, parts: &[ChatPart]) -> (u16, Value) {
    result(
        room.world
            .app
            .call(ChatMessageUpdate, &[&chat, &message])
            .token(token)
            .body(&ChatMessageUpdateBody {
                parts: parts.to_vec(),
            })
            .send_bytes()
            .await,
    )
}
fn full(reply: (u16, Value), message: Id) -> Value {
    assert_eq!(reply.0, 200, "{}", reply.1);
    assert_eq!(
        reply.1["id"],
        json!(message),
        "ordinary wire stays a bare ChatMessage"
    );
    assert!(reply.1.get("parts").is_some());
    assert!(reply.1.get("projection").is_none());
    reply.1
}
fn receipt(reply: (u16, Value), chat: Id, message: Id) {
    assert_eq!(reply.0, 200, "{}", reply.1);
    let seq = reply.1["seq"].as_i64().unwrap();
    assert_eq!(
        reply.1,
        json!({"projection":"receipt","chat_id":chat,"message_id":message,"seq":seq})
    );
}
fn source_refused(reply: (u16, Value)) {
    assert!(
        matches!(reply.0, 403 | 404),
        "source authority must be refused before mutation: {}",
        reply.1
    );
}
fn attachment(hash: Hash, size: i64) -> ChatPart {
    ChatPart::Attachment {
        blob: hash,
        name: "kept.txt".into(),
        size,
        media_type: "text/plain".into(),
    }
}
async fn posted(room: &Room, by: &places::Who, chat: Id, parts: Vec<ChatPart>) -> Id {
    room.send(by, chat, parts, &new_id().to_string())
        .await
        .ok()
        .message
        .id
}
async fn stored(room: &Room, chat: Id, message: Id) -> orch_db::queries::chats::MessageRow {
    orch_db::queries::chats::message(&mut *room.world.conn().await, chat, message)
        .await
        .unwrap()
        .unwrap()
}
async fn snapshot_conn(conn: &mut Conn, chat: Id, hashes: &[Hash]) -> Value {
    let hashes: Vec<Vec<u8>> = hashes.iter().map(|hash| hash.as_bytes().to_vec()).collect();
    // A Room has no native fixture or background runner. Capture actual finite rows,
    // not inferred counters, so a refused/no-op edit cannot quietly mutate other work.
    sqlx::query_scalar("SELECT jsonb_build_object(
        'chat',(SELECT to_jsonb(c) FROM chats c WHERE id=$1),
        'messages',COALESCE((SELECT jsonb_agg(to_jsonb(m) ORDER BY m.id) FROM chat_messages m WHERE chat_id=$1),'[]'::jsonb),
        'blobs',COALESCE((SELECT jsonb_agg(to_jsonb(b) ORDER BY b.hash) FROM blobs b WHERE hash=ANY($2::bytea[])),'[]'::jsonb),
        'uploads',COALESCE((SELECT jsonb_agg(to_jsonb(u) ORDER BY to_jsonb(u)::text) FROM completed_uploads u WHERE hash=ANY($2::bytea[])),'[]'::jsonb),
        'quotes',COALESCE((SELECT jsonb_agg(to_jsonb(r) ORDER BY r.message_id,r.item_id) FROM chat_item_refs r JOIN chat_messages m ON m.id=r.message_id WHERE m.chat_id=$1),'[]'::jsonb),
        'notifications',COALESCE((SELECT jsonb_agg(to_jsonb(n) ORDER BY to_jsonb(n)::text) FROM notifications n),'[]'::jsonb),
        'inputs',COALESCE((SELECT jsonb_agg(to_jsonb(i) ORDER BY to_jsonb(i)::text) FROM thread_inputs i),'[]'::jsonb),
        'jobs',COALESCE((SELECT jsonb_agg(to_jsonb(j) ORDER BY to_jsonb(j)::text) FROM jobs j),'[]'::jsonb),
        'commands',COALESCE((SELECT jsonb_agg(to_jsonb(c) ORDER BY to_jsonb(c)::text) FROM commands c),'[]'::jsonb),
        'audit',(SELECT count(*) FROM audit_log WHERE object_id=$1))")
        .bind(chat).bind(hashes).fetch_one(conn).await.unwrap()
}
async fn snapshot(room: &Room, chat: Id, hashes: &[Hash]) -> Value {
    snapshot_conn(&mut *room.world.conn().await, chat, hashes).await
}
fn no_dispatch(before: &Value, after: &Value) {
    for field in ["inputs", "jobs", "commands"] {
        assert_eq!(
            after[field], before[field],
            "edits do not dispatch agent work: {field}"
        );
    }
}

#[tokio::test]
async fn a_plain_text_edit_keeps_the_native_writer_and_returns_only_a_write_receipt() {
    let room = Room::new().await;
    let message = posted(&room, &room.bea, room.chat, vec![text("Before")]).await;
    let native = full(
        edit(
            &room,
            &room.bea.token,
            room.chat,
            message,
            &[text("Native prime")],
        )
        .await,
        message,
    );
    assert_eq!(native["parts"], json!([text("Native prime")]));
    let token = scoped(
        &room,
        room.bea.id(),
        vec![rule(OP, "chat", room.chat, &["write"])],
    )
    .await;
    let before = snapshot(&room, room.chat, &[]).await;
    receipt(
        edit(&room, &token, room.chat, message, &[text("Accepted text")]).await,
        room.chat,
        message,
    );
    let saved = stored(&room, room.chat, message).await;
    assert_eq!(saved.content, json!([text("Accepted text")]));
    assert!(saved.seq_updated > native["seq_updated"].as_i64().unwrap());
    let after = snapshot(&room, room.chat, &[]).await;
    no_dispatch(&before, &after);
    assert_eq!(after["blobs"], before["blobs"]);
}

#[tokio::test]
async fn edit_response_read_and_live_group_role_names_belong_to_this_same_operation() {
    let room = Room::new().await;
    let group = room
        .world
        .app
        .call(OrgGroupCreate, &[&room.root])
        .token(&room.ada.token)
        .body(&OrgGroupCreateBody {
            name: "Protected edit crew".into(),
            description: None,
        })
        .send()
        .await
        .ok()
        .into_full()
        .expect("this ordinary control requires the full native creation response");
    room.world
        .app
        .call(GroupMemberAdd, &[&group.id, &room.cai.id()])
        .token(&room.ada.token)
        .send()
        .await
        .ok();
    let group_target = ChatMentionTarget::Group { id: group.id };
    let role_target = ChatMentionTarget::Role {
        node_id: room.topic,
        role: Role::Developer,
    };
    let body = |words: &str| {
        vec![
            text(words),
            ChatPart::Mention {
                target: group_target.clone(),
            },
            text(" / "),
            ChatPart::Mention {
                target: role_target.clone(),
            },
        ]
    };
    let message = posted(&room, &room.ada, room.chat, vec![text("Before names")]).await;
    let before = snapshot(&room, room.chat, &[]).await;
    let native = full(
        edit(
            &room,
            &room.ada.token,
            room.chat,
            message,
            &body("Literal Protected edit crew. "),
        )
        .await,
        message,
    );
    let label = |value: &Value, target: &ChatMentionTarget| {
        value["mention_names"]
            .as_array()
            .unwrap()
            .iter()
            .find(|entry| entry["target"] == json!(target))
            .unwrap()["name"]
            .as_str()
            .unwrap()
            .to_owned()
    };
    assert!(label(&native, &group_target).contains("Protected edit crew"));
    assert!(label(&native, &role_target).contains("Login"));
    let unrelated = scoped(
        &room,
        room.ada.id(),
        vec![
            rule(OP, "chat", room.chat, &["write"]),
            rule("ChatGet", "chat", room.chat, &["read"]),
        ],
    )
    .await;
    receipt(
        edit(
            &room,
            &unrelated,
            room.chat,
            message,
            &body("Literal Protected edit crew changed. "),
        )
        .await,
        room.chat,
        message,
    );
    let reader = scoped(
        &room,
        room.ada.id(),
        vec![rule(OP, "chat", room.chat, &["write", "read"])],
    )
    .await;
    let hidden = full(
        edit(
            &room,
            &reader,
            room.chat,
            message,
            &body("Literal Protected edit crew kept. "),
        )
        .await,
        message,
    );
    assert_eq!(label(&hidden, &group_target), "a group");
    assert_eq!(
        label(&hidden, &role_target),
        "Developers of a private place"
    );
    assert!(
        hidden["parts"]
            .to_string()
            .contains("Literal Protected edit crew kept.")
    );
    let reader = scoped(
        &room,
        room.ada.id(),
        vec![
            rule(OP, "chat", room.chat, &["write", "read"]),
            rule(OP, "group", group.id, &["read"]),
            rule(OP, "topic", room.topic, &["read"]),
            rule(OP, "project", room.project, &["read"]),
            rule(OP, "org", room.root, &["read"]),
        ],
    )
    .await;
    let shown = full(
        edit(
            &room,
            &reader,
            room.chat,
            message,
            &body("Restored live names. "),
        )
        .await,
        message,
    );
    assert_eq!(shown["mention_names"], native["mention_names"]);
    assert_eq!(
        stored(&room, room.chat, message).await.content,
        json!(body("Restored live names. "))
    );
    no_dispatch(&before, &snapshot(&room, room.chat, &[]).await);
}

#[tokio::test]
async fn exact_attachment_noop_needs_no_byte_read_but_changed_text_consumes_the_existing_attachment()
 {
    let room = Room::new().await;
    let (hash, size) = room.store(b"native attachment no-op").await;
    let original = vec![attachment(hash, size)];
    let message = posted(&room, &room.bea, room.chat, original.clone()).await;
    let before = snapshot(&room, room.chat, &[hash]).await;
    full(
        edit(&room, &room.bea.token, room.chat, message, &original).await,
        message,
    );
    assert_eq!(
        snapshot(&room, room.chat, &[hash]).await,
        before,
        "the native exact parts/recipient branch is a true no-op"
    );
    let writer = scoped(
        &room,
        room.bea.id(),
        vec![rule(OP, "chat", room.chat, &["write"])],
    )
    .await;
    receipt(
        edit(&room, &writer, room.chat, message, &original).await,
        room.chat,
        message,
    );
    assert_eq!(
        snapshot(&room, room.chat, &[hash]).await,
        before,
        "a scoped true no-op consumes no source and changes no effects"
    );
    let changed = vec![
        text("Changed text still carries the bytes"),
        attachment(hash, size),
    ];
    source_refused(edit(&room, &writer, room.chat, message, &changed).await);
    assert_eq!(snapshot(&room, room.chat, &[hash]).await, before);
    let unrelated = scoped(
        &room,
        room.bea.id(),
        vec![
            rule(OP, "chat", room.chat, &["write"]),
            rule("ChatGet", "chat", room.chat, &["read"]),
        ],
    )
    .await;
    source_refused(edit(&room, &unrelated, room.chat, message, &changed).await);
    assert_eq!(snapshot(&room, room.chat, &[hash]).await, before);
    let reader = scoped(
        &room,
        room.bea.id(),
        vec![rule(OP, "chat", room.chat, &["write", "read"])],
    )
    .await;
    full(
        edit(&room, &reader, room.chat, message, &changed).await,
        message,
    );
    assert_eq!(
        stored(&room, room.chat, message).await.content,
        json!(changed)
    );
    assert_eq!(
        room.pins(&hash).await,
        1,
        "replacing this message does not double-pin its retained file"
    );
    no_dispatch(&before, &snapshot(&room, room.chat, &[hash]).await);
}

#[tokio::test]
async fn changed_edits_select_an_actual_readable_saved_chat_source_not_an_unrelated_read() {
    let room = Room::new().await;
    let source = room.chat_in_another_topic().await;
    let (hash, size) = room.store(b"external saved edit source").await;
    posted(&room, &room.bea, source, vec![attachment(hash, size)]).await;
    assert!(
        !orch_db::queries::uploads::owns(&mut *room.world.conn().await, room.ada.id(), &hash)
            .await
            .unwrap()
    );
    let prime = posted(
        &room,
        &room.ada,
        source,
        vec![text("Native saved-source prime")],
    )
    .await;
    full(
        edit(
            &room,
            &room.ada.token,
            source,
            prime,
            &[
                text("Native consumes the saved source"),
                attachment(hash, size),
            ],
        )
        .await,
        prime,
    );
    assert!(
        !orch_db::queries::uploads::owns(&mut *room.world.conn().await, room.ada.id(), &hash)
            .await
            .unwrap()
    );
    let destination = posted(
        &room,
        &room.ada,
        room.chat,
        vec![text("Destination stays unchanged on refusal")],
    )
    .await;
    let other = room.direct(&room.ada, &room.cai).await.base.id;
    let body = vec![text("Saved bytes copied here"), attachment(hash, size)];
    let before = snapshot(&room, room.chat, &[hash]).await;
    let source_before = snapshot(&room, source, &[hash]).await;
    for extra in [
        vec![],
        vec![rule("ChatGet", "chat", source, &["read"])],
        vec![rule(OP, "chat", other, &["read"])],
    ] {
        let mut rules = vec![rule(OP, "chat", room.chat, &["write"])];
        rules.extend(extra);
        let token = scoped(&room, room.ada.id(), rules).await;
        source_refused(edit(&room, &token, room.chat, destination, &body).await);
        assert_eq!(snapshot(&room, room.chat, &[hash]).await, before);
        assert_eq!(snapshot(&room, source, &[hash]).await, source_before);
    }
    let reader = scoped(
        &room,
        room.ada.id(),
        vec![
            rule(OP, "chat", room.chat, &["write"]),
            rule(OP, "chat", source, &["read"]),
        ],
    )
    .await;
    let pins = room.pins(&hash).await;
    receipt(
        edit(&room, &reader, room.chat, destination, &body).await,
        room.chat,
        destination,
    );
    assert_eq!(
        stored(&room, room.chat, destination).await.content,
        json!(body)
    );
    assert_eq!(room.pins(&hash).await, pins + 1);
    no_dispatch(&before, &snapshot(&room, room.chat, &[hash]).await);
}

#[tokio::test]
async fn missing_or_unreadable_bytes_cannot_be_lent_by_a_key_or_change_any_edit_effect() {
    let room = Room::new().await;
    let private = room.direct(&room.ada, &room.cai).await.base.id;
    let (hash, size) = room.store(b"a private edit source").await;
    orch_db::queries::uploads::remember(
        &mut *room.world.conn().await,
        room.ada.id(),
        &hash,
        room.world.now(),
    )
    .await
    .unwrap();
    posted(&room, &room.ada, private, vec![attachment(hash, size)]).await;
    // Room.store represents Bea's upload. End that custody explicitly, leaving only
    // the real private saved-chat witness; otherwise the native retention path would win.
    sqlx::query("DELETE FROM completed_uploads WHERE user_id=$1 AND hash=$2")
        .bind(room.bea.id())
        .bind(hash.as_bytes().to_vec())
        .execute(&room.world.db.pool)
        .await
        .unwrap();
    assert!(
        !orch_db::queries::uploads::owns(&mut *room.world.conn().await, room.bea.id(), &hash)
            .await
            .unwrap()
    );
    let destination = posted(
        &room,
        &room.bea,
        room.chat,
        vec![text("No private-byte copy")],
    )
    .await;
    let body = vec![attachment(hash, size)];
    let before = snapshot(&room, room.chat, &[hash]).await;
    let native = edit(&room, &room.bea.token, room.chat, destination, &body).await;
    assert_eq!(
        native.0, 404,
        "the native caller cannot read the private witness: {}",
        native.1
    );
    assert_eq!(snapshot(&room, room.chat, &[hash]).await, before);
    let reader = scoped(
        &room,
        room.bea.id(),
        vec![
            rule(OP, "chat", room.chat, &["write"]),
            rule(OP, "chat", private, &["read"]),
        ],
    )
    .await;
    source_refused(edit(&room, &reader, room.chat, destination, &body).await);
    assert_eq!(snapshot(&room, room.chat, &[hash]).await, before);
    source_refused(
        edit(
            &room,
            &reader,
            room.chat,
            destination,
            &[attachment(Hash::of(b"never stored for this edit"), 4)],
        )
        .await,
    );
    assert_eq!(snapshot(&room, room.chat, &[hash]).await, before);
}

#[tokio::test]
async fn native_author_message_chat_deleted_and_disabled_rules_survive_explicit_write_grants() {
    let room = Room::new().await;
    let message = posted(
        &room,
        &room.bea,
        room.chat,
        vec![text("Only its author edits")],
    )
    .await;
    full(
        edit(
            &room,
            &room.bea.token,
            room.chat,
            message,
            &[text("Native author prime")],
        )
        .await,
        message,
    );
    let mut conn = room.world.conn().await;
    let moderator = orch_authz::Principal::load(
        &mut conn,
        orch_authz::Caller::session(room.ada.id()).with_admin_mode(false),
        orch_types::Time::new(room.world.now()),
    )
    .await
    .unwrap();
    assert!(
        orch_authz::nodes::check(
            &mut conn,
            &moderator,
            room.topic,
            orch_access_contract::permissions::Permission::ChatManage,
            orch_authz::Act::Use,
            orch_authz::Policy::default()
        )
        .await
        .unwrap()
        .is_allowed()
    );
    drop(conn);
    let token = scoped(
        &room,
        room.ada.id(),
        vec![rule(OP, "chat", room.chat, &["write", "manage"])],
    )
    .await;
    let before = snapshot(&room, room.chat, &[]).await;
    for actor in [&room.ada.token, &token] {
        let refused = edit(
            &room,
            actor,
            room.chat,
            message,
            &[text("Moderator may not replace it")],
        )
        .await;
        assert_eq!(refused.0, 403, "{}", refused.1);
        assert_eq!(snapshot(&room, room.chat, &[]).await, before);
    }
    let token = scoped(
        &room,
        room.bea.id(),
        vec![rule(OP, "chat", room.chat, &["write"])],
    )
    .await;
    let elsewhere = room.direct(&room.bea, &room.cai).await.base.id;
    let foreign = posted(&room, &room.bea, elsewhere, vec![text("A different chat")]).await;
    let before = snapshot(&room, room.chat, &[]).await;
    let foreign_before = snapshot(&room, elsewhere, &[]).await;
    let refused = edit(&room, &token, room.chat, foreign, &[text("Wrong path")]).await;
    assert_eq!(refused.0, 404, "{}", refused.1);
    assert_eq!(snapshot(&room, room.chat, &[]).await, before);
    assert_eq!(snapshot(&room, elsewhere, &[]).await, foreign_before);
    assert_eq!(
        stored(&room, elsewhere, foreign).await.content,
        json!([text("A different chat")])
    );
    let deleted = result(
        room.world
            .app
            .call(ChatMessageDelete, &[&room.chat, &message])
            .token(&room.bea.token)
            .send_bytes()
            .await,
    );
    assert_eq!(deleted.0, 200, "{}", deleted.1);
    let before = snapshot(&room, room.chat, &[]).await;
    for (actor, can_read) in [(&room.bea.token, true), (&token, false)] {
        let refused = edit(
            &room,
            actor,
            room.chat,
            message,
            &[text("Deleted content stays deleted")],
        )
        .await;
        // A tombstone fails the native message validation before any write.
        assert_eq!(refused.0, 400, "{}", refused.1);
        assert_eq!(refused.1["code"], "request.invalid");
        if can_read {
            assert_eq!(
                refused.1["errors"],
                json!([{"path":"message_id","message":"was deleted"}])
            );
        } else {
            assert_eq!(refused.1["errors"], Value::Null);
            assert_eq!(
                refused.1["detail"],
                "Something in this request is not valid. Check what you entered and try again."
            );
        }
        assert_eq!(snapshot(&room, room.chat, &[]).await, before);
    }
    let live = posted(
        &room,
        &room.bea,
        room.chat,
        vec![text("Policy takes precedence")],
    )
    .await;
    room.world
        .set_at(
            orch_settings::Target::Node(room.topic),
            "chat.enabled",
            json!(false),
        )
        .await;
    let before = snapshot(&room, room.chat, &[]).await;
    for actor in [&room.bea.token, &token] {
        let refused = edit(&room, actor, room.chat, live, &[text("Disabled chat")]).await;
        assert_eq!(refused.0, 403, "{}", refused.1);
        assert_eq!(refused.1["code"], "chat.not_allowed");
        assert_eq!(snapshot(&room, room.chat, &[]).await, before);
    }
}

type Pending = tokio::task::JoinHandle<(u16, Value)>;
// Same real Chat-lock observation budget as chat_group_mutation_scopes: below
// the deployed 15s statement limit, for scheduler headroom rather than authority time.
const WAIT: std::time::Duration = std::time::Duration::from_secs(10);
fn pending_edit(room: &Room, token: &str, chat: Id, message: Id, parts: Vec<ChatPart>) -> Pending {
    let app = room.world.app.clone();
    let token = token.to_owned();
    tokio::spawn(async move {
        let request = app
            .call(ChatMessageUpdate, &[&chat, &message])
            .token(&token)
            .body(&ChatMessageUpdateBody { parts });
        result(request.send_bytes().await)
    })
}
async fn wait_for_chat(room: &Room, blocker: i32, action: &Pending) -> bool {
    tokio::time::timeout(WAIT, async {
        loop {
            let blocked: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM pg_stat_activity WHERE datname=current_database() AND wait_event_type='Lock' AND $1=ANY(pg_blocking_pids(pid)))")
                .bind(blocker).fetch_one(&room.world.db.pool).await.unwrap();
            if blocked { break true; }
            if action.is_finished() { break false; }
            tokio::task::yield_now().await;
        }
    }).await.unwrap_or(false)
}
async fn finish(mut action: Pending) -> (u16, Value) {
    let result = tokio::time::timeout(WAIT, &mut action).await;
    if result.is_err() {
        action.abort();
        let _ = action.await;
    }
    result
        .expect("the released edit completes within the existing request budget")
        .unwrap()
}

#[tokio::test]
async fn a_waiting_edit_preserves_its_original_cas_and_cannot_rebase_over_the_first_writer() {
    let room = Room::new().await;
    let message = posted(
        &room,
        &room.bea,
        room.chat,
        vec![text("Before competing edits")],
    )
    .await;
    let token = key(&room, room.bea.id(), None, chrono::Duration::hours(1)).await;
    full(
        edit(
            &room,
            &token,
            room.chat,
            message,
            &[text("Native legacy prime")],
        )
        .await,
        message,
    );
    // Reuse chats_content's actual competing-writer transaction: the first edit
    // holds the Chat lock while its replacement remains invisible to the second.
    let mut first = room.world.db.pool.begin().await.unwrap();
    let blocker: i32 = sqlx::query_scalar("SELECT pg_backend_pid()")
        .fetch_one(&mut *first)
        .await
        .unwrap();
    let revision: i64 =
        sqlx::query_scalar("UPDATE chats SET next_seq=next_seq+1 WHERE id=$1 RETURNING next_seq-1")
            .bind(room.chat)
            .fetch_one(&mut *first)
            .await
            .unwrap();
    let winning = vec![text("The first writer's accepted content")];
    sqlx::query("UPDATE chat_messages SET content=$2,seq_updated=$3,edited_at=$4,search_text=$5 WHERE id=$1")
        .bind(message)
        .bind(json!(winning))
        .bind(revision)
        .bind(room.world.now())
        .bind("The first writer's accepted content")
        .execute(&mut *first)
        .await
        .unwrap();
    let expected = snapshot_conn(&mut first, room.chat, &[]).await;
    let action = pending_edit(
        &room,
        &token,
        room.chat,
        message,
        vec![text("Must not overwrite the winner")],
    );
    if !wait_for_chat(&room, blocker, &action).await {
        action.abort();
        let _ = action.await;
        first.rollback().await.unwrap();
        panic!("the second edit must actually wait on the first writer's Chat row");
    }
    first.commit().await.unwrap();
    let refused = finish(action).await;
    assert_eq!(refused.0, 409, "{}", refused.1);
    assert_eq!(refused.1["code"], "conflict.stale_revision");
    assert_eq!(refused.1["seq_updated"], revision);
    assert_eq!(
        stored(&room, room.chat, message).await.content,
        json!(winning)
    );
    assert_eq!(
        snapshot(&room, room.chat, &[]).await,
        expected,
        "a stale edit consumes no bytes, recipient effects or later work"
    );
}

#[tokio::test]
async fn a_short_legacy_key_cannot_commit_an_edit_after_its_real_chat_lock_wait() {
    let room = Room::new().await;
    let message = posted(&room, &room.bea, room.chat, vec![text("Before expiry")]).await;
    // Established group-mutation clock control: one second of life, then two
    // seconds advanced only after the actual database blocker is observed.
    let token = key(&room, room.bea.id(), None, chrono::Duration::seconds(1)).await;
    full(
        edit(
            &room,
            &token,
            room.chat,
            message,
            &[text("Native unexpired prime")],
        )
        .await,
        message,
    );
    let before = snapshot(&room, room.chat, &[]).await;
    let mut held = room.world.db.pool.begin().await.unwrap();
    orch_db::queries::chats::lock(&mut held, room.chat)
        .await
        .unwrap()
        .unwrap();
    let blocker: i32 = sqlx::query_scalar("SELECT pg_backend_pid()")
        .fetch_one(&mut *held)
        .await
        .unwrap();
    let action = pending_edit(
        &room,
        &token,
        room.chat,
        message,
        vec![text("Expired work must not commit")],
    );
    if !wait_for_chat(&room, blocker, &action).await {
        action.abort();
        let _ = action.await;
        held.rollback().await.unwrap();
        panic!("the request must reach the known Chat blocker while the key is still live");
    }
    room.world.advance(chrono::Duration::seconds(2));
    held.commit().await.unwrap();
    let refused = finish(action).await;
    assert_eq!(refused.0, 401, "{}", refused.1);
    assert_eq!(refused.1["code"], "auth.invalid_token");
    assert_eq!(
        snapshot(&room, room.chat, &[]).await,
        before,
        "the expired key changes no message/chat/pin/notification/input/job/command state"
    );
}
