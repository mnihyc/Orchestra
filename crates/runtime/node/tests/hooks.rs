//! The operator's lifecycle hooks as the node runs them for its units, on the in-memory hub with the scripted controller and the in-memory driver: what each is told, an up hook that keeps a unit from running, down hooks that never hold a stop, the run again at a node's start, and what the system is told of each run.
#![cfg(unix)]
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]

mod common;

use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::time::Duration;

use common::Rig;
use orch_driver_contract::runtime::{RuntimeKind, SubjectEnvironment, UnitReason, UnitSubject};
use orch_driver_contract::units::UnitStateKind;
use orch_link::commands::Progress;
use orch_link_contract::commands::{NodeRuntimeStop, SerialKey};
use orch_node_core::hooks::HookSettings;
use orch_types::Json;
use serde_json::json;

/// A hook that keeps what it was told, one document a line, and the moment it ran at, then does `then`.
fn hook(dir: &Path, event: &str, then: &str) -> PathBuf {
    let path = dir.join(event);
    let told = dir.join(format!("{event}.jsonl"));
    let order = dir.join("order.txt");
    std::fs::write(
        &path,
        format!(
            "#!/bin/sh\n(cat; echo) >> '{}'\necho {event} >> '{}'\n{then}\n",
            told.display(),
            order.display()
        ),
    )
    .unwrap();
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
    path
}

/// The documents a hook was given, in order.
fn told(dir: &Path, event: &str) -> Vec<Json> {
    std::fs::read_to_string(dir.join(format!("{event}.jsonl")))
        .unwrap_or_default()
        .lines()
        .filter(|line| !line.trim().is_empty())
        .map(|line| serde_json::from_str(line).unwrap())
        .collect()
}

/// The moments the hooks ran at, in order.
fn order(dir: &Path) -> Vec<String> {
    std::fs::read_to_string(dir.join("order.txt"))
        .unwrap_or_default()
        .lines()
        .map(str::to_owned)
        .collect()
}

/// A rig whose node runs these hooks, each of which only records, except where `then` says otherwise.
fn rig_with(dir: &Path, then: &[(&str, &str)], timeout: Duration) -> Rig {
    let mut rig = Rig::new();
    let program = |event: &str| {
        let more = then
            .iter()
            .find(|(named, _)| *named == event)
            .map_or("exit 0", |(_, more)| *more);
        Some(hook(dir, event, more))
    };
    rig.settings.hooks = HookSettings {
        pre_up: program("pre_up"),
        post_up: program("post_up"),
        pre_down: program("pre_down"),
        post_down: program("post_down"),
        timeout,
    };
    rig.have_connector();
    rig
}

/// An environment's request that says what it serves and why it is made.
fn about(
    rig: &Rig,
    label: &str,
    made_for: UnitReason,
) -> orch_driver_contract::runtime::RuntimeSpec {
    let mut spec = rig.spec(RuntimeKind::Environment, label);
    spec.subject = Some(UnitSubject {
        made_for,
        environment: Some(SubjectEnvironment {
            id: orch_types::new_id(),
            name: "payments".to_owned(),
            path: "Acme / Platform / Payments".to_owned(),
            template_id: None,
            template: "Default".to_owned(),
            generation: 2,
        }),
        provider: None,
    });
    spec
}

/// The `node.hook` reports of one runtime, in order.
fn reported(rig: &Rig, runtime: orch_types::Id) -> Vec<Json> {
    rig.applied()
        .into_iter()
        .filter(|event| event.kind == "node.hook" && event.body["runtime_id"] == json!(runtime))
        .map(|event| event.body)
        .collect()
}

#[tokio::test]
async fn a_unit_comes_up_between_its_up_hooks_and_each_hook_is_told_what_the_unit_serves_and_why() {
    let dir = tempfile::tempdir().unwrap();
    let mut rig = rig_with(dir.path(), &[], Duration::from_secs(10));
    rig.start().await;
    let spec = about(&rig, "hooked", UnitReason::Recreate);
    rig.made(&spec).await;
    assert!(matches!(
        rig.start_runtime(spec.runtime_id).await,
        Progress::Succeeded(_)
    ));
    assert_eq!(order(dir.path()), ["pre_up", "post_up"]);
    assert_eq!(rig.driver.state_of(&spec.name), UnitStateKind::Running);
    let first = &told(dir.path(), "pre_up")[0];
    assert_eq!(first["event"], "pre_up");
    assert_eq!(
        first["reason"], "recreate",
        "a unit's first start is told why it was made"
    );
    assert_eq!(first["replay"], false);
    assert_eq!(first["runtime"]["id"], json!(spec.runtime_id));
    assert_eq!(first["runtime"]["kind"], "environment");
    assert_eq!(first["runtime"]["driver"], "memory");
    assert_eq!(first["unit"]["name"], spec.name);
    assert_eq!(first["environment"]["name"], "payments");
    assert_eq!(first["environment"]["path"], "Acme / Platform / Payments");
    assert_eq!(first["environment"]["generation"], 2);
    assert_eq!(first["network"]["egress"], "open");
    assert_eq!(first["network"]["mode"], "bridge");
    assert_eq!(first["ports"], json!([]));
    assert_eq!(first["host"]["system"], common::SYSTEM);
    assert!(
        first.get("enroll_token").is_none() && !first.to_string().contains("BEGIN CERTIFICATE")
    );

    // A stop is told its reason; the down hooks run around it.
    let stopped = rig
        .send(
            &SerialKey::Runtime(spec.runtime_id),
            &NodeRuntimeStop {
                runtime_id: spec.runtime_id,
                reason: Some(UnitReason::Move),
            },
        )
        .await;
    assert!(matches!(stopped, Progress::Succeeded(_)), "{stopped:?}");
    assert_eq!(
        order(dir.path()),
        ["pre_up", "post_up", "pre_down", "post_down"]
    );
    assert_eq!(told(dir.path(), "pre_down")[0]["reason"], "move");
    assert_eq!(told(dir.path(), "post_down")[0]["reason"], "move");

    // A later start is an ordinary one.
    assert!(matches!(
        rig.start_runtime(spec.runtime_id).await,
        Progress::Succeeded(_)
    ));
    assert_eq!(told(dir.path(), "pre_up")[1]["reason"], "start");

    // The system is told each run, and nothing a hook printed.
    rig.wait_told("six hook runs", |events| {
        events
            .iter()
            .filter(|event| event.kind == "node.hook")
            .count()
            >= 6
    })
    .await;
    let runs = reported(&rig, spec.runtime_id);
    let moments: Vec<(&str, &str)> = runs
        .iter()
        .map(|run| {
            (
                run["event"].as_str().unwrap(),
                run["reason"].as_str().unwrap(),
            )
        })
        .collect();
    assert_eq!(
        moments,
        [
            ("pre_up", "recreate"),
            ("post_up", "recreate"),
            ("pre_down", "move"),
            ("post_down", "move"),
            ("pre_up", "start"),
            ("post_up", "start"),
        ]
    );
    assert!(
        runs.iter()
            .all(|run| run["failed"] == false && run["exit"] == 0)
    );
}

#[tokio::test]
async fn a_failing_pre_up_keeps_the_unit_from_starting_and_the_start_says_what_the_hook_printed_first()
 {
    let dir = tempfile::tempdir().unwrap();
    let mut rig = rig_with(
        dir.path(),
        &[("pre_up", "echo 'the tunnel wg0 is down' >&2; exit 4")],
        Duration::from_secs(10),
    );
    rig.start().await;
    let spec = about(&rig, "closed", UnitReason::Start);
    rig.made(&spec).await;
    let Progress::Failed(error) = rig.start_runtime(spec.runtime_id).await else {
        panic!("the start went on past a failing pre_up");
    };
    assert_eq!(error.code, "hook.failed");
    assert!(
        error.detail.contains("pre_up hook exited with 4"),
        "{}",
        error.detail
    );
    assert!(
        error.detail.contains("the tunnel wg0 is down"),
        "{}",
        error.detail
    );
    assert_ne!(rig.driver.state_of(&spec.name), UnitStateKind::Running);
    assert_eq!(
        order(dir.path()),
        ["pre_up"],
        "nothing ran after a failed pre_up"
    );
    rig.wait_told("the failed run", |events| {
        events.iter().any(|event| event.kind == "node.hook")
    })
    .await;
    let run = &reported(&rig, spec.runtime_id)[0];
    assert_eq!(
        (
            run["event"].as_str(),
            run["exit"].as_i64(),
            run["failed"].as_bool()
        ),
        (Some("pre_up"), Some(4), Some(true))
    );
    assert!(
        !run.to_string().contains("wg0"),
        "the system is not told what the hook printed"
    );
}

#[tokio::test]
async fn a_failing_post_up_stops_the_unit_again_with_its_down_hooks_and_fails_the_start() {
    let dir = tempfile::tempdir().unwrap();
    let mut rig = rig_with(
        dir.path(),
        &[("post_up", "echo 'no route was added' >&2; exit 1")],
        Duration::from_secs(10),
    );
    rig.start().await;
    let spec = about(&rig, "reclosed", UnitReason::Start);
    rig.made(&spec).await;
    let Progress::Failed(error) = rig.start_runtime(spec.runtime_id).await else {
        panic!("a unit whose post_up failed was left running");
    };
    assert_eq!(error.code, "hook.failed");
    assert!(
        error.detail.contains("no route was added"),
        "{}",
        error.detail
    );
    assert_ne!(rig.driver.state_of(&spec.name), UnitStateKind::Running);
    assert_eq!(
        order(dir.path()),
        ["pre_up", "post_up", "pre_down", "post_down"]
    );
}

#[tokio::test]
async fn a_down_hook_that_fails_or_runs_out_of_time_never_holds_a_stop() {
    let dir = tempfile::tempdir().unwrap();
    let mut rig = rig_with(
        dir.path(),
        &[("pre_down", "sleep 30"), ("post_down", "exit 7")],
        Duration::from_secs(1),
    );
    rig.start().await;
    let spec = about(&rig, "down", UnitReason::Start);
    rig.made(&spec).await;
    assert!(matches!(
        rig.start_runtime(spec.runtime_id).await,
        Progress::Succeeded(_)
    ));
    let began = std::time::Instant::now();
    assert!(matches!(
        rig.stop_runtime(spec.runtime_id).await,
        Progress::Succeeded(_)
    ));
    assert!(
        began.elapsed() < Duration::from_secs(15),
        "the stop waited for its hook"
    );
    assert_eq!(rig.driver.state_of(&spec.name), UnitStateKind::Stopped);
    rig.wait_told("the down runs", |events| {
        events
            .iter()
            .filter(|event| event.kind == "node.hook")
            .count()
            >= 4
    })
    .await;
    let runs = reported(&rig, spec.runtime_id);
    let pre_down = runs.iter().find(|run| run["event"] == "pre_down").unwrap();
    assert_eq!(
        (
            pre_down["timed_out"].as_bool(),
            pre_down["failed"].as_bool()
        ),
        (Some(true), Some(true))
    );
    let post_down = runs.iter().find(|run| run["event"] == "post_down").unwrap();
    assert_eq!(post_down["exit"], 7);
}

#[tokio::test]
async fn a_starting_node_runs_post_up_again_for_a_unit_it_finds_running_and_post_down_for_one_that_ended()
 {
    let dir = tempfile::tempdir().unwrap();
    let mut rig = rig_with(dir.path(), &[], Duration::from_secs(10));
    rig.start().await;
    let running = about(&rig, "still-running", UnitReason::Start);
    let ending = about(&rig, "ended-away", UnitReason::Start);
    for spec in [&running, &ending] {
        rig.made(spec).await;
        assert!(matches!(
            rig.start_runtime(spec.runtime_id).await,
            Progress::Succeeded(_)
        ));
    }
    rig.stop().await;
    rig.driver
        .crash(&ending.name, 137, "killed while its node was away");
    rig.start().await;
    let replayed = |dir: &Path, event: &str| {
        told(dir, event)
            .into_iter()
            .filter(|document| document["replay"] == true)
            .collect::<Vec<_>>()
    };
    // The ended unit's post_down comes from the replay or from the first look after the start, whichever has the
    // unit first; it comes once.
    let ended = |dir: &Path| {
        told(dir, "post_down")
            .into_iter()
            .filter(|document| document["runtime"]["id"] == json!(ending.runtime_id))
            .collect::<Vec<_>>()
    };
    let deadline = tokio::time::Instant::now() + common::PATIENCE;
    while (replayed(dir.path(), "post_up").is_empty() || ended(dir.path()).is_empty())
        && tokio::time::Instant::now() < deadline
    {
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    let up = replayed(dir.path(), "post_up");
    assert_eq!(up.len(), 1, "{up:?}");
    assert_eq!(up[0]["runtime"]["id"], json!(running.runtime_id));
    assert_eq!(up[0]["reason"], "start");
    tokio::time::sleep(Duration::from_millis(500)).await;
    let down = ended(dir.path());
    assert_eq!(down.len(), 1, "{down:?}");
    assert_eq!(down[0]["reason"], "stop");
    assert!(
        replayed(dir.path(), "pre_up").is_empty(),
        "an up hook's moment before the start is not replayed"
    );
}

#[tokio::test]
async fn a_unit_that_came_up_and_ended_by_itself_is_given_its_post_down_at_the_next_look() {
    let dir = tempfile::tempdir().unwrap();
    let mut rig = rig_with(dir.path(), &[], Duration::from_secs(10));
    rig.start().await;
    let spec = about(&rig, "crashed", UnitReason::Start);
    rig.made(&spec).await;
    assert!(matches!(
        rig.start_runtime(spec.runtime_id).await,
        Progress::Succeeded(_)
    ));
    rig.driver.crash(&spec.name, 1, "it fell over");
    rig.role().tick().await;
    let down = told(dir.path(), "post_down");
    assert_eq!(down.len(), 1, "{down:?}");
    assert_eq!(down[0]["reason"], "stop");
    rig.role().tick().await;
    assert_eq!(
        told(dir.path(), "post_down").len(),
        1,
        "post_down is given once"
    );
}
