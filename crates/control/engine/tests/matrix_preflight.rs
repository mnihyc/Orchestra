//! The combination matrix of failures before a turn: a node that is offline or does not take the agent's role,
//! a provider switched off, not set up or used up, a model unset or not allowed, an approval mode a place or the agent
//! does not have, an environment that failed, no arrangement, a usage cap; each combination of them on one thread.
//!
//! Expected: the checks run in their order and the first that fails blocks the thread with its reason;
//! when that cause clears, the next one in the order is the reason, and when every cause has cleared the same input
//! starts a turn without anybody asking again. So each case expects the whole sequence of reasons, cause by cause.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]

mod common;
mod matrix_kit;

use common::{World, queued, turn_ended};
use matrix_kit::{Case, Matrix, dim, failure};
use orch_authz::Caller;
use orch_link_contract::events::TurnOutcome;
use orch_settings_contract::registry::keys;
use orch_types::{Id, Json};
use serde_json::json;
use std::time::{Duration, Instant};

const SOURCE: &str = "Ordered preflight: report the first failure, retain the input and start once every cause clears";

/// One cause: its check's priority, the reason it gives, and how the matrix names it.
#[derive(Debug, Clone, Copy)]
struct Cause {
    dimension: &'static str,
    value: &'static str,
    check: u8,
    reason: &'static str,
}

/// Every failing dimension value and the preflight check it fails.
const CAUSES: &[Cause] = &[
    Cause {
        dimension: "binding",
        value: "unbound",
        check: 2,
        reason: "unbound",
    },
    Cause {
        dimension: "agent",
        value: "disabled",
        check: 3,
        reason: "agent_disabled",
    },
    Cause {
        dimension: "provider",
        value: "disabled",
        check: 4,
        reason: "provider_unavailable",
    },
    Cause {
        dimension: "provider",
        value: "needs_setup",
        check: 5,
        reason: "provider_unavailable",
    },
    Cause {
        dimension: "provider",
        value: "exhausted",
        check: 5,
        reason: "provider_exhausted",
    },
    Cause {
        dimension: "model",
        value: "not_set",
        check: 6,
        reason: "model_not_set",
    },
    Cause {
        dimension: "model",
        value: "disabled_on_provider",
        check: 7,
        reason: "model_not_allowed",
    },
    Cause {
        dimension: "model",
        value: "unlisted_effort",
        check: 7,
        reason: "model_not_allowed",
    },
    Cause {
        dimension: "model",
        value: "narrowed_by_place",
        check: 7,
        reason: "model_not_allowed",
    },
    Cause {
        dimension: "approval",
        value: "narrowed_by_place",
        check: 8,
        reason: "binding_invalid",
    },
    Cause {
        dimension: "approval",
        value: "agent_lacks_mode",
        check: 8,
        reason: "binding_invalid",
    },
    Cause {
        dimension: "environment",
        value: "failed",
        check: 9,
        reason: "environment_unavailable",
    },
    Cause {
        dimension: "machine",
        value: "offline",
        check: 9,
        reason: "environment_unavailable",
    },
    Cause {
        dimension: "arrangement",
        value: "none_offered",
        check: 10,
        reason: "binding_invalid",
    },
    Cause {
        dimension: "machine",
        value: "no_agent_role",
        check: 11,
        reason: "agent_offline",
    },
    Cause {
        dimension: "cap",
        value: "covering",
        check: 12,
        reason: "quota_exceeded",
    },
];

const ALL_MODES: &str = r#"["ask","on_request","bypass"]"#;
const ALL_ARRANGEMENTS: &str = r#"[{"type":"together"},{"type":"separated","executor":"orch-exec","executor_version":"*","tool_sets":["plain"]}]"#;

/// The adapter declares `member` as `value` from now on: at every start of its harness too, which the profile then
/// keeps, so that a start the case does not see cannot bring back what it declared before.
async fn declare(world: &World, member: &str, value: &str) {
    let mut descriptor: Json = world
        .scalar("SELECT descriptor FROM agents WHERE id = $1", world.agent)
        .await;
    descriptor[member] = serde_json::from_str(value).unwrap();
    world.declares(descriptor).await;
}

async fn break_it(world: &World, thread: Id, cause: &Cause) {
    match (cause.dimension, cause.value) {
        ("binding", _) => {
            world
                .exec("UPDATE threads SET agent_id = NULL, provider_id = NULL")
                .await
        }
        ("agent", _) => world.exec("UPDATE agents SET disabled = true").await,
        ("provider", "disabled") => world.exec("UPDATE providers SET status = 'disabled'").await,
        ("provider", "needs_setup") => {
            world
                .exec("UPDATE providers SET status = 'needs_setup'")
                .await
        }
        ("provider", _) => world
            .exec("UPDATE providers SET status = 'exhausted', status_detail = 'this month is used'")
            .await,
        ("model", "not_set") => world.unset(thread, keys::AGENT_MODEL).await,
        ("model", "disabled_on_provider") => {
            world
                .exec("UPDATE provider_models SET enabled = false")
                .await
        }
        ("model", "unlisted_effort") => world.set(thread, keys::AGENT_EFFORT, json!("ultra")).await,
        ("model", _) => {
            world
                .set(world.topic, keys::MODELS_ALLOWED, json!(["other-*"]))
                .await
        }
        ("approval", "narrowed_by_place") => {
            world
                .set(world.topic, keys::APPROVAL_ALLOWED_MODES, json!(["bypass"]))
                .await
        }
        ("approval", _) => declare(world, "approval_modes", r#"["bypass"]"#).await,
        ("environment", _) => {
            world
                .exec(
                    "UPDATE environments SET status = 'failed', status_detail = 'the unit is gone'",
                )
                .await
        }
        ("machine", "offline") => world.exec("UPDATE machines SET online = false").await,
        ("machine", _) => {
            world
                .exec("UPDATE machines SET roles = ARRAY['environment','provider']")
                .await
        }
        ("arrangement", _) => declare(world, "arrangements", "[]").await,
        _ => {
            world.cap(Some(world.topic), Some(0), true, &[], &[]).await;
        }
    }
}

async fn mend_it(world: &World, thread: Id, cause: &Cause) {
    match (cause.dimension, cause.value) {
        ("binding", _) => world.exec("UPDATE threads SET agent_id = (SELECT id FROM agents), provider_id = (SELECT id FROM providers)").await,
        ("agent", _) => world.exec("UPDATE agents SET disabled = false").await,
        ("provider", _) => world.exec("UPDATE providers SET status = 'ready', status_detail = NULL").await,
        ("model", "not_set") => world.set(thread, keys::AGENT_MODEL, json!(common::MODEL)).await,
        ("model", "disabled_on_provider") => world.exec("UPDATE provider_models SET enabled = true").await,
        ("model", "unlisted_effort") => world.unset(thread, keys::AGENT_EFFORT).await,
        ("model", _) => world.unset(world.topic, keys::MODELS_ALLOWED).await,
        ("approval", "narrowed_by_place") => world.unset(world.topic, keys::APPROVAL_ALLOWED_MODES).await,
        ("approval", _) => declare(world, "approval_modes", ALL_MODES).await,
        ("environment", _) => world.exec("UPDATE environments SET status = 'running', status_detail = NULL").await,
        ("machine", "offline") => world.exec("UPDATE machines SET online = true").await,
        ("machine", _) => world.exec("UPDATE machines SET roles = ARRAY['agent','environment','provider']").await,
        ("arrangement", _) => declare(world, "arrangements", ALL_ARRANGEMENTS).await,
        _ => world.exec("DELETE FROM usage_caps").await,
    }
}

/// The thread's state and reason once it has been one of the wanted reasons for a moment, or what it was when the
/// patience ran out.
async fn settle_on(world: &World, thread: Id, reason: &str) -> Result<(), String> {
    let started = Instant::now();
    loop {
        let (state, why, detail) = world.state(thread).await;
        if state == "blocked" && why.as_deref() == Some(reason) {
            return Ok(());
        }
        // Patient: on a loaded machine the engine's recheck of a blocked thread can lag by seconds; a reason that is
        // still the old one after this long is stale.
        if started.elapsed() > Duration::from_secs(30) {
            return Err(format!("{state} {why:?} {detail:?}"));
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
}

async fn run(case: &Case) -> Result<(), matrix_kit::Failure> {
    // No read of the subscription starts its harness while the case breaks what the adapter declares: a start
    // already on its way when a break is declared would store the declaration it was answered with.
    let world = World::observed().await;
    let thread = world.thread("Matrix").await;
    let mut causes: Vec<&Cause> = CAUSES
        .iter()
        .filter(|cause| case.is(cause.dimension, cause.value))
        .collect();
    causes.sort_by_key(|cause| cause.check);
    // Broken from the last check to the first, so that no earlier cause clears a later one on its way.
    for cause in causes.iter().rev() {
        break_it(&world, thread, cause).await;
    }
    let words = format!("Matrix case {}", case.id());
    let submitted = world
        .engine()
        .submit_input(&Caller::session(world.ada), thread, queued(&words))
        .await
        .map_err(|error| failure("the input is accepted", SOURCE, format!("{error:?}")))?;
    let expected: Vec<&str> = causes.iter().map(|cause| cause.reason).collect();
    let mut seen: Vec<String> = Vec::new();
    for (n, cause) in causes.iter().enumerate() {
        if let Err(got) = settle_on(&world, thread, cause.reason).await {
            return Err(failure(
                format!("reasons in order {expected:?}"),
                SOURCE,
                format!(
                    "after {seen:?}, at {} ({}={}): {got}",
                    cause.reason, cause.dimension, cause.value
                ),
            ));
        }
        seen.push(cause.reason.to_owned());
        if !world.turn_starts(thread).is_empty() {
            return Err(failure(
                "nothing starts while a check fails",
                SOURCE,
                format!("a turn started at step {n}"),
            ));
        }
        mend_it(&world, thread, cause).await;
    }
    // Every cause cleared: the same input starts, and ends.
    let start = world.turn_start(thread, 0).await;
    if common::words_after_where(&start.text) != words {
        return Err(failure(words.clone(), SOURCE, start.text.clone()));
    }
    world.answer(thread, &start, "m0", "Done.").await;
    world
        .say(thread, turn_ended(start.turn_id, TurnOutcome::Completed))
        .await;
    world.wait_state(thread, "idle").await;
    let (state, _) = world.input_state(submitted.input.id).await;
    if state != "delivered" {
        return Err(failure("the input delivered", SOURCE, state));
    }
    Ok(())
}

fn the_matrix() -> Matrix {
    Matrix::new(
        "preflight",
        &[
            dim("binding", &["bound", "unbound"]),
            dim("agent", &["enabled", "disabled"]),
            dim(
                "provider",
                &["ready", "disabled", "needs_setup", "exhausted"],
            ),
            dim(
                "model",
                &[
                    "set",
                    "not_set",
                    "disabled_on_provider",
                    "unlisted_effort",
                    "narrowed_by_place",
                ],
            ),
            dim(
                "approval",
                &["allowed", "narrowed_by_place", "agent_lacks_mode"],
            ),
            dim("environment", &["running", "failed"]),
            dim("machine", &["online", "offline", "no_agent_role"]),
            dim("arrangement", &["offered", "none_offered"]),
            dim("cap", &["none", "covering"]),
        ],
        // Every combination can meet: each cause has a column of its own.
        |_| true,
    )
}

#[test]
fn every_combination_of_failures_blocks_with_the_first_in_the_order_and_clears_cause_by_cause() {
    let matrix = the_matrix();
    // The provider, the model and the node interact in the order of checks 4 to 11: their whole product.
    let cases = matrix.cases(5, &[&["provider", "model", "machine"]], 20_261_015, 24);
    // Each case on a thread of its own with a runtime of its own (see `matrix_kit::run_cases`).
    matrix_kit::run_cases(&cases, 3, SOURCE, |case| {
        tokio::runtime::Builder::new_multi_thread()
            .worker_threads(3)
            .thread_stack_size(matrix_kit::CASE_STACK)
            .enable_all()
            .build()
            .expect("a runtime for a case")
            .block_on(run(case))
    })
    .finish("preflight");
}
