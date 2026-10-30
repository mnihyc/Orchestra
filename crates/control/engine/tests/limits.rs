//! A turn that ends with a limit of the plan or with a fault that passes: the provider is closed until the time the vendor names and looked at again by itself, a limit of one family of models closes nothing, a fault that passes leaves the provider as it was and the thread free, and the thread says which limit it waits for and until when.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]

mod common;

use std::time::Duration;

use chrono::Utc;
use common::{World, eventually, meanwhile, queued, turn_began};
use orch_authz::Caller;
use orch_link_contract::events::{
    Event, ProviderStatus, SessionLost, TurnCompleted, TurnError, TurnOutcome,
};
use orch_types::{Id, Json, Time};
use serde_json::json;

/// The vendor's own words for the limit that stopped eight agents on a Monday.
const WEEKLY: &str = "You've hit your weekly limit · resets Oct 14, 5pm (Asia/Singapore)";

/// A moment `seconds` from now.
fn in_secs(seconds: i64) -> Time {
    Time::new(Utc::now() + chrono::Duration::seconds(seconds))
}

/// A limit of the whole plan, the way an adapter reports it: its sentence, the vendor's words and the moment.
fn plan_limit(ends: Option<Time>) -> TurnError {
    TurnError {
        reason: None,
        status: None,
        retry_after_secs: None,
        code: "provider_exhausted".to_owned(),
        message: "The weekly limit of the plan is reached; it ends Wednesday 09:00 UTC.".to_owned(),
        retryable: false,
        said: Some(WEEKLY.to_owned()),
        resets_at: ends,
        window: None,
        incomplete: false,
        limit: None,
    }
}

/// A fault with a code that the adapter calls retryable.
fn fault(code: &str, said: &str, incomplete: bool) -> TurnError {
    TurnError {
        reason: None,
        status: None,
        retry_after_secs: None,
        code: code.to_owned(),
        message: format!("The harness reports {code}."),
        retryable: true,
        said: Some(said.to_owned()),
        resets_at: None,
        window: None,
        incomplete,
        limit: None,
    }
}

/// A turn of the thread that the harness takes up and ends as failed with `error`; gives the turn.
async fn failing_turn(world: &World, thread: Id, text: &str, error: TurnError) -> Id {
    // These cases isolate provider state; call-policy scheduling has its own integration cases.
    world.set(world.topic, "calls.rules", json!([])).await;
    let nth = world.turn_starts(thread).len();
    world
        .engine()
        .submit_input(&Caller::session(world.ada), thread, queued(text))
        .await
        .unwrap();
    let start = world.turn_start(thread, nth).await;
    world.say(thread, turn_began(start.turn_id)).await;
    world
        .say(
            thread,
            Event::TurnCompleted(TurnCompleted {
                turn_id: start.turn_id,
                status: TurnOutcome::Failed,
                error: Some(error),
            }),
        )
        .await;
    world.wait_turn(start.turn_id, "failed").await;
    start.turn_id
}

/// The time of the vendor passes: the moment the provider's facts name for the end of the limit is a second ago.
/// (Waiting for a real moment would make a test depend on how busy the machine is.)
async fn let_the_vendors_time_pass(world: &World) {
    world
        .exec(&format!(
            "UPDATE providers SET facts = jsonb_set(facts, '{{limit_reached,resets_at}}',
                 to_jsonb(to_char((now() - interval '1 second') AT TIME ZONE 'UTC', 'YYYY-MM-DD\"T\"HH24:MI:SS.MS\"Z\"')))
              WHERE id = '{}'",
            world.provider
        ))
        .await;
}

/// What the provider says of itself: its status and the sentence beside it.
async fn provider_status(world: &World) -> (String, Option<String>) {
    sqlx::query_as("SELECT status, status_detail FROM providers WHERE id = $1")
        .bind(world.provider)
        .fetch_one(world.pool.pool())
        .await
        .unwrap()
}

/// The limit the provider's facts name, or null.
async fn limit_of_provider(world: &World) -> Json {
    let found: Option<Json> =
        sqlx::query_scalar("SELECT facts -> 'limit_reached' FROM providers WHERE id = $1")
            .bind(world.provider)
            .fetch_one(world.pool.pool())
            .await
            .unwrap();
    found.unwrap_or(Json::Null)
}

/// The error a turn ended with, as it is kept.
async fn error_of(world: &World, turn: Id) -> Json {
    world
        .scalar("SELECT error FROM turns WHERE id = $1", turn)
        .await
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_limit_of_the_plan_closes_the_provider_until_the_vendors_time_and_the_turn_keeps_the_vendors_words()
 {
    let world = World::new().await;
    let thread = world.thread("Limited").await;
    let ends = in_secs(3_600);
    let turn = failing_turn(&world, thread, "Go", plan_limit(Some(ends))).await;

    let (status, detail) = provider_status(&world).await;
    assert_eq!(status, "exhausted");
    assert_eq!(
        detail.as_deref(),
        Some("The weekly limit of the plan is reached; it ends Wednesday 09:00 UTC."),
        "the provider says which limit and until when, in the adapter's sentence"
    );
    let limit = limit_of_provider(&world).await;
    assert_eq!(limit["type"], WEEKLY, "which limit, as the vendor named it");
    assert_eq!(limit["resets_at"], ends.to_string());

    let kept = error_of(&world, turn).await;
    assert_eq!(kept["code"], "provider_exhausted");
    assert_eq!(kept["said"], WEEKLY, "the vendor's words go along, whole");
    assert_eq!(kept["resets_at"], ends.to_string());
    assert_eq!(kept["incomplete"], false);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_input_that_waits_for_a_closed_provider_is_told_which_limit_and_until_when() {
    let world = World::new().await;
    let thread = world.thread("Waiting").await;
    failing_turn(&world, thread, "Go", plan_limit(Some(in_secs(3_600)))).await;
    world.wait_state(thread, "idle").await;

    world
        .engine()
        .submit_input(&Caller::session(world.ada), thread, queued("Again"))
        .await
        .unwrap();
    world.wait_blocked(thread, "provider_exhausted").await;
    let detail: Option<String> = world
        .scalar(
            "SELECT blocked_detail FROM threads WHERE node_id = $1",
            thread,
        )
        .await;
    let detail = detail.unwrap();
    assert!(detail.contains("weekly limit"), "which limit: {detail}");
    assert!(
        detail.contains("ends Wednesday 09:00 UTC"),
        "until when: {detail}"
    );
    assert_eq!(
        world.turn_starts(thread).len(),
        1,
        "nothing was started on a closed provider"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_provider_whose_limit_has_ended_is_looked_at_again_by_itself_and_the_waiting_input_starts()
 {
    let world = World::new().await;
    let thread = world.thread("Reopened").await;
    failing_turn(&world, thread, "Go", plan_limit(Some(in_secs(3_600)))).await;
    world.wait_state(thread, "idle").await;
    world
        .engine()
        .submit_input(&Caller::session(world.ada), thread, queued("Again"))
        .await
        .unwrap();
    world.wait_blocked(thread, "provider_exhausted").await;
    assert_eq!(world.turn_starts(thread).len(), 1);

    // Nothing is done but waiting: the time comes, the provider opens, and the input that waited starts.
    let_the_vendors_time_pass(&world).await;
    let second = world.turn_start(thread, 1).await;
    assert_eq!(common::words_after_where(&second.text), "Again");
    let (status, detail) = provider_status(&world).await;
    assert_eq!(status, "ready");
    assert_eq!(
        detail, None,
        "the sentence of the limit goes with the limit"
    );
    assert_eq!(limit_of_provider(&world).await, Json::Null);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_provider_closed_by_a_limit_opens_by_itself_when_the_time_passes_with_nothing_waiting() {
    let world = World::new().await;
    let thread = world.thread("Nothing waits").await;
    failing_turn(&world, thread, "Go", plan_limit(Some(in_secs(3_600)))).await;
    meanwhile(Duration::from_millis(400)).await;
    assert_eq!(
        provider_status(&world).await.0,
        "exhausted",
        "not before its time"
    );
    let_the_vendors_time_pass(&world).await;
    eventually("the provider to be ready again", || async {
        provider_status(&world).await.0 == "ready"
    })
    .await;
    assert_eq!(limit_of_provider(&world).await, Json::Null);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_limit_without_a_time_stays_until_the_machine_says_the_provider_is_ready() {
    let world = World::new().await;
    let thread = world.thread("No time").await;
    failing_turn(&world, thread, "Go", plan_limit(None)).await;
    assert_eq!(provider_status(&world).await.0, "exhausted");
    let limit = limit_of_provider(&world).await;
    assert_eq!(limit["type"], WEEKLY);
    assert_eq!(limit["resets_at"], Json::Null);

    // No time to look again at: the provider is not opened by guessing.
    meanwhile(Duration::from_millis(600)).await;
    assert_eq!(provider_status(&world).await.0, "exhausted");

    world
        .say_of_provider_taken_in(Event::ProviderStatus(ProviderStatus {
            status: "ready".to_owned(),
            detail: None,
        }))
        .await;
    assert_eq!(provider_status(&world).await.0, "ready");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_turn_that_ends_with_a_limit_and_no_time_leaves_the_time_another_turn_gave() {
    let world = World::new().await;
    let (first, second) = (world.thread("First").await, world.thread("Second").await);
    let (first_turn, _) = world.running_turn(first, "Go").await;
    let (second_turn, _) = world.running_turn(second, "Go too").await;
    let ends = in_secs(3_600);
    for (thread, turn, limit) in [
        (first, first_turn.turn_id, plan_limit(Some(ends))),
        (second, second_turn.turn_id, plan_limit(None)),
    ] {
        world
            .say(
                thread,
                Event::TurnCompleted(TurnCompleted {
                    turn_id: turn,
                    status: TurnOutcome::Failed,
                    error: Some(limit),
                }),
            )
            .await;
        world.wait_turn(turn, "failed").await;
    }
    let limit = limit_of_provider(&world).await;
    assert_eq!(limit["resets_at"], ends.to_string(), "{limit}");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_limit_of_one_family_of_models_closes_nothing_and_the_next_input_starts() {
    let world = World::new().await;
    let thread = world.thread("One family").await;
    let mut opus = plan_limit(Some(in_secs(3_600)));
    opus.retryable = true;
    opus.said = Some("You've hit your Opus limit · resets 4pm (UTC)".to_owned());
    opus.message = "The weekly Opus limit of the plan is reached.".to_owned();
    let turn = failing_turn(&world, thread, "Go", opus).await;

    assert_eq!(provider_status(&world).await, ("ready".to_owned(), None));
    assert_eq!(limit_of_provider(&world).await, Json::Null);
    let kept = error_of(&world, turn).await;
    assert_eq!(kept["code"], "provider_exhausted");
    assert_eq!(kept["retryable"], true, "another model may work");
    assert_eq!(
        kept["said"],
        "You've hit your Opus limit · resets 4pm (UTC)"
    );

    world.wait_state(thread, "idle").await;
    world
        .engine()
        .submit_input(
            &Caller::session(world.ada),
            thread,
            queued("With another model"),
        )
        .await
        .unwrap();
    let second = world.turn_start(thread, 1).await;
    assert_eq!(
        common::words_after_where(&second.text),
        "With another model"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_vendor_that_is_busy_or_out_of_reach_is_not_held_against_the_provider_and_the_thread_is_free()
 {
    let world = World::new().await;
    let thread = world.thread("Passing").await;
    let cases = [
        (
            "provider_busy",
            "529 Overloaded (tried again 10 times)",
            false,
        ),
        ("provider_unreachable", "connection refused", false),
        (
            "provider_unreachable",
            "the stream ended in the middle of an answer",
            true,
        ),
        ("harness_error", "something nobody has seen before", false),
    ];
    for (nth, (code, said, incomplete)) in cases.into_iter().enumerate() {
        let turn = failing_turn(
            &world,
            thread,
            &format!("Try {nth}"),
            fault(code, said, incomplete),
        )
        .await;
        assert_eq!(
            provider_status(&world).await,
            ("ready".to_owned(), None),
            "{code} leaves the provider as it was"
        );
        assert_eq!(limit_of_provider(&world).await, Json::Null);
        let kept = error_of(&world, turn).await;
        assert_eq!(kept["code"], code);
        assert_eq!(kept["said"], said, "the words are kept whatever the code");
        assert_eq!(kept["incomplete"], incomplete);
        world.wait_state(thread, "idle").await;
    }
    // And the thread tries again, as the fifth turn.
    world
        .engine()
        .submit_input(&Caller::session(world.ada), thread, queued("Once more"))
        .await
        .unwrap();
    world.turn_start(thread, cases.len()).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn what_a_machine_reports_of_its_facts_does_not_lift_a_limit_before_its_time() {
    let world = World::new().await;
    let thread = world.thread("Facts").await;
    let ends = in_secs(3_600);
    failing_turn(&world, thread, "Go", plan_limit(Some(ends))).await;

    // A report that does not know of the limit leaves it standing.
    world
        .say_of_provider_taken_in(Event::ProviderFacts(
            serde_json::from_value(json!({
                "plan": "max", "windows": [], "plan_credits": null, "limit_reached": null,
                "identity": "dana@example.com", "credential": null,
            }))
            .unwrap(),
        ))
        .await;
    let limit = limit_of_provider(&world).await;
    assert_eq!(limit["resets_at"], ends.to_string());
    let plan: String = world
        .scalar(
            "SELECT facts ->> 'plan' FROM providers WHERE id = $1",
            world.provider,
        )
        .await;
    assert_eq!(plan, "max", "the rest of the report is taken as it came");

    // One that names a limit of its own is taken as it is.
    world
        .say_of_provider_taken_in(Event::ProviderFacts(
            serde_json::from_value(json!({
                "plan": "max", "windows": [], "plan_credits": null,
                "limit_reached": { "type": "seven_day", "resets_at": ends.to_string() },
                "identity": null, "credential": null,
            }))
            .unwrap(),
        ))
        .await;
    assert_eq!(limit_of_provider(&world).await["type"], "seven_day");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_credential_the_vendor_refused_puts_the_provider_in_error() {
    let world = World::new().await;
    let thread = world.thread("Refused").await;
    let mut refused = fault("provider_unauthorized", "401 invalid x-api-key", false);
    refused.retryable = false;
    failing_turn(&world, thread, "Go", refused).await;
    assert_eq!(provider_status(&world).await.0, "error");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_harness_that_is_gone_ends_the_turn_it_ran_and_the_thread_goes_on() {
    let world = World::new().await;
    let thread = world.thread("Gone").await;
    let (start, _) = world.running_turn(thread, "Go").await;
    world
        .say(
            thread,
            Event::SessionLost(SessionLost {
                reason: "the harness process ended with signal 9".to_owned(),
            }),
        )
        .await;
    world.wait_turn(start.turn_id, "failed").await;
    let kept = error_of(&world, start.turn_id).await;
    assert_eq!(kept["code"], "session_lost");
    assert!(
        kept["message"].as_str().unwrap().contains("signal 9"),
        "what the harness said of its end is in the sentence: {kept}"
    );
    world.wait_state(thread, "idle").await;
    assert_eq!(
        provider_status(&world).await.0,
        "ready",
        "a harness that died is not the provider's fault"
    );

    world
        .engine()
        .submit_input(&Caller::session(world.ada), thread, queued("Again"))
        .await
        .unwrap();
    world.turn_start(thread, 1).await;
}
