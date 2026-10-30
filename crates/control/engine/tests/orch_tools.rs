//! What an agent does through Orch: the tools it is offered, a call carried and answered once, and what `announce` and `send_message` do to the other threads of its topic.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]

mod common;
mod hosting;
mod native_tools;

use common::{World, eventually, queued, turn_ended};
use orch_authz::Caller;
use orch_link_contract::commands::{Command, CommandKind, OrchTool, OrchToolResult};
use orch_link_contract::events::TurnOutcome;
use orch_types::{Id, Json, new_id};
use serde_json::json;
use std::time::Duration;

/// The tools a session is opened with, once the descriptor of the tests' agent says it takes an MCP server.
async fn a_world_whose_agent_takes_tools() -> World {
    let world = World::new().await;
    native_tools::enable(&world).await;
    native_tools::existing_workspace(&world);
    world
}

/// A real node/runtime/unit topology, with each native feature declared and
/// allowed by that node. The harness still runs on the world's provider machine.
async fn a_world_with_all_native_tools() -> (World, hosting::ScriptedHost, hosting::Unit) {
    use orch_link_contract::policy::{LocalPolicy, PolicyValues};
    use orch_link_contract::session::{MachineFacts, Role};
    let mut world = a_world_whose_agent_takes_tools().await;
    hosting::serve(&world);
    let host = hosting::ScriptedHost::on(&world, "tool-catalogue-node", 16_384).await;
    let mut driver: Json = world
        .scalar("SELECT drivers->'docker' FROM hosts WHERE id=$1", host.id)
        .await;
    driver["version"] = json!("0.1.0");
    driver["confinement"] = json!("isolated");
    driver["process_scope"] = json!("all");
    driver["network_egress"] = json!(["open"]);
    driver["recreate"] = json!("replace");
    let driver: orch_driver_contract::descriptor::DriverDescriptor =
        serde_json::from_value(driver).unwrap();
    sqlx::query("UPDATE hosts SET drivers=$2 WHERE id=$1")
        .bind(host.id)
        .bind(json!({"docker":driver}))
        .execute(world.pool.pool())
        .await
        .unwrap();
    hosting::physical::report(&world, host.machine, false, Some(true), "open").await;
    let mut facts: MachineFacts = serde_json::from_value(
        world
            .scalar("SELECT facts FROM machines WHERE id=$1", host.machine)
            .await,
    )
    .unwrap();
    let values = PolicyValues::from([
        ("environment.browser".into(), json!(true)),
        ("environment.vnc".into(), json!(true)),
        ("ports".into(), json!({"allowed":["place"]})),
    ]);
    let mut policy =
        LocalPolicy::resolve(&["*".into()], &values, &PolicyValues::default()).unwrap();
    policy.roles = Some(vec![Role::Node]);
    facts.policy = Some(policy);
    let mut conn = world.pool.acquire().await.unwrap();
    orch_db::queries::resources::set_machine_facts(
        &mut conn,
        host.machine,
        &json!(facts),
        (0, 0),
        1.0,
        world.clock.now(),
    )
    .await
    .unwrap();
    drop(conn);
    let template = hosting::template(&world).await;
    let (environment, _, unit) =
        hosting::running(&world, &host, template, "All native tools").await;
    let mut conn = world.pool.acquire().await.unwrap();
    orch_authz::ownership::start_resource(
        &mut conn,
        orch_access_contract::decisions::ObjectRef {
            kind: orch_access_contract::permissions::ResourceKind::Environment.into(),
            id: environment,
        },
        world.ada,
        orch_authz::ownership::NewAccess::Defaults,
        world.clock.time(),
    )
    .await
    .unwrap();
    let mut facts = hosting::facts();
    facts["browser_profiles_dir"] = json!("/home/agent/.orch/browsers");
    facts["browser"] = json!({"programs":[{"name":"chromium","path":"/usr/bin/chromium","version":"scripted"}],
        "window":true,"virtual_window":true,"playwright_cli":"scripted","sandbox":true});
    facts["vnc"] = json!({"servers":[{"name":"Xvnc","path":"/usr/bin/Xvnc","version":"scripted"}],"window_manager":"openbox"});
    facts["ports"] = json!({"discovery":"native","transports":["tcp","udp"],"reason":null});
    let facts: orch_link_contract::events::EnvironmentFacts =
        serde_json::from_value(facts).unwrap();
    sqlx::query("UPDATE environments SET browser=true,vnc=true,facts=$2 WHERE id=$1")
        .bind(environment)
        .bind(json!(facts))
        .execute(&mut *conn)
        .await
        .unwrap();
    let physical = orch_db::queries::environment_facts::of(&mut conn, environment)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(physical.machine_id, Some(host.machine));
    let policy: LocalPolicy = serde_json::from_value(physical.policy.unwrap()).unwrap();
    assert!(policy.environment_browser && policy.environment_vnc);
    assert!(policy.permits_port(Some(orch_api_contract::ports::PortOpeningKind::Place)));
    drop(conn);
    world.environment = environment;
    native_tools::workspace_on(&world, &unit.added.machine, environment);
    (world, host, unit)
}

/// The harness's side: the agent of `thread` calls a tool, and Orch's answer is waited for.
async fn call(world: &World, thread: Id, tool: OrchTool, arguments: Json) -> OrchToolResult {
    let call_ref = new_id().to_string();
    say_call(world, thread, &call_ref, tool, arguments).await;
    answer_to(world, thread, &call_ref).await
}

async fn say_call(world: &World, thread: Id, call_ref: &str, tool: OrchTool, arguments: Json) {
    let turn = native_tools::prepare(world, thread).await;
    native_tools::send(world, thread, turn, call_ref, tool, arguments).await;
}

fn results(world: &World, thread: Id, call_ref: &str) -> Vec<OrchToolResult> {
    native_tools::results(world, thread, call_ref)
}

async fn answer_to(world: &World, thread: Id, call_ref: &str) -> OrchToolResult {
    native_tools::answer(world, thread, call_ref).await
}

fn refused(result: &OrchToolResult) -> (&str, &str) {
    let error = result.error.as_ref().expect("a refusal");
    (error.code.as_str(), error.message.as_str())
}

/// Runs the first turn of a thread so that its session is opened, and returns what the session was opened with.
async fn open_session(world: &World, thread: Id) -> orch_link_contract::commands::SessionOpen {
    native_tools::existing_workspace(world);
    world
        .engine()
        .submit_input(&Caller::session(world.ada), thread, queued("Start"))
        .await
        .unwrap();
    world.run_turn(thread, 0, "Fine").await;
    world
        .seen
        .of_kind(CommandKind::SessionOpen)
        .into_iter()
        .filter_map(|command| match command {
            Command::SessionOpen(open) if open.thread_id == thread => Some(open),
            _ => None,
        })
        .next_back()
        .expect("a session was opened")
}

// ---------------------------------------------------------------- which tools an agent is offered

#[tokio::test]
async fn a_failed_database_step_answers_the_tool_with_unavailable_without_waiting_for_recovery() {
    let world = a_world_whose_agent_takes_tools().await;
    let thread = world.thread("A database failure is answered").await;
    let turn = native_tools::prepare(&world, thread).await;
    let engine = world.engine();
    // Stop unrelated background work, then permanently close only this fixture's data pool.
    // The machine link and its command recorder stay usable, so the real failure boundary
    // must answer the native caller even though acquiring a database connection cannot recover.
    engine.stop();
    tokio::time::timeout(common::PATIENCE, world.pool.pool().close())
        .await
        .expect("the fixture's pool closes within the ordinary test bound");
    assert!(
        world.pool.acquire().await.is_err(),
        "the database step really fails"
    );
    let call = orch_engine::tools::CallContext {
        machine: world.machine_id,
        thread,
        turn,
        call_ref: "failed-database-step".to_owned(),
        attempt: None,
        tool: orch_agent_contract::events::OrchTool::RequestCapability,
        arguments: json!({ "capability": "ptrace", "reason": "Inspect the running program" }),
    };
    tokio::time::timeout(common::PATIENCE, engine.answer_tool_call(&call))
        .await
        .expect("a failed database read must not leave the tool waiting")
        .expect("the native caller receives a refusal, not an unhandled database error");
    let answer = answer_to(&world, thread, &call.call_ref).await;
    assert_eq!(answer.thread_id, thread);
    assert_eq!(
        refused(&answer),
        (
            "unavailable",
            "Orch did not confirm this call's result. A change may have been carried out. Inspect the target before repeating it."
        )
    );
    assert!(
        answer.result.is_none(),
        "an internal failure never claims the capability was granted"
    );
    tokio::time::timeout(common::PATIENCE, engine.answer_tool_call(&call))
        .await
        .expect("replaying the saved refusal does not wait for database recovery")
        .unwrap();
    assert_eq!(
        world.commands.issued_of("orch.tool_result"),
        1,
        "the same call keeps the same protected answer on replay"
    );
}

#[tokio::test]
async fn a_session_is_opened_with_the_tools_the_thread_enables_that_are_built() {
    let world = a_world_whose_agent_takes_tools().await;
    world
        .set(
            world.topic,
            "orch_tools.enabled",
            json!(["announce", "list_shared", "request_capability"]),
        )
        .await;
    let thread = world.thread("Repro").await;
    let open = open_session(&world, thread).await;
    // Only what is enabled is offered, in the fixed order of the tools.
    assert_eq!(
        open.orch_tools,
        vec![
            OrchTool::ListShared,
            OrchTool::Announce,
            OrchTool::RequestCapability
        ]
    );
    assert!(
        open.instructions
            .contains("\n\nOrch's tools: read what your topic permits"),
        "the guide names the tools after the preamble: {}",
        open.instructions
    );
    assert!(
        open.instructions
            .contains("announce tells every thread of your topic")
    );
    assert!(!open.instructions.contains("send_message"));
}

#[tokio::test]
async fn prompts_no_duplicates_a_session_says_the_frames_the_guide_and_where_it_is_once() {
    let world = a_world_whose_agent_takes_tools().await;
    world
        .set(world.topic, "agent.instructions", json!("Answer briefly."))
        .await;
    let thread = world.thread("Repro").await;
    let open = open_session(&world, thread).await;
    let said = |words: &str| open.instructions.matches(words).count();
    assert_eq!(said("Who is speaking\n"), 1, "{}", open.instructions);
    assert_eq!(said("Whose word counts, from most to least\n"), 1);
    assert_eq!(
        said("\n\nOrch's tools: "),
        1,
        "one guide, right after the preamble"
    );
    // The standing instructions stand under the place that gave them, last.
    assert!(
        open.instructions.ends_with("\"\nAnswer briefly.")
            && open.instructions.contains("\n## From the topic \""),
        "{}",
        open.instructions
    );
    let at = |words: &str| open.instructions.find(words).unwrap();
    assert!(at("Who is speaking") < at("Orch's tools: "));
    assert!(at("Orch's tools: ") < at("Answer briefly."));
    // Where the agent is is no part of the session: its first input begins with it, once, with one line on the directory.
    assert_eq!(said("Where you are"), 0);
    let first = world.turn_starts(thread).remove(0);
    assert!(
        first.text.starts_with("<orch>\nWhere you are\n"),
        "{}",
        first.text
    );
    assert_eq!(first.text.matches("Where you are").count(), 1);
    assert_eq!(first.text.matches("\n- Directory ").count(), 1);
    assert_eq!(common::words_after_where(&first.text), "Start");
}

#[tokio::test]
async fn every_tool_that_is_built_is_offered_while_the_thread_enables_all_of_them() {
    let (world, _host, _unit) = a_world_with_all_native_tools().await;
    let thread = world.thread("Repro").await;
    let open = open_session(&world, thread).await;
    let built: Vec<&str> = orch_engine::tools::definitions::BUILT
        .iter()
        .map(|tool| tool.name())
        .collect();
    assert_eq!(
        serde_json::to_value(&open.orch_tools).unwrap(),
        json!(built),
        "as the setting ships, a thread enables them all"
    );
    assert!(
        open.orch_tools.contains(&OrchTool::ManageShared),
        "the tool that manages documents is one of them, and a share to manage is what lets it act"
    );
}

#[tokio::test]
async fn no_tools_are_offered_to_a_harness_that_takes_no_mcp_server() {
    let world = World::new().await;
    let thread = world.thread("Repro").await;
    let open = open_session(&world, thread).await;
    assert!(open.orch_tools.is_empty());
    assert!(!open.instructions.contains("Orch's tools"));
}

#[tokio::test]
async fn no_tools_are_offered_where_the_setting_enables_none_or_the_environment_switches_them_off()
{
    let world = a_world_whose_agent_takes_tools().await;
    world
        .set(world.topic, "orch_tools.enabled", json!([]))
        .await;
    let thread = world.thread("Repro").await;
    assert!(open_session(&world, thread).await.orch_tools.is_empty());

    let other = a_world_whose_agent_takes_tools().await;
    other
        .exec("UPDATE env_templates SET spec = '{\"exec\": {\"orch_tools\": false}}'")
        .await;
    let thread = other.thread("Repro").await;
    assert!(open_session(&other, thread).await.orch_tools.is_empty());
}

// ---------------------------------------------------------------- a call carried and answered

#[tokio::test]
async fn native_tool_calls_use_recorded_active_turns_and_replay_names_are_local_to_their_thread() {
    let world = a_world_whose_agent_takes_tools().await;
    let first = world.thread("First native caller").await;
    let second = world.thread("Second native caller").await;
    let reference = "same-native-reference";
    for (thread, title) in [(first, "First effect"), (second, "Second effect")] {
        say_call(
            &world,
            thread,
            reference,
            OrchTool::Announce,
            json!({"title":title,"category":"milestone"}),
        )
        .await;
        let answer = answer_to(&world, thread, reference).await;
        assert_eq!(answer.thread_id, thread);
        assert!(answer.error.is_none(), "{answer:?}");
        let turn = native_tools::active(&world, thread).await;
        let input: Id = world
            .scalar("SELECT input_id FROM turns WHERE id=$1", turn)
            .await;
        let bound:bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM turns u JOIN thread_inputs i ON i.id=u.input_id
            JOIN segments s ON s.id=u.segment_id WHERE u.id=$1 AND u.thread_id=$2 AND i.id=$3
                AND s.thread_id=u.thread_id AND s.machine_id=$4 AND s.native_session_ref IS NOT NULL)")
            .bind(turn).bind(thread).bind(input).bind(world.machine_id).fetch_one(world.pool.pool()).await.unwrap();
        assert!(
            bound,
            "native ingress receives the recorded turn/input/session owned by this actual machine"
        );
        assert_eq!(
            world
                .count("SELECT count(*) FROM turns WHERE thread_id=$1", thread)
                .await,
            1
        );
    }
    let before = results(&world, first, reference).remove(0);
    say_call(
        &world,
        first,
        reference,
        OrchTool::Announce,
        json!({"title":"First effect","category":"milestone"}),
    )
    .await;
    let replay = answer_to(&world, first, reference).await;
    assert_eq!(replay.result, before.result);
    assert_eq!(results(&world, first, reference).len(), 1);
    assert_eq!(results(&world, second, reference).len(), 1);
    assert_ne!(
        orch_engine::ids::tool_result(first, reference),
        orch_engine::ids::tool_result(second, reference)
    );
    for thread in [first, second] {
        assert_eq!(
            world
                .count(
                    "SELECT count(*) FROM announcements WHERE from_thread_id=$1",
                    thread
                )
                .await,
            1
        );
        assert_eq!(
            world.turn_starts(thread).len(),
            1,
            "a call or replay authors no additional model input"
        );
    }
    let descriptor: Json = world
        .scalar("SELECT descriptor FROM agents WHERE id=$1", world.agent)
        .await;
    assert_eq!(
        descriptor["capabilities"]["orch_tools"], "mcp",
        "native agent.start preserves its declared own tools"
    );
}

#[tokio::test]
async fn a_late_answer_is_resent_after_a_delivery_outage_under_the_same_call_reference_without_a_second_effect()
 {
    use orch_engine::commands::Outcome;
    use orch_testkit::doubles::link::Direction;
    use orch_types::ManualClock;
    use std::sync::Arc;
    let clock = Arc::new(ManualClock::new(orch_testkit::clock::start()));
    let world = World::with_clock(clock.clone()).await;
    native_tools::enable(&world).await;
    native_tools::existing_workspace(&world);
    let thread = world.thread("Retry this answer").await;
    let (turn, _) = world.running_turn(thread, "Work").await;
    // Lose every outbound command until the old 30-second delivery has expired, then restore the link.
    world
        .hub
        .faults()
        .drop_frames(Direction::Down, "cmd", 0, usize::MAX);
    let first_attempt = new_id();
    let mut call = orch_engine::tools::CallContext {
        machine: world.machine_id,
        thread,
        turn: turn.turn_id,
        call_ref: "retry-reference".to_owned(),
        attempt: Some(first_attempt),
        tool: orch_agent_contract::events::OrchTool::Announce,
        arguments: json!({"title":"A saved result","category":"milestone"}),
    };
    world.engine().answer_tool_call(&call).await.unwrap();
    let canonical = orch_engine::ids::tool_result(thread, &call.call_ref);
    let before = world
        .commands
        .as_engine()
        .retained_body(canonical)
        .await
        .unwrap();
    clock.advance(chrono::Duration::seconds(31));
    world.commands.tick(world.machine_id);
    eventually("the old result delivery expired", || async {
        matches!(
            world
                .commands
                .as_engine()
                .status(canonical)
                .await
                .unwrap()
                .and_then(|info| info.outcome),
            Some(Outcome::Expired)
        )
    })
    .await;
    assert!(
        results(&world, thread, &call.call_ref).is_empty(),
        "the connector did not receive the first answer"
    );
    world.hub.faults().clear();
    world.restart_api().await;
    let retry = new_id();
    call.attempt = Some(retry);
    world.engine().answer_tool_call(&call).await.unwrap();
    let answer = answer_to(&world, thread, &call.call_ref).await;
    let original: OrchToolResult = serde_json::from_value(before.unwrap()).unwrap();
    assert_eq!(answer.call_ref, original.call_ref);
    assert_eq!(answer.result, original.result);
    assert_eq!(answer.attempt, Some(retry));
    world.engine().answer_tool_call(&call).await.unwrap();
    common::meanwhile(Duration::from_millis(200)).await;
    assert_eq!(
        results(&world, thread, &call.call_ref).len(),
        1,
        "one result for one retry attempt"
    );
    let effects: i64 = world
        .scalar(
            "SELECT count(*) FROM audit_log WHERE action='announcement.sent' AND node_id=$1",
            world.topic,
        )
        .await;
    assert_eq!(effects, 1, "the retry carries an existing effect's answer");
}

#[tokio::test]
async fn a_call_is_answered_once_with_a_result_however_often_the_harness_says_it() {
    let world = a_world_whose_agent_takes_tools().await;
    let thread = world.thread("Repro").await;
    let follower = world.person("follower").await;
    world.give(follower, "viewer").await;
    let hidden = world.person("hidden").await;
    for person in [follower, hidden] {
        let mut conn = world.pool.acquire().await.unwrap();
        orch_db::queries::hierarchy::put_subscription(
            &mut conn,
            person,
            thread,
            "all",
            "manual",
            world.clock.now(),
        )
        .await
        .unwrap();
    }
    let destination = new_id();
    sqlx::query(
        "INSERT INTO destinations (id, node_id, name, config, level, created_by)
        VALUES ($1, $2, 'Team room', '{\"url\":\"https://hooks.example/team\"}', 'all', $3)",
    )
    .bind(destination)
    .bind(world.topic)
    .bind(world.ada)
    .execute(world.pool.pool())
    .await
    .unwrap();
    let arguments = json!({ "title": "Build is green", "category": "milestone" });
    say_call(
        &world,
        thread,
        "call-1",
        OrchTool::Announce,
        arguments.clone(),
    )
    .await;
    let first = answer_to(&world, thread, "call-1").await;
    say_call(&world, thread, "call-1", OrchTool::Announce, arguments).await;
    common::meanwhile(Duration::from_millis(400)).await;
    assert_eq!(
        results(&world, thread, "call-1").len(),
        1,
        "one answer to one call"
    );
    assert!(first.error.is_none());
    let rows: i64 = world
        .count(
            "SELECT count(*) FROM announcements WHERE from_thread_id = $1",
            thread,
        )
        .await;
    assert_eq!(rows, 1, "the call made one announcement");
    let notices: Vec<Id> = sqlx::query_scalar(
        "SELECT user_id FROM notifications WHERE thread_id = $1 AND kind = 'milestone'",
    )
    .bind(thread)
    .fetch_all(world.pool.pool())
    .await
    .unwrap();
    assert_eq!(
        notices.iter().filter(|id| **id == follower).count(),
        1,
        "the replay tells the follower once"
    );
    assert!(
        !notices.contains(&hidden),
        "following a hidden thread grants no notification access"
    );
    let posted = world
        .count(
            "SELECT count(*) FROM outbox WHERE destination_id = $1",
            destination,
        )
        .await;
    assert_eq!(posted, 1, "the same event reaches its eligible room once");
}

#[tokio::test]
async fn a_refusal_is_a_result_marked_as_an_error_that_names_its_code_and_what_to_do() {
    let world = a_world_whose_agent_takes_tools().await;
    let thread = world.thread("Repro").await;
    let result = call(
        &world,
        thread,
        OrchTool::Announce,
        json!({ "body": "no title" }),
    )
    .await;
    let (code, message) = refused(&result);
    assert_eq!(code, "invalid");
    assert!(message.contains("\"title\" is required"), "{message}");
    assert!(
        message.contains("It takes: title (required), body, category"),
        "{message}"
    );

    let unknown = call(
        &world,
        thread,
        OrchTool::Announce,
        json!({ "title": "x", "audience": "everyone" }),
    )
    .await;
    let (code, message) = refused(&unknown);
    assert_eq!(code, "invalid");
    assert!(
        message.contains("one of: followers, topic, project"),
        "{message}"
    );

    let stray = call(
        &world,
        thread,
        OrchTool::Announce,
        json!({ "title": "x", "colour": "red" }),
    )
    .await;
    assert!(
        refused(&stray)
            .1
            .contains("\"colour\" is not an argument of announce")
    );

    let long = call(
        &world,
        thread,
        OrchTool::Announce,
        json!({ "title": "t".repeat(121) }),
    )
    .await;
    assert!(refused(&long).1.contains("1 to 120 characters"));
}

#[tokio::test]
async fn a_tool_the_thread_does_not_have_is_refused_and_the_tools_it_has_are_named() {
    let world = a_world_whose_agent_takes_tools().await;
    world
        .set(world.topic, "orch_tools.enabled", json!(["announce"]))
        .await;
    let thread = world.thread("Repro").await;
    let result = call(
        &world,
        thread,
        OrchTool::SendMessage,
        json!({ "target": { "kind": "thread", "path": "Other" }, "text": "hi" }),
    )
    .await;
    let (code, message) = refused(&result);
    assert_eq!(code, "invalid");
    assert!(
        message.contains("send_message is not a tool you have here"),
        "{message}"
    );
    assert!(message.contains("You have: announce."), "{message}");
}

#[tokio::test]
async fn an_agent_that_calls_more_than_a_minute_allows_is_told_how_long_to_wait() {
    let world = a_world_whose_agent_takes_tools().await;
    world
        .set(world.topic, "orch_tools.calls_per_minute", json!(2))
        .await;
    let thread = world.thread("Repro").await;
    for title in ["one", "two"] {
        let result = call(
            &world,
            thread,
            OrchTool::Announce,
            json!({ "title": title, "audience": "followers" }),
        )
        .await;
        assert!(result.error.is_none(), "{result:?}");
    }
    let third = call(
        &world,
        thread,
        OrchTool::Announce,
        json!({ "title": "three", "audience": "followers" }),
    )
    .await;
    let (code, message) = refused(&third);
    assert_eq!(code, "rate_limited");
    assert!(message.contains("Wait "), "{message}");
    // Another thread's agent has a rate of its own.
    let other = world.thread("Other").await;
    let fine = call(
        &world,
        other,
        OrchTool::Announce,
        json!({ "title": "mine", "audience": "followers" }),
    )
    .await;
    assert!(fine.error.is_none());
}

// ---------------------------------------------------------------- announce

fn delivered(result: &OrchToolResult) -> Json {
    result.result.clone().expect("a result")["delivered"].clone()
}

#[tokio::test]
async fn announce_bound_agent_walk_accepts_1500_and_refuses_1501_without_a_second_effect() {
    let world = a_world_whose_agent_takes_tools().await;
    let sender = world.thread("Build").await;
    let receivers = [world.thread("Review").await, world.thread("Check").await];
    let original = format!("<orch>{}", "界".repeat(1494));
    assert_eq!(original.chars().count(), 1500);
    let arguments = json!({"title":"A finding","body":original});
    say_call(
        &world,
        sender,
        "body-bound",
        OrchTool::Announce,
        arguments.clone(),
    )
    .await;
    let first = answer_to(&world, sender, "body-bound").await;
    assert!(first.error.is_none(), "{first:?}");
    assert_eq!(delivered(&first)["deferred"], 2);
    say_call(&world, sender, "body-bound", OrchTool::Announce, arguments).await;
    common::meanwhile(Duration::from_millis(400)).await;
    assert_eq!(results(&world, sender, "body-bound").len(), 1);
    let refused_call = call(
        &world,
        sender,
        OrchTool::Announce,
        json!({"title":"Too long","body":format!("{original}界")}),
    )
    .await;
    assert_eq!(refused(&refused_call).0, "invalid");
    assert_eq!(
        world
            .count(
                "SELECT count(*) FROM announcements WHERE from_thread_id = $1",
                sender
            )
            .await,
        1,
    );
    let stored: String = world
        .scalar(
            "SELECT body FROM announcements WHERE from_thread_id = $1",
            sender,
        )
        .await;
    assert_eq!(stored, original);
    for receiver in receivers {
        assert_eq!(
            world
                .count(
                    "SELECT count(*) FROM items WHERE thread_id = $1 AND kind = 'announcement'",
                    receiver
                )
                .await,
            1,
        );
        let pending: Json = world
            .scalar(
                "SELECT pending_notices FROM threads WHERE node_id = $1",
                receiver,
            )
            .await;
        assert_eq!(pending.as_array().unwrap().len(), 1);
        world
            .engine()
            .submit_input(&Caller::session(world.ada), receiver, queued("Review it"))
            .await
            .unwrap();
        let delivered = world.run_turn(receiver, 0, "Reviewed").await;
        assert!(
            delivered
                .text
                .contains("<relayed from-agent=\"Build\" kind=\"announcement\" at=\"")
        );
        assert!(
            delivered
                .text
                .contains(&orch_engine::instructions::escape_body(&original))
        );
        assert!(delivered.text.ends_with("Review it"));
        let pending: Json = world
            .scalar(
                "SELECT pending_notices FROM threads WHERE node_id = $1",
                receiver,
            )
            .await;
        assert_eq!(pending, json!([]));
    }
}

#[tokio::test]
async fn a_notice_goes_into_every_other_thread_of_the_topic_as_a_line_and_a_note() {
    let world = a_world_whose_agent_takes_tools().await;
    let (sender, idle, other) = (
        world.thread("Build").await,
        world.thread("Validate").await,
        world.thread("Docs").await,
    );
    let result = call(
        &world,
        sender,
        OrchTool::Announce,
        json!({ "title": "The interface changed", "body": "login() takes a realm now" }),
    )
    .await;
    let body = result.result.expect("a result");
    assert_eq!(body["category"], "notice");
    assert_eq!(body["audience"], "topic");
    assert_eq!(body["applied"]["delivery"], "steer");
    assert_eq!(body["applied"]["fallback"], "next_turn");
    assert_eq!(body["applied"]["wake_idle"], false);
    // Idle threads, with the topic as shipped, are told at their next turn.
    assert_eq!(
        body["delivered"],
        json!({ "interrupted": 0, "steered": 0, "queued": 0, "deferred": 2 })
    );
    for thread in [sender, idle, other] {
        let lines: i64 = world
            .count(
                "SELECT count(*) FROM items WHERE thread_id = $1 AND kind = 'announcement'",
                thread,
            )
            .await;
        assert_eq!(
            lines, 1,
            "every thread of the topic, the sender's own, has the line"
        );
    }
    let notices: Json = world
        .scalar(
            "SELECT pending_notices FROM threads WHERE node_id = $1",
            idle,
        )
        .await;
    assert_eq!(notices.as_array().map(Vec::len), Some(1));
    let nobody: Json = world
        .scalar(
            "SELECT pending_notices FROM threads WHERE node_id = $1",
            sender,
        )
        .await;
    assert_eq!(
        nobody,
        json!([]),
        "the sender is told nothing it said itself"
    );
}

#[tokio::test]
async fn a_milestone_goes_to_the_followers_alone_and_enters_no_thread_nor_the_rate() {
    let world = a_world_whose_agent_takes_tools().await;
    let (sender, other) = (world.thread("Build").await, world.thread("Validate").await);
    let result = call(
        &world,
        sender,
        OrchTool::Announce,
        json!({ "title": "Build is green", "category": "milestone" }),
    )
    .await;
    let body = result.result.expect("a result");
    assert_eq!(body["audience"], "followers");
    assert_eq!(body["applied"], Json::Null);
    assert_eq!(body["delivered"], Json::Null);
    let lines: i64 = world
        .count(
            "SELECT count(*) FROM items WHERE thread_id = $1 AND kind = 'announcement'",
            other,
        )
        .await;
    assert_eq!(lines, 0, "no other thread has a line of it");
    let targets: String = world
        .scalar(
            "SELECT targets FROM announcements WHERE from_thread_id = $1",
            sender,
        )
        .await;
    assert_eq!(targets, "none");
    // Not counted against the hour: with a rate of one, a notice still goes after any number of milestones.
    world
        .set(world.topic, "broadcast.rate_per_hour", json!(1))
        .await;
    for title in ["one", "two"] {
        call(
            &world,
            sender,
            OrchTool::Announce,
            json!({ "title": title, "category": "milestone" }),
        )
        .await;
    }
    let notice = call(
        &world,
        sender,
        OrchTool::Announce,
        json!({ "title": "A notice" }),
    )
    .await;
    assert!(notice.error.is_none(), "{notice:?}");
}

#[tokio::test]
async fn a_milestone_raised_to_the_topic_is_delivered_like_a_notice_with_its_own_first_words() {
    let world = a_world_whose_agent_takes_tools().await;
    let (sender, other) = (world.thread("Build").await, world.thread("Validate").await);
    let result = call(
        &world,
        sender,
        OrchTool::Announce,
        json!({ "title": "Build is done", "category": "milestone", "audience": "topic" }),
    )
    .await;
    assert_eq!(delivered(&result)["deferred"], 1);
    let notices: Json = world
        .scalar(
            "SELECT pending_notices FROM threads WHERE node_id = $1",
            other,
        )
        .await;
    assert_eq!(notices[0]["title"], "Build is done");
}

#[tokio::test]
async fn an_announcement_reaches_a_thread_by_what_it_is_doing_and_what_the_topic_says() {
    let world = a_world_whose_agent_takes_tools().await;
    world
        .set(
            world.topic,
            "broadcast.agent_options",
            json!(["interrupt", "next_turn", "wake_idle"]),
        )
        .await;
    let sender = world.thread("Build").await;
    let (working, idle, stuck) = (
        world.thread("Working").await,
        world.thread("Idle").await,
        world.thread("Stuck").await,
    );
    let (turn, _) = world.running_turn(working, "Go on").await;
    world
        .exec(&format!(
            "UPDATE threads SET state = 'blocked', blocked_reason = 'error' WHERE node_id = '{stuck}'"
        ))
        .await;
    // As shipped, a running agent that can be steered is steered, an idle one and a blocked one are told later.
    let steered = call(
        &world,
        sender,
        OrchTool::Announce,
        json!({ "title": "Heads up" }),
    )
    .await;
    assert_eq!(
        delivered(&steered),
        json!({ "interrupted": 0, "steered": 1, "queued": 0, "deferred": 2 })
    );
    world.wait_ran(CommandKind::TurnSteer, 1).await;
    let Command::TurnSteer(steer) = world.seen.of_kind(CommandKind::TurnSteer).remove(0) else {
        panic!("a steer");
    };
    assert_eq!(steer.turn_id, turn.turn_id);
    assert!(
        steer
            .text
            .starts_with("<relayed from-agent=\"Build\" kind=\"announcement\" at=\""),
        "{}",
        steer.text
    );
    assert!(steer.text.contains("\nHeads up\n"));
    assert!(!steer.text.contains("Do not re-announce it."));

    // The agent chooses: waiting for the next turn puts it first in the queue of a thread at work.
    let later = call(
        &world,
        sender,
        OrchTool::Announce,
        json!({ "title": "Later", "delivery": "next_turn" }),
    )
    .await;
    assert_eq!(delivered(&later)["queued"], 1);
    let waiting: i64 = world
        .count(
            "SELECT count(*) FROM thread_inputs WHERE thread_id = $1 AND source = 'announcement' AND state = 'pending'",
            working,
        )
        .await;
    assert_eq!(waiting, 1);

    // It can wake an idle thread, and then a turn starts there.
    let woken = call(
        &world,
        sender,
        OrchTool::Announce,
        json!({ "title": "Wake up", "wake_idle": true }),
    )
    .await;
    assert_eq!(
        delivered(&woken),
        json!({ "interrupted": 0, "steered": 1, "queued": 1, "deferred": 1 }),
        "the working thread is steered again, the idle one is started, the blocked one is told later"
    );
    world.wait_state(idle, "starting").await;
    let _ = stuck;
}

#[tokio::test]
async fn an_agent_chooses_its_delivery_only_where_the_topic_lists_the_choice() {
    let world = a_world_whose_agent_takes_tools().await;
    let sender = world.thread("Build").await;
    let _working = world.thread("Working").await;
    // A stale or forged override is refused before any thread is disturbed.
    let result = call(
        &world,
        sender,
        OrchTool::Announce,
        json!({ "title": "Stop everything", "delivery": "interrupt", "wake_idle": true }),
    )
    .await;
    let (code, message) = refused(&result);
    assert_eq!(code, "invalid");
    assert!(message.contains("broadcast.agent_options"));
    assert!(result.result.is_none());
}

#[tokio::test]
async fn an_agent_interrupts_a_running_turn_where_the_topic_lets_it_and_the_turn_is_stopped() {
    let world = a_world_whose_agent_takes_tools().await;
    world
        .set(world.topic, "broadcast.agent_options", json!(["interrupt"]))
        .await;
    let sender = world.thread("Build").await;
    let working = world.thread("Working").await;
    let (turn, _) = world.running_turn(working, "Go on").await;
    let result = call(
        &world,
        sender,
        OrchTool::Announce,
        json!({ "title": "Stop", "delivery": "interrupt" }),
    )
    .await;
    assert_eq!(delivered(&result)["interrupted"], 1);
    world.wait_ran(CommandKind::TurnInterrupt, 1).await;
    world
        .say(working, turn_ended(turn.turn_id, TurnOutcome::Interrupted))
        .await;
    // The announcement is first in its queue, so the next turn starts with it.
    let start = world.turn_start(working, 1).await;
    assert!(
        start
            .text
            .contains("<relayed from-agent=\"Build\" kind=\"announcement\" at=\""),
        "{}",
        start.text
    );
}

#[tokio::test]
async fn an_audience_of_the_project_is_refused_while_the_topic_keeps_agents_to_it() {
    let world = a_world_whose_agent_takes_tools().await;
    let sender = world.thread("Build").await;
    let result = call(
        &world,
        sender,
        OrchTool::Announce,
        json!({ "title": "To all", "audience": "project" }),
    )
    .await;
    let (code, message) = refused(&result);
    assert_eq!(code, "invalid");
    assert!(message.contains("broadcast.agent_scope"), "{message}");

    world
        .set(world.root, "broadcast.agent_scope", json!("project"))
        .await;
    let other_topic = world.topic("Elsewhere").await;
    let _ = other_topic;
    let allowed = call(
        &world,
        sender,
        OrchTool::Announce,
        json!({ "title": "To all", "audience": "project" }),
    )
    .await;
    assert!(allowed.error.is_none(), "{allowed:?}");
    assert_eq!(allowed.result.expect("a result")["audience"], "project");
}

#[tokio::test]
async fn what_an_agent_sends_into_other_threads_has_a_rate_in_the_hour_and_the_refusal_says_when() {
    let world = a_world_whose_agent_takes_tools().await;
    world
        .set(world.topic, "broadcast.rate_per_hour", json!(2))
        .await;
    let sender = world.thread("Build").await;
    let _other = world.thread("Validate").await;
    for title in ["one", "two"] {
        let sent = call(
            &world,
            sender,
            OrchTool::Announce,
            json!({ "title": title }),
        )
        .await;
        assert!(sent.error.is_none(), "{sent:?}");
    }
    // A message to a single thread counts with the announcements.
    let third = call(
        &world,
        sender,
        OrchTool::SendMessage,
        json!({ "target": { "kind": "thread", "path": "Validate" }, "text": "where is the report?" }),
    )
    .await;
    let (code, message) = refused(&third);
    assert_eq!(code, "rate_limited");
    assert!(message.contains("seconds"), "{message}");
    // Milestones for the followers are not counted: the agent can still mark one.
    let milestone = call(
        &world,
        sender,
        OrchTool::Announce,
        json!({ "title": "Done", "category": "milestone" }),
    )
    .await;
    assert!(milestone.error.is_none());
}

#[tokio::test]
async fn a_zero_rate_switches_what_agents_send_into_other_threads_off() {
    let world = a_world_whose_agent_takes_tools().await;
    world
        .set(world.topic, "broadcast.rate_per_hour", json!(0))
        .await;
    let sender = world.thread("Build").await;
    let result = call(&world, sender, OrchTool::Announce, json!({ "title": "x" })).await;
    let (code, message) = refused(&result);
    assert_eq!(code, "limit");
    assert!(message.contains("broadcast.rate_per_hour"));
}

#[tokio::test]
async fn what_waited_for_a_thread_is_said_first_at_its_next_turn_and_only_once() {
    let world = a_world_whose_agent_takes_tools().await;
    let (sender, idle) = (world.thread("Build").await, world.thread("Validate").await);
    call(
        &world,
        sender,
        OrchTool::Announce,
        json!({ "title": "The build moved", "body": "It is in ./out now" }),
    )
    .await;
    world
        .engine()
        .submit_input(&Caller::session(world.ada), idle, queued("Please validate"))
        .await
        .unwrap();
    let first = world.turn_start(idle, 0).await;
    assert!(
        common::words_after_where(&first.text)
            .starts_with("<relayed from-agent=\"Build\" kind=\"announcement\" at=\""),
        "{}",
        first.text
    );
    assert!(first.text.contains("from-agent=\"Build\""));
    assert!(first.text.contains("The build moved\nIt is in ./out now"));
    assert!(first.text.ends_with("Please validate"));
    world.answer(idle, &first, "m0", "Fine").await;
    world
        .say(idle, turn_ended(first.turn_id, TurnOutcome::Completed))
        .await;
    world.wait_turn(first.turn_id, "completed").await;
    let notices: Json = world
        .scalar(
            "SELECT pending_notices FROM threads WHERE node_id = $1",
            idle,
        )
        .await;
    assert_eq!(notices, json!([]), "said, and forgotten");
    world
        .engine()
        .submit_input(&Caller::session(world.ada), idle, queued("Again"))
        .await
        .unwrap();
    let second = world.turn_start(idle, 1).await;
    assert_eq!(common::words_after_where(&second.text), "Again");
}

// ---------------------------------------------------------------- send_message

#[tokio::test]
async fn a_message_reaches_the_thread_of_the_topic_called_by_that_name_as_a_note_from_the_sender() {
    let world = a_world_whose_agent_takes_tools().await;
    let (sender, receiver) = (world.thread("Build").await, world.thread("Validate").await);
    let listed = call(&world, sender, OrchTool::ListShared, json!({}))
        .await
        .result
        .unwrap();
    let path = listed["entries"]
        .as_array()
        .unwrap()
        .iter()
        .find(|entry| entry["name"] == "Validate")
        .unwrap()["path"]
        .clone();
    let result = call(
        &world,
        sender,
        OrchTool::SendMessage,
        json!({ "target": { "kind": "thread", "path": "Validate" }, "text": "The report is in ./out/report.txt" }),
    )
    .await;
    let body = result.result.expect("a result");
    assert_eq!(body["target"], json!({"kind":"thread", "path":path}));
    assert_eq!(body["status"], "queued");
    assert_eq!(
        body["delivery"], "deferred",
        "as shipped an idle thread is told at its next turn"
    );
    assert_eq!(body["files"], json!([]));
    let (title, text, to): (String, String, Option<Id>) = sqlx::query_as(
        "SELECT title, body, to_thread_id FROM announcements WHERE from_thread_id = $1",
    )
    .bind(sender)
    .fetch_one(world.pool.pool())
    .await
    .unwrap();
    assert_eq!(title, "", "a message has no title");
    assert_eq!(text, "The report is in ./out/report.txt");
    assert_eq!(to, Some(receiver));
    let line: Json = world
        .scalar(
            "SELECT payload FROM items WHERE thread_id = $1 AND kind = 'announcement'",
            receiver,
        )
        .await;
    assert_eq!(
        line["to_thread"], true,
        "the receiving thread shows it as a message from that thread"
    );
    assert_eq!(line["from"]["thread"]["name"], "Build");
}

#[tokio::test]
async fn a_message_to_a_running_agent_is_steered_into_its_turn_with_the_senders_name() {
    let world = a_world_whose_agent_takes_tools().await;
    let (sender, receiver) = (world.thread("Build").await, world.thread("Validate").await);
    let _ = world.running_turn(receiver, "Working").await;
    let result = call(
        &world,
        sender,
        OrchTool::SendMessage,
        json!({ "target": { "kind": "thread", "path": "Validate" }, "text": "Skip the slow tests" }),
    )
    .await;
    let body = result.result.expect("a result");
    assert_eq!(body["delivery"], "steered");
    assert_eq!(
        body["status"], "queued",
        "a scheduled steer is not a harness acknowledgement"
    );
    world.wait_ran(CommandKind::TurnSteer, 1).await;
    let Command::TurnSteer(steer) = world.seen.of_kind(CommandKind::TurnSteer).remove(0) else {
        panic!("a steer");
    };
    assert!(
        steer
            .text
            .starts_with("<relayed from-agent=\"Build\" kind=\"message\" at=\"")
    );
    assert!(steer.text.ends_with("\nSkip the slow tests\n</relayed>"));
}

#[tokio::test]
async fn a_message_to_a_thread_nobody_has_is_answered_with_the_names_that_can_be_addressed() {
    let world = a_world_whose_agent_takes_tools().await;
    let sender = world.thread("Build").await;
    let receiver = world.thread("Validate").await;
    let result = call(
        &world,
        sender,
        OrchTool::SendMessage,
        json!({ "target": { "kind": "thread", "path": "Valdate" }, "text": "hello" }),
    )
    .await;
    let (code, message) = refused(&result);
    assert_eq!(code, "not_found");
    assert!(message.contains("\"Validate\""), "{message}");
    // The address is a shared path: its recovery listing names the reachable
    // directory entries. Being listed does not make a target a legal recipient.
    assert!(message.contains("\"Build\""), "{message}");
    assert!(message.contains("\"docs\""), "{message}");
    let own = call(
        &world,
        sender,
        OrchTool::SendMessage,
        json!({ "target": { "kind": "thread", "path": "Build" }, "text": "hello" }),
    )
    .await;
    assert_eq!(refused(&own).0, "invalid");
    let directory = call(
        &world,
        sender,
        OrchTool::SendMessage,
        json!({ "target": { "kind": "thread", "path": "docs" }, "text": "hello" }),
    )
    .await;
    assert_eq!(refused(&directory).0, "wrong_kind");
    let none = call(
        &world,
        sender,
        OrchTool::SendMessage,
        json!({ "target": { "kind": "thread", "path": "Validate" }}),
    )
    .await;
    assert!(refused(&none).1.contains("Give text, files, or both"));
    assert_eq!(
        world
            .count(
                "SELECT count(*) FROM announcements WHERE from_thread_id=$1",
                sender
            )
            .await,
        0,
        "a misspelled, self, directory or empty target creates no message"
    );
    let corrected = call(
        &world,
        sender,
        OrchTool::SendMessage,
        json!({"target":{"kind":"thread","path":"Validate"},"text":"hello"}),
    )
    .await;
    assert_eq!(corrected.result.as_ref().unwrap()["status"], "queued");
    assert_eq!(
        world
            .count(
                "SELECT count(*) FROM announcements WHERE from_thread_id=$1",
                sender
            )
            .await,
        1
    );
    let addressed: Option<Id> = world
        .scalar(
            "SELECT to_thread_id FROM announcements WHERE from_thread_id=$1",
            sender,
        )
        .await;
    assert_eq!(addressed, Some(receiver));
    assert_eq!(
        world
            .count(
                "SELECT count(*) FROM items WHERE thread_id=$1 AND kind='announcement'",
                receiver
            )
            .await,
        1
    );
}

#[tokio::test]
async fn a_message_cannot_carry_a_file_from_outside_the_sending_workspace() {
    let world = a_world_whose_agent_takes_tools().await;
    let sender = world.thread("Build").await;
    let _ = world.thread("Validate").await;
    let result = call(
        &world,
        sender,
        OrchTool::SendMessage,
        json!({ "target": { "kind": "thread", "path": "Validate" }, "text": "see", "files": ["../private/report.txt"] }),
    )
    .await;
    let (code, message) = refused(&result);
    assert_eq!(code, "invalid");
    assert!(message.contains("relative path"), "{message}");
    assert_eq!(
        world
            .count(
                "SELECT count(*) FROM announcements WHERE from_thread_id = $1",
                sender
            )
            .await,
        0
    );
    // The world keeps no record of the file commands it does not answer itself:
    // what the engine asked for counts.
    assert_eq!(world.commands.issued_of("fs.fetch"), 0);
}

#[tokio::test]
async fn a_message_is_audited_as_sent_by_the_agent_and_made_once_however_often_the_call_is_made() {
    let world = a_world_whose_agent_takes_tools().await;
    let (sender, _receiver) = (world.thread("Build").await, world.thread("Validate").await);
    let arguments = json!({ "target": { "kind": "thread", "path": "Validate" }, "text": "once" });
    say_call(
        &world,
        sender,
        "same",
        OrchTool::SendMessage,
        arguments.clone(),
    )
    .await;
    let _ = answer_to(&world, sender, "same").await;
    say_call(&world, sender, "same", OrchTool::SendMessage, arguments).await;
    common::meanwhile(Duration::from_millis(400)).await;
    let rows: i64 = world
        .count(
            "SELECT count(*) FROM announcements WHERE from_thread_id = $1",
            sender,
        )
        .await;
    assert_eq!(rows, 1);
    let audited: i64 = world
        .count(
            "SELECT count(*) FROM audit_log WHERE actor_id = $1 AND action = 'thread.sent_by_agent'",
            sender,
        )
        .await;
    assert_eq!(audited, 1);
}

#[tokio::test]
async fn announcement_schema_follows_the_place_and_refreshes_only_before_the_next_turn() {
    let world = a_world_whose_agent_takes_tools().await;
    world
        .set(world.topic, "orch_tools.enabled", json!(["announce"]))
        .await;
    let thread = world.thread("Announce choices").await;
    let (running, _) = world.running_turn(thread, "Begin").await;
    let first = world
        .seen
        .of_kind(CommandKind::SessionOpen)
        .into_iter()
        .find_map(|command| match command {
            Command::SessionOpen(open) if open.thread_id == thread => Some(open),
            _ => None,
        })
        .unwrap();
    let schema = &first.orch_tool_schemas["announce"];
    assert_eq!(
        schema["properties"]["audience"]["enum"],
        json!(["followers", "topic"])
    );
    assert!(schema["properties"].get("delivery").is_none());
    assert!(schema["properties"].get("wake_idle").is_none());
    let before = world.seen.of_kind(CommandKind::SessionOpen).len();
    world
        .set(world.project, "broadcast.agent_scope", json!("project"))
        .await;
    world
        .set(
            world.topic,
            "broadcast.agent_options",
            json!(["steer", "wake_idle"]),
        )
        .await;
    world
        .engine()
        .submit_input(
            &Caller::session(world.ada),
            thread,
            common::steering("Keep working"),
        )
        .await
        .unwrap();
    world.wait_ran(CommandKind::TurnSteer, 1).await;
    assert_eq!(
        world.seen.of_kind(CommandKind::SessionOpen).len(),
        before,
        "a schema change starts no hidden work or session while a turn runs"
    );
    world
        .say(thread, turn_ended(running.turn_id, TurnOutcome::Completed))
        .await;
    world.wait_state(thread, "idle").await;
    world
        .engine()
        .submit_input(&Caller::session(world.ada), thread, queued("Go on"))
        .await
        .unwrap();
    world.turn_start(thread, 1).await;
    let last = world
        .seen
        .of_kind(CommandKind::SessionOpen)
        .into_iter()
        .rev()
        .find_map(|command| match command {
            Command::SessionOpen(open) if open.thread_id == thread => Some(open),
            _ => None,
        })
        .unwrap();
    assert_eq!(
        last.orch_tool_schemas["announce"]["properties"]["audience"]["enum"],
        json!(["followers", "topic", "project"])
    );
    assert_eq!(
        last.orch_tool_schemas["announce"]["properties"]["delivery"]["enum"],
        json!(["steer"])
    );
    assert!(
        last.orch_tool_schemas["announce"]["properties"]
            .get("wake_idle")
            .is_some()
    );
    assert_eq!(
        world.seen.of_kind(CommandKind::TurnStart).len(),
        2,
        "only the two authored queued inputs start turns"
    );
}

// ---------------------------------------------------------------- the definitions

#[tokio::test]
async fn the_listing_the_connector_gives_a_harness_is_what_the_engine_declares() {
    // The connector lists the tools from a file beside its listener, made from the engine's definitions with their
    // shipped texts: the two must not part, or a harness would be taught something the system does not answer.
    let given: Json = serde_json::from_str(include_str!(
        "../../../runtime/connector/src/orch_tools.json"
    ))
    .unwrap();
    let declared: Vec<Json> =
        orch_engine::tools::definitions::all(&std::collections::BTreeMap::new())
            .iter()
            .map(orch_engine::tools::definitions::ToolDefinition::to_mcp)
            .collect();
    assert_eq!(given, Json::Array(declared));
}

#[tokio::test]
async fn a_place_replaces_the_description_of_a_tool_by_a_text_of_its_own() {
    let world = World::new().await;
    world
        .set(
            world.topic,
            "agent.messages",
            json!({ "tool_announce": "Tell the others on the team, rarely." }),
        )
        .await;
    let thread = world.thread("Repro").await;
    let mut conn = world.pool.acquire().await.unwrap();
    let settings = orch_engine::tools::ToolSettings::load(&mut conn, thread)
        .await
        .unwrap();
    let announce = orch_engine::tools::definitions::definition(
        orch_agent_contract::events::OrchTool::Announce,
        &settings.messages,
    );
    assert_eq!(announce.description, "Tell the others on the team, rarely.");
    let list = orch_engine::tools::definitions::definition(
        orch_agent_contract::events::OrchTool::ListShared,
        &settings.messages,
    );
    assert!(
        list.description
            .starts_with("List what the team shared with you")
    );
}
