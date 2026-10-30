//! 128 persisted HTTP/native-link rounds and 10B synthetic reported tokens; no paid model calls.

mod realtime_support;
mod thread_history_observation;
mod thread_history_support;

use orch_agent_contract::events::{
    FileChange, FileChangeKind, ItemStatus, RawCall, ReasoningPayload, ToolCallPayload, ToolPayload,
};
use orch_api_contract::threads::OutputRef;
use orch_link_contract::commands::TurnStart;
use orch_link_contract::events::{
    EmittedItemKind, Event, ItemCompleted, ItemStarted, TurnOutcome, UsageUpdated,
};
use orch_link_contract::foreign::ItemPayload;
use orch_types::{Id, Time};
use realtime_support::{Client, PATIENCE, Rig};
use serde_json::json;
use std::collections::BTreeSet;
use std::io::Write;
use std::time::Instant;
use thread_history_observation::{Observed, subscribe};
use thread_history_support::{
    PAGE_BYTES, Proof, accounted, body, get, input, native, report, url, usage,
};

const ROUNDS: usize = 128; // 128 * 78,125,000 is exactly 10,000,000,000.
const ITEM_BYTES: usize = 8 * 1024;
const COMMAND_BYTES: usize = 2 * 1024; // display + raw output fit together inside ITEM_BYTES.

fn output(text: &str) -> Proof<OutputRef> {
    Ok(OutputRef {
        inline: text.to_owned(),
        total_len: i64::try_from(text.len())?,
        truncated: false,
        blob: None,
    })
}

async fn item(
    rig: &Rig,
    thread: Id,
    turn: &TurnStart,
    reference: String,
    payload: ItemPayload,
) -> Proof {
    assert!(
        serde_json::to_vec(&payload)?.len() <= ITEM_BYTES,
        "generated payload exceeds its declared bound"
    );
    let kind = match &payload {
        ItemPayload::AgentMessage(_) => EmittedItemKind::AgentMessage,
        ItemPayload::Reasoning(_) => EmittedItemKind::Reasoning,
        ItemPayload::ToolCall(_) => EmittedItemKind::ToolCall,
        ItemPayload::Plan(_) => EmittedItemKind::Plan,
        ItemPayload::Marker(_) => EmittedItemKind::Marker,
        ItemPayload::Error(_) => EmittedItemKind::Error,
    };
    rig.say_many(
        thread,
        &[
            Event::ItemStarted(ItemStarted {
                turn_id: turn.turn_id,
                native_ref: reference.clone(),
                stream: "main".into(),
                parent_native_ref: None,
                kind,
                payload: payload.clone(),
            }),
            Event::ItemCompleted(ItemCompleted {
                native_ref: reference,
                status: ItemStatus::Completed,
                payload,
                output_blob: None,
            }),
        ],
    )
    .await;
    Ok(())
}

async fn generated_round(rig: &Rig, thread: Id, turn: &TurnStart, round: usize) -> Proof {
    rig.begin_message(thread, turn, &format!("r{round}-intro"))
        .await;
    rig.say(
        thread,
        realtime_support::message_done(
            &format!("r{round}-intro"),
            "Synthetic fixture: inspect and update a small module.",
        ),
    )
    .await;
    let summary = "Generated fixture bookkeeping; no model reasoning was consumed.";
    item(
        rig,
        thread,
        turn,
        format!("r{round}-reasoning"),
        ItemPayload::Reasoning(ReasoningPayload {
            summary: summary.into(),
            text_len: i64::try_from(summary.len())?,
            hidden: false,
        }),
    )
    .await?;
    let printed = format!("fixture-{round:03} ").repeat(200);
    let printed = &printed[..COMMAND_BYTES.min(printed.len())];
    for step in 0..8 {
        let command = format!("fixture-read module-{step}");
        item(
            rig,
            thread,
            turn,
            format!("r{round}-command-{step}"),
            ItemPayload::ToolCall(ToolCallPayload {
                title: format!("Synthetic command {step}"),
                started_at: Time::new(rig.clock.now()),
                duration_ms: Some(1),
                tool: ToolPayload::Command {
                    command: command.clone(),
                    cwd: "/workspace/web".into(),
                    exit_code: Some(0),
                    output: output(printed)?,
                    process_id: None,
                    background: false,
                    actions: Vec::new(),
                },
                raw: RawCall {
                    name: "SyntheticShell".into(),
                    input: output(&json!({"command":command}).to_string())?,
                    output: Some(output(printed)?),
                    is_error: false,
                },
            }),
        )
        .await?;
    }
    for step in 0..4 {
        item(
            rig,
            thread,
            turn,
            format!("r{round}-file-{step}"),
            ItemPayload::ToolCall(ToolCallPayload {
                title: format!("Synthetic file change {step}"),
                started_at: Time::new(rig.clock.now()),
                duration_ms: Some(1),
                tool: ToolPayload::FileChange {
                    changes: vec![FileChange {
                        path: format!("src/module-{step}.rs"),
                        change: FileChangeKind::Modify,
                        old_path: None,
                        diff: Some("@@ -1 +1 @@\n-old\n+fixture\n".into()),
                        added: 1,
                        removed: 1,
                        binary: false,
                        truncated: false,
                    }],
                },
                raw: RawCall {
                    name: "SyntheticEdit".into(),
                    input: output("{\"synthetic\":true}")?,
                    output: Some(output("reported")?),
                    is_error: false,
                },
            }),
        )
        .await?;
    }
    item(
        rig,
        thread,
        turn,
        format!("r{round}-final"),
        ItemPayload::AgentMessage(orch_agent_contract::events::AgentMessagePayload {
            text: format!("Synthetic interaction {round} is complete."),
            text_len: i64::try_from(format!("Synthetic interaction {round} is complete.").len())?,
            phase: Some(orch_agent_contract::events::AgentMessagePhase::Final),
        }),
    )
    .await?;
    report(rig, thread, turn, i64::try_from(round)?).await;
    rig.say(
        thread,
        realtime_support::turn_ended(turn.turn_id, TurnOutcome::Completed),
    )
    .await;
    Ok(())
}

async fn pages(rig: &Rig, http: &reqwest::Client, thread: Id) -> Proof {
    let expected: Vec<(Id,i64)> = sqlx::query_as("SELECT id,seq_created FROM items WHERE thread_id=$1 AND stream='main' ORDER BY seq_created")
        .bind(thread).fetch_all(rig.pool.pool()).await?;
    assert!(
        expected.len() >= ROUNDS * 16,
        "the generated history must be persisted, not only a reported total"
    );
    let mut before = i64::MAX;
    let mut seen = BTreeSet::new();
    let mut count = 0;
    loop {
        let page = get(
            rig,
            http,
            &format!("/threads/{thread}/items?before={before}&stream=main&limit=50"),
            PAGE_BYTES,
        )
        .await?;
        let rows = page["items"].as_array().ok_or("item page absent")?;
        assert!(rows.len() <= 50);
        let mut prior = 0;
        for row in rows {
            let seq = row["seq_created"].as_i64().ok_or("item sequence absent")?;
            assert!(seq > prior && seq < before);
            prior = seq;
            let id: Id = serde_json::from_value(row["id"].clone())?;
            assert!(seen.insert(id), "pagination repeated a persisted item");
            count += 1;
        }
        if page["has_older"] == false {
            break;
        }
        before = rows
            .first()
            .and_then(|row| row["seq_created"].as_i64())
            .ok_or("a continuing page is empty")?;
    }
    assert_eq!(count, expected.len());
    assert_eq!(seen, expected.into_iter().map(|(id, _)| id).collect());
    let anchor = get(
        rig,
        http,
        &format!("/threads/{thread}/items?turn=64&stream=main&limit=50"),
        PAGE_BYTES,
    )
    .await?;
    assert!(
        !anchor["items"]
            .as_array()
            .ok_or("anchored page absent")?
            .is_empty()
    );
    Ok(())
}

async fn exact_usage(rig: &Rig, http: &reqwest::Client, thread: Id) -> Proof {
    let day = rig.clock.now().date_naive();
    let until = day + chrono::Duration::days(1);
    let figures = get(
        rig,
        http,
        &format!("/usage?scope=node:{thread}&from={day}&to={until}"),
        PAGE_BYTES,
    )
    .await?;
    let expected = usage(i64::try_from(ROUNDS)?);
    // Daily analytics intentionally omit the cache-write subcategory; require exact
    // supported aggregates and prove every category through the native ledger/events.
    for key in [
        "input_tokens",
        "cached_input_tokens",
        "output_tokens",
        "reasoning_tokens",
        "total_tokens",
    ] {
        assert_eq!(figures["totals"]["tokens"][key], json!(expected)[key]);
    }
    let mut cursor = None::<String>;
    let mut ids = BTreeSet::new();
    let mut total = usage(0);
    loop {
        let suffix = cursor
            .as_ref()
            .map(|cursor| {
                format!(
                    "&cursor={}",
                    url::form_urlencoded::byte_serialize(cursor.as_bytes()).collect::<String>()
                )
            })
            .unwrap_or_default();
        let page=get(rig,http,&format!("/usage/events?scope=node:{thread}&from={day}&to={until}&counted=true&limit=50{suffix}"),PAGE_BYTES).await?;
        let rows = page["items"].as_array().ok_or("usage page absent")?;
        assert!(rows.len() <= 50);
        for row in rows {
            let id = row["id"]
                .as_str()
                .ok_or("usage identity absent")?
                .to_owned();
            assert!(ids.insert(id));
            let tokens: orch_api_contract::threads::TokenUsage =
                serde_json::from_value(row["tokens"].clone())?;
            assert!(tokens.is_consistent());
            total = total.plus(tokens);
        }
        cursor = page["next_cursor"].as_str().map(str::to_owned);
        if cursor.is_none() {
            break;
        }
    }
    assert_eq!(ids.len(), ROUNDS);
    assert_eq!(total, expected);
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn one_hundred_twenty_eight_http_rounds_and_ten_billion_synthetic_tokens_keep_socket_progress_and_bounded_history_pages()
-> Proof {
    let rig = Rig::tuned(|tuning| tuning.sweep_every = std::time::Duration::ZERO).await;
    native(&rig).await?;
    let http = reqwest::Client::builder().timeout(PATIENCE).build()?;
    let thread = rig.thread("Synthetic 128-round 10B-token fixture").await;
    let opening = get(
        &rig,
        &http,
        &format!("/threads/{thread}/opening?limit=50"),
        2 * PAGE_BYTES,
    )
    .await?;
    let initial = opening["sync"]["seq"]
        .as_i64()
        .ok_or("opening sequence absent")?;
    let mut a = subscribe(&rig, thread, initial).await?;
    let mut observed = Observed::new(initial);
    let mut b = Some(subscribe(&rig, thread, initial).await?);
    let mut other = Observed::new(initial);
    let mut c = subscribe(&rig, thread, initial).await?;
    c.ping().await;
    drop(c);
    let began = Instant::now();
    let mut input_ids = BTreeSet::new();
    for round in 1..=ROUNDS {
        let round_began = Instant::now();
        let key = format!("synthetic-history-{round}");
        let (turn, input_id) = input(&rig, &http, thread, round - 1, &key).await?;
        assert!(input_ids.insert(input_id));
        generated_round(&rig, thread, &turn, round).await?;
        observed
            .terminal(&mut a, &rig, thread, turn.turn_id, "completed")
            .await?;
        assert_eq!(
            observed.thread["state"], "idle",
            "a healthy synthetic round must finish all core state without a failure block"
        );
        if round == 40 {
            b = Some(subscribe(&rig, thread, other.seq).await?);
        }
        if let Some(socket) = b.as_mut() {
            other
                .terminal(socket, &rig, thread, turn.turn_id, "completed")
                .await?;
        }
        if round == 32 {
            drop(b.take());
        }
        if [27, 28, 54, 55, 128].contains(&round) {
            assert_eq!(
                observed.thread["usage"]["total_tokens"],
                json!(usage(i64::try_from(round)?).total_tokens)
            );
            assert_eq!(
                observed.thread["usage"]["input_tokens"],
                json!(usage(i64::try_from(round)?).input_tokens)
            );
        }
        // Keep completed-round progress in a watchdog's log even when the test process times out
        // before libtest can release its captured output.
        writeln!(
            std::io::stderr(),
            "synthetic persisted round={round} elapsed_ms={}",
            round_began.elapsed().as_millis()
        )?;
    }
    accounted(&rig, thread).await;
    let stored:(i64,i64,i64)=sqlx::query_as("SELECT count(*),count(*) FILTER(WHERE state='completed'),count(DISTINCT input_id) FROM turns WHERE thread_id=$1")
        .bind(thread).fetch_one(rig.pool.pool()).await?;
    assert_eq!(stored, (128, 128, 128));
    let summed:(i64,i64)=sqlx::query_as("SELECT count(*),COALESCE(sum(total_tokens),0)::bigint FROM usage_events WHERE thread_id=$1 AND counted AND origin='harness'")
        .bind(thread).fetch_one(rig.pool.pool()).await?;
    assert_eq!(summed, (128, 10_000_000_000));
    assert_eq!(rig.turn_starts(thread).len(), ROUNDS);
    assert!(observed.max_frame < PAGE_BYTES && other.max_frame < PAGE_BYTES);
    // A whole-history gap uses the existing reload path, without closing its socket.
    let mut returning = Client::signed_in(rig.server.addr, &rig.token).await;
    returning
        .sub(1, &format!("thread:{thread}"), Some(initial))
        .await;
    assert_eq!(returning.expect("subscribed").await["mode"], "reload");
    returning.ping().await;
    let gap = get(
        &rig,
        &http,
        &format!("/threads/{thread}/sync?since={initial}"),
        PAGE_BYTES,
    )
    .await?;
    assert_eq!(gap["complete"], false);
    let whole = get(
        &rig,
        &http,
        &format!("/threads/{thread}/sync?limit=50"),
        2 * PAGE_BYTES,
    )
    .await?;
    assert!(whole["items"].as_array().ok_or("sync items absent")?.len() <= 100);
    assert!(whole["turns"].as_array().ok_or("sync turns absent")?.len() <= 20);
    assert_eq!(whole["thread"]["usage"]["total_tokens"], 10_000_000_000i64);
    let current = whole["seq"].as_i64().ok_or("current sequence absent")?;
    let mut c = subscribe(&rig, thread, current).await?;
    let mut repaired = Observed::new(current);
    pages(&rig, &http, thread).await?;
    exact_usage(&rig, &http, thread).await?;
    get(
        &rig,
        &http,
        &format!("/threads/{thread}/opening?limit=50"),
        2 * PAGE_BYTES,
    )
    .await?;
    for (nth, outcome, state) in [
        (128, TurnOutcome::Failed, "failed"),
        (129, TurnOutcome::Interrupted, "interrupted"),
    ] {
        let (turn, _) = input(&rig, &http, thread, nth, &format!("synthetic-probe-{nth}")).await?;
        rig.begin_message(thread, &turn, &format!("probe-{nth}"))
            .await;
        rig.say(
            thread,
            Event::UsageUpdated(UsageUpdated {
                turn_id: turn.turn_id,
                turn: usage(0),
                thread: usage(128),
                context_window: None,
                context_used: None,
                report: None,
            }),
        )
        .await;
        rig.say(thread, realtime_support::turn_ended(turn.turn_id, outcome))
            .await;
        observed
            .terminal(&mut a, &rig, thread, turn.turn_id, state)
            .await?;
        repaired
            .terminal(&mut c, &rig, thread, turn.turn_id, state)
            .await?;
    }
    accounted(&rig, thread).await;
    let total:i64=sqlx::query_scalar("SELECT COALESCE(sum(total_tokens),0)::bigint FROM usage_events WHERE thread_id=$1 AND counted")
        .bind(thread).fetch_one(rig.pool.pool()).await?;
    assert_eq!(total, 10_000_000_000);
    // A repeated HTTP client identity and native terminal event cannot create another round.
    let repeated=body(http.post(url(&rig,&format!("/threads/{thread}/inputs"))).bearer_auth(&rig.token)
        .json(&json!({"content":[{"type":"text","text":"Synthetic interaction 0"}],"delivery":"queue","client_id":"synthetic-history-1"}))
        .send().await?,PAGE_BYTES).await?.0;
    let id: Id = serde_json::from_value(repeated["input"]["id"].clone())?;
    assert!(input_ids.contains(&id));
    let last = rig
        .turn_starts(thread)
        .last()
        .cloned()
        .ok_or("last probe absent")?;
    rig.say(
        thread,
        realtime_support::turn_ended(last.turn_id, TurnOutcome::Interrupted),
    )
    .await;
    let turns: i64 = sqlx::query_scalar("SELECT count(*) FROM turns WHERE thread_id=$1")
        .bind(thread)
        .fetch_one(rig.pool.pool())
        .await?;
    assert_eq!(turns, 130);
    eprintln!(
        "synthetic history proof: rounds=128 total_tokens=10000000000 items={} max_socket_bytes={} elapsed_ms={}",
        observed.items.len(),
        observed.max_frame,
        began.elapsed().as_millis()
    );
    rig.server.engine.stop();
    rig.server.stop().await;
    Ok(())
}
