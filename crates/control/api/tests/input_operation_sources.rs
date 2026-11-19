//! Direct input scopes bind actual stored-byte sources and native/bypass effects to the same operation.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]

mod places;
mod rig;
mod support;

use orch_api_contract::chats::{
    ChatMessageCreate, ChatMessageCreateBody, ChatMessageDelete, ChatPart,
};
use orch_api_contract::common::ErrorCode;
use orch_api_contract::threads::{
    ContentPart, Delivery, LimitBypassRequest, ThreadInputCreate, ThreadInputCreateBody,
    ThreadInputCreateResponse,
};
use orch_api_core::testing::Reply;
use orch_types::{Hash, Id, new_id};
use rig::Scene;
use serde_json::{Value, json};

fn rule(kind: &str, id: Id, resources: Option<Vec<&str>>, actions: &[&str]) -> Value {
    let mut row = json!({"target":{"kind":kind,"id":id,"reach":"self"},"allow":[{"operation":"ThreadInputCreate","actions":actions}]});
    if let Some(resources) = resources {
        row["resources"] = json!(resources);
    }
    row
}

async fn key(s: &Scene, rules: Vec<Value>) -> String {
    use orch_access_contract::key_scope::KeyScope;
    use orch_api_core::middleware::authentication::{API_TOKEN_PREFIX, new_token, token_hash};
    use orch_db::queries::identity::{NewApiToken, create_api_token_scoped};
    let scope: KeyScope = serde_json::from_value(json!({"rules":rules})).unwrap();
    assert!(scope.validate().is_empty());
    let stored = serde_json::to_value(scope).unwrap();
    let secret = new_token(API_TOKEN_PREFIX);
    let hash = token_hash(&secret);
    let now = s.rig.world.now();
    let mut conn = s.rig.world.conn().await;
    create_api_token_scoped(
        &mut conn,
        &NewApiToken {
            id: new_id(),
            user_id: s.ada.id(),
            name: "Input source control",
            token_hash: &hash,
            scopes: &[],
            node_id: None,
            at: now,
            // A short ordinary lifetime is enough for this fixed-clock acceptance sequence.
            expires_at: Some(now + chrono::Duration::hours(1)),
        },
        Some(&stored),
    )
    .await
    .unwrap();
    secret
}

async fn hold(s: &Scene, thread: Id) {
    sqlx::query("UPDATE threads SET queue_held=true WHERE node_id=$1")
        .bind(thread)
        .execute(s.rig.db.pool())
        .await
        .unwrap();
}

async fn scene() -> (Scene, Id) {
    let s = Scene::new().await;
    let thread = s.thread().await;
    assert!(thread.input_access.allowed);
    hold(&s, thread.base.id).await;
    (s, thread.base.id)
}

async fn stored(s: &Scene, bytes: &[u8]) -> (Hash, i64) {
    let hash = Hash::of(bytes);
    let size = i64::try_from(bytes.len()).unwrap();
    sqlx::query("INSERT INTO blobs(hash,size) VALUES($1,$2) ON CONFLICT DO NOTHING")
        .bind(hash.as_bytes().to_vec())
        .bind(size)
        .execute(s.rig.db.pool())
        .await
        .unwrap();
    let mut conn = s.rig.world.conn().await;
    orch_db::queries::uploads::remember(&mut conn, s.ada.id(), &hash, s.rig.world.now())
        .await
        .unwrap();
    (hash, size)
}

fn file(hash: Hash, size: i64) -> ContentPart {
    ContentPart::File {
        path: ".orch/attachments/provided.txt".into(),
        name: "provided.txt".into(),
        size,
        blob: Some(hash),
    }
}

fn input(content: Vec<ContentPart>) -> ThreadInputCreateBody {
    ThreadInputCreateBody {
        content,
        delivery: Delivery::Queue,
        client_id: new_id().to_string(),
        bypass: None,
        native_action: None,
        plan: false,
    }
}

async fn send(
    s: &Scene,
    thread: Id,
    token: &str,
    body: &ThreadInputCreateBody,
) -> Reply<ThreadInputCreateResponse> {
    s.rig
        .world
        .app
        .call(ThreadInputCreate, &[&thread])
        .token(token)
        .body(body)
        .send()
        .await
}

fn receipt(reply: Reply<ThreadInputCreateResponse>, thread: Id, status: u16) -> Id {
    assert_eq!(reply.status.as_u16(), status, "{:?}", reply.body);
    let raw: Value = serde_json::from_slice(&reply.raw).unwrap();
    let ThreadInputCreateResponse::Receipt(row) = reply.ok() else {
        panic!("no Read facet was granted on the input target")
    };
    assert_eq!(row.thread_id, thread);
    assert_eq!(
        raw,
        json!({"projection":"receipt","thread_id":thread,"input_id":row.input_id,"seq":row.seq})
    );
    row.input_id
}

async fn count(s: &Scene, thread: Id) -> i64 {
    sqlx::query_scalar("SELECT count(*) FROM thread_inputs WHERE thread_id=$1")
        .bind(thread)
        .fetch_one(s.rig.db.pool())
        .await
        .unwrap()
}

async fn carries(s: &Scene, id: Id, hash: Hash) {
    let (author, content, held): (Id, Value, bool) =
        sqlx::query_as("SELECT author_id,content,queue_held FROM thread_inputs WHERE id=$1")
            .bind(id)
            .fetch_one(s.rig.db.pool())
            .await
            .unwrap();
    assert_eq!(author, s.ada.id());
    assert!(held);
    let parts: Vec<ContentPart> = serde_json::from_value(content).unwrap();
    assert!(
        parts
            .iter()
            .any(|part| matches!(part,ContentPart::File{blob:Some(found),..} if *found==hash))
    );
}

#[tokio::test]
async fn input_retained_bytes_need_same_operation_upload_read_on_the_actual_account() {
    let (s, thread) = scene().await;
    let (hash, size) = stored(&s, b"Input bytes retained by the same account").await;
    let body = input(vec![file(hash, size)]);
    let narrow = key(&s, vec![rule("thread", thread, None, &["write"])]).await;
    assert_eq!(
        send(&s, thread, &narrow, &body).await.problem().code,
        ErrorCode::NotFound
    );
    assert_eq!(count(&s, thread).await, 0);
    let permitted = key(
        &s,
        vec![
            rule("thread", thread, None, &["write"]),
            rule("account", s.ada.id(), Some(vec!["upload"]), &["read"]),
        ],
    )
    .await;
    let accepted = receipt(send(&s, thread, &permitted, &body).await, thread, 201);
    carries(&s, accepted, hash).await;
    let ordinary = send(&s, thread, &s.ada.token, &input(vec![file(hash, size)]))
        .await
        .ok()
        .into_full()
        .unwrap();
    assert!(ordinary.input.queue_held);
    assert_eq!(count(&s, thread).await, 2);
    assert!(s.rig.turn_starts(thread).is_empty());
    s.rig.engine.stop();
}

#[tokio::test]
async fn input_saved_source_can_replace_disallowed_custody_and_replay_needs_no_new_source() {
    let (s, thread) = scene().await;
    let source = s.tree.topic.base.chat_id;
    let (hash, size) = stored(&s, b"Saved chat bytes supplied to a different thread").await;
    let message = s
        .rig
        .world
        .app
        .call(ChatMessageCreate, &[&source])
        .token(&s.ada.token)
        .body(&ChatMessageCreateBody {
            parts: vec![ChatPart::Attachment {
                blob: hash,
                name: "provided.txt".into(),
                size,
                media_type: "text/plain".into(),
            }],
            reply_to: None,
            agent_review: None,
            client_id: new_id().to_string(),
        })
        .send()
        .await
        .ok()
        .into_full()
        .unwrap()
        .message;
    let scoped = key(
        &s,
        vec![
            rule("thread", thread, None, &["write"]),
            rule("chat", source, None, &["read"]),
        ],
    )
    .await;
    let mut body = input(vec![file(hash, size)]);
    let accepted = receipt(send(&s, thread, &scoped, &body).await, thread, 201);
    carries(&s, accepted, hash).await;
    s.rig
        .world
        .app
        .call(ChatMessageDelete, &[&source, &message.id])
        .token(&s.ada.token)
        .send()
        .await
        .ok();
    // The replacement is not another operation. Neither the deleted saved source nor these new
    // unreadable bytes may be demanded or copied when returning an existing acceptance receipt.
    let replacement = b"Never supplied to this account";
    body.content = vec![file(
        Hash::of(replacement),
        i64::try_from(replacement.len()).unwrap(),
    )];
    assert_eq!(
        receipt(send(&s, thread, &scoped, &body).await, thread, 200),
        accepted
    );
    assert_eq!(count(&s, thread).await, 1);
    carries(&s, accepted, hash).await;
    assert!(s.rig.turn_starts(thread).is_empty());
    s.rig.engine.stop();
}

#[tokio::test]
async fn input_bypass_scopes_cover_actual_provider_effects_but_ignore_nonapplicable_caps() {
    let (s, thread) = scene().await;
    let ordinary = key(&s, vec![rule("thread", thread, None, &["write"])]).await;
    let mut no_effect = input(vec![ContentPart::Text {
        text: "A cap which does not apply stays ignored".into(),
    }]);
    no_effect.bypass = Some(LimitBypassRequest {
        caps: Some(vec![new_id()]),
        provider_limit: None,
    });
    let ignored = receipt(send(&s, thread, &ordinary, &no_effect).await, thread, 201);
    let bypass: Option<Value> =
        sqlx::query_scalar("SELECT limit_bypass FROM thread_inputs WHERE id=$1")
            .bind(ignored)
            .fetch_one(s.rig.db.pool())
            .await
            .unwrap();
    assert!(
        bypass.is_none(),
        "the ordinary producer creates no bypass for a non-applicable cap"
    );
    // A configured zero-running cap is reached without fabricating vendor quota or ledger use.
    let cap = new_id();
    sqlx::query("INSERT INTO usage_caps (id, provider_id, subject_kind, split, meter, amount, at_limit, created_by)
        VALUES ($1, $2, 'everyone', 'shared', 'running', 0, 'stop', $3)")
        .bind(cap).bind(s.on.provider).bind(s.ada.id()).execute(s.rig.db.pool()).await.unwrap();
    let mut actual = input(vec![ContentPart::Text {
        text: "An explicit bypass of the configured provider cap".into(),
    }]);
    actual.bypass = Some(LimitBypassRequest {
        caps: Some(vec![cap]),
        provider_limit: None,
    });
    assert_eq!(
        send(&s, thread, &ordinary, &actual).await.problem().code,
        ErrorCode::AuthzDenied
    );
    assert_eq!(count(&s, thread).await, 1);
    let permitted = key(
        &s,
        vec![
            rule("thread", thread, None, &["write"]),
            rule("provider", s.on.provider, None, &["manage"]),
        ],
    )
    .await;
    let accepted = receipt(send(&s, thread, &permitted, &actual).await, thread, 201);
    let bypass: Value = sqlx::query_scalar("SELECT limit_bypass FROM thread_inputs WHERE id=$1")
        .bind(accepted)
        .fetch_one(s.rig.db.pool())
        .await
        .unwrap();
    assert_eq!(bypass.as_array().unwrap().len(), 1);
    assert_eq!(bypass[0]["caps"], json!([cap]));
    assert_eq!(bypass[0]["provider_limit"], false);
    assert_eq!(bypass[0]["by"], json!(s.ada.id()));
    assert_eq!(count(&s, thread).await, 2);
    assert!(s.rig.turn_starts(thread).is_empty());
    s.rig.engine.stop();
}

#[tokio::test]
async fn a_stored_native_manage_action_requires_the_current_operations_manage_facet() {
    use orch_api_contract::input_schedules::NativeInvocation;
    use orch_api_contract::native::*;
    let s = Scene::new().await;
    let thread = s.thread().await.base.id;
    s.say(
        &s.ada,
        thread,
        "Open the ordinary session",
        Delivery::Queue,
        "native-source-open",
    )
    .await
    .ok();
    s.rig.run_turn(thread, 0, "ready").await;
    hold(&s, thread).await;
    *s.rig.commands.native.lock().unwrap() = Some(NativeCatalog {
        revision: "input-source-catalogue".into(),
        state: NativeCatalogState::Ready,
        actions: vec![NativeAction {
            id: "manage-artifact".into(),
            name: "context-action".into(),
            title: "Context action".into(),
            kind: NativeActionKind::Skill,
            description: "Verified fixture action with declared Manage authority".into(),
            arguments_schema: json!({"type":"object","additionalProperties":false}),
            argument_hint: None,
            origin: NativeOrigin {
                kind: NativeOriginKind::Builtin,
                name: None,
                path: None,
            },
            permission: NativePermission::Manage,
            delivery_modes: vec![NativeDelivery::Queue],
            available: true,
            reason: None,
        }],
        reason: None,
        project_skills: NativeProjectSkills {
            available: false,
            reason: Some("No project discovery in this fixture".into()),
        },
        as_of: None,
    });
    let catalog = s
        .rig
        .world
        .app
        .call(ThreadNativeActionList, &[&thread])
        .token(&s.ada.token)
        .query(&ThreadNativeActionListQuery::default())
        .send()
        .await
        .ok();
    assert_eq!(catalog.actions[0].permission, NativePermission::Manage);
    let mut body = input(vec![ContentPart::Text {
        text: "The supplied native action".into(),
    }]);
    body.native_action = Some(NativeInvocation {
        action_id: catalog.actions[0].id.clone(),
        revision: catalog.revision,
        arguments: json!({}),
    });
    let before = count(&s, thread).await;
    let narrow = key(&s, vec![rule("thread", thread, None, &["write"])]).await;
    assert_eq!(
        send(&s, thread, &narrow, &body).await.problem().code,
        ErrorCode::AuthzDenied
    );
    assert_eq!(count(&s, thread).await, before);
    let permitted = key(&s, vec![rule("thread", thread, None, &["write", "manage"])]).await;
    let accepted = receipt(send(&s, thread, &permitted, &body).await, thread, 201);
    let actual: Value = sqlx::query_scalar("SELECT native_action FROM thread_inputs WHERE id=$1")
        .bind(accepted)
        .fetch_one(s.rig.db.pool())
        .await
        .unwrap();
    assert_eq!(actual, json!(body.native_action.unwrap()));
    assert_eq!(count(&s, thread).await, before + 1);
    assert_eq!(
        s.rig.turn_starts(thread).len(),
        1,
        "the held native input was accepted but not invoked"
    );
    s.rig.engine.stop();
}

async fn native_input_for_edit() -> (
    Scene,
    Id,
    Id,
    orch_api_contract::input_schedules::NativeInvocation,
) {
    use orch_api_contract::native::*;
    let s = Scene::new().await;
    let thread = s.thread().await.base.id;
    s.say(
        &s.ada,
        thread,
        "Open native edit session",
        Delivery::Queue,
        "native-edit-open",
    )
    .await
    .ok();
    s.rig.run_turn(thread, 0, "ready").await;
    hold(&s, thread).await;
    *s.rig.commands.native.lock().unwrap() = Some(NativeCatalog {
        revision: "native-edit-catalogue".into(),
        state: NativeCatalogState::Ready,
        actions: vec![NativeAction {
            id: "managed-edit-control".into(),
            name: "managed-edit-control".into(),
            title: "Managed edit control".into(),
            kind: NativeActionKind::Skill,
            description: "A real stored Manage declaration for the existing native dispatch path"
                .into(),
            arguments_schema: json!({"type":"object","properties":{"argument":{"type":"string"}},"required":["argument"],"additionalProperties":false}),
            argument_hint: None,
            origin: NativeOrigin {
                kind: NativeOriginKind::Builtin,
                name: None,
                path: None,
            },
            permission: NativePermission::Manage,
            delivery_modes: vec![NativeDelivery::Queue],
            available: true,
            reason: None,
        }],
        reason: None,
        project_skills: NativeProjectSkills {
            available: false,
            reason: Some("No project discovery in this control".into()),
        },
        as_of: None,
    });
    let catalog = s
        .rig
        .world
        .app
        .call(ThreadNativeActionList, &[&thread])
        .token(&s.ada.token)
        .query(&ThreadNativeActionListQuery::default())
        .send()
        .await
        .ok();
    assert_eq!(catalog.actions[0].permission, NativePermission::Manage);
    let selected = orch_api_contract::input_schedules::NativeInvocation {
        action_id: catalog.actions[0].id.clone(),
        revision: catalog.revision,
        arguments: json!({"argument":"unchanged native argument"}),
    };
    let creator = key(&s, vec![rule("thread", thread, None, &["write", "manage"])]).await;
    let mut body = input(vec![ContentPart::Text {
        text: "Original native edit words".into(),
    }]);
    body.native_action = Some(selected.clone());
    let id = receipt(send(&s, thread, &creator, &body).await, thread, 201);
    (s, thread, id, selected)
}

async fn content_edit_boundary(s: &Scene, thread: Id, manage: bool) -> orch_authz::Caller {
    use orch_access_contract::key_scope::{KeyScope, KeyTargetKind};
    use orch_access_contract::permissions::ObjectKind;
    use orch_api_contract::operations::TargetResolver;
    use orch_api_contract::routes::Endpoint;
    use orch_api_contract::threads::ThreadInputUpdate;
    use orch_engine::operation_scope::{Prepared, targets};
    let actions = if manage {
        json!(["write", "manage"])
    } else {
        json!(["write"])
    };
    let rules = vec![
        json!({"target":{"kind":"thread","id":thread,"reach":"self"},
        "allow":[{"operation":"ThreadInputUpdate","actions":actions}]}),
    ];
    let policy: KeyScope = serde_json::from_value(json!({"rules":rules})).unwrap();
    let secret = key(
        s,
        policy
            .rules
            .iter()
            .map(|rule| serde_json::to_value(rule).unwrap())
            .collect(),
    )
    .await;
    let hash = orch_api_core::middleware::authentication::token_hash(&secret);
    let mut conn = s.rig.world.conn().await;
    let id: Id = sqlx::query_scalar("SELECT id FROM api_tokens WHERE token_hash=$1")
        .bind(hash)
        .fetch_one(&mut *conn)
        .await
        .unwrap();
    let caller = orch_authz::Caller::token(
        s.ada.id(),
        orch_authz::TokenGrant::from_stored_scope(
            &[],
            None,
            Some(policy),
            Some(orch_types::Time::new(
                s.rig.world.now() + chrono::Duration::hours(1),
            )),
        )
        .unwrap()
        .for_token(id),
    );
    let target = targets::node(&mut conn, thread).await.unwrap();
    let mut plan = Prepared::new(ThreadInputUpdate::ROUTE, "ThreadInputUpdate").unwrap();
    for (index, facet) in plan.variant().facets.clone().iter().enumerate() {
        match facet.target {
            TargetResolver::Path("id") => plan
                .bind(
                    index,
                    KeyTargetKind::Object(ObjectKind::Thread),
                    std::slice::from_ref(&target),
                )
                .unwrap(),
            TargetResolver::ResolvedCollection(_) => plan.bind_empty(index).unwrap(),
            _ => panic!("content-only edit has no invented singleton effect"),
        }
    }
    // This is the actual content-edit producer boundary; HTTP reply projection is covered separately.
    // No native operation is performed by this edit, so its native collection is explicitly empty.
    plan.admit(&caller, false).unwrap()
}

async fn release_native_input(s: &Scene, thread: Id) {
    use orch_api_contract::threads::{ThreadQueueResume, ThreadQueueResumeBody};
    let row = orch_db::queries::threads::thread(&mut *s.rig.world.conn().await, thread)
        .await
        .unwrap()
        .unwrap();
    s.rig
        .world
        .app
        .call(ThreadQueueResume, &[&thread])
        .token(&s.ada.token)
        .body(&ThreadQueueResumeBody {
            revision: row.queue_revision,
        })
        .send()
        .await
        .ok();
    s.rig.engine.preflight(thread).await.unwrap();
}

#[tokio::test]
async fn write_only_content_edit_preserves_words_but_holds_retained_manage_native_work() {
    let (s, thread, id, selected) = native_input_for_edit().await;
    let editor = content_edit_boundary(&s, thread, false).await;
    let edited = "Edited native words without new management authority";
    s.rig
        .engine
        .edit_input(
            &editor,
            thread,
            id,
            Some(vec![ContentPart::Text {
                text: edited.into(),
            }]),
            None,
        )
        .await
        .unwrap();
    // Let the ordinary author check settle before reading the queue revision. A cancellation
    // here is the product failure this control must expose, not an unrelated resume conflict.
    s.rig.engine.preflight(thread).await.unwrap();
    let row = orch_db::queries::threads::input(&mut *s.rig.world.conn().await, id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(row.content, json!([{"type":"text","text":edited}]));
    assert_eq!(row.native_action, Some(json!(selected)));
    assert_eq!(
        row.state, "pending",
        "editing words must retain the queued command for correction"
    );
    release_native_input(&s, thread).await;
    let row = orch_db::queries::threads::input(&mut *s.rig.world.conn().await, id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        row.state, "pending",
        "the narrowed command keeps its words and arguments for correction"
    );
    assert!(
        row.state_detail
            .as_deref()
            .is_some_and(|reason| reason.starts_with("native_action.")),
        "{:?}",
        row.state_detail
    );
    assert_eq!(
        s.rig.turn_starts(thread).len(),
        1,
        "the stored Manage invocation must not dispatch"
    );
    s.rig.engine.stop();
}

#[tokio::test]
async fn manage_enabled_content_edit_dispatches_edited_text_and_unchanged_native_arguments() {
    let (s, thread, id, selected) = native_input_for_edit().await;
    let editor = content_edit_boundary(&s, thread, true).await;
    let edited = "Edited native words with matching management authority";
    s.rig
        .engine
        .edit_input(
            &editor,
            thread,
            id,
            Some(vec![ContentPart::Text {
                text: edited.into(),
            }]),
            None,
        )
        .await
        .unwrap();
    release_native_input(&s, thread).await;
    let start = s.rig.turn_start(thread, 1).await;
    assert!(
        start.text.ends_with(edited),
        "the exact edited text reaches the native path: {}",
        start.text
    );
    assert!(!start.text.contains("Original native edit words"));
    assert_eq!(start.native_action, Some(selected.clone()));
    // The native TurnStarted event acknowledges delivery; seeing the transport command alone
    // intentionally leaves the input delivering until the harness takes the turn up.
    s.rig
        .answer(thread, &start, "native-edit-complete", "done")
        .await;
    rig::eventually("the native input delivery acknowledgement", || async {
        orch_db::queries::threads::input(&mut *s.rig.world.conn().await, id)
            .await
            .unwrap()
            .unwrap()
            .state
            == "delivered"
    })
    .await;
    let row = orch_db::queries::threads::input(&mut *s.rig.world.conn().await, id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(row.state, "delivered");
    assert_eq!(row.native_action, Some(json!(selected)));
    s.rig
        .say(
            thread,
            rig::turn_ended(
                start.turn_id,
                orch_link_contract::events::TurnOutcome::Completed,
            ),
        )
        .await;
    s.rig.engine.stop();
}
