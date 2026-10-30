//! What fills a thread's context, what its turns used and what a turn was counted at, over HTTP: the harness's report as the turn kept it, Orch's estimate where the harness broke nothing down, a page of turns with their use, and nothing of it for whoever may not read the thread.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]

mod places;
mod rig;
mod support;

use orch_api_contract::context::{
    ContextCount, ContextShortening, ThreadContext, ThreadContextGet, ThreadContextGetQuery,
    ThreadTurnCostGet, ThreadTurnUsage, ThreadTurnUsageList, ThreadTurnUsageListQuery, TurnCost,
    TurnUsageReport,
};
use orch_api_contract::notifications::{PersonNoticeAnchor, PersonNotifyBody, PersonNotifyCreate};
use orch_api_contract::threads::Delivery;
use orch_link_contract::events::{Event, TurnOutcome, UsageUpdated};
use orch_types::{Id, Time, new_id};
use rig::{Scene, turn_ended};
use serde_json::json;

fn usage(turn: Id) -> Event {
    let tokens = json!({ "input_tokens": 30_000, "cached_input_tokens": 20_000, "output_tokens": 500,
                         "reasoning_tokens": 100, "total_tokens": 30_500 });
    Event::UsageUpdated(UsageUpdated {
        turn_id: turn,
        turn: serde_json::from_value(tokens.clone()).unwrap(),
        thread: serde_json::from_value(tokens).unwrap(),
        context_window: Some(258_400),
        context_used: Some(30_000),
        report: Some(TurnUsageReport {
            subagents: Some(
                serde_json::from_value(json!({ "input_tokens": 1_000, "cached_input_tokens": 0,
                "output_tokens": 100, "reasoning_tokens": 0, "total_tokens": 1_100 }))
                .unwrap(),
            ),
            compact_at: Some(244_800),
            shortened: Some(ContextShortening {
                at: Time::new(chrono::DateTime::from_timestamp(1_791_700_000, 0).unwrap()),
                before: Some(240_000),
                after: Some(30_000),
                automatic: Some(true),
            }),
            ..TurnUsageReport::default()
        }),
    })
}

/// Ada's thread with one turn that ended after its harness reported its use.
async fn thread_with_a_turn(s: &Scene) -> (Id, Id) {
    let thread = s.thread().await.base.id;
    let sent = s
        .say(
            &s.ada,
            thread,
            "Fix the login",
            Delivery::Queue,
            "context-0",
        )
        .await;
    assert!(sent.status.is_success(), "{:?}", sent.body);
    let start = s.rig.turn_start(thread, 0).await;
    s.rig.answer(thread, &start, "m0", "Done").await;
    s.rig.say(thread, usage(start.turn_id)).await;
    s.rig
        .say(thread, turn_ended(start.turn_id, TurnOutcome::Completed))
        .await;
    s.rig.wait_turn(start.turn_id, "completed").await;
    (thread, start.turn_id)
}

#[tokio::test]
async fn an_explicit_person_notice_opens_the_named_thread_turn() {
    let s = Scene::new().await;
    let (thread, turn) = thread_with_a_turn(&s).await;
    let (ordinal, continuation): (i32, Id) =
        sqlx::query_as("SELECT ordinal, continuation_id FROM turns WHERE thread_id=$1 AND id=$2")
            .bind(thread)
            .bind(turn)
            .fetch_one(&s.rig.world.db.pool)
            .await
            .unwrap();
    let body = PersonNotifyBody {
        user_ids: vec![s.cai.id()],
        anchor: Some(PersonNoticeAnchor::Turn(turn)),
        note: None,
    };
    s.rig
        .world
        .app
        .call(PersonNotifyCreate, &[&"nodes", &thread])
        .token(&s.ada.token)
        .body(&body)
        .send()
        .await
        .ok();
    let (path, stored_turn): (String, String) = sqlx::query_as(
        "SELECT data->>'path', data->>'turn_id' FROM notifications WHERE user_id=$1 AND kind='person.notify'",
    )
    .bind(s.cai.id())
    .fetch_one(&s.rig.world.db.pool)
    .await
    .unwrap();
    assert_eq!(
        path,
        format!("/thread/{thread}?continuation={continuation}&turn={ordinal}")
    );
    assert_eq!(stored_turn, turn.to_string());
    let invalid = PersonNotifyBody {
        anchor: Some(PersonNoticeAnchor::Turn(new_id())),
        ..body
    };
    let refused = s
        .rig
        .world
        .app
        .call(PersonNotifyCreate, &[&"nodes", &thread])
        .token(&s.ada.token)
        .body(&invalid)
        .send()
        .await;
    assert!(refused.status.is_client_error());
    let count: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM notifications WHERE user_id=$1 AND kind='person.notify'",
    )
    .bind(s.cai.id())
    .fetch_one(&s.rig.world.db.pool)
    .await
    .unwrap();
    assert_eq!(count, 1);
}

async fn context(
    s: &Scene,
    by: &places::Who,
    thread: Id,
) -> orch_api_core::testing::Reply<ThreadContext> {
    s.rig
        .world
        .app
        .call(ThreadContextGet, &[&thread])
        .token(&by.token)
        .query(&ThreadContextGetQuery::default())
        .send()
        .await
}

#[tokio::test]
async fn a_reader_of_the_thread_reads_what_filled_its_context_and_what_each_turn_used() {
    let s = Scene::new().await;
    let (thread, turn) = thread_with_a_turn(&s).await;

    let read = rig::eventually("the estimate at the end of the turn was kept", || async {
        context(&s, &s.ada, thread)
            .await
            .body
            .is_ok_and(|context| context.fill.is_some())
    });
    read.await;
    let kept = context(&s, &s.ada, thread).await.body.unwrap();
    let fill = kept.fill.expect("a fill");
    assert_eq!(kept.turn_id, Some(turn));
    assert_eq!(
        fill.count,
        ContextCount::Orch,
        "the harness broke nothing down: Orch estimated it"
    );
    assert_eq!(fill.used, 30_000);
    assert_eq!(fill.parts_total(), 30_000);
    assert_eq!(fill.compact_at, Some(244_800));
    let shortened = kept.shortened.expect("the shortening");
    assert_eq!(
        (shortened.before, shortened.after),
        (Some(240_000), Some(30_000))
    );
    assert!(
        !kept.can_count,
        "a harness that breaks nothing down is not asked to count"
    );

    let page: ThreadTurnUsage = s
        .rig
        .world
        .app
        .call(ThreadTurnUsageList, &[&thread])
        .token(&s.ada.token)
        .query(&ThreadTurnUsageListQuery::default())
        .send()
        .await
        .body
        .unwrap();
    assert_eq!(page.turns.len(), 1);
    assert_eq!(page.next_before, None);
    let row = &page.turns[0];
    assert_eq!((row.turn_id, row.ordinal), (turn, 1));
    assert_eq!(row.usage.map(|usage| usage.total_tokens), Some(30_500));
    let detail = row
        .detail
        .as_ref()
        .expect("what the harness reported beyond the totals");
    assert_eq!(
        detail.subagents.map(|share| share.total_tokens),
        Some(1_100)
    );
    assert_eq!(detail.shortenings.len(), 1);

    let cost: TurnCost = s
        .rig
        .world
        .app
        .call(ThreadTurnCostGet, &[&thread, &turn])
        .token(&s.ada.token)
        .send()
        .await
        .body
        .unwrap();
    assert_eq!(cost.turn_id, turn);
    let unknown = s
        .rig
        .world
        .app
        .call(ThreadTurnCostGet, &[&thread, &orch_types::new_id()])
        .token(&s.ada.token)
        .send()
        .await;
    assert_eq!(
        unknown.status.as_u16(),
        404,
        "a turn the thread does not have"
    );
}

#[tokio::test]
async fn somebody_who_may_not_read_the_thread_reads_nothing_of_its_context_or_its_turns() {
    let s = Scene::new().await;
    let (thread, turn) = thread_with_a_turn(&s).await;
    let stranger = s.rig.person("gus").await;
    let read = context(&s, &stranger, thread).await;
    assert!(read.status.is_client_error(), "{:?}", read.status);
    let turns = s
        .rig
        .world
        .app
        .call(ThreadTurnUsageList, &[&thread])
        .token(&stranger.token)
        .query(&ThreadTurnUsageListQuery::default())
        .send()
        .await;
    assert!(turns.status.is_client_error(), "{:?}", turns.status);
    let cost = s
        .rig
        .world
        .app
        .call(ThreadTurnCostGet, &[&thread, &turn])
        .token(&stranger.token)
        .send()
        .await;
    assert!(cost.status.is_client_error(), "{:?}", cost.status);
}

/// The use an adapter reports for a program that counts nothing: zero everywhere.
fn nothing_counted(turn: Id, context_used: Option<i64>) -> Event {
    let zero = json!({ "input_tokens": 0, "cached_input_tokens": 0, "output_tokens": 0,
                       "reasoning_tokens": 0, "total_tokens": 0 });
    Event::UsageUpdated(UsageUpdated {
        turn_id: turn,
        turn: serde_json::from_value(zero.clone()).unwrap(),
        thread: serde_json::from_value(zero).unwrap(),
        context_window: context_used.map(|_| 200_000),
        context_used,
        report: None,
    })
}

/// A thread of a program that says in its descriptor what it reports of its use, with one turn in which its adapter
/// reports zero. What the thread keeps of use is the thread's `usage`, read from the row.
async fn usage_kept_for(
    overrides: serde_json::Value,
    context_used: Option<i64>,
) -> Option<serde_json::Value> {
    let s = Scene::new().await;
    sqlx::query("UPDATE agents SET descriptor_overrides = $1")
        .bind(overrides)
        .execute(&s.rig.world.db.pool)
        .await
        .unwrap();
    let thread = s.thread().await.base.id;
    let sent = s
        .say(&s.ada, thread, "Fix the login", Delivery::Queue, "usage-0")
        .await;
    assert!(sent.status.is_success(), "{:?}", sent.body);
    let start = s.rig.turn_start(thread, 0).await;
    s.rig.answer(thread, &start, "m0", "Done").await;
    s.rig
        .say(thread, nothing_counted(start.turn_id, context_used))
        .await;
    s.rig
        .say(thread, turn_ended(start.turn_id, TurnOutcome::Completed))
        .await;
    s.rig.wait_turn(start.turn_id, "completed").await;
    sqlx::query_scalar("SELECT usage FROM threads WHERE node_id = $1")
        .bind(thread)
        .fetch_one(&s.rig.world.db.pool)
        .await
        .unwrap()
}

#[tokio::test]
async fn a_program_that_reports_no_usage_leaves_its_threads_totals_unknown_and_never_zero() {
    let kept = usage_kept_for(
        json!({ "capabilities": { "usage_tokens": false, "usage_context": "none" } }),
        None,
    )
    .await;
    assert_eq!(
        kept, None,
        "what its adapter says is zero, which is not a count: the thread has no totals"
    );
}

#[tokio::test]
async fn a_program_that_reports_only_how_full_its_context_is_keeps_that_and_says_its_tokens_are_not_reported()
 {
    let kept = usage_kept_for(
        json!({ "capabilities": { "usage_tokens": false, "usage_context": "size_only" } }),
        Some(30_000),
    )
    .await
    .expect("the context is kept");
    assert_eq!(kept["tokens_reported"], false);
    assert_eq!(kept["context_used"], 30_000);
    assert_eq!(kept["context_window"], 200_000);
    assert!(
        kept.get("total_tokens").is_none() && kept.get("input_tokens").is_none(),
        "no token member is written, so none can read as zero: {kept}"
    );
}

#[tokio::test]
async fn a_program_that_reports_its_tokens_keeps_them_even_when_they_are_zero_so_far() {
    let kept = usage_kept_for(json!({}), None)
        .await
        .expect("the totals are kept");
    assert_eq!(kept["total_tokens"], 0);
    assert!(kept.get("tokens_reported").is_none());
}
