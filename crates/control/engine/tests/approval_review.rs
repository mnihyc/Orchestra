//! Native reviewer facts are visible and charged once; an unproved fake harness still cannot open Auto.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]
mod common;

use common::{World, eventually, queued};
use orch_agent_contract::events::TokenUsage;
use orch_authz::Caller;
use orch_link_contract::commands::CommandKind;
use orch_link_contract::events::Event;
use orch_settings_contract::registry::keys;
use orch_types::Json;
use serde_json::json;

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_reported_denial_is_a_visible_item_with_its_model_reason_and_separate_use_once() {
    let world = World::new().await;
    let thread = world.thread("Reviewer facts").await;
    let (start, _) = world.running_turn(thread, "Work").await;
    let events =
        orch_agent_contract::review::reported(orch_agent_contract::review::ReportedReview {
            native_ref: "approval-review:r1".to_owned(),
            turn: Some(start.turn_id),
            at: world.clock.time(),
            model: "codex-auto-review",
            model_reported: true,
            outcome: "denied",
            reason: Some("The action would send private files away"),
            action: json!({"command":"upload"}),
            usage: Some(TokenUsage {
                input_tokens: 10,
                output_tokens: 5,
                total_tokens: 15,
                ..TokenUsage::default()
            }),
            raw: &json!({"reviewId":"r1"}),
        });
    for event in &events {
        let wire = serde_json::to_value(event).unwrap();
        world
            .say(
                thread,
                Event::decode(wire["kind"].as_str().unwrap(), wire["body"].clone()).unwrap(),
            )
            .await;
    }
    // Publishing to the connector's spool does not await the engine's committed projection.
    eventually("the native denial to be visible as a completed thread item", || async {
        world.count("SELECT count(*) FROM items WHERE thread_id=$1 AND native_ref='approval-review:r1' AND status='completed'",thread).await == 1
    }).await;
    let item: Json = world
        .scalar(
            "SELECT payload FROM items WHERE thread_id=$1 AND native_ref='approval-review:r1'",
            thread,
        )
        .await;
    assert_eq!(
        item.pointer("/tool/result/model"),
        Some(&json!("codex-auto-review"))
    );
    assert_eq!(
        item.pointer("/tool/result/reason"),
        Some(&json!("The action would send private files away"))
    );
    let reviewer = orch_engine::approval_review::session_reviewer(
        &mut world.pool.acquire().await.unwrap(),
        thread,
    )
    .await
    .unwrap()
    .unwrap();
    assert_eq!(reviewer.model, "codex-auto-review");
    assert!(reviewer.reported && reviewer.usage_apart);
    assert_eq!(world.ran(CommandKind::ApprovalResolve), 0);
    let approvals: i64 = world
        .scalar("SELECT count(*) FROM approvals WHERE thread_id=$1", thread)
        .await;
    assert_eq!(
        approvals, 0,
        "a denied native review does not ask somebody to approve it"
    );
    let segment = world.segment_of(thread).await;
    let stream = orch_link_contract::streams::Stream::Session {
        thread_id: thread,
        segment_id: segment,
    }
    .to_string();
    let before =
        orch_db::queries::link::cursors(&mut world.pool.acquire().await.unwrap(), world.machine_id)
            .await
            .unwrap()
            .get(&stream)
            .copied()
            .unwrap_or(0);
    for event in &events {
        let wire = serde_json::to_value(event).unwrap();
        world
            .say(
                thread,
                Event::decode(wire["kind"].as_str().unwrap(), wire["body"].clone()).unwrap(),
            )
            .await;
    }
    let replayed = before + i64::try_from(events.len()).unwrap();
    eventually(
        "both replayed review events to commit before checking that usage was counted once",
        || async {
            orch_db::queries::link::cursors(
                &mut world.pool.acquire().await.unwrap(),
                world.machine_id,
            )
            .await
            .unwrap()
            .get(&stream)
            .copied()
            .unwrap_or(0)
                >= replayed
        },
    )
    .await;
    assert_eq!(
        world
            .count(
                "SELECT count(*) FROM items WHERE thread_id=$1 AND native_ref='approval-review:r1'",
                thread
            )
            .await,
        1
    );
    let shown = world.sync(thread).await;
    assert!(shown.items.iter().any(|item| match &item.payload {
        orch_api_contract::threads::ItemPayload::ToolCall(tool) => tool.title == "Permission review: denied"
            && matches!(&tool.tool, orch_api_contract::threads::ToolPayload::Other { result: Some(result), .. }
                if result["model"] == "codex-auto-review" && result["outcome"] == "denied" && result["reason"] == "The action would send private files away"),
        _ => false,
    }),"the person receives the denial, model and reason in the thread projection");
    let use_: (i64,i64)=sqlx::query_as("SELECT count(*)::bigint,COALESCE(sum(total_tokens),0)::bigint FROM usage_events WHERE thread_id=$1 AND purpose='review' AND model='codex-auto-review'")
        .bind(thread).fetch_one(world.pool.pool()).await.unwrap();
    assert_eq!(use_, (1, 15));
    let totals:i64=world.scalar("SELECT COALESCE(sum(total_tokens),0)::bigint FROM usage_daily WHERE thread_id=$1 AND purpose='review'",thread).await;
    assert_eq!(totals, 15);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn auto_is_refused_before_opening_a_session_for_an_unproved_harness() {
    let world = World::new().await;
    let thread = world.thread("Auto not proved").await;
    world.set(thread, keys::APPROVAL_MODE, json!("auto")).await;
    world
        .engine()
        .submit_input(&Caller::session(world.ada), thread, queued("Work"))
        .await
        .unwrap();
    world.wait_state(thread, "blocked").await;
    assert_eq!(world.ran(CommandKind::SessionOpen), 0);
    let blocked: String = world
        .scalar(
            "SELECT blocked_reason FROM threads WHERE node_id=$1",
            thread,
        )
        .await;
    assert_eq!(blocked, "binding_invalid");
    let detail: String = world
        .scalar(
            "SELECT blocked_detail FROM threads WHERE node_id=$1",
            thread,
        )
        .await;
    assert_eq!(detail, "Auto: No Auto check is available for this program.");
    assert!(!detail.contains("does not support"));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn auto_names_the_unrun_or_failed_build_check_before_opening_any_native_session() {
    // A machine a person connected is checked on a build host of the system; only with none online does Auto say
    // that a build host is missing.
    for (has_node, build_host, failed, detail) in [
        (true, false, false, "Auto: Not proven on this machine yet."),
        (
            false,
            false,
            false,
            "No build host is online to run the Auto approval check.",
        ),
        (false, true, false, "Auto: Not proven on this machine yet."),
        (
            false,
            false,
            true,
            "Auto: Failed its check on this machine.",
        ),
    ] {
        // The provider's first scheduled read would start the harness with the world's first declaration and can
        // save that answer after this case's own; the read of an observed world starts nothing.
        let world = World::observed().await;
        let thread = world.thread("Auto check state").await;
        let mut descriptor = common::descriptor();
        descriptor["kind"] = json!("codex");
        descriptor["approval_modes"] = json!(["on_request", "auto"]);
        world.declares(descriptor.clone()).await;
        sqlx::query("UPDATE harnesses SET descriptor=$2,harness_version='1.0.0' WHERE agent_id=$1")
            .bind(world.agent)
            .bind(descriptor)
            .execute(world.pool.pool())
            .await
            .unwrap();
        sqlx::query("UPDATE agents SET harness_kind='codex' WHERE id=$1")
            .bind(world.agent)
            .execute(world.pool.pool())
            .await
            .unwrap();
        sqlx::query("UPDATE machines SET placement='connected',roles=array_remove(roles,'node') || $3::text[],facts=facts || $2 WHERE id=$1")
            .bind(world.machine_id)
            .bind(json!({"policy":{"agent.harnesses":[{"kind":"codex","path":"/selected/codex"}]},
                "harnesses":[{"kind":"codex","path":"/selected/codex","version":"1.0.0","sha256":"c".repeat(64)}]}))
            .bind(if has_node { vec!["node"] } else { vec![] })
            .execute(world.pool.pool()).await.unwrap();
        let check = failed.then(|| {
            json!({"at":world.clock.time(),"machine":"test machine","missing":[],
            "off":["approval_modes.auto"],"error":null,"stage":"complete","cases":[{
                "name":"auto_review","feature":"approval_modes.auto","passed":false,
                "error":"The scripted reviewer did not answer","unsupported":null,"elapsed_ms":1}]})
        });
        let mut conn = world.pool.acquire().await.unwrap();
        orch_db::queries::resources::management::put_channel_build(&mut conn, "harness_channel:auto-choice", &json!({
            "kind":"codex","version":"1.0.0","os":"linux","arch":"x86_64",
            "url":"https://example.com/program","sha256":"c".repeat(64),"is_default":false,"source":"channel",
            "status":if failed {"failed"} else {"untested"},"check":check
        }),world.clock.now()).await.unwrap();
        drop(conn);
        if build_host {
            let mut conn = world.pool.acquire().await.unwrap();
            let host = orch_db::queries::resources::access_fixtures::insert_resource(
                &mut conn,
                "host",
                None,
                None,
                "check-host",
            )
            .await
            .unwrap();
            sqlx::query("UPDATE machines SET online=true WHERE id=(SELECT machine_id FROM hosts WHERE id=$1)")
                .bind(host)
                .execute(&mut *conn)
                .await
                .unwrap();
        }
        world.set(thread, keys::APPROVAL_MODE, json!("auto")).await;
        world
            .engine()
            .submit_input(&Caller::session(world.ada), thread, queued("Work"))
            .await
            .unwrap();
        world.wait_blocked(thread, "binding_invalid").await;
        let actual: String = world
            .scalar(
                "SELECT blocked_detail FROM threads WHERE node_id=$1",
                thread,
            )
            .await;
        assert_eq!(actual, detail);
        assert_eq!(world.ran(CommandKind::SessionOpen), 0);
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_approved_native_review_is_a_visible_factual_card_without_a_person_request() {
    let world = World::new().await;
    let thread = world.thread("Approved review facts").await;
    let (start, _) = world.running_turn(thread, "Work").await;
    let events =
        orch_agent_contract::review::reported(orch_agent_contract::review::ReportedReview {
            native_ref: "approval-review:approved".into(),
            turn: Some(start.turn_id),
            at: world.clock.time(),
            model: "codex-auto-review",
            model_reported: false,
            outcome: "approved",
            reason: Some("The action stays inside the workspace"),
            action: json!({"command":"build"}),
            usage: None,
            raw: &json!({"reviewId":"approved"}),
        });
    for event in events.iter().chain(events.iter()) {
        let wire = serde_json::to_value(event).unwrap();
        world
            .say(
                thread,
                Event::decode(wire["kind"].as_str().unwrap(), wire["body"].clone()).unwrap(),
            )
            .await;
    }
    eventually("the native approval card",||async{world.count("SELECT count(*) FROM items WHERE thread_id=$1 AND native_ref='approval-review:approved' AND status='completed'",thread).await==1}).await;
    let payload:Json=world.scalar("SELECT payload FROM items WHERE thread_id=$1 AND native_ref='approval-review:approved'",thread).await;
    assert_eq!(
        payload.pointer("/tool/result/outcome"),
        Some(&json!("approved"))
    );
    assert_eq!(
        payload.pointer("/tool/result/reported"),
        Some(&json!(false))
    );
    assert!(payload.pointer("/tool/result/usage").unwrap().is_null());
    assert_eq!(
        world
            .count("SELECT count(*) FROM approvals WHERE thread_id=$1", thread)
            .await,
        0
    );
    assert_eq!(world.ran(CommandKind::ApprovalResolve), 0);
    assert_eq!(world.count("SELECT count(*) FROM usage_events WHERE turn_id=$1 AND request_ref='approval-review:approved'",start.turn_id).await,0,"an unreported separate use stays unknown");
}
