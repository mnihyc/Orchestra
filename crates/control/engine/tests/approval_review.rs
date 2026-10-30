//! Native reviewer facts are visible and charged once; Auto admission stays closed before the real trial.
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
async fn auto_is_refused_before_opening_a_session_until_the_integrators_trial() {
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
}
