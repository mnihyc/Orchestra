//! Machine view subscriptions use real sockets and connector reports, stop when hidden and retain HTTP access cuts.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]

mod realtime_support;
#[path = "realtime_machine_views/transport.rs"]
mod transport;

use orch_db::queries::access;
use orch_link_contract::commands::{Command, CommandKind, CommandResult, Empty, FsListResult};
use orch_link_contract::events::Report;
use orch_link_contract::streams::Stream;
use orch_realtime_contract::feeds::{DirData, Feed};
use orch_types::{Id, Json, new_id};
use realtime_support::{Client, Rig, coordinator_ready, eventually};
use serde_json::json;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use transport::MachineViews;

type Seen = Arc<Mutex<Vec<Command>>>;

fn status(branch: &str) -> orch_api_contract::workspace::GitStatus {
    serde_json::from_value(json!({"available":true,"reason":null,"repository_dir":"web", "status_hash":null,
        "remotes":[],"upstream":null,"branch":branch,"head":null,"ahead":null,"behind":null,"changes":[]})).unwrap()
}

fn scripts(rig: &Rig) -> Seen {
    let seen: Seen = Arc::default();
    for kind in [
        CommandKind::WatchSet,
        CommandKind::FsList,
        CommandKind::FsUnwatch,
        CommandKind::GitStatus,
    ] {
        let seen = Arc::clone(&seen);
        rig.machine.handler.on(kind, move |command| {
            seen.lock().unwrap().push(command.clone());
            Ok(match command {
                Command::WatchSet(_) => CommandResult::WatchSet(Empty {}),
                Command::FsList(_) => CommandResult::FsList(FsListResult {
                    entries: Vec::new(),
                    truncated: false,
                    watch_instance: None,
                    page: None,
                }),
                Command::FsUnwatch(_) => CommandResult::FsUnwatch(Empty {}),
                Command::GitStatus(_) => CommandResult::GitStatus(status("main")),
                _ => unreachable!(),
            })
        });
    }
    seen
}

async fn commanded(seen: &Seen, predicate: impl Fn(&Command) -> bool) -> Command {
    eventually("the shared native watch command", || async {
        seen.lock().unwrap().iter().any(&predicate)
    })
    .await;
    seen.lock()
        .unwrap()
        .iter()
        .rev()
        .find(|command| predicate(command))
        .unwrap()
        .clone()
}

fn report(rig: &Rig, kind: &str, body: Json) {
    rig.machine.link.report(
        &Stream::Env {
            environment_id: rig.environment,
        },
        rig.clock.time(),
        &Report::decode(kind, body).unwrap(),
    );
}

async fn data(client: &mut Client, feed: &Feed) -> Json {
    loop {
        let frame = client.frame().await;
        assert!(
            !matches!(frame["op"].as_str(), Some("error" | "closed")),
            "{frame}"
        );
        if frame["op"] == "event"
            && frame["feed"] == feed.to_string()
            && frame["data"]["offline"] != true
        {
            // Synthetic diagnostics name shape/state, never command bodies or content.
            let value = &frame["data"];
            let keys: Vec<_> = value
                .as_object()
                .into_iter()
                .flat_map(|object| object.keys())
                .take(12)
                .collect();
            eprintln!(
                "machine-view event: keys={keys:?} reset={:?} invalidated={:?} error_code={:?} changed={} cursor={}",
                value["reset"].as_bool(),
                value["invalidated"].as_bool(),
                value["error"]["code"].as_str(),
                value["changed"].as_array().map_or(0, Vec::len),
                value["next_cursor"].is_string()
            );
            return frame["data"].clone();
        }
    }
}

async fn installed(rig: &Rig, list: &orch_link_contract::commands::FsList) {
    let instance = list
        .watch_instance
        .expect("a modern watched list has an exact installation instance");
    assert!(list.watch_id.is_some());
    eventually(
        "the exact native directory installation completed in the API journal",
        || async {
            sqlx::query_scalar::<_, bool>(
                "SELECT EXISTS(SELECT 1 FROM commands WHERE machine_id=$1 AND kind='fs.list'
            AND body->>'watch_instance'=$2 AND state='succeeded' AND result->>'watch_instance'=$2)",
            )
            .bind(rig.machine_id)
            .bind(instance.to_string())
            .fetch_one(rig.pool.pool())
            .await
            .unwrap()
        },
    )
    .await;
}

async fn unsub(client: &mut Client, id: i64) {
    client.send(json!({"op":"unsub","id":id})).await;
    let frames = client.until("closed").await;
    assert_eq!(frames.last().unwrap()["reason"], "unsubscribed");
}

async fn grant(rig: &Rig, person: Id, kind: &str, object: Id, permission: &str) -> Id {
    let id = new_id();
    let mut conn = rig.pool.acquire().await.unwrap();
    access::insert_entry(
        &mut conn,
        &access::NewEntry {
            id,
            page: &access::PageKey::new(kind, object, "live"),
            profile: "any",
            permission,
            holder_kind: "user",
            holder_id: Some(person),
            holder_role: None,
            options: &json!({}),
            expires_at: None,
            note: "",
            source: "page",
            created_by: Some(rig.ada),
            at: rig.clock.now(),
        },
    )
    .await
    .unwrap();
    access::bump_epoch(&mut conn).await.unwrap();
    id
}

fn sample(rig: &Rig, cpu: f64) -> Json {
    json!({"at":rig.clock.time(),"cpu_percent":cpu,"cpu_limit_millis":null,"memory_bytes":123,
        "memory_limit_bytes":null,"disk_used_bytes":null,"disk_limit_bytes":null,"net_rx_bps":0,"net_tx_bps":0,"pids":1})
}

fn entry(rig: &Rig, name: &str) -> Json {
    json!({"name":name,"path":format!("web/{name}"),"type":"file","size":4,"mtime":rig.clock.time(),
        "mode":420,"link_target":null,"cached":false})
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn processes_and_samples_share_one_watch_seed_later_readers_and_stop_after_the_last_unsubscribe()
 {
    let rig = MachineViews::new().await;
    let seen = scripts(&rig);
    let procs = Feed::EnvProcs(rig.environment);
    let stats = Feed::EnvStats(rig.environment);
    let mut client = Client::signed_in(rig.server.addr, &rig.token).await;
    for (id, feed) in [(1, &procs), (2, &stats)] {
        client.sub(id, &feed.to_string(), None).await;
        client.expect("subscribed").await;
    }
    coordinator_ready(&rig).await;
    commanded(
        &seen,
        |command| matches!(command, Command::WatchSet(body) if body.procs && body.stats),
    )
    .await;
    report(
        &rig,
        "procs.snapshot",
        json!({"as_of":rig.clock.time(),"processes":[]}),
    );
    assert_eq!(data(&mut client, &procs).await["processes"], json!([]));
    report(&rig, "stats.sample", sample(&rig, 12.0));
    assert_eq!(data(&mut client, &stats).await["cpu_percent"], 12.0);
    let mut later = Client::signed_in(rig.server.addr, &rig.token).await;
    later.sub(3, &stats.to_string(), None).await;
    later.expect("subscribed").await;
    assert_eq!(data(&mut later, &stats).await["cpu_percent"], 12.0);
    report(&rig, "stats.sample", sample(&rig, 37.0));
    assert_eq!(data(&mut client, &stats).await["cpu_percent"], 37.0);
    assert_eq!(data(&mut later, &stats).await["cpu_percent"], 37.0);
    unsub(&mut client, 1).await;
    unsub(&mut client, 2).await;
    unsub(&mut later, 3).await;
    commanded(&seen, |command| {
        matches!(command, Command::WatchSet(body)
        if !body.procs && !body.stats && !body.ports && !body.terminals)
    })
    .await;
    report(&rig, "stats.sample", sample(&rig, 99.0));
    assert!(client.is_quiet_for(Duration::from_millis(300)).await);
    assert!(later.is_quiet_for(Duration::from_millis(300)).await);
    drop((client, later));
    rig.close().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_visible_directory_sends_a_reset_then_deltas_seeds_its_current_entries_and_unwatches() {
    let rig = MachineViews::new().await;
    let seen = scripts(&rig);
    let listed = entry(&rig, "old.txt");
    let calls = Arc::clone(&seen);
    rig.machine.handler.on(CommandKind::FsList, move |command| {
        calls.lock().unwrap().push(command.clone());
        let Command::FsList(body) = command else {
            unreachable!()
        };
        Ok(CommandResult::FsList(FsListResult {
            entries: vec![serde_json::from_value(listed.clone()).unwrap()],
            truncated: false,
            watch_instance: body.watch_instance,
            page: None,
        }))
    });
    let feed = Feed::EnvDir {
        environment_id: rig.environment,
        path: "web".into(),
    };
    let mut client = Client::signed_in(rig.server.addr, &rig.token).await;
    client.sub(1, &feed.to_string(), None).await;
    client.expect("subscribed").await;
    let Command::FsList(list) = commanded(&seen, |command| matches!(command, Command::FsList(body) if body.path == "web" && body.watch_id.is_some())).await else { unreachable!() };
    installed(&rig, &list).await;
    let watch = list.watch_id.unwrap();
    let instance = list.watch_instance.unwrap();
    // The command result is sufficient even when the transient native reset was lost.
    let initial = data(&mut client, &feed).await;
    assert_eq!(initial["reset"], true);
    assert_eq!(initial["changed"][0]["name"], "old.txt");
    report(
        &rig,
        "fs.changed",
        json!({"watch_id":watch,"watch_instance":instance,"changed":[entry(&rig,"new.txt")],"removed":["old.txt"]}),
    );
    let changed = data(&mut client, &feed).await;
    assert_eq!(changed["removed"], json!(["old.txt"]));
    assert_eq!(changed["changed"][0]["name"], "new.txt");
    let mut later = Client::signed_in(rig.server.addr, &rig.token).await;
    later.sub(2, &feed.to_string(), None).await;
    later.expect("subscribed").await;
    let now = data(&mut later, &feed).await;
    assert_eq!(now["reset"], true);
    assert_eq!(now["changed"].as_array().unwrap().len(), 1);
    assert_eq!(now["changed"][0]["name"], "new.txt");
    assert_eq!(now["removed"], json!([]));
    assert_eq!(
        seen.lock()
            .unwrap()
            .iter()
            .filter(|command| matches!(command, Command::FsList(_)))
            .count(),
        1,
        "a later reader shares the successful native installation"
    );
    report(
        &rig,
        "fs.changed",
        json!({"watch_id":watch,"watch_instance":new_id(),"reset":true,"changed":[entry(&rig,"old-installation")],"removed":[]}),
    );
    assert!(client.is_quiet_for(Duration::from_millis(300)).await);
    report(
        &rig,
        "fs.changed",
        json!({"watch_id":new_id(),"changed":[entry(&rig,"wrong-watch")],"removed":[]}),
    );
    assert!(client.is_quiet_for(Duration::from_millis(300)).await);
    unsub(&mut client, 1).await;
    unsub(&mut later, 2).await;
    commanded(
        &seen,
        |command| matches!(command, Command::FsUnwatch(body) if body.watch_id == watch),
    )
    .await;
    drop((client, later));
    rig.close().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_paged_watch_exposes_its_http_cursor_and_invalidates_changes_outside_the_window() {
    let rig = MachineViews::new().await;
    let seen = scripts(&rig);
    let calls = Arc::clone(&seen);
    let listed = entry(&rig, "first.txt");
    let revision = orch_types::Hash::of(b"a synthetic frozen directory");
    let at = rig.clock.time();
    rig.machine.handler.on(CommandKind::FsList, move |command| {
        calls.lock().unwrap().push(command.clone());
        let Command::FsList(body) = command else {
            unreachable!()
        };
        Ok(CommandResult::FsList(FsListResult {
            entries: vec![serde_json::from_value(listed.clone()).unwrap()],
            truncated: true,
            watch_instance: body.watch_instance,
            page: Some(orch_link_contract::commands::FsListPageResult {
                revision,
                next_cursor: Some("synthetic-native-next".to_owned()),
                as_of: at,
                observed_from: None,
                expires_at: orch_types::Time::new(at.get() + chrono::Duration::minutes(5)),
            }),
        }))
    });
    let feed = Feed::EnvDir {
        environment_id: rig.environment,
        path: "web".to_owned(),
    };
    let mut client = Client::signed_in(rig.server.addr, &rig.token).await;
    client.sub(1, &feed.to_string(), None).await;
    client.expect("subscribed").await;
    let initial = data(&mut client, &feed).await;
    assert_eq!(initial["reset"], true);
    assert_eq!(initial["revision"], format!("live:{revision}"));
    assert_eq!(initial["as_of"], serde_json::to_value(at).unwrap());
    assert!(initial["next_cursor"].is_string());
    assert_ne!(initial["next_cursor"], "synthetic-native-next");
    let Command::FsList(list) = commanded(
        &seen,
        |command| matches!(command, Command::FsList(body) if body.watch_id.is_some()),
    )
    .await
    else {
        unreachable!()
    };
    installed(&rig, &list).await;
    report(
        &rig,
        "fs.changed",
        json!({"watch_id":list.watch_id,"watch_instance":list.watch_instance,
        "invalidated":true,"truncated":true,"changed":[],"removed":[]}),
    );
    let changed: DirData = serde_json::from_value(data(&mut client, &feed).await).unwrap();
    assert!(changed.invalidated);
    assert!(!changed.reset);
    assert!(changed.next_cursor.is_none());
    let mut later = Client::signed_in(rig.server.addr, &rig.token).await;
    later.sub(2, &feed.to_string(), None).await;
    later.expect("subscribed").await;
    let current = data(&mut later, &feed).await;
    assert_eq!(current["invalidated"], true);
    assert_eq!(current["changed"][0]["name"], "first.txt");
    assert!(current["next_cursor"].is_null());
    assert_eq!(
        seen.lock()
            .unwrap()
            .iter()
            .filter(|command| matches!(command, Command::FsList(_)))
            .count(),
        1,
        "an invalidation is not a timer-driven native reread"
    );
    unsub(&mut client, 1).await;
    unsub(&mut later, 2).await;
    drop((client, later));
    rig.close().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn failed_directory_activation_is_scoped_and_retries_only_for_fresh_demand() {
    use orch_link_contract::commands::{CommandError, ErrorCode};
    use std::sync::atomic::{AtomicUsize, Ordering};
    let rig = MachineViews::new().await;
    let seen = scripts(&rig);
    let attempts = Arc::new(AtomicUsize::new(0));
    let calls = Arc::clone(&attempts);
    let commands = Arc::clone(&seen);
    rig.machine.handler.on(CommandKind::FsList, move |command| {
        commands.lock().unwrap().push(command.clone());
        let Command::FsList(body) = command else {
            unreachable!()
        };
        if calls.fetch_add(1, Ordering::SeqCst) == 0 {
            return Err(CommandError::new(
                ErrorCode::Busy,
                "the directory watch capacity is full",
            ));
        }
        Ok(CommandResult::FsList(FsListResult {
            entries: Vec::new(),
            truncated: false,
            watch_instance: body.watch_instance,
            page: None,
        }))
    });
    coordinator_ready(&rig).await;
    let feed = Feed::EnvDir {
        environment_id: rig.environment,
        path: "web".to_owned(),
    };
    let (_, token) = rig.person("no-directory-door").await;
    let mut denied = Client::signed_in(rig.server.addr, &token).await;
    denied.sub(90, &feed.to_string(), None).await;
    let refusal = denied.expect("error").await;
    assert!(matches!(
        refusal["code"].as_str(),
        Some("forbidden" | "not_found")
    ));
    assert_eq!(
        attempts.load(Ordering::SeqCst),
        0,
        "a refused reader starts no native watch"
    );
    let mut client = Client::signed_in(rig.server.addr, &rig.token).await;
    client.sub(1, &feed.to_string(), None).await;
    client.expect("subscribed").await;
    let failure: DirData = serde_json::from_value(data(&mut client, &feed).await).unwrap();
    let error = failure.error.expect("the actual scoped activation error");
    assert_eq!(
        error.code,
        orch_api_contract::common::ErrorCode::EnvironmentUnavailable
    );
    assert_eq!(error.reason.as_deref(), Some("busy"));
    assert_eq!(error.detail, "the directory watch capacity is full");
    assert!(error.retryable);
    assert!(!failure.reset);
    assert!(failure.changed.is_empty());
    assert!(client.is_quiet_for(Duration::from_millis(2200)).await);
    assert_eq!(
        attempts.load(Ordering::SeqCst),
        1,
        "idle reconciliation never polls/retries the failed directory"
    );
    assert!(denied.is_quiet_for(Duration::from_millis(300)).await);
    client.sub(2, &feed.to_string(), None).await;
    client.expect("subscribed").await;
    tokio::time::timeout(realtime_support::PATIENCE, async {
        let mut readers = std::collections::BTreeSet::new();
        while readers.len() < 2 {
            let frame = client.frame().await;
            assert!(
                !matches!(frame["op"].as_str(), Some("error" | "closed")),
                "{frame}"
            );
            if frame["op"] == "event"
                && frame["feed"] == feed.to_string()
                && frame["data"]["reset"] == true
            {
                assert!(frame["data"]["error"].is_null());
                assert!(frame["data"]["changed"].as_array().unwrap().is_empty());
                assert!(
                    readers.insert(frame["id"].as_i64().unwrap()),
                    "one reset per current reader"
                );
            }
        }
        assert_eq!(readers, std::collections::BTreeSet::from([1, 2]));
    })
    .await
    .unwrap();
    assert_eq!(attempts.load(Ordering::SeqCst), 2);
    assert!(client.is_quiet_for(Duration::from_millis(2200)).await);
    assert_eq!(attempts.load(Ordering::SeqCst), 2);
    drop((client, denied));
    rig.close().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn git_follows_the_current_thread_directory_shares_its_native_watch_and_rejects_a_late_old_binding()
 {
    let rig = MachineViews::new().await;
    let seen = scripts(&rig);
    let thread = rig.thread("Work").await;
    let feed = Feed::ThreadGit(thread);
    let explicit = Feed::EnvGit {
        environment_id: rig.environment,
        path: "web".into(),
    };
    let mut client = Client::signed_in(rig.server.addr, &rig.token).await;
    client.sub(1, &feed.to_string(), None).await;
    client.expect("subscribed").await;
    let Command::GitStatus(first) = commanded(
        &seen,
        |command| matches!(command, Command::GitStatus(body) if body.watch_id.is_some()),
    )
    .await
    else {
        unreachable!()
    };
    let watch = first.watch_id.unwrap();
    assert_eq!(first.path.as_deref(), Some("web"));
    report(
        &rig,
        "git.changed",
        json!({"watch_id":watch,"status":status("main")}),
    );
    assert_eq!(data(&mut client, &feed).await["status"]["branch"], "main");
    client.sub(2, &explicit.to_string(), None).await;
    client.expect("subscribed").await;
    report(
        &rig,
        "git.changed",
        json!({"watch_id":watch,"status":status("work")}),
    );
    let mut got_thread = false;
    let mut got_explicit = false;
    while !(got_thread && got_explicit) {
        let frame = client.frame().await;
        if frame["op"] == "event" && frame["data"]["status"]["branch"] == "work" {
            got_thread |= frame["feed"] == feed.to_string();
            got_explicit |= frame["feed"] == explicit.to_string();
        }
    }
    assert_eq!(seen.lock().unwrap().iter().filter(|command| matches!(command, Command::GitStatus(body) if body.watch_id == Some(watch))).count(), 1);
    unsub(&mut client, 2).await;
    sqlx::query("UPDATE threads SET workspace_dir='other' WHERE node_id=$1")
        .bind(thread)
        .execute(rig.pool.pool())
        .await
        .unwrap();
    report(
        &rig,
        "git.changed",
        json!({"watch_id":watch,"status":status("old-directory")}),
    );
    assert!(client.is_quiet_for(Duration::from_millis(300)).await);
    let Command::GitStatus(next) = commanded(&seen, |command| matches!(command, Command::GitStatus(body) if body.path.as_deref() == Some("other") && body.watch_id.is_some())).await else { unreachable!() };
    report(
        &rig,
        "git.changed",
        json!({"watch_id":next.watch_id,"status":status("new-directory")}),
    );
    let changed = data(&mut client, &feed).await;
    assert_eq!(changed["path"], "other");
    assert_eq!(changed["status"]["branch"], "new-directory");
    unsub(&mut client, 1).await;
    commanded(&seen, |command| matches!(command, Command::FsUnwatch(body) if Some(body.watch_id) == next.watch_id)).await;
    drop(client);
    rig.close().await;
}

async fn shell(rig: &Rig, opener: Id, state: &str) -> Id {
    let id = new_id();
    sqlx::query("INSERT INTO terminal_sessions(id,environment_id,env_generation,opened_by,shell,cwd,cols,rows,state)
        VALUES($1,$2,1,$3,'sh','/workspace',80,24,$4)")
        .bind(id).bind(rig.environment).bind(opener).bind(state).execute(rig.pool.pool()).await.unwrap();
    id
}

async fn terminal_host(rig: &Rig) {
    use orch_db::queries::{environment_facts, resources};
    let driver: orch_driver_contract::descriptor::DriverDescriptor =
        serde_json::from_value(json!({
            "name":"test","version":"1.0.0","title":"Synthetic unit driver","lifecycle":"unit",
            "spec_schemas":{"environment":{"type":"object","additionalProperties":false}}
        }))
        .unwrap();
    let policy = orch_link_contract::policy::LocalPolicy {
        roles: Some(vec![orch_link_contract::session::Role::Node]),
        environment_terminal: true,
        ..Default::default()
    };
    let mut conn = rig.pool.acquire().await.unwrap();
    let machine = resources::testing::insert_machine(&mut conn, "Terminal fixture node", "managed")
        .await
        .unwrap();
    let host = new_id();
    resources::insert_host(
        &mut conn,
        &resources::NewHost {
            id: host,
            machine_id: machine,
            owner_id: None,
            home_id: None,
            name: "Terminal fixture host",
        },
        rig.clock.now(),
    )
    .await
    .unwrap();
    sqlx::query("UPDATE hosts SET drivers=$2 WHERE id=$1")
        .bind(host)
        .bind(json!({"test":driver}))
        .execute(&mut *conn)
        .await
        .unwrap();
    sqlx::query(
        "UPDATE machines SET host_id=$2,roles=ARRAY['node'],online=true,
        facts=facts || jsonb_build_object('policy',$3::jsonb) WHERE id=$1",
    )
    .bind(machine)
    .bind(host)
    .bind(policy.reported())
    .execute(&mut *conn)
    .await
    .unwrap();
    // A unit uses its physical host's ceiling, not the unit connector's harness facts.
    // Establish it before any terminal exists, so even the first keeper pass sees a valid shell.
    sqlx::query("UPDATE environments SET config=config || jsonb_build_object('env.host_id',$2::text) WHERE id=$1")
        .bind(rig.environment).bind(host.to_string()).execute(&mut *conn).await.unwrap();
    let facts = environment_facts::of(&mut conn, rig.environment)
        .await
        .unwrap()
        .expect("the unit has physical host facts");
    assert_ne!(
        machine, rig.machine_id,
        "the native report still comes from the distinct unit connector"
    );
    assert_eq!(facts.machine_id, Some(machine));
    assert!(facts.makes_units());
    assert_eq!(
        facts
            .descriptor
            .as_ref()
            .and_then(|descriptor| descriptor.get("name"))
            .and_then(Json::as_str),
        Some("test")
    );
    assert_eq!(
        facts
            .policy
            .as_ref()
            .and_then(|policy| policy.get("environment.terminal"))
            .and_then(Json::as_bool),
        Some(true)
    );
    assert!(
        environment_facts::permits(&mut conn, rig.environment, "environment.terminal")
            .await
            .unwrap()
    );
}

#[derive(Debug)]
struct TerminalRowState {
    exact_source: bool,
    state: String,
    reason: Option<String>,
    closed: bool,
    holding: orch_engine::shells::Holding,
}

async fn terminal_rows(rig: &Rig, sessions: &[(Id, Id); 2]) -> Vec<TerminalRowState> {
    let shells = rig.server.engine.shells();
    let mut conn = rig.pool.acquire().await.unwrap();
    let mut states = Vec::with_capacity(sessions.len());
    for (index, (id, opener)) in sessions.iter().enumerate() {
        let row = orch_db::queries::terminals::get(&mut conn, *id)
            .await
            .unwrap()
            .unwrap_or_else(|| panic!("terminal fixture row {index} is missing"));
        let holding = shells.holds(&mut conn, &row).await.unwrap();
        states.push(TerminalRowState {
            exact_source: row.id == *id
                && row.environment_id == rig.environment
                && row.env_generation == 1
                && row.opened_by == *opener,
            state: row.state,
            reason: row.close_reason,
            closed: row.closed_at.is_some(),
            holding,
        });
    }
    states
}

async fn assert_terminal_rows_open(rig: &Rig, sessions: &[(Id, Id); 2], phase: &str) {
    let states = terminal_rows(rig, sessions).await;
    assert!(
        states.iter().all(|row| row.exact_source
            && row.state == "open"
            && row.reason.is_none()
            && !row.closed
            && row.holding == orch_engine::shells::Holding::Yes),
        "terminal fixture {phase}, own then foreign: {states:?}"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn terminal_reports_are_projected_for_the_opener_and_row_commits_push_closure_without_a_get()
{
    let rig = MachineViews::new().await;
    let seen = scripts(&rig);
    terminal_host(&rig).await;
    let (reader, token) = rig.person("shell-reader").await;
    grant(
        &rig,
        reader,
        "environment",
        rig.environment,
        "terminal.manage",
    )
    .await;
    let own = shell(&rig, reader, "open").await;
    let foreign = shell(&rig, rig.ada, "open").await;
    let sessions = [(own, reader), (foreign, rig.ada)];
    assert_terminal_rows_open(&rig, &sessions, "before keeper recheck").await;
    let closed = rig
        .server
        .engine
        .shells()
        .recheck(&|_, _| {})
        .await
        .unwrap();
    assert_terminal_rows_open(&rig, &sessions, "after keeper recheck").await;
    assert!(
        closed.is_empty(),
        "the actual keeper recheck closed {} fixture rows",
        closed.len()
    );
    let feed = Feed::EnvTerminals(rig.environment);
    let mut client = Client::signed_in(rig.server.addr, &token).await;
    client.sub(1, &feed.to_string(), None).await;
    client.expect("subscribed").await;
    commanded(&seen, |command| matches!(command, Command::WatchSet(body) if body.environment_id==rig.environment && body.terminals)).await;
    assert_terminal_rows_open(&rig, &sessions, "native watch admitted").await;
    report(
        &rig,
        "terminals.snapshot",
        json!({"terminals":[{"terminal_id":own,"busy":false},{"terminal_id":foreign,"busy":true}]}),
    );
    let initial = data(&mut client, &feed).await;
    if initial["open"]["items"].as_array().map(Vec::len) != Some(1)
        || initial["open"]["items"][0]["id"] != own.to_string()
        || initial["open"]["items"][0]["busy"] != false
        || initial.to_string().contains(&foreign.to_string())
    {
        eprintln!(
            "terminal fixture first report, own then foreign: {:?}",
            terminal_rows(&rig, &sessions).await
        );
    }
    assert_eq!(initial["open"]["items"].as_array().unwrap().len(), 1);
    assert_eq!(initial["open"]["items"][0]["id"], own.to_string());
    assert_eq!(initial["open"]["items"][0]["busy"], false);
    assert!(!initial.to_string().contains(&foreign.to_string()));
    report(
        &rig,
        "terminals.snapshot",
        json!({"terminals":[{"terminal_id":own,"busy":true},{"terminal_id":foreign,"busy":true}]}),
    );
    let busy = data(&mut client, &feed).await;
    assert_eq!(busy["open"]["items"][0]["busy"], true);
    assert_eq!(busy["open"]["items"].as_array().unwrap().len(), 1);
    assert_eq!(busy["open"]["items"][0]["id"], own.to_string());
    assert!(!busy.to_string().contains(&foreign.to_string()));
    sqlx::query("UPDATE terminal_sessions SET state='closed',closed_at=now(),close_reason='exited',exit_code=0 WHERE id=$1")
        .bind(own).execute(rig.pool.pool()).await.unwrap();
    let closed = data(&mut client, &feed).await;
    assert_eq!(closed["open"]["items"], json!([]));
    assert_eq!(closed["closed"]["items"][0]["id"], own.to_string());
    assert_eq!(closed["closed"]["items"][0]["state"], "closed");
    assert_eq!(closed["closed"]["items"][0]["busy"], false);
    assert!(!closed.to_string().contains(&foreign.to_string()));
    unsub(&mut client, 1).await;
    commanded(&seen, |command| matches!(command, Command::WatchSet(body) if body.environment_id==rig.environment && !body.terminals)).await;
    drop(client);
    rig.close().await;
}

fn ports(rig: &Rig, port: u16) -> Json {
    json!({"request_ref":null,"snapshot":{"environment_id":rig.environment,"generation":1,"as_of":rig.clock.time(),
        "activity_at":null,"discovery":"native","reason":null,"udp_dropped":{"oversized":0,"full":0,"senders":0,"local_send":0,"link_lost":0},
        "listeners":[{"port":port,"transport":"tcp","local_address":"127.0.0.1","generation":1,"observation_id":format!("socket-{port}"),
            "first_seen_at":rig.clock.time(),"binding":"listening","process":null,"internal":false,"unavailable_reason":null}]}})
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn job_and_batch_waits_receive_committed_progress_with_the_same_personal_read_rules() {
    let rig = MachineViews::new().await;
    let thread = rig.thread("A batch member").await;
    rig.server.engine.stop();
    let batch = new_id();
    let job = new_id();
    sqlx::query(
        "INSERT INTO jobs(id,kind,node_id,environment_id,batch_id,state,created_by)
        VALUES($1,'thread_fork',$2,$3,$4,'queued',$5)",
    )
    .bind(job)
    .bind(thread)
    .bind(rig.environment)
    .bind(batch)
    .bind(rig.ada)
    .execute(rig.pool.pool())
    .await
    .unwrap();
    let job_feed = Feed::Job(job);
    let batch_feed = Feed::Fork(batch);
    let mut client = Client::signed_in(rig.server.addr, &rig.token).await;
    client.sub(1, &job_feed.to_string(), None).await;
    client.expect("subscribed").await;
    let first = data(&mut client, &job_feed).await;
    assert_eq!(first["job"]["state"], "queued");
    let http: Json = reqwest::Client::new()
        .get(format!("http://{}/api/v1/jobs/{job}", rig.server.addr))
        .bearer_auth(&rig.token)
        .send()
        .await
        .unwrap()
        .error_for_status()
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(http, first["job"]);
    client.sub(2, &batch_feed.to_string(), None).await;
    client.expect("subscribed").await;
    assert_eq!(
        data(&mut client, &batch_feed).await["batch"]["jobs"][0]["job"]["id"],
        job.to_string()
    );
    coordinator_ready(&rig).await;
    sqlx::query("UPDATE jobs SET state='succeeded',finished_at=now(),result=$2 WHERE id=$1")
        .bind(job)
        .bind(json!({"thread":thread}))
        .execute(rig.pool.pool())
        .await
        .unwrap();
    let mut got_job = false;
    let mut got_batch = false;
    while !(got_job && got_batch) {
        let frame = client.frame().await;
        assert_ne!(frame["op"], "error", "{frame}");
        if frame["feed"] == job_feed.to_string() && frame["data"]["job"]["state"] == "succeeded" {
            got_job = true;
        }
        if frame["feed"] == batch_feed.to_string()
            && frame["data"]["batch"]["jobs"][0]["job"]["state"] == "succeeded"
        {
            got_batch = true;
        }
    }
    let (_, stranger_token) = rig.person("job-stranger").await;
    let mut stranger = Client::signed_in(rig.server.addr, &stranger_token).await;
    for feed in [&job_feed, &batch_feed] {
        stranger.sub(1, &feed.to_string(), None).await;
        let refused = stranger.expect("error").await;
        assert!(
            matches!(refused["code"].as_str(), Some("not_found" | "forbidden")),
            "{refused}"
        );
    }
    unsub(&mut client, 1).await;
    unsub(&mut client, 2).await;
    sqlx::query("UPDATE jobs SET result=$2 WHERE id=$1")
        .bind(job)
        .bind(json!({"after":"hidden"}))
        .execute(rig.pool.pool())
        .await
        .unwrap();
    assert!(client.is_quiet_for(Duration::from_millis(300)).await);
    drop((client, stranger));
    rig.close().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_browser_list_also_pushes_changes_and_is_dropped_with_its_view() {
    let rig = MachineViews::new().await;
    let feed = Feed::EnvBrowsers(rig.environment);
    let list = |name: &str| json!({"browsers":[],"profiles":[{"name":name,"size_bytes":null,"used_at":null,"browser":null}]});
    rig.server
        .realtime
        .publish_list(feed.clone(), list("first"))
        .await
        .unwrap();
    let mut client = Client::signed_in(rig.server.addr, &rig.token).await;
    client.sub(1, &feed.to_string(), None).await;
    client.expect("subscribed").await;
    assert_eq!(
        data(&mut client, &feed).await["profiles"][0]["name"],
        "first"
    );
    coordinator_ready(&rig).await;
    rig.server
        .realtime
        .publish_list(feed.clone(), list("changed"))
        .await
        .unwrap();
    assert_eq!(
        data(&mut client, &feed).await["profiles"][0]["name"],
        "changed"
    );
    unsub(&mut client, 1).await;
    rig.server
        .realtime
        .publish_list(feed, list("hidden"))
        .await
        .unwrap();
    assert!(client.is_quiet_for(Duration::from_millis(300)).await);
    drop(client);
    rig.close().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn native_port_changes_and_committed_target_options_push_the_same_current_view_as_the_get() {
    let rig = MachineViews::new().await;
    let seen = scripts(&rig);
    let feed = Feed::EnvPorts(rig.environment);
    let mut client = Client::signed_in(rig.server.addr, &rig.token).await;
    client.sub(1, &feed.to_string(), None).await;
    client.expect("subscribed").await;
    commanded(
        &seen,
        |command| matches!(command, Command::WatchSet(body) if body.ports),
    )
    .await;
    report(&rig, "ports.snapshot", ports(&rig, 8080));
    let mut current = data(&mut client, &feed).await;
    while current["snapshot"]["listeners"]
        .as_array()
        .is_none_or(Vec::is_empty)
    {
        current = data(&mut client, &feed).await;
    }
    assert_eq!(current["targets"][0]["port"], 8080);
    let http: Json = reqwest::Client::new()
        .get(format!(
            "http://{}/api/v1/environments/{}/ports?limit=200",
            rig.server.addr, rig.environment
        ))
        .bearer_auth(&rig.token)
        .send()
        .await
        .unwrap()
        .error_for_status()
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(http, current);
    report(&rig, "ports.snapshot", ports(&rig, 9090));
    assert_eq!(data(&mut client, &feed).await["targets"][0]["port"], 9090);
    sqlx::query(
        "INSERT INTO port_targets(environment_id,port,http_scheme) VALUES($1,9090,'https')",
    )
    .bind(rig.environment)
    .execute(rig.pool.pool())
    .await
    .unwrap();
    let updated = data(&mut client, &feed).await;
    assert_eq!(updated["targets"][0]["program"]["http_scheme"], "https");
    unsub(&mut client, 1).await;
    commanded(
        &seen,
        |command| matches!(command, Command::WatchSet(body) if !body.ports),
    )
    .await;
    drop(client);
    rig.close().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn every_machine_feed_uses_its_get_permission_and_a_thread_grant_is_never_a_door_into_its_environment()
 {
    let rig = MachineViews::new().await;
    let seen = scripts(&rig);
    let (reader, token) = rig.person("no-machine-doors").await;
    let thread = rig.thread("A shared thread").await;
    grant(&rig, reader, "node", thread, "thread.view").await;
    let mut client = Client::signed_in(rig.server.addr, &token).await;
    let feeds = [
        Feed::EnvStats(rig.environment),
        Feed::EnvProcs(rig.environment),
        Feed::EnvBrowsers(rig.environment),
        Feed::EnvTerminals(rig.environment),
        Feed::EnvPorts(rig.environment),
        Feed::EnvDir {
            environment_id: rig.environment,
            path: "web".into(),
        },
        Feed::EnvGit {
            environment_id: rig.environment,
            path: "web".into(),
        },
        Feed::ThreadGit(thread),
    ];
    for (id, feed) in feeds.iter().enumerate() {
        client
            .sub(i64::try_from(id).unwrap() + 1, &feed.to_string(), None)
            .await;
        let refusal = client.expect("error").await;
        assert!(
            matches!(refusal["code"].as_str(), Some("forbidden" | "not_found")),
            "{refusal}"
        );
    }
    for suffix in [
        "processes",
        "resources",
        "browsers",
        "terminals",
        "ports",
        "files?path=web",
        "git/status?path=web",
    ] {
        let status = reqwest::Client::new()
            .get(format!(
                "http://{}/api/v1/environments/{}/{suffix}",
                rig.server.addr, rig.environment
            ))
            .bearer_auth(&token)
            .send()
            .await
            .unwrap()
            .status();
        assert!(matches!(status.as_u16(), 403 | 404), "{suffix}: {status}");
    }
    assert!(seen.lock().unwrap().iter().all(|command| !matches!(
        command,
        Command::WatchSet(_)
            | Command::FsList(_)
            | Command::GitStatus(orch_link_contract::commands::GitStatus {
                watch_id: Some(_),
                ..
            })
    )));
    let grant_id = grant(&rig, reader, "environment", rig.environment, "ports.view").await;
    let feed = Feed::EnvPorts(rig.environment);
    client.sub(20, &feed.to_string(), None).await;
    client.expect("subscribed").await;
    data(&mut client, &feed).await;
    sqlx::query("UPDATE permission_entries SET removed_at=now() WHERE id=$1")
        .bind(grant_id)
        .execute(rig.pool.pool())
        .await
        .unwrap();
    rig.bump_epoch().await;
    let frames = client.until("closed").await;
    assert_eq!(frames.last().unwrap()["reason"], "revoked");
    drop(client);
    rig.close().await;
}
