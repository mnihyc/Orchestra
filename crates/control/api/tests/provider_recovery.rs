//! Vendor-authenticated reads recover refused providers; local files preserve refusal time and only a manager may grant one unsupported-read trial.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]

mod places;
mod rig;
mod support;

use chrono::Duration;
use orch_api_contract::resources::{
    ProviderGet, ProviderRefresh, ProviderRefreshBody, ProviderStatus,
};
use orch_api_contract::threads::Delivery;
use orch_link_contract::commands::{
    AgentStartResult, CommandKind, CommandResult, ProviderRefreshResult,
};
use orch_link_contract::events::{Event, TurnCompleted, TurnError, TurnOutcome};
use orch_types::{Id, Time};
use rig::Scene;
use serde_json::json;
use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering},
};

fn vendor_read(s: &Scene, supported: bool, verified: bool) -> Arc<AtomicUsize> {
    let mut descriptor = rig::descriptor();
    descriptor["capabilities"]["provider_facts"] = if supported {
        json!(["plan", "credential_verification"])
    } else {
        json!(["plan"])
    };
    s.rig.machine.handler.on(CommandKind::AgentStart, move |_| {
        Ok(CommandResult::AgentStart(AgentStartResult {
            reused: false,
            harness_version: "1.0.0".into(),
            descriptor: serde_json::from_value(descriptor.clone()).unwrap(),
        }))
    });
    let calls = Arc::new(AtomicUsize::new(0));
    let recorded = calls.clone();
    s.rig.machine.handler.on(CommandKind::ProviderRefresh,move |_| {
        recorded.fetch_add(1,Ordering::SeqCst);
        Ok(CommandResult::ProviderRefresh(ProviderRefreshResult {source:None,status:"ready".into(),facts:serde_json::from_value(json!({"plan":"plus","identity":"ada@example.invalid","windows":[],"credential_verified":verified})).unwrap()}))
    });
    calls
}

fn refusal(turn: Id) -> Event {
    Event::TurnCompleted(TurnCompleted {
        turn_id: turn,
        status: TurnOutcome::Failed,
        error: Some(TurnError {
            reason: None,
            status: None,
            retry_after_secs: None,
            code: "provider_unauthorized".into(),
            message: "the provider's credential was refused".into(),
            retryable: false,
            said: None,
            resets_at: None,
            window: None,
            incomplete: false,
            limit: None,
        }),
    })
}

async fn rejected(s: &Scene) -> (Id, Id, Id) {
    let thread = s.thread().await.base.id;
    s.say(
        &s.ada,
        thread,
        "First refused",
        Delivery::Queue,
        "first-refused",
    )
    .await
    .ok();
    let turn = s.rig.turn_start(thread, 0).await;
    s.rig.say(thread, refusal(turn.turn_id)).await;
    s.rig.wait_turn(turn.turn_id, "failed").await;
    let first = s
        .say(
            &s.ada,
            thread,
            "Oldest waiting",
            Delivery::Queue,
            "oldest-waiting",
        )
        .await
        .ok()
        .input
        .id;
    let second = s
        .say(
            &s.ada,
            thread,
            "Next waiting",
            Delivery::Queue,
            "next-waiting",
        )
        .await
        .ok()
        .input
        .id;
    s.rig.wait_state(thread, "blocked").await;
    (thread, first, second)
}

#[tokio::test]
async fn the_five_minute_clock_finds_a_vendor_verified_credential_and_releases_the_waiting_turn() {
    let s = Scene::new().await;
    // The scene's direct SQL uses PostgreSQL's wall clock; this rule starts on the application's stepped clock.
    sqlx::query("UPDATE providers SET created_at = $2 WHERE id = $1")
        .bind(s.on.provider)
        .bind(s.rig.world.now())
        .execute(s.rig.db.pool())
        .await
        .unwrap();
    let reads = vendor_read(&s, true, true);
    let (thread, first, _) = rejected(&s).await;
    s.rig
        .world
        .app
        .clock
        .advance(Duration::minutes(5) - Duration::milliseconds(1));
    s.rig.engine.refresh_due_provider_facts().await.unwrap();
    assert_eq!(reads.load(Ordering::SeqCst), 0);
    s.rig.world.app.clock.advance(Duration::milliseconds(1));
    s.rig.engine.refresh_due_provider_facts().await.unwrap();
    let running = s.rig.turn_start(thread, 1).await;
    assert_eq!(reads.load(Ordering::SeqCst), 1);
    let input: Id = sqlx::query_scalar("SELECT input_id FROM turns WHERE id = $1")
        .bind(running.turn_id)
        .fetch_one(s.rig.db.pool())
        .await
        .unwrap();
    assert_eq!(input, first);
    let page = s
        .rig
        .world
        .app
        .call(ProviderGet, &[&s.on.provider])
        .token(&s.ada.token)
        .send()
        .await
        .ok();
    assert_eq!(page.base.status, ProviderStatus::Ready);
    assert_eq!(page.facts.credential_verified, Some(true));
}

#[tokio::test]
async fn a_files_only_read_preserves_the_refusal_and_its_time_and_only_a_manager_bypasses_the_minute()
 {
    let s = Scene::new().await;
    let reads = vendor_read(&s, true, false);
    let (thread, _, _) = rejected(&s).await;
    let refused = Time::new(s.rig.world.now());
    let stored: (
        Option<chrono::DateTime<chrono::Utc>>,
        Option<chrono::DateTime<chrono::Utc>>,
    ) = sqlx::query_as("SELECT facts_at,facts_requested_at FROM providers WHERE id=$1")
        .bind(s.on.provider)
        .fetch_one(s.rig.db.pool())
        .await
        .unwrap();
    let before_read = reads.load(Ordering::SeqCst);
    let page = s
        .rig
        .world
        .app
        .call(ProviderGet, &[&s.on.provider])
        .token(&s.ada.token)
        .send()
        .await
        .ok();
    assert_eq!(page.base.status, ProviderStatus::Error);
    assert_eq!(page.status_at, refused);
    // GET is a stored projection. Only explicit Refresh or the background cadence reads.
    assert_eq!(page.facts_at, stored.0.map(Time::new));
    assert_eq!(page.facts_requested_at, stored.1.map(Time::new));
    assert_eq!(reads.load(Ordering::SeqCst), before_read);
    s.rig.world.app.clock.advance(Duration::minutes(5));
    s.rig
        .lets_use(
            "provider",
            s.on.provider,
            "provider.view",
            &s.bea,
            json!({}),
        )
        .await;
    let page = s
        .rig
        .world
        .app
        .call(ProviderRefresh, &[&s.on.provider])
        .body(&orch_api_contract::resources::ProviderRefreshBody::default())
        .token(&s.bea.token)
        .send()
        .await
        .ok();
    assert_eq!(page.base.status, ProviderStatus::Error);
    assert_eq!(
        reads.load(Ordering::SeqCst),
        before_read + 1,
        "a viewer uses the same minute-bound observation"
    );
    let page = s
        .rig
        .world
        .app
        .call(ProviderRefresh, &[&s.on.provider])
        .body(&ProviderRefreshBody { retry: Some(true) })
        .token(&s.ada.token)
        .send()
        .await
        .ok();
    assert_eq!(page.base.status, ProviderStatus::Error);
    assert_eq!(page.status_at, refused);
    assert_eq!(
        reads.load(Ordering::SeqCst),
        before_read + 2,
        "the manager's Try again runs immediately"
    );
    assert_eq!(
        s.rig.turn_starts(thread).len(),
        1,
        "a declared vendor read never receives a local-file trial"
    );
}

#[tokio::test]
async fn try_again_without_an_eligible_authored_input_refuses_without_starting_a_turn() {
    let s = Scene::new().await;
    vendor_read(&s, false, false);
    let (thread, first, second) = rejected(&s).await;
    sqlx::query("UPDATE nodes SET archived_at = $2 WHERE id = $1")
        .bind(thread)
        .bind(s.rig.world.now())
        .execute(s.rig.db.pool())
        .await
        .unwrap();
    let refusal = s
        .rig
        .world
        .app
        .call(ProviderRefresh, &[&s.on.provider])
        .body(&ProviderRefreshBody { retry: Some(true) })
        .token(&s.ada.token)
        .send()
        .await
        .problem();
    assert_eq!(
        refusal.code,
        orch_api_contract::common::ErrorCode::RequestInvalid
    );
    assert_eq!(refusal.extra["current"]["status"], "error");
    assert_eq!(s.rig.turn_starts(thread).len(), 1);
    let pending: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM thread_inputs WHERE id = ANY($1) AND state = 'pending'",
    )
    .bind(vec![first, second])
    .fetch_one(s.rig.db.pool())
    .await
    .unwrap();
    assert_eq!(pending, 2);
}

#[tokio::test]
async fn without_a_vendor_read_one_oldest_input_trials_once_and_acceptance_recovers_or_refusal_keeps_every_other_input_waiting()
 {
    for accepted in [true, false] {
        let s = Scene::new().await;
        vendor_read(&s, false, false);
        let (thread, first, second) = rejected(&s).await;
        s.rig
            .world
            .app
            .call(ProviderRefresh, &[&s.on.provider])
            .body(&ProviderRefreshBody { retry: Some(true) })
            .token(&s.ada.token)
            .send()
            .await
            .ok();
        let trial = s.rig.turn_start(thread, 1).await;
        let input: Id = sqlx::query_scalar("SELECT input_id FROM turns WHERE id = $1")
            .bind(trial.turn_id)
            .fetch_one(s.rig.db.pool())
            .await
            .unwrap();
        assert_eq!(input, first);
        s.rig
            .world
            .app
            .call(ProviderRefresh, &[&s.on.provider])
            .body(&ProviderRefreshBody { retry: Some(true) })
            .token(&s.ada.token)
            .send()
            .await
            .ok();
        assert_eq!(
            s.rig.turn_starts(thread).len(),
            2,
            "an unfinished trial prevents a second grant"
        );
        if accepted {
            s.rig
                .answer(thread, &trial, "accepted", "The vendor accepted the turn")
                .await;
            s.rig
                .say(
                    thread,
                    rig::turn_ended(trial.turn_id, TurnOutcome::Completed),
                )
                .await;
            s.rig.wait_turn(trial.turn_id, "completed").await;
            s.rig.turn_start(thread, 2).await;
            let state: String = sqlx::query_scalar("SELECT status FROM providers WHERE id = $1")
                .bind(s.on.provider)
                .fetch_one(s.rig.db.pool())
                .await
                .unwrap();
            assert_eq!(state, "ready");
        } else {
            s.rig.say(thread, refusal(trial.turn_id)).await;
            s.rig.wait_turn(trial.turn_id, "failed").await;
            let reservation: Option<orch_types::Id> =
                sqlx::query_scalar("SELECT credential_trial_turn_id FROM providers WHERE id = $1")
                    .bind(s.on.provider)
                    .fetch_one(s.rig.db.pool())
                    .await
                    .unwrap();
            assert!(
                reservation.is_none(),
                "a failed trial clears its slot without authorizing another turn"
            );
            s.rig
                .engine
                .reevaluate_provider(s.on.provider)
                .await
                .unwrap();
            s.rig.wait_state(thread, "blocked").await;
            assert_eq!(s.rig.turn_starts(thread).len(), 2);
            let state: String = sqlx::query_scalar("SELECT state FROM thread_inputs WHERE id = $1")
                .bind(second)
                .fetch_one(s.rig.db.pool())
                .await
                .unwrap();
            assert_eq!(state, "pending");
            let page = s
                .rig
                .world
                .app
                .call(ProviderGet, &[&s.on.provider])
                .token(&s.ada.token)
                .send()
                .await
                .ok();
            assert_eq!(page.base.status, ProviderStatus::Error);
            // The queued re-evaluation crosses the local link before the absence of another start is checked.
            tokio::time::sleep(std::time::Duration::from_secs(1)).await;
            assert_eq!(s.rig.turn_starts(thread).len(), 2);
        }
    }
}
