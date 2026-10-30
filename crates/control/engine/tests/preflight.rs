//! Everything that stands between an input and a harness is checked in order, and the first thing that fails keeps the input waiting with its reason, until its cause clears.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]

mod common;

use common::{World, eventually, meanwhile, queued, turn_ended};
use orch_api_contract::threads::{ContentPart, LimitBypassRequest};
use orch_authz::Caller;
use orch_engine::inputs::InputError;
use orch_link_contract::commands::CommandKind;
use orch_link_contract::events::TurnOutcome;
use orch_settings_contract::registry::keys;
use orch_types::Id;
use serde_json::json;
use std::time::Duration;

/// What is wrong, and how it is put right.
#[derive(Clone, Copy)]
enum Cause {
    /// Statements that break and mend it.
    Sql(&'static str, &'static str),
    /// The thread has no model.
    NoModel,
    /// The thread asks for an effort the model does not list.
    UnlistedEffort,
    /// The place allows only models the provider does not offer.
    ModelsNarrowed,
    /// The place allows only an approval mode the thread does not use.
    ModesNarrowed,
    /// A usage cap covers the turn.
    UsageCap,
}

struct Case {
    name: &'static str,
    reason: &'static str,
    cause: Cause,
}

const ALL_MODES: &str = r#"UPDATE agents SET descriptor = jsonb_set(descriptor, '{approval_modes}', '["ask","on_request","bypass"]')"#;
const ALL_ARRANGEMENTS: &str = r#"UPDATE agents SET descriptor = jsonb_set(descriptor, '{arrangements}', '[{"type":"together"},{"type":"separated","executor":"orch-exec","executor_version":"*","tool_sets":["plain"]}]')"#;

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_profile_pin_excludes_a_different_running_build_before_another_turn_opens() {
    let world = World::observed().await;
    let thread = world.thread("Pinned program").await;
    let caller = Caller::session(world.ada);
    world
        .engine()
        .submit_input(&caller, thread, queued("First"))
        .await
        .unwrap();
    let first = world.turn_start(thread, 0).await;
    world
        .say(thread, turn_ended(first.turn_id, TurnOutcome::Completed))
        .await;
    world.wait_state(thread, "idle").await;
    world
        .exec("UPDATE agents SET profile = '{\"harness_version\":\"9.0.0\"}'")
        .await;
    let submitted = world
        .engine()
        .submit_input(&caller, thread, queued("Pinned"))
        .await
        .unwrap();
    world.wait_blocked(thread, "binding_invalid").await;
    assert_eq!(world.turn_starts(thread).len(), 1);
    assert_eq!(world.input_state(submitted.input.id).await.0, "pending");
    let detail: Option<String> = world
        .scalar(
            "SELECT blocked_detail FROM threads WHERE node_id=$1",
            thread,
        )
        .await;
    let detail = detail.unwrap_or_default();
    assert!(
        detail.contains("1.0.0") && detail.contains("9.0.0"),
        "{detail}"
    );
    world.exec("UPDATE agents SET descriptor=jsonb_set(descriptor,'{capabilities,discover_sessions}','true')").await;
    let starts = world.commands.issued_commands("agent.start").len();
    let discovered = world
        .engine()
        .discover_sessions(world.provider, world.agent)
        .await
        .unwrap();
    let why = match discovered {
        orch_engine::harnesses::Discovered::Failed(why) => Some(why),
        _ => None,
    }
    .expect("an excluded warm program must not be reused for session discovery");
    assert!(why.contains("1.0.0") && why.contains("9.0.0"), "{why}");
    assert_eq!(
        world.commands.issued_commands("agent.start").len(),
        starts,
        "the pin does not silently restart an already running program"
    );
    world
        .exec("UPDATE agents SET profile = '{\"harness_version\":\"1.0.0\"}'")
        .await;
    world.engine().preflight(thread).await.unwrap();
    let second = world.turn_start(thread, 1).await;
    world
        .say(thread, turn_ended(second.turn_id, TurnOutcome::Completed))
        .await;
    world.wait_state(thread, "idle").await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_first_report_that_misses_the_profile_pin_opens_no_session_and_keeps_the_input_for_a_matching_build()
 {
    let world = World::pinned("9.0.0").await;
    let thread = world.thread("First pinned start").await;
    let caller = Caller::session(world.ada);
    let submitted = world
        .engine()
        .submit_input(&caller, thread, queued("Keep this input"))
        .await
        .unwrap();
    eventually(
        "the first report names its excluded and required builds",
        || async {
            let (state, reason, _) = world.state(thread).await;
            let detail: Option<String> = world
                .scalar(
                    "SELECT blocked_detail FROM threads WHERE node_id=$1",
                    thread,
                )
                .await;
            let refused: bool = world.scalar("SELECT EXISTS(SELECT 1 FROM harnesses h JOIN threads t ON t.agent_id=h.agent_id AND t.provider_id=h.provider_id WHERE t.node_id=$1 AND h.status='error' AND h.after_up_plan->'version_pin_refusal'->>'actual'='1.0.0' AND h.after_up_plan->'version_pin_refusal'->>'required'='9.0.0')", thread).await;
            state == "blocked"
                && reason.as_deref() == Some("binding_invalid")
                && detail.is_some_and(|detail| detail.contains("1.0.0") && detail.contains("9.0.0"))
                && refused
        },
    )
    .await;
    assert!(world.commands.issued_commands("session.open").is_empty());
    assert!(world.turn_starts(thread).is_empty());
    assert_eq!(world.input_state(submitted.input.id).await.0, "pending");
    let detail: Option<String> = world
        .scalar(
            "SELECT blocked_detail FROM threads WHERE node_id=$1",
            thread,
        )
        .await;
    let detail = detail.unwrap_or_default();
    assert!(
        detail.contains("1.0.0") && detail.contains("9.0.0"),
        "{detail}"
    );
    world
        .exec("WITH changed AS (UPDATE agents SET profile = '{\"harness_version\":\"1.0.0\"}' RETURNING id) UPDATE harnesses SET status='error',status_detail='An independent startup failure' WHERE agent_id IN (SELECT id FROM changed)")
        .await;
    world.engine().preflight(thread).await.unwrap();
    let (state, reason, _) = world.state(thread).await;
    assert_eq!(state, "blocked");
    assert_eq!(reason.as_deref(), Some("binding_invalid"));
    let independent: bool = world.scalar("SELECT EXISTS(SELECT 1 FROM harnesses WHERE agent_id=(SELECT agent_id FROM threads WHERE node_id=$1) AND status='error' AND status_detail='An independent startup failure')", thread).await;
    assert!(independent);
    assert!(world.commands.issued_commands("session.open").is_empty());
    assert_eq!(world.input_state(submitted.input.id).await.0, "pending");
    world
        .exec("UPDATE harnesses SET status_detail=after_up_plan->'version_pin_refusal'->>'detail'")
        .await;
    world.engine().preflight(thread).await.unwrap();
    let started = world.turn_start(thread, 0).await;
    assert_eq!(common::words_after_where(&started.text), "Keep this input");
    world
        .say(thread, turn_ended(started.turn_id, TurnOutcome::Completed))
        .await;
    world.wait_state(thread, "idle").await;
}

/// One row for each check that can fail, in the order the checks run.
const CASES: &[Case] = &[
    Case {
        name: "the thread is bound to nothing",
        reason: "unbound",
        cause: Cause::Sql(
            "UPDATE threads SET agent_id = NULL, provider_id = NULL",
            "UPDATE threads SET agent_id = (SELECT id FROM agents), provider_id = (SELECT id FROM providers)",
        ),
    },
    Case {
        name: "the agent profile is switched off",
        reason: "agent_disabled",
        cause: Cause::Sql(
            "UPDATE agents SET disabled = true",
            "UPDATE agents SET disabled = false",
        ),
    },
    Case {
        name: "the provider is switched off",
        reason: "provider_unavailable",
        cause: Cause::Sql(
            "UPDATE providers SET status = 'disabled'",
            "UPDATE providers SET status = 'ready'",
        ),
    },
    Case {
        name: "the provider has not been set up",
        reason: "provider_unavailable",
        cause: Cause::Sql(
            "UPDATE providers SET status = 'needs_setup'",
            "UPDATE providers SET status = 'ready'",
        ),
    },
    Case {
        name: "the provider's allowance is used up",
        reason: "provider_exhausted",
        cause: Cause::Sql(
            "UPDATE providers SET status = 'exhausted', status_detail = 'this month is used'",
            "UPDATE providers SET status = 'ready', status_detail = NULL",
        ),
    },
    Case {
        name: "the thread has no model",
        reason: "model_not_set",
        cause: Cause::NoModel,
    },
    Case {
        name: "the provider does not offer the model any more",
        reason: "model_not_allowed",
        cause: Cause::Sql(
            "UPDATE provider_models SET enabled = false",
            "UPDATE provider_models SET enabled = true",
        ),
    },
    Case {
        name: "the model does not list the effort",
        reason: "model_not_allowed",
        cause: Cause::UnlistedEffort,
    },
    Case {
        name: "the place allows other models only",
        reason: "model_not_allowed",
        cause: Cause::ModelsNarrowed,
    },
    Case {
        name: "the place does not allow the approval mode",
        reason: "binding_invalid",
        cause: Cause::ModesNarrowed,
    },
    Case {
        name: "the agent lacks the approval mode",
        reason: "binding_invalid",
        cause: Cause::Sql(
            r#"UPDATE agents SET descriptor = jsonb_set(descriptor, '{approval_modes}', '["bypass"]')"#,
            ALL_MODES,
        ),
    },
    Case {
        name: "the environment failed",
        reason: "environment_unavailable",
        cause: Cause::Sql(
            "UPDATE environments SET status = 'failed', status_detail = 'the unit is gone'",
            "UPDATE environments SET status = 'running', status_detail = NULL",
        ),
    },
    Case {
        name: "the environment's machine is offline",
        reason: "environment_unavailable",
        cause: Cause::Sql(
            "UPDATE machines SET online = false",
            "UPDATE machines SET online = true",
        ),
    },
    Case {
        name: "the adapter offers no arrangement",
        reason: "binding_invalid",
        cause: Cause::Sql(
            r#"UPDATE agents SET descriptor = jsonb_set(descriptor, '{arrangements}', '[]')"#,
            ALL_ARRANGEMENTS,
        ),
    },
    Case {
        name: "the machine does not take the role of an agent",
        reason: "agent_offline",
        cause: Cause::Sql(
            "UPDATE machines SET roles = ARRAY['environment','provider']",
            "UPDATE machines SET roles = ARRAY['agent','environment','provider']",
        ),
    },
    Case {
        name: "a usage cap covers the turn",
        reason: "quota_exceeded",
        cause: Cause::UsageCap,
    },
];

async fn break_it(world: &World, thread: Id, cause: Cause) {
    match cause {
        Cause::Sql(broken, _) => world.exec(broken).await,
        Cause::NoModel => world.unset(thread, keys::AGENT_MODEL).await,
        Cause::UnlistedEffort => world.set(thread, keys::AGENT_EFFORT, json!("ultra")).await,
        Cause::ModelsNarrowed => {
            world
                .set(world.topic, keys::MODELS_ALLOWED, json!(["other-*"]))
                .await
        }
        Cause::ModesNarrowed => {
            world
                .set(world.topic, keys::APPROVAL_ALLOWED_MODES, json!(["bypass"]))
                .await
        }
        Cause::UsageCap => {
            world.cap(Some(world.topic), Some(0), true, &[], &[]).await;
        }
    }
}

async fn mend_it(world: &World, thread: Id, cause: Cause) {
    match cause {
        Cause::Sql(_, mended) => world.exec(mended).await,
        Cause::NoModel => {
            world
                .set(thread, keys::AGENT_MODEL, json!(common::MODEL))
                .await
        }
        Cause::UnlistedEffort => world.unset(thread, keys::AGENT_EFFORT).await,
        Cause::ModelsNarrowed => world.unset(world.topic, keys::MODELS_ALLOWED).await,
        Cause::ModesNarrowed => world.unset(world.topic, keys::APPROVAL_ALLOWED_MODES).await,
        Cause::UsageCap => world.exec("DELETE FROM usage_caps").await,
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn every_check_blocks_the_thread_with_its_reason_and_lets_it_through_when_its_cause_clears() {
    let world = World::new().await;
    let thread = world.thread("Checks").await;
    let ada = Caller::session(world.ada);
    for (n, case) in CASES.iter().enumerate() {
        break_it(&world, thread, case.cause).await;
        let words = format!("Case {n}: {}", case.name);
        let submitted = world
            .engine()
            .submit_input(&ada, thread, queued(&words))
            .await
            .unwrap();
        eventually(
            &format!("{} -> blocked as {}", case.name, case.reason),
            || async {
                let (state, reason, _) = world.state(thread).await;
                state == "blocked" && reason.as_deref() == Some(case.reason)
            },
        )
        .await;

        // The input is still there, nothing was started, and the reader is told why.
        meanwhile(Duration::from_millis(100)).await;
        assert_eq!(
            world.input_state(submitted.input.id).await.0,
            "pending",
            "{}",
            case.name
        );
        assert_eq!(
            world.turn_starts(thread).len(),
            n,
            "{}: nothing was started",
            case.name
        );
        assert_eq!(
            world
                .count("SELECT count(*) FROM turns WHERE thread_id = $1", thread)
                .await,
            n as i64
        );
        let sync = world.sync(thread).await;
        let blocked = sync
            .thread
            .blocked
            .as_ref()
            .unwrap_or_else(|| panic!("{}: the sync says why", case.name));
        assert_eq!(
            serde_json::to_value(blocked.reason).unwrap(),
            json!(case.reason),
            "{}",
            case.name
        );
        assert_eq!(sync.thread.pending_inputs, 1, "{}", case.name);

        // The cause clears, and the same input starts without anybody asking again.
        mend_it(&world, thread, case.cause).await;
        let start = world.turn_start(thread, n).await;
        assert_eq!(
            common::words_after_where(&start.text),
            words,
            "{}",
            case.name
        );
        world
            .answer(thread, &start, &format!("m{n}"), "Done.")
            .await;
        world
            .say(thread, turn_ended(start.turn_id, TurnOutcome::Completed))
            .await;
        world.wait_state(thread, "idle").await;
        assert_eq!(
            world.input_state(submitted.input.id).await.0,
            "delivered",
            "{}",
            case.name
        );
    }
    assert_eq!(
        world
            .count("SELECT count(*) FROM turns WHERE thread_id = $1", thread)
            .await,
        CASES.len() as i64
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn queued_work_intersects_provider_patterns_and_rechecks_the_current_address_without_losing_its_input()
 {
    let world = World::new().await;
    let thread = world.thread("Provider address policy").await;
    let address = {
        let mut conn = world.pool.acquire().await.unwrap();
        orch_authz::resources::addresses(&mut conn, &[world.provider])
            .await
            .unwrap()[0]
            .1
            .clone()
    };
    let prefix = address
        .rsplit_once('.')
        .map_or_else(|| "*".to_owned(), |(parent, _)| format!("{parent}.*"));
    world
        .set(world.root, keys::PROVIDERS_ALLOWED, json!([prefix]))
        .await;
    world
        .set(world.project, keys::PROVIDERS_ALLOWED, json!([address]))
        .await;
    world
        .set(world.topic, keys::PROVIDERS_ALLOWED, json!([]))
        .await;
    let submitted = world
        .engine()
        .submit_input(
            &Caller::session(world.ada),
            thread,
            queued("Keep this input"),
        )
        .await
        .unwrap();
    world.wait_blocked(thread, "provider_not_allowed").await;
    let detail: Option<String> = world
        .scalar(
            "SELECT blocked_detail FROM threads WHERE node_id = $1",
            thread,
        )
        .await;
    assert!(detail.unwrap().contains(&world.topic.to_string()));
    assert_eq!(world.input_state(submitted.input.id).await.0, "pending");
    world.unset(world.topic, keys::PROVIDERS_ALLOWED).await;
    let start = world.turn_start(thread, 0).await;
    world.say(thread, common::turn_began(start.turn_id)).await;
    world
        .say(thread, turn_ended(start.turn_id, TurnOutcome::Completed))
        .await;
    world.wait_turn(start.turn_id, "completed").await;
    sqlx::query("UPDATE providers SET name = 'renamed-provider' WHERE id = $1")
        .bind(world.provider)
        .execute(world.pool.pool())
        .await
        .unwrap();
    let next = world
        .engine()
        .submit_input(&Caller::session(world.ada), thread, queued("After rename"))
        .await
        .unwrap();
    world.wait_blocked(thread, "provider_not_allowed").await;
    let detail: Option<String> = world
        .scalar(
            "SELECT blocked_detail FROM threads WHERE node_id = $1",
            thread,
        )
        .await;
    assert!(detail.unwrap().contains(&world.project.to_string()));
    assert_eq!(world.input_state(next.input.id).await.0, "pending");
    world.unset(world.project, keys::PROVIDERS_ALLOWED).await;
    world.turn_start(thread, 1).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn when_several_checks_fail_the_first_in_the_order_is_the_reason_and_the_next_shows_when_it_clears()
 {
    let world = World::new().await;
    let thread = world.thread("Order").await;
    let ada = Caller::session(world.ada);
    let named = |name: &str| CASES.iter().position(|case| case.name == name).unwrap();
    // Pairs of causes, the one that is checked later broken first.
    let pairs = [
        (
            "the agent profile is switched off",
            "the environment failed",
        ),
        (
            "the provider is switched off",
            "the provider does not offer the model any more",
        ),
        (
            "the model does not list the effort",
            "the place does not allow the approval mode",
        ),
        (
            "the environment failed",
            "the machine does not take the role of an agent",
        ),
        (
            "the machine does not take the role of an agent",
            "a usage cap covers the turn",
        ),
    ];
    for (turn, (first, second)) in pairs.into_iter().enumerate() {
        let (first, second) = (&CASES[named(first)], &CASES[named(second)]);
        break_it(&world, thread, second.cause).await;
        break_it(&world, thread, first.cause).await;
        let words = format!("Pair {turn}: {} and {}", first.name, second.name);
        world
            .engine()
            .submit_input(&ada, thread, queued(&words))
            .await
            .unwrap();
        world.wait_blocked(thread, first.reason).await;
        meanwhile(Duration::from_millis(150)).await;
        assert_eq!(
            world.state(thread).await.1.as_deref(),
            Some(first.reason),
            "{words}"
        );

        // The cause that comes first clears: the next one is what blocks the thread now.
        mend_it(&world, thread, first.cause).await;
        world.wait_blocked(thread, second.reason).await;
        assert_eq!(
            world.turn_starts(thread).len(),
            turn,
            "nothing was started while one check still failed"
        );
        mend_it(&world, thread, second.cause).await;
        let start = world.turn_start(thread, turn).await;
        assert_eq!(common::words_after_where(&start.text), words);
        world
            .answer(thread, &start, &format!("m{turn}"), "Done.")
            .await;
        world
            .say(thread, turn_ended(start.turn_id, TurnOutcome::Completed))
            .await;
        world.wait_state(thread, "idle").await;
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_author_who_lost_the_right_to_interact_while_the_input_waited_has_it_cancelled() {
    let world = World::new().await;
    let thread = world.thread("Shared").await;
    let bob = world.person("bob").await;
    let role = world.give(bob, "interactor").await;
    world
        .exec("UPDATE environments SET status = 'failed', status_detail = 'the unit is gone'")
        .await;

    let bobs = world
        .engine()
        .submit_input(&Caller::session(bob), thread, queued("From bob"))
        .await
        .unwrap();
    let adas = world
        .engine()
        .submit_input(&Caller::session(world.ada), thread, queued("From ada"))
        .await
        .unwrap();
    world.wait_blocked(thread, "environment_unavailable").await;
    assert_eq!(world.input_state(bobs.input.id).await.0, "pending");

    // Bob's right goes while his input waits. The engine asks of everybody who waits whether they may
    // still be served each time it looks at the thread, whatever blocks it: his input is cancelled while
    // the thread is still blocked. The test waits for that before it lets the thread run. A look that began
    // before his right went could otherwise go on past the moment the thread is mended (it reads his right
    // and the environment a few statements apart) and serve him on the right he had when it began; the
    // looks at one thread are made one after the other, so once one that began after is over, no other
    // can do that.
    world.take(role).await;
    world.wait_input(bobs.input.id, "cancelled").await;
    world
        .exec("UPDATE environments SET status = 'running', status_detail = NULL")
        .await;
    let start = world.turn_start(thread, 0).await;
    assert_eq!(
        common::words_after_where(&start.text),
        "From ada",
        "the one who may still be served is served"
    );
    assert_eq!(
        world.input_state(bobs.input.id).await,
        (
            "cancelled".to_owned(),
            Some("author_lost_access".to_owned())
        )
    );
    world.answer(thread, &start, "m1", "Done.").await;
    world
        .say(thread, turn_ended(start.turn_id, TurnOutcome::Completed))
        .await;
    world.wait_state(thread, "idle").await;
    assert_eq!(world.input_state(adas.input.id).await.0, "delivered");
    meanwhile(Duration::from_millis(300)).await;
    assert_eq!(
        world.turn_starts(thread).len(),
        1,
        "what was cancelled is not served later"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_writer_who_loses_the_model_while_waiting_is_held_until_somebody_who_may_runs_it() {
    let world = World::new().await;
    let thread = world.thread("Provider").await;
    let bob = world.person("bob").await;
    world.give(bob, "interactor").await;
    world
        .exec("UPDATE environments SET status = 'failed', status_detail = 'the unit is gone'")
        .await;
    let bobs = world
        .engine()
        .submit_input(&Caller::session(bob), thread, queued("From bob"))
        .await
        .unwrap();
    world.wait_blocked(thread, "environment_unavailable").await;

    // Losing the model while the input waits keeps it in its place, waiting for somebody who may use the model;
    // it is never cancelled for this, and it holds nobody else up.
    world.access.deny(bob);
    eventually(
        "his input waits for somebody who may use the model",
        || async {
            world.input_state(bobs.input.id).await
                == ("pending".to_owned(), Some("model_use_required".to_owned()))
        },
    )
    .await;
    world
        .exec("UPDATE environments SET status = 'running', status_detail = NULL")
        .await;
    world.wait_state(thread, "idle").await;
    meanwhile(Duration::from_millis(300)).await;
    assert_eq!(
        world.turn_starts(thread).len(),
        0,
        "it runs on nobody's access by itself"
    );

    // Somebody who may use the model runs it on their own access; the turn is theirs to answer for.
    let ran = world
        .engine()
        .run_input(&Caller::session(world.ada), thread, bobs.input.id)
        .await
        .unwrap();
    assert_eq!(ran.run_as.as_ref().map(|person| person.id), Some(world.ada));
    assert_eq!(ran.author.as_ref().map(|person| person.id), Some(bob));
    let start = world.turn_start(thread, 0).await;
    assert_eq!(common::words_after_where(&start.text), "From bob");
    world.answer(thread, &start, "m1", "Done.").await;
    world
        .say(thread, turn_ended(start.turn_id, TurnOutcome::Completed))
        .await;
    world.wait_state(thread, "idle").await;
    assert_eq!(world.input_state(bobs.input.id).await.0, "delivered");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_writer_without_the_model_is_held_refused_or_served_by_the_owner_as_the_place_says() {
    let world = World::new().await;
    let thread = world.thread("Policy").await;
    let bob = world.person("bob").await;
    world.give(bob, "interactor").await;
    world.access.deny(bob);

    // As shipped, the message is taken and waits for somebody who may use the model.
    let held = world
        .engine()
        .submit_input(&Caller::session(bob), thread, queued("Held"))
        .await
        .unwrap();
    assert_eq!(
        held.input.state_detail.as_deref(),
        Some("model_use_required")
    );
    world.wait_state(thread, "idle").await;
    assert_eq!(world.turn_starts(thread).len(), 0);

    // Where the place refuses it, it is refused when it is sent, with what is missing.
    world
        .set(
            world.topic,
            keys::THREAD_INPUTS_WITHOUT_USE,
            json!("refused"),
        )
        .await;
    let refused = world
        .engine()
        .submit_input(&Caller::session(bob), thread, queued("Refused"))
        .await;
    assert!(matches!(refused, Err(InputError::Denied(_))));

    // Where the place allows it, what waits runs on the access of the thread's first owner.
    world
        .set(
            world.topic,
            keys::THREAD_INPUTS_WITHOUT_USE,
            json!("allowed"),
        )
        .await;
    let allowed = world
        .engine()
        .submit_input(&Caller::session(bob), thread, queued("Allowed"))
        .await
        .unwrap();
    let start = world.turn_start(thread, 0).await;
    assert_eq!(common::words_after_where(&start.text), "Held");
    assert_eq!(world.input_run_as(held.input.id).await, Some(world.ada));
    world.answer(thread, &start, "m1", "Done.").await;
    world
        .say(thread, turn_ended(start.turn_id, TurnOutcome::Completed))
        .await;
    let second = world.turn_start(thread, 1).await;
    assert_eq!(common::words_after_where(&second.text), "Allowed");
    assert_eq!(world.input_run_as(allowed.input.id).await, Some(world.ada));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_usage_cap_that_covers_the_turn_blocks_it_and_caps_that_do_not_cover_it_do_not() {
    let world = World::new().await;
    let thread = world.thread("Caps").await;
    let ada = Caller::session(world.ada);
    let elsewhere = world.topic("Elsewhere").await;
    // None of these covers a turn of this thread.
    world.cap(Some(elsewhere), Some(10), true, &[], &[]).await;
    world.cap(Some(world.topic), None, true, &[], &[]).await;
    world
        .cap(Some(world.topic), Some(10), false, &[], &[])
        .await;
    world
        .cap(Some(world.topic), Some(10), true, &["another-harness"], &[])
        .await;
    world
        .cap(Some(world.topic), Some(10), true, &[], &["other-model"])
        .await;
    world
        .engine()
        .submit_input(&ada, thread, queued("Not covered"))
        .await
        .unwrap();
    world.run_turn(thread, 0, "Done.").await;
    world.wait_state(thread, "idle").await;

    // This one does, by the node, the harness and the family of the model, and nothing of it is left.
    let covering = world
        .cap(Some(world.topic), Some(0), true, &["fake"], &["fake-*"])
        .await;
    let covered = world
        .engine()
        .submit_input(&ada, thread, queued("Covered"))
        .await
        .unwrap();
    world.wait_blocked(thread, "quota_exceeded").await;
    meanwhile(Duration::from_millis(300)).await;
    assert_eq!(
        world.turn_starts(thread).len(),
        1,
        "a turn is never let through a limit that covers it"
    );
    assert_eq!(world.input_state(covered.input.id).await.0, "pending");
    let detail = world
        .sync(thread)
        .await
        .thread
        .blocked
        .unwrap()
        .detail
        .unwrap_or_default();
    assert!(detail.contains("cap"), "{detail}");

    world
        .exec(&format!("DELETE FROM usage_caps WHERE id = '{covering}'"))
        .await;
    let start = world.turn_start(thread, 1).await;
    assert_eq!(common::words_after_where(&start.text), "Covered");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_input_let_past_the_providers_limit_by_someone_who_may_runs_while_the_limit_is_reached()
{
    let world = World::new().await;
    let thread = world.thread("Past the limit").await;
    let ada = Caller::session(world.ada);
    world
        .exec("UPDATE providers SET status = 'exhausted', status_detail = 'used up'")
        .await;

    let plain = world
        .engine()
        .submit_input(&ada, thread, queued("Plain"))
        .await
        .unwrap();
    world.wait_blocked(thread, "provider_exhausted").await;

    let mut request = queued("Regardless");
    request.bypass = Some(LimitBypassRequest {
        caps: None,
        provider_limit: Some(true),
    });
    // The input that waits is first, and it stays blocked; the one let past goes ahead of it only
    // when it is first in the queue, so the first is removed.
    world
        .engine()
        .cancel_input(&ada, thread, plain.input.id)
        .await
        .unwrap();
    let passed = world
        .engine()
        .submit_input(&ada, thread, request)
        .await
        .unwrap();
    assert_eq!(
        passed.input.limit_bypass.len(),
        1,
        "the record says who let it past"
    );
    let start = world.turn_start(thread, 0).await;
    assert_eq!(common::words_after_where(&start.text), "Regardless");

    // Somebody who does not hold the right cannot let an input past.
    world.access.refuse_bypass(world.ada);
    let mut again = queued("Again");
    again.bypass = Some(LimitBypassRequest {
        caps: None,
        provider_limit: Some(true),
    });
    let refused = world.engine().submit_input(&ada, thread, again).await;
    assert!(matches!(refused, Err(InputError::Denied(_))), "{refused:?}");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_input_nobody_sent_waits_while_the_first_owner_may_not_use_the_provider_and_holds_nobody_up()
 {
    let world = World::new().await;
    let thread = world.thread("Announcements").await;
    let bob = world.person("bob").await;
    world.give(bob, "interactor").await;
    world.access.deny(world.ada);

    let notice = world
        .engine()
        .queue_system_input(
            thread,
            "system",
            &[ContentPart::Text {
                text: "Notice from the system".to_owned(),
            }],
        )
        .await
        .unwrap();
    world.wait_blocked(thread, "binding_invalid").await;
    assert_eq!(
        world.input_state(notice.id).await.1.as_deref(),
        Some("owner_lacks_access")
    );

    // Somebody else's input is served meanwhile.
    world
        .engine()
        .submit_input(&Caller::session(bob), thread, queued("From bob"))
        .await
        .unwrap();
    let theirs = world.turn_start(thread, 0).await;
    assert_eq!(common::words_after_where(&theirs.text), "From bob");
    world.answer(thread, &theirs, "m1", "Done.").await;
    world
        .say(thread, turn_ended(theirs.turn_id, TurnOutcome::Completed))
        .await;
    world.wait_blocked(thread, "binding_invalid").await;
    assert_eq!(
        world.input_state(notice.id).await.0,
        "pending",
        "it waits, it is not lost"
    );

    // The thread's first owner may use the provider again: the held input is let go.
    world.access.allow(world.ada);
    let held = world.turn_start(thread, 1).await;
    assert!(
        held.text.contains("Notice from the system"),
        "{}",
        held.text
    );
    world
        .say(thread, turn_ended(held.turn_id, TurnOutcome::Completed))
        .await;
    world.wait_state(thread, "idle").await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn when_a_question_cannot_be_answered_nothing_is_let_through_and_nothing_is_lost() {
    let world = World::new().await;
    let thread = world.thread("Unanswerable").await;
    let ada = Caller::session(world.ada);

    // The input waits behind a turn that runs, then the resource side goes away.
    let (start, _) = world.running_turn(thread, "First").await;
    let waiting = world
        .engine()
        .submit_input(&ada, thread, queued("Second"))
        .await
        .unwrap();
    world.access.disconnect();
    world
        .say(thread, turn_ended(start.turn_id, TurnOutcome::Completed))
        .await;
    world.wait_turn(start.turn_id, "completed").await;

    meanwhile(Duration::from_millis(700)).await;
    assert_eq!(
        world.turn_starts(thread).len(),
        1,
        "a check that cannot be made lets nothing through"
    );
    let (state, detail) = world.input_state(waiting.input.id).await;
    assert_eq!(
        (state.as_str(), detail),
        ("pending", None),
        "and nothing is cancelled for it"
    );
    assert_eq!(world.state(thread).await.0, "idle");

    // A sender cannot be told yes either.
    let sender = world
        .engine()
        .submit_input(&ada, thread, queued("Third"))
        .await;
    assert!(matches!(sender, Err(InputError::Internal(_))), "{sender:?}");

    world.access.reconnect();
    let second = world.turn_start(thread, 1).await;
    assert_eq!(common::words_after_where(&second.text), "Second");
    world
        .say(thread, turn_ended(second.turn_id, TurnOutcome::Completed))
        .await;
    world.wait_state(thread, "idle").await;
    assert_eq!(world.ran(CommandKind::TurnStart), 2);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_cause_of_the_thread_stays_in_view_when_its_queue_empties_and_the_thread_is_idle_once_it_is_mended()
 {
    let world = World::new().await;
    let thread = world.thread("Kept in view").await;
    let ada = Caller::session(world.ada);
    // Every cause but a reached cap is the thread's own: what it is bound to and where it runs.
    for case in CASES.iter().filter(|case| case.reason != "quota_exceeded") {
        break_it(&world, thread, case.cause).await;
        let submitted = world
            .engine()
            .submit_input(&ada, thread, queued(case.name))
            .await
            .unwrap();
        world.wait_blocked(thread, case.reason).await;

        // The input is taken back. What the thread stands on is still wrong, and the thread still says
        // so after the engine has looked at it again (the tests look again every quarter of a second).
        world
            .engine()
            .cancel_input(&ada, thread, submitted.input.id)
            .await
            .unwrap();
        meanwhile(Duration::from_millis(800)).await;
        let (state, reason, _) = world.state(thread).await;
        assert_eq!(state, "blocked", "{}", case.name);
        assert_eq!(reason.as_deref(), Some(case.reason), "{}", case.name);

        // Mended, it is idle, and nothing is started for the input that is gone.
        mend_it(&world, thread, case.cause).await;
        world.wait_state(thread, "idle").await;
        meanwhile(Duration::from_millis(300)).await;
        assert!(world.turn_starts(thread).is_empty(), "{}", case.name);
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_input_held_for_a_tokens_missing_model_scope_does_not_acquire_its_users_full_authority()
{
    let world = World::new().await;
    let thread = world.thread("A scoped input").await;
    let caller = Caller::token(
        world.ada,
        orch_authz::TokenGrant::from_stored(
            &["thread.view".to_owned(), "thread.interact".to_owned()],
            Some(world.topic),
        )
        .unwrap(),
    );
    let submitted = world
        .engine()
        .submit_input(
            &caller,
            thread,
            queued("This input needs model-use approval"),
        )
        .await
        .unwrap();
    eventually("the input remains held or a turn is requested", || async {
        world.input_state(submitted.input.id).await.1 == Some("model_use_required".to_owned())
            || !world.turn_starts(thread).is_empty()
    })
    .await;
    assert_eq!(
        world.turn_starts(thread).len(),
        0,
        "no turn may run without a current actor who may use the model"
    );
    assert_eq!(
        world.input_state(submitted.input.id).await,
        ("pending".to_owned(), Some("model_use_required".to_owned())),
        "the author may use the model in a browser, but this token may not"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_token_that_carries_model_use_can_start_its_authors_turn() {
    let world = World::new().await;
    let thread = world.thread("A permitted scoped input").await;
    let caller = Caller::token(
        world.ada,
        orch_authz::TokenGrant::from_stored(
            &[
                "thread.view".to_owned(),
                "thread.interact".to_owned(),
                "provider.use".to_owned(),
                "environment.use".to_owned(),
            ],
            Some(world.topic),
        )
        .unwrap(),
    );
    world
        .engine()
        .submit_input(&caller, thread, queued("An explicitly permitted turn"))
        .await
        .unwrap();
    let start = world.turn_start(thread, 0).await;
    assert_eq!(
        common::words_after_where(&start.text),
        "An explicitly permitted turn"
    );
    world.answer(thread, &start, "m1", "Done.").await;
    world
        .say(thread, turn_ended(start.turn_id, TurnOutcome::Completed))
        .await;
    world.wait_state(thread, "idle").await;
}

async fn stored_input(world: &World, input: Id) -> orch_db::queries::threads::InputRow {
    orch_db::queries::threads::input(&mut world.pool.acquire().await.unwrap(), input)
        .await
        .unwrap()
        .unwrap()
}

async fn scoped_input_token(world: &World, provider_use: bool) -> (Id, Caller) {
    let id = orch_types::new_id();
    let mut scopes = vec!["thread.view".to_owned(), "thread.interact".to_owned()];
    if provider_use {
        scopes.push("provider.use".to_owned());
        scopes.push("environment.use".to_owned());
    }
    let digest = orch_types::Hash::of(id.as_bytes());
    let name = format!("input-{id}");
    orch_db::queries::identity::create_api_token(
        &mut world.pool.acquire().await.unwrap(),
        &orch_db::queries::identity::NewApiToken {
            id,
            user_id: world.ada,
            name: &name,
            token_hash: digest.as_bytes(),
            scopes: &scopes,
            node_id: Some(world.topic),
            at: world.clock.now(),
            expires_at: None,
        },
    )
    .await
    .unwrap();
    (
        id,
        Caller::token(
            world.ada,
            orch_authz::TokenGrant::from_stored(&scopes, Some(world.topic))
                .unwrap()
                .for_token(id),
        ),
    )
}

async fn token_without_environment_use(world: &World) -> (Id, Caller) {
    let (id, _) = scoped_input_token(world, true).await;
    let scopes = vec![
        "thread.view".to_owned(),
        "thread.interact".to_owned(),
        "provider.use".to_owned(),
    ];
    sqlx::query("UPDATE api_tokens SET scopes=$2 WHERE id=$1")
        .bind(id)
        .bind(&scopes)
        .execute(world.pool.pool())
        .await
        .unwrap();
    let caller = Caller::token(
        world.ada,
        orch_authz::TokenGrant::from_stored(&scopes, Some(world.topic))
            .unwrap()
            .for_token(id),
    );
    (id, caller)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_token_without_environment_use_cannot_run_a_held_input_on_its_owners_access() {
    let world = World::new().await;
    let thread = world.thread("Token environment scope").await;
    let (_, narrow) = token_without_environment_use(&world).await;
    let submitted = world
        .engine()
        .submit_input(&narrow, thread, queued("Work needing the environment"))
        .await
        .unwrap();
    assert_eq!(
        world.input_state(submitted.input.id).await,
        ("pending".into(), Some("model_use_required".into()))
    );
    assert!(matches!(
        world
            .engine()
            .run_input(&narrow, thread, submitted.input.id)
            .await,
        Err(InputError::Denied(_))
    ));
    assert_eq!(world.input_run_as(submitted.input.id).await, None);
    assert_eq!(world.turn_starts(thread).len(), 0);
    let (_, entitled) = scoped_input_token(&world, true).await;
    let approval: Id = world
        .scalar(
            "SELECT approval_id FROM thread_inputs WHERE id=$1",
            submitted.input.id,
        )
        .await;
    let mut conn = world.pool.acquire().await.unwrap();
    let row = orch_db::queries::approvals::approval(&mut conn, approval)
        .await
        .unwrap()
        .unwrap();
    for (caller, expected) in [(&narrow, false), (&entitled, true)] {
        let principal = orch_authz::Principal::load(
            &mut conn,
            caller.clone(),
            orch_types::Time::new(world.clock.now()),
        )
        .await
        .unwrap();
        assert_eq!(
            world
                .engine()
                .may_decide_source_request(&mut conn, &principal, &row)
                .await
                .unwrap(),
            expected
        );
    }
    drop(conn);
    let approved = world
        .engine()
        .run_input(&entitled, thread, submitted.input.id)
        .await
        .unwrap();
    assert_eq!(approved.author.as_ref().unwrap().id, world.ada);
    assert_eq!(approved.run_as.as_ref().unwrap().id, world.ada);
    world
        .run_turn(thread, 0, "Only the entitled token runs it.")
        .await;
    world.wait_state(thread, "idle").await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn removing_only_a_tokens_environment_scope_holds_its_waiting_input() {
    let world = World::new().await;
    let thread = world.thread("Environment scope withdrawn").await;
    let (token, caller) = scoped_input_token(&world, true).await;
    world
        .exec("UPDATE environments SET status='failed',status_detail='waiting for the environment'")
        .await;
    let submitted = world
        .engine()
        .submit_input(&caller, thread, queued("Pending original"))
        .await
        .unwrap();
    sqlx::query("UPDATE api_tokens SET scopes=ARRAY['thread.view','thread.interact','provider.use'] WHERE id=$1")
        .bind(token).execute(world.pool.pool()).await.unwrap();
    world
        .exec("UPDATE environments SET status='running',status_detail=NULL")
        .await;
    world.engine().preflight(thread).await.unwrap();
    assert_eq!(
        world.input_state(submitted.input.id).await,
        ("pending".into(), Some("model_use_required".into()))
    );
    assert_eq!(world.turn_starts(thread).len(), 0);
    assert!(matches!(
        world
            .engine()
            .run_input(&caller, thread, submitted.input.id)
            .await,
        Err(InputError::Denied(_))
    ));
    assert_eq!(world.input_run_as(submitted.input.id).await, None);
}

async fn derive_waiting_input(world: &World, thread: Id, origin: Id) -> Id {
    let origin = stored_input(world, origin).await;
    let provenance = orch_engine::inputs::derived_input_provenance(&origin, None).unwrap();
    let content = serde_json::to_value(queued("Continue").content).unwrap();
    let id = orch_types::new_id();
    let mut tx = world.pool.begin().await.unwrap();
    orch_db::queries::threads::lock(&mut tx, thread)
        .await
        .unwrap()
        .unwrap();
    let seq = orch_db::sequences::next(&mut tx, thread)
        .await
        .unwrap()
        .unwrap();
    orch_db::queries::threads::insert_input(
        &mut tx,
        &orch_db::queries::threads::NewInput {
            id,
            thread_id: thread,
            author_id: provenance.author_id,
            submission_authority: provenance.submission_authority.as_ref(),
            run_as: provenance.run_as,
            run_as_authority: provenance.run_as_authority.as_ref(),
            source: "system",
            content: &content,
            delivery: "queue",
            state: "pending",
            turn_id: None,
            client_id: None,
            limit_bypass: None,
            place: orch_db::queries::threads::Place::End,
            at: world.clock.now(),
            plan: false,
        },
        seq,
    )
    .await
    .unwrap()
    .unwrap();
    tx.commit().await.unwrap();
    world.engine().preflight(thread).await.unwrap();
    id
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn run_it_keeps_the_approvers_token_boundary_after_restart_and_does_not_rewrite_the_author() {
    let world = World::new().await;
    let thread = world.thread("Recorded approval").await;
    let (_, narrow) = scoped_input_token(&world, false).await;
    let (runner_id, runner) = scoped_input_token(&world, true).await;
    world
        .exec("UPDATE environments SET status='failed',status_detail='waiting for the environment'")
        .await;
    let sent = world
        .engine()
        .submit_input(&narrow, thread, queued("Approved words"))
        .await
        .unwrap();
    let original = stored_input(&world, sent.input.id)
        .await
        .submission_authority;
    assert!(matches!(
        world
            .engine()
            .run_input(&narrow, thread, sent.input.id)
            .await,
        Err(InputError::Denied(_))
    ));
    assert_eq!(world.input_run_as(sent.input.id).await, None);
    world
        .engine()
        .run_input(&runner, thread, sent.input.id)
        .await
        .unwrap();
    let approved = stored_input(&world, sent.input.id).await;
    assert_eq!(approved.submission_authority, original);
    assert_eq!(approved.author_id, Some(world.ada));
    assert_eq!(approved.run_as, Some(world.ada));
    assert_eq!(
        approved.run_as_authority.as_ref().unwrap()["credentials"][0]["token_id"],
        runner_id.to_string()
    );
    sqlx::query("UPDATE api_tokens SET scopes=ARRAY['thread.view','thread.interact'] WHERE id=$1")
        .bind(runner_id)
        .execute(world.pool.pool())
        .await
        .unwrap();
    world.restart_api().await;
    world
        .exec("UPDATE environments SET status='running',status_detail=NULL")
        .await;
    world.engine().preflight(thread).await.unwrap();
    eventually(
        "the revoked model scope holds the approved input",
        || async {
            world.input_state(sent.input.id).await
                == ("pending".into(), Some("model_use_required".into()))
        },
    )
    .await;
    assert_eq!(world.turn_starts(thread).len(), 0);
    assert_eq!(world.input_run_as(sent.input.id).await, None);
    let ran = world
        .engine()
        .run_input(&Caller::session(world.ada), thread, sent.input.id)
        .await
        .unwrap();
    assert_eq!(ran.author.as_ref().map(|person| person.id), Some(world.ada));
    assert_eq!(ran.run_as.as_ref().map(|person| person.id), Some(world.ada));
    world.run_turn(thread, 0, "Done.").await;
    world.wait_state(thread, "idle").await;
    assert_eq!(
        stored_input(&world, sent.input.id)
            .await
            .submission_authority,
        original
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn automatic_derived_work_keeps_its_origins_token_and_approver_instead_of_borrowing_a_browser()
 {
    let world = World::new().await;
    let thread = world.thread("Derived authority").await;
    let (_, narrow) = scoped_input_token(&world, false).await;
    let (runner_id, runner) = scoped_input_token(&world, true).await;
    let sent = world
        .engine()
        .submit_input(&narrow, thread, queued("Original work"))
        .await
        .unwrap();
    world
        .engine()
        .run_input(&runner, thread, sent.input.id)
        .await
        .unwrap();
    world.run_turn(thread, 0, "The first turn ended.").await;
    world.wait_state(thread, "idle").await;
    let original = stored_input(&world, sent.input.id).await;
    sqlx::query("UPDATE api_tokens SET scopes=ARRAY['thread.view','thread.interact'] WHERE id=$1")
        .bind(runner_id)
        .execute(world.pool.pool())
        .await
        .unwrap();
    let continued = derive_waiting_input(&world, thread, sent.input.id).await;
    let held = stored_input(&world, continued).await;
    assert_eq!(held.author_id, original.author_id);
    assert_eq!(held.submission_authority, original.submission_authority);
    assert_eq!(
        world.input_state(continued).await,
        ("pending".into(), Some("model_use_required".into()))
    );
    assert_eq!(
        world.turn_starts(thread).len(),
        1,
        "an automatic producer cannot replace the token with a browser"
    );
    world
        .engine()
        .run_input(&Caller::session(world.ada), thread, continued)
        .await
        .unwrap();
    let start = world.turn_start(thread, 1).await;
    assert_eq!(common::words_after_where(&start.text), "Continue");
    world.answer(thread, &start, "m2", "Done.").await;
    world
        .say(thread, turn_ended(start.turn_id, TurnOutcome::Completed))
        .await;
    world.wait_state(thread, "idle").await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_same_persons_browser_input_does_not_clear_their_narrow_tokens_hold() {
    let world = World::new().await;
    let thread = world.thread("Two credential contexts").await;
    let (_, narrow) = scoped_input_token(&world, false).await;
    let held = world
        .engine()
        .submit_input(&narrow, thread, queued("Token words"))
        .await
        .unwrap();
    world
        .engine()
        .submit_input(&Caller::session(world.ada), thread, queued("Browser words"))
        .await
        .unwrap();
    let start = world.turn_start(thread, 0).await;
    assert_eq!(common::words_after_where(&start.text), "Browser words");
    assert_eq!(
        world.input_state(held.input.id).await,
        ("pending".into(), Some("model_use_required".into()))
    );
    world.answer(thread, &start, "m1", "Done.").await;
    world
        .say(thread, turn_ended(start.turn_id, TurnOutcome::Completed))
        .await;
    world.wait_state(thread, "idle").await;
    assert_eq!(world.turn_starts(thread).len(), 1);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn older_authored_work_with_unknown_provenance_waits_for_explicit_run_it() {
    let world = World::new().await;
    let thread = world.thread("Older input").await;
    let id = orch_types::new_id();
    sqlx::query("INSERT INTO thread_inputs(id,thread_id,author_id,source,content,delivery,state,position,seq_updated)
        VALUES($1,$2,$3,'user',$4,'queue','pending',1,1)")
        .bind(id).bind(thread).bind(world.ada).bind(serde_json::to_value(queued("Older words").content).unwrap())
        .execute(world.pool.pool()).await.unwrap();
    world.engine().preflight(thread).await.unwrap();
    assert_eq!(
        world.input_state(id).await,
        ("pending".into(), Some("model_use_required".into()))
    );
    assert_eq!(world.turn_starts(thread).len(), 0);
    world
        .engine()
        .run_input(&Caller::session(world.ada), thread, id)
        .await
        .unwrap();
    world.run_turn(thread, 0, "Done.").await;
    world.wait_state(thread, "idle").await;
    assert!(
        stored_input(&world, id)
            .await
            .submission_authority
            .is_none()
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_narrow_token_cannot_replace_privileged_pending_words_and_keep_their_browser_authority() {
    let world = World::new().await;
    let thread = world.thread("Restricted edit").await;
    world
        .exec("UPDATE environments SET status='failed',status_detail='waiting for the environment'")
        .await;
    let sent = world
        .engine()
        .submit_input(
            &Caller::session(world.ada),
            thread,
            queued("Browser original"),
        )
        .await
        .unwrap();
    let (_, narrow) = scoped_input_token(&world, false).await;
    world
        .engine()
        .edit_input(
            &narrow,
            thread,
            sent.input.id,
            Some(queued("Token replacement").content),
            None,
        )
        .await
        .unwrap();
    world
        .exec("UPDATE environments SET status='running',status_detail=NULL")
        .await;
    world.engine().preflight(thread).await.unwrap();
    assert_eq!(
        world.input_state(sent.input.id).await,
        ("pending".into(), Some("model_use_required".into()))
    );
    assert_eq!(world.turn_starts(thread).len(), 0);
    world
        .engine()
        .run_input(&Caller::session(world.ada), thread, sent.input.id)
        .await
        .unwrap();
    let start = world.turn_start(thread, 0).await;
    assert_eq!(common::words_after_where(&start.text), "Token replacement");
    world.answer(thread, &start, "m1", "Done.").await;
    world
        .say(thread, turn_ended(start.turn_id, TurnOutcome::Completed))
        .await;
    world.wait_state(thread, "idle").await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_explicit_allowed_policy_can_serve_a_narrow_tokens_input_on_its_owners_access() {
    let world = World::new().await;
    let thread = world.thread("Explicit owner policy").await;
    let (_, narrow) = scoped_input_token(&world, false).await;
    world
        .set(
            world.topic,
            keys::THREAD_INPUTS_WITHOUT_USE,
            json!("allowed"),
        )
        .await;
    let sent = world
        .engine()
        .submit_input(&narrow, thread, queued("Allowed by this place"))
        .await
        .unwrap();
    world.run_turn(thread, 0, "Done.").await;
    world.wait_state(thread, "idle").await;
    assert_eq!(world.input_run_as(sent.input.id).await, Some(world.ada));
    assert!(
        stored_input(&world, sent.input.id)
            .await
            .submission_authority
            .is_some()
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn broadening_a_stored_token_does_not_release_words_accepted_with_its_narrower_scopes() {
    let world = World::new().await;
    let thread = world.thread("Immutable submission scopes").await;
    let (token, narrow) = scoped_input_token(&world, false).await;
    let sent = world
        .engine()
        .submit_input(&narrow, thread, queued("Narrow original"))
        .await
        .unwrap();
    let original = stored_input(&world, sent.input.id)
        .await
        .submission_authority;
    sqlx::query("UPDATE api_tokens SET scopes=ARRAY['thread.view','thread.interact','provider.use','environment.use'] WHERE id=$1")
        .bind(token).execute(world.pool.pool()).await.unwrap();
    world.restart_api().await;
    world.engine().preflight(thread).await.unwrap();
    let continued = derive_waiting_input(&world, thread, sent.input.id).await;
    for input in [sent.input.id, continued] {
        assert_eq!(
            world.input_state(input).await,
            ("pending".into(), Some("model_use_required".into()))
        );
        assert_eq!(
            stored_input(&world, input).await.submission_authority,
            original
        );
    }
    assert_eq!(
        world.turn_starts(thread).len(),
        0,
        "a later token edit cannot approve previously accepted or automatically derived words"
    );
    let entitled = Caller::token(
        world.ada,
        orch_authz::TokenGrant::from_stored(
            &[
                "thread.view".to_owned(),
                "thread.interact".to_owned(),
                "provider.use".to_owned(),
                "environment.use".to_owned(),
            ],
            Some(world.topic),
        )
        .unwrap()
        .for_token(token),
    );
    world
        .engine()
        .run_input(&entitled, thread, continued)
        .await
        .unwrap();
    world.run_turn(thread, 0, "Explicitly approved.").await;
    world.wait_state(thread, "idle").await;
    assert_eq!(
        world.input_state(sent.input.id).await,
        ("pending".into(), Some("model_use_required".into()))
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn narrowing_a_stored_token_holds_its_pending_input_and_refuses_a_stale_run_it_caller() {
    let world = World::new().await;
    let thread = world.thread("Current token scopes").await;
    let (token, original_caller) = scoped_input_token(&world, true).await;
    world
        .exec("UPDATE environments SET status='failed',status_detail='waiting for the environment'")
        .await;
    let sent = world
        .engine()
        .submit_input(&original_caller, thread, queued("Pending original"))
        .await
        .unwrap();
    sqlx::query("UPDATE api_tokens SET scopes=ARRAY['thread.view','thread.interact'] WHERE id=$1")
        .bind(token)
        .execute(world.pool.pool())
        .await
        .unwrap();
    world
        .exec("UPDATE environments SET status='running',status_detail=NULL")
        .await;
    world.engine().preflight(thread).await.unwrap();
    assert_eq!(
        world.input_state(sent.input.id).await,
        ("pending".into(), Some("model_use_required".into()))
    );
    assert_eq!(world.turn_starts(thread).len(), 0);
    assert!(matches!(
        world
            .engine()
            .run_input(&original_caller, thread, sent.input.id)
            .await,
        Err(InputError::Denied(_))
    ));
    assert_eq!(world.input_run_as(sent.input.id).await, None);
    world
        .engine()
        .run_input(&Caller::session(world.ada), thread, sent.input.id)
        .await
        .unwrap();
    world
        .run_turn(thread, 0, "A current entitled approver.")
        .await;
    world.wait_state(thread, "idle").await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_model_approver_does_not_lend_an_authors_token_new_driving_rights_when_steering() {
    let world = World::new().await;
    let thread = world.thread("Approved input still needs its author").await;
    let browser = Caller::session(world.ada);
    let (active, _) = world.running_turn(thread, "Current work").await;
    let (token, narrow) = scoped_input_token(&world, false).await;
    let waiting = world
        .engine()
        .submit_input(&narrow, thread, queued("Approved guidance"))
        .await
        .unwrap();
    world
        .engine()
        .run_input(&browser, thread, waiting.input.id)
        .await
        .unwrap();
    let unapproved = world
        .engine()
        .submit_input(
            &narrow,
            thread,
            queued("A later approval still checks its author"),
        )
        .await
        .unwrap();
    sqlx::query("UPDATE api_tokens SET scopes=ARRAY['thread.view'] WHERE id=$1")
        .bind(token)
        .execute(world.pool.pool())
        .await
        .unwrap();
    assert!(matches!(
        world.engine().run_input(&browser, thread, unapproved.input.id).await,
        Err(InputError::Denied(ref refusal))
            if refusal.required == orch_access_contract::permissions::Permission::ThreadInteract
    ));
    assert_eq!(world.input_run_as(unapproved.input.id).await, None);
    let mut tx = world.pool.begin().await.unwrap();
    let principal =
        orch_authz::Principal::load(&mut tx, browser, orch_types::Time::new(world.clock.now()))
            .await
            .unwrap();
    let result = world
        .engine()
        .update_input_guarded(
            tx,
            principal,
            thread,
            waiting.input.id,
            orch_api_contract::threads::ThreadInputUpdateBody {
                delivery: Some(orch_api_contract::threads::Delivery::Steer),
                ..Default::default()
            },
        )
        .await;
    assert!(matches!(result, Err(InputError::Denied(ref refusal))
        if refusal.required == orch_access_contract::permissions::Permission::ThreadInteract));
    assert_eq!(world.input_state(waiting.input.id).await.0, "pending");
    world
        .say(thread, common::message_done("m", "Working"))
        .await;
    world
        .say(thread, turn_ended(active.turn_id, TurnOutcome::Completed))
        .await;
    world.wait_state(thread, "idle").await;
    eventually(
        "the ended turn's retained follow-up has committed",
        || async {
            !orch_db::queries::threads::turn_end_work_pending(
                &mut world.pool.acquire().await.unwrap(),
                thread,
            )
            .await
            .unwrap()
        },
    )
    .await;
    world.engine().preflight(thread).await.unwrap();
    assert_eq!(
        world.input_state(waiting.input.id).await,
        ("cancelled".into(), Some("author_lost_access".into()))
    );
    assert_eq!(
        world.input_state(unapproved.input.id).await,
        ("cancelled".into(), Some("author_lost_access".into()))
    );
    assert_eq!(world.turn_starts(thread).len(), 1);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn removing_an_author_reference_never_turns_scoped_words_into_an_authorless_notice() {
    for retain_provenance in [true, false] {
        let world = World::new().await;
        let thread = world.thread("Missing author").await;
        let (_, narrow) = scoped_input_token(&world, false).await;
        let waiting = world
            .engine()
            .submit_input(&narrow, thread, queued("Authored words"))
            .await
            .unwrap();
        // This is the input-side effect of the author's user row being removed.
        sqlx::query("UPDATE thread_inputs SET author_id=NULL,submission_authority=CASE WHEN $2 THEN submission_authority ELSE NULL END WHERE id=$1")
            .bind(waiting.input.id).bind(retain_provenance)
            .execute(world.pool.pool()).await.unwrap();
        let origin = stored_input(&world, waiting.input.id).await;
        assert!(
            orch_engine::inputs::derived_input_provenance(&origin, None).is_err(),
            "a removed author cannot be replaced with an anonymous producer"
        );
        world.engine().preflight(thread).await.unwrap();
        assert_eq!(
            world.input_state(waiting.input.id).await,
            ("cancelled".into(), Some("author_lost_access".into()))
        );
        assert_eq!(world.turn_starts(thread).len(), 0);
    }
}
