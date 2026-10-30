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
