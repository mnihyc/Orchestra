//! Ordinary native tools share Ask's reader, retain accepted results, and disclose them before sharing.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "test assertions fail by panicking"
)]

mod places;
mod rig;
mod rig_tool_source;
mod support;

use orch_agent_contract::events::{ItemContent, OrchTool, ToolCallPayload, ToolPayload};
use orch_api_contract::access::{OwnerReadDisclosure, PermissionPageUpdate, SharePreviewCreate};
use orch_api_contract::common::ErrorCode;
use orch_api_contract::directory::OrgGroupCreate;
use orch_api_contract::share::ShareAgentCreate;
use orch_api_contract::threads::{ItemPayload, OutputRef, RawCall, ThreadItemList};
use orch_api_core::dispatch::StoredCommand;
use orch_engine::commands::Commands;
use orch_engine::tools::{CallContext, offered_for_session, read_tools::owner_is_only_viewer};
use orch_link_contract::events::{Event, ItemCompleted};
use orch_settings::Target;
use orch_types::{Id, Json};
use serde_json::json;

async fn setup(enabled: bool) -> (rig::Scene, Id, Id) {
    let scene = rig::Scene::new().await;
    let thread = scene.thread().await.base.id;
    scene
        .rig
        .world
        .app
        .call(PermissionPageUpdate, &[&"nodes", &thread])
        .token(&scene.ada.token)
        .raw_body(&json!({"whole":"private"}))
        .send()
        .await
        .ok();
    if enabled {
        scene
            .rig
            .world
            .set_at(Target::Node(thread), "orch_tools.owner_reads", json!(true))
            .await;
    }
    let turn = rig_tool_source::active(&scene, thread).await.unwrap();
    (scene, thread, turn)
}

async fn call(
    scene: &rig::Scene,
    thread: Id,
    turn: Id,
    reference: &str,
    tool: OrchTool,
    arguments: Json,
) -> Json {
    scene
        .rig
        .engine
        .answer_tool_call(&CallContext {
            machine: scene.rig.machine_id,
            thread,
            turn,
            call_ref: reference.into(),
            attempt: None,
            tool,
            arguments,
        })
        .await
        .unwrap();
    scene
        .rig
        .commands
        .retained_body(orch_engine::ids::tool_result(thread, reference))
        .await
        .unwrap()
        .unwrap()
}

/// Rig's scripted command transport has no SQL publisher. Exercise the production publisher
/// with the same genuine admitted source and retained answer, rather than manufacturing a read grant.
async fn publish(scene: &rig::Scene, thread: Id, reference: &str, body: Json) -> Json {
    let id = orch_engine::ids::tool_result(thread, reference);
    scene
        .rig
        .world
        .app
        .app
        .link
        .dispatcher()
        .issue_stored(StoredCommand {
            id: Some(id),
            machine: scene.rig.machine_id,
            kind: "orch.tool_result".into(),
            key: None,
            deadline: None,
            body,
            authority: None,
        })
        .await
        .unwrap();
    sqlx::query_scalar("SELECT body FROM commands WHERE id=$1")
        .bind(id)
        .fetch_one(scene.rig.db.pool())
        .await
        .unwrap()
}

async fn eligible(scene: &rig::Scene, thread: Id) -> bool {
    let mut conn = scene.rig.db.acquire().await.unwrap();
    let row = orch_db::queries::threads::thread(&mut conn, thread)
        .await
        .unwrap()
        .unwrap();
    owner_is_only_viewer(&mut conn, &row, scene.rig.world.app.app.clock.time())
        .await
        .unwrap()
}

fn read_args(thread: Id) -> Json {
    json!({"operation":"NodeGet","parameters":{"path":{"id":thread}}})
}
fn share(person: Id) -> Json {
    json!({"name":[{"holders":[{"kind":"user","id":person}],"choice":"full_view"}]})
}

async fn retained_history(scene: &rig::Scene, thread: Id, turn: Id, result: &Json) {
    let payload = ToolCallPayload {
        tool: ToolPayload::Orch {
            tool: OrchTool::Read,
            arguments: read_args(thread),
            result: Some(result.clone()),
        },
        title: "Read the private thread".into(),
        started_at: scene.rig.world.app.app.clock.time(),
        duration_ms: Some(1),
        raw: RawCall {
            name: "read".into(),
            input: OutputRef::of(&read_args(thread).to_string()),
            output: Some(OutputRef::of(&result.to_string())),
            is_error: false,
        },
    };
    scene
        .rig
        .say(
            thread,
            Event::ItemStarted(
                orch_agent_contract::events::ItemStarted {
                    turn_id: turn,
                    native_ref: "retained-personal-result".into(),
                    stream: "main".into(),
                    parent_native_ref: None,
                    content: ItemContent::ToolCall(payload.clone()),
                }
                .into(),
            ),
        )
        .await;
    scene
        .rig
        .say(
            thread,
            Event::ItemCompleted(ItemCompleted {
                native_ref: "retained-personal-result".into(),
                status: orch_agent_contract::events::ItemStatus::Completed,
                payload: orch_link_contract::foreign::ItemPayload::ToolCall(payload),
                output_blob: None,
            }),
        )
        .await;
    rig::eventually("the native tool result is recorded in ordinary history", || async {
        sqlx::query_scalar::<_,bool>("SELECT EXISTS(SELECT 1 FROM items WHERE thread_id=$1 AND native_ref='retained-personal-result' AND status='completed')")
            .bind(thread).fetch_one(scene.rig.db.pool()).await.unwrap()
    }).await;
}

#[tokio::test]
async fn docs_are_offered_without_opt_in_and_personal_reads_require_the_current_option() {
    let (scene, thread, turn) = setup(false).await;
    let mut conn = scene.rig.db.acquire().await.unwrap();
    let row = orch_db::queries::threads::thread(&mut conn, thread)
        .await
        .unwrap()
        .unwrap();
    let offered = offered_for_session(&mut conn, &row, true, scene.rig.world.app.app.clock.time())
        .await
        .unwrap();
    assert!(offered.contains(&OrchTool::OrchDocs));
    assert!(!offered.iter().any(|tool| tool.is_owner_read()));
    drop(conn);
    for (reference, query) in [("docs-en", "thread"), ("docs-zh", "线程")] {
        let answer = call(
            &scene,
            thread,
            turn,
            reference,
            OrchTool::OrchDocs,
            json!({"query":query}),
        )
        .await;
        assert!(answer["error"].is_null(), "{answer}");
        let result = answer["result"].to_string();
        assert!(
            result.contains("/help/") && result.contains('#'),
            "article and heading: {result}"
        );
    }
    let refused = call(
        &scene,
        thread,
        turn,
        "off",
        OrchTool::Read,
        read_args(thread),
    )
    .await;
    assert!(refused["error"].is_object(), "{refused}");
    scene
        .rig
        .world
        .set_at(Target::Node(thread), "orch_tools.owner_reads", json!(true))
        .await;
    let allowed = call(
        &scene,
        thread,
        turn,
        "on",
        OrchTool::Read,
        read_args(thread),
    )
    .await;
    assert!(allowed["error"].is_null(), "{allowed}");
    let inputs_before: i64 =
        sqlx::query_scalar("SELECT count(*) FROM thread_inputs WHERE thread_id=$1")
            .bind(thread)
            .fetch_one(scene.rig.db.pool())
            .await
            .unwrap();
    let write=call(&scene,thread,turn,"write-is-not-read",OrchTool::Read,json!({"operation":"ThreadInputCreate","parameters":{"path":{"id":thread},"body":{"text":"Never send this"}}})).await;
    assert!(write["error"].is_object(), "{write}");
    let inputs_after: i64 =
        sqlx::query_scalar("SELECT count(*) FROM thread_inputs WHERE thread_id=$1")
            .bind(thread)
            .fetch_one(scene.rig.db.pool())
            .await
            .unwrap();
    assert_eq!(
        inputs_after, inputs_before,
        "the derived Read allowlist never runs a creation"
    );
    scene
        .rig
        .world
        .set_at(Target::Node(thread), "orch_tools.owner_reads", json!(false))
        .await;
    let revoked = call(
        &scene,
        thread,
        turn,
        "off-again",
        OrchTool::Read,
        read_args(thread),
    )
    .await;
    assert!(revoked["error"].is_object(), "{revoked}");
    let counted:i64=sqlx::query_scalar("SELECT count(*) FROM thread_tool_calls WHERE thread_id=$1 AND plan#>>'{owner_read,completed}'='true'")
        .bind(thread).fetch_one(scene.rig.db.pool()).await.unwrap();
    assert_eq!(
        counted, 0,
        "docs, refusals and an unpublished candidate are not counted results"
    );
}

#[tokio::test]
async fn sharing_requires_the_current_result_notice_keeps_raw_history_and_refuses_the_next_read() {
    let (scene, thread, turn) = setup(true).await;
    let change = share(scene.cai.id());
    let empty = scene
        .rig
        .world
        .app
        .call(SharePreviewCreate, &[&"nodes", &thread])
        .token(&scene.ada.token)
        .raw_body(&json!({"kind":"page","change":change}))
        .send()
        .await
        .ok();
    assert!(empty.owner_reads.is_none());
    let answer = call(
        &scene,
        thread,
        turn,
        "personal",
        OrchTool::Read,
        read_args(thread),
    )
    .await;
    assert!(answer["error"].is_null(), "{answer}");
    let published = publish(&scene, thread, "personal", answer.clone()).await;
    assert_eq!(published["result"], answer["result"]);
    retained_history(&scene, thread, turn, &answer["result"]).await;
    let preview = scene
        .rig
        .world
        .app
        .call(SharePreviewCreate, &[&"nodes", &thread])
        .token(&scene.ada.token)
        .raw_body(&json!({"kind":"page","change":change}))
        .send()
        .await
        .ok();
    let disclosure = preview.owner_reads.unwrap();
    assert_eq!((disclosure.count, disclosure.threads), (1, 1));
    let before = scene
        .rig
        .world
        .app
        .call(ThreadItemList, &[&thread])
        .token(&scene.cai.token)
        .send()
        .await;
    assert!(!before.status.is_success());
    let refused = scene
        .rig
        .world
        .app
        .call(PermissionPageUpdate, &[&"nodes", &thread])
        .token(&scene.ada.token)
        .raw_body(&change)
        .send()
        .await
        .problem();
    assert_eq!(refused.code, ErrorCode::ConflictConfirmationRequired);
    let current: OwnerReadDisclosure =
        serde_json::from_value(refused.extra["owner_reads"].clone()).unwrap();
    assert_eq!(current, disclosure);
    assert!(
        eligible(&scene, thread).await,
        "an unconfirmed share applied nothing"
    );
    scene
        .rig
        .world
        .app
        .call(PermissionPageUpdate, &[&"nodes", &thread])
        .token(&scene.ada.token)
        .header("orch-owner-reads-confirmed", &disclosure.confirmation)
        .raw_body(&change)
        .send()
        .await
        .ok();
    let visible = scene
        .rig
        .world
        .app
        .call(ThreadItemList, &[&thread])
        .token(&scene.cai.token)
        .send()
        .await
        .ok();
    let expected = answer["result"].to_string();
    assert!(
        visible
            .items
            .iter()
            .any(|item| matches!(&item.payload,ItemPayload::ToolCall(body)
        if body.raw.output.as_ref().is_some_and(|output|output.inline==expected))),
        "confirmed sharing keeps the raw result in ordinary history"
    );
    let next = call(
        &scene,
        thread,
        turn,
        "after-sharing",
        OrchTool::Read,
        read_args(thread),
    )
    .await;
    assert_eq!(next["error"]["code"], "not_yours");
    for name in ["list_shared", "read_shared", "search_shared"] {
        assert!(next["error"]["message"].as_str().unwrap().contains(name));
    }
    assert_eq!(
        publish(&scene, thread, "personal", answer.clone()).await["result"],
        answer["result"],
        "accepted same-call material is retained"
    );
}

#[tokio::test]
async fn empty_groups_and_future_agent_and_link_audiences_are_shared_grants() {
    let (scene, thread, turn) = setup(true).await;
    let group = scene
        .rig
        .world
        .app
        .call(OrgGroupCreate, &[&scene.tree.org.base.id])
        .token(&scene.ada.token)
        .raw_body(&json!({"name":"Empty readers"}))
        .send()
        .await
        .ok()
        .into_full()
        .unwrap();
    for (nth, holder) in [
        json!({"kind":"group","id":group.id}),
        json!({"kind":"agents","id":scene.tree.topic.base.id}),
        json!({"kind":"members","id":scene.tree.org.base.id}),
        json!({"kind":"user","id":scene.cai.id()}),
        json!({"kind":"role","at":scene.tree.topic.base.id,"role":"viewer"}),
    ]
    .into_iter()
    .enumerate()
    {
        scene
            .rig
            .world
            .app
            .call(PermissionPageUpdate, &[&"nodes", &thread])
            .token(&scene.ada.token)
            .raw_body(&json!({"whole":"private"}))
            .send()
            .await
            .ok();
        assert!(eligible(&scene, thread).await);
        scene
            .rig
            .world
            .app
            .call(PermissionPageUpdate, &[&"nodes", &thread])
            .token(&scene.ada.token)
            .raw_body(&json!({"rows":[{"permission":"thread.view","add":[{"holder":holder}]}]}))
            .send()
            .await
            .ok();
        assert!(
            !eligible(&scene, thread).await,
            "the grant itself is shared, regardless of its current members"
        );
        let next = call(
            &scene,
            thread,
            turn,
            &format!("non-owner-grant-{nth}"),
            OrchTool::Read,
            read_args(thread),
        )
        .await;
        assert_eq!(next["error"]["code"], "not_yours", "{next}");
    }
}

#[tokio::test]
async fn a_share_between_reading_and_command_publication_withholds_the_first_personal_result() {
    let (scene, thread, turn) = setup(true).await;
    let answer = call(
        &scene,
        thread,
        turn,
        "candidate",
        OrchTool::Read,
        read_args(thread),
    )
    .await;
    assert!(answer["error"].is_null(), "{answer}");
    scene
        .rig
        .world
        .app
        .call(PermissionPageUpdate, &[&"nodes", &thread])
        .token(&scene.ada.token)
        .raw_body(&share(scene.cai.id()))
        .send()
        .await
        .ok();
    let narrowed = publish(&scene, thread, "candidate", answer).await;
    assert!(narrowed["result"].is_null());
    assert_eq!(narrowed["error"]["code"], "not_yours");
    let count:i64=sqlx::query_scalar("SELECT count(*) FROM thread_tool_calls WHERE thread_id=$1 AND plan#>>'{owner_read,completed}'='true'")
        .bind(thread).fetch_one(scene.rig.db.pool()).await.unwrap();
    assert_eq!(count, 0);
}

#[tokio::test]
async fn sharing_confirmation_uses_a_new_http_key_and_never_repeats_native_creation() {
    let (scene, thread, turn) = setup(true).await;
    let answer = call(
        &scene,
        thread,
        turn,
        "before-keyed-share",
        OrchTool::Read,
        read_args(thread),
    )
    .await;
    assert!(answer["error"].is_null(), "{answer}");
    publish(&scene, thread, "before-keyed-share", answer).await;
    let recipient = scene.thread().await.base.id;
    let key = orch_types::new_id().to_string();
    let body = json!({"client_id":key,"what":{"node_id":thread},"mode":"live","to":[{"thread_id":recipient}],"parts":{"timeline":"read"}});
    let refused = scene
        .rig
        .world
        .app
        .call(ShareAgentCreate, &[])
        .token(&scene.ada.token)
        .header("idempotency-key", &key)
        .raw_body(&body)
        .send()
        .await
        .problem();
    assert_eq!(refused.code, ErrorCode::ConflictConfirmationRequired);
    let review: OwnerReadDisclosure =
        serde_json::from_value(refused.extra["owner_reads"].clone()).unwrap();
    assert!(
        eligible(&scene, thread).await,
        "the refused native transaction committed no reader"
    );
    let confirmed_key = orch_types::new_id().to_string();
    let made = scene
        .rig
        .world
        .app
        .call(ShareAgentCreate, &[])
        .token(&scene.ada.token)
        .header("idempotency-key", &confirmed_key)
        .header("orch-owner-reads-confirmed", &review.confirmation)
        .raw_body(&body)
        .send()
        .await;
    assert!(
        made.status.is_success(),
        "a confirmed request has a new HTTP key and the original native client_id: {:?}",
        made.body
    );
    let replay = scene
        .rig
        .world
        .app
        .call(ShareAgentCreate, &[])
        .token(&scene.ada.token)
        .header("idempotency-key", &confirmed_key)
        .header("orch-owner-reads-confirmed", &review.confirmation)
        .raw_body(&body)
        .send()
        .await;
    assert!(replay.status.is_success(), "{:?}", replay.body);
    assert_eq!(
        replay.raw, made.raw,
        "the accepted receipt and body replay unchanged"
    );
    let original = scene
        .rig
        .world
        .app
        .call(ShareAgentCreate, &[])
        .token(&scene.ada.token)
        .header("idempotency-key", &key)
        .header("orch-owner-reads-confirmed", &review.confirmation)
        .raw_body(&body)
        .send()
        .await
        .problem();
    assert_eq!(
        original.code,
        ErrorCode::ConflictConfirmationRequired,
        "the first key retains its refusal even after the other request succeeds"
    );
    let another_key = orch_types::new_id().to_string();
    let accepted_again = scene
        .rig
        .world
        .app
        .call(ShareAgentCreate, &[])
        .token(&scene.ada.token)
        .header("idempotency-key", &another_key)
        .header("orch-owner-reads-confirmed", &review.confirmation)
        .raw_body(&body)
        .send()
        .await;
    assert!(
        accepted_again.status.is_success(),
        "{:?}",
        accepted_again.body
    );
    let first: Json = serde_json::from_slice(&made.raw).unwrap();
    let again: Json = serde_json::from_slice(&accepted_again.raw).unwrap();
    let first_id = first["shares"][0]["id"]
        .as_str()
        .expect("the accepted native share has a real identity");
    assert!(first_id.parse::<Id>().is_ok());
    assert_eq!(
        again["shares"][0]["id"], first_id,
        "a new HTTP key with the same client_id identifies the original native share"
    );
    let gifts:i64=sqlx::query_scalar("SELECT count(*) FROM permission_entries WHERE object_kind='node' AND object_id=$1 AND page='live' AND permission='thread.view' AND holder_kind='agents' AND holder_id=$2 AND removed_at IS NULL")
        .bind(thread).bind(recipient).fetch_one(scene.rig.db.pool()).await.unwrap();
    assert_eq!(gifts, 1, "a retry never adds a second native gift");
}
