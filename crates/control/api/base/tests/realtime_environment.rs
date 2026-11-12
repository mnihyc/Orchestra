//! Live environment feeds on a real connector: shared watches, signals, direct-child changes and offline recovery.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]
#![cfg(target_os = "linux")]

mod feeds_machine;
mod realtime_support;

use feeds_machine::Machine;
use orch_link_contract::commands::{ProcSignal, PtyClose, PtyOpen, Signal};
use orch_realtime_contract::feeds::Feed;
use orch_types::{Time, new_id};
use realtime_support::{Client, Rig, Server, eventually, quick};
use serde_json::Value;
use std::time::{Duration, Instant};

/// Long enough for an already gathered directory batch to traverse a busy shared machine.
const PATIENCE: Duration = Duration::from_secs(90);

async fn event(client: &mut Client, id: i64) -> Value {
    let frame = match client.hear_within(realtime_support::PATIENCE).await {
        Some(realtime_support::Heard::Frame(frame)) => frame,
        other => panic!("environment event id={id}: expected the next frame, got {other:?}"),
    };
    assert_eq!(
        frame["op"], "event",
        "environment event id={id}: code={} feed={}",
        frame["code"], frame["feed"]
    );
    assert_eq!(
        frame["id"], id,
        "another subscription's frame cannot be discarded"
    );
    frame["data"].clone()
}

/// Real commands may wait for the machine; keep the same socket alive without consuming its state.
async fn command_while_following<B: orch_link_contract::commands::CommandBody>(
    machine: &Machine,
    client: &mut Client,
    body: &B,
) -> Value {
    let work = machine.send(body);
    tokio::pin!(work);
    let period = Duration::from_secs(u64::from(orch_realtime_contract::version::HEARTBEAT_SECS));
    let mut heartbeat = tokio::time::interval_at(tokio::time::Instant::now() + period, period);
    tokio::time::timeout(PATIENCE, async {
        loop {
            tokio::select! {
                answer = &mut work => break answer,
                _ = heartbeat.tick() => client.heartbeat().await,
            }
        }
    })
    .await
    .expect("the native command retains its original fixture deadline")
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn two_processes_share_one_machine_watch_until_the_last_departure() {
    let rig = Rig::new().await;
    let mut machine = Machine::start(rig.server.app.clone(), rig.environment).await;
    let second = Server::over(
        &rig.pool,
        &rig.clock,
        quick(&orch_config::ServerConfig::default()),
    )
    .await;
    let feed = Feed::EnvProcs(rig.environment).to_string();
    let mut one = Client::signed_in(rig.server.addr, &rig.token).await;
    let mut two = Client::signed_in(second.addr, &rig.token).await;
    for client in [&mut one, &mut two] {
        client.sub(1, &feed, None).await;
        client.expect("subscribed").await;
    }
    let data = event(&mut one, 1).await;
    assert_eq!(event(&mut two, 1).await, data);
    assert!(data["processes"].is_array());
    assert_eq!(machine.seen("watch.set").len(), 1);
    one.send(serde_json::json!({"op": "unsub", "id": 1})).await;
    one.expect("closed").await;
    let _ = two.ping().await;
    assert_eq!(machine.seen("watch.set").len(), 1);
    two.send(serde_json::json!({"op": "unsub", "id": 1})).await;
    two.expect("closed").await;
    // The coordinator's own controlled-clock test proves the exact grace; here the
    // live connector proves that one watch is shared and the last departure ends it.
    eventually("watch disabled after departure", || async {
        machine.seen("watch.set").len() == 2
    })
    .await;
    assert_eq!(machine.seen("watch.set")[1].body["procs"], false);
    drop((one, two));
    second.stop().await;
    rig.server.stop().await;
    machine.stop().await;
}

// A departure during an outage must release the host's watch when it returns.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_last_departure_during_an_outage_stops_the_watch_when_the_machine_returns() {
    let rig = Rig::new().await;
    let mut machine = Machine::start(rig.server.app.clone(), rig.environment).await;
    let mut client = Client::signed_in(rig.server.addr, &rig.token).await;
    client
        .sub(1, &Feed::EnvStats(rig.environment).to_string(), None)
        .await;
    client.expect("subscribed").await;
    assert!(event(&mut client, 1).await["cpu_percent"].is_number());
    eventually("the first watch settled", || async {
        machine.settled("watch.set").await
    })
    .await;
    machine.online(false).await;
    loop {
        if event(&mut client, 1).await["offline"] == true {
            break;
        }
    }
    client
        .send(serde_json::json!({"op": "unsub", "id": 1}))
        .await;
    client.expect("closed").await;
    tokio::time::sleep(orch_api_core::realtime::ephemeral::STOP_GRACE).await;
    assert_eq!(machine.seen("watch.set").len(), 1);
    machine.online(true).await;
    eventually("the abandoned watch disabled after return", || async {
        machine.seen("watch.set").len() == 2
    })
    .await;
    assert_eq!(machine.seen("watch.set")[1].body["stats"], false);
    drop(client);
    rig.server.stop().await;
    machine.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_file_made_changed_and_removed_reaches_its_directory_and_leaves_another_directory_quiet()
{
    let rig = Rig::new().await;
    let mut machine = Machine::start(rig.server.app.clone(), rig.environment).await;
    std::fs::create_dir(machine.root.join("other")).unwrap();
    let mut client = Client::signed_in(rig.server.addr, &rig.token).await;
    for (id, path) in [(1, ""), (2, "other")] {
        client
            .sub(
                id,
                &Feed::EnvDir {
                    environment_id: rig.environment,
                    path: path.to_owned(),
                }
                .to_string(),
                None,
            )
            .await;
        client.expect("subscribed").await;
        let initial = event(&mut client, id).await;
        assert!(
            initial.get("error").is_none(),
            "directory activation id={id}: code={} reason={}",
            initial["error"]["code"],
            initial["error"]["reason"]
        );
        assert_ne!(
            initial["offline"], true,
            "the actual attachment must be reachable before the initial directory snapshot"
        );
        assert_eq!(
            initial["reset"], true,
            "each newly watched directory starts with its own snapshot"
        );
    }
    eventually("both directories watched", || async {
        machine.seen("fs.list").len() == 2 && machine.settled("fs.list").await
    })
    .await;
    let file = machine.root.join("notes.txt");
    for contents in [Some("made"), Some("changed"), None] {
        let began = Instant::now();
        if let Some(contents) = contents {
            std::fs::write(&file, contents).unwrap();
        } else {
            std::fs::remove_file(&file).unwrap();
        }
        let data = tokio::time::timeout(PATIENCE, async {
            loop {
                let frame = client.expect("event").await;
                assert_eq!(frame["id"], 1, "another directory was told of this file");
                let data = frame["data"].clone();
                if contents.is_none()
                    && data["removed"]
                        .as_array()
                        .unwrap()
                        .iter()
                        .any(|name| name == "notes.txt")
                {
                    break data;
                }
                if contents.is_some()
                    && data["changed"]
                        .as_array()
                        .unwrap()
                        .iter()
                        .any(|entry| entry["name"] == "notes.txt")
                {
                    break data;
                }
            }
        })
        .await
        .unwrap();
        println!(
            "file event delay_ms={} removed={}",
            began.elapsed().as_millis(),
            contents.is_none()
        );
        assert!(data["as_of"].is_string());
    }
    drop(client);
    rig.server.stop().await;
    machine.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn load_is_sampled_and_offline_is_said_once_until_the_machine_returns() {
    let rig = Rig::new().await;
    let mut machine = Machine::start(rig.server.app.clone(), rig.environment).await;
    let mut client = Client::signed_in(rig.server.addr, &rig.token).await;
    client
        .sub(1, &Feed::EnvStats(rig.environment).to_string(), None)
        .await;
    client.expect("subscribed").await;
    assert!(event(&mut client, 1).await["cpu_percent"].is_number());
    eventually("the watch command settled", || async {
        machine.settled("watch.set").await
    })
    .await;
    machine.online(false).await;
    loop {
        if event(&mut client, 1).await["offline"] == true {
            break;
        }
    }
    assert!(client.is_quiet_for(Duration::from_secs(2)).await);
    let late = Server::over(
        &rig.pool,
        &rig.clock,
        quick(&orch_config::ServerConfig::default()),
    )
    .await;
    let mut late_client = Client::signed_in(late.addr, &rig.token).await;
    late_client
        .sub(1, &Feed::EnvStats(rig.environment).to_string(), None)
        .await;
    late_client.expect("subscribed").await;
    assert_eq!(event(&mut late_client, 1).await["offline"], true);
    assert!(late_client.is_quiet_for(Duration::from_secs(2)).await);
    machine.online(true).await;
    assert!(event(&mut client, 1).await["cpu_percent"].is_number());
    assert!(event(&mut late_client, 1).await["cpu_percent"].is_number());
    drop((client, late_client));
    late.stop().await;
    rig.server.stop().await;
    machine.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_signal_to_a_process_is_followed_by_a_snapshot_without_that_process() {
    let rig = Rig::new().await;
    let mut machine = Machine::start(rig.server.app.clone(), rig.environment).await;
    let terminal = new_id();
    machine
        .send(&PtyOpen {
            environment_id: rig.environment,
            terminal_id: terminal,
            cols: 80,
            rows: 24,
            cwd: "".to_owned(),
            record: false,
        })
        .await;
    let mut client = Client::signed_in(rig.server.addr, &rig.token).await;
    client
        .sub(1, &Feed::EnvProcs(rig.environment).to_string(), None)
        .await;
    client.expect("subscribed").await;
    let first = event(&mut client, 1).await;
    let process = first["processes"]
        .as_array()
        .unwrap()
        .first()
        .expect("the tracked shell");
    // TERM only asks a login shell to end; this positive removal oracle needs an unignorable signal.
    // Native TERM/grace behavior has its own process tests. Keep the exact pid/start identity here.
    let signalled = command_while_following(
        &machine,
        &mut client,
        &ProcSignal {
            environment_id: rig.environment,
            pid: process["pid"].as_i64().unwrap(),
            started_at: serde_json::from_value::<Time>(process["started_at"].clone()).unwrap(),
            signal: Signal::Kill,
        },
    )
    .await;
    assert_eq!(
        signalled,
        serde_json::json!({}),
        "the exact native proc.signal must acknowledge success"
    );
    let period = Duration::from_secs(u64::from(orch_realtime_contract::version::HEARTBEAT_SECS));
    let mut heartbeat = tokio::time::interval_at(tokio::time::Instant::now() + period, period);
    let mut last_state = None;
    let gone = tokio::time::timeout(PATIENCE, async {
        loop {
            let snapshot = tokio::select! {
                snapshot = event(&mut client, 1) => snapshot,
                _ = heartbeat.tick() => { client.heartbeat().await; continue; },
            };
            let rows = snapshot["processes"]
                .as_array()
                .expect("a process snapshot has its process array");
            if let Some(row) = rows.iter().find(|row| {
                row["pid"] == process["pid"] && row["started_at"] == process["started_at"]
            }) {
                last_state = row["state"]
                    .as_str()
                    .map(|state| state.chars().take(64).collect::<String>());
            } else {
                break;
            }
        }
    })
    .await;
    assert!(
        gone.is_ok(),
        "native signal acknowledged but the exact process remained: pid={} started_at={} last_state={last_state:?}",
        process["pid"],
        process["started_at"]
    );
    machine
        .send(&PtyClose {
            terminal_id: terminal,
        })
        .await;
    drop(client);
    rig.server.stop().await;
    machine.stop().await;
}
