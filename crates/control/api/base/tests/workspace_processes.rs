//! The processes of an environment and what it used lately, over HTTP: every process listed with the caller's own thread's first, a signal that names the process by its number and the moment it started, and the measurements the machine sent in the last hours; each needing the right on the thread (or on the environment) and a machine that is there.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]

mod places;
mod support;
mod workspace;

use chrono::Duration;
use orch_api_contract::common::ErrorCode;
use orch_api_contract::environment::ThroughThreadQuery;
use orch_api_contract::processes::{
    EnvironmentProcessList, EnvironmentProcessListReply, EnvironmentProcessSignal,
    EnvironmentProcessSignalBody, EnvironmentResourceList, EnvironmentResourceListQuery,
    ProcessSignal, RESOURCE_MINUTES_MAX, ResourceSample,
};
use orch_api_core::backplane::Report;
use orch_api_core::testing::Reply;
use orch_link_contract::commands::{CommandKind, Signal};
use orch_link_contract::streams::Eph;
use orch_types::{Id, Json, Time};
use places::Who;
use serde_json::json;
use workspace::Site;

const STARTED: &str = "2026-10-08T17:04:05.123Z";

fn process(pid: u32, name: &str, thread: Option<Id>) -> Json {
    json!({
        "pid": pid, "ppid": 1, "name": name, "command": format!("{name} --serve"), "user": "agent",
        "state": "S", "cpu_percent": 1.5, "memory_bytes": 1_048_576, "started_at": STARTED,
        "thread_id": thread, "item_id": null
    })
}

async fn live_site() -> Site {
    let mut site = Site::new().await;
    *site.disk.processes.lock().unwrap() = vec![
        process(40, "postgres", None),
        process(41, "vite", Some(site.thread)),
        process(42, "cargo", None),
    ];
    site.power_on();
    site
}

async fn list(site: &Site, by: &Who) -> Reply<EnvironmentProcessListReply> {
    site.app()
        .call(EnvironmentProcessList, &[&site.environment])
        .token(&by.token)
        .query(&ThroughThreadQuery {
            thread: Some(site.thread),
        })
        .send()
        .await
}

async fn signal(site: &Site, by: &Who, pid: u32, how: ProcessSignal, started: &str) -> Reply<()> {
    site.app()
        .call(EnvironmentProcessSignal, &[&site.environment, &pid])
        .token(&by.token)
        .query(&ThroughThreadQuery {
            thread: Some(site.thread),
        })
        .body(&EnvironmentProcessSignalBody {
            signal: how,
            started_at: started.parse().unwrap(),
        })
        .send()
        .await
}

fn sample(at: chrono::DateTime<chrono::Utc>, cpu: f64) -> ResourceSample {
    ResourceSample {
        at: Time::new(at),
        cpu_percent: cpu,
        cpu_limit_millis: Some(2_000),
        memory_bytes: 512 * 1024 * 1024,
        memory_limit_bytes: None,
        disk_used_bytes: Some(10),
        disk_limit_bytes: None,
        net_rx_bps: 5,
        net_tx_bps: 6,
        pids: 3,
    }
}

async fn resources(site: &Site, by: &Who, minutes: Option<u32>) -> Reply<Vec<ResourceSample>> {
    site.app()
        .call(EnvironmentResourceList, &[&site.environment])
        .token(&by.token)
        .query(&EnvironmentResourceListQuery {
            thread: Some(site.thread),
            minutes,
        })
        .send()
        .await
}

/// A measurement a machine sent for the environment, as the link hands it to the API.
fn report(site: &Site, sample: &ResourceSample) -> Report {
    Report {
        machine_id: site.machine,
        eph: Eph {
            stream: format!("env:{}", site.environment),
            kind: "stats.sample".to_owned(),
            at: sample.at,
            body: serde_json::to_value(sample).unwrap(),
        },
    }
}

/// These HTTP history cases inject admitted samples; the socket suite separately proves that
/// subscribing starts the native producer and the last unsubscribe stops it.
fn admitted_samples(site: &Site) {
    site.app().app.cache.watching_on(
        site.environment,
        orch_api_core::cache::Watching {
            machine: site.machine,
            session: site.app().app.link.session_of(site.machine),
        },
    );
}

// ---------------------------------------------------------------- the list

#[tokio::test]
async fn every_process_of_the_environment_is_listed_with_the_threads_own_first() {
    let site = live_site().await;
    let listed = list(&site, &site.vic).await.ok();
    let pids: Vec<_> = listed.processes.iter().map(|process| process.pid).collect();
    assert_eq!(pids, [41, 40, 42], "the thread's own first, then by number");
    assert_eq!(listed.processes[0].thread_id, Some(site.thread));
    assert_eq!(listed.processes[1].thread_id, None);
    assert_eq!(
        listed.as_of.get().to_rfc3339(),
        "2026-10-08T17:04:05.123+00:00"
    );
}

#[tokio::test]
async fn processes_are_listed_to_whoever_may_see_them_and_only_while_the_machine_is_there() {
    let site = live_site().await;
    let stranger = list(&site, &site.eve).await;
    assert!(!stranger.status.is_success());
    assert_eq!(site.sent(CommandKind::ProcList), 0);
    site.power_off().await;
    assert_eq!(
        list(&site, &site.ada).await.problem().code,
        ErrorCode::SourceOffline
    );
}

// ---------------------------------------------------------------- signals

#[tokio::test]
async fn a_process_is_sent_a_signal_by_its_number_and_the_moment_it_started() {
    let site = live_site().await;
    signal(&site, &site.ada, 41, ProcessSignal::Term, STARTED)
        .await
        .ok();
    assert_eq!(site.disk.signals(), [(41, Signal::Term)]);
    signal(&site, &site.dev, 42, ProcessSignal::Kill, STARTED)
        .await
        .ok();
    signal(&site, &site.ada, 40, ProcessSignal::Int, STARTED)
        .await
        .ok();
    assert_eq!(
        site.disk.signals(),
        [(41, Signal::Term), (42, Signal::Kill), (40, Signal::Int)]
    );
    let audit = site.world.audit().await;
    assert_eq!(
        audit
            .iter()
            .filter(|action| *action == "process.signalled")
            .count(),
        3
    );
}

#[tokio::test]
async fn a_process_number_that_was_reused_is_never_signalled() {
    let site = live_site().await;
    // The process that was listed has gone and another has the number now.
    let refused = signal(
        &site,
        &site.ada,
        41,
        ProcessSignal::Kill,
        "2026-10-08T18:00:00.000Z",
    )
    .await
    .problem();
    assert_eq!(refused.code, ErrorCode::NotFound);
    assert!(site.disk.signals().is_empty());
}

#[tokio::test]
async fn a_viewer_sees_the_processes_and_cannot_signal_them() {
    let site = live_site().await;
    assert!(list(&site, &site.vic).await.status.is_success());
    let refused = signal(&site, &site.vic, 41, ProcessSignal::Kill, STARTED)
        .await
        .problem();
    assert_eq!(refused.code, ErrorCode::AuthzDenied);
    assert!(places::missing_permissions(&refused).contains(&"terminal.manage".to_owned()));
    let stranger = signal(&site, &site.eve, 41, ProcessSignal::Kill, STARTED).await;
    assert!(!stranger.status.is_success());
    assert_eq!(site.sent(CommandKind::ProcSignal), 0);
}

#[tokio::test]
async fn a_signal_needs_the_terminal_to_be_allowed_where_the_environment_is() {
    let site = live_site().await;
    sqlx::query("UPDATE environments SET terminal = false WHERE id = $1")
        .bind(site.environment)
        .execute(&site.world.db.pool)
        .await
        .unwrap();
    let refused = signal(&site, &site.ada, 41, ProcessSignal::Term, STARTED)
        .await
        .problem();
    assert_eq!(refused.code, ErrorCode::TerminalNotAllowed);
    assert!(
        list(&site, &site.ada).await.status.is_success(),
        "seeing them is another matter"
    );

    sqlx::query("UPDATE environments SET terminal = true WHERE id = $1")
        .bind(site.environment)
        .execute(&site.world.db.pool)
        .await
        .unwrap();
    site.world.set("terminal.enabled", json!(false)).await;
    let by_setting = signal(&site, &site.ada, 41, ProcessSignal::Term, STARTED)
        .await
        .problem();
    assert_eq!(by_setting.code, ErrorCode::TerminalNotAllowed);
    assert!(site.disk.signals().is_empty());
}

#[tokio::test]
async fn a_signal_needs_the_machine_and_a_number() {
    let site = live_site().await;
    let no_number = site
        .app()
        .call(
            EnvironmentProcessSignal,
            &[&site.environment, &"not-a-number"],
        )
        .token(&site.ada.token)
        .query(&ThroughThreadQuery {
            thread: Some(site.thread),
        })
        .body(&EnvironmentProcessSignalBody {
            signal: ProcessSignal::Term,
            started_at: STARTED.parse().unwrap(),
        })
        .send()
        .await
        .problem();
    assert_eq!(no_number.code, ErrorCode::NotFound);
    site.power_off().await;
    let away = signal(&site, &site.ada, 41, ProcessSignal::Term, STARTED)
        .await
        .problem();
    assert_eq!(away.code, ErrorCode::SourceOffline);
}

// ---------------------------------------------------------------- measurements

#[tokio::test]
async fn what_the_environment_used_lately_is_what_the_machine_sent_oldest_first() {
    let site = live_site().await;
    // Reading history does not subscribe a hidden view to future samples.
    assert!(resources(&site, &site.ada, None).await.ok().is_empty());
    assert_eq!(site.sent(CommandKind::WatchSet), 0);
    admitted_samples(&site);

    let now = site.world.now();
    for (minutes_ago, cpu) in [(90, 1.0), (30, 2.0), (5, 3.0)] {
        let taken = sample(now - Duration::minutes(minutes_ago), cpu);
        site.app().app.cache.hear(&report(&site, &taken));
    }
    let lately = resources(&site, &site.vic, None).await.ok();
    assert_eq!(
        lately
            .iter()
            .map(|each| each.cpu_percent)
            .collect::<Vec<_>>(),
        [2.0, 3.0],
        "the last hour, oldest first"
    );
    let longer = resources(&site, &site.vic, Some(120)).await.ok();
    assert_eq!(longer.len(), 3);
}

#[tokio::test]
async fn how_far_back_the_measurements_go_is_bounded() {
    let site = live_site().await;
    admitted_samples(&site);
    resources(&site, &site.ada, None).await.ok();
    let now = site.world.now();
    let old = sample(
        now - Duration::minutes(i64::from(RESOURCE_MINUTES_MAX) - 1),
        9.0,
    );
    let older = sample(
        now - Duration::minutes(i64::from(RESOURCE_MINUTES_MAX) + 60),
        8.0,
    );
    site.app().app.cache.hear(&report(&site, &old));
    site.app().app.cache.hear(&report(&site, &older));
    let far = resources(&site, &site.ada, Some(100_000)).await.ok();
    assert_eq!(far.len(), 1, "at most {RESOURCE_MINUTES_MAX} minutes");
    assert_eq!(far[0].cpu_percent, 9.0);
    assert_eq!(
        resources(&site, &site.ada, Some(0)).await.problem().code,
        ErrorCode::RequestInvalid
    );
}

#[tokio::test]
async fn a_machine_speaks_only_for_the_environments_it_holds() {
    let site = live_site().await;
    admitted_samples(&site);
    resources(&site, &site.ada, None).await.ok();
    let now = site.world.now();
    let own = sample(now, 1.0);
    site.app().app.cache.hear(&report(&site, &own));
    // The same measurement from a machine that does not hold the environment is not heard.
    let mut foreign = report(&site, &sample(now, 99.0));
    foreign.machine_id = Id::nil();
    site.app().app.cache.hear(&foreign);
    // Nor is a report of another kind, or of a stream that is no environment's.
    let mut other_kind = report(&site, &sample(now, 98.0));
    other_kind.eph.kind = "something.else".to_owned();
    site.app().app.cache.hear(&other_kind);
    let mut no_environment = report(&site, &sample(now, 97.0));
    no_environment.eph.stream = "session:x".to_owned();
    site.app().app.cache.hear(&no_environment);
    let kept = resources(&site, &site.ada, None).await.ok();
    assert_eq!(
        kept.iter().map(|each| each.cpu_percent).collect::<Vec<_>>(),
        [1.0]
    );
}

#[tokio::test]
async fn the_measurements_are_for_whoever_may_see_the_terminal_and_are_kept_while_the_machine_is_away()
 {
    let site = live_site().await;
    admitted_samples(&site);
    resources(&site, &site.ada, None).await.ok();
    site.app()
        .app
        .cache
        .hear(&report(&site, &sample(site.world.now(), 4.0)));
    assert!(!resources(&site, &site.eve, None).await.status.is_success());
    site.power_off().await;
    let kept = resources(&site, &site.vic, None).await.ok();
    assert_eq!(kept.len(), 1, "what was sent stays for its six hours");
}
