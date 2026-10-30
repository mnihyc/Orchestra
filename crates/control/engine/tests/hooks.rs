//! A host's report of its operator's hook runs: a line of the history of the environment its unit serves, heard only from the runtime's own host, holding the moment, why and how it ended and nothing the hook printed.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]

mod common;
mod hosting;

use common::{World, eventually};
use hosting::{ScriptedHost, serve, template};
use orch_driver_contract::runtime::UnitReason;
use orch_link_contract::commands::CommandKind;
use orch_link_contract::events::{Event, HookEvent, NodeHook};
use orch_link_contract::streams::Stream;
use orch_types::Json;

const LINES: &str =
    "SELECT count(*) FROM audit_log WHERE action = 'unit.hook_ran' AND environment_id = $1";

#[tokio::test]
async fn a_hook_run_its_own_host_reports_is_a_line_of_the_environments_history_and_a_foreign_report_is_not()
 {
    let world = World::new().await;
    serve(&world);
    let host = ScriptedHost::on(&world, "hooked-node", 16_384).await;
    let template = template(&world).await;
    let made = hosting::make(&world, template, "Hooked").await;
    host.wait_for(CommandKind::NodeRuntimeStart, 1).await;
    let runtime = host.started().unwrap();
    let run = Event::NodeHook(NodeHook {
        runtime_id: runtime,
        event: HookEvent::PreUp,
        reason: UnitReason::Recreate,
        replay: false,
        at: "2026-10-14T15:00:00.000Z".parse().unwrap(),
        duration_ms: 42,
        exit: Some(3),
        timed_out: false,
        failed: true,
    });
    // Another host's node cannot write into this environment's history.
    let foreign = ScriptedHost::on(&world, "other-node", 16_384).await;
    foreign.added.machine.publish(&Stream::Node, &run).await;
    eventually("the foreign report to be taken in", || async {
        world.commands.acknowledged(foreign.machine, &Stream::Node) >= 1
    })
    .await;
    assert_eq!(world.count(LINES, made.id).await, 0);
    // Its own host's node can, once for each run.
    host.added.machine.publish(&Stream::Node, &run).await;
    eventually("the run to be written", || async {
        world.count(LINES, made.id).await >= 1
    })
    .await;
    let details: Json = world
        .scalar(
            "SELECT details FROM audit_log WHERE action = 'unit.hook_ran' AND environment_id = $1",
            made.id,
        )
        .await;
    assert_eq!(details["event"], "pre_up");
    assert_eq!(details["reason"], "recreate");
    assert_eq!(details["exit"], 3);
    assert_eq!(details["failed"], true);
    let summary: String = world
        .scalar(
            "SELECT summary FROM audit_log WHERE action = 'unit.hook_ran' AND environment_id = $1",
            made.id,
        )
        .await;
    assert!(
        summary.contains("pre_up") && summary.contains("exit 3"),
        "{summary}"
    );
    let history: bool = world
        .scalar(
            "SELECT history FROM audit_log WHERE action = 'unit.hook_ran' AND environment_id = $1",
            made.id,
        )
        .await;
    assert!(history, "it is a line of the environment's history");
    world.engine().stop();
}
