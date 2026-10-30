//! The combination matrix of confinement: the approval mode a thread asks for, the modes its places and its
//! agent allow, the sandbox mode it asks for, the environment's widest and default sandbox (a managed environment's
//! template, or a person's machine's local policy), the modes the harness can enforce and those its executor found
//! it can enforce there; and what the turn is started with, or why it is not.
//!
//! Expected behavior: the approval mode is the nearest scope's, else the
//! environment's default, which is `on_request` everywhere; it must be among `approval.allowed_modes` (intersected
//! along the places, unset meaning all) and among the agent's modes. The shipped adapter fixture omits `auto`.
//! The sandbox mode is the nearest scope's, else the environment's default (a template's
//! `sandbox.default`; `workspace` on a person's machine), lowered to the environment's widest (a machine's
//! `agent.max_sandbox`, `workspace` when its policy says nothing, narrowed by a template's `sandbox.max`), then to the
//! widest mode at or below that which the harness declares and its executor reports it can enforce there; when no
//! mode remains, or the approval mode is not allowed, the thread is `blocked(binding_invalid)`. Otherwise
//! `turn.start` carries both modes.
//!
//! An actual template with no `sandbox.max` uses its confinement default; no template adds no ceiling.
//! A running executor with no report or an empty list has declared no enforceable mode.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]

mod common;
mod matrix_kit;

use common::{World, queued};
use matrix_kit::{Case, Matrix, dim, failure};
use orch_authz::Caller;
use orch_link_contract::commands::{Command, CommandKind, CommandResult};
use orch_settings_contract::registry::keys;
use serde_json::{Value as Json, json};
use std::time::{Duration, Instant};

const SOURCE: &str = "Approval and sandbox defaults, inherited ceilings, isolation requirements, and modes declared by the harness and executor";

const ORDER: [&str; 3] = ["read_only", "workspace", "open"];

fn rank(mode: &str) -> usize {
    ORDER
        .iter()
        .position(|each| *each == mode)
        .expect("a sandbox mode")
}

fn modes(word: &str) -> Vec<&'static str> {
    match word {
        "all" => ORDER.to_vec(),
        "open_only" => vec!["open"],
        "no_open" => vec!["read_only", "workspace"],
        "workspace_open" => vec!["workspace", "open"],
        "none" => Vec::new(),
        other => panic!("no list {other}"),
    }
}

/// What the case should come to: the two modes of the turn, or the reason the thread is blocked.
/// `arrangement` is the one the thread runs on: an executor's probe counts only where an executor runs the tools
/// (`separated`); where the harness runs in the environment (`together`) its own declaration is the report.
fn expected(case: &Case, arrangement: &str) -> Result<(&'static str, &'static str), &'static str> {
    let mode = match case.get("mode") {
        "unset" => "on_request",
        other => other,
    };
    let allowed: Vec<&str> = match case.get("allowed") {
        "unset" | "all" => vec!["ask", "on_request", "auto", "bypass"],
        "no_bypass" => vec!["ask", "on_request", "auto"],
        _ => vec!["bypass"],
    };
    let agent: Vec<&str> = match case.get("agent_modes") {
        "shipped" => vec!["ask", "on_request", "bypass"],
        _ => vec!["on_request"],
    };
    if !allowed.contains(&mode) || !agent.contains(&mode) {
        return Err("binding_invalid");
    }
    // Bypass is offered only where another boundary confines the agent: an environment whose driver
    // declares isolation. The rig's environment is on no host, so no driver declares it.
    if mode == "bypass" {
        return Err("binding_invalid");
    }
    let connected = case.is("placement", "connected");
    let asked = match (case.get("sandbox"), case.get("env_default")) {
        ("unset", "unset") => "workspace", // only a person's machine reaches this: the rule keeps it so
        ("unset", default) => default,
        (asked, _) => asked,
    };
    // An actual template's `sandbox.max` narrows the ceiling of the machine the environment is on, never widens
    // it: the rig's machine states no `agent.max_sandbox` for a managed environment and declares no isolation,
    // so its ceiling is the cautious `workspace`.
    let widest = match (case.get("widest"), connected) {
        ("unset", _) => "workspace", // this rig has no isolating driver
        (widest, true) => widest,
        (widest, false) => ORDER[rank(widest).min(rank("workspace"))],
    };
    let harness = modes(case.get("harness_modes"));
    let enforceable: Vec<&str> = match case.get("probed") {
        _ if arrangement != "separated" => harness,
        "unreported" => Vec::new(),
        probed => {
            let found = modes(probed);
            harness
                .into_iter()
                .filter(|mode| found.contains(mode))
                .collect()
        }
    };
    let ceiling = rank(asked).min(rank(widest));
    let sandbox = (0..=ceiling)
        .rev()
        .map(|at| ORDER[at])
        .find(|mode| enforceable.contains(mode));
    sandbox
        .map(|sandbox| (mode, sandbox))
        .ok_or("binding_invalid")
}

async fn arrange(world: &World, case: &Case) -> orch_types::Id {
    let harness: Vec<&str> = modes(case.get("harness_modes"));
    let agent_modes: Vec<&str> = match case.get("agent_modes") {
        "shipped" => vec!["ask", "on_request", "bypass"],
        _ => vec!["on_request"],
    };
    // What the adapter declares is what its harness says at every start, which the profile then keeps: a start
    // after this point must not bring the shipped modes back.
    let mut descriptor = common::descriptor();
    descriptor["sandbox_modes"] = json!(harness);
    descriptor["approval_modes"] = json!(agent_modes);
    world.declares(descriptor).await;
    let mut sandbox = serde_json::Map::new();
    if case.get("widest") != "unset" && case.is("placement", "managed") {
        sandbox.insert("max".to_owned(), json!(case.get("widest")));
    }
    if case.get("env_default") != "unset" {
        sandbox.insert("default".to_owned(), json!(case.get("env_default")));
    }
    sqlx::query("UPDATE env_templates SET spec = jsonb_set(spec, '{sandbox}', $1)")
        .bind(Json::Object(sandbox))
        .execute(world.pool.pool())
        .await
        .unwrap();
    if case.is("placement", "connected") {
        // A directory on a person's machine is attached in each session of its machine, whose answer carries the
        // directory's facts: here none of an executor, as nothing is probed on a person's machine in this rig.
        world.machine.handler.on(CommandKind::EnvAttach, |command| {
            let mut facts = common_facts();
            facts["executors"] = json!([]);
            if let Command::EnvAttach(attach) = command {
                if let Some(root) = attach
                    .workspace_root
                    .as_deref()
                    .filter(|root| !root.is_empty())
                {
                    facts["workspace_root"] = json!(root);
                }
                facts["generation"] = json!(attach.data_generation.unwrap_or(0));
            }
            Ok(CommandResult::EnvAttach(
                serde_json::from_value(facts).unwrap(),
            ))
        });
        world
            .exec("UPDATE environments SET placement = 'connected', template_id = NULL")
            .await;
        if case.get("widest") != "unset" {
            sqlx::query(
                "UPDATE machines SET facts = jsonb_set(facts, '{policy,agent.max_sandbox}', $1)",
            )
            .bind(json!(case.get("widest")))
            .execute(world.pool.pool())
            .await
            .unwrap();
        }
    }
    if case.get("probed") != "unreported" {
        let mut facts = common_facts();
        facts["executors"][0]["sandbox_modes"] = json!(modes(case.get("probed")));
        sqlx::query("UPDATE environments SET facts = $1")
            .bind(facts)
            .execute(world.pool.pool())
            .await
            .unwrap();
    } else {
        world
            .exec("UPDATE environments SET facts = '{}'::jsonb")
            .await;
    }
    match case.get("allowed") {
        "unset" => {}
        "all" => {
            world
                .set(
                    world.topic,
                    keys::APPROVAL_ALLOWED_MODES,
                    json!(["ask", "on_request", "auto", "bypass"]),
                )
                .await
        }
        "no_bypass" => {
            world
                .set(
                    world.topic,
                    keys::APPROVAL_ALLOWED_MODES,
                    json!(["ask", "on_request", "auto"]),
                )
                .await
        }
        _ => {
            world
                .set(world.topic, keys::APPROVAL_ALLOWED_MODES, json!(["bypass"]))
                .await
        }
    }
    let thread = world.thread("Confined").await;
    if case.get("mode") != "unset" {
        world
            .set(thread, keys::APPROVAL_MODE, json!(case.get("mode")))
            .await;
    }
    if case.get("sandbox") != "unset" {
        world
            .set(thread, keys::SANDBOX_MODE, json!(case.get("sandbox")))
            .await;
    }
    if case.is("placement", "connected") {
        common::eventually("the directory is attached in its machine's session", || async move {
            sqlx::query_scalar::<_, bool>(
                "SELECT COALESCE(e.status = 'running'
                        AND e.attached_session = COALESCE(m.session_started_at, m.last_seen_at), false)
                   FROM environments e JOIN machines m ON m.id = e.machine_id WHERE e.id = $1",
            )
            .bind(world.environment)
            .fetch_one(world.pool.pool())
            .await
            .unwrap()
        })
        .await;
    }
    thread
}

/// The facts a connector reports for an environment, with one executor (as the hosting tests write them).
fn common_facts() -> Json {
    json!({
        "os": "linux", "arch": "x86_64",
        "shell": { "name": "sh", "path": "/bin/sh" },
        "user": "agent", "home": "/home/agent", "workspace_root": "/workspace",
        "path_style": "posix", "case_sensitive": true,
        "executors": [{
            "kind": "orch-exec", "version": "0.1.0", "available": true, "reason": null,
            "sandbox_modes": ["read_only", "workspace", "open"], "tool_sets": ["plain"]
        }],
        "mcp_found": [], "index": "full", "mounts": [],
        "private_views": false, "suspend": false, "git": true, "generation": 1
    })
}

async fn run(case: &Case) -> Result<(), matrix_kit::Failure> {
    // No read of the subscription starts its harness while the case changes what the adapter declares: a start
    // already on its way would store the declaration it was answered with.
    let world = World::observed().await;
    let thread = arrange(&world, case).await;
    world
        .engine()
        .submit_input(&Caller::session(world.ada), thread, queued("Confined work"))
        .await
        .map_err(|error| failure("the input is accepted", SOURCE, format!("{error:?}")))?;
    let started = Instant::now();
    let got: Result<(String, String), String> = loop {
        if let Some(start) = world.turn_starts(thread).first() {
            let words = |value: Json| value.as_str().unwrap_or_default().to_owned();
            break Ok((
                words(serde_json::to_value(start.approval_mode).unwrap()),
                words(serde_json::to_value(start.sandbox_mode).unwrap()),
            ));
        }
        let (state, reason, detail) = world.state(thread).await;
        if state == "blocked" {
            // A moment more, so that a thread blocked on its way to a start is not taken for the answer.
            tokio::time::sleep(Duration::from_millis(150)).await;
            let (again, reason_again, _) = world.state(thread).await;
            if again == "blocked" && reason_again == reason {
                break Err(format!(
                    "{} ({})",
                    reason.unwrap_or_default(),
                    detail.unwrap_or_default()
                ));
            }
        }
        if started.elapsed() > Duration::from_secs(15) {
            break Err(format!("neither a start nor a block: {state} {reason:?}"));
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    };
    let arrangement = world.arrangement_of(thread).await;
    let want = expected(case, &arrangement);
    match (want, &got) {
        (Ok((mode, sandbox)), Ok((m, s))) if m == mode && s == sandbox => Ok(()),
        (Err(reason), Err(words)) if words.starts_with(reason) => Ok(()),
        (want, got) => Err(failure(
            format!("{want:?}"),
            SOURCE,
            format!("{got:?} (arrangement {arrangement})"),
        )),
    }
}

#[test]
fn the_turn_is_confined_as_the_places_the_agent_and_the_environment_allow_or_says_what_has_to_give()
{
    let matrix = Matrix::new(
        "confinement",
        &[
            dim("placement", &["managed", "connected"]),
            dim("mode", &["unset", "ask", "on_request", "bypass", "auto"]),
            dim("allowed", &["unset", "all", "no_bypass", "bypass_only"]),
            dim("agent_modes", &["shipped", "on_request_only"]),
            dim("sandbox", &["unset", "read_only", "workspace", "open"]),
            dim("env_default", &["unset", "read_only", "workspace", "open"]),
            dim("widest", &["unset", "read_only", "workspace", "open"]),
            dim("harness_modes", &["all", "open_only", "no_open"]),
            dim(
                "probed",
                &["unreported", "all", "open_only", "workspace_open", "none"],
            ),
        ],
        |pick| {
            // A person's machine has no template: its default is the machine's own; a managed environment whose
            // template states no default is left to the driver's descriptor, which this rig does not model.
            // A person's machine reports its own facts: the rig writes none for it.
            !(pick.is("placement", "connected") && !pick.is("probed", "unreported"))
                && !(pick.is("placement", "connected") && !pick.is("env_default", "unset"))
                && !(pick.is("placement", "managed")
                    && pick.is("env_default", "unset")
                    && pick.is("sandbox", "unset"))
        },
    );
    // Asked, widest and enforceable decide the sandbox together: their whole product.
    let cases = matrix.cases(
        7,
        &[
            &["sandbox", "widest", "probed"],
            &["mode", "allowed", "agent_modes"],
        ],
        20_261_015,
        24,
    );
    // Each case on a thread of its own with a runtime of its own (see `matrix_kit::run_cases`).
    matrix_kit::run_cases(&cases, 4, SOURCE, |case| {
        tokio::runtime::Builder::new_multi_thread()
            .worker_threads(3)
            .thread_stack_size(matrix_kit::CASE_STACK)
            .enable_all()
            .build()
            .expect("a runtime for a case")
            .block_on(run(case))
    })
    .finish("confinement");
}
