//! A rejected provider stays blocked until a new sign-in or an accepted turn proves it works again.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]

mod common;

use chrono::{DateTime, Utc};
use common::{World, eventually, queued, turn_ended};
use orch_authz::Caller;
use orch_db::queries::access::{PageKey, testing as access_testing};
use orch_link_contract::commands::{Command, CommandKind, CommandResult, ProviderRefreshResult};
use orch_link_contract::events::{Event, ProviderStatus, TurnCompleted, TurnError, TurnOutcome};
use orch_types::{Id, Json};
use serde_json::json;

fn refusal() -> TurnError {
    TurnError {
        reason: None,
        status: None,
        retry_after_secs: None,
        code: "provider_unauthorized".into(),
        message: "The vendor refused the provider's sign-in or key.".into(),
        retryable: false,
        said: Some("workspace routing discovery unauthorized (401)".into()),
        resets_at: None,
        window: None,
        incomplete: false,
        limit: None,
    }
}

async fn reject(world: &World, thread: Id, turn: Id) {
    world
        .say(
            thread,
            Event::TurnCompleted(TurnCompleted {
                turn_id: turn,
                status: TurnOutcome::Failed,
                error: Some(refusal()),
            }),
        )
        .await;
    world.wait_turn(turn, "failed").await;
}

async fn rejected(world: &World) -> Id {
    let thread = world.thread("Rejected sign-in").await;
    let (start, _) = world.running_turn(thread, "Go").await;
    reject(world, thread, start.turn_id).await;
    thread
}

async fn status(world: &World) -> (String, Option<String>, DateTime<Utc>) {
    sqlx::query_as("SELECT status, status_detail, updated_at FROM providers WHERE id = $1")
        .bind(world.provider)
        .fetch_one(world.pool.pool())
        .await
        .unwrap()
}

fn a_refresh_still_finds_the_login_files(world: &World) {
    world
        .machine
        .handler
        .on(CommandKind::ProviderRefresh, |_: &Command| {
            Ok(CommandResult::ProviderRefresh(ProviderRefreshResult {
                source: None,
                status: "ready".into(),
                facts: serde_json::from_value(
                    json!({ "plan": "pro", "identity": "test@example.invalid", "windows": [],
                "credential": { "method": null, "yields": "login", "expires_at": null } }),
                )
                .unwrap(),
            }))
        });
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_refused_turn_keeps_the_provider_in_error_with_the_reason_and_the_time() {
    let world = World::new().await;
    let before = world.clock.now();
    rejected(&world).await;
    let (state, detail, at) = status(&world).await;
    assert_eq!(state, "error");
    assert!(detail.unwrap().contains("refused"));
    assert!(at >= before && at <= world.clock.now());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn provider_attention_reaches_its_managers_once_per_rejection_and_no_other_person() {
    let world = World::new().await;
    let manager = world.person("bora").await;
    let group_manager = world.person("dana").await;
    let unrelated = world.person("chen").await;
    let mut conn = world.pool.acquire().await.unwrap();
    // Ada owns the provider: its owner row is made with it.
    let page = PageKey::new("provider", world.provider, "live");
    let group = access_testing::insert_group(&mut conn, world.root, "Operators", world.ada)
        .await
        .unwrap();
    access_testing::add_to_group(&mut conn, group, group_manager)
        .await
        .unwrap();
    access_testing::add_entry(
        &mut conn,
        &page,
        "any",
        "provider.manage",
        ("group", Some(group), None),
        None,
        world.clock.now(),
    )
    .await
    .unwrap();
    access_testing::add_entry(
        &mut conn,
        &PageKey::new("provider", world.provider, "live"),
        "any",
        "provider.manage",
        ("user", Some(manager), None),
        None,
        world.clock.now(),
    )
    .await
    .unwrap();
    drop(conn);
    let first = world.thread("First").await;
    let second = world.thread("Second").await;
    let (a, _) = world.running_turn(first, "First").await;
    let (b, _) = world.running_turn(second, "Second").await;
    reject(&world, first, a.turn_id).await;
    reject(&world, second, b.turn_id).await;
    for user in [world.ada, manager, group_manager] {
        let count: i64 = world.scalar("SELECT count(*) FROM notifications WHERE user_id = $1 AND kind = 'provider.attention'", user).await;
        assert_eq!(count, 1, "each manager is told once for the same rejection");
    }
    let count: i64 = world
        .scalar(
            "SELECT count(*) FROM notifications WHERE user_id = $1 AND kind = 'provider.attention'",
            unrelated,
        )
        .await;
    assert_eq!(count, 0);
    world.say_of_provider_taken_in(Event::ProviderLogin(serde_json::from_value(json!({ "state": "completed", "method": "device_code", "shows": [], "asks": null, "expires_at": null, "error": null })).unwrap())).await;
    eventually("the new sign-in recovers the provider", || async {
        status(&world).await.0 == "ready"
    })
    .await;
    let third = world.thread("Third").await;
    let (c, _) = world.running_turn(third, "Third").await;
    reject(&world, third, c.turn_id).await;
    for user in [world.ada, manager, group_manager] {
        let count: i64 = world.scalar("SELECT count(*) FROM notifications WHERE user_id = $1 AND kind = 'provider.attention'", user).await;
        assert_eq!(count, 2, "a new rejection after recovery is a new notice");
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_thread_bound_to_a_rejected_provider_starts_no_turn_and_names_that_provider() {
    let world = World::new().await;
    let waiting = world.thread("Waiting").await;
    rejected(&world).await;
    let before = world.turn_starts(waiting).len();
    let input = world
        .engine()
        .submit_input(&Caller::session(world.ada), waiting, queued("Go"))
        .await
        .unwrap();
    world
        .wait_told_blocked(waiting, "provider_unavailable")
        .await;
    assert_eq!(world.turn_starts(waiting).len(), before);
    assert_eq!(
        world
            .count("SELECT count(*) FROM turns WHERE thread_id = $1", waiting)
            .await,
        0
    );
    assert_eq!(world.input_state(input.input.id).await.0, "pending");
    let detail: String = world
        .scalar(
            "SELECT blocked_detail FROM threads WHERE node_id = $1",
            waiting,
        )
        .await;
    assert!(
        detail.contains("Fake subscription") && detail.contains("refused"),
        "{detail}"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn reading_a_rejected_providers_login_files_again_does_not_make_it_ready() {
    let world = World::new().await;
    rejected(&world).await;
    let original_reason = status(&world).await.1;
    a_refresh_still_finds_the_login_files(&world);
    // A manager's Try again reads a refused provider at once. An ordinary Refresh is one read a minute, and the
    // world's own first read of its subscription was moments ago.
    world
        .engine()
        .refresh_provider_for_retry(world.provider)
        .await
        .unwrap();
    assert_eq!(status(&world).await.0, "error");
    assert_eq!(status(&world).await.1, original_reason);
    let facts: Json = world
        .scalar("SELECT facts FROM providers WHERE id = $1", world.provider)
        .await;
    assert_eq!(
        facts["identity"], "test@example.invalid",
        "facts still refresh independently of readiness"
    );
    world
        .say_of_provider_taken_in(Event::ProviderFacts(
            serde_json::from_value(json!({ "identity": "test@example.invalid", "plan": "pro" }))
                .unwrap(),
        ))
        .await;
    world
        .say_of_provider_taken_in(Event::ProviderStatus(ProviderStatus {
            status: "ready".into(),
            detail: None,
        }))
        .await;
    assert_eq!(
        status(&world).await.0,
        "error",
        "a subscription's runtime saying ready is no new vendor proof"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_accepted_turn_recovers_the_provider_and_releases_its_waiting_threads() {
    let world = World::new().await;
    let accepted = world.thread("Accepted").await;
    let (start, _) = world.running_turn(accepted, "Keep working").await;
    let waiting = world.thread("Waiting").await;
    rejected(&world).await;
    world
        .engine()
        .submit_input(
            &Caller::session(world.ada),
            waiting,
            queued("Resume when it works"),
        )
        .await
        .unwrap();
    world.wait_blocked(waiting, "provider_unavailable").await;
    world
        .say(accepted, turn_ended(start.turn_id, TurnOutcome::Completed))
        .await;
    world.wait_turn(start.turn_id, "completed").await;
    assert_eq!(status(&world).await.0, "ready");
    assert!(status(&world).await.1.is_none());
    world.turn_start(waiting, 0).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn accepted_model_output_recovers_the_provider_while_its_turn_is_still_running() {
    let world = World::new().await;
    let accepted = world.thread("Accepted model output").await;
    let (start, _) = world.running_turn(accepted, "Keep working").await;
    rejected(&world).await;
    world
        .answer(
            accepted,
            &start,
            "accepted-message",
            "The vendor accepted this turn.",
        )
        .await;
    eventually("accepted model output recovers the provider", || async {
        status(&world).await.0 == "ready"
    })
    .await;
    let state: String = world
        .scalar("SELECT state FROM turns WHERE id = $1", start.turn_id)
        .await;
    assert_eq!(
        state, "running",
        "recovery does not wait for the accepted turn to end"
    );
    world
        .say(accepted, turn_ended(start.turn_id, TurnOutcome::Completed))
        .await;
    world.wait_turn(start.turn_id, "completed").await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_completed_new_sign_in_recovers_a_rejected_provider_and_releases_its_waiting_threads() {
    let world = World::new().await;
    let waiting = world.thread("Waiting for sign-in").await;
    rejected(&world).await;
    world
        .engine()
        .submit_input(
            &Caller::session(world.ada),
            waiting,
            queued("Go after sign-in"),
        )
        .await
        .unwrap();
    world.wait_blocked(waiting, "provider_unavailable").await;
    world.say_of_provider_taken_in(Event::ProviderLogin(serde_json::from_value(json!({ "state": "completed", "method": "device_code", "shows": [], "asks": null, "expires_at": null, "error": null })).unwrap())).await;
    assert_eq!(status(&world).await.0, "ready");
    assert!(status(&world).await.1.is_none());
    world.turn_start(waiting, 0).await;
}
