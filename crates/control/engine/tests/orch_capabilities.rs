//! What an agent gets when it asks for a capability of its environment: given by policy, refused, or asked of whoever decides on the machine's side; and what it is told of the environment it has.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]

mod common;
mod hosting;
mod native_tools;

use common::{World, eventually, queued};
use orch_api_contract::approvals::{ApprovalDecision, CapabilityDecision};
use orch_authz::Caller;
use orch_engine::capabilities::ApprovalError;
use orch_link_contract::commands::{Command, CommandKind, OrchTool, OrchToolResult, TurnStart};
use orch_link_contract::events::{Event, TurnOutcome};
use orch_types::{Id, Json, new_id};
use serde_json::json;
use std::ops::Deref;

/// Keep the physical node alive while its independently connected harness is used.
/// The base capability cases precede the first owned runtime; live cases enroll one.
struct CapabilityWorld {
    world: World,
    host: hosting::ScriptedHost,
}

impl Deref for CapabilityWorld {
    type Target = World;
    fn deref(&self) -> &World {
        &self.world
    }
}

async fn declared_drivers(world: &World, host: &hosting::ScriptedHost) {
    let mut docker: Json = world
        .scalar("SELECT drivers->'docker' FROM hosts WHERE id=$1", host.id)
        .await;
    docker["version"] = json!("0.1.0");
    docker["confinement"] = json!("isolated");
    docker["process_scope"] = json!("all");
    docker["network_egress"] = json!(["open"]);
    docker["recreate"] = json!("replace");
    docker["capabilities"] =
        json!({"ptrace":{},"kvm":{},"gpu":{},"net_admin":{},"privileged":{},"unconfined":{}});
    let docker: orch_driver_contract::descriptor::DriverDescriptor =
        serde_json::from_value(docker).unwrap();
    let mut programs = json!(docker);
    programs["name"] = json!("host");
    programs["title"] = json!("Programs on the host");
    programs["confinement"] = json!("none");
    programs["capabilities"] = json!({});
    let programs: orch_driver_contract::descriptor::DriverDescriptor =
        serde_json::from_value(programs).unwrap();
    sqlx::query("UPDATE hosts SET drivers=$2 WHERE id=$1")
        .bind(host.id)
        .bind(json!({"docker":docker,"host":programs}))
        .execute(world.pool.pool())
        .await
        .unwrap();
}

async fn thread_on(world: &World, environment: Id, name: &str) -> Id {
    world
        .engine()
        .create_thread(
            &Caller::session(world.ada),
            orch_engine::binding::NewThread {
                topic: world.topic,
                name: name.into(),
                agent: world.agent,
                provider: world.provider,
                environment,
                directory: "web".into(),
                arrangement: None,
                model: Some(common::MODEL.into()),
                effort: None,
            },
        )
        .await
        .unwrap()
        .thread
}

/// Author an actual input, wait for its admitted native start, and keep that exact
/// turn running for tool calls. Request helpers never create inputs or turns.
async fn authored_turn(world: &World, thread: Id, text: &str) -> TurnStart {
    native_tools::start(world, thread, text).await
}

async fn tool_thread(world: &World, name: &str) -> Id {
    let thread = world.thread(name).await;
    authored_turn(
        world,
        thread,
        "Inspect the environment's boundary capabilities",
    )
    .await;
    thread
}

async fn finish_authored_turn(world: &World, thread: Id) -> Id {
    let turn: Id = world
        .scalar(
            "SELECT current_turn_id FROM threads WHERE node_id=$1",
            thread,
        )
        .await;
    world
        .say(thread, common::turn_ended(turn, TurnOutcome::Completed))
        .await;
    world.wait_turn(turn, "completed").await;
    hosting::physical::wait(
        world,
        world
            .scalar(
                "SELECT environment_id FROM threads WHERE node_id=$1",
                thread,
            )
            .await,
        thread,
        "the authored capability turn's terminal follow-up",
        || async {
            world
                .scalar::<bool>("SELECT end_work IS NULL FROM turns WHERE id=$1", turn)
                .await
        },
    )
    .await;
    let current: Option<Id> = world
        .scalar(
            "SELECT current_turn_id FROM threads WHERE node_id=$1",
            thread,
        )
        .await;
    assert!(
        current.is_none(),
        "only the test's next explicitly authored input may create a turn"
    );
    turn
}

async fn a_prepared_live_environment(
    world: &World,
) -> (hosting::ScriptedHost, Id, Id, hosting::Unit) {
    hosting::serve(world);
    let host = hosting::ScriptedHost::on(world, "live-capabilities", 16_384).await;
    hosting::physical::report(world, host.machine, false, Some(true), "open").await;
    declared_drivers(world, &host).await;
    let template = hosting::template(world).await;
    sqlx::query("UPDATE env_templates SET spec=spec || $2 WHERE id=$1")
        .bind(template)
        .bind(json!({"allowed_capabilities":["kvm"],"base_capabilities":[]}))
        .execute(world.pool.pool())
        .await
        .unwrap();
    sqlx::query("UPDATE capability_defs SET driver_patch=$2 WHERE key=$1")
        .bind("kvm")
        .bind(json!({"docker":{"devices":[{"path":"/dev/kvm","permissions":"rwm"}]}}))
        .execute(world.pool.pool())
        .await
        .unwrap();
    sqlx::query("UPDATE hosts SET drivers=jsonb_set(drivers,'{docker,live_changes}',$2) WHERE id=$1").bind(host.id)
        .bind(json!({"resources":true,"mounts":true,"capabilities":["kvm","docker_socket"],"gateway_given":["port_place","port_host","port_link"]})).execute(world.pool.pool()).await.unwrap();
    let (environment, runtime, unit) =
        hosting::running(world, &host, template, "Prepared live unit").await;
    native_tools::workspace_on(world, &unit.added.machine, environment);
    let capabilities = json!({"allowed":["kvm"],"base":[],"granted":[],"patches":{"kvm":{"devices":[{"path":"/dev/kvm","permissions":"rwm"}]}}});
    sqlx::query("UPDATE jobs SET plan=jsonb_set(plan,'{created_spec,capabilities}',$2) WHERE environment_id=$1
        AND kind='environment_provision' AND state='succeeded' AND plan->>'runtime'=$3")
        .bind(environment).bind(capabilities).bind(runtime.to_string()).execute(world.pool.pool()).await.unwrap();
    (host, environment, runtime, unit)
}

#[tokio::test]
async fn a_live_grant_reports_available_now_after_the_node_applies_it_and_sends_no_recreate_notices()
 {
    let world = a_world_with_capabilities().await;
    let (host, environment, runtime, _unit) = a_prepared_live_environment(&world).await;
    world
        .set(
            world.topic,
            "capability.policy",
            json!({"kvm":{"decision":"allow","approver_scope":"host"}}),
        )
        .await;
    world
        .set(
            world.topic,
            "orch_tools.enabled",
            json!(["read_shared", "request_capability"]),
        )
        .await;
    world
        .set(
            world.topic,
            "agent.instructions",
            json!("FIXED-CAPABILITY-CORE: keep the workspace and its guidance unchanged."),
        )
        .await;
    let thread = thread_on(&world, environment, "Live KVM").await;
    let baseline = authored_turn(&world, thread, "Before the capability").await;
    let initial_open = world.commands.issued_bodies("session.open")[0].1.clone();
    assert_eq!(
        initial_open["orch_tools"],
        json!(["read_shared", "request_capability"])
    );
    assert!(
        initial_open["instructions"]
            .as_str()
            .unwrap()
            .contains("FIXED-CAPABILITY-CORE")
    );
    let initial_tools = own_tool_bytes(&world, thread).await;
    let initial_records = core_records(&world, thread).await;
    assert_eq!(
        initial_records.len(),
        2,
        "the accepted opening has real core and tool records"
    );
    let initial_files = world.commands.issued_of("fs.materialize");
    let result = ask(&world, thread, "kvm").await;
    let answer = result.result.unwrap();
    assert_eq!(answer["status"], "granted");
    let notice = answer["notice"].as_str().unwrap();
    assert!(notice.contains("is available now"));
    assert!(notice.contains("Nothing stops"));
    assert_eq!(host.count(CommandKind::NodeCapabilitiesApply), 1);
    for kind in [
        CommandKind::NodeRuntimeCreate,
        CommandKind::NodeRuntimeStop,
        CommandKind::NodeRuntimeStart,
        CommandKind::NodeRuntimeRemove,
    ] {
        assert_eq!(host.count(kind), 0, "a live grant has no {kind}");
    }
    let mut conn = world.pool.acquire().await.unwrap();
    assert_eq!(
        orch_db::queries::environments::hosting::live_runtime(&mut conn, environment)
            .await
            .unwrap()
            .unwrap()
            .id,
        runtime
    );
    let rows = orch_db::queries::jobs::open_of_environment(&mut conn, environment)
        .await
        .unwrap();
    assert!(rows.iter().all(|row| row.kind != "environment_recreate"));
    let pending: Json = sqlx::query_scalar("SELECT pending_notices FROM threads WHERE node_id=$1")
        .bind(thread)
        .fetch_one(&mut *conn)
        .await
        .unwrap();
    assert!(pending.as_array().unwrap().iter().all(|frame| !matches!(
        frame["kind"].as_str(),
        Some("recreate_before" | "recreate_after")
    )));
    let active: Option<Id> =
        sqlx::query_scalar("SELECT current_turn_id FROM threads WHERE node_id=$1")
            .bind(thread)
            .fetch_one(&mut *conn)
            .await
            .unwrap();
    assert_eq!(
        active,
        Some(baseline.turn_id),
        "the capability outcome preserves the actual requesting turn"
    );
    assert_eq!(
        world.turn_starts(thread).len(),
        1,
        "only the explicitly authored baseline has run"
    );
    drop(conn);
    assert_eq!(finish_authored_turn(&world, thread).await, baseline.turn_id);
    assert_eq!(
        world
            .count("SELECT count(*) FROM turns WHERE thread_id=$1", thread)
            .await,
        1,
        "finishing the requesting turn starts no autonomous capability-outcome turn"
    );
    world
        .engine()
        .submit_input(
            &Caller::session(world.ada),
            thread,
            queued("Continue with the newly available device"),
        )
        .await
        .unwrap();
    let input = world.run_turn(thread, 1, "Done").await;
    assert!(
        input.text.contains("The environment has: kvm."),
        "{}",
        input.text
    );
    assert_eq!(
        input.text,
        "<orch>\nWhere you are\n- The environment has: kvm. It can be given: nothing.\n</orch>\n\nContinue with the newly available device",
        "only the changed capability line precedes the real input; the fixed core is not reinserted"
    );
    assert_eq!(
        own_tool_bytes(&world, thread).await,
        initial_tools,
        "capability facts alone preserve exact ordered tool bytes"
    );
    assert_eq!(
        core_records(&world, thread).await,
        initial_records,
        "the accepted core/tool bytes and their records are neither replaced nor rewritten"
    );
    let openings = world.commands.issued_bodies("session.open");
    assert_eq!(
        openings.len(),
        1,
        "the live capability adds no instruction/tool reconfiguration"
    );
    assert_eq!(openings[0].1["instructions"], initial_open["instructions"]);
    assert_eq!(world.commands.issued_of("session.mcp.reload"), 0);
    assert_eq!(
        world.commands.issued_of("fs.materialize"),
        initial_files,
        "no capability path rewrites workspace guidance"
    );
    assert_eq!(
        world.turn_starts(thread).len(),
        2,
        "there are just the two authored inputs"
    );
    let revoke = world
        .engine()
        .revoke_capability(&Caller::session(world.ada), environment, "kvm")
        .await
        .unwrap()
        .unwrap();
    let (state, _, error) = hosting::wait_over(&world, revoke.id).await;
    assert_eq!(state, "succeeded", "{error:?}");
    assert_eq!(host.count(CommandKind::NodeCapabilitiesApply), 2);
    let mut conn = world.pool.acquire().await.unwrap();
    assert_eq!(
        orch_db::queries::environments::hosting::live_runtime(&mut conn, environment)
            .await
            .unwrap()
            .unwrap()
            .id,
        runtime
    );
}

#[tokio::test]
async fn a_failed_live_application_is_not_reported_or_put_in_context_as_a_granted_capability() {
    let world = a_world_with_capabilities().await;
    let (host, environment, _, _unit) = a_prepared_live_environment(&world).await;
    world
        .set(
            world.topic,
            "capability.policy",
            json!({"kvm":{"decision":"allow","approver_scope":"host"}}),
        )
        .await;
    host.refuse(
        CommandKind::NodeCapabilitiesApply,
        orch_link_contract::commands::CommandError::new(
            orch_link_contract::commands::ErrorCode::Unsupported,
            "the prepared source changed",
        ),
    );
    let thread = thread_on(&world, environment, "Unavailable KVM").await;
    authored_turn(&world, thread, "Ask the node to apply the device").await;
    let result = ask(&world, thread, "kvm").await;
    assert!(result.result.is_none());
    assert!(
        result
            .error
            .unwrap()
            .message
            .contains("could not apply it live")
    );
    let mut conn = world.pool.acquire().await.unwrap();
    let view = world
        .engine()
        .environment_capabilities(&mut conn, environment)
        .await
        .unwrap()
        .unwrap();
    assert!(view.held.iter().all(|capability| capability.key != "kvm"));
    assert_eq!(host.count(CommandKind::NodeRuntimeCreate), 0);
}

#[tokio::test]
async fn an_approved_live_grant_that_fails_reports_its_reason_and_rollback_to_the_person_and_agent()
{
    let world = a_world_with_capabilities().await;
    let (host, environment, _, _unit) = a_prepared_live_environment(&world).await;
    host.added
        .machine
        .handler
        .on(CommandKind::NodeCapabilitiesApply, |_| {
            Ok(
                orch_link_contract::commands::CommandResult::NodeCapabilitiesApply(
                    orch_link_contract::commands::NodeCapabilitiesApplyResult {
                        applied: false,
                        restarted: true,
                        reason: Some("the live device helper exited with status 1".into()),
                    },
                ),
            )
        });
    let thread = thread_on(&world, environment, "Approved but unavailable KVM").await;
    authored_turn(&world, thread, "Ask for the device").await;
    let (status, _, request) = said(&ask(&world, thread, "kvm").await);
    assert_eq!(status, "pending");
    let request: Id = request.unwrap().parse().unwrap();
    world
        .engine()
        .decide_capability(&Caller::session(world.ada), request, &approve(true, None))
        .await
        .unwrap();
    eventually("the approved grant's application failure", || async {
        world.scalar::<bool>("SELECT COALESCE(decision->'application'->>'state'='failed',false) FROM approvals WHERE id=$1", request).await
    }).await;
    let decision: Json = world
        .scalar("SELECT decision FROM approvals WHERE id=$1", request)
        .await;
    let reason = decision["application"]["reason"].as_str().unwrap();
    assert!(reason.contains("device helper exited with status 1"));
    assert!(reason.contains("restarted to restore its previous permissions"));
    let state: String = world
        .scalar("SELECT state FROM approvals WHERE id=$1", request)
        .await;
    assert_eq!(
        state, "approved",
        "consent is retained separately from application"
    );
    let notices: Json = world
        .scalar(
            "SELECT pending_notices FROM threads WHERE node_id=$1",
            thread,
        )
        .await;
    let notice = notices.to_string();
    assert!(notice.contains("was not granted"));
    assert!(notice.contains("device helper exited with status 1"));
    assert!(notice.contains("restarted to restore its previous permissions"));
    let inbox: (String, Json) = sqlx::query_as("SELECT body,data FROM notifications WHERE user_id=$1 AND approval_id=$2 AND kind='job.finished'")
        .bind(world.ada).bind(request).fetch_one(world.pool.pool()).await.unwrap();
    assert!(inbox.0.contains("device helper exited with status 1"));
    assert!(
        inbox
            .0
            .contains("restarted to restore its previous permissions")
    );
    assert_eq!(inbox.1["path"], "/approvals");
    let view = world
        .engine()
        .environment_capabilities(&mut world.pool.acquire().await.unwrap(), environment)
        .await
        .unwrap()
        .unwrap();
    assert!(view.held.iter().all(|capability| capability.key != "kvm"));
    assert_eq!(host.count(CommandKind::NodeRuntimeCreate), 0);
}

/// A world whose environment is made from a template of the `docker` driver that allows some capabilities and
/// gives one from the start, with the catalogue of capabilities the system ships.
async fn a_world_with_capabilities() -> CapabilityWorld {
    let world = World::new().await;
    native_tools::enable(&world).await;
    for (key, label, risk, decision, scope) in [
        ("ptrace", "Debugger access", "medium", "allow", "host"),
        ("kvm", "KVM virtualisation", "high", "ask", "host"),
        ("gpu", "GPU devices", "medium", "ask", "host"),
        ("net_admin", "Network administration", "high", "ask", "host"),
        (
            "privileged",
            "Privileged container",
            "critical",
            "deny",
            "system",
        ),
        (
            "unconfined",
            "No container sandbox",
            "critical",
            "ask",
            "org",
        ),
    ] {
        sqlx::query(
            "INSERT INTO capability_defs (key, label, description, risk, driver_patch, default_decision, default_approver_scope)
             VALUES ($1, $2, 'A thing that is wanted.', $3, '{\"docker\": {\"cap_add\": [\"X\"]}}', $4, $5)",
        )
        .bind(key)
        .bind(label)
        .bind(risk)
        .bind(decision)
        .bind(scope)
        .execute(world.pool.pool())
        .await
        .unwrap();
    }
    world
        .exec(
            "UPDATE env_templates SET driver = 'docker', spec = '{\"allowed_capabilities\": [\"ptrace\", \"kvm\", \"gpu\", \"privileged\"], \"base_capabilities\": [\"net_admin\"]}'",
        )
        .await;
    // Placement alone declares no driver ability or machine-policy ceiling. Keep
    // the existing scripted connector, explicitly name its prospective physical
    // host, and leave the first owned runtime absent for the before-creation cases.
    let host = hosting::ScriptedHost::on(&world, "capability-host", 16_384).await;
    hosting::physical::report(&world, host.machine, false, Some(true), "open").await;
    hosting::physical::report(&world, world.machine_id, false, Some(true), "open").await;
    declared_drivers(&world, &host).await;
    sqlx::query("UPDATE environments SET config=config || $2 WHERE id=$1")
        .bind(world.environment)
        .bind(json!({"env.host_id":host.id}))
        .execute(world.pool.pool())
        .await
        .unwrap();
    native_tools::existing_workspace(&world);
    let mut conn = world.pool.acquire().await.unwrap();
    let facts = orch_db::queries::environment_facts::of(&mut conn, world.environment)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(facts.machine_id, Some(host.machine));
    assert_eq!(facts.confinement(), "isolated");
    assert_eq!(
        facts.descriptor.as_ref().unwrap()["capabilities"]["kvm"],
        json!({})
    );
    assert_eq!(facts.policy.as_ref().unwrap()["agent.max_sandbox"], "open");
    assert!(
        orch_db::queries::environments::hosting::live_runtime(&mut conn, world.environment)
            .await
            .unwrap()
            .is_none()
    );
    drop(conn);
    CapabilityWorld { world, host }
}

async fn own_tool_bytes(world: &World, thread: Id) -> Vec<u8> {
    let mut conn = world.pool.acquire().await.unwrap();
    let row = orch_db::queries::threads::thread(&mut conn, thread)
        .await
        .unwrap()
        .unwrap();
    let settings = orch_engine::resolved::load(&mut conn, thread)
        .await
        .unwrap();
    let offered = orch_engine::tools::offered_for_session(
        &mut conn,
        &row,
        true,
        orch_types::Time::new(world.clock.now()),
    )
    .await
    .unwrap();
    let catalogue: Vec<_> = offered
        .into_iter()
        .map(|tool| {
            orch_engine::tools::definitions::definition_at(
                tool,
                &settings.messages,
                &settings.broadcast_agent_scope,
                &settings.broadcast_agent_options,
            )
            .to_mcp()
        })
        .collect();
    serde_json::to_vec(&catalogue).unwrap()
}

async fn core_records(world: &World, thread: Id) -> Vec<(Id, Json)> {
    sqlx::query_as("SELECT id,blocks FROM model_inputs WHERE node_id=$1 AND purpose IN ('session','tool') ORDER BY purpose,id")
        .bind(thread).fetch_all(world.pool.pool()).await.unwrap()
}

async fn ask(world: &World, thread: Id, capability: &str) -> OrchToolResult {
    ask_with_reason(
        world,
        thread,
        capability,
        "I need it for a test of the kernel",
    )
    .await
}

async fn ask_with_reason(
    world: &World,
    thread: Id,
    capability: &str,
    reason: &str,
) -> OrchToolResult {
    let call_ref = new_id().to_string();
    let current: Option<Id> = world
        .scalar(
            "SELECT current_turn_id FROM threads WHERE node_id = $1",
            thread,
        )
        .await;
    let current =
        current.expect("a capability request belongs to an explicitly authored active turn");
    assert_eq!(
        native_tools::prepare(world, thread).await,
        current,
        "the baseline already exists before a request is sent"
    );
    let counts: (i64, i64) = sqlx::query_as(
        "SELECT (SELECT count(*) FROM turns WHERE thread_id=$1),
        (SELECT count(*) FROM thread_inputs WHERE thread_id=$1)",
    )
    .bind(thread)
    .fetch_one(world.pool.pool())
    .await
    .unwrap();
    native_tools::send(
        world,
        thread,
        current,
        &call_ref,
        OrchTool::RequestCapability,
        json!({"capability":capability,"reason":reason}),
    )
    .await;
    let answer = native_tools::answer(world, thread, &call_ref).await;
    assert_eq!(native_tools::active(world, thread).await, current);
    let after: (i64, i64) = sqlx::query_as(
        "SELECT (SELECT count(*) FROM turns WHERE thread_id=$1),
        (SELECT count(*) FROM thread_inputs WHERE thread_id=$1)",
    )
    .bind(thread)
    .fetch_one(world.pool.pool())
    .await
    .unwrap();
    assert_eq!(
        after, counts,
        "a capability answer creates neither an input nor an autonomous turn"
    );
    answer
}

fn said(result: &OrchToolResult) -> (String, String, Option<String>) {
    let body = result.result.clone().expect("a result, not a refusal");
    (
        body["status"].as_str().unwrap().to_owned(),
        body["notice"].as_str().unwrap().to_owned(),
        body["request_id"].as_str().map(str::to_owned),
    )
}

async fn a_system_admin(world: &World, name: &str) -> Id {
    let admin = world.person(name).await;
    world
        .exec(&format!(
            "UPDATE users SET is_system_admin = true WHERE id = '{admin}'"
        ))
        .await;
    admin
}

/// Policy that sends the decision of `kvm` to the system's admins, who a test can name.
async fn decided_by_the_system(world: &World) {
    world
        .set(
            world.root,
            "capability.policy",
            json!({ "kvm": { "decision": "ask", "approver_scope": "system" } }),
        )
        .await;
}

fn approve(approve: bool, note: Option<&str>) -> ApprovalDecision {
    ApprovalDecision::Capability(CapabilityDecision {
        approve,
        note: note.map(str::to_owned),
    })
}

async fn gateways(world: &World) {
    for (key, risk, decision) in [
        ("port_place", "medium", "ask"),
        ("port_host", "high", "deny"),
        ("port_link", "high", "deny"),
    ] {
        sqlx::query("INSERT INTO capability_defs (key,label,description,risk,driver_patch,default_decision,default_approver_scope) VALUES ($1,$1,'A gateway audience.',$2,'{}',$3,'environment')")
            .bind(key).bind(risk).bind(decision).execute(world.pool.pool()).await.unwrap();
    }
    world.exec("UPDATE env_templates SET spec = jsonb_set(spec,'{allowed_capabilities}',(spec->'allowed_capabilities') || '[\"port_place\",\"port_host\",\"port_link\"]'::jsonb)").await;
}

#[tokio::test]
async fn a_gateway_capability_is_granted_and_revoked_without_recreating_or_restarting_the_unit() {
    let world = a_world_with_capabilities().await;
    gateways(&world).await;
    world
        .set(
            world.topic,
            "capability.policy",
            json!({"port_place":{"decision":"allow","approver_scope":"environment"}}),
        )
        .await;
    let thread = tool_thread(&world, "A place port").await;
    let before_generation: i32 = world
        .scalar(
            "SELECT generation FROM environments WHERE id = $1",
            world.environment,
        )
        .await;
    let (status, notice, _) = said(&ask(&world, thread, "port_place").await);
    assert_eq!(status, "granted");
    assert!(notice.contains("available now") && notice.contains("Nothing stops"));
    assert!(!notice.contains("recreat"));
    assert!(
        world
            .engine()
            .revoke_capability(&Caller::session(world.ada), world.environment, "port_place")
            .await
            .unwrap()
            .is_none()
    );
    let after_generation: i32 = world
        .scalar(
            "SELECT generation FROM environments WHERE id = $1",
            world.environment,
        )
        .await;
    assert_eq!(before_generation, after_generation);
    assert_eq!(world.count("SELECT count(*) FROM jobs WHERE environment_id = $1 AND kind = 'environment_recreate'",world.environment).await,0);
    for kind in [
        CommandKind::NodeRuntimeCreate,
        CommandKind::NodeRuntimeStop,
        CommandKind::NodeRuntimeStart,
        CommandKind::NodeRuntimeRemove,
    ] {
        assert!(
            world.seen.of_kind(kind).is_empty(),
            "the gateway owns this capability's effects"
        );
    }
    let given: Vec<String> = world
        .scalar(
            "SELECT granted_capabilities FROM environments WHERE id = $1",
            world.environment,
        )
        .await;
    assert!(!given.contains(&"port_place".to_owned()));
}

#[tokio::test]
async fn a_gateway_request_needs_no_driver_patch_and_is_decided_by_current_ports_managers() {
    let world = a_world_with_capabilities().await;
    gateways(&world).await;
    world.exec("UPDATE env_templates SET driver = 'host'").await;
    let thread = tool_thread(&world, "Gateway request").await;
    let (status, notice, request) = said(&ask(&world, thread, "port_place").await);
    assert_eq!(status, "pending");
    assert!(notice.contains("without stopping anything"));
    let request: Id = request.unwrap().parse().unwrap();
    let scope: String = world
        .scalar(
            "SELECT approver_scope FROM approvals WHERE id = $1",
            request,
        )
        .await;
    assert_eq!(scope, "environment");
    let outsider = world.person("reader").await;
    assert!(matches!(
        world
            .engine()
            .decide_capability(&Caller::session(outsider), request, &approve(true, None))
            .await,
        Err(ApprovalError::Denied(_))
    ));
    let approvers = world
        .engine()
        .capability_approvers(
            &mut world.pool.acquire().await.unwrap(),
            world.environment,
            "environment",
        )
        .await
        .unwrap();
    assert!(approvers.contains(&world.ada));
    assert!(!approvers.contains(&outsider));
    world
        .engine()
        .decide_capability(&Caller::session(world.ada), request, &approve(true, None))
        .await
        .unwrap();
    assert_eq!(world.count("SELECT count(*) FROM jobs WHERE environment_id = $1 AND kind = 'environment_recreate'",world.environment).await,0);
}

#[tokio::test]
async fn an_identical_denied_capability_reuses_the_first_note_for_an_hour_and_changed_content_asks_again()
 {
    let world = a_world_with_capabilities().await;
    decided_by_the_system(&world).await;
    let thread = tool_thread(&world, "Kernel work").await;
    let (_, _, request) = said(&ask(&world, thread, "kvm").await);
    let first: Id = request.unwrap().parse().unwrap();
    let admin = a_system_admin(&world, "hostadmin").await;
    world
        .engine()
        .decide_capability(
            &Caller::session(admin).with_admin_mode(true),
            first,
            &approve(false, Some("This host must stay without virtual machines.")),
        )
        .await
        .unwrap();
    let (status, notice, request) = said(&ask(&world, thread, "kvm").await);
    assert_eq!(status, "denied");
    assert!(notice.contains("This host must stay without virtual machines."));
    assert_eq!(request, Some(first.to_string()));
    let count: i64 = world
        .count(
            "SELECT count(*) FROM approvals WHERE environment_id = $1 AND kind = 'capability'",
            world.environment,
        )
        .await;
    assert_eq!(count, 1, "no second card is raised for identical content");
    let (status, _, next) = said(
        &ask_with_reason(
            &world,
            thread,
            "kvm",
            "The host has changed; I now need a different integration test.",
        )
        .await,
    );
    assert_eq!(status, "pending");
    assert_ne!(next, Some(first.to_string()));
    world
        .exec(&format!(
            "UPDATE approvals SET decided_at = now() - interval '61 minutes' WHERE id = '{first}'"
        ))
        .await;
    world.exec(&format!("UPDATE approvals SET state = 'cancelled' WHERE environment_id = '{}' AND state = 'pending'", world.environment)).await;
    assert_eq!(
        said(&ask(&world, thread, "kvm").await).0,
        "pending",
        "an expired denial hold permits a fresh request"
    );
}

#[tokio::test]
async fn every_thread_approval_mode_leaves_boundary_capabilities_to_the_policy_and_scoped_approvers()
 {
    let world = a_world_with_capabilities().await;
    decided_by_the_system(&world).await;
    for mode in ["ask", "on_request", "bypass"] {
        let thread = world.thread(&format!("Boundary {mode}")).await;
        world.set(thread, "approval.mode", json!(mode)).await;
        let start = authored_turn(
            &world,
            thread,
            "Check the boundary independently of harness approvals",
        )
        .await;
        assert_eq!(
            json!(start.approval_mode),
            json!(mode),
            "the real turn uses the requested declared mode"
        );
        let (status, _, request) = said(&ask(&world, thread, "kvm").await);
        assert_eq!(status, "pending", "{mode} never grants a capability");
        let approval: Id = request.unwrap().parse().unwrap();
        assert!(matches!(
            world
                .engine()
                .decide_capability(&Caller::session(world.ada), approval, &approve(true, None))
                .await,
            Err(ApprovalError::Denied(_))
        ));
        assert_eq!(
            said(&ask(&world, thread, "privileged").await).0,
            "denied",
            "{mode} never bypasses boundary policy"
        );
    }
    assert!(
        world.seen.of_kind(CommandKind::ApprovalResolve).is_empty(),
        "capability calls reach no harness reviewer"
    );
    let given: Vec<String> = world
        .scalar(
            "SELECT granted_capabilities FROM environments WHERE id = $1",
            world.environment,
        )
        .await;
    assert!(given.is_empty());
}

#[tokio::test]
async fn an_unavailable_native_approval_mode_starts_no_turn_and_grants_no_boundary_capability() {
    let world = a_world_with_capabilities().await;
    decided_by_the_system(&world).await;
    let thread = world.thread("Unavailable native Auto").await;
    let mut conn = world.pool.acquire().await.unwrap();
    let declared: Json = sqlx::query_scalar("SELECT descriptor FROM agents WHERE id=$1")
        .bind(world.agent)
        .fetch_one(&mut *conn)
        .await
        .unwrap();
    let descriptor: orch_agent_contract::descriptor::AgentDescriptor =
        serde_json::from_value(declared).unwrap();
    assert!(
        !descriptor
            .approval_modes
            .contains(&orch_agent_contract::descriptor::ApprovalMode::Auto),
        "the fixture must not claim a native mode its harness cannot run"
    );
    let settings = orch_engine::resolved::load(&mut conn, thread)
        .await
        .unwrap();
    let offered = world
        .engine()
        .offered_approval_modes(
            &mut conn,
            Some(world.environment),
            &descriptor,
            &settings.allowed_modes,
            &[common::MODEL.to_owned()],
        )
        .await
        .unwrap();
    assert!(!offered.contains(&orch_agent_contract::descriptor::ApprovalMode::Auto));
    drop(conn);
    // Reconstruct a persisted request for an unavailable mode. No fabricated
    // tool call or turn is used to pretend the harness accepted it.
    world.set(thread, "approval.mode", json!("auto")).await;
    let submitted = world
        .engine()
        .submit_input(
            &Caller::session(world.ada),
            thread,
            queued("Use only an approval mode this harness actually supports"),
        )
        .await
        .unwrap();
    let state: (String, Option<Id>, Option<String>, Option<String>) = sqlx::query_as(
        "SELECT state,current_turn_id,blocked_reason,blocked_detail FROM threads WHERE node_id=$1",
    )
    .bind(thread)
    .fetch_one(world.pool.pool())
    .await
    .unwrap();
    assert_eq!(state.0, "blocked");
    assert_eq!(state.1, None);
    assert_eq!(state.2.as_deref(), Some("binding_invalid"));
    assert!(
        state
            .3
            .as_deref()
            .is_some_and(|detail| detail.contains("approval mode") && detail.contains("Auto")),
        "{state:?}"
    );
    assert_eq!(
        submitted.input.state,
        orch_api_contract::threads::InputState::Pending
    );
    assert_eq!(
        world
            .count("SELECT count(*) FROM turns WHERE thread_id=$1", thread)
            .await,
        0
    );
    assert!(world.turn_starts(thread).is_empty());
    assert_eq!(
        world
            .count(
                "SELECT count(*) FROM approvals WHERE node_id=$1 AND kind='capability'",
                thread
            )
            .await,
        0
    );
    let given: Vec<String> = world
        .scalar(
            "SELECT granted_capabilities FROM environments WHERE id=$1",
            world.environment,
        )
        .await;
    assert!(given.is_empty());
    assert!(
        world.seen.of_kind(CommandKind::ApprovalResolve).is_empty(),
        "an unavailable mode grants neither native nor boundary authority"
    );
}

#[tokio::test]
async fn capability_expiry_uses_the_shared_shortest_request_setting() {
    let world = a_world_with_capabilities().await;
    world
        .set(world.root, "requests.expiry_hours", json!(3))
        .await;
    world
        .set(world.topic, "requests.expiry_hours", json!(12))
        .await;
    let thread = tool_thread(&world, "Short request").await;
    let (_, _, request) = said(&ask(&world, thread, "kvm").await);
    let request: Id = request.unwrap().parse().unwrap();
    let hours: f64 = world.scalar("SELECT extract(epoch FROM (expires_at - created_at))::float8 / 3600 FROM approvals WHERE id = $1", request).await;
    assert_eq!(hours, 3.0);
}

#[tokio::test]
async fn a_kept_harness_rule_and_stop_never_decide_a_pending_boundary_capability() {
    let world = a_world_with_capabilities().await;
    decided_by_the_system(&world).await;
    let thread = world.thread("Boundary and native work").await;
    let start = authored_turn(&world, thread, "Run native work").await;
    world.say(thread, Event::ApprovalRequested(orch_link_contract::events::ApprovalRequested {
        turn_id:start.turn_id, request_ref:"native-rule".into(), title:"Kernel test command".into(),
        request:serde_json::from_value(json!({"request_kind":"command","detail":{"command":"kvm test","cwd":"/workspace","reason":null},"options":[
            {"id":"allow_once","label":"Allow once","style":"allow","scope":"once","rule":null},
            {"id":"allow_thread","label":"Allow for this thread","style":"allow","scope":"thread","rule":{"type":"command_prefix","argv":["kvm"],"cwd":"/workspace"}},
            {"id":"deny","label":"Deny","style":"deny","scope":null,"rule":null}
        ]})).unwrap(),
    })).await;
    world.wait_state(thread, "waiting_approval").await;
    let native: Id = world.scalar("SELECT id FROM approvals WHERE thread_id = $1 AND kind = 'harness' AND native_ref = 'native-rule'", thread).await;
    world
        .engine()
        .decide_approval(
            &Caller::session(world.ada),
            native,
            &ApprovalDecision::Option(orch_api_contract::approvals::HarnessOptionDecision {
                option_id: "allow_thread".into(),
                note: None,
            }),
        )
        .await
        .unwrap();
    world.wait_state(thread, "running").await;
    let (status, _, request) = said(&ask(&world, thread, "kvm").await);
    assert_eq!(status, "pending");
    let request: Id = request.unwrap().parse().unwrap();
    let before: String = world
        .scalar("SELECT state FROM approvals WHERE id = $1", request)
        .await;
    assert_eq!(
        before, "pending",
        "the native allow rule cannot grant host capabilities"
    );
    world
        .engine()
        .interrupt(&Caller::session(world.ada), thread, start.turn_id)
        .await
        .unwrap();
    let after: String = world
        .scalar("SELECT state FROM approvals WHERE id = $1", request)
        .await;
    assert_eq!(
        after, "pending",
        "Stop affects the turn, while the boundary card still belongs to scoped approvers"
    );
}

#[tokio::test]
async fn a_capability_the_policy_allows_before_a_unit_exists_is_applied_at_its_first_creation() {
    let world = a_world_with_capabilities().await;
    let thread = tool_thread(&world, "Debug").await;
    let (status, notice, request) = said(&ask(&world, thread, "ptrace").await);
    assert_eq!(status, "granted");
    assert!(
        notice.starts_with("\"Debugger access\" was granted."),
        "{notice}"
    );
    assert!(notice.contains("first unit is made; there is no unit to stop"));
    assert!(request.is_none());
    let granted: Vec<String> = world
        .scalar(
            "SELECT granted_capabilities FROM environments WHERE id = $1",
            world.environment,
        )
        .await;
    assert_eq!(granted, ["ptrace"]);
    assert_eq!(
        world
            .count(
                "SELECT count(*) FROM jobs WHERE environment_id=$1 AND kind='environment_recreate'",
                world.environment
            )
            .await,
        0
    );
    let marker: Json = world
        .scalar(
            "SELECT payload FROM items WHERE thread_id = $1 AND kind = 'marker'
              AND payload ->> 'marker' = 'capability_granted'",
            thread,
        )
        .await;
    assert_eq!(marker["data"]["capability"], "ptrace");
    assert_eq!(marker["data"]["by"], Json::Null, "policy allowed it");
    let audited: i64 = world
        .count(
            "SELECT count(*) FROM audit_log WHERE environment_id = $1 AND action IN ('capability.requested', 'capability.granted')",
            world.environment,
        )
        .await;
    assert_eq!(audited, 2);
}

#[tokio::test]
async fn a_capability_the_policy_asks_about_waits_for_a_decision_and_is_asked_for_once() {
    let world = a_world_with_capabilities().await;
    let thread = tool_thread(&world, "Debug").await;
    let (status, notice, request) = said(&ask(&world, thread, "kvm").await);
    assert_eq!(status, "pending");
    assert!(notice.starts_with(
        "Requested \"KVM virtualisation\". If approved: This capability is applied when the environment's first unit is made"
    ));
    assert!(notice.ends_with("Continue with work that does not need it."));
    let request = request.expect("the request's identifier");
    let (kind, state, scope, node): (String, String, Option<String>, Id) = sqlx::query_as(
        "SELECT kind, state, approver_scope, node_id FROM approvals WHERE id = $1::uuid",
    )
    .bind(&request)
    .fetch_one(world.pool.pool())
    .await
    .unwrap();
    assert_eq!((kind.as_str(), state.as_str()), ("capability", "pending"));
    assert_eq!(scope.as_deref(), Some("host"));
    assert_eq!(node, thread);
    // Nothing is given before a decision, and asking again is the same request.
    let granted: Vec<String> = world
        .scalar(
            "SELECT granted_capabilities FROM environments WHERE id = $1",
            world.environment,
        )
        .await;
    assert!(granted.is_empty());
    let (again, _, same) = said(&ask(&world, thread, "kvm").await);
    assert_eq!(again, "pending");
    assert_eq!(same.as_deref(), Some(request.as_str()));
    let pending: i64 = world
        .count(
            "SELECT count(*) FROM approvals WHERE environment_id = $1 AND kind = 'capability'",
            world.environment,
        )
        .await;
    assert_eq!(pending, 1);
    let notices: Vec<Id> = sqlx::query_scalar(
        "SELECT user_id FROM notifications
        WHERE kind = 'capability.requested' AND approval_id = $1::uuid",
    )
    .bind(&request)
    .fetch_all(world.pool.pool())
    .await
    .unwrap();
    let mut conn = world.pool.acquire().await.unwrap();
    let eligible = world
        .engine()
        .capability_approvers(&mut conn, world.environment, "host")
        .await
        .unwrap();
    assert!(!eligible.is_empty(), "the requesting host has approvers");
    for person in &eligible {
        assert_eq!(
            notices.iter().filter(|id| *id == person).count(),
            1,
            "an eligible approver is told once"
        );
    }
    assert!(notices.iter().all(|person| eligible.contains(person)));
}

#[tokio::test]
async fn a_request_before_the_first_unit_uses_only_the_current_selected_hosts_managers() {
    let world = a_world_with_capabilities().await;
    let thread = tool_thread(&world, "Selected host boundary").await;
    let (_, _, request) = said(&ask(&world, thread, "kvm").await);
    let request: Id = request.unwrap().parse().unwrap();
    let recorded: Json = world
        .scalar("SELECT request FROM approvals WHERE id=$1", request)
        .await;
    assert_eq!(
        recorded["host"]["id"],
        json!(world.host.id),
        "the approval describes the actual selected host before creation"
    );
    assert_eq!(
        world
            .count(
                "SELECT count(*) FROM runtimes WHERE environment_id=$1",
                world.environment
            )
            .await,
        0
    );
    let manager = world.person("environment-manager").await;
    let mut conn = world.pool.acquire().await.unwrap();
    orch_db::queries::access::testing::add_entry(
        &mut conn,
        &orch_db::queries::access::PageKey::new("environment", world.environment, "live"),
        "any",
        "environment.manage",
        ("user", Some(manager), None),
        None,
        world.clock.now(),
    )
    .await
    .unwrap();
    let eligible = world
        .engine()
        .capability_approvers(&mut conn, world.environment, "host")
        .await
        .unwrap();
    assert!(
        eligible.contains(&world.ada),
        "the selected host's owner can decide its boundary"
    );
    assert!(
        !eligible.contains(&manager),
        "managing the environment grants no host right"
    );
    drop(conn);
    assert!(matches!(
        world
            .engine()
            .decide_capability(&Caller::session(manager), request, &approve(true, None))
            .await,
        Err(ApprovalError::Denied(_))
    ));
    // A descriptor offered by some host is not a selection and confers no right.
    sqlx::query("UPDATE environments SET config=config-'env.host_id' WHERE id=$1")
        .bind(world.environment)
        .execute(world.pool.pool())
        .await
        .unwrap();
    assert!(
        matches!(
            world
                .engine()
                .decide_capability(&Caller::session(world.ada), request, &approve(true, None))
                .await,
            Err(ApprovalError::Denied(_))
        ),
        "the previous host owner cannot decide after the host was unselected"
    );
    sqlx::query("UPDATE environments SET config=config || $2 WHERE id=$1")
        .bind(world.environment)
        .bind(json!({"env.host_id":world.host.id}))
        .execute(world.pool.pool())
        .await
        .unwrap();
    let decided = world
        .engine()
        .decide_capability(&Caller::session(world.ada), request, &approve(true, None))
        .await
        .unwrap();
    assert_eq!(decided.state, "approved");
    assert_eq!(decided.decided_by, Some(world.ada));
    let granted: Vec<String> = world
        .scalar(
            "SELECT granted_capabilities FROM environments WHERE id=$1",
            world.environment,
        )
        .await;
    assert_eq!(granted, ["kvm"]);
    assert_eq!(
        world
            .count(
                "SELECT count(*) FROM runtimes WHERE environment_id=$1",
                world.environment
            )
            .await,
        0
    );
    assert_eq!(
        world.turn_starts(thread).len(),
        1,
        "the scoped decision adds no autonomous turn"
    );
}

#[tokio::test]
async fn a_request_is_decided_by_the_scope_the_policy_names_and_never_by_the_people_whose_work_asked()
 {
    let world = a_world_with_capabilities().await;
    decided_by_the_system(&world).await;
    let thread = tool_thread(&world, "Debug").await;
    let (_, _, request) = said(&ask(&world, thread, "kvm").await);
    let request: Id = request.unwrap().parse().unwrap();
    // Ada owns the thread, the topic and the environment, and decides nothing here.
    let refused = world
        .engine()
        .decide_capability(&Caller::session(world.ada), request, &approve(true, None))
        .await;
    assert!(
        matches!(refused, Err(ApprovalError::Denied(_))),
        "{refused:?}"
    );
    let admin = a_system_admin(&world, "root").await;
    assert!(
        matches!(
            world
                .engine()
                .decide_capability(&Caller::session(admin), request, &approve(true, None))
                .await,
            Err(ApprovalError::Denied(_))
        ),
        "a system-admin user in an ordinary session does not exercise administrative authority"
    );
    let decided = world
        .engine()
        .decide_capability(
            &Caller::session(admin).with_admin_mode(true),
            request,
            &approve(true, Some("fine")),
        )
        .await
        .unwrap();
    assert_eq!(decided.state, "approved");
    assert_eq!(decided.decided_by, Some(admin));
    let granted: Vec<String> = world
        .scalar(
            "SELECT granted_capabilities FROM environments WHERE id = $1",
            world.environment,
        )
        .await;
    assert_eq!(granted, ["kvm"]);
    let jobs: i64 = world
        .count(
            "SELECT count(*) FROM jobs WHERE environment_id = $1 AND kind = 'environment_recreate'",
            world.environment,
        )
        .await;
    assert_eq!(
        jobs, 0,
        "a grant before the unit exists applies at creation, without a replacement job"
    );
    // The first decision wins.
    let late = world
        .engine()
        .decide_capability(
            &Caller::session(admin).with_admin_mode(true),
            request,
            &approve(false, None),
        )
        .await;
    assert!(matches!(late, Err(ApprovalError::AlreadyDecided)));
}

#[tokio::test]
async fn a_denied_request_tells_the_asking_thread_who_denied_it_and_why_and_gives_nothing() {
    let world = a_world_with_capabilities().await;
    decided_by_the_system(&world).await;
    let thread = tool_thread(&world, "Debug").await;
    let (_, _, request) = said(&ask(&world, thread, "kvm").await);
    let request: Id = request.unwrap().parse().unwrap();
    let admin = a_system_admin(&world, "rootadmin").await;
    world
        .engine()
        .decide_capability(
            &Caller::session(admin).with_admin_mode(true),
            request,
            &approve(false, Some("not on this host")),
        )
        .await
        .unwrap();
    let granted: Vec<String> = world
        .scalar(
            "SELECT granted_capabilities FROM environments WHERE id = $1",
            world.environment,
        )
        .await;
    assert!(granted.is_empty());
    let first = finish_authored_turn(&world, thread).await;
    assert_eq!(
        world
            .count("SELECT count(*) FROM turns WHERE thread_id = $1", thread)
            .await,
        1,
        "only the explicitly authored requesting turn exists; an outcome never starts one"
    );
    let start = authored_turn(&world, thread, "Continue after the decision").await;
    assert_ne!(start.turn_id, first);
    assert_eq!(
        world.turn_starts(thread).len(),
        2,
        "there are exactly the two explicitly authored inputs"
    );
    assert!(common::words_after_where(&start.text).contains("Your request for \"KVM virtualisation\" was denied by rootadmin (@rootadmin): not on this host."));
    let audited: i64 = world
        .count("SELECT count(*) FROM audit_log WHERE environment_id = $1 AND action = 'capability.denied'", world.environment)
        .await;
    assert_eq!(audited, 1);
}

#[tokio::test]
async fn where_the_place_names_nobody_a_denial_comes_from_an_approver_with_the_note_kept() {
    let world = a_world_with_capabilities().await;
    decided_by_the_system(&world).await;
    world
        .set(world.topic, "collab.attribution", json!("never"))
        .await;
    let thread = tool_thread(&world, "Debug").await;
    let (_, _, request) = said(&ask(&world, thread, "kvm").await);
    let request: Id = request.unwrap().parse().unwrap();
    let admin = a_system_admin(&world, "rootadmin").await;
    world
        .engine()
        .decide_capability(
            &Caller::session(admin).with_admin_mode(true),
            request,
            &approve(false, Some("not on this host")),
        )
        .await
        .unwrap();
    let first = finish_authored_turn(&world, thread).await;
    assert_eq!(
        world
            .count("SELECT count(*) FROM turns WHERE thread_id=$1", thread)
            .await,
        1
    );
    let start = authored_turn(&world, thread, "Continue after the decision").await;
    assert_ne!(start.turn_id, first);
    assert_eq!(world.turn_starts(thread).len(), 2);
    assert!(common::words_after_where(&start.text).contains(
        "Your request for \"KVM virtualisation\" was denied by an approver: not on this host."
    ));
}

#[tokio::test]
async fn a_request_nobody_decided_in_time_lapses_and_the_asking_thread_is_told() {
    let world = a_world_with_capabilities().await;
    decided_by_the_system(&world).await;
    let thread = tool_thread(&world, "Debug").await;
    let (_, _, request) = said(&ask(&world, thread, "kvm").await);
    let request: Id = request.unwrap().parse().unwrap();
    world
        .exec(&format!(
            "UPDATE approvals SET expires_at = now() - interval '1 minute' WHERE id = '{request}'"
        ))
        .await;
    eventually("the capability expired without starting a turn", || {
        let world = &world;
        async move {
            world
                .count(
                    "SELECT count(*) FROM approvals WHERE id = $1 AND state = 'expired'",
                    request,
                )
                .await
                == 1
        }
    })
    .await;
    let first = finish_authored_turn(&world, thread).await;
    assert_eq!(
        world
            .count("SELECT count(*) FROM turns WHERE thread_id = $1", thread)
            .await,
        1,
        "expiry starts no turn beyond the authored baseline"
    );
    let start = authored_turn(&world, thread, "Continue after expiry").await;
    assert_ne!(start.turn_id, first);
    assert_eq!(world.turn_starts(thread).len(), 2);
    assert!(
        common::words_after_where(&start.text)
            .contains("Your request for \"KVM virtualisation\" expired without a decision.")
    );
    let state: String = world
        .scalar("SELECT state FROM approvals WHERE id = $1", request)
        .await;
    assert_eq!(state, "expired");
    let late = world
        .engine()
        .decide_capability(
            &Caller::session(a_system_admin(&world, "root").await).with_admin_mode(true),
            request,
            &approve(true, None),
        )
        .await;
    assert!(matches!(late, Err(ApprovalError::AlreadyDecided)));
}

#[tokio::test]
async fn a_key_the_environment_does_not_offer_is_answered_with_the_keys_it_does() {
    let world = a_world_with_capabilities().await;
    let thread = tool_thread(&world, "Debug").await;
    for key in ["teleport", "unconfined"] {
        let (status, notice, _) = said(&ask(&world, thread, key).await);
        assert_eq!(status, "denied");
        assert_eq!(
            notice,
            format!(
                "\"{key}\" is not offered by this environment. Offered: gpu, kvm, privileged, ptrace."
            )
        );
    }
}

#[tokio::test]
async fn what_the_environment_holds_is_available_and_what_policy_denies_is_denied() {
    let world = a_world_with_capabilities().await;
    let thread = tool_thread(&world, "Debug").await;
    let (status, notice, _) = said(&ask(&world, thread, "net_admin").await);
    assert_eq!(
        (status.as_str(), notice.as_str()),
        (
            "granted",
            "\"Network administration\" is already available."
        )
    );
    let (status, notice, _) = said(&ask(&world, thread, "privileged").await);
    assert_eq!(
        (status.as_str(), notice.as_str()),
        ("denied", "\"Privileged container\" is denied by policy.")
    );
}

#[tokio::test]
async fn capabilities_do_not_apply_to_a_directory_on_a_persons_machine() {
    let world = a_world_with_capabilities().await;
    world
        .exec("UPDATE environments SET placement = 'connected', template_id = NULL, workspace_root = '/home/ada/work'")
        .await;
    hosting::physical::report(&world, world.machine_id, true, Some(true), "open").await;
    let connector = hosting::primary(&world);
    native_tools::workspace_on(&world, &connector.added.machine, world.environment);
    let thread = tool_thread(&world, "Debug").await;
    let (status, notice, _) = said(&ask(&world, thread, "kvm").await);
    assert_eq!(status, "denied");
    assert_eq!(notice, "Capabilities do not apply to this environment.");
}

#[tokio::test]
async fn the_entry_its_creator_used_on_the_template_limits_what_may_be_given() {
    let world = a_world_with_capabilities().await;
    let entry = new_id();
    sqlx::query(
        "INSERT INTO permission_entries (id, object_kind, object_id, page, permission, holder_kind, options)
         SELECT $1, 'template', template_id, 'live', 'template.use', 'everyone', '{\"capabilities\": [\"ptrace\"]}'
           FROM environments WHERE id = $2",
    )
    .bind(entry)
    .bind(world.environment)
    .execute(world.pool.pool())
    .await
    .unwrap();
    world
        .exec(&format!(
            "UPDATE environments SET template_entry_id = '{entry}'"
        ))
        .await;
    let thread = tool_thread(&world, "Debug").await;
    let (status, notice, _) = said(&ask(&world, thread, "kvm").await);
    assert_eq!(status, "denied");
    assert!(notice.ends_with("Offered: ptrace."), "{notice}");
}

#[tokio::test]
async fn the_preamble_says_what_the_environment_has_and_can_be_given() {
    let world = a_world_with_capabilities().await;
    let thread = world.thread("Debug").await;
    let start = authored_turn(&world, thread, "Start").await;
    let Command::SessionOpen(open) = world.seen.of_kind(CommandKind::SessionOpen).remove(0) else {
        panic!("a session was opened");
    };
    // Where the agent is goes with its first input, and says what the environment has and can be given.
    assert!(
        start.text.contains(
            "- The environment has: net_admin. It can be given: gpu, kvm, privileged, ptrace (request_capability)."
        ),
        "{}",
        start.text
    );
    assert!(
        open.instructions
            .contains("request_capability asks for what your environment lacks.")
    );
    assert_eq!(finish_authored_turn(&world, thread).await, start.turn_id);
}

#[tokio::test]
async fn whoever_manages_the_environment_takes_a_capability_back_before_its_first_creation() {
    let world = a_world_with_capabilities().await;
    let thread = tool_thread(&world, "Debug").await;
    ask(&world, thread, "ptrace").await;
    let job = world
        .engine()
        .revoke_capability(&Caller::session(world.ada), world.environment, "ptrace")
        .await
        .unwrap();
    assert!(job.is_none(), "there is no unit to replace");
    let granted: Vec<String> = world
        .scalar(
            "SELECT granted_capabilities FROM environments WHERE id = $1",
            world.environment,
        )
        .await;
    assert!(granted.is_empty());
    // Somebody who manages nothing there is refused, and so is taking back what was never given.
    let stranger = world.person("stranger").await;
    assert!(
        world
            .engine()
            .revoke_capability(&Caller::session(stranger), world.environment, "ptrace")
            .await
            .is_err()
    );
    assert!(
        world
            .engine()
            .revoke_capability(&Caller::session(world.ada), world.environment, "ptrace")
            .await
            .is_err()
    );
}
