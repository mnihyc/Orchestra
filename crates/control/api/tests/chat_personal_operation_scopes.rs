//! Personal Chat state and drafts share identity, not an invented ordinary permission gate.
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
    ChatDraftGet, ChatDraftSet, ChatDraftSetBody, ChatMentionTarget, ChatPart, ChatStateSet,
};
use orch_api_contract::directory::{GroupMemberAdd, OrgGroupCreate, OrgGroupCreateBody};
use orch_api_contract::settings::{NodeSettingSet, NodeSettingSetBody};
use orch_types::{Id, new_id};
use serde_json::{Value, json};

fn rule(op: &str, kind: &str, id: Id, actions: &[&str]) -> Value {
    json!({"target":{"kind":kind,"id":id,"reach":"self"},"allow":[{"operation":op,"actions":actions}]})
}
struct Key {
    id: Id,
    secret: String,
}
async fn key(room: &Room, user: Id, rules: Vec<Value>) -> Key {
    use orch_access_contract::key_scope::KeyScope;
    use orch_api_core::middleware::authentication::{API_TOKEN_PREFIX, new_token, token_hash};
    use orch_db::queries::identity::{NewApiToken, create_api_token_scoped};
    let scope: KeyScope = serde_json::from_value(json!({"rules":rules})).unwrap();
    assert!(scope.validate().is_empty());
    let scope = serde_json::to_value(scope).unwrap();
    let secret = new_token(API_TOKEN_PREFIX);
    let id = new_id();
    create_api_token_scoped(
        &mut *room.world.conn().await,
        &NewApiToken {
            id,
            user_id: user,
            name: "Personal Chat scope control",
            token_hash: &token_hash(&secret),
            scopes: &[],
            node_id: None,
            at: room.world.now(),
            // The fixture clock is fixed; this established short lifetime bounds only the test credential.
            expires_at: Some(room.world.now() + chrono::Duration::hours(1)),
        },
        Some(&scope),
    )
    .await
    .unwrap();
    Key { id, secret }
}
fn result(reply: orch_api_core::testing::RawReply) -> (u16, Value) {
    (
        reply.status.as_u16(),
        serde_json::from_slice(&reply.raw).unwrap(),
    )
}
async fn state(room: &Room, token: &str, chat: Id, body: Value) -> (u16, Value) {
    result(
        room.world
            .app
            .call(ChatStateSet, &[&chat])
            .token(token)
            .raw_body(&body)
            .send_bytes()
            .await,
    )
}
async fn save(
    room: &Room,
    token: &str,
    chat: Id,
    parts: Vec<ChatPart>,
    reply_to: Option<Id>,
) -> (u16, Value) {
    result(
        room.world
            .app
            .call(ChatDraftSet, &[&chat])
            .token(token)
            .body(&ChatDraftSetBody { parts, reply_to })
            .send_bytes()
            .await,
    )
}
async fn get(room: &Room, token: &str, chat: Id) -> (u16, Value) {
    result(
        room.world
            .app
            .call(ChatDraftGet, &[&chat])
            .token(token)
            .send_bytes()
            .await,
    )
}
fn ok(reply: (u16, Value)) -> Value {
    assert_eq!(reply.0, 200, "{}", reply.1);
    reply.1
}
fn receipt(reply: (u16, Value), chat: Id) {
    let body = ok(reply);
    assert_eq!(
        body,
        json!({"projection":"receipt","chat_id":chat}),
        "an acknowledgement contains no existing counters, fences, defaults, hidden state or current names"
    );
}
async fn rows(room: &Room, user: Id, chat: Id) -> Option<Value> {
    sqlx::query_scalar("SELECT to_jsonb(s) FROM chat_states s WHERE user_id=$1 AND chat_id=$2")
        .bind(user)
        .bind(chat)
        .fetch_optional(&room.world.db.pool)
        .await
        .unwrap()
}

#[tokio::test]
async fn state_write_receipts_keep_native_follow_mute_null_and_empty_semantics_private() {
    let room = Room::new().await;
    for (body, level) in [
        (json!({"level":"all"}), Some("all")),
        (json!({"level":"off"}), Some("off")),
        (json!({"level":null}), None),
        (json!({}), None),
    ] {
        let shown = ok(state(&room, &room.bea.token, room.chat, body).await);
        assert_eq!(shown["chat_id"], json!(room.chat));
        assert_eq!(
            shown["level"],
            level.map_or(Value::Null, |value| json!(value))
        );
        assert!(
            shown.get("read_revision").is_some() && shown.get("mentions_unread").is_some(),
            "the native Full really contains existing state"
        );
    }
    let before = rows(&room, room.ada.id(), room.chat).await;
    let token = key(
        &room,
        room.bea.id(),
        vec![rule("ChatStateSet", "chat", room.chat, &["write"])],
    )
    .await;
    receipt(
        state(&room, &token.secret, room.chat, json!({"level":"all"})).await,
        room.chat,
    );
    let actual = ok(state(&room, &room.bea.token, room.chat, json!({})).await);
    assert_eq!(actual["level"], "all");
    assert_eq!(actual["effective_level"], "all");
    assert_eq!(
        rows(&room, room.ada.id(), room.chat).await,
        before,
        "one person never changes another's preference row"
    );
    let unrelated = key(
        &room,
        room.bea.id(),
        vec![
            rule("ChatStateSet", "chat", room.chat, &["write"]),
            rule("ChatGet", "chat", room.chat, &["read"]),
        ],
    )
    .await;
    receipt(
        state(&room, &unrelated.secret, room.chat, json!({"level":"off"})).await,
        room.chat,
    );
    let full = key(
        &room,
        room.bea.id(),
        vec![rule("ChatStateSet", "chat", room.chat, &["write", "read"])],
    )
    .await;
    let actual = ok(state(&room, &full.secret, room.chat, json!({"level":null})).await);
    assert!(actual["level"].is_null());
    assert!(actual.get("projection").is_none());
    assert_eq!(
        actual,
        ok(state(&room, &room.bea.token, room.chat, json!({})).await)
    );
}

#[tokio::test]
async fn state_hidden_is_people_chat_only_and_invalid_node_change_is_atomic() {
    let room = Room::new().await;
    let direct = room.direct(&room.bea, &room.ada).await.base.id;
    let native = ok(state(
        &room,
        &room.bea.token,
        direct,
        json!({"hidden":true,"level":"off"}),
    )
    .await);
    assert_eq!(native["hidden"], true);
    assert_eq!(native["level"], "off");
    let before = rows(&room, room.bea.id(), room.chat).await;
    let invalid = state(
        &room,
        &room.bea.token,
        room.chat,
        json!({"hidden":true,"level":"all"}),
    )
    .await;
    assert_eq!(invalid.0, 400);
    assert_eq!(rows(&room, room.bea.id(), room.chat).await, before);
    let token = key(
        &room,
        room.bea.id(),
        vec![rule("ChatStateSet", "chat", direct, &["write"])],
    )
    .await;
    let other = rows(&room, room.ada.id(), direct).await;
    receipt(
        state(&room, &token.secret, direct, json!({"hidden":false})).await,
        direct,
    );
    let updated = ok(state(&room, &room.bea.token, direct, json!({})).await);
    assert_eq!(updated["hidden"], false);
    assert_eq!(updated["level"], "off", "omitted level stays as it was");
    assert_eq!(rows(&room, room.ada.id(), direct).await, other);
    let denied = state(&room, &token.secret, room.chat, json!({"level":"all"})).await;
    assert_eq!(denied.0, 403);
    assert_eq!(rows(&room, room.bea.id(), room.chat).await, before);
    let node = key(
        &room,
        room.bea.id(),
        vec![rule("ChatStateSet", "chat", room.chat, &["write"])],
    )
    .await;
    assert_eq!(
        state(
            &room,
            &node.secret,
            room.chat,
            json!({"hidden":true,"level":"all"})
        )
        .await
        .0,
        400
    );
    assert_eq!(rows(&room, room.bea.id(), room.chat).await, before);
}

#[tokio::test]
async fn private_draft_reads_and_writes_keep_current_person_and_exact_operation_identity() {
    let room = Room::new().await;
    let parts = vec![text("Bea's own private draft")];
    let native = ok(save(&room, &room.bea.token, room.chat, parts.clone(), None).await);
    assert_eq!(native["parts"], json!(parts));
    assert_eq!(ok(get(&room, &room.bea.token, room.chat).await), native);
    assert!(ok(get(&room, &room.ada.token, room.chat).await).is_null());
    let reader = key(
        &room,
        room.bea.id(),
        vec![rule("ChatDraftGet", "chat", room.chat, &["read"])],
    )
    .await;
    assert_eq!(ok(get(&room, &reader.secret, room.chat).await), native);
    let other = key(
        &room,
        room.ada.id(),
        vec![rule("ChatDraftGet", "chat", room.chat, &["read"])],
    )
    .await;
    assert!(ok(get(&room, &other.secret, room.chat).await).is_null());
    let unrelated = key(
        &room,
        room.bea.id(),
        vec![rule("ChatGet", "chat", room.chat, &["read"])],
    )
    .await;
    assert_eq!(get(&room, &unrelated.secret, room.chat).await.0, 403);
    let writer = key(
        &room,
        room.bea.id(),
        vec![rule("ChatDraftSet", "chat", room.chat, &["write"])],
    )
    .await;
    receipt(
        save(
            &room,
            &writer.secret,
            room.chat,
            vec![text("Accepted replacement")],
            None,
        )
        .await,
        room.chat,
    );
    assert_eq!(
        ok(get(&room, &room.bea.token, room.chat).await)["parts"],
        json!([text("Accepted replacement")])
    );
    let unrelated_read = key(
        &room,
        room.bea.id(),
        vec![
            rule("ChatDraftSet", "chat", room.chat, &["write"]),
            rule("ChatDraftGet", "chat", room.chat, &["read"]),
        ],
    )
    .await;
    receipt(
        save(
            &room,
            &unrelated_read.secret,
            room.chat,
            vec![text("Another accepted replacement")],
            None,
        )
        .await,
        room.chat,
    );
    let full = key(
        &room,
        room.bea.id(),
        vec![rule("ChatDraftSet", "chat", room.chat, &["write", "read"])],
    )
    .await;
    let shown = ok(save(&room, &full.secret, room.chat, parts.clone(), None).await);
    assert!(shown.get("projection").is_none());
    assert_eq!(shown["parts"], json!(parts));
    assert_eq!(shown, ok(get(&room, &room.bea.token, room.chat).await));
}

#[tokio::test]
async fn disabled_sending_does_not_invent_a_gate_for_state_or_the_existing_draft_reader() {
    let room = Room::new().await;
    let parts = vec![text("A private draft authored while sending was allowed")];
    ok(save(&room, &room.bea.token, room.chat, parts.clone(), None).await);
    room.world
        .app
        .call(NodeSettingSet, &[&room.topic, &"chat.enabled"])
        .token(&room.ada.token)
        .body(&NodeSettingSetBody {
            value: json!(false),
            locked: None,
            expected_revision: None,
        })
        .send()
        .await
        .ok();
    let native = ok(get(&room, &room.bea.token, room.chat).await);
    assert_eq!(native["parts"], json!(parts));
    assert_eq!(
        save(
            &room,
            &room.bea.token,
            room.chat,
            vec![text("No new sending")],
            None
        )
        .await
        .0,
        403
    );
    assert_eq!(
        ok(state(&room, &room.bea.token, room.chat, json!({"level":"off"})).await)["level"],
        "off"
    );
    let token = key(
        &room,
        room.bea.id(),
        vec![
            rule("ChatDraftGet", "chat", room.chat, &["read"]),
            rule("ChatDraftSet", "chat", room.chat, &["write"]),
            rule("ChatStateSet", "chat", room.chat, &["write"]),
        ],
    )
    .await;
    assert_eq!(ok(get(&room, &token.secret, room.chat).await), native);
    receipt(
        state(&room, &token.secret, room.chat, json!({"level":"all"})).await,
        room.chat,
    );
    assert_eq!(
        save(
            &room,
            &token.secret,
            room.chat,
            vec![text("Still refused")],
            None
        )
        .await
        .0,
        403
    );
    assert_eq!(
        ok(get(&room, &room.bea.token, room.chat).await)["parts"],
        json!(parts)
    );
}

fn label<'a>(draft: &'a Value, target: &ChatMentionTarget) -> &'a str {
    draft["mention_names"]
        .as_array()
        .unwrap()
        .iter()
        .find(|n| n["target"] == json!(target))
        .unwrap()["name"]
        .as_str()
        .unwrap()
}
#[tokio::test]
async fn draft_get_and_set_share_current_source_bounds_but_keep_authored_typed_parts() {
    let room = Room::new().await;
    let group = room
        .world
        .app
        .call(OrgGroupCreate, &[&room.root])
        .token(&room.ada.token)
        .body(&OrgGroupCreateBody {
            name: "Protected personal crew".into(),
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
    let parts = vec![
        text("Authored literal: Protected personal crew. "),
        ChatPart::Mention {
            target: group_target.clone(),
        },
        text(" / "),
        ChatPart::Mention {
            target: role_target.clone(),
        },
    ];
    let ordinary = ok(save(&room, &room.ada.token, room.chat, parts.clone(), None).await);
    assert!(label(&ordinary, &group_target).contains("Protected personal crew"));
    assert_eq!(label(&ordinary, &role_target), "Developers of Login");
    assert_eq!(ordinary["parts"], json!(parts));
    assert_eq!(ok(get(&room, &room.ada.token, room.chat).await), ordinary);
    for (operation, actions) in [
        ("ChatDraftGet", vec!["read"]),
        ("ChatDraftSet", vec!["write", "read"]),
    ] {
        let limited = key(
            &room,
            room.ada.id(),
            vec![
                rule(operation, "chat", room.chat, &actions),
                rule("ChatOpeningGet", "group", group.id, &["read"]),
            ],
        )
        .await;
        let hidden = if operation == "ChatDraftGet" {
            ok(get(&room, &limited.secret, room.chat).await)
        } else {
            ok(save(&room, &limited.secret, room.chat, parts.clone(), None).await)
        };
        assert_eq!(label(&hidden, &group_target), "a group");
        assert_eq!(
            label(&hidden, &role_target),
            "Developers of a private place"
        );
        assert_eq!(
            hidden["parts"],
            json!(parts),
            "private draft editing parts are not the rendered message wire"
        );
        let full = key(
            &room,
            room.ada.id(),
            vec![
                rule(operation, "chat", room.chat, &actions),
                rule(operation, "group", group.id, &["read"]),
                rule(operation, "topic", room.topic, &["read"]),
                rule(operation, "org", room.root, &["read"]),
            ],
        )
        .await;
        let shown = if operation == "ChatDraftGet" {
            ok(get(&room, &full.secret, room.chat).await)
        } else {
            ok(save(&room, &full.secret, room.chat, parts.clone(), None).await)
        };
        assert_eq!(shown["mention_names"], ordinary["mention_names"]);
        assert_eq!(shown["parts"], ordinary["parts"]);
    }
}

#[tokio::test]
async fn current_person_scope_and_ordinary_view_cuts_protect_all_three_operations() {
    let room = Room::new().await;
    ok(save(
        &room,
        &room.bea.token,
        room.chat,
        vec![text("Saved before the current boundary")],
        None,
    )
    .await);
    ok(state(&room, &room.bea.token, room.chat, json!({"level":"all"})).await);
    for operation in ["ChatStateSet", "ChatDraftGet", "ChatDraftSet"] {
        let actions = if operation == "ChatDraftGet" {
            vec!["read"]
        } else {
            vec!["write"]
        };
        let denied = key(
            &room,
            room.eve.id(),
            vec![rule(operation, "chat", room.chat, &actions)],
        )
        .await;
        let before = rows(&room, room.bea.id(), room.chat).await;
        let reply = match operation {
            "ChatStateSet" => state(&room, &denied.secret, room.chat, json!({"level":"off"})).await,
            "ChatDraftGet" => get(&room, &denied.secret, room.chat).await,
            _ => {
                save(
                    &room,
                    &denied.secret,
                    room.chat,
                    vec![text("Not accepted")],
                    None,
                )
                .await
            }
        };
        assert_eq!(
            reply.0, 403,
            "normal person rights remain a separate requirement: {}",
            reply.1
        );
        assert_eq!(rows(&room, room.bea.id(), room.chat).await, before);
    }
    let token = key(
        &room,
        room.bea.id(),
        vec![
            rule("ChatStateSet", "chat", room.chat, &["write"]),
            rule("ChatDraftGet", "chat", room.chat, &["read"]),
            rule("ChatDraftSet", "chat", room.chat, &["write"]),
        ],
    )
    .await;
    receipt(
        state(&room, &token.secret, room.chat, json!({"level":"off"})).await,
        room.chat,
    );
    let old = ok(get(&room, &token.secret, room.chat).await);
    assert!(old["parts"].is_array());
    let narrowed = json!({"rules":[rule("ChatGet","chat",room.chat,&["read"])]});
    sqlx::query("UPDATE api_tokens SET operation_scope=$2 WHERE id=$1")
        .bind(token.id)
        .bind(narrowed)
        .execute(&room.world.db.pool)
        .await
        .unwrap();
    assert_eq!(
        state(&room, &token.secret, room.chat, json!({"level":"all"}))
            .await
            .0,
        403
    );
    assert_eq!(get(&room, &token.secret, room.chat).await.0, 403);
    assert_eq!(
        save(
            &room,
            &token.secret,
            room.chat,
            vec![text("Denied after narrowing")],
            None
        )
        .await
        .0,
        403
    );
    sqlx::query("UPDATE api_tokens SET revoked_at=$2 WHERE id=$1")
        .bind(token.id)
        .bind(room.world.now())
        .execute(&room.world.db.pool)
        .await
        .unwrap();
    assert_eq!(get(&room, &token.secret, room.chat).await.0, 401);
    assert_eq!(
        ok(get(&room, &room.bea.token, room.chat).await)["parts"],
        old["parts"]
    );
}

#[tokio::test]
async fn a_text_draft_writer_needs_no_get_grant_and_receives_only_its_acceptance_receipt() {
    let room = Room::new().await;
    let first = vec![text("Native own draft before the key")];
    assert_eq!(
        ok(save(&room, &room.bea.token, room.chat, first.clone(), None).await)["parts"],
        json!(first)
    );
    let before = ok(get(&room, &room.ada.token, room.chat).await);
    let token = key(
        &room,
        room.bea.id(),
        vec![rule("ChatDraftSet", "chat", room.chat, &["write"])],
    )
    .await;
    let supplied = vec![text("The write-only key supplied this entire text")];
    receipt(
        save(&room, &token.secret, room.chat, supplied.clone(), None).await,
        room.chat,
    );
    assert_eq!(
        ok(get(&room, &room.bea.token, room.chat).await)["parts"],
        json!(supplied)
    );
    assert_eq!(ok(get(&room, &room.ada.token, room.chat).await), before);
    assert_eq!(
        get(&room, &token.secret, room.chat).await.0,
        403,
        "a write receipt never grants a draft-read endpoint"
    );
}

fn upload_read(user: Id) -> Value {
    json!({"target":{"kind":"account","id":user,"reach":"self"},"resources":["upload"],
        "allow":[{"operation":"ChatDraftSet","actions":["read"]}]})
}
async fn uploaded(room: &Room, by: &places::Who, chat: Id, bytes: &[u8]) -> ChatPart {
    let file = room
        .world
        .app
        .call(orch_api_contract::chats::ChatAttachmentCreate, &[&chat])
        .token(&by.token)
        .header(orch_api_contract::threads::FILENAME_HEADER, "personal.txt")
        .bytes(bytes)
        .send()
        .await
        .ok();
    ChatPart::Attachment {
        blob: file.blob,
        name: file.name,
        size: file.size,
        media_type: file.media_type,
    }
}
fn hash_of(part: &ChatPart) -> orch_types::Hash {
    let ChatPart::Attachment { blob, .. } = part else {
        panic!("this native fixture is an uploaded attachment")
    };
    *blob
}
async fn pins(room: &Room, hash: orch_types::Hash) -> i32 {
    sqlx::query_scalar("SELECT pin_count FROM blobs WHERE hash=$1")
        .bind(hash.as_bytes().as_slice())
        .fetch_one(&room.world.db.pool)
        .await
        .unwrap()
}
#[tokio::test]
async fn stored_draft_bytes_keep_existing_upload_and_protected_source_boundaries_before_custody() {
    let room = Room::new().await;
    let own = uploaded(&room, &room.bea, room.chat, b"owned staged bytes").await;
    let hash = hash_of(&own);
    assert!(
        orch_db::queries::uploads::owns(&mut *room.world.conn().await, room.bea.id(), &hash)
            .await
            .unwrap()
    );
    assert_eq!(
        ok(save(&room, &room.bea.token, room.chat, vec![own.clone()], None).await)["parts"],
        json!([own.clone()])
    );
    let before = ok(get(&room, &room.bea.token, room.chat).await);
    let before_pins = pins(&room, hash).await;
    let write = key(
        &room,
        room.bea.id(),
        vec![rule("ChatDraftSet", "chat", room.chat, &["write"])],
    )
    .await;
    let retained = key(
        &room,
        room.bea.id(),
        vec![
            rule("ChatDraftSet", "chat", room.chat, &["write"]),
            upload_read(room.bea.id()),
        ],
    )
    .await;
    receipt(
        save(&room, &retained.secret, room.chat, vec![own.clone()], None).await,
        room.chat,
    );
    assert_eq!(
        save(&room, &write.secret, room.chat, vec![own.clone()], None)
            .await
            .0,
        404,
        "existing planner requires Upload-family Read for stored personal custody; no fresh/owns exemption"
    );
    assert_eq!(pins(&room, hash).await, before_pins);
    assert_eq!(ok(get(&room, &room.bea.token, room.chat).await), before);
    let source = room.direct(&room.ada, &room.bea).await.base.id;
    let external = uploaded(&room, &room.ada, source, b"separate protected source bytes").await;
    let hash = hash_of(&external);
    let posted = room
        .send(
            &room.ada,
            source,
            vec![external.clone()],
            &new_id().to_string(),
        )
        .await;
    assert_eq!(posted.status, 201);
    assert!(
        !orch_db::queries::uploads::owns(&mut *room.world.conn().await, room.bea.id(), &hash)
            .await
            .unwrap()
    );
    let principal = orch_authz::Principal::load(
        &mut *room.world.conn().await,
        orch_authz::Caller::session(room.bea.id()),
        orch_types::Time::new(room.world.now()),
    )
    .await
    .unwrap();
    assert_eq!(
        orch_chat::following::blob_origin(&mut *room.world.conn().await, &principal, &hash)
            .await
            .unwrap(),
        Some(orch_chat::following::BlobOrigin::Chat(source)),
        "the person has a genuine normal readable source, not personal custody yet"
    );
    let before = ok(get(&room, &room.bea.token, room.chat).await);
    let count = pins(&room, hash).await;
    assert_eq!(
        save(
            &room,
            &write.secret,
            room.chat,
            vec![external.clone()],
            None
        )
        .await
        .0,
        404
    );
    assert_eq!(pins(&room, hash).await, count);
    assert!(
        !orch_db::queries::uploads::owns(&mut *room.world.conn().await, room.bea.id(), &hash)
            .await
            .unwrap()
    );
    assert_eq!(ok(get(&room, &room.bea.token, room.chat).await), before);
    let copied = key(
        &room,
        room.bea.id(),
        vec![
            rule("ChatDraftSet", "chat", room.chat, &["write"]),
            rule("ChatDraftSet", "chat", source, &["read"]),
        ],
    )
    .await;
    receipt(
        save(
            &room,
            &copied.secret,
            room.chat,
            vec![external.clone()],
            None,
        )
        .await,
        room.chat,
    );
    assert_eq!(
        ok(get(&room, &room.bea.token, room.chat).await)["parts"],
        json!([external.clone()])
    );
    assert!(
        orch_db::queries::uploads::owns(&mut *room.world.conn().await, room.bea.id(), &hash)
            .await
            .unwrap()
    );
    assert_eq!(
        save(&room, &write.secret, room.chat, vec![external], None)
            .await
            .0,
        404,
        "another credential's accepted copy must not turn this key into unrestricted private-upload Read"
    );
}

#[tokio::test]
async fn native_legacy_chat_view_keeps_state_and_own_draft_read_without_adding_send() {
    use orch_api_core::middleware::authentication::{API_TOKEN_PREFIX, new_token, token_hash};
    use orch_db::queries::identity::{NewApiToken, create_api_token};
    let room = Room::new().await;
    let parts = vec![text(
        "An existing private draft before narrower legacy access",
    )];
    ok(save(&room, &room.bea.token, room.chat, parts.clone(), None).await);
    let secret = new_token(API_TOKEN_PREFIX);
    create_api_token(
        &mut *room.world.conn().await,
        &NewApiToken {
            id: new_id(),
            user_id: room.bea.id(),
            name: "Legacy ChatView boundary",
            token_hash: &token_hash(&secret),
            scopes: &["chat.view".to_owned()],
            node_id: None,
            at: room.world.now(),
            expires_at: Some(room.world.now() + chrono::Duration::hours(1)),
        },
    )
    .await
    .unwrap();
    let read = ok(get(&room, &secret, room.chat).await);
    assert_eq!(read["parts"], json!(parts));
    let preference = ok(state(&room, &secret, room.chat, json!({"level":"off"})).await);
    assert_eq!(preference["level"], "off");
    assert!(
        preference.get("projection").is_none(),
        "legacy native wire remains Full"
    );
    let refused = save(
        &room,
        &secret,
        room.chat,
        vec![text("This credential cannot send")],
        None,
    )
    .await;
    assert_eq!(refused.0, 403);
    assert_eq!(
        ok(get(&room, &room.bea.token, room.chat).await)["parts"],
        json!(parts)
    );
}
