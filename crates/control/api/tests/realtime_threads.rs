//! A thread's feed on a real socket, with the thread engine and a scripted machine behind it: items in order while a turn runs, what a reader that came back is sent, deltas and the rows that catch up with them.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]

mod realtime_support;

use orch_engine::live::LiveDelta;
use orch_link_contract::commands::TurnStart;
use orch_link_contract::events::DeltaField;
use orch_types::Id;
use realtime_support::{Client, Mirror, Rig, Server, SlowLine, message_grew, quick};
use serde_json::json;
use std::time::{Duration, Instant};

/// Takes in frames until the mirror holds what `done` asks for.
async fn follow(client: &mut Client, mirror: &mut Mirror, done: impl Fn(&Mirror) -> bool) {
    let started = std::time::Instant::now();
    while !done(mirror) {
        assert!(
            started.elapsed() < realtime_support::PATIENCE,
            "gave up waiting; the mirror holds {mirror:?}"
        );
        if let Some(realtime_support::Heard::Frame(frame)) =
            client.hear_within(Duration::from_millis(200)).await
        {
            mirror.take(&frame);
        }
    }
}

fn feed(thread: Id) -> String {
    format!("thread:{thread}")
}

/// Model a browser's advertised heartbeat during a held-domain-clock jump.
/// Each pong is observed before advancing again; intervening decision/state
/// frames stay in the client's queue for progress(), rather than being lost.
async fn pass_time_with_readers(rig: &Rig, by: chrono::Duration, readers: &mut [&mut Client]) {
    let mut left = by;
    let heartbeat =
        chrono::Duration::seconds(i64::from(orch_realtime_contract::version::HEARTBEAT_SECS));
    while left > chrono::Duration::zero() {
        let step = left.min(heartbeat);
        rig.let_time_pass(step);
        for reader in readers.iter_mut() {
            reader.heartbeat().await;
        }
        left -= step;
    }
}

struct ExpectedProgress<'a> {
    state: &'a str,
    turn: Id,
    turn_state: &'a str,
    waited_ms: i64,
    /// The request and how it settled: a question is answered, a request for an action approved.
    approval: Option<(Id, &'a str)>,
}

/// Observe the committed facts without repairing the page over HTTP or reconnecting it.
async fn progress(
    client: &mut Client,
    rig: &Rig,
    thread_id: Id,
    page: &str,
    wanted: &ExpectedProgress<'_>,
) -> serde_json::Value {
    let (mut thread, mut turn): (Option<serde_json::Value>, Option<serde_json::Value>) =
        (None, None);
    let (mut approval, mut notice) = (wanted.approval.is_none(), wanted.approval.is_none());
    let mut card = None;
    let mut frames = 0usize;
    let mut last_op = None;
    let started = Instant::now();
    loop {
        if started.elapsed() >= realtime_support::PATIENCE {
            let state = thread
                .as_ref()
                .map(|row| (&row["state"], &row["pending_approvals"], &row["seq"]));
            let timing = turn.as_ref().map(|row| {
                (
                    &row["state"],
                    &row["waited_ms"],
                    &row["waiting_since"],
                    &row["seq_updated"],
                )
            });
            eprintln!(
                "decision socket page={page} wanted={}/{}/{} approval={approval} notice={notice} frames={frames} last_op={last_op:?} state={state:?} timing={timing:?} card={card:?}",
                wanted.state, wanted.turn_state, wanted.waited_ms
            );
            rig.native_diagnostics(thread_id, "decision socket progress did not converge")
                .await;
            panic!(
                "the decision/progress did not reach the {page} open page within {:?}",
                realtime_support::PATIENCE
            );
        }
        let frame = match client.hear_within(Duration::from_millis(200)).await {
            Some(realtime_support::Heard::Frame(frame)) => frame,
            Some(realtime_support::Heard::Closed(code)) => {
                rig.native_diagnostics(
                    thread_id,
                    "decision socket closed before progress converged",
                )
                .await;
                panic!(
                    "the {page} decision socket closed with {code:?}; approval={approval} notice={notice} frames={frames} last_op={last_op:?} card={card:?}"
                );
            }
            None => continue,
        };
        frames += 1;
        last_op = frame["op"].as_str().map(str::to_owned);
        if frame["op"] == "event" && frame["data"]["what"] == "approvals" {
            notice = true;
        }
        if frame["op"] == "sync" {
            let data = &frame["data"];
            if data["thread"].is_object() {
                thread = Some(data["thread"].clone());
            }
            if let Some(row) = data["turns"]
                .as_array()
                .into_iter()
                .flatten()
                .find(|row| row["id"] == wanted.turn.to_string())
            {
                turn = Some(row.clone());
            }
            if let Some((id, settled)) = wanted.approval {
                if let Some(item) = data["items"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .find(|item| item["payload"]["approval_id"] == id.to_string())
                {
                    card = Some(
                        json!({"id":id,"status":item["status"],"state":item["payload"]["approval"]["state"],
                        "projected":item["payload"]["approval"].is_object(),"seq":item["seq_updated"]}),
                    );
                }
                approval |= data["items"].as_array().into_iter().flatten().any(|item| {
                    item["payload"]["approval_id"] == id.to_string()
                        && item["status"] == settled
                        && item["payload"]["approval"]["state"] == settled
                });
            }
        }
        if notice
            && approval
            && thread
                .as_ref()
                .is_some_and(|row| row["state"] == wanted.state && row["pending_approvals"] == 0)
            && let Some(turn) = turn.as_ref().filter(|row| {
                row["state"] == wanted.turn_state
                    && row["waited_ms"] == wanted.waited_ms
                    && row["waiting_since"].is_null()
            })
        {
            return turn.clone();
        }
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn allow_once_and_answers_reach_every_open_page_and_human_waits_do_not_become_running_time() {
    use orch_api_contract::threads::ItemPayload;
    use orch_link_contract::events::{ApprovalRequested, Event};
    let rig = Rig::with_time_held().await;
    let second = Server::over(
        &rig.pool,
        &rig.clock,
        quick(&orch_config::ServerConfig::default()),
    )
    .await;
    let thread = rig.thread("Human decisions are live").await;
    let http = reqwest::Client::new();
    for question in [false, true] {
        let start = rig.send(thread, "Work until a person answers").await;
        let result_ref = format!("decision-result-{}", start.turn_id);
        rig.begin_message(thread, &start, &result_ref).await;
        rig.let_time_pass(chrono::Duration::seconds(5));
        let request = if question {
            json!({"request_kind":"user_input","options":[],"detail":{"questions":[
                {"id":"scope","header":"Scope","question":"Which scope?","multiple":false,"options":null,"allow_other":true}
            ]}})
        } else {
            json!({"request_kind":"command","detail":{"command":"./deploy staging","cwd":"/workspace","reason":null},
            "options":[{"id":"allow_once","label":"Allow once","style":"allow","scope":"once"}]})
        };
        rig.say(
            thread,
            Event::ApprovalRequested(ApprovalRequested {
                turn_id: start.turn_id,
                request_ref: format!("decision-{}", start.turn_id),
                title: "A person's decision".into(),
                request: serde_json::from_value(request).unwrap(),
            }),
        )
        .await;
        realtime_support::eventually("the pending human request", || async {
            rig.rows(thread).await.thread.state
                == orch_api_contract::threads::ThreadState::WaitingApproval
        })
        .await;
        let before = rig.rows(thread).await;
        let approval = before
            .items
            .iter()
            .find_map(|item| match &item.payload {
                ItemPayload::Approval(payload) if item.turn_id == Some(start.turn_id) => {
                    Some(payload.approval_id)
                }
                _ => None,
            })
            .unwrap();
        let waiting = before
            .turns
            .iter()
            .find(|turn| turn.id == start.turn_id)
            .unwrap();
        assert!(waiting.waiting_since.is_some());
        assert_eq!(waiting.waited_ms, 0);
        assert!(
            rig.server
                .engine
                .eligible_deciders(thread)
                .await
                .unwrap()
                .contains(&rig.ada),
            "the actual thread manager must be eligible for this request's personal event"
        );
        let mut local = Client::signed_in(rig.server.addr, &rig.token).await;
        let mut remote = Client::signed_in(second.addr, &rig.token).await;
        for client in [&mut local, &mut remote] {
            client.sub(1, &feed(thread), Some(before.seq)).await;
            client.expect("subscribed").await;
            client.expect("presence").await;
            client.sub(2, "user", None).await;
            client.expect("subscribed").await;
        }
        pass_time_with_readers(
            &rig,
            chrono::Duration::minutes(10),
            &mut [&mut local, &mut remote],
        )
        .await;
        let answer = if question {
            json!({"answers":{"scope":["Login"]},"note":null})
        } else {
            json!({"option_id":"allow_once","note":null})
        };
        let answered = http
            .post(format!(
                "http://{}/api/v1/approvals/{approval}/decision",
                rig.server.addr
            ))
            .bearer_auth(&rig.token)
            .json(&answer)
            .send()
            .await
            .unwrap();
        assert!(
            answered.status().is_success(),
            "{}",
            answered.text().await.unwrap()
        );
        let decided: serde_json::Value = answered.json().await.unwrap();
        assert_eq!(decided["id"], approval.to_string());
        let settled = if question { "answered" } else { "approved" };
        assert_eq!(decided["state"], settled);
        let resumed = ExpectedProgress {
            state: "running",
            turn: start.turn_id,
            turn_state: "running",
            waited_ms: 600_000,
            approval: Some((approval, settled)),
        };
        for (page, client) in [("local", &mut local), ("remote", &mut remote)] {
            progress(client, &rig, thread, page, &resumed).await;
        }
        rig.let_time_pass(chrono::Duration::seconds(7));
        rig.finish_turn(thread, &start, &result_ref, "Done after the decision")
            .await;
        let ended = ExpectedProgress {
            state: "idle",
            turn: start.turn_id,
            turn_state: "completed",
            waited_ms: 600_000,
            approval: None,
        };
        for (page, client) in [("local", &mut local), ("remote", &mut remote)] {
            let turn = progress(client, &rig, thread, page, &ended).await;
            let started: chrono::DateTime<chrono::Utc> =
                turn["started_at"].as_str().unwrap().parse().unwrap();
            let completed: chrono::DateTime<chrono::Utc> =
                turn["completed_at"].as_str().unwrap().parse().unwrap();
            assert_eq!(
                (completed - started).num_milliseconds() - turn["waited_ms"].as_i64().unwrap(),
                12_000
            );
        }
    }
    second.stop().await;
    rig.server.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn overlapping_requests_pause_once_and_a_withdrawal_closes_the_remaining_wait() {
    use orch_link_contract::events::{ApprovalClosed, ApprovalRequested, Event};
    let rig = Rig::with_time_held().await;
    let thread = rig.thread("Overlapping human waits").await;
    let start = rig.send(thread, "Wait for two decisions").await;
    rig.begin_message(thread, &start, "overlapping-result")
        .await;
    rig.let_time_pass(chrono::Duration::seconds(5));
    for (request_ref, advance) in [("first", 2), ("second", 3)] {
        rig.say(thread,Event::ApprovalRequested(ApprovalRequested {
            turn_id:start.turn_id,request_ref:request_ref.into(),title:request_ref.into(),
            request:serde_json::from_value(json!({"request_kind":"command","detail":{"command":"./check","cwd":"/workspace","reason":null},
                "options":[{"id":"allow_once","label":"Allow once","style":"allow","scope":"once"}]})).unwrap(),
        })).await;
        realtime_support::eventually("the requested approval", || async {
            let mut conn = rig.pool.acquire().await.unwrap();
            orch_db::queries::approvals::harness_approval_by_ref(&mut conn, thread, request_ref)
                .await
                .unwrap()
                .is_some()
        })
        .await;
        rig.let_time_pass(chrono::Duration::minutes(advance));
    }
    let before = rig.rows(thread).await;
    let first_wait = before
        .turns
        .iter()
        .find(|turn| turn.id == start.turn_id)
        .unwrap()
        .waiting_since;
    let approval = {
        let mut conn = rig.pool.acquire().await.unwrap();
        orch_db::queries::approvals::harness_approval_by_ref(&mut conn, thread, "first")
            .await
            .unwrap()
            .unwrap()
            .id
    };
    let answer = reqwest::Client::new()
        .post(format!(
            "http://{}/api/v1/approvals/{approval}/decision",
            rig.server.addr
        ))
        .bearer_auth(&rig.token)
        .json(&json!({"option_id":"allow_once","note":null}))
        .send()
        .await
        .unwrap();
    assert!(answer.status().is_success());
    let partial = rig.rows(thread).await;
    assert_eq!(partial.thread.pending_approvals, 1);
    assert_eq!(
        partial.thread.state,
        orch_api_contract::threads::ThreadState::WaitingApproval
    );
    let turn = partial
        .turns
        .iter()
        .find(|turn| turn.id == start.turn_id)
        .unwrap();
    assert_eq!(turn.waiting_since, first_wait);
    assert_eq!(turn.waited_ms, 0);
    rig.let_time_pass(chrono::Duration::minutes(4));
    rig.say(
        thread,
        Event::ApprovalClosed(ApprovalClosed {
            request_ref: "second".into(),
        }),
    )
    .await;
    realtime_support::eventually("withdrawal ends the human wait", || async {
        rig.rows(thread).await.thread.state == orch_api_contract::threads::ThreadState::Running
    })
    .await;
    let resumed = rig.rows(thread).await;
    let turn = resumed
        .turns
        .iter()
        .find(|turn| turn.id == start.turn_id)
        .unwrap();
    assert!(turn.waiting_since.is_none());
    assert_eq!(turn.waited_ms, 540_000);
    rig.let_time_pass(chrono::Duration::seconds(7));
    rig.finish_turn(thread, &start, "overlapping-result", "Done")
        .await;
    let ended = rig.rows(thread).await;
    let turn = ended
        .turns
        .iter()
        .find(|turn| turn.id == start.turn_id)
        .unwrap();
    assert_eq!(
        (turn.completed_at.unwrap().get() - turn.started_at.unwrap().get()).num_milliseconds()
            - turn.waited_ms,
        12_000
    );
    rig.server.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn pending_input_edits_delivery_order_and_removal_arrive_with_the_queue_revision_on_the_live_feed()
 {
    use orch_api_contract::threads::{ThreadInput, ThreadInputCreateReply, ThreadInputUpdateReply};
    let rig = Rig::new().await;
    let second_server = Server::over(
        &rig.pool,
        &rig.clock,
        quick(&orch_config::ServerConfig::default()),
    )
    .await;
    let thread = rig.thread("A live waiting queue").await;
    sqlx::query("UPDATE provider_models SET enabled = false WHERE provider_id = $1")
        .bind(rig.provider)
        .execute(rig.pool.pool())
        .await
        .unwrap();
    let http = reqwest::Client::new();
    let url = format!("http://{}/api/v1/threads/{thread}/inputs", rig.server.addr);
    let mut inputs = Vec::new();
    for words in ["First", "Second"] {
        let response = http.post(&url).bearer_auth(&rig.token)
            .json(&json!({ "content": [{ "type": "text", "text": words }], "delivery": "queue", "client_id": words }))
            .send().await.unwrap();
        assert!(response.status().is_success());
        inputs.push(
            response
                .json::<ThreadInputCreateReply>()
                .await
                .unwrap()
                .input,
        );
    }
    let first = inputs[0].id;
    let second = inputs[1].id;
    let before = rig.rows(thread).await;
    let mut revision = before.thread.queue.revision;
    let mut clients = [
        Client::signed_in(rig.server.addr, &rig.token).await,
        Client::signed_in(second_server.addr, &rig.token).await,
    ];
    for client in &mut clients {
        client.sub(1, &feed(thread), Some(before.seq)).await;
        client.expect("subscribed").await;
        client.expect("presence").await;
    }

    // Wait for each committed row before making the next edit: a coalesced later answer
    // must not conceal an omitted content, order, delivery or cancellation update.
    let changes = [
        Some(json!({ "content": [{ "type": "text", "text": "First, edited" }] })),
        Some(json!({ "after_input_id": second })),
        Some(json!({ "delivery": "steer" })),
        Some(json!({ "delivery": "interrupt" })),
        Some(json!({ "delivery": "queue" })),
        None,
    ];
    for change in changes {
        let updating = change.is_some();
        let response = match change {
            Some(body) => http
                .patch(format!("{url}/{first}"))
                .bearer_auth(&rig.token)
                .json(&body)
                .send()
                .await
                .unwrap(),
            None => http
                .delete(format!("{url}/{first}"))
                .bearer_auth(&rig.token)
                .send()
                .await
                .unwrap(),
        };
        assert!(response.status().is_success());
        let changed = if updating {
            response
                .json::<ThreadInputUpdateReply>()
                .await
                .unwrap()
                .inputs
        } else {
            vec![response.json::<ThreadInput>().await.unwrap()]
        };
        assert!(!changed.is_empty());
        let current = rig.rows(thread).await.thread.queue.revision;
        assert!(
            current > revision,
            "each changed queue has a new resume revision"
        );
        revision = current;
        let expected = changed
            .iter()
            .map(|input| serde_json::to_value(input).unwrap())
            .collect::<Vec<_>>();
        for client in &mut clients {
            let started = Instant::now();
            loop {
                assert!(
                    started.elapsed() < realtime_support::PATIENCE,
                    "the whole input update did not reach the socket: {expected:?}"
                );
                let Some(realtime_support::Heard::Frame(frame)) =
                    client.hear_within(Duration::from_millis(200)).await
                else {
                    continue;
                };
                if frame["op"] != "sync" {
                    continue;
                }
                let rows = frame["data"]["inputs"].as_array();
                if !expected.iter().all(|wanted| {
                    rows.into_iter().flatten().any(|row| {
                        row["id"] == wanted["id"] && row["seq_updated"] == wanted["seq_updated"]
                    })
                }) {
                    continue;
                }
                for wanted in &expected {
                    let row = rows
                        .into_iter()
                        .flatten()
                        .find(|row| row["id"] == wanted["id"])
                        .unwrap();
                    for field in [
                        "content",
                        "delivery",
                        "position",
                        "state",
                        "queue_held",
                        "seq_updated",
                    ] {
                        assert_eq!(
                            row[field], wanted[field],
                            "every changed neighbor keeps {field}"
                        );
                    }
                }
                assert!(
                    frame["data"]["thread"]["queue"]["revision"]
                        .as_i64()
                        .unwrap()
                        >= revision
                );
                assert_eq!(frame["data"]["thread"]["queue"]["held"], false);
                break;
            }
        }
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_subscriber_gets_a_threads_items_in_order_while_a_turn_runs() {
    let rig = Rig::new().await;
    let thread = rig.thread("Fix the login").await;
    let loaded = rig.rows(thread).await.seq;

    let mut client = Client::signed_in(rig.server.addr, &rig.token).await;
    client.sub(1, &feed(thread), Some(loaded)).await;
    let subscribed = client.expect("subscribed").await;
    assert_eq!(subscribed["mode"], "delta");
    assert_eq!(subscribed["seq"], loaded);
    let presence = client.expect("presence").await;
    assert_eq!(presence["users"].as_array().unwrap().len(), 0);

    let turn = rig.send(thread, "Fix the login page").await;
    rig.begin_message(thread, &turn, "m1").await;
    rig.say(thread, message_grew("m1", 0, "I'll start by read"))
        .await;
    rig.say(thread, message_grew("m1", 18, "ing the handler."))
        .await;
    rig.finish_turn(thread, &turn, "m1", "I'll start by reading the handler.")
        .await;

    let mut mirror = Mirror::loaded(loaded);
    let expected = "I'll start by reading the handler.";
    follow(&mut client, &mut mirror, |held| {
        held.text.values().any(|text| text == expected)
            && held
                .items
                .values()
                .any(|item| item["kind"] == "agent_message" && item["status"] == "completed")
    })
    .await;

    // The user's message came before the agent's, in the order they were made.
    let mut said: Vec<(i64, String)> = mirror
        .items
        .values()
        .filter(|item| {
            matches!(
                item["kind"].as_str(),
                Some("user_message" | "agent_message")
            )
        })
        .map(|item| {
            (
                item["seq_created"].as_i64().unwrap(),
                item["kind"].as_str().unwrap().to_owned(),
            )
        })
        .collect();
    said.sort();
    let kinds: Vec<&str> = said.iter().map(|(_, kind)| kind.as_str()).collect();
    assert_eq!(kinds, ["user_message", "agent_message"]);

    // No frame skipped a stretch of the sequence: each begins where the last ended.
    assert!(!mirror.seqs.is_empty());
    let mut last = loaded;
    for (prev, seq) in &mirror.seqs {
        assert!(
            *prev <= last,
            "a frame began at {prev} after {last}: a hole"
        );
        assert!(*seq > *prev, "a frame ends where it began");
        last = last.max(*seq);
    }
    assert_eq!(mirror.resyncs, 0);

    // What the client built is what the server holds.
    let held = rig.texts(thread).await;
    assert_eq!(
        mirror.text.values().collect::<Vec<_>>(),
        held.values().collect::<Vec<_>>()
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn deltas_arrive_and_the_stored_text_catches_up_with_the_same_content() {
    let rig = Rig::new().await;
    let thread = rig.thread("Stream").await;
    let loaded = rig.rows(thread).await.seq;
    let mut client = Client::signed_in(rig.server.addr, &rig.token).await;
    client.sub(1, &feed(thread), Some(loaded)).await;
    client.expect("subscribed").await;
    client.expect("presence").await;

    let turn = rig.send(thread, "Say something long").await;
    rig.begin_message(thread, &turn, "m1").await;
    let mut mirror = Mirror::loaded(loaded);
    // The message exists once its row came.
    follow(&mut client, &mut mirror, |held| {
        held.items
            .values()
            .any(|item| item["kind"] == "agent_message")
    })
    .await;

    rig.say(thread, message_grew("m1", 0, "The first stretch. "))
        .await;
    // Observe the same content in the delta stream and the committed row.
    follow(&mut client, &mut mirror, |held| held.appended >= 1).await;
    // The row of the message catches up with what was forwarded.
    follow(&mut client, &mut mirror, |held| {
        held.items
            .values()
            .any(|item| item["payload"]["text"] == "The first stretch. ")
    })
    .await;

    rig.finish_turn(thread, &turn, "m1", "The first stretch. ")
        .await;
    follow(&mut client, &mut mirror, |held| {
        held.items
            .values()
            .any(|item| item["status"] == "completed" && item["kind"] == "agent_message")
    })
    .await;
    assert_eq!(mirror.text.values().next().unwrap(), "The first stretch. ");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_reader_that_comes_back_with_its_last_sequence_is_sent_exactly_what_it_missed() {
    let rig = Rig::new().await;
    let thread = rig.thread("Reconnect").await;
    let loaded = rig.rows(thread).await.seq;

    // A reader follows the first turn to its end, and goes away.
    let mut first = Client::signed_in(rig.server.addr, &rig.token).await;
    first.sub(1, &feed(thread), Some(loaded)).await;
    first.expect("subscribed").await;
    first.expect("presence").await;
    let turn = rig.send(thread, "One").await;
    rig.begin_message(thread, &turn, "m1").await;
    rig.finish_turn(thread, &turn, "m1", "First answer.").await;
    let mut mirror = Mirror::loaded(loaded);
    follow(&mut first, &mut mirror, |held| {
        held.items
            .values()
            .any(|item| item["status"] == "completed" && item["kind"] == "agent_message")
    })
    .await;
    let held_seq = mirror.seqs.iter().map(|(_, seq)| *seq).max().unwrap();
    drop(first);

    // While nobody is connected a second turn is run.
    let turn = rig.send(thread, "Two").await;
    rig.begin_message(thread, &turn, "m2").await;
    rig.say(thread, message_grew("m2", 0, "Second ")).await;
    rig.finish_turn(thread, &turn, "m2", "Second answer.").await;
    // Background metadata may still advance the feed after the conversation ends. Pin this row
    // while comparing the stored cut with the socket's read of that same cut.
    let mut frozen = rig.pool.begin().await.unwrap();
    orch_db::queries::threads::lock(&mut frozen, thread)
        .await
        .unwrap()
        .unwrap();
    let now = rig.rows(thread).await.seq;

    // It comes back, says where it was, and is sent what came after, and that only.
    let mut second = Client::signed_in(rig.server.addr, &rig.token).await;
    second.sub(7, &feed(thread), Some(held_seq)).await;
    let subscribed = second.expect("subscribed").await;
    assert_eq!(subscribed["mode"], "delta");
    assert_eq!(subscribed["seq"], held_seq);
    second.expect("presence").await;
    let frame = second.expect("sync").await;
    assert_eq!(frame["id"], 7);
    assert_eq!(frame["prev"], held_seq);
    assert_eq!(frame["seq"], now);
    let items = frame["data"]["items"].as_array().unwrap();
    assert!(!items.is_empty());
    for item in items {
        assert!(
            item["seq_updated"].as_i64().unwrap() > held_seq,
            "a row it already had was sent again: {item}"
        );
    }
    // The first answer is not among them; the second is.
    let texts: Vec<&str> = items
        .iter()
        .filter_map(|item| item["payload"]["text"].as_str())
        .collect();
    assert_eq!(texts, ["Second answer."]);
    let user_messages = items
        .iter()
        .filter(|item| item["kind"] == "user_message")
        .count();
    assert_eq!(user_messages, 1);

    // And the client's store is the server's.
    mirror.take(&frame);
    let held = rig.texts(thread).await;
    assert_eq!(
        mirror.text.values().collect::<Vec<_>>(),
        held.values().collect::<Vec<_>>()
    );
    frozen.rollback().await.unwrap();
}

/// Whether the rows a client holds of a thread show a step of [`frames_of_a_turn`]: the message was begun,
/// the text the database holds of it caught up with what was said, it is complete.
fn shows_step(held: &Mirror, step: usize) -> bool {
    held.items.values().any(|item| {
        item["kind"] == "agent_message"
            && match step {
                0 => true,
                1 => item["payload"]["text"] == "In steps.",
                _ => item["status"] == "completed",
            }
    })
}

/// Every frame of a thread's feed that a client is sent while a turn is run in steps, so that several come.
async fn frames_of_a_turn(rig: &Rig, thread: Id, loaded: i64) -> Vec<serde_json::Value> {
    let mut client = Client::signed_in(rig.server.addr, &rig.token).await;
    client.sub(1, &feed(thread), Some(loaded)).await;
    client.expect("subscribed").await;
    client.expect("presence").await;
    let mut frames = Vec::new();
    let mut watching = Mirror::loaded(loaded);
    let turn = rig.send(thread, "Steps").await;
    for step in 0..3 {
        match step {
            0 => rig.begin_message(thread, &turn, "m1").await,
            1 => rig.say(thread, message_grew("m1", 0, "In steps.")).await,
            _ => rig.finish_turn(thread, &turn, "m1", "In steps.").await,
        }
        // Each step is read and sent before the next is taken. How long that takes is not for the test to
        // guess: the server reads the feed of a growing text once a second at most, and its row may be a
        // second behind the text, before the time the read itself takes.
        let started = Instant::now();
        while !shows_step(&watching, step) {
            assert!(
                started.elapsed() < realtime_support::PATIENCE,
                "gave up waiting for step {step} to be sent; the client holds {watching:?}"
            );
            if let Some(realtime_support::Heard::Frame(frame)) =
                client.hear_within(Duration::from_millis(150)).await
            {
                watching.take(&frame);
                frames.push(frame);
            }
        }
    }
    frames
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_client_that_misses_a_frame_sees_the_hole_and_repairs_it_over_the_read_request() {
    let rig = Rig::new().await;
    let thread = rig.thread("Holes").await;
    let loaded = rig.rows(thread).await.seq;
    let frames = frames_of_a_turn(&rig, thread, loaded).await;
    let syncs: Vec<&serde_json::Value> = frames
        .iter()
        .filter(|frame| frame["op"] == "sync")
        .collect();
    assert!(syncs.len() >= 3, "{} frames of rows", syncs.len());
    // Each frame begins where the one before it ended.
    for pair in syncs.windows(2) {
        assert_eq!(
            pair[1]["prev"], pair[0]["seq"],
            "a hole in the sequence of frames"
        );
    }

    // A client to which the second frame was lost sees that the third does not follow what it has.
    let mut lossy = Mirror::loaded(loaded);
    for frame in &frames {
        let lost = frame["op"] == "sync" && std::ptr::eq(frame, syncs[1]);
        if !lost {
            lossy.take(frame);
        }
    }
    assert!(lossy.resyncs >= 1, "the hole was not seen");

    // It reads what it lacks from where it stands, applies that as a frame, and goes on.
    let held = lossy.cursor.unwrap().last_seq();
    let read = rig
        .server
        .engine
        .sync(thread, Some(rig.ada), Some(held), 500)
        .await
        .unwrap();
    assert!(read.complete);
    lossy.take(&json!({
        "op": "sync", "prev": held, "seq": read.seq,
        "data": { "items": serde_json::to_value(&read.items).unwrap() }
    }));
    lossy
        .cursor
        .as_mut()
        .unwrap()
        .on_repaired(read.seq, read.complete);
    for frame in &frames {
        lossy.take(frame);
    }
    let stored = rig.texts(thread).await;
    assert_eq!(
        lossy.text.values().collect::<Vec<_>>(),
        stored.values().collect::<Vec<_>>()
    );
    // Everything stamped after what it loaded is in its store.
    let after_load = rig
        .rows(thread)
        .await
        .items
        .iter()
        .filter(|item| item.seq_updated > loaded)
        .count();
    assert_eq!(lossy.items.len(), after_load);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_frame_that_arrives_twice_changes_nothing() {
    let rig = Rig::new().await;
    let thread = rig.thread("Twice over").await;
    let loaded = rig.rows(thread).await.seq;
    let frames = frames_of_a_turn(&rig, thread, loaded).await;
    let mut once = Mirror::loaded(loaded);
    let mut twice = Mirror::loaded(loaded);
    for frame in &frames {
        once.take(frame);
        twice.take(frame);
        twice.take(frame);
    }
    assert_eq!(once.text, twice.text);
    assert_eq!(once.items, twice.items);
    assert_eq!(twice.resyncs, 0, "a repeat is no hole");
    assert_eq!(
        twice.text.values().collect::<Vec<_>>(),
        rig.texts(thread).await.values().collect::<Vec<_>>()
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_subscription_without_a_starting_point_begins_at_the_feed_now() {
    let rig = Rig::new().await;
    let thread = rig.thread("Now").await;
    let turn = rig.send(thread, "Hello").await;
    rig.begin_message(thread, &turn, "m1").await;
    rig.finish_turn(thread, &turn, "m1", "Hi.").await;
    // Accounting can finish before the source-end observation and its deferred
    // workspace invalidation stamp. Take the quiet cut only after all three
    // durable obligations settle; their real changes must not be ignored.
    realtime_support::eventually(
        "all terminal/source work settles before the quiet subscription cut",
        || async {
            sqlx::query_scalar::<_, bool>(
                "SELECT NOT EXISTS(SELECT 1 FROM turns WHERE thread_id=$1
            AND (end_work IS NOT NULL OR files_end_due OR files_work_pending))",
            )
            .bind(thread)
            .fetch_one(rig.pool.pool())
            .await
            .unwrap()
        },
    )
    .await;
    let now = rig.rows(thread).await.seq;

    let mut client = Client::signed_in(rig.server.addr, &rig.token).await;
    client.sub(1, &feed(thread), None).await;
    let subscribed = client.expect("subscribed").await;
    assert_eq!(subscribed["mode"], "delta");
    assert_eq!(subscribed["seq"], now);
    client.expect("presence").await;
    assert!(
        client.is_quiet_for(Duration::from_millis(500)).await,
        "nothing is owed to a reader who asked for what comes next"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_reader_that_is_ahead_of_the_feed_or_too_far_behind_is_told_to_load_again() {
    let rig = Rig::new().await;
    let thread = rig.thread("Reload").await;
    let now = rig.rows(thread).await.seq;

    let mut client = Client::signed_in(rig.server.addr, &rig.token).await;
    // The database was restored from an older copy: the reader holds more than there is.
    client.sub(1, &feed(thread), Some(now + 1_000)).await;
    let ahead = client.expect("subscribed").await;
    assert_eq!(ahead["mode"], "reload");
    assert_eq!(ahead["seq"], now);

    // A negative number is no use either.
    client.sub(2, &feed(thread), Some(-1)).await;
    let negative = client.expect("subscribed").await;
    assert_eq!(negative["mode"], "reload");

    // An answer to load again keeps no subscription: nothing is sent on it.
    let turn = rig.send(thread, "Hello").await;
    rig.begin_message(thread, &turn, "m1").await;
    assert!(client.is_quiet_for(Duration::from_millis(600)).await);
}

// ---------------------------------------------------------------- access

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_reader_that_loses_access_is_closed_out_before_any_further_row_is_sent() {
    // Nothing tells the server that the epoch moved but the rows and the text that follow it: they are what asks.
    let rig = Rig::tuned(|tuning| {
        tuning.epoch_every = Duration::from_secs(120);
        tuning.sweep_every = Duration::from_secs(120);
    })
    .await;
    let thread = rig.thread("Secret").await;
    let (bo, bo_token) = rig.person("bobby").await;
    let role = rig.give(bo, "viewer").await;
    let loaded = rig.rows(thread).await.seq;
    let mut bobby = Client::signed_in(rig.server.addr, &bo_token).await;
    bobby.sub(1, &feed(thread), Some(loaded)).await;
    assert_eq!(bobby.expect("subscribed").await["mode"], "delta");
    bobby.expect("presence").await;

    // What happens while he may look reaches him.
    let turn = rig.send(thread, "First question").await;
    let frames = bobby.until("sync").await;
    assert!(frames.iter().any(|frame| frame["op"] == "sync"));

    // His role is taken away: the epoch moves. The row written right after must not reach him.
    rig.take(role).await;
    rig.begin_message(thread, &turn, "m1").await;
    rig.say(
        thread,
        message_grew("m1", 0, "a secret that is not for him"),
    )
    .await;
    rig.finish_turn(thread, &turn, "m1", "a secret that is not for him")
        .await;

    let mut seen = Vec::new();
    let closed = loop {
        let frame = bobby.frame().await;
        if frame["op"] == "closed" {
            break frame;
        }
        seen.push(frame);
    };
    assert_eq!(closed["id"], 1);
    assert_eq!(closed["reason"], "revoked");
    assert!(
        !seen
            .iter()
            .any(|frame| frame.to_string().contains("a secret")),
        "a row reached him after his role was taken away: {seen:?}"
    );
    // Nothing more is sent on it, ever.
    assert!(bobby.is_quiet_for(Duration::from_millis(600)).await);
    // The socket itself stays open, and so does the owner's subscription.
    bobby.ping().await;
    let mut ada = Client::signed_in(rig.server.addr, &rig.token).await;
    ada.sub(1, &feed(thread), Some(loaded)).await;
    ada.until("sync").await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_announcement_that_the_epoch_moved_closes_out_a_reader_at_once_without_any_row_being_written()
 {
    // Disable both repair polls: only the announcement can close the reader.
    let rig = Rig::tuned(|tuning| {
        tuning.epoch_every = Duration::ZERO;
        tuning.sweep_every = Duration::ZERO;
    })
    .await;
    let thread = rig.thread("Quiet").await;
    let (bo, bo_token) = rig.person("bobby").await;
    let role = rig.give(bo, "viewer").await;
    let mut bobby = Client::signed_in(rig.server.addr, &bo_token).await;
    bobby.sub(1, &feed(thread), None).await;
    assert_eq!(bobby.expect("subscribed").await["mode"], "delta");
    bobby.expect("presence").await;

    let mut tx = rig.pool.begin().await.unwrap();
    orch_db::queries::access::testing::take_role_away(&mut tx, role, rig.ada, rig.clock.now())
        .await
        .unwrap();
    let epoch = orch_db::queries::access::bump_epoch(&mut tx).await.unwrap();
    orch_db::sequences::notify_epoch(&mut tx, epoch)
        .await
        .unwrap();
    tx.commit().await.unwrap();
    let closed = bobby.expect("closed").await;
    assert_eq!(closed["reason"], "revoked");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_subscription_whose_thread_is_deleted_is_closed_as_deleted() {
    let rig = Rig::new().await;
    let thread = rig.thread("Doomed").await;
    let mut client = Client::signed_in(rig.server.addr, &rig.token).await;
    client.sub(1, &feed(thread), None).await;
    client.expect("subscribed").await;
    client.expect("presence").await;
    sqlx::query("UPDATE nodes SET deleted_at = now(), deleted_by = $2 WHERE id = $1")
        .bind(thread)
        .bind(rig.ada)
        .execute(rig.pool.pool())
        .await
        .unwrap();
    let closed = client.expect("closed").await;
    assert_eq!(
        (closed["id"].as_i64(), closed["reason"].as_str()),
        (Some(1), Some("deleted"))
    );
    assert_eq!(rig.server.realtime.subscriptions(), 0);
}

// ---------------------------------------------------------------- a process that restarts

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn after_a_process_restarts_mid_stream_the_item_completes_with_the_full_text_and_nothing_is_repeated()
 {
    let rig = Rig::new().await;
    let thread = rig.thread("Restart").await;
    let loaded = rig.rows(thread).await.seq;
    let mut client = Client::signed_in(rig.server.addr, &rig.token).await;
    client.sub(1, &feed(thread), Some(loaded)).await;
    client.expect("subscribed").await;
    client.expect("presence").await;

    let turn = rig.send(thread, "Tell me a story").await;
    rig.begin_message(thread, &turn, "m1").await;
    rig.say(thread, message_grew("m1", 0, "Once upon a time, "))
        .await;
    let mut mirror = Mirror::loaded(loaded);
    follow(&mut client, &mut mirror, |held| {
        held.text.values().any(|text| text == "Once upon a time, ")
    })
    .await;

    // The process the browser was connected to goes away, in the middle of the message; what the
    // machine says meanwhile is still committed, as by the process the machine is connected to.
    rig.server.close_sockets().await;
    assert_eq!(client.closed().await, Some(1001));
    rig.say(thread, message_grew("m1", 18, "there was a socket. "))
        .await;
    rig.finish_turn(
        thread,
        &turn,
        "m1",
        "Once upon a time, there was a socket. ",
    )
    .await;

    // Another one takes its place, on the same database; the browser connects to it and says where it was.
    let config = orch_config::ServerConfig::default();
    let second = Server::over(&rig.pool, &rig.clock, quick(&config)).await;
    let mut client = Client::signed_in(second.addr, &rig.token).await;
    let held = mirror.cursor.unwrap().last_seq();
    client.sub(2, &feed(thread), Some(held)).await;
    assert_eq!(client.expect("subscribed").await["mode"], "delta");
    follow(&mut client, &mut mirror, |held| {
        held.items
            .values()
            .any(|item| item["kind"] == "agent_message" && item["status"] == "completed")
    })
    .await;

    assert_eq!(
        mirror.text.values().collect::<Vec<_>>(),
        ["Once upon a time, there was a socket. "]
    );
    let stored = rig.texts(thread).await;
    assert_eq!(
        mirror.text.values().collect::<Vec<_>>(),
        stored.values().collect::<Vec<_>>()
    );
    assert_eq!(mirror.gaps, 0);
}

// ---------------------------------------------------------------- a reader that is slow

/// Streams `deltas` small stretches of text at about 500 a second into a
/// message, through the sink the engine hands text to, to a reader that is
/// sent them through `down` bytes a second and a round trip of 300 ms, and
/// follows it to the end. Returns how many `delta` frames the reader was sent.
async fn stream_to_a_slow_reader(down: Option<u64>, deltas: usize) -> (usize, usize, Mirror) {
    let rig = Rig::cramped(|_| {}).await;
    let thread = rig.thread("Slow").await;
    let loaded = rig.rows(thread).await.seq;
    let link = SlowLine::shaped(rig.server.addr, Duration::from_millis(150), down).await;
    let mut client = Client::connect_cramped(link.addr, 2_048).await;
    client.auth(&rig.token).await;
    client.sub(1, &feed(thread), Some(loaded)).await;
    client.expect("subscribed").await;
    client.expect("presence").await;

    let turn = rig.send(thread, "Write a lot").await;
    rig.begin_message(thread, &turn, "m1").await;
    let mut mirror = Mirror::loaded(loaded);
    follow(&mut client, &mut mirror, |held| {
        held.items
            .values()
            .any(|item| item["kind"] == "agent_message")
    })
    .await;
    let item: Id = rig
        .rows(thread)
        .await
        .items
        .iter()
        .find(|item| {
            matches!(
                item.payload,
                orch_api_contract::threads::ItemPayload::AgentMessage(_)
            )
        })
        .unwrap()
        .id;

    let stretch = "forty characters of an agent talking... ";
    let sink = rig.server.realtime.sink();
    let producer = {
        let (sink, stretch) = (sink.clone(), stretch.to_owned());
        tokio::spawn(async move {
            let length = i64::try_from(stretch.chars().count()).unwrap();
            // Five hundred a second on average: a stall of the scheduler is made up for by a burst,
            // as the harness of a busy agent would make it.
            let mut ticks = tokio::time::interval(Duration::from_millis(2));
            ticks.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Burst);
            for number in 0..deltas {
                ticks.tick().await;
                sink.delta(&LiveDelta {
                    thread_id: thread,
                    item_id: item,
                    turn_id: None,
                    stream: "main".to_owned(),
                    field: DeltaField::Text,
                    offset: i64::try_from(number).unwrap() * length,
                    len: length,
                    data: stretch.clone(),
                });
            }
        })
    };

    // The reader takes what comes, one frame at a time, and never asks for more than the line gives.
    let mut delta_frames = 0;
    let mut rows = 0;
    let started = Instant::now();
    let full: String = stretch.repeat(deltas);
    while !producer.is_finished()
        || mirror.text.values().next().map(String::len) != Some(full.len())
    {
        assert!(
            started.elapsed() < Duration::from_secs(60),
            "the reader never got the text"
        );
        if let Some(realtime_support::Heard::Frame(frame)) =
            client.hear_within(Duration::from_millis(200)).await
        {
            match frame["op"].as_str() {
                Some("delta") => delta_frames += 1,
                Some("sync") => rows += 1,
                _ => {}
            }
            mirror.take(&frame);
        }
    }

    // The message is completed by the harness with the whole text, as it persisted.
    rig.finish_turn(thread, &turn, "m1", &full).await;
    follow(&mut client, &mut mirror, |held| {
        held.items
            .values()
            .any(|item| item["kind"] == "agent_message" && item["status"] == "completed")
    })
    .await;
    // The connection was kept open throughout.
    client.ping().await;
    let stored = rig.texts(thread).await;
    assert_eq!(
        mirror.text.values().collect::<Vec<_>>(),
        stored.values().collect::<Vec<_>>()
    );
    assert_eq!(mirror.text.values().next().unwrap(), &full);
    assert_eq!(mirror.gaps, 0, "a delta left a hole");
    assert_eq!(
        mirror.resyncs, 0,
        "nothing was dropped that had to be read again"
    );
    (delta_frames, rows, mirror)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn at_a_tenth_of_the_slow_links_rate_a_stream_of_small_deltas_arrives_as_fewer_larger_ones_and_the_connection_stays_open()
 {
    let deltas = 1_200;
    let (delta_frames, _, mirror) = stream_to_a_slow_reader(Some(20_000), deltas).await;
    assert!(
        delta_frames * 3 < deltas,
        "{delta_frames} delta frames were sent for {deltas} stretches of text"
    );
    assert!(mirror.appended > 0);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn on_the_slow_link_the_text_equals_the_persisted_text_at_completion_and_the_connection_stays_open()
 {
    // 1.6 Mbit/s down and a round trip of 300 ms.
    let deltas = 1_500;
    let (delta_frames, _, _) = stream_to_a_slow_reader(Some(200_000), deltas).await;
    assert!(delta_frames <= deltas);
}

// ---------------------------------------------------------------- a reader that takes nothing

/// How many of the messages a client holds are complete.
fn completed(held: &Mirror) -> usize {
    held.items
        .values()
        .filter(|item| item["kind"] == "agent_message" && item["status"] == "completed")
        .count()
}

/// The harness completes the messages `numbers` of a turn, each with a large text.
async fn large_messages(
    rig: &Rig,
    thread: Id,
    turn: &TurnStart,
    numbers: std::ops::Range<usize>,
    keeping_up: &mut Client,
    seen: &mut Mirror,
) {
    for number in numbers {
        let native = format!("m{number}");
        let text = format!("{number}: {}", "x".repeat(40_000));
        rig.say(
            thread,
            realtime_support::message_began(turn.turn_id, &native),
        )
        .await;
        rig.say(thread, realtime_support::message_done(&native, &text))
            .await;
        // This reader actually keeps up. Each original 40KB row fits the
        // unchanged 64KiB frame budget; the other reader still takes nothing.
        follow(keeping_up, seen, |held| completed(held) == number + 1).await;
        assert_eq!(
            seen.resyncs, 0,
            "a healthy reader must receive every original row incrementally"
        );
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_reader_that_takes_nothing_for_the_time_allowed_is_closed_with_4408_and_equals_the_servers_rows_after_it_loads_again()
 {
    // More than this waiting for a reader, for this long, closes it.
    let (buffer, allowed) = (64 * 1024, 1_500);
    // The server's time is held: "the time allowed" is over when this test says so, not when a busy
    // machine has let that much go by while the rows were still on their way. The grace for the close
    // is long, so that the reader gets the code whenever it comes back.
    let rig = Rig::cramped_with_time_held(|tuning| {
        tuning.buffer_max_bytes = buffer;
        tuning.buffer_max_millis = allowed;
        tuning.close_grace = Duration::from_secs(20);
    })
    .await;
    let thread = rig.thread("Flood").await;
    let loaded = rig.rows(thread).await.seq;
    let mut client = Client::connect_cramped(rig.server.addr, 2_048).await;
    client.auth(&rig.token).await;
    client.sub(1, &feed(thread), Some(loaded)).await;
    client.expect("subscribed").await;
    client.expect("presence").await;
    // A reader that keeps up follows the thread beside it. A pass sends the rows to the readers in the
    // order they subscribed, so what this one has been sent has been queued for the one that takes nothing.
    let mut keeping_up = Client::signed_in(rig.server.addr, &rig.token).await;
    keeping_up.sub(2, &feed(thread), Some(loaded)).await;
    keeping_up.expect("subscribed").await;
    let mut seen = Mirror::loaded(loaded);

    // Rows, each of them large, that the reader does not take: all but the last come first.
    let turn = rig.send(thread, "Flood").await;
    rig.say(thread, realtime_support::turn_began(turn.turn_id))
        .await;
    large_messages(&rig, thread, &turn, 0..23, &mut keeping_up, &mut seen).await;
    follow(&mut keeping_up, &mut seen, |held| completed(held) == 23).await;
    // More than the buffer has waited for the reader, and the time allowed passes; the last row is queued
    // for it after that, which is when the server looks, and it is let go.
    rig.let_time_pass(chrono::Duration::milliseconds(allowed + 1));
    large_messages(&rig, thread, &turn, 23..24, &mut keeping_up, &mut seen).await;
    follow(&mut keeping_up, &mut seen, |held| completed(held) == 24).await;
    // The code comes when it reads again.
    let mut mirror = Mirror::loaded(loaded);
    let mut code = None;
    let started = Instant::now();
    while code.is_none() {
        assert!(
            started.elapsed() < realtime_support::PATIENCE,
            "it was never closed"
        );
        match client.hear_within(Duration::from_millis(500)).await {
            Some(realtime_support::Heard::Frame(frame)) => mirror.take(&frame),
            Some(realtime_support::Heard::Closed(found)) => code = Some(found),
            None => {}
        }
    }
    assert_eq!(code, Some(Some(4408)));

    // It loads again and is equal to the server's rows.
    rig.say(
        thread,
        realtime_support::turn_ended(
            turn.turn_id,
            orch_link_contract::events::TurnOutcome::Completed,
        ),
    )
    .await;
    rig.wait_turn(turn.turn_id, "completed").await;
    let mut again = Client::signed_in(rig.server.addr, &rig.token).await;
    let held = mirror.cursor.unwrap().last_seq();
    again.sub(2, &feed(thread), Some(held)).await;
    assert_eq!(again.expect("subscribed").await["mode"], "delta");
    // A missed batch can exceed this deliberately small socket frame budget.
    // Repair the known gap once through HTTP, as the resync contract requires;
    // this is not a status-polling loop. The 24 original 40KB rows plus metadata
    // fit the same 1MiB body budget as the history fixture.
    let mut reply = reqwest::Client::builder()
        .timeout(realtime_support::PATIENCE)
        .build()
        .unwrap()
        .get(format!(
            "http://{}/api/v1/threads/{thread}/sync?since={held}&limit=50",
            rig.server.addr
        ))
        .bearer_auth(&rig.token)
        .send()
        .await
        .unwrap();
    assert!(reply.status().is_success());
    let mut bytes = Vec::new();
    while let Some(chunk) = reply.chunk().await.unwrap() {
        assert!(
            chunk.len() <= (1024 * 1024usize).saturating_sub(bytes.len()),
            "the bounded gap repair exceeds 1MiB"
        );
        bytes.extend_from_slice(&chunk);
    }
    let repaired: orch_api_contract::threads::ThreadSync = serde_json::from_slice(&bytes).unwrap();
    assert!(
        repaired.complete,
        "the unchanged 24-row history fits the bounded HTTP repair"
    );
    mirror
        .take(&json!({"op":"sync","prev":held,"seq":repaired.seq,"data":{"items":repaired.items}}));
    mirror
        .cursor
        .as_mut()
        .unwrap()
        .on_repaired(repaired.seq, repaired.complete);
    let stored = rig.texts(thread).await;
    follow(&mut again, &mut mirror, |held| {
        held.text.len() == stored.len()
            && held
                .text
                .values()
                .all(|text| stored.values().any(|stored| stored == text))
    })
    .await;
    assert_eq!(mirror.text.len(), 24);
    let next = rig
        .send(thread, "Continue on the same repaired socket")
        .await;
    rig.begin_message(thread, &next, "after-gap-repair").await;
    rig.finish_turn(
        thread,
        &next,
        "after-gap-repair",
        "The repaired reader still follows this thread.",
    )
    .await;
    follow(&mut again, &mut mirror, |view| completed(view) == 25).await;
    assert_eq!(mirror.text, rig.texts(thread).await);
}
