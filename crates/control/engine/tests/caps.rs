//! A turn does not start while a usage cap that covers it has no room, and says which; one person's allowance holds their input and nobody else's; caps on turns at once make threads wait in the order of their oldest input; a running turn is stopped when its cap is reached; a lease carries what room is left; and a turn can be let past a cap that is reached by whoever holds the row where the cap is defined, which is written down.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]

mod common;

use chrono::{DateTime, Duration, Utc};
use common::{MODEL, World, meanwhile, queued, turn_began, turn_ended};
use orch_api_contract::threads::{ContentPart, LimitBypassRequest};
use orch_authz::Caller;
use orch_db::queries::access::PageKey;
use orch_db::queries::access::testing::add_entry;
use orch_db::usage::ledger::{self, Attribution, NewEvent, Tokens};
use orch_engine::inputs::InputError;
use orch_link_contract::events::{Event, TurnOutcome, UsageUpdated};
use orch_testkit::clock;
use orch_types::{Id, Json, new_id};
use serde_json::json;
use std::sync::Arc;
use std::time::Duration as Wait;

/// A cap, by hand: the members the tests vary, the rest as a cap for everyone that is shared, counts tokens
/// in a day and stops what it reaches.
#[derive(Clone)]
struct Cap {
    node: Option<Id>,
    provider: Option<Id>,
    subject: &'static str,
    subject_id: Option<Id>,
    subject_role: Option<&'static str>,
    split: &'static str,
    models: Vec<&'static str>,
    meter: &'static str,
    period: Option<&'static str>,
    amount: Option<String>,
    at_limit: &'static str,
    warn: Option<i16>,
    uncounted: &'static str,
}

impl Cap {
    fn tokens(amount: i64) -> Self {
        Self {
            node: None,
            provider: None,
            subject: "everyone",
            subject_id: None,
            subject_role: None,
            split: "shared",
            models: Vec::new(),
            meter: "tokens",
            period: Some("day"),
            amount: Some(amount.to_string()),
            at_limit: "stop",
            warn: None,
            uncounted: "allow",
        }
    }

    fn credits(amount: &str) -> Self {
        Self {
            meter: "credits",
            amount: Some(amount.to_owned()),
            ..Self::tokens(0)
        }
    }

    fn running(amount: i64) -> Self {
        Self {
            meter: "running",
            period: None,
            ..Self::tokens(amount)
        }
    }

    fn on(self, node: Id) -> Self {
        Self {
            node: Some(node),
            ..self
        }
    }

    fn of_provider(self, provider: Id) -> Self {
        Self {
            provider: Some(provider),
            ..self
        }
    }

    fn each(self) -> Self {
        Self {
            split: "each",
            ..self
        }
    }

    fn for_user(self, user: Id) -> Self {
        Self {
            subject: "user",
            subject_id: Some(user),
            ..self
        }
    }

    fn for_group(self, group: Id) -> Self {
        Self {
            subject: "group",
            subject_id: Some(group),
            split: "each",
            ..self
        }
    }

    fn for_role(self, role: &'static str) -> Self {
        Self {
            subject: "role",
            subject_role: Some(role),
            split: "each",
            ..self
        }
    }

    fn without_limit(self) -> Self {
        Self {
            amount: None,
            ..self
        }
    }

    fn finishing(self) -> Self {
        Self {
            at_limit: "finish_turn",
            ..self
        }
    }

    fn warning_at(self, percent: i16) -> Self {
        Self {
            warn: Some(percent),
            ..self
        }
    }

    fn blocking_what_it_cannot_count(self) -> Self {
        Self {
            uncounted: "block",
            ..self
        }
    }

    fn naming(self, models: &[&'static str]) -> Self {
        Self {
            models: models.to_vec(),
            ..self
        }
    }

    async fn make(self, world: &World) -> Id {
        let id = new_id();
        sqlx::query(
            "INSERT INTO usage_caps (id, node_id, provider_id, subject_kind, subject_id, subject_role, split, models,
                                     meter, period, amount, at_limit, warn_percent, uncounted, created_by)
             VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11::text::numeric, $12, $13, $14, $15)",
        )
        .bind(id)
        .bind(self.node)
        .bind(self.provider)
        .bind(self.subject)
        .bind(self.subject_id)
        .bind(self.subject_role)
        .bind(self.split)
        .bind(
            self.models
                .iter()
                .map(|model| (*model).to_owned())
                .collect::<Vec<_>>(),
        )
        .bind(self.meter)
        .bind(self.period)
        .bind(self.amount)
        .bind(self.at_limit)
        .bind(self.warn)
        .bind(self.uncounted)
        .bind(world.ada)
        .execute(world.pool.pool())
        .await
        .unwrap();
        id
    }
}

/// Writes down that `person` used `tokens` in the world's topic, at `at`, counted in the ledger as a forwarder's
/// request would be, with `credits` where there was a rate.
async fn used_at(
    world: &World,
    thread: Id,
    person: Option<Id>,
    tokens: i64,
    credits: Option<&str>,
    at: DateTime<Utc>,
) {
    let place = Attribution {
        org_id: world.root,
        project_id: world.project,
        topic_id: world.topic,
        thread_id: thread,
        segment_id: new_id(),
        turn_id: new_id(),
        provider_id: world.provider,
        entry_id: None,
        user_id: person,
        agent_id: Some(world.agent),
        harness_kind: Some("fake".to_owned()),
        environment_id: Some(world.environment),
        environment_kind: Some("managed".to_owned()),
        arrangement: Some("together".to_owned()),
        turn_model: MODEL.to_owned(),
        effort: None,
        params: Json::Null,
    };
    let mut conn = world.pool.acquire().await.unwrap();
    ledger::record(
        &mut conn,
        &NewEvent {
            id: new_id(),
            at,
            origin: ledger::origin::FORWARDER,
            scope: ledger::scope::REQUEST,
            counted: true,
            attribution: &place,
            model: MODEL,
            tier: None,
            tokens: Tokens {
                input: tokens,
                cached_input: 0,
                cache_write: 0,
                output: 0,
                reasoning: 0,
                total: tokens,
            },
            context_tokens: None,
            request_ref: None,
            rate: None,
            rate_source: credits.map(|_| "rate_card"),
            credits,
        },
    )
    .await
    .unwrap();
}

async fn used(world: &World, thread: Id, person: Option<Id>, tokens: i64) {
    used_at(world, thread, person, tokens, None, Utc::now()).await;
}

/// The sentence a thread is blocked with.
async fn told(world: &World, thread: Id) -> String {
    world
        .sync(thread)
        .await
        .thread
        .blocked
        .unwrap()
        .detail
        .unwrap_or_default()
}

fn reported(turn: Id, tokens: i64) -> Event {
    let usage = json!({
        "input_tokens": tokens, "cached_input_tokens": 0, "output_tokens": 0,
        "reasoning_tokens": 0, "total_tokens": tokens
    });
    Event::UsageUpdated(UsageUpdated {
        turn_id: turn,
        turn: serde_json::from_value(usage.clone()).unwrap(),
        thread: serde_json::from_value(usage).unwrap(),
        context_window: None,
        context_used: None,
        report: None,
    })
}

async fn send(world: &World, who: Id, thread: Id, text: &str) -> orch_engine::inputs::Submitted {
    world
        .engine()
        .submit_input(&Caller::session(who), thread, queued(text))
        .await
        .unwrap()
}

async fn send_past(
    world: &World,
    who: Id,
    thread: Id,
    text: &str,
    caps: &[Id],
) -> Result<orch_engine::inputs::Submitted, InputError> {
    let mut request = queued(text);
    request.bypass = Some(LimitBypassRequest {
        caps: Some(caps.to_vec()),
        provider_limit: None,
    });
    world
        .engine()
        .submit_input(&Caller::session(who), thread, request)
        .await
}

async fn count(world: &World, sql: &str) -> i64 {
    sqlx::query_scalar(sqlx::AssertSqlSafe(sql.to_owned()))
        .fetch_one(world.pool.pool())
        .await
        .unwrap()
}

/// A person who may interact with the world's topic.
async fn teammate(world: &World, name: &str, role: &str) -> Id {
    let person = world.person(name).await;
    world.give(person, role).await;
    person
}

/// A second provider beside the world's, for a cap that names it.
async fn another_provider(world: &World) -> Id {
    let id = new_id();
    sqlx::query(
        "INSERT INTO providers (id, name, owner_id, org_id, home_id, kind, harness_kind, agent_id,
                                machine_id, arrangements, status)
         VALUES ($1, 'Other subscription', $2, $3, $3, 'subscription', 'fake', $4, $5,
                 ARRAY['separated'], 'ready')",
    )
    .bind(id)
    .bind(world.ada)
    .bind(world.root)
    .bind(world.agent)
    .bind(world.machine_id)
    .execute(world.pool.pool())
    .await
    .unwrap();
    id
}

// ---------------------------------------------------------------- shared caps

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_shared_cap_whose_room_is_used_up_blocks_the_thread_says_which_and_a_cap_raised_lets_the_turn_start()
 {
    let world = World::new().await;
    let thread = world.thread("Caps").await;
    let cap = Cap::tokens(1_000).on(world.project).make(&world).await;
    used(&world, thread, Some(world.ada), 999).await;

    // One token of room is room.
    send(&world, world.ada, thread, "First").await;
    let first = world.run_turn(thread, 0, "Done.").await;
    assert_eq!(common::words_after_where(&first.text), "First");
    world.wait_state(thread, "idle").await;

    used(&world, thread, Some(world.ada), 1).await;
    send(&world, world.ada, thread, "Second").await;
    world.wait_blocked(thread, "quota_exceeded").await;
    let detail = told(&world, thread).await;
    assert!(
        detail.starts_with(
            "Daily cap of project \"Web\" reached: 1 000 of 1 000 tokens. Starts again "
        ),
        "{detail}"
    );
    meanwhile(Wait::from_millis(300)).await;
    assert_eq!(world.turn_starts(thread).len(), 1);

    // Raising it takes effect at the next pre-flight.
    sqlx::query("UPDATE usage_caps SET amount = 5000, revision = revision + 1 WHERE id = $1")
        .bind(cap)
        .execute(world.pool.pool())
        .await
        .unwrap();
    world.engine().room_may_have_been_made();
    let second = world.turn_start(thread, 1).await;
    assert_eq!(common::words_after_where(&second.text), "Second");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_cap_counts_only_the_use_of_its_period_and_only_what_it_covers() {
    let world = World::new().await;
    let thread = world.thread("Periods").await;
    let elsewhere = world.topic("Elsewhere").await;
    Cap::tokens(100).on(world.project).make(&world).await;
    // Yesterday's use is before the day began.
    used_at(
        &world,
        thread,
        Some(world.ada),
        5_000,
        None,
        Utc::now() - Duration::days(1),
    )
    .await;
    // Full caps of another place, of another provider, of another model, of another agent, for another
    // person, that are off, or that have no limit cover nothing here.
    let others = another_provider(&world).await;
    let bob = world.person("bob").await;
    used(&world, thread, Some(world.ada), 1).await;
    Cap::tokens(0).on(elsewhere).make(&world).await;
    Cap::tokens(0)
        .on(world.project)
        .of_provider(others)
        .make(&world)
        .await;
    Cap::tokens(0)
        .on(world.project)
        .naming(&["other-model*"])
        .make(&world)
        .await;
    Cap::tokens(0)
        .on(world.project)
        .for_user(bob)
        .make(&world)
        .await;
    Cap::tokens(0)
        .on(world.project)
        .without_limit()
        .make(&world)
        .await;
    let off = Cap::tokens(0).on(world.project).make(&world).await;
    sqlx::query("UPDATE usage_caps SET enabled = false WHERE id = $1")
        .bind(off)
        .execute(world.pool.pool())
        .await
        .unwrap();
    sqlx::query("INSERT INTO usage_caps (id, node_id, meter, period, amount, harness_kinds, created_by) VALUES ($1, $2, 'tokens', 'day', 0, ARRAY['claude-code'], $3)")
        .bind(new_id())
        .bind(world.project)
        .bind(world.ada)
        .execute(world.pool.pool())
        .await
        .unwrap();
    send(&world, world.ada, thread, "Go").await;
    let start = world.turn_start(thread, 0).await;
    assert_eq!(common::words_after_where(&start.text), "Go");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_cap_in_credits_counts_the_credits_of_the_rows_and_use_without_a_rate_adds_nothing() {
    let world = World::new().await;
    let thread = world.thread("Credits").await;
    Cap::credits("10").on(world.project).make(&world).await;
    // A great many tokens that had no rate are in no sum of credits.
    used(&world, thread, Some(world.ada), 9_000_000).await;
    used_at(
        &world,
        thread,
        Some(world.ada),
        1_000,
        Some("9.999999"),
        Utc::now(),
    )
    .await;
    send(&world, world.ada, thread, "Under").await;
    world.run_turn(thread, 0, "Done.").await;
    world.wait_state(thread, "idle").await;

    used_at(
        &world,
        thread,
        Some(world.ada),
        1,
        Some("0.000001"),
        Utc::now(),
    )
    .await;
    send(&world, world.ada, thread, "At the limit").await;
    world.wait_blocked(thread, "quota_exceeded").await;
    let detail = told(&world, thread).await;
    assert!(
        detail.starts_with("Daily cap of project \"Web\" reached: 10 of 10 credits."),
        "{detail}"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_cap_that_cannot_count_a_model_without_a_rate_runs_the_turn_or_refuses_it_as_the_cap_says()
 {
    let world = World::new().await;
    let thread = world.thread("No rate").await;
    // The model has no rate: a cap in credits leaves its use out. `allow` goes on, `block` holds back.
    Cap::credits("10").on(world.project).make(&world).await;
    send(&world, world.ada, thread, "Allowed").await;
    world.run_turn(thread, 0, "Done.").await;
    world.wait_state(thread, "idle").await;

    let strict = Cap::credits("10")
        .on(world.project)
        .each()
        .blocking_what_it_cannot_count()
        .make(&world)
        .await;
    send(&world, world.ada, thread, "Refused").await;
    // Counted for a person, it holds the input: nothing but held inputs blocks the thread.
    world.wait_blocked(thread, "quota_exceeded").await;
    meanwhile(Wait::from_millis(300)).await;
    assert_eq!(world.turn_starts(thread).len(), 1);
    // Once the model has a rate, the use is counted and the turn runs.
    sqlx::query("UPDATE provider_models SET parameters = parameters || jsonb_build_object('rate', $1::jsonb)")
        .bind(json!({ "input_per_mtok": "2", "cached_input_per_mtok": "0.2", "output_per_mtok": "10" }))
        .execute(world.pool.pool())
        .await
        .unwrap();
    world.engine().room_may_have_been_made();
    let start = world.turn_start(thread, 1).await;
    assert_eq!(common::words_after_where(&start.text), "Refused");
    let _ = strict;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn use_that_nobody_reports_is_refused_by_a_cap_that_says_block_and_runs_under_one_that_says_allow()
 {
    let world = World::new().await;
    // The harness reports no usage of its own and does not pass the forwarder.
    sqlx::query(r#"UPDATE agents SET descriptor_overrides = '{"capabilities": {"usage_tokens": false}}'::jsonb"#)
        .execute(world.pool.pool())
        .await
        .unwrap();
    let thread = world.thread("Blind").await;
    let lenient = Cap::tokens(1_000_000).on(world.project).make(&world).await;
    send(&world, world.ada, thread, "Counted nowhere").await;
    world.run_turn(thread, 0, "Done.").await;
    world.wait_state(thread, "idle").await;

    sqlx::query("DELETE FROM usage_caps WHERE id = $1")
        .bind(lenient)
        .execute(world.pool.pool())
        .await
        .unwrap();
    Cap::tokens(1_000_000)
        .on(world.project)
        .blocking_what_it_cannot_count()
        .make(&world)
        .await;
    send(&world, world.ada, thread, "Refused").await;
    world.wait_blocked(thread, "quota_exceeded").await;
    let detail = told(&world, thread).await;
    assert!(
        detail.contains("nobody reports what this turn uses"),
        "{detail}"
    );
}

// ---------------------------------------------------------------- caps counted per person

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_persons_own_cap_holds_their_input_while_a_teammates_goes_ahead_and_it_is_released_when_there_is_room()
 {
    let world = World::new().await;
    let bob = teammate(&world, "bob", "interactor").await;
    let thread = world.thread("Allowance").await;
    let cap = Cap::tokens(1_000)
        .on(world.project)
        .each()
        .make(&world)
        .await;
    used(&world, thread, Some(bob), 1_000).await;

    let held = send(&world, bob, thread, "From bob").await;
    world.wait_blocked(thread, "quota_exceeded").await;
    let (state, detail) = world.input_state(held.input.id).await;
    assert_eq!(
        (state.as_str(), detail.as_deref()),
        ("pending", Some("cap_reached"))
    );

    // A teammate's input is not stopped by it.
    send(&world, world.ada, thread, "From ada").await;
    let theirs = world.turn_start(thread, 0).await;
    assert_eq!(common::words_after_where(&theirs.text), "From ada");
    world.answer(thread, &theirs, "m1", "Done.").await;
    world
        .say(thread, turn_ended(theirs.turn_id, TurnOutcome::Completed))
        .await;
    world.wait_turn(theirs.turn_id, "completed").await;
    world.wait_blocked(thread, "quota_exceeded").await;
    assert_eq!(world.input_state(held.input.id).await.0, "pending");

    // Raised for them, it goes ahead.
    sqlx::query("UPDATE usage_caps SET amount = 5000, revision = revision + 1 WHERE id = $1")
        .bind(cap)
        .execute(world.pool.pool())
        .await
        .unwrap();
    world.engine().room_may_have_been_made();
    let second = world.turn_start(thread, 1).await;
    assert!(second.text.contains("From bob"), "{}", second.text);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_cap_that_names_a_person_or_a_group_with_no_limit_exempts_them_from_the_cap_for_everyone()
 {
    let world = World::new().await;
    let (bob, carol, dave) = (
        teammate(&world, "bob", "interactor").await,
        teammate(&world, "carol", "interactor").await,
        teammate(&world, "dave", "interactor").await,
    );
    let group = new_id();
    sqlx::query("INSERT INTO groups (id, org_id, name, created_by) VALUES ($1, $2, 'Backend', $3)")
        .bind(group)
        .bind(world.root)
        .bind(world.ada)
        .execute(world.pool.pool())
        .await
        .unwrap();
    sqlx::query("INSERT INTO group_members (group_id, user_id) VALUES ($1, $2)")
        .bind(group)
        .bind(bob)
        .execute(world.pool.pool())
        .await
        .unwrap();
    // 100 tokens a day for each person; bob's group has no limit; carol is named with 50 000.
    Cap::tokens(100).on(world.project).each().make(&world).await;
    Cap::tokens(0)
        .on(world.project)
        .for_group(group)
        .without_limit()
        .make(&world)
        .await;
    Cap::tokens(50_000)
        .on(world.project)
        .for_user(carol)
        .make(&world)
        .await;
    let threads = [
        world.thread("Bob").await,
        world.thread("Carol").await,
        world.thread("Dave").await,
    ];
    for (person, thread) in [bob, carol, dave].into_iter().zip(threads) {
        used(&world, thread, Some(person), 500).await;
        send(&world, person, thread, "Go").await;
    }
    world.turn_start(threads[0], 0).await;
    world.turn_start(threads[1], 0).await;
    world.wait_blocked(threads[2], "quota_exceeded").await;
    meanwhile(Wait::from_millis(300)).await;
    assert_eq!(world.turn_starts(threads[2]).len(), 0);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_cap_for_a_role_covers_that_role_and_higher_roles_but_not_a_lower_one() {
    let world = World::new().await;
    let bob = teammate(&world, "bob", "developer").await;
    let carol = teammate(&world, "carol", "interactor").await;
    Cap::tokens(100)
        .on(world.project)
        .for_role("developer")
        .make(&world)
        .await;
    let (theirs, mine) = (world.thread("Bob").await, world.thread("Ada").await);
    used(&world, theirs, Some(bob), 500).await;
    used(&world, mine, Some(world.ada), 500).await;
    send(&world, bob, theirs, "From a developer").await;
    send(&world, world.ada, mine, "From an admin").await;
    // A role holder includes people with that role or a higher one.
    world.wait_blocked(theirs, "quota_exceeded").await;
    world.wait_blocked(mine, "quota_exceeded").await;
    assert_eq!(world.turn_starts(theirs).len(), 0);
    assert_eq!(world.turn_starts(mine).len(), 0);
    let lower = world.thread("Carol").await;
    used(&world, lower, Some(carol), 500).await;
    send(&world, carol, lower, "From an interactor").await;
    let start = world.turn_start(lower, 0).await;
    assert!(start.text.contains("From an interactor"), "{}", start.text);
}

// ---------------------------------------------------------------- turns at once

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_cap_on_turns_at_once_makes_threads_wait_and_releases_them_in_the_order_of_their_oldest_input()
 {
    let clock = std::sync::Arc::new(orch_testkit::clock::manual());
    let world = World::with_clock(clock.clone()).await;
    Cap::running(1).on(world.topic).make(&world).await;
    let (first, second, third) = (
        world.thread("First").await,
        world.thread("Second").await,
        world.thread("Third").await,
    );
    let (running, _) = world.running_turn(first, "A").await;
    send(&world, world.ada, second, "B").await;
    world.wait_blocked(second, "quota_exceeded").await;
    // One wire timestamp step makes the second input older, independent of scheduler speed.
    const NEXT_INPUT: Duration = Duration::milliseconds(1);
    clock.advance(NEXT_INPUT);
    send(&world, world.ada, third, "C").await;
    world.wait_blocked(third, "quota_exceeded").await;
    let detail = told(&world, second).await;
    assert!(detail.contains("cap of 1 turns at once"), "{detail}");

    // The first turn ends: the thread that waited longest goes ahead, the other waits.
    world
        .say(first, turn_ended(running.turn_id, TurnOutcome::Completed))
        .await;
    assert_eq!(
        common::words_after_where(&world.turn_start(second, 0).await.text),
        "B"
    );
    meanwhile(Wait::from_millis(400)).await;
    assert_eq!(world.turn_starts(third).len(), 0, "one at a time");
    let (state, reason, _) = world.state(third).await;
    assert_eq!(
        (state.as_str(), reason.as_deref()),
        ("blocked", Some("quota_exceeded"))
    );

    let second_turn = world.turn_start(second, 0).await;
    world.say(second, turn_began(second_turn.turn_id)).await;
    world
        .say(
            second,
            turn_ended(second_turn.turn_id, TurnOutcome::Completed),
        )
        .await;
    assert_eq!(
        common::words_after_where(&world.turn_start(third, 0).await.text),
        "C"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn fifty_threads_that_start_at_once_under_a_cap_of_three_make_three_turns() {
    let world = World::new().await;
    Cap::running(3).on(world.topic).make(&world).await;
    let mut threads = Vec::new();
    for number in 0..50 {
        threads.push(world.thread(&format!("Thread {number}")).await);
    }
    let engine = world.engine();
    let mut tasks = Vec::new();
    for thread in &threads {
        let (engine, thread, ada) = (engine.clone(), *thread, world.ada);
        tasks.push(tokio::spawn(async move {
            // The API exposes a rolled-back transaction race as a retryable conflict. Keep
            // the same client id on retry, exactly as a client does; other errors still fail.
            let request = queued("Go");
            // Bound the database await as well as the retry loop, so a blocked statement
            // cannot keep the spawned task (and then its join) alive forever.
            tokio::time::timeout(common::PATIENCE, async {
                loop {
                    match engine
                        .submit_input(&Caller::session(ada), thread, request.clone())
                        .await
                    {
                        Ok(_) => break,
                        Err(InputError::Internal(orch_engine::EngineError::Db(
                            orch_db::DbError::Retry,
                        ))) => {
                            tokio::task::yield_now().await;
                        }
                        Err(error) => panic!("submission failed: {error:?}"),
                    }
                }
            })
            .await
            .expect("the same input was not accepted within the bounded retry window");
        }));
    }
    for task in tasks {
        task.await.unwrap();
    }
    common::eventually("all fifty submissions to reach their admitted or capped outcome", || async {
        let running = count(&world, "SELECT count(*) FROM turns").await;
        let waiting = count(&world, "SELECT count(*) FROM threads WHERE state = 'blocked' AND blocked_reason = 'quota_exceeded'").await;
        running + waiting == 50
    }).await;
    let turns = count(&world, "SELECT count(*) FROM turns").await;
    assert_eq!(
        count(&world, "SELECT count(*) FROM thread_inputs").await,
        50,
        "retries never duplicate authored inputs"
    );
    assert_eq!(turns, 3, "no more than the cap says start");
    let waiting = count(
        &world,
        "SELECT count(*) FROM threads WHERE state = 'blocked' AND blocked_reason = 'quota_exceeded'",
    )
    .await;
    assert_eq!(waiting, 47);
}

// ---------------------------------------------------------------- while a turn runs

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_running_turn_is_stopped_when_a_report_of_its_usage_shows_the_cap_reached_and_the_next_input_waits()
 {
    let clock = Arc::new(clock::manual());
    let world = World::with_clock(clock.clone()).await;
    let check_every = orch_engine::config::EngineConfig::default().cap_check;
    world.set_config(|config| config.cap_check = check_every);
    world.restart_api().await;
    let thread = world.thread("Stopped").await;
    Cap::tokens(1_000).on(world.topic).make(&world).await;
    let (start, _) = world.running_turn(thread, "Go").await;
    world.say(thread, reported(start.turn_id, 400)).await;
    common::eventually("the below-cap usage report to be committed", || async {
        let reported: Option<i64> = world
            .scalar(
                "SELECT (usage->>'total_tokens')::bigint FROM turns WHERE id = $1",
                start.turn_id,
            )
            .await;
        reported == Some(400)
    })
    .await;
    assert_eq!(world.turn_state(start.turn_id).await, "running", "not yet");
    // The next report is eligible for the deployed cap-check cadence even
    // when the machine running this test has only just processed the first.
    clock.advance(Duration::from_std(check_every).unwrap());
    world.say(thread, reported(start.turn_id, 1_500)).await;
    world.wait_turn(start.turn_id, "interrupted").await;
    let code: String = world
        .scalar(
            "SELECT error->>'code' FROM turns WHERE id = $1",
            start.turn_id,
        )
        .await;
    assert_eq!(code, "cap_reached");
    let item = count(
        &world,
        "SELECT count(*) FROM items WHERE kind = 'error' AND payload->>'code' = 'cap_reached'",
    )
    .await;
    assert_eq!(item, 1, "the timeline says why");
    // Nothing is queued again; the next input meets the cap.
    send(&world, world.ada, thread, "Again").await;
    world.wait_blocked(thread, "quota_exceeded").await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_cap_made_below_what_a_running_turn_has_used_stops_it_as_soon_as_the_engine_is_told() {
    let world = World::new().await;
    let thread = world.thread("Told").await;
    let (start, _) = world.running_turn(thread, "Go").await;
    world.say(thread, reported(start.turn_id, 1_500)).await;
    meanwhile(Wait::from_millis(300)).await;
    assert_eq!(
        world.turn_state(start.turn_id).await,
        "running",
        "no cap covers it yet"
    );

    let cap = Cap::tokens(1_000).on(world.topic).make(&world).await;
    world.engine().cap_changed(cap).await.unwrap();

    world.wait_turn(start.turn_id, "interrupted").await;
    let code: String = world
        .scalar(
            "SELECT error->>'code' FROM turns WHERE id = $1",
            start.turn_id,
        )
        .await;
    assert_eq!(code, "cap_reached", "without another report of usage");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_cap_that_lets_the_turn_finish_stops_nothing_and_holds_the_next_input() {
    let world = World::new().await;
    let thread = world.thread("Finished").await;
    Cap::tokens(1_000)
        .on(world.topic)
        .finishing()
        .make(&world)
        .await;
    let (start, _) = world.running_turn(thread, "Go").await;
    world.say(thread, reported(start.turn_id, 1_500)).await;
    meanwhile(Wait::from_millis(500)).await;
    assert_eq!(world.turn_state(start.turn_id).await, "running");
    world
        .say(thread, turn_ended(start.turn_id, TurnOutcome::Completed))
        .await;
    world.wait_turn(start.turn_id, "completed").await;
    send(&world, world.ada, thread, "Next").await;
    world.wait_blocked(thread, "quota_exceeded").await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_lease_of_a_forwarded_turn_carries_the_room_that_the_tightest_cap_which_would_stop_it_leaves()
 {
    let world = World::on_a_key("sk-orch-test-allowance").await;
    sqlx::query("UPDATE provider_models SET parameters = parameters || jsonb_build_object('rate', $1::jsonb)")
        .bind(json!({ "input_per_mtok": "2", "cached_input_per_mtok": "0.2", "output_per_mtok": "10" }))
        .execute(world.pool.pool())
        .await
        .unwrap();
    let thread = world.thread("Allowance").await;
    used_at(
        &world,
        thread,
        Some(world.ada),
        2_500,
        Some("1.500000"),
        Utc::now(),
    )
    .await;
    Cap::tokens(10_000).on(world.topic).make(&world).await;
    // A cap that lets a turn finish is not in the way of one that runs.
    Cap::tokens(3_000)
        .on(world.topic)
        .finishing()
        .make(&world)
        .await;
    Cap::credits("5").on(world.topic).make(&world).await;
    send(&world, world.ada, thread, "Go").await;
    world.turn_start(thread, 0).await;
    let allowance: Json = world
        .scalar(
            "SELECT allowance FROM leases WHERE thread_id = $1 AND closed_at IS NULL",
            thread,
        )
        .await;
    assert_eq!(allowance["tokens"], 7_500);
    assert_eq!(allowance["credits"]["amount"], "3.5");
    assert_eq!(allowance["credits"]["rate"]["input_per_mtok"], "2");
    let sent = world.commands.issued_bodies("lease.open");
    assert_eq!(sent.len(), 1);
    assert_eq!(sent[0].1["allowance"]["tokens"], 7_500);
}

// ---------------------------------------------------------------- letting a turn past a cap

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_admin_lets_a_turn_past_a_cap_that_is_reached_it_is_written_down_and_only_another_cap_stops_it()
 {
    let world = World::new().await;
    let thread = world.thread("Past the cap").await;
    let cap = Cap::tokens(1_000).on(world.project).make(&world).await;
    used(&world, thread, Some(world.ada), 1_000).await;
    let plain = send(&world, world.ada, thread, "Plain").await;
    world.wait_blocked(thread, "quota_exceeded").await;
    world
        .engine()
        .cancel_input(&Caller::session(world.ada), thread, plain.input.id)
        .await
        .unwrap();

    send_past(&world, world.ada, thread, "Regardless", &[cap])
        .await
        .unwrap();
    let start = world.turn_start(thread, 0).await;
    assert_eq!(common::words_after_where(&start.text), "Regardless");

    // Written down: a row, the line of the audit trail, the marker, and the entry on the turn.
    let row: (Option<Id>, Id, Option<Id>, String, String, String) = sqlx::query_as(
        "SELECT cap_id, by_user, author_id, meter, used::text, amount::text FROM usage_limit_bypasses",
    )
    .fetch_one(world.pool.pool())
    .await
    .unwrap();
    assert_eq!(
        row,
        (
            Some(cap),
            world.ada,
            Some(world.ada),
            "tokens".to_owned(),
            "1000".to_owned(),
            "1000".to_owned()
        )
    );
    assert_eq!(
        count(
            &world,
            "SELECT count(*) FROM audit_log WHERE action = 'usage.limit_bypassed'"
        )
        .await,
        1
    );
    assert_eq!(
        count(&world, "SELECT count(*) FROM items WHERE kind = 'marker' AND payload->>'marker' = 'limit_bypassed'").await,
        1
    );
    let entries: Json = world
        .scalar(
            "SELECT limit_bypass FROM turns WHERE id = $1",
            start.turn_id,
        )
        .await;
    assert_eq!(entries[0]["caps"], json!([cap]));

    // The cap stays as it was, and the turn it was passed for is not stopped by it.
    let amount: String = world
        .scalar("SELECT amount::text FROM usage_caps WHERE id = $1", cap)
        .await;
    assert_eq!(amount, "1000");
    world.say(thread, turn_began(start.turn_id)).await;
    world.say(thread, reported(start.turn_id, 600)).await;
    meanwhile(Wait::from_millis(500)).await;
    assert_eq!(world.turn_state(start.turn_id).await, "running");
    // Another cap that is reached stops it.
    Cap::tokens(300).on(world.topic).make(&world).await;
    world.say(thread, reported(start.turn_id, 700)).await;
    world.wait_turn(start.turn_id, "interrupted").await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn somebody_without_the_row_cannot_let_a_turn_past_but_a_holder_releases_the_held_input_of_a_teammate()
 {
    let world = World::new().await;
    let bob = teammate(&world, "bob", "developer").await;
    let thread = world.thread("Held").await;
    let cap = Cap::tokens(1_000)
        .on(world.project)
        .each()
        .make(&world)
        .await;
    used(&world, thread, Some(bob), 1_000).await;

    // A Developer does not hold the row as shipped.
    let refused = send_past(&world, bob, thread, "Please", &[cap]).await;
    assert!(matches!(refused, Err(InputError::Denied(_))), "{refused:?}");
    let held = send(&world, bob, thread, "From bob").await;
    world.wait_blocked(thread, "quota_exceeded").await;
    assert_eq!(
        world.input_state(held.input.id).await.1.as_deref(),
        Some("cap_reached")
    );

    // An Admin who may put its queue in order lets it through: the turn is bob's, and the record says who.
    world
        .engine()
        .bypass_input(
            &Caller::session(world.ada),
            thread,
            held.input.id,
            &LimitBypassRequest {
                caps: Some(vec![cap]),
                provider_limit: None,
            },
        )
        .await
        .unwrap();
    let start = world.turn_start(thread, 0).await;
    assert!(start.text.contains("From bob"), "{}", start.text);
    let row: (Id, Option<Id>) =
        sqlx::query_as("SELECT by_user, author_id FROM usage_limit_bypasses WHERE cap_id = $1")
            .bind(cap)
            .fetch_one(world.pool.pool())
            .await
            .unwrap();
    assert_eq!(row, (world.ada, Some(bob)));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_turn_under_a_projects_cap_and_a_providers_cap_waits_between_the_two_who_hold_one_row_each()
 {
    let world = World::new().await;
    let carol = teammate(&world, "carol", "interactor").await;
    // Carol is an owner of the provider and an Interactor in the topic; Ada is an Admin there and no owner of it.
    let mut conn = world.pool.acquire().await.unwrap();
    add_entry(
        &mut conn,
        &PageKey::new("provider", world.provider, "live"),
        "any",
        "access.own",
        ("user", Some(carol), None),
        None,
        Utc::now(),
    )
    .await
    .unwrap();
    // World creates a real provider Owners row now. This case explicitly gives that
    // ownership to Carol alone; owner_id remains creation attribution, not authority.
    sqlx::query(
        "UPDATE permission_entries SET removed_at=$3, removed_by=$2, revision=revision+1
        WHERE object_kind='provider' AND object_id=$1 AND page='live' AND permission='access.own'
          AND holder_kind='user' AND holder_id=$2 AND removed_at IS NULL",
    )
    .bind(world.provider)
    .bind(world.ada)
    .bind(Utc::now())
    .execute(&mut *conn)
    .await
    .unwrap();
    drop(conn);
    let thread = world.thread("Two caps").await;
    let project_cap = Cap::tokens(1_000).on(world.project).make(&world).await;
    let provider_cap = Cap::tokens(1_000)
        .of_provider(world.provider)
        .make(&world)
        .await;
    used(&world, thread, Some(world.ada), 1_000).await;

    // Naming a cap whose row one does not hold refuses the whole request.
    let both = send_past(
        &world,
        world.ada,
        thread,
        "Both",
        &[project_cap, provider_cap],
    )
    .await;
    assert!(matches!(both, Err(InputError::Denied(_))), "{both:?}");
    // Ada lets it past hers; it waits for the other.
    let waiting = send_past(&world, world.ada, thread, "Mine", &[project_cap])
        .await
        .unwrap();
    world.wait_blocked(thread, "quota_exceeded").await;
    let detail = told(&world, thread).await;
    assert!(detail.contains("provider"), "{detail}");
    meanwhile(Wait::from_millis(300)).await;
    assert_eq!(world.turn_starts(thread).len(), 0);

    // Carol lets it past the provider's, and it starts. Each is named for what they passed.
    world
        .engine()
        .bypass_input(
            &Caller::session(carol),
            thread,
            waiting.input.id,
            &LimitBypassRequest {
                caps: Some(vec![provider_cap]),
                provider_limit: None,
            },
        )
        .await
        .unwrap();
    assert_eq!(
        common::words_after_where(&world.turn_start(thread, 0).await.text),
        "Mine"
    );
    let mut passed: Vec<(Option<Id>, Id)> =
        sqlx::query_as("SELECT cap_id, by_user FROM usage_limit_bypasses")
            .fetch_all(world.pool.pool())
            .await
            .unwrap();
    passed.sort();
    let mut wanted = vec![(Some(project_cap), world.ada), (Some(provider_cap), carol)];
    wanted.sort();
    assert_eq!(passed, wanted);
    assert_eq!(
        count(&world, "SELECT count(*) FROM items WHERE kind = 'marker' AND payload->>'marker' = 'limit_bypassed'").await,
        2,
        "a line in the thread for each of them"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_input_nobody_sent_is_never_let_past_a_limit() {
    let world = World::new().await;
    let thread = world.thread("System").await;
    let cap = Cap::tokens(1_000).on(world.project).make(&world).await;
    used(&world, thread, None, 1_000).await;
    let notice = world
        .engine()
        .queue_system_input(
            thread,
            "system",
            &[ContentPart::Text {
                text: "A notice".to_owned(),
            }],
        )
        .await
        .unwrap();
    world.wait_blocked(thread, "quota_exceeded").await;
    let tried = world
        .engine()
        .bypass_input(
            &Caller::session(world.ada),
            thread,
            notice.id,
            &LimitBypassRequest {
                caps: Some(vec![cap]),
                provider_limit: None,
            },
        )
        .await;
    assert!(matches!(tried, Err(InputError::Invalid(_))), "{tried:?}");
    meanwhile(Wait::from_millis(300)).await;
    assert_eq!(world.turn_starts(thread).len(), 0);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_entry_whose_holder_lost_the_row_before_the_turn_started_is_dropped_and_the_input_waits_as_before()
 {
    let world = World::new().await;
    let bob = teammate(&world, "bob", "developer").await;
    let thread = world.thread("Lost the row").await;
    let project_cap = Cap::tokens(1_000).on(world.project).make(&world).await;
    let provider_cap = Cap::tokens(1_000)
        .of_provider(world.provider)
        .make(&world)
        .await;
    used(&world, thread, Some(bob), 1_000).await;
    // Bob holds the row for the project's caps; nobody holds the provider's.
    let mut conn = world.pool.acquire().await.unwrap();
    let row = add_entry(
        &mut conn,
        &PageKey::new("node", world.project, "live"),
        "any",
        "usage.bypass",
        ("user", Some(bob), None),
        None,
        Utc::now(),
    )
    .await
    .unwrap();
    drop(conn);
    let waiting = send_past(&world, bob, thread, "Mine", &[project_cap])
        .await
        .unwrap();
    world.wait_blocked(thread, "quota_exceeded").await;
    assert!(
        told(&world, thread).await.contains("provider"),
        "the project's cap is passed, the provider's is not"
    );
    let entries: Json = world
        .scalar(
            "SELECT limit_bypass FROM thread_inputs WHERE id = $1",
            waiting.input.id,
        )
        .await;
    assert_eq!(entries[0]["caps"], json!([project_cap]));

    // The row is taken from him. At the next pre-flight the entry is gone.
    sqlx::query("UPDATE permission_entries SET removed_at = now() WHERE id = $1")
        .bind(row)
        .execute(world.pool.pool())
        .await
        .unwrap();
    world.engine().room_may_have_been_made();
    let pool = world.pool.clone();
    let input = waiting.input.id;
    common::eventually("the entry is dropped", move || {
        let pool = pool.clone();
        async move {
            let kept: i64 = sqlx::query_scalar(
                "SELECT count(*) FROM thread_inputs WHERE id = $1 AND limit_bypass IS NOT NULL
                    AND jsonb_array_length(limit_bypass) > 0",
            )
            .bind(input)
            .fetch_one(pool.pool())
            .await
            .unwrap();
            kept == 0
        }
    })
    .await;
    assert_eq!(world.turn_starts(thread).len(), 0);
    let _ = provider_cap;
}

// ---------------------------------------------------------------- telling people

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_warning_and_the_word_that_a_cap_was_reached_are_sent_once_for_each_period() {
    let world = World::new().await;
    let thread = world.thread("Told").await;
    Cap::tokens(1_000)
        .on(world.project)
        .warning_at(80)
        .make(&world)
        .await;
    let notices = |kind: &'static str| {
        let sql = format!(
            "SELECT count(*) FROM notifications WHERE user_id = '{}' AND kind = '{kind}'",
            world.ada
        );
        let world = &world;
        async move { count(world, &sql).await }
    };
    assert_eq!(notices("cap.warning").await, 0);
    used(&world, thread, Some(world.ada), 850).await;
    send(&world, world.ada, thread, "First").await;
    world.run_turn(thread, 0, "Done.").await;
    world.wait_state(thread, "idle").await;
    assert_eq!(notices("cap.warning").await, 1);
    send(&world, world.ada, thread, "Second").await;
    world.run_turn(thread, 1, "Done.").await;
    world.wait_state(thread, "idle").await;
    assert_eq!(notices("cap.warning").await, 1, "once for the period");
    assert_eq!(notices("cap.reached").await, 0);

    used(&world, thread, Some(world.ada), 200).await;
    send(&world, world.ada, thread, "Third").await;
    world.wait_blocked(thread, "quota_exceeded").await;
    meanwhile(Wait::from_millis(700)).await;
    assert_eq!(notices("cap.reached").await, 1);
    assert_eq!(
        count(&world, "SELECT count(*) FROM audit_log WHERE action = 'usage_cap.reached' AND actor_kind = 'system'").await,
        1
    );
    assert_eq!(
        count(&world, "SELECT count(*) FROM usage_cap_notices").await,
        2
    );
}
