//! The combination matrix of boundary requests: what an agent's `request_capability` comes to for each
//! capability (the driver-given ones and the three port audiences), whether its template offers it, whether
//! the environment already has it, the policy at a place (or the definition's default), the thread's approval mode,
//! the driver and the placement, asked once or twice; and, where a person decides, what the thread is told under each
//! `collab.attribution`.
//!
//! Expected: a key that does not apply to the
//! environment, or that its template does not offer, is denied; one it has is available; the place's policy (or the
//! definition's default) denies, grants at once (a port audience live, without a recreate) or asks the approvers of
//! its scope; **no approval mode decides or changes any of it** (a thread in bypass asks like one in ask); the
//! same request again finds the pending one, and a denied one is denied again from the record within the hour;
//! a person's approval grants the key, a denial tells the thread who denied it, or "an approver" where the place
//! has `collab.attribution = never`.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]

mod common;
mod hosting;
mod matrix_kit;

use common::{World, eventually};
use matrix_kit::{Case, Matrix, dim, failure};
use orch_api_contract::approvals::{ApprovalDecision, CapabilityDecision};
use orch_authz::Caller;
use orch_link_contract::commands::{Command, CommandKind, CommandResult, OrchTool, OrchToolResult};
use orch_link_contract::events::{Event, OrchToolCall};
use orch_settings_contract::registry::keys;
use orch_types::{Id, Json, new_id};
use serde_json::json;

const SOURCE: &str = "Capability eligibility, policy and pending-decision order; grants, denial reuse, attribution and port audiences";

/// The capabilities of the matrix: key, label, risk, default decision, default approver scope.
const DEFINITIONS: &[(&str, &str, &str, &str, &str)] = &[
    ("ptrace", "Debugger access", "medium", "allow", "host"),
    ("kvm", "KVM virtualisation", "high", "ask", "host"),
    ("gpu", "GPU devices", "medium", "ask", "host"),
    ("net_admin", "Network administration", "high", "ask", "host"),
    (
        "unconfined",
        "No container sandbox",
        "critical",
        "ask",
        "org",
    ),
    (
        "privileged",
        "Privileged container",
        "critical",
        "deny",
        "system",
    ),
    (
        "port_place",
        "Ports for a named place",
        "medium",
        "ask",
        "environment",
    ),
    (
        "port_host",
        "Ports on the host",
        "high",
        "deny",
        "environment",
    ),
    (
        "port_link",
        "Ports for anyone with a link",
        "high",
        "deny",
        "environment",
    ),
];

fn gateway(key: &str) -> bool {
    key.starts_with("port_")
}

fn default_decision(key: &str) -> &'static str {
    DEFINITIONS
        .iter()
        .find(|row| row.0 == key)
        .map_or("deny", |row| row.3)
}

/// The answer an agent should get, as a status and the words that tell its kind.
#[derive(Debug, Clone, PartialEq)]
struct Answer {
    status: &'static str,
    /// Words the notice must hold (any one of them).
    words: Vec<&'static str>,
}

fn answer(status: &'static str, words: &[&'static str]) -> Answer {
    Answer {
        status,
        words: words.to_vec(),
    }
}

/// The expected first-request outcome, following eligibility and policy order.
fn first_answer(case: &Case) -> Answer {
    let key = case.get("key");
    let not_applying = ["Capabilities do not apply", "is not offered"];
    if case.is("placement", "connected") {
        return answer("denied", &not_applying);
    }
    if case.is("driver", "host") && !gateway(key) && key != "unknown" {
        // No patch for this driver: the key does not apply here (row 1), or is not what the template can give (row 2).
        return answer("denied", &not_applying);
    }
    if key == "unknown" || case.is("offered", "not_listed") {
        return answer("denied", &["is not offered"]);
    }
    if !case.is("held", "no") {
        return answer("granted", &["is already available"]);
    }
    let decision = match case.get("policy") {
        "default" => default_decision(key),
        explicit => explicit,
    };
    match decision {
        "deny" => answer("denied", &["is denied by policy"]),
        "allow" if gateway(key) => answer("granted", &["is available now"]),
        // A recreate: said with its grace and what is kept; the rig's environment has no unit yet, for which the
        // engine says it is applied when the first unit is made (no unit to stop), which is the same path.
        "allow" => answer("granted", &["recreat", "Recreat", "first unit is made"]),
        _ if gateway(key) => answer("pending", &["without stopping anything"]),
        _ => answer("pending", &[""]),
    }
}

/// What the same request asked again comes to.
fn second_answer(first: &Answer) -> Answer {
    match first.status {
        "pending" => answer("pending", &[""]),
        "granted" => answer("granted", &["is already available"]),
        _ => answer("denied", &[""]),
    }
}

async fn a_world(case: &Case) -> (World, hosting::ScriptedHost) {
    // No read of the subscription starts its harness while the case changes what the adapter declares: a start
    // already on its way would store the declaration it was answered with.
    let world = World::observed().await;
    // The adapter declares Orch's tools at every start of its harness, which the profile then keeps.
    let mut descriptor = common::descriptor();
    descriptor["capabilities"]["orch_tools"] = json!("mcp");
    world.declares(descriptor).await;
    // A driver-given key is offered only where the driver of the template declares the keys it can apply: the host
    // that offers Docker here declares the matrix's keys, and none offers the host driver.
    let host = hosting::ScriptedHost::on(&world, "boundary-host", 16_384).await;
    let applies: serde_json::Map<String, Json> = DEFINITIONS
        .iter()
        .filter(|row| !gateway(row.0))
        .map(|row| (row.0.to_owned(), json!({ "type": "object" })))
        .collect();
    sqlx::query(
        "UPDATE hosts SET drivers = jsonb_set(drivers, '{docker,capabilities}', $2) WHERE id = $1",
    )
    .bind(host.id)
    .bind(Json::Object(applies))
    .execute(world.pool.pool())
    .await
    .unwrap();
    for (key, label, risk, decision, scope) in DEFINITIONS {
        let patch = if gateway(key) {
            json!({})
        } else {
            json!({"docker": {"cap_add": ["X"]}})
        };
        sqlx::query(
            "INSERT INTO capability_defs (key, label, description, risk, driver_patch, default_decision, default_approver_scope)
             VALUES ($1, $2, 'A thing that is wanted.', $3, $4, $5, $6)",
        )
        .bind(key)
        .bind(label)
        .bind(risk)
        .bind(patch)
        .bind(decision)
        .bind(scope)
        .execute(world.pool.pool())
        .await
        .unwrap();
    }
    let key = case.get("key");
    // The template offers every key but the asked one, which it offers or not as the case says.
    let mut allowed: Vec<&str> = DEFINITIONS
        .iter()
        .map(|row| row.0)
        .filter(|each| *each != key)
        .collect();
    if case.is("offered", "listed") {
        allowed.push(key);
    }
    let base: Vec<&str> = if case.is("held", "base") {
        vec![key]
    } else {
        Vec::new()
    };
    sqlx::query("UPDATE env_templates SET driver = $1, spec = $2")
        .bind(case.get("driver"))
        .bind(json!({ "allowed_capabilities": allowed, "base_capabilities": base }))
        .execute(world.pool.pool())
        .await
        .unwrap();
    if case.is("held", "granted") {
        sqlx::query("UPDATE environments SET granted_capabilities = ARRAY[$1]")
            .bind(key)
            .execute(world.pool.pool())
            .await
            .unwrap();
    }
    if case.is("placement", "connected") {
        // A directory on a person's machine is attached in each session of its machine, whose answer carries the
        // directory's facts.
        world.machine.handler.on(CommandKind::EnvAttach, |command| {
            let mut facts = json!({
                "os": "linux", "arch": "x86_64",
                "shell": { "name": "sh", "path": "/bin/sh" },
                "user": "agent", "home": "/home/agent", "workspace_root": "/workspace",
                "path_style": "posix", "case_sensitive": true, "executors": [],
                "mcp_found": [], "index": "full", "mounts": [],
                "private_views": false, "suspend": false, "git": true, "generation": 1
            });
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
        // A person's directory has no template.
        world
            .exec("UPDATE environments SET placement = 'connected', template_id = NULL")
            .await;
        eventually("the directory is attached in its machine's session", || async {
            let attached: bool = world
                .scalar(
                    "SELECT COALESCE(e.status = 'running'
                            AND e.attached_session = COALESCE(m.session_started_at, m.last_seen_at), false)
                       FROM environments e JOIN machines m ON m.id = e.machine_id WHERE e.id = $1",
                    world.environment,
                )
                .await;
            attached
        })
        .await;
    }
    if !case.is("policy", "default") {
        let scope = if gateway(key) {
            "environment"
        } else {
            "system"
        };
        let place = match case.get("policy_at") {
            "root" => world.root,
            "project" => world.project,
            _ => world.topic,
        };
        world
            .set(
                place,
                "capability.policy",
                json!({ key: { "decision": case.get("policy"), "approver_scope": scope } }),
            )
            .await;
    }
    world
        .set(
            world.topic,
            keys::COLLAB_ATTRIBUTION,
            json!(case.get("attribution")),
        )
        .await;
    (world, host)
}

async fn ask(world: &World, thread: Id, capability: &str) -> OrchToolResult {
    let call_ref = new_id().to_string();
    let current: Option<Id> = world
        .scalar(
            "SELECT current_turn_id FROM threads WHERE node_id = $1",
            thread,
        )
        .await;
    let turn = current.unwrap_or_else(new_id);
    let arguments = json!({ "capability": capability, "reason": "I need it for the matrix" });
    world
        .say(
            thread,
            Event::OrchToolCall(OrchToolCall {
                turn_id: turn,
                call_ref: call_ref.clone(),
                attempt: None,
                tool: OrchTool::RequestCapability,
                arguments: arguments.clone(),
            }),
        )
        .await;
    let started = std::time::Instant::now();
    while answers(world, &call_ref).is_empty() {
        if started.elapsed() > std::time::Duration::from_secs(10) {
            // The engine answers a call in a task of its own and only logs a database error there (no answer is
            // sent): ask it once more directly, so that the case says what went wrong.
            let context = orch_engine::tools::CallContext {
                machine: world.machine_id,
                thread,
                turn,
                call_ref: call_ref.clone(),
                attempt: None,
                tool: serde_json::from_value(json!("request_capability")).expect("the tool's name"),
                arguments: arguments.clone(),
            };
            match world.engine().answer_tool_call(&context).await {
                Err(error) => {
                    panic!("no answer to request_capability; answering it directly fails: {error}")
                }
                Ok(()) => {
                    eventually("the answer to a request asked directly", || {
                        let found = answers(world, &call_ref).is_empty();
                        async move { !found }
                    })
                    .await;
                    break;
                }
            }
        }
        tokio::time::sleep(std::time::Duration::from_millis(25)).await;
    }
    answers(world, &call_ref).remove(0)
}

fn answers(world: &World, call_ref: &str) -> Vec<OrchToolResult> {
    world
        .seen
        .of_kind(CommandKind::OrchToolResult)
        .into_iter()
        .filter_map(|command| match command {
            Command::OrchToolResult(result) if result.call_ref == call_ref => Some(result),
            _ => None,
        })
        .collect()
}

/// The status, the notice and the request of an answer; a refusal (an error result) reads as `refused`.
fn read(result: &OrchToolResult) -> (String, String, Option<String>) {
    match &result.result {
        Some(body) => (
            body["status"].as_str().unwrap_or_default().to_owned(),
            body["notice"].as_str().unwrap_or_default().to_owned(),
            body["request_id"].as_str().map(str::to_owned),
        ),
        None => ("refused".to_owned(), format!("{:?}", result.error), None),
    }
}

fn matches(want: &Answer, got: &(String, String, Option<String>)) -> bool {
    got.0 == want.status && want.words.iter().any(|words| got.1.contains(words))
}

async fn run(case: &Case) -> Result<(), matrix_kit::Failure> {
    let (world, _host) = a_world(case).await;
    let thread = world.thread("Boundary").await;
    // An agent asks from its running turn: a call names the turn it comes from, and one from no running turn is
    // refused. The thread takes the case's mode once that turn runs, since bypass starts no turn where no driver
    // declares isolation (this rig's environment is on no host); no mode decides what a request comes to.
    world.running_turn(thread, "Work at the boundary").await;
    world
        .set(thread, keys::APPROVAL_MODE, json!(case.get("mode")))
        .await;
    let key = case.get("key");
    let asked = if key == "unknown" { "warp_drive" } else { key };
    let first = read(&ask(&world, thread, asked).await);
    let want = first_answer(case);
    if !matches(&want, &first) {
        return Err(failure(format!("{want:?}"), SOURCE, format!("{first:?}")));
    }
    if case.is("repeat", "twice") {
        let second = read(&ask(&world, thread, asked).await);
        let again = second_answer(&want);
        if !matches(&again, &second) {
            return Err(failure(
                format!("asked again: {again:?}"),
                SOURCE,
                format!("{second:?}"),
            ));
        }
        if want.status == "pending" && second.2 != first.2 {
            return Err(failure(
                "the same pending request (rule 4: one identity)",
                SOURCE,
                format!("{:?} then {:?}", first.2, second.2),
            ));
        }
    }
    if case.is("decide", "none") || want.status != "pending" {
        return Ok(());
    }
    let request: Id = first
        .2
        .as_deref()
        .unwrap_or_default()
        .parse()
        .map_err(|_| failure("a request id", SOURCE, format!("{first:?}")))?;
    // The decider of the policy's scope: the environment's owner for a port audience, a system admin otherwise, who
    // decides as one only in a session's explicit As admin mode.
    let decider = if gateway(key) {
        Caller::session(world.ada)
    } else {
        let admin = world.person("hostadmin").await;
        world
            .exec(&format!(
                "UPDATE users SET is_system_admin = true WHERE id = '{admin}'"
            ))
            .await;
        Caller::session(admin).with_admin_mode(true)
    };
    let approve = case.is("decide", "approve");
    world
        .engine()
        .decide_capability(
            &decider,
            request,
            &ApprovalDecision::Capability(CapabilityDecision {
                approve,
                note: (!approve).then(|| "Not on this host.".to_owned()),
            }),
        )
        .await
        .map_err(|error| {
            failure(
                "the decision of the policy's approver is taken",
                SOURCE,
                format!("{error:?}"),
            )
        })?;
    if approve {
        let granted: Vec<String> = world
            .scalar(
                "SELECT granted_capabilities FROM environments WHERE id = $1",
                world.environment,
            )
            .await;
        // A key given by a recreate is the environment's at once, and its recreate is a job; in this rig no host
        // carries the job out, and a failed job takes the key back, so the job stands for the grant too.
        let recreating = world
            .count("SELECT count(*) FROM jobs WHERE environment_id = $1 AND kind = 'environment_recreate'", world.environment)
            .await;
        if !granted.iter().any(|each| each == key) && (gateway(key) || recreating == 0) {
            return Err(failure(
                format!("{key} among the environment's grants"),
                SOURCE,
                format!("{granted:?}, {recreating} recreate jobs"),
            ));
        }
        return Ok(());
    }
    let pending: Json = world
        .scalar(
            "SELECT pending_notices FROM threads WHERE node_id = $1",
            thread,
        )
        .await;
    let text = pending.to_string();
    let name = if gateway(key) { "ada" } else { "hostadmin" };
    let named = text.contains(&format!("denied by {name}"));
    let anonymous = text.contains("denied by an approver");
    let ok = match case.get("attribution") {
        "never" => anonymous && !named && text.contains("Not on this host."),
        _ => named && text.contains("Not on this host."),
    };
    if ok {
        Ok(())
    } else {
        Err(failure(
            format!(
                "the thread told it was denied by {} with the note",
                if case.is("attribution", "never") {
                    "an approver"
                } else {
                    name
                }
            ),
            SOURCE,
            text,
        ))
    }
}

fn the_matrix() -> Matrix {
    Matrix::new(
        "boundary",
        &[
            dim(
                "key",
                &[
                    "ptrace",
                    "kvm",
                    "gpu",
                    "net_admin",
                    "unconfined",
                    "privileged",
                    "port_place",
                    "port_host",
                    "port_link",
                    "unknown",
                ],
            ),
            dim("offered", &["listed", "not_listed"]),
            dim("held", &["no", "base", "granted"]),
            dim("policy", &["default", "allow", "ask", "deny"]),
            dim("policy_at", &["root", "project", "topic"]),
            dim("mode", &["ask", "on_request", "bypass"]),
            dim("driver", &["docker", "host"]),
            dim("placement", &["managed", "connected"]),
            dim("repeat", &["once", "twice"]),
            dim("decide", &["none", "approve", "deny"]),
            dim("attribution", &["auto", "always", "never"]),
        ],
        |pick| {
            let key = pick.get("key");
            let pending_possible = pick.is("policy", "ask")
                && pick.is("offered", "listed")
                && pick.is("held", "no")
                && pick.is("placement", "managed")
                && (gateway(key) || pick.is("driver", "docker"))
                && key != "unknown";
            !(key == "unknown" && (!pick.is("offered", "not_listed") || !pick.is("held", "no") || !pick.is("policy", "default")))
                && !(!pick.is("held", "no") && pick.is("offered", "not_listed"))
                // A driver-given key is held only where the driver can give it (a template of the host driver cannot
                // list it among its defaults, and a request for it there is refused before any grant).
                && !(!pick.is("held", "no") && !gateway(key) && pick.is("driver", "host"))
                && !(pick.is("policy", "default") && !pick.is("policy_at", "root"))
                // A person's directory has no template: its driver, offer and holdings are not the matrix's to vary,
                // and a port audience there is decided by its machine's Ports ceiling, which is not modelled here.
                && !(pick.is("placement", "connected")
                    && (gateway(key) || !pick.is("driver", "docker") || !pick.is("held", "no") || !pick.is("offered", "listed")))
                && !(!pick.is("decide", "none") && !pending_possible)
                && !(!pick.is("attribution", "auto") && !pick.is("decide", "deny"))
        },
    )
}

#[test]
fn every_boundary_request_is_answered_by_its_policy_whatever_the_approval_mode() {
    let matrix = the_matrix();
    // Cross policy, mode and capability; the mode never grants a capability by itself. Also cross the
    // decision and the attribution together, for what the thread is told.
    let cases = matrix.cases(
        3,
        &[
            &["key", "policy", "mode"],
            &["decide", "attribution", "key"],
        ],
        20_261_015,
        30,
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
    .finish("boundary");
}
