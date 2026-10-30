//! A plugin kept: pinged, restarted with back-off, attached to, logged apart and stopped, with real programs.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]

mod support;

use std::collections::BTreeMap;
use std::process::Stdio;
use std::time::Instant;

use orch_plugin::requests::CallError;
use orch_plugin::supervisor::Plugin;
use orch_plugin_contract::registration::Restart;
use orch_plugin_contract::{PluginKind, PluginRegistration, PluginState};
use orch_types::{Clock, Json, ManualClock};
use serde_json::json;
use support::{
    PYTHON, Work, advance, alive, capture_logs, clock, logged_by, options, registration, script,
    scripted, settle, wait_until,
};
use tokio::process::{Child, Command};

fn summarizer_script(responses: &Json) -> Json {
    script(
        PluginKind::Summarizer,
        &["describe", "summarize"],
        responses,
    )
}

async fn start(
    work: &Work,
    clock: &ManualClock,
    args: &[&str],
    responses: &Json,
    restart: Option<Restart>,
) -> Plugin {
    let mut entry = registration(
        PluginKind::Summarizer,
        "rules",
        work,
        &summarizer_script(responses),
        args,
    );
    entry.restart = restart;
    Plugin::start(&entry, options(clock)).await.unwrap()
}

#[tokio::test]
async fn a_plugin_that_hangs_is_pinged_declared_dead_and_started_again_after_a_pause() {
    let (clock, work) = (clock(), Work::new());
    let plugin = start(&work, &clock, &["--no-pong"], &json!({}), None).await;
    plugin.settled().await;
    assert_eq!(plugin.report().state, PluginState::Running);

    advance(&clock, 29);
    settle().await;
    assert!(
        work.received("ping").is_empty(),
        "thirty seconds of silence are needed"
    );
    advance(&clock, 1);
    wait_until("the plugin was pinged", || work.received("ping").len() == 1).await;
    assert_eq!(
        plugin.report().state,
        PluginState::Running,
        "a ping is not yet a verdict"
    );

    advance(&clock, 9);
    settle().await;
    assert_eq!(plugin.report().state, PluginState::Running);
    assert_eq!(work.received("ping").len(), 1, "one ping at a time");
    advance(&clock, 1);
    wait_until("the plugin was declared dead", || {
        plugin.report().state == PluginState::Restarting && plugin.report().retry_at.is_some()
    })
    .await;
    assert!(
        plugin
            .report()
            .detail
            .unwrap()
            .contains("did not answer a ping")
    );
    wait_until("the hung program was ended", || !alive(work.pids()[0])).await;

    advance(&clock, 1);
    wait_until("it was started again", || plugin.report().starts == 2).await;
    wait_until("it runs again", || {
        plugin.report().state == PluginState::Running
    })
    .await;
    plugin.shutdown().await;
}

#[tokio::test]
async fn a_plugin_that_answers_pings_is_left_alone_however_long_it_is_quiet() {
    let (clock, work) = (clock(), Work::new());
    let plugin = start(&work, &clock, &[], &json!({}), None).await;
    plugin.settled().await;
    for _ in 0..5 {
        advance(&clock, 31);
        let pings = work.received("ping").len();
        wait_until("a ping was answered", || {
            work.received("ping").len() > pings
        })
        .await;
        settle().await;
    }
    assert_eq!(plugin.report().state, PluginState::Running);
    assert_eq!(plugin.report().starts, 1);
    plugin.shutdown().await;
}

#[tokio::test]
async fn a_plugin_that_exits_in_the_middle_of_a_request_fails_it_and_serves_the_next_after_its_restart()
 {
    let (clock, work) = (clock(), Work::new());
    let marker = work.text("exited-once");
    let plugin = start(
        &work,
        &clock,
        &["--exit-on", "create", "--once-marker", &marker],
        &json!({ "create": { "result": { "unit": "u1" } } }),
        None,
    )
    .await;
    plugin.settled().await;

    let failed = plugin.call("create", json!({})).await.unwrap_err();
    let CallError::Lost {
        plugin: name,
        method,
        reason,
    } = &failed
    else {
        panic!("a request lost with the plugin, not {failed:?}");
    };
    assert_eq!((name.as_str(), method.as_str()), ("rules", "create"));
    assert!(reason.contains("exit status: 3"), "{reason}");
    assert!(failed.retryable(), "the next attempt may find it back");

    wait_until("it is waiting to be restarted", || {
        plugin.report().retry_at.is_some()
    })
    .await;
    assert_eq!(plugin.report().state, PluginState::Restarting);
    assert!(
        matches!(
            plugin.call("create", json!({})).await,
            Err(CallError::Unavailable { .. })
        ),
        "while it is down, whatever depends on it is unavailable under its name"
    );
    advance(&clock, 1);
    wait_until("it runs again", || {
        plugin.report().state == PluginState::Running
    })
    .await;
    assert_eq!(plugin.report().starts, 2);
    assert_eq!(
        plugin.call("create", json!({})).await.unwrap(),
        json!({ "unit": "u1" }),
        "the next request is served"
    );
    plugin.shutdown().await;
}

#[tokio::test]
async fn a_plugin_that_keeps_dying_is_started_again_after_pauses_that_double_up_to_a_minute() {
    let (clock, work) = (clock(), Work::new());
    let plugin = start(&work, &clock, &["--exit-after-hello"], &json!({}), None).await;
    for (attempt, pause) in [1i64, 2, 4, 8, 16, 32, 60, 60].into_iter().enumerate() {
        let starts = u64::try_from(attempt).unwrap() + 1;
        wait_until("a start failed", || {
            let report = plugin.report();
            report.starts == starts && report.retry_at.is_some()
        })
        .await;
        let failed_at = clock.time();
        assert_eq!(
            plugin
                .report()
                .retry_at
                .map(|at| at.unix_millis() - failed_at.unix_millis()),
            Some(pause * 1000),
            "the pause after failure number {starts}"
        );
        advance(&clock, pause - 1);
        settle().await;
        assert_eq!(
            plugin.report().starts,
            starts,
            "{pause} s have not passed yet"
        );
        advance(&clock, 1);
        wait_until("it was started again", || {
            plugin.report().starts == starts + 1
        })
        .await;
    }
    plugin.shutdown().await;
}

#[tokio::test]
async fn the_pauses_start_over_after_a_session_that_lasted_as_long_as_the_longest_of_them() {
    let (clock, work) = (clock(), Work::new());
    let plugin = start(
        &work,
        &clock,
        &["--exit-after-hello-until", "2", "--exit-on", "die"],
        &json!({}),
        None,
    )
    .await;
    // The first two starts fail at once: pauses of one and of two seconds.
    wait_until("the first start failed", || {
        plugin.report().retry_at.is_some()
    })
    .await;
    advance(&clock, 1);
    wait_until("the second start failed", || {
        plugin.report().starts == 2 && plugin.report().retry_at.is_some()
    })
    .await;
    advance(&clock, 2);
    wait_until("the third start runs", || {
        plugin.report().state == PluginState::Running && plugin.report().starts == 3
    })
    .await;

    // It runs for more than a minute, and then dies: the pause is the first again, not the third.
    advance(&clock, 61);
    settle().await;
    assert!(plugin.call("die", json!({})).await.is_err());
    wait_until("it is waiting to be restarted", || {
        plugin.report().retry_at.is_some()
    })
    .await;
    let waited = plugin.report().retry_at.unwrap().unix_millis() - clock.time().unix_millis();
    assert_eq!(waited, 1000);
    plugin.shutdown().await;
}

#[tokio::test]
async fn a_restart_policy_decides_whether_a_program_that_is_gone_is_started_again() {
    // `never` leaves it down, whatever it ended with.
    let (clock, work) = (clock(), Work::new());
    let never = start(
        &work,
        &clock,
        &["--exit-after-hello"],
        &json!({}),
        Some(Restart::Never),
    )
    .await;
    wait_until("it is down for good", || {
        never.report().state == PluginState::Failed
    })
    .await;
    advance(&clock, 3600);
    settle().await;
    assert_eq!((never.report().starts, work.pids().len()), (1, 1));
    never.shutdown().await;

    // `on-failure`, the default, starts it again unless it ended by itself with success.
    let (clock, work) = (clock_again(), Work::new());
    let clean = start(
        &work,
        &clock,
        &["--exit-after-hello", "--exit-code", "0"],
        &json!({}),
        None,
    )
    .await;
    wait_until("it is down for good", || {
        clean.report().state == PluginState::Failed
    })
    .await;
    assert!(
        clean
            .report()
            .detail
            .unwrap()
            .contains("exited by itself with success")
    );
    advance(&clock, 3600);
    settle().await;
    assert_eq!(clean.report().starts, 1);
    clean.shutdown().await;

    // `always` starts it again whatever it ended with.
    let (clock, work) = (clock_again(), Work::new());
    let always = start(
        &work,
        &clock,
        &["--exit-after-hello", "--exit-code", "0"],
        &json!({}),
        Some(Restart::Always),
    )
    .await;
    wait_until("it is waiting to be restarted", || {
        always.report().retry_at.is_some()
    })
    .await;
    advance(&clock, 1);
    wait_until("it was started again", || always.report().starts == 2).await;
    always.shutdown().await;
}

fn clock_again() -> ManualClock {
    clock()
}

/// A scripted plugin that serves a socket, ended when this is dropped.
struct Service {
    _child: Child,
    address: String,
}

async fn serve(work: &Work, listen: &str, args: &[&str]) -> Service {
    let ready = work.path("ready");
    let child = Command::new(PYTHON)
        .arg(scripted())
        .args([
            "--script",
            &work.script(&summarizer_script(&json!({
                "create": { "result": { "unit": "u1" } }
            }))),
        ])
        .args(["--log", &work.text("log.jsonl"), "--listen", listen])
        .args(["--ready", &ready.to_string_lossy()])
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .kill_on_drop(true)
        .spawn()
        .unwrap();
    wait_until("the service listens", || ready.exists()).await;
    let written: Json = serde_json::from_str(&std::fs::read_to_string(&ready).unwrap()).unwrap();
    let address = match written.get("port") {
        Some(port) => format!("tcp://127.0.0.1:{port}"),
        None => format!("unix:{}", written["path"].as_str().unwrap()),
    };
    Service {
        _child: child,
        address,
    }
}

fn attach(name: &str, address: &str) -> PluginRegistration {
    PluginRegistration {
        kind: PluginKind::Summarizer,
        name: name.to_owned(),
        command: None,
        connect: Some(address.to_owned()),
        env: BTreeMap::new(),
        restart: None,
    }
}

#[tokio::test]
async fn a_plugin_is_attached_to_over_a_socket_in_the_file_system_and_over_the_loopback_address() {
    for listen in ["unix:SOCKET", "tcp:0"] {
        let (clock, work) = (clock(), Work::new());
        let listen = listen.replace("SOCKET", &work.text("plugin.sock"));
        let service = serve(&work, &listen, &[]).await;
        let plugin = Plugin::start(&attach("rules", &service.address), options(&clock))
            .await
            .unwrap();
        plugin.settled().await;
        assert_eq!(plugin.report().state, PluginState::Running, "{listen}");
        assert_eq!(plugin.report().version, "0.3.0");
        assert_eq!(
            plugin.call("create", json!({})).await.unwrap(),
            json!({ "unit": "u1" })
        );
        plugin.shutdown().await;
    }
}

#[tokio::test]
async fn an_attached_plugin_that_disconnects_is_attached_to_again_and_the_service_is_never_told_to_stop()
 {
    let (clock, work) = (clock(), Work::new());
    let marker = work.text("closed-once");
    let service = serve(
        &work,
        &format!("unix:{}", work.text("plugin.sock")),
        &["--exit-on", "create", "--once-marker", &marker],
    )
    .await;
    let plugin = Plugin::start(&attach("rules", &service.address), options(&clock))
        .await
        .unwrap();
    plugin.settled().await;

    let lost = plugin.call("create", json!({})).await.unwrap_err();
    assert!(matches!(lost, CallError::Lost { .. }), "{lost:?}");
    wait_until("it waits to be attached to again", || {
        plugin.report().retry_at.is_some()
    })
    .await;
    advance(&clock, 1);
    wait_until("it runs again", || {
        plugin.report().state == PluginState::Running
    })
    .await;
    assert_eq!(plugin.report().starts, 2);
    assert_eq!(
        plugin.call("create", json!({})).await.unwrap(),
        json!({ "unit": "u1" })
    );
    let accepted = work
        .records()
        .iter()
        .filter(|record| record.get("accepted").is_some())
        .count();
    assert_eq!(accepted, 2, "one service, two connections");

    // A host that stops lets the service go: the service is its operator's, and is not told to exit.
    let pid = work.pids()[0];
    plugin.shutdown().await;
    settle().await;
    assert!(alive(pid), "the service goes on");
    assert!(work.received("shutdown").is_empty());
}

#[tokio::test]
async fn an_attached_plugin_that_is_not_there_yet_is_attached_to_when_it_is() {
    let (clock, work) = (clock(), Work::new());
    let socket = work.text("later.sock");
    let plugin = Plugin::start(&attach("rules", &format!("unix:{socket}")), options(&clock))
        .await
        .unwrap();
    wait_until("the first attempt failed", || {
        plugin.report().retry_at.is_some()
    })
    .await;
    assert!(
        plugin
            .report()
            .detail
            .unwrap()
            .contains("could not be attached to")
    );
    let _service = serve(&work, &format!("unix:{socket}"), &[]).await;
    advance(&clock, 1);
    wait_until("it was attached to", || {
        plugin.report().state == PluginState::Running
    })
    .await;
    plugin.shutdown().await;
}

#[tokio::test]
async fn a_started_program_is_told_to_shut_down_and_is_gone_when_the_host_has_stopped() {
    let (clock, work) = (clock(), Work::new());
    let plugin = start(&work, &clock, &[], &json!({}), None).await;
    plugin.settled().await;
    let pid = work.pids()[0];
    assert!(alive(pid));
    plugin.shutdown().await;
    assert_eq!(work.received("shutdown").len(), 1, "it was told to finish");
    assert!(!alive(pid), "and it did");
    let report = plugin.report();
    assert_eq!(report.state, PluginState::Failed);
    assert!(report.detail.unwrap().contains("stopped by the host"));
    assert!(
        matches!(
            plugin.call("summarize", json!({})).await,
            Err(CallError::Unavailable { .. })
        ),
        "nothing is asked of a plugin that is stopped"
    );
    plugin.shutdown().await;
}

#[tokio::test]
async fn a_program_that_does_not_exit_after_shutdown_is_ended_when_ten_seconds_have_passed() {
    let (clock, work) = (clock(), Work::new());
    let plugin = start(&work, &clock, &["--shutdown-ignore"], &json!({}), None).await;
    plugin.settled().await;
    let pid = work.pids()[0];
    let stopping = tokio::spawn({
        let plugin = plugin.clone();
        async move { plugin.shutdown().await }
    });
    wait_until("it was told", || work.received("shutdown").len() == 1).await;
    advance(&clock, 9);
    settle().await;
    assert!(!stopping.is_finished(), "ten seconds are given");
    assert!(alive(pid));
    advance(&clock, 1);
    stopping.await.unwrap();
    assert!(!alive(pid), "after them the program is ended");
}

#[tokio::test]
async fn a_program_is_ended_when_the_last_handle_of_its_plugin_is_gone() {
    let (clock, work) = (clock(), Work::new());
    let plugin = start(&work, &clock, &[], &json!({}), None).await;
    plugin.settled().await;
    let pid = work.pids()[0];
    drop(plugin);
    wait_until("the program was told to finish", || !alive(pid)).await;
}

#[tokio::test]
async fn what_a_program_writes_to_its_standard_error_is_its_log_and_a_flood_of_it_changes_nothing()
{
    capture_logs();
    let (clock, work) = (clock(), Work::new());
    let plugin = start(
        &work,
        &clock,
        &[
            "--stderr-line",
            "listening on nothing in particular",
            "--flood-stderr",
        ],
        &json!({ "echo": { "result": { "n": { "$params": "n" } } } }),
        None,
    )
    .await;
    plugin.settled().await;
    let began = Instant::now();
    // The plugin keeps writing as fast as it can; its requests are answered all the same.
    for n in 0..50 {
        assert_eq!(
            plugin.call("echo", json!({ "n": n })).await.unwrap()["n"],
            n
        );
    }
    wait_until("the flood was held back in the log", || {
        logged_by("rules").iter().any(|line| {
            line.fields
                .get("message")
                .is_some_and(|m| m.contains("were not written"))
        })
    })
    .await;
    let lines = logged_by("rules");
    let theirs: Vec<_> = lines
        .iter()
        .filter(|line| line.target == "orch_plugin::log" && line.level == tracing::Level::INFO)
        .collect();
    assert!(
        theirs
            .iter()
            .any(|line| line.fields["message"] == "listening on nothing in particular"),
        "the program's own words, apart from the host's"
    );
    let seconds = usize::try_from(began.elapsed().as_secs()).unwrap() + 2;
    assert!(
        theirs.len() <= orch_plugin::transport::LOG_LINES_PER_SECOND as usize * seconds,
        "{} lines of the log in {seconds} s",
        theirs.len()
    );
    assert!(
        lines.iter().all(|line| line.target != "orch_plugin::log"
            || line
                .fields
                .get("kind")
                .is_some_and(|kind| kind == "summarizer")),
        "each line says what kind of plugin it comes from"
    );
    assert_eq!(plugin.report().state, PluginState::Running);
    plugin.shutdown().await;
}

#[tokio::test]
async fn what_a_program_wrote_just_before_it_died_reaches_the_log() {
    capture_logs();
    let (clock, work) = (clock(), Work::new());
    let entry = registration(
        PluginKind::Summarizer,
        "dying",
        &work,
        &summarizer_script(&json!({})),
        &[
            "--stderr-line",
            "the last words of the program",
            "--exit-after-hello",
        ],
    );
    let plugin = Plugin::start(&entry, options(&clock)).await.unwrap();
    wait_until("it died", || plugin.report().retry_at.is_some()).await;
    wait_until("its last words were written", || {
        logged_by("dying").iter().any(|line| {
            line.target == "orch_plugin::log"
                && line
                    .fields
                    .get("message")
                    .is_some_and(|m| m == "the last words of the program")
        })
    })
    .await;
    plugin.shutdown().await;
}

#[tokio::test]
async fn a_program_that_cannot_be_started_is_a_failure_that_is_retried_unless_the_registration_says_never()
 {
    let clock = clock();
    let missing = |restart: Option<Restart>| PluginRegistration {
        kind: PluginKind::Summarizer,
        name: "missing".into(),
        command: Some(vec!["/nonexistent/orch-plugin".into()]),
        connect: None,
        env: BTreeMap::new(),
        restart,
    };
    let retried = Plugin::start(&missing(None), options(&clock))
        .await
        .unwrap();
    wait_until("the first start failed", || {
        retried.report().retry_at.is_some()
    })
    .await;
    let report = retried.report();
    assert_eq!(
        (report.state, report.starts),
        (PluginState::Restarting, 0),
        "it never started"
    );
    assert!(report.detail.unwrap().contains("could not be started"));
    advance(&clock, 1);
    wait_until("it was tried again", || {
        retried
            .report()
            .retry_at
            .is_some_and(|at| at.unix_millis() > clock.time().unix_millis())
    })
    .await;
    retried.shutdown().await;

    let given_up = Plugin::start(&missing(Some(Restart::Never)), options(&clock))
        .await
        .unwrap();
    given_up.settled().await;
    assert_eq!(given_up.report().state, PluginState::Failed);
    given_up.shutdown().await;
}

#[tokio::test]
async fn a_program_that_closes_its_output_but_goes_on_running_is_ended_and_started_again() {
    let (clock, work) = (clock(), Work::new());
    let plugin = start(
        &work,
        &clock,
        &["--close-output-after-hello"],
        &json!({}),
        None,
    )
    .await;
    wait_until("the closed output was noticed", || {
        plugin.report().retry_at.is_some()
    })
    .await;
    let report = plugin.report();
    assert!(report.detail.unwrap().contains("closed its output"));
    wait_until("the program was ended", || !alive(work.pids()[0])).await;
    plugin.shutdown().await;
}

#[tokio::test]
async fn a_host_that_stops_while_a_plugin_is_still_saying_hello_ends_it_at_once() {
    let (clock, work) = (clock(), Work::new());
    let plugin = start_without_waiting(&work, &clock, &["--silent-hello"]).await;
    wait_until("the program started", || !work.pids().is_empty()).await;
    let pid = work.pids()[0];
    plugin.shutdown().await;
    assert!(!alive(pid));
    assert_eq!(plugin.report().state, PluginState::Failed);
}

async fn start_without_waiting(work: &Work, clock: &ManualClock, args: &[&str]) -> Plugin {
    let entry = registration(
        PluginKind::Summarizer,
        "rules",
        work,
        &summarizer_script(&json!({})),
        args,
    );
    Plugin::start(&entry, options(clock)).await.unwrap()
}

#[tokio::test]
async fn a_request_that_is_still_waiting_when_the_host_stops_fails_as_lost_and_nothing_hangs() {
    let (clock, work) = (clock(), Work::new());
    let plugin = start(
        &work,
        &clock,
        &[],
        &json!({ "work": { "result": {}, "delay_ms": 60_000 } }),
        None,
    )
    .await;
    plugin.settled().await;
    let call = plugin.start_call("work", json!({})).unwrap();
    wait_until("the plugin has the request", || {
        !work.received("work").is_empty()
    })
    .await;
    plugin.shutdown().await;
    let failed = call.await.unwrap_err();
    assert!(matches!(failed, CallError::Lost { .. }), "{failed:?}");
    assert!(failed.retryable());
}
