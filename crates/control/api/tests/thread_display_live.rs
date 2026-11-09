//! Live thread icons and read acknowledgements arrive through the existing feeds.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]

mod realtime_support;

use realtime_support::{Client, PATIENCE, Rig};
use serde_json::{Value, json};

type Proof<T = ()> = Result<T, Box<dyn std::error::Error>>;

async fn icon(client: &mut Client, state: &str) -> Proof<Value> {
    Ok(tokio::time::timeout(PATIENCE, async {
        loop {
            let frame = client.frame().await;
            if frame["op"] == "sync" && frame["data"]["thread"]["display"]["state"] == state {
                return frame;
            }
        }
    })
    .await?)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_live_thread_carries_its_icon_and_seeing_its_result_publishes_the_quiet_icon() -> Proof {
    let rig = Rig::new().await;
    let thread = rig.thread("Watch its icon").await;
    let before: i64 = sqlx::query_scalar("SELECT next_seq-1 FROM threads WHERE node_id=$1")
        .bind(thread)
        .fetch_one(rig.pool.pool())
        .await?;
    let mut client = Client::signed_in(rig.server.addr, &rig.token).await;
    client
        .sub(1, &format!("thread:{thread}"), Some(before))
        .await;
    client.expect("subscribed").await;
    client.expect("presence").await;
    client.sub(2, "user", None).await;
    client.expect("subscribed").await;
    let turn = rig.send(thread, "Work now").await;
    rig.begin_message(thread, &turn, "answer").await;
    icon(&mut client, "running").await?;
    rig.finish_turn(thread, &turn, "answer", "All done").await;
    let completed = icon(&mut client, "finished").await?;
    assert!(completed["data"]["thread"]["display"]["finished_at"].is_string());
    let response = reqwest::Client::new()
        .post(format!(
            "http://{}/api/v1/threads/{thread}/read",
            rig.server.addr
        ))
        .bearer_auth(&rig.token)
        .json(&json!({"seq":completed["seq"]}))
        .send()
        .await?;
    assert!(response.status().is_success(), "{}", response.text().await?);
    let notice = tokio::time::timeout(PATIENCE, async {
        loop {
            let frame = client.frame().await;
            if frame["op"] == "event" && frame["data"]["what"] == "continuation_read_state" {
                return frame;
            }
        }
    })
    .await?;
    assert_eq!(notice["data"]["thread_display"]["state"], "idle");
    rig.server.stop().await;
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_personal_read_notice_never_carries_state_past_the_recipients_current_view() -> Proof {
    use orch_api_contract::threads::{ThreadDisplay, ThreadDisplayState};
    use orch_realtime_contract::feeds::UserEvent;
    let rig = Rig::new().await;
    let thread = rig.thread("A private result").await;
    let (reader, token) = rig.person("reader").await;
    let role = rig.give(reader, "viewer").await;
    let mut client = Client::signed_in(rig.server.addr, &token).await;
    client.sub(1, "user", None).await;
    client.expect("subscribed").await;
    rig.take(role).await;
    let denied = reqwest::Client::new()
        .get(format!(
            "http://{}/api/v1/threads/{thread}/sync",
            rig.server.addr
        ))
        .bearer_auth(&token)
        .send()
        .await?;
    assert!(matches!(denied.status().as_u16(), 403 | 404));
    rig.server.realtime.tell_users(
        &[reader],
        UserEvent::ContinuationReadState {
            thread_id: thread,
            continuation_id: thread,
            last_read_seq: 1,
            thread_display: Some(ThreadDisplay {
                state: ThreadDisplayState::Finished,
                ..Default::default()
            }),
        },
    );
    let notice = tokio::time::timeout(PATIENCE, async {
        loop {
            let frame = client.frame().await;
            if frame["op"] == "event" && frame["data"]["what"] == "continuation_read_state" {
                return frame;
            }
        }
    })
    .await?;
    assert!(notice["data"].get("thread_display").is_none());
    rig.server.stop().await;
    Ok(())
}

fn reported_thread(thread: &Value, state: &str) {
    assert_eq!(thread["kind"], "thread");
    assert!(
        thread.get("topic_id").is_some(),
        "a full thread, not a name reference"
    );
    reported_display(&thread["display"], state);
}

fn reported_display(display: &Value, state: &str) {
    assert!(
        display.is_object(),
        "every full thread projection includes display"
    );
    assert_eq!(display["state"], state);
    for field in [
        "seq",
        "read_seq",
        "queued_inputs",
        "held_inputs",
        "waiting_inputs",
        "waiting_for_answer",
        "blocked_reason",
        "finished_at",
    ] {
        assert!(
            display.get(field).is_some(),
            "the presentation includes {field}"
        );
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn full_thread_http_and_live_readers_always_send_the_actual_display() -> Proof {
    let rig = Rig::new().await;
    let thread = rig.thread("Display across every full read").await;
    let turn = rig.send(thread, "Show running work").await;
    rig.begin_message(thread, &turn, "answer").await;
    let http = reqwest::Client::new();
    let paths = [
        (format!("nodes/{thread}"), ""),
        (format!("threads/{thread}/sync"), "/thread"),
        (format!("threads/{thread}/sync?since=0"), "/thread"),
        (
            format!("threads/{thread}/sync?continuation_id={thread}"),
            "/thread",
        ),
        (format!("threads/{thread}/opening"), "/sync/thread"),
    ];
    for (path, pointer) in paths {
        let request = http
            .get(format!("http://{}/api/v1/{path}", rig.server.addr))
            .bearer_auth(&rig.token);
        let response = request.send().await?;
        assert!(
            response.status().is_success(),
            "{path}: {}",
            response.text().await?
        );
        let body: Value = response.json().await?;
        reported_thread(
            body.pointer(pointer)
                .expect("the native full-thread location"),
            "running",
        );
    }
    let tree_request = http
        .get(format!(
            "http://{}/api/v1/orgs/{}/tree",
            rig.server.addr, rig.root
        ))
        .bearer_auth(&rig.token);
    let tree_response = tree_request.send().await?;
    assert!(
        tree_response.status().is_success(),
        "{}",
        tree_response.text().await?
    );
    let tree: Value = tree_response.json().await?;
    let row = tree["nodes"]
        .as_array()
        .expect("native tree rows")
        .iter()
        .find(|row| row["id"] == thread.to_string())
        .expect("the real thread is in its org tree");
    reported_display(&row["thread"]["display"], "running");

    let mut live = Client::signed_in(rig.server.addr, &rig.token).await;
    // Request the existing rows explicitly; a cursorless subscription only follows future changes.
    live.sub(1, &format!("thread:{thread}"), Some(0)).await;
    let subscribed = live.expect("subscribed").await;
    assert_eq!(subscribed["mode"], "delta");
    assert_eq!(subscribed["seq"], 0);
    live.expect("presence").await;
    let snapshot = icon(&mut live, "running").await?;
    assert_eq!(snapshot["prev"], 0);
    reported_thread(&snapshot["data"]["thread"], "running");
    let mut org = Client::signed_in(rig.server.addr, &rig.token).await;
    org.sub(1, &format!("org:{}", rig.root), Some(0)).await;
    let subscribed = org.expect("subscribed").await;
    assert_eq!(subscribed["mode"], "delta");
    assert_eq!(subscribed["seq"], 0);
    let row = tokio::time::timeout(PATIENCE, async {
        loop {
            let frame = org.frame().await;
            if let Some(row) = frame["data"]["nodes"]
                .as_array()
                .and_then(|rows| rows.iter().find(|row| row["id"] == thread.to_string()))
            {
                assert_eq!(frame["prev"], 0);
                break row.clone();
            }
        }
    })
    .await?;
    reported_display(&row["thread"]["display"], "running");
    rig.server.stop().await;
    Ok(())
}
