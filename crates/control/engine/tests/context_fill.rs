//! What a harness reports beyond a turn's totals is kept with the turn whole; where it breaks nothing down, Orch estimates what filled the context at the turn's end, its parts adding up to what the harness reported; where it breaks it down itself, its breakdown is kept and no estimate replaces it.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]

mod common;

use common::{World, meanwhile, turn_ended};
use orch_api_contract::context::{
    ContextCount, ContextFill, ContextPart, ContextPartKind, ContextShortening, StreamTokens,
    TurnUsageDetail, TurnUsageReport,
};
use orch_link_contract::events::{Event, TurnOutcome, UsageUpdated};
use orch_types::{Id, Json, Time};
use serde_json::json;
use std::time::Duration;

/// How long a test waits for what the engine applies after a machine said it.
const PATIENCE: Duration = Duration::from_secs(20);
const STEP: Duration = Duration::from_millis(100);

fn tokens(input: i64, cached: i64, output: i64) -> Json {
    json!({ "input_tokens": input, "cached_input_tokens": cached, "output_tokens": output,
            "reasoning_tokens": 0, "total_tokens": input + output })
}

fn at() -> Time {
    Time::new(chrono::DateTime::from_timestamp(1_791_700_000, 0).unwrap())
}

fn reported(turn: Id, prompt: i64, report: TurnUsageReport) -> Event {
    Event::UsageUpdated(UsageUpdated {
        turn_id: turn,
        turn: serde_json::from_value(tokens(prompt, prompt / 2, 300)).unwrap(),
        thread: serde_json::from_value(tokens(prompt, prompt / 2, 300)).unwrap(),
        context_window: Some(258_400),
        context_used: Some(prompt),
        report: Some(report),
    })
}

/// The turn's two columns, once `ready` says they are what the test waits for.
async fn columns_of(
    world: &World,
    turn: Id,
    ready: impl Fn(&Option<Json>, &Option<Json>) -> bool,
) -> (Option<Json>, Option<Json>) {
    let waited = tokio::time::timeout(PATIENCE, async {
        loop {
            let (detail, context): (Option<Json>, Option<Json>) =
                sqlx::query_as("SELECT usage_detail, context FROM turns WHERE id = $1")
                    .bind(turn)
                    .fetch_one(world.pool.pool())
                    .await
                    .unwrap();
            if ready(&detail, &context) {
                return (detail, context);
            }
            meanwhile(STEP).await;
        }
    })
    .await;
    waited.expect("the engine kept what was reported in time")
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_report_is_kept_whole_and_orch_estimates_what_filled_a_context_the_harness_did_not_break_down()
 {
    let world = World::new().await;
    let thread = world.thread("Estimated").await;
    let (start, _) = world.running_turn(thread, "Fix the login redirect").await;
    let share = serde_json::from_value(tokens(2_000, 0, 500)).unwrap();
    let report = TurnUsageReport {
        api_time_ms: None,
        subagents: Some(share),
        by_subagent: vec![StreamTokens {
            stream: "sub:helper".to_owned(),
            tokens: share,
        }],
        models: Vec::new(),
        compact_at: Some(244_800),
        shortened: Some(ContextShortening {
            at: at(),
            before: Some(200_000),
            after: Some(40_000),
            automatic: None,
        }),
        context: None,
        breakdown: false,
    };
    world
        .say(thread, reported(start.turn_id, 40_000, report))
        .await;
    world
        .say(thread, turn_ended(start.turn_id, TurnOutcome::Completed))
        .await;
    world.wait_turn(start.turn_id, "completed").await;
    let (detail, context) = columns_of(&world, start.turn_id, |detail, context| {
        detail.is_some() && context.is_some()
    })
    .await;

    let detail: TurnUsageDetail = serde_json::from_value(detail.unwrap()).unwrap();
    assert_eq!(
        detail.subagents.map(|share| share.total_tokens),
        Some(2_500),
        "the sub-agents' share, apart"
    );
    assert_eq!(detail.by_subagent[0].stream, "sub:helper");
    assert_eq!(
        (
            detail.context_used,
            detail.context_window,
            detail.compact_at
        ),
        (Some(40_000), Some(258_400), Some(244_800))
    );
    assert_eq!(detail.shortenings.len(), 1);
    assert!(!detail.breakdown);

    let fill: ContextFill = serde_json::from_value(context.unwrap()).unwrap();
    assert_eq!(fill.count, ContextCount::Orch, "estimated by Orch");
    assert_eq!(fill.used, 40_000, "the size the harness reported");
    assert_eq!(fill.parts_total(), fill.used, "the parts add up to it");
    assert!(
        fill.parts
            .iter()
            .all(|part| part.count == ContextCount::Orch)
    );
    assert!(
        fill.parts
            .iter()
            .any(|part| part.kind == ContextPartKind::Harness),
        "what Orch neither sent nor saw is the harness's own: {:?}",
        fill.parts
    );

    // The thread keeps where its harness shortens the conversation.
    let compact_at: Option<i64> =
        sqlx::query_scalar("SELECT (usage->>'compact_at')::bigint FROM threads WHERE node_id = $1")
            .bind(thread)
            .fetch_one(world.pool.pool())
            .await
            .unwrap();
    assert_eq!(compact_at, Some(244_800));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_harness_s_own_breakdown_is_kept_and_no_estimate_of_orch_s_replaces_it() {
    let world = World::new().await;
    let thread = world.thread("Broken down").await;
    let (start, _) = world.running_turn(thread, "Look at the logs").await;
    let fill = ContextFill {
        count: ContextCount::Estimated,
        used: 30_000,
        window: Some(200_000),
        compact_at: Some(167_000),
        parts: vec![
            ContextPart {
                kind: ContextPartKind::Harness,
                tokens: 18_000,
                count: ContextCount::Estimated,
                rows: Vec::new(),
            },
            ContextPart {
                kind: ContextPartKind::ToolCalls,
                tokens: 12_000,
                count: ContextCount::Estimated,
                rows: Vec::new(),
            },
        ],
        scaled: false,
        at: at(),
    };
    let report = TurnUsageReport {
        compact_at: Some(167_000),
        breakdown: true,
        ..TurnUsageReport::default()
    };
    world
        .say(thread, reported(start.turn_id, 30_000, report))
        .await;
    world
        .say(thread, turn_ended(start.turn_id, TurnOutcome::Completed))
        .await;
    world.wait_turn(start.turn_id, "completed").await;
    // The breakdown comes after the end of the turn, carried with the turn's last use.
    let carried = TurnUsageReport {
        compact_at: Some(167_000),
        context: Some(fill),
        breakdown: true,
        ..TurnUsageReport::default()
    };
    world
        .say(thread, reported(start.turn_id, 30_000, carried))
        .await;
    let (_, context) = columns_of(&world, start.turn_id, |_, context| {
        context
            .as_ref()
            .is_some_and(|context| context["count"] == "estimated")
    })
    .await;
    let kept: ContextFill = serde_json::from_value(context.unwrap()).unwrap();
    assert_eq!(kept.used, 30_000);
    assert_eq!(
        kept.parts_total(),
        30_000,
        "the harness's parts as it gave them"
    );
    assert!(kept.parts.iter().any(
        |part| part.kind == ContextPartKind::ToolCalls && part.count == ContextCount::Estimated
    ));
    // A while later, nothing of Orch's has taken its place.
    meanwhile(Duration::from_millis(500)).await;
    let count: String = sqlx::query_scalar("SELECT context->>'count' FROM turns WHERE id = $1")
        .bind(start.turn_id)
        .fetch_one(world.pool.pool())
        .await
        .unwrap();
    assert_eq!(count, "estimated");
}
