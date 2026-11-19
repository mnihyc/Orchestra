//! A person's feed carries their Inbox live: a row that is written reaches an open screen with the two counters of the badge, and an approval that waits for them is counted in the second.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]

mod realtime_support;

use orch_agent_contract::events::{
    ApprovalOption, HarnessRequest, HarnessRequestDetail, HarnessRequestKind, OptionScope,
    OptionStyle,
};
use orch_db::queries::notifications::inbox::{ReadFilter, mark_read};
use orch_db::queries::notifications::people::{NewNotification, announce};
use orch_link_contract::events::{ApprovalRequested, Event};
use realtime_support::{Client, Rig};
use serde_json::{Value, json};
use std::time::Duration;

/// Waits for the next frame of `op` among those that come.
async fn next_of(client: &mut Client, op: &str) -> Value {
    tokio::time::timeout(realtime_support::PATIENCE, async {
        loop {
            let frame = client.frame().await;
            if frame["op"] == op {
                return frame;
            }
        }
    })
    .await
    .expect("the requested feed frame arrives within the test's bound")
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_row_in_the_inbox_reaches_an_open_screen_with_the_counters_of_the_badge() {
    let rig = Rig::tuned(|tuning| tuning.sweep_every = Duration::from_secs(60)).await;
    let thread = rig.thread("Inbox thread").await;
    let mut client = Client::signed_in(rig.server.addr, &rig.token).await;
    client.sub(1, "user", None).await;
    assert_eq!(client.expect("subscribed").await["seq"], 0);

    for (title, expected_unread) in [("Done", 1), ("Done again", 2)] {
        let mut tx = rig.pool.begin().await.unwrap();
        announce(
            &mut tx,
            &NewNotification {
                user_id: rig.ada,
                kind: "milestone",
                node_id: Some(rig.topic),
                thread_id: Some(thread),
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
        let frame = next_of(&mut client, "sync").await;
        assert_eq!(frame["data"]["notifications"][0]["title"], title);
        assert_eq!(
            frame["data"]["notifications"][0]["thread"]["id"],
            thread.to_string()
        );
        assert!(
            frame["data"]["notifications"][0]["thread"]["name"]
                .as_str()
                .is_some_and(|name| !name.is_empty())
        );
        assert!(
            frame["data"]["notifications"][0]["path"]
                .as_array()
                .is_some_and(|path| path
                    .last()
                    .is_some_and(|node| node["id"] == thread.to_string()))
        );
        assert_eq!(
            frame["data"]["summary"],
            json!({ "unread": expected_unread, "approvals_pending": 0 })
        );
    }

    // Reading a row sends it again, read, and the counter goes down.
    let mut conn = rig.pool.acquire().await.unwrap();
    mark_read(
        &mut conn,
        rig.ada,
        &ReadFilter {
            node: Some(rig.topic),
            ..ReadFilter::default()
        },
        rig.clock.now(),
    )
    .await
    .unwrap();
    drop(conn);
    let frame = next_of(&mut client, "sync").await;
    assert_eq!(
        frame["data"]["summary"],
        json!({ "unread": 0, "approvals_pending": 0 })
    );
    assert!(frame["data"]["notifications"][0]["read_at"].is_string());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_approval_that_waits_for_the_person_is_counted_in_the_second_counter() {
    let rig = Rig::tuned(|tuning| tuning.sweep_every = Duration::from_secs(60)).await;
    let thread = rig.thread("Fix login").await;
    let mut client = Client::signed_in(rig.server.addr, &rig.token).await;
    client.sub(1, "user", None).await;
    client.expect("subscribed").await;

    let turn = rig.send(thread, "Clean up").await;
    rig.say(thread, realtime_support::turn_began(turn.turn_id))
        .await;
    rig.say(
        thread,
        Event::ApprovalRequested(ApprovalRequested {
            turn_id: turn.turn_id,
            request_ref: "r1".to_owned(),
            title: "Run a command".to_owned(),
            request: HarnessRequest {
                request_kind: HarnessRequestKind::Command,
                detail: HarnessRequestDetail::Command {
                    command: "rm -rf build".to_owned(),
                    cwd: "/workspace/web".to_owned(),
                    reason: None,
                },
                options: vec![
                    ApprovalOption {
                        id: "allow_once".to_owned(),
                        label: "Allow once".to_owned(),
                        style: OptionStyle::Allow,
                        scope: Some(OptionScope::Once),
                        rule: None,
                    },
                    ApprovalOption {
                        id: "deny".to_owned(),
                        label: "Deny".to_owned(),
                        style: OptionStyle::Deny,
                        scope: None,
                        rule: None,
                    },
                ],
            },
        }),
    )
    .await;

    let frame = next_of(&mut client, "sync").await;
    let row = &frame["data"]["notifications"][0];
    assert_eq!(row["kind"], "approval.requested");
    assert_eq!(row["thread_id"], json!(thread));
    assert_eq!(
        frame["data"]["summary"],
        json!({ "unread": 1, "approvals_pending": 1 }),
        "the thread's owner can decide what its agent asks"
    );
    let approval = row["approval_id"].as_str().unwrap().parse().unwrap();
    let notice = row["id"].as_str().unwrap().parse().unwrap();
    // Opening reads the notice, but leaves the actual approval pending.
    let mut conn = rig.pool.acquire().await.unwrap();
    mark_read(
        &mut conn,
        rig.ada,
        &ReadFilter {
            ids: &[notice],
            ..ReadFilter::default()
        },
        rig.clock.now(),
    )
    .await
    .unwrap();
    drop(conn);
    let opened = next_of(&mut client, "sync").await;
    assert_eq!(opened["data"]["summary"]["approvals_pending"], 1);
    rig.server
        .engine
        .decide_approval(
            &orch_authz::Caller::session(rig.ada),
            approval,
            &orch_api_contract::approvals::ApprovalDecision::Option(
                orch_api_contract::approvals::HarnessOptionDecision {
                    option_id: "allow_once".to_owned(),
                    note: None,
                },
            ),
        )
        .await
        .unwrap();
    // No unread row is needed: the event itself supplies authoritative session counters.
    let (settled, updated) = tokio::time::timeout(realtime_support::PATIENCE, async {
        let mut settled = None;
        let mut updated = None;
        loop {
            let event = client.frame().await;
            if event["op"] == "event"
                && event["data"]["what"] == "approvals"
                && event["data"]["summary"]["approvals_pending"] == 0
            {
                settled = Some(event.clone());
            }
            if event["op"] == "sync"
                && let Some(rows) = event["data"]["notifications"].as_array()
                && let Some(row) = rows.iter().find(|row| {
                    row["id"] == json!(notice) && row["data"]["approval_state"] == "approved"
                })
            {
                updated = Some(row.clone());
            }
            if let (Some(settled), Some(updated)) = (&settled, &updated) {
                break (settled.clone(), updated.clone());
            }
        }
    })
    .await
    .expect("the committed decision supplies the current session's counters");
    assert_eq!(settled["data"]["summary"]["unread"], 0);
    assert_eq!(
        updated["id"],
        json!(notice),
        "the request changes in place, with no second notice"
    );
    assert_eq!(updated["actor"]["id"], json!(rig.ada));
    assert_eq!(
        updated["details"]["parameters"]["approval_state"],
        "approved"
    );
    assert_eq!(
        updated["details"]["parameters"]["decided_by"],
        json!(rig.ada)
    );
    assert!(updated["read_at"].is_string());
    assert_eq!(
        updated["read_at"], opened["data"]["notifications"][0]["read_at"],
        "the original read time stays"
    );
    assert_eq!(updated["waiting_for_me"], false);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn visible_thread_frames_quiet_only_the_authors_success_and_leaving_restores_delivery() {
    let rig = Rig::tuned(|tuning| tuning.sweep_every = Duration::from_secs(60)).await;
    let thread = rig.thread("Finish the change").await;
    let feed = format!("thread:{thread}");
    let mut viewer = Client::signed_in(rig.server.addr, &rig.token).await;
    viewer.sub(1, &feed, None).await;
    viewer.expect("subscribed").await;
    viewer.expect("presence").await;
    viewer
        .send(json!({"op":"visible", "feed":feed, "visible":true}))
        .await;
    let present = next_of(&mut viewer, "presence").await;
    assert_eq!(present["users"][0]["user"]["id"], json!(rig.ada));

    let turn = rig.send(thread, "Finish it").await;
    rig.begin_message(thread, &turn, "visible-reply").await;
    rig.finish_turn(thread, &turn, "visible-reply", "Finished here.")
        .await;
    realtime_support::eventually(
        "the visible completion's durable notification decision",
        || async {
            sqlx::query_scalar::<_, bool>("SELECT end_work IS NULL FROM turns WHERE id=$1")
                .bind(turn.turn_id)
                .fetch_one(rig.pool.pool())
                .await
                .unwrap()
        },
    )
    .await;
    let count = |kind: &'static str| {
        let pool = &rig.pool;
        let person = rig.ada;
        async move {
            sqlx::query_scalar::<_, i64>(
                "SELECT count(*) FROM notifications WHERE user_id=$1 AND thread_id=$2 AND kind=$3",
            )
            .bind(person)
            .bind(thread)
            .bind(kind)
            .fetch_one(pool.pool())
            .await
            .unwrap()
        }
    };
    assert_eq!(
        count("turn.completed").await,
        0,
        "a live visible frame reaches the actual engine sink"
    );

    viewer
        .send(json!({"op":"visible", "feed":feed, "visible":false}))
        .await;
    let absent = next_of(&mut viewer, "presence").await;
    assert_eq!(absent["users"].as_array().unwrap().len(), 0);
    let away = rig.send(thread, "Another change").await;
    rig.begin_message(thread, &away, "away-reply").await;
    rig.finish_turn(thread, &away, "away-reply", "Ready for your return.")
        .await;
    realtime_support::eventually("the away completion's durable notification", || async {
        sqlx::query_scalar::<_, bool>("SELECT end_work IS NULL FROM turns WHERE id=$1")
            .bind(away.turn_id)
            .fetch_one(rig.pool.pool())
            .await
            .unwrap()
    })
    .await;
    assert_eq!(
        count("turn.completed").await,
        1,
        "a subscription without visibility is not watching"
    );
}
