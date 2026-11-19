//! The feeds beside a thread's, and what several API processes and a lost announcement ask of all of them: the Inbox, an organization's tree, a topic, who is looking, and the news that is no row.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]

mod realtime_support;

use axum::body::Body;
use http::Request;
use orch_api_contract::nodes::OrgTreeGetReply;
use orch_api_core::registry::Registry;
use orch_db::queries::hierarchy;
use orch_db::queries::notifications::people::{NewNotification, announce};
use orch_realtime_contract::feeds::UserEvent;
use orch_types::Id;
use realtime_support::{Client, Rig, Server, message_grew, quick};
use serde_json::{Value, json};
use std::time::{Duration, Instant};
use tower::ServiceExt;

/// An announcement of a row in a person's Inbox, as the writer of one makes it.
async fn write_to_inbox(rig: &Rig, person: Id, title: &str, node: Option<Id>) -> i64 {
    let mut tx = rig.pool.begin().await.unwrap();
    let written = announce(
        &mut tx,
        &NewNotification {
            user_id: person,
            kind: "milestone",
            node_id: node,
            thread_id: None,
            approval_id: None,
            chat_id: None,
            title,
            body: "",
            data: &json!({}),
            dedupe_key: &format!("milestone:{title}"),
            at: rig.clock.now(),
        },
    )
    .await
    .unwrap()
    .expect("a row was written");
    tx.commit().await.unwrap();
    written.inbox_seq
}

/// Waits for the next frame of `op` among those that come.
async fn next_of(client: &mut Client, op: &str) -> Value {
    loop {
        let frame = client.frame().await;
        if frame["op"] == op {
            return frame;
        }
    }
}

// ---------------------------------------------------------------- announcements and the sweep

/// Notification cases disable periodic repair, so delivery proves its source without a timing race.
const SWEEP_FAR_AWAY: Duration = Duration::ZERO;

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_row_that_is_announced_reaches_the_reader_at_once() {
    let rig = Rig::tuned(|tuning| tuning.sweep_every = SWEEP_FAR_AWAY).await;
    let mut client = Client::signed_in(rig.server.addr, &rig.token).await;
    client.sub(1, "user", None).await;
    assert_eq!(client.expect("subscribed").await["seq"], 0);

    let seq = write_to_inbox(&rig, rig.ada, "Done", Some(rig.topic)).await;
    let frame = next_of(&mut client, "sync").await;
    assert_eq!(
        (frame["prev"].as_i64(), frame["seq"].as_i64()),
        (Some(0), Some(seq))
    );
    let row = &frame["data"]["notifications"][0];
    assert_eq!(row["title"], "Done");
    assert_eq!(row["kind"], "milestone");
    assert_eq!(row["inbox_seq"], seq);
    assert_eq!(row["node"]["id"], rig.topic.to_string());
    assert_eq!(row["url"], format!("/topic/{}", rig.topic));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_notification_lost_between_commit_and_fan_out_is_repaired_by_the_next_sweep() {
    let rig = Rig::tuned(|tuning| tuning.sweep_every = Duration::ZERO).await;
    let mut client = Client::signed_in(rig.server.addr, &rig.token).await;
    client.sub(1, "user", None).await;
    client.expect("subscribed").await;

    // A row is written and numbered, and the announcement of it is never sent.
    let mut conn = rig.pool.acquire().await.unwrap();
    let seq: i64 = sqlx::query_scalar(
        "UPDATE users SET inbox_seq = inbox_seq + 1 WHERE id = $1 RETURNING inbox_seq",
    )
    .bind(rig.ada)
    .fetch_one(&mut *conn)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO notifications (id, user_id, kind, title, dedupe_key, inbox_seq)
         VALUES ($1, $2, 'milestone', 'Never announced', 'milestone:lost', $3)",
    )
    .bind(orch_types::new_id())
    .bind(rig.ada)
    .bind(seq)
    .execute(&mut *conn)
    .await
    .unwrap();
    rig.server.realtime.sweep().await.unwrap();
    let frame = next_of(&mut client, "sync").await;
    assert_eq!(
        frame["data"]["notifications"][0]["title"],
        "Never announced"
    );
    assert_eq!(frame["seq"], seq);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn two_server_instances_on_one_database_both_deliver() {
    let rig = Rig::tuned(|tuning| tuning.sweep_every = SWEEP_FAR_AWAY).await;
    let config = orch_config::ServerConfig::default();
    let mut slow = quick(&config);
    slow.sweep_every = SWEEP_FAR_AWAY;
    let second = Server::over(&rig.pool, &rig.clock, slow).await;

    let mut one = Client::signed_in(rig.server.addr, &rig.token).await;
    let mut two = Client::signed_in(second.addr, &rig.token).await;
    for client in [&mut one, &mut two] {
        client.sub(1, "user", None).await;
        client.expect("subscribed").await;
    }
    let seq = write_to_inbox(&rig, rig.ada, "For both", None).await;
    for client in [&mut one, &mut two] {
        let frame = next_of(client, "sync").await;
        assert_eq!(frame["seq"], seq);
        assert_eq!(frame["data"]["notifications"][0]["title"], "For both");
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_rows_of_a_thread_reach_a_reader_on_another_process_than_the_one_that_runs_the_turn() {
    let rig = Rig::new().await;
    let config = orch_config::ServerConfig::default();
    let second = Server::over(&rig.pool, &rig.clock, quick(&config)).await;
    let thread = rig.thread("Two processes").await;
    let loaded = rig.rows(thread).await.seq;
    let mut client = Client::signed_in(second.addr, &rig.token).await;
    client
        .sub(1, &format!("thread:{thread}"), Some(loaded))
        .await;
    client.expect("subscribed").await;
    client.expect("presence").await;

    let turn = rig.send(thread, "Hello").await;
    rig.begin_message(thread, &turn, "m1").await;
    rig.say(thread, message_grew("m1", 0, "Hi there.")).await;
    rig.finish_turn(thread, &turn, "m1", "Hi there.").await;

    let mut mirror = realtime_support::Mirror::loaded(loaded);
    let started = Instant::now();
    while !mirror
        .items
        .values()
        .any(|item| item["kind"] == "agent_message" && item["status"] == "completed")
    {
        assert!(started.elapsed() < Duration::from_secs(20));
        if let Some(realtime_support::Heard::Frame(frame)) =
            client.hear_within(Duration::from_millis(200)).await
        {
            mirror.take(&frame);
        }
    }
    // The growing text of the process that ingests it is handed over in that process only; the rows are the same everywhere.
    assert_eq!(mirror.text.values().next().unwrap(), "Hi there.");
    assert_eq!(mirror.gaps, 0);
    let stored = rig.texts(thread).await;
    assert_eq!(
        mirror.text.values().collect::<Vec<_>>(),
        stored.values().collect::<Vec<_>>()
    );
}

// ---------------------------------------------------------------- news that is no row

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn news_for_a_person_reaches_each_of_their_connections_on_every_process_and_nobody_else() {
    let rig = Rig::new().await;
    let config = orch_config::ServerConfig::default();
    let second = Server::over(&rig.pool, &rig.clock, quick(&config)).await;
    let (_, bobby_token) = rig.person("bobby").await;

    let mut here = Client::signed_in(rig.server.addr, &rig.token).await;
    let mut there = Client::signed_in(second.addr, &rig.token).await;
    let mut other = Client::signed_in(rig.server.addr, &bobby_token).await;
    for client in [&mut here, &mut there, &mut other] {
        client.sub(1, "user", None).await;
        client.expect("subscribed").await;
    }

    rig.server
        .realtime
        .tell_users(&[rig.ada], UserEvent::Favorites);
    for client in [&mut here, &mut there] {
        let frame = next_of(client, "event").await;
        assert_eq!(
            (frame["id"].as_i64(), frame["feed"].as_str()),
            (Some(1), Some("user"))
        );
        assert_eq!(frame["data"], json!({ "what": "favorites" }));
    }
    assert!(other.is_quiet_for(Duration::from_millis(500)).await);

    // The engine tells those who may decide an approval that there is one.
    let sink = rig.server.realtime.sink();
    sink.approvals_changed(&[rig.ada]);
    // The notice carries the person's summary of what waits for them, read after the commit, so
    // that their page asks nothing again.
    let frame = next_of(&mut here, "event").await;
    assert_eq!(frame["data"]["what"], "approvals");
    assert_eq!(frame["data"]["summary"]["approvals_pending"], 0, "{frame}");
}

// ---------------------------------------------------------------- who is looking

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn presence_lists_those_who_look_and_is_sent_at_most_once_a_second() {
    let rig = Rig::new().await;
    let thread = rig.thread("Watched").await;
    let (bo, bobby_token) = rig.person("bobby").await;
    rig.give(bo, "viewer").await;
    let feed = format!("thread:{thread}");

    let mut ada = Client::signed_in(rig.server.addr, &rig.token).await;
    ada.sub(1, &feed, None).await;
    ada.expect("subscribed").await;
    assert_eq!(
        ada.expect("presence").await["users"]
            .as_array()
            .unwrap()
            .len(),
        0
    );
    let mut bobby = Client::signed_in(rig.server.addr, &bobby_token).await;
    bobby.sub(1, &feed, None).await;
    bobby.expect("subscribed").await;
    bobby.expect("presence").await;

    // Ada looks; both are told, with who she is.
    ada.send(json!({ "op": "visible", "feed": feed, "visible": true }))
        .await;
    for client in [&mut ada, &mut bobby] {
        let frame = next_of(client, "presence").await;
        let users = frame["users"].as_array().unwrap();
        assert_eq!(users.len(), 1);
        assert_eq!(users[0]["user"]["id"], rig.ada.to_string());
        assert!(users[0]["since"].is_string());
    }

    // Switching back and forth many times is reported once a second at most, and with the list as it then stands.
    let started = Instant::now();
    for _ in 0..6 {
        bobby
            .send(json!({ "op": "visible", "feed": feed, "visible": true }))
            .await;
        bobby
            .send(json!({ "op": "visible", "feed": feed, "visible": false }))
            .await;
    }
    bobby
        .send(json!({ "op": "visible", "feed": feed, "visible": true }))
        .await;
    let mut frames = Vec::new();
    while started.elapsed() < Duration::from_millis(2_500) {
        if let Some(realtime_support::Heard::Frame(frame)) =
            ada.hear_within(Duration::from_millis(100)).await
            && frame["op"] == "presence"
        {
            frames.push((started.elapsed(), frame));
        }
    }
    assert!(
        !frames.is_empty() && frames.len() <= 3,
        "{} frames",
        frames.len()
    );
    let last = &frames.last().unwrap().1;
    assert_eq!(
        last["users"].as_array().unwrap().len(),
        2,
        "both look at the end"
    );
    // A person who looks in two tabs is one reader, and leaves when the last tab does.
    let mut second_tab = Client::signed_in(rig.server.addr, &bobby_token).await;
    second_tab.sub(5, &feed, None).await;
    second_tab.expect("subscribed").await;
    second_tab.expect("presence").await;
    second_tab
        .send(json!({ "op": "visible", "feed": feed, "visible": true }))
        .await;
    bobby
        .send(json!({ "op": "visible", "feed": feed, "visible": false }))
        .await;
    tokio::time::sleep(Duration::from_millis(1_300)).await;
    assert_eq!(rig.server.realtime.subscriptions(), 3);
    drop(second_tab);
    // The last tab went: bobby is not looking, and Ada is told.
    let mut latest = None;
    let started = Instant::now();
    while started.elapsed() < Duration::from_secs(5) {
        if let Some(realtime_support::Heard::Frame(frame)) =
            ada.hear_within(Duration::from_millis(200)).await
            && frame["op"] == "presence"
        {
            latest = Some(frame);
            if latest.as_ref().unwrap()["users"].as_array().unwrap().len() == 1 {
                break;
            }
        }
    }
    let latest = latest.expect("a list came");
    assert_eq!(latest["users"].as_array().unwrap().len(), 1);
    assert_eq!(latest["users"][0]["user"]["id"], rig.ada.to_string());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_reader_is_listed_only_for_a_feed_the_connection_follows() {
    let rig = Rig::new().await;
    let thread = rig.thread("Unfollowed").await;
    let mut client = Client::signed_in(rig.server.addr, &rig.token).await;
    // Looking at what is not followed counts for nothing.
    client
        .send(json!({ "op": "visible", "feed": format!("thread:{thread}"), "visible": true }))
        .await;
    client.sub(1, &format!("thread:{thread}"), None).await;
    client.expect("subscribed").await;
    assert_eq!(
        client.expect("presence").await["users"]
            .as_array()
            .unwrap()
            .len(),
        0
    );
    assert!(client.is_quiet_for(Duration::from_millis(1_200)).await);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn those_who_look_on_another_process_are_listed_too_and_forgotten_when_it_goes() {
    let rig = Rig::new().await;
    let config = orch_config::ServerConfig::default();
    let second = Server::over(&rig.pool, &rig.clock, quick(&config)).await;
    let thread = rig.thread("Across").await;
    let (bo, bobby_token) = rig.person("bobby").await;
    rig.give(bo, "viewer").await;
    let feed = format!("thread:{thread}");

    let mut ada = Client::signed_in(rig.server.addr, &rig.token).await;
    ada.sub(1, &feed, None).await;
    ada.until("presence").await;
    let mut bobby = Client::signed_in(second.addr, &bobby_token).await;
    bobby.sub(1, &feed, None).await;
    bobby.until("presence").await;

    ada.send(json!({ "op": "visible", "feed": feed, "visible": true }))
        .await;
    let frame = next_of(&mut bobby, "presence").await;
    assert_eq!(frame["users"][0]["user"]["id"], rig.ada.to_string());
    bobby
        .send(json!({ "op": "visible", "feed": feed, "visible": true }))
        .await;
    // The list that Ada was sent when she began to look comes first; then the one with both.
    let both = loop {
        let frame = next_of(&mut ada, "presence").await;
        if frame["users"].as_array().unwrap().len() == 2 {
            break frame;
        }
    };
    assert_eq!(
        both["users"][0]["user"]["id"],
        rig.ada.to_string(),
        "the longest first"
    );

    // When the connection that looks goes, the other process is told.
    drop(ada);
    let mut told = false;
    let started = Instant::now();
    while !told && started.elapsed() < Duration::from_secs(8) {
        if let Some(realtime_support::Heard::Frame(frame)) =
            bobby.hear_within(Duration::from_millis(200)).await
            && frame["op"] == "presence"
        {
            told = frame["users"].as_array().unwrap().len() == 1;
        }
    }
    assert!(told);
}

// ---------------------------------------------------------------- the tree of an organization

fn router(rig: &Rig) -> axum::Router {
    let mut registry = Registry::new();
    orch_api_core::routes::mount(&mut registry);
    orch_api_core::server::router(rig.server.app.clone(), registry)
}

/// What `GET /orgs/{id}/tree` answers a person, from a number.
async fn tree_over_http(rig: &Rig, token: &str, since: i64) -> OrgTreeGetReply {
    let request = Request::builder()
        .uri(format!("/api/v1/orgs/{}/tree?since={since}", rig.root))
        .header("authorization", format!("Bearer {token}"))
        .body(Body::empty())
        .unwrap();
    let response = router(rig).oneshot(request).await.unwrap();
    assert_eq!(response.status(), 200);
    let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    serde_json::from_slice(&bytes).unwrap()
}

async fn rename(rig: &Rig, node: Id, name: &str) -> i64 {
    let mut tx = rig.pool.begin().await.unwrap();
    sqlx::query("UPDATE nodes SET name = $2 WHERE id = $1")
        .bind(node)
        .bind(name)
        .execute(&mut *tx)
        .await
        .unwrap();
    let seq = hierarchy::stamp(&mut tx, node).await.unwrap();
    tx.commit().await.unwrap();
    seq
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_nodes_of_a_tree_that_changed_arrive_cut_down_to_what_the_subscriber_may_view_and_as_the_http_request_returns_them()
 {
    let rig = Rig::new().await;
    let (bo, bobby_token) = rig.person("bobby").await;
    let before = hierarchy::tree_seq(&mut rig.pool.acquire().await.unwrap(), rig.root)
        .await
        .unwrap();
    let mut ada = Client::signed_in(rig.server.addr, &rig.token).await;
    ada.sub(1, &format!("org:{}", rig.root), Some(before)).await;
    let subscribed = ada.expect("subscribed").await;
    assert_eq!(
        (subscribed["mode"].as_str(), subscribed["seq"].as_i64()),
        (Some("delta"), Some(before))
    );
    let mut bobby = Client::signed_in(rig.server.addr, &bobby_token).await;
    bobby
        .sub(1, &format!("org:{}", rig.root), Some(before))
        .await;
    bobby.expect("subscribed").await;

    let seq = rename(&rig, rig.topic, "Login page").await;
    assert!(seq > before);

    let ada_frame = next_of(&mut ada, "sync").await;
    assert_eq!(
        (ada_frame["prev"].as_i64(), ada_frame["seq"].as_i64()),
        (Some(before), Some(seq))
    );
    let nodes = ada_frame["data"]["nodes"].as_array().unwrap();
    assert_eq!(nodes.len(), 1);
    assert_eq!(nodes[0]["id"], rig.topic.to_string());
    assert_eq!(nodes[0]["name"], "Login page");
    assert_eq!(nodes[0]["deleted"], false);
    assert_eq!(nodes[0]["tree_seq"], seq);

    // The same node reaches the other person as they may see it: by name or as a row that says it is not theirs.
    let bobby_frame = next_of(&mut bobby, "sync").await;
    assert_eq!(bobby_frame["seq"], seq);
    let row = &bobby_frame["data"]["nodes"][0];
    assert_eq!(row["id"], rig.topic.to_string());
    assert!(
        (row["deleted"] == true && row["name"] == "")
            || (row["deleted"] == false && row["name"] == "Login page"),
        "{row}"
    );

    // And each is what the request for the tree from that number returns for them.
    for (token, frame) in [(&rig.token, &ada_frame), (&bobby_token, &bobby_frame)] {
        let http = tree_over_http(&rig, token, before).await;
        let over_http: Vec<(Id, String, bool)> = http
            .readable()
            .into_iter()
            .map(|node| (node.id, node.name.clone(), node.deleted))
            .collect();
        assert_eq!(over_http, nodes_of(frame));
        assert_eq!(http.seq, seq);
    }
    let _ = bo;
}

/// Waits until the tree's feed says that `thread` is in the state `wanted`, or the patience of the
/// test is spent, and then fails with the last state it said.
async fn thread_state_in_the_tree(client: &mut Client, thread: Id, wanted: &str) {
    let started = Instant::now();
    let mut last = String::from("no row");
    while started.elapsed() < realtime_support::PATIENCE {
        let Some(realtime_support::Heard::Frame(frame)) =
            client.hear_within(Duration::from_millis(200)).await
        else {
            continue;
        };
        if frame["op"] != "sync" {
            continue;
        }
        for node in frame["data"]["nodes"].as_array().unwrap() {
            if node["id"] == thread.to_string() {
                last = node["thread"]["state"].to_string();
                if node["thread"]["state"] == wanted {
                    return;
                }
            }
        }
    }
    panic!("the tree never said the thread was {wanted}; the last it said was {last}");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn those_who_follow_a_tree_are_told_what_each_thread_in_it_is_doing_as_a_turn_goes_through_its_states()
 {
    let rig = Rig::new().await;
    let thread = rig.thread("Fix the login").await;
    let before = hierarchy::tree_seq(&mut rig.pool.acquire().await.unwrap(), rig.root)
        .await
        .unwrap();
    let mut ada = Client::signed_in(rig.server.addr, &rig.token).await;
    ada.sub(1, &format!("org:{}", rig.root), Some(before)).await;
    ada.expect("subscribed").await;

    // A person says something and the harness takes the turn up: the thread is running.
    let turn = rig.send(thread, "Fix the login page").await;
    rig.begin_message(thread, &turn, "m1").await;
    thread_state_in_the_tree(&mut ada, thread, "running").await;

    // The turn ends: the thread waits for the next thing said.
    rig.finish_turn(thread, &turn, "m1", "Done.").await;
    thread_state_in_the_tree(&mut ada, thread, "idle").await;

    // What the feed said is what the request for the tree says.
    let over_http = tree_over_http(&rig, &rig.token, before).await;
    let row = over_http
        .nodes
        .iter()
        .filter_map(|node| node.as_visible())
        .find(|node| node.id == thread)
        .expect("the readable thread is in the tree");
    let in_the_row = row
        .thread
        .as_ref()
        .expect("a thread's row says what it does");
    assert_eq!(json!(in_the_row.state), json!("idle"));
}

fn nodes_of(frame: &Value) -> Vec<(Id, String, bool)> {
    frame["data"]["nodes"]
        .as_array()
        .unwrap()
        .iter()
        .map(|node| {
            (
                node["id"].as_str().unwrap().parse().unwrap(),
                node["name"].as_str().unwrap().to_owned(),
                node["deleted"].as_bool().unwrap(),
            )
        })
        .collect()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn only_the_feed_of_a_root_organization_that_is_there_can_be_followed() {
    let rig = Rig::new().await;
    let mut client = Client::signed_in(rig.server.addr, &rig.token).await;
    for (id, feed) in [
        (1, format!("org:{}", rig.project)),
        (2, format!("org:{}", orch_types::new_id())),
    ] {
        client.sub(id, &feed, None).await;
        let refused = client.expect("error").await;
        assert_eq!(
            (refused["id"].as_i64(), refused["code"].as_str()),
            (Some(id), Some("not_found"))
        );
    }
}

// ---------------------------------------------------------------- a topic

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_topic_follows_its_own_sequence_and_is_followed_by_those_who_may_view_it() {
    let rig = Rig::new().await;
    let (_, bobby_token) = rig.person("bobby").await;
    let feed = format!("topic:{}", rig.topic);
    let mut bobby = Client::signed_in(rig.server.addr, &bobby_token).await;
    bobby.sub(1, &feed, None).await;
    let refused = bobby.expect("error").await;
    assert_eq!(refused["code"], "forbidden");

    let mut ada = Client::signed_in(rig.server.addr, &rig.token).await;
    ada.sub(1, &feed, Some(0)).await;
    let subscribed = ada.expect("subscribed").await;
    assert_eq!(subscribed["mode"], "delta");
    let seq = rename(&rig, rig.topic, "Login, renamed").await;
    let frame = next_of(&mut ada, "sync").await;
    assert_eq!(frame["feed"], feed);
    assert_eq!(frame["seq"], seq);
    assert_eq!(frame["data"]["topic"]["name"], "Login, renamed");
    assert_eq!(frame["data"]["topic"]["seq"], frame["seq"]);
}
