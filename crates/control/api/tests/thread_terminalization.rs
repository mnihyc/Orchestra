//! Accepted native ends are atomic core state; frozen usage/notifications replay independently.

mod realtime_support;
mod terminal_work_diagnostics;
mod thread_history_observation;
mod thread_history_support;

use orch_db::usage::caps;
use orch_link_contract::events::{Event, TurnOutcome};
use orch_types::{Id, new_id};
use realtime_support::{Client, PATIENCE, Rig, Server, quick};
use serde_json::{Value, json};
use thread_history_observation::{Observed, subscribe};
use thread_history_support::{
    PAGE_BYTES, Proof, accounted, body, get, input, native, report, url, usage, usage_committed,
};

const FOLLOWUP_FAILED: &str = "The last turn ended, but its usage and notifications could not be saved. Check the agent profile or storage.";
const OTHER_BLOCK: &str = "Synthetic manual review is required before another turn.";

async fn agent_patch(
    rig: &Rig,
    http: &reqwest::Client,
    overrides: Value,
) -> Proof<reqwest::Response> {
    let current = get(rig, http, &format!("/agents/{}", rig.agent), PAGE_BYTES).await?;
    Ok(http
        .patch(url(rig, &format!("/agents/{}", rig.agent)))
        .bearer_auth(&rig.token)
        .json(&json!({"expected_revision":current["revision"],"descriptor_overrides":overrides}))
        .send()
        .await?)
}

async fn retained(rig: &Rig, turn: Id) -> Proof<Value> {
    Ok(
        sqlx::query_scalar::<_, Option<Value>>(
            "SELECT end_work->'snapshot' FROM turns WHERE id=$1",
        )
        .bind(turn)
        .fetch_one(rig.pool.pool())
        .await?
        .unwrap_or(Value::Null),
    )
}

async fn ledger(rig: &Rig, turn: Id) -> Proof<(i64, i64)> {
    Ok(sqlx::query_as("SELECT count(*),COALESCE(sum(total_tokens),0)::bigint FROM usage_events WHERE turn_id=$1 AND counted")
        .bind(turn).fetch_one(rig.pool.pool()).await?)
}

async fn visible(
    socket: &mut Client,
    seen: &mut Observed,
    rig: &Rig,
    thread: Id,
    value: bool,
) -> Proof {
    socket
        .send(json!({"op":"visible","feed":format!("thread:{thread}"),"visible":value}))
        .await;
    tokio::time::timeout(PATIENCE, async {
        loop {
            let frame = socket.frame().await;
            seen.take(&frame)?;
            if frame["op"] == "presence" {
                let watching = frame["users"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .any(|row| row["user"]["id"] == rig.ada.to_string());
                if watching == value {
                    return Ok::<(), Box<dyn std::error::Error + Send + Sync>>(());
                }
            }
        }
    })
    .await?
}

async fn socket_until(
    socket: &mut Client,
    seen: &mut Observed,
    phase: &str,
    condition: impl Fn(&Observed) -> bool,
) -> Proof {
    tokio::time::timeout(PATIENCE, async {
        while !condition(seen) {
            seen.take(&socket.frame().await)?;
        }
        Ok::<(), Box<dyn std::error::Error + Send + Sync>>(())
    })
    .await
    .map_err(|_| format!("the existing socket did not show {phase}"))?
}

fn has_block(thread: &Value, detail: &str) -> bool {
    thread["state"] == "blocked"
        && thread["current_turn_id"].is_null()
        && thread["blocked"]["reason"] == "error"
        && thread["blocked"]["detail"] == detail
}

#[derive(Clone, Copy)]
enum RecoveryDecision {
    Continue,
    Stop,
    OtherBlock,
}

async fn pending_followup_recovery(decision: RecoveryDecision) -> Proof {
    let rig = Rig::new().await;
    native(&rig).await?;
    let http = reqwest::Client::builder().timeout(PATIENCE).build()?;
    let thread = rig.thread("Synthetic visible terminal recovery").await;
    let initial = get(
        &rig,
        &http,
        &format!("/threads/{thread}/sync?limit=20"),
        PAGE_BYTES,
    )
    .await?;
    let seq = initial["seq"].as_i64().ok_or("initial sequence absent")?;
    let mut socket = subscribe(&rig, thread, seq).await?;
    let mut seen = Observed::new(seq);
    let (start, _) = input(&rig, &http, thread, 0, "visible-original").await?;
    rig.begin_message(thread, &start, "visible-original-answer")
        .await;
    report(&rig, thread, &start, 1).await;
    sqlx::query("UPDATE agents SET descriptor_overrides=$2 WHERE id=$1")
        .bind(rig.agent)
        .bind(json!({"harness":0}))
        .execute(rig.pool.pool())
        .await?;
    rig.say(
        thread,
        realtime_support::turn_ended(start.turn_id, TurnOutcome::Completed),
    )
    .await;
    seen.terminal(&mut socket, &rig, thread, start.turn_id, "completed")
        .await?;
    socket_until(
        &mut socket,
        &mut seen,
        "the failed follow-up's exact waiting cause",
        |view| has_block(&view.thread, FOLLOWUP_FAILED),
    )
    .await?;
    let original = retained(&rig, start.turn_id).await?;
    assert_eq!(original["usage"], json!(usage(1)));
    assert_eq!(ledger(&rig, start.turn_id).await?, (0, 0));
    let pending = body(http.post(url(&rig, &format!("/threads/{thread}/inputs"))).bearer_auth(&rig.token)
        .json(&json!({"content":[{"type":"text","text":"The same next input"}],"delivery":"queue","client_id":"visible-next"}))
        .send().await?, PAGE_BYTES).await?.0;
    let queued: Id = serde_json::from_value(pending["input"]["id"].clone())?;
    assert_eq!(pending["input"]["state"], "pending");
    assert!(pending["input"]["turn_id"].is_null());
    socket_until(
        &mut socket,
        &mut seen,
        "the same pending input beside its cause",
        |view| {
            has_block(&view.thread, FOLLOWUP_FAILED)
                && view
                    .inputs
                    .get(&queued.to_string())
                    .is_some_and(|row| row["state"] == "pending" && row["turn_id"].is_null())
        },
    )
    .await?;
    // A person opening after posting receives the same cause. Recovery below is
    // observed on this already-open socket, without an HTTP status refresh loop.
    let waiting = get(
        &rig,
        &http,
        &format!("/threads/{thread}/sync?limit=20"),
        PAGE_BYTES,
    )
    .await?;
    assert!(has_block(&waiting["thread"], FOLLOWUP_FAILED));
    assert!(
        waiting["inputs"]
            .as_array()
            .ok_or("inputs absent")?
            .iter()
            .any(|row| row["id"] == queued.to_string() && row["state"] == "pending")
    );
    assert_eq!(rig.turn_starts(thread).len(), 1);
    let before_repair: i64 = sqlx::query_scalar("SELECT count(*) FROM turns WHERE thread_id=$1")
        .bind(thread)
        .fetch_one(rig.pool.pool())
        .await?;
    assert_eq!(
        before_repair, 1,
        "waiting accounting cannot admit even an undispatched next turn"
    );
    match decision {
        RecoveryDecision::Continue => {}
        RecoveryDecision::Stop => {
            let stopped = body(
                http.post(url(&rig, &format!("/threads/{thread}/interrupt")))
                    .bearer_auth(&rig.token)
                    .json(&json!({"turn_id":start.turn_id}))
                    .send()
                    .await?,
                PAGE_BYTES,
            )
            .await?
            .0;
            assert_eq!(
                stopped["state"], "completed",
                "Stop must not resurrect the old turn"
            );
            socket_until(
                &mut socket,
                &mut seen,
                "Stop holding this exact queued input",
                |view| {
                    view.thread["queue"]["held"] == true
                        && view
                            .inputs
                            .get(&queued.to_string())
                            .is_some_and(|row| row["queue_held"] == true)
                },
            )
            .await?;
        }
        RecoveryDecision::OtherBlock => {
            // Reconstruct an independent manual/start failure through the normal
            // locked metadata/stamp/notification path. No endpoint clears it here.
            let mut tx = rig.pool.begin().await?;
            orch_db::queries::threads::lock(&mut tx, thread)
                .await?
                .ok_or("thread disappeared")?;
            let mut unit = orch_engine::unit::Unit::new(&mut tx, rig.clock.now());
            let seq = unit.stamp(thread).await?;
            assert!(
                orch_db::queries::threads::set_state(
                    &mut *unit.conn,
                    thread,
                    &["blocked"],
                    &orch_db::queries::threads::StateChange::blocked("error", Some(OTHER_BLOCK)),
                    seq
                )
                .await?
            );
            unit.finish().await?;
            tx.commit().await?;
            socket_until(
                &mut socket,
                &mut seen,
                "the independent manual block",
                |view| has_block(&view.thread, OTHER_BLOCK),
            )
            .await?;
            let previous: Option<Value> =
                sqlx::query_scalar("SELECT end_work->'attempted_at' FROM turns WHERE id=$1")
                    .bind(start.turn_id)
                    .fetch_one(rig.pool.pool())
                    .await?;
            rig.server.engine.wake(thread);
            realtime_support::eventually(
                "another actual failed attempt preserves the manual block",
                || async {
                    sqlx::query_scalar::<_, Option<Value>>(
                        "SELECT end_work->'attempted_at' FROM turns WHERE id=$1",
                    )
                    .bind(start.turn_id)
                    .fetch_one(rig.pool.pool())
                    .await
                    .is_ok_and(|value| value != previous)
                },
            )
            .await;
        }
    }
    assert_eq!(retained(&rig, start.turn_id).await?, original);
    body(agent_patch(&rig, &http, json!({})).await?, PAGE_BYTES).await?;
    accounted(&rig, thread).await;
    assert_eq!(
        ledger(&rig, start.turn_id).await?,
        (1, usage(1).total_tokens)
    );
    let at: chrono::DateTime<chrono::Utc> =
        sqlx::query_scalar("SELECT at FROM usage_events WHERE turn_id=$1 AND counted")
            .bind(start.turn_id)
            .fetch_one(rig.pool.pool())
            .await?;
    let frozen_at: chrono::DateTime<chrono::Utc> = serde_json::from_value(original["at"].clone())?;
    assert_eq!(
        at.timestamp_micros(),
        frozen_at.timestamp_micros(),
        "the ledger preserves the frozen end at PostgreSQL timestamp precision"
    );
    if matches!(decision, RecoveryDecision::OtherBlock) {
        let after = get(
            &rig,
            &http,
            &format!("/threads/{thread}/sync?limit=20"),
            PAGE_BYTES,
        )
        .await?;
        assert!(has_block(&after["thread"], OTHER_BLOCK));
        assert!(
            after["inputs"]
                .as_array()
                .ok_or("inputs absent")?
                .iter()
                .any(|row| row["id"] == queued.to_string() && row["state"] == "pending")
        );
        assert_eq!(rig.turn_starts(thread).len(), 1);
        assert!(has_block(&seen.thread, OTHER_BLOCK));
    } else {
        if matches!(decision, RecoveryDecision::Stop) {
            socket_until(
                &mut socket,
                &mut seen,
                "successful follow-up clearing only its block while Stop remains",
                |view| {
                    view.thread["state"] == "idle"
                        && view.thread["blocked"].is_null()
                        && view.thread["queue"]["held"] == true
                        && view.inputs.get(&queued.to_string()).is_some_and(|row| {
                            row["state"] == "pending" && row["queue_held"] == true
                        })
                },
            )
            .await?;
            assert_eq!(rig.turn_starts(thread).len(), 1);
            let revision = seen.thread["queue"]["revision"]
                .as_i64()
                .ok_or("held queue revision absent")?;
            let resumed = body(
                http.post(url(&rig, &format!("/threads/{thread}/queue/resume")))
                    .bearer_auth(&rig.token)
                    .json(&json!({"revision":revision}))
                    .send()
                    .await?,
                PAGE_BYTES,
            )
            .await?
            .0;
            assert_eq!(resumed["held"], false);
        }
        let next = rig.turn_start(thread, 1).await;
        assert_ne!(next.turn_id, start.turn_id);
        socket_until(
            &mut socket,
            &mut seen,
            "recovery starting that same input exactly once",
            |view| {
                view.thread["current_turn_id"] == next.turn_id.to_string()
                    && view.thread["blocked"].is_null()
                    && view
                        .turns
                        .get(&next.turn_id.to_string())
                        .is_some_and(|row| row["input_id"] == queued.to_string())
                    && view
                        .inputs
                        .get(&queued.to_string())
                        .is_some_and(|row| row["turn_id"] == next.turn_id.to_string())
            },
        )
        .await?;
        rig.begin_message(thread, &next, "visible-recovered-answer")
            .await;
        rig.say(
            thread,
            Event::UsageUpdated(orch_link_contract::events::UsageUpdated {
                turn_id: next.turn_id,
                turn: usage(0),
                thread: usage(1),
                context_window: None,
                context_used: None,
                report: None,
            }),
        )
        .await;
        usage_committed(&rig, next.turn_id, 0).await;
        rig.say(
            thread,
            realtime_support::turn_ended(next.turn_id, TurnOutcome::Completed),
        )
        .await;
        seen.terminal(&mut socket, &rig, thread, next.turn_id, "completed")
            .await?;
        accounted(&rig, thread).await;
        assert_eq!(rig.turn_starts(thread).len(), 2);
        assert_eq!(
            seen.turns
                .get(&start.turn_id.to_string())
                .ok_or("old turn absent")?["state"],
            "completed"
        );
        let delivered = seen
            .inputs
            .get(&queued.to_string())
            .ok_or("recovered input absent")?;
        assert_eq!(delivered["state"], "delivered");
        assert_eq!(delivered["turn_id"], next.turn_id.to_string());
        assert_eq!(delivered["queue_held"], false);
    }
    // Redelivery cannot recreate the obligation, change the original usage or
    // bypass either queue decision after recovery.
    rig.say(
        thread,
        realtime_support::turn_ended(start.turn_id, TurnOutcome::Completed),
    )
    .await;
    assert_eq!(
        ledger(&rig, start.turn_id).await?,
        (1, usage(1).total_tokens)
    );
    rig.server.engine.stop();
    rig.server.stop().await;
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_next_http_input_sees_the_followup_failure_and_recovers_on_its_open_socket() -> Proof {
    pending_followup_recovery(RecoveryDecision::Continue).await
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn recovered_followup_preserves_http_stop_until_the_same_queued_input_is_resumed() -> Proof {
    pending_followup_recovery(RecoveryDecision::Stop).await
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn recovered_followup_does_not_clear_an_unrelated_manual_error_block() -> Proof {
    pending_followup_recovery(RecoveryDecision::OtherBlock).await
}

async fn reserve_cap(rig: &Rig, turn: Id) -> Proof {
    let mut conn = rig.pool.acquire().await?;
    let mut cap = caps::insert(
        &mut conn,
        &caps::NewCap {
            id: new_id(),
            node_id: Some(rig.root),
            provider_id: None,
            subject_kind: caps::subject::EVERYONE,
            subject_id: None,
            subject_role: None,
            split: caps::split::SHARED,
            models: &[],
            tiers: &[],
            harness_kinds: &[],
            meter: caps::meter::TOKENS,
            period: Some(caps::period::DAY),
            counted_from: None,
            amount: Some("1"),
            at_limit: caps::at_limit::STOP,
            warn_percent: None,
            uncounted: caps::uncounted::ALLOW,
            note: "Synthetic terminal accounting reservation",
            created_by: rig.ada,
            at: rig.clock.now(),
        },
    )
    .await?
    .ok_or("fixture cap was not made")?;
    let pending = caps::running(&mut conn, &cap, None, rig.clock.now()).await?;
    assert_eq!(pending.len(), 1);
    assert_eq!(pending[0].turn_id, turn);
    assert_eq!(
        pending[0]
            .usage
            .as_ref()
            .and_then(|value| value.get("total_tokens")),
        Some(&json!(usage(1).total_tokens))
    );
    cap.meter = caps::meter::RUNNING.into();
    assert!(
        caps::running(&mut conn, &cap, None, rig.clock.now())
            .await?
            .is_empty(),
        "a terminal reservation is not a running turn"
    );
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn typed_overrides_refuse_invalid_shapes_without_changing_the_profile_revision() -> Proof {
    let rig = Rig::new().await;
    let http = reqwest::Client::builder().timeout(PATIENCE).build()?;
    // Begin with a successfully validated nonempty override. Failed writes must
    // preserve that actual choice as well as the profile revision and descriptor.
    body(
        agent_patch(
            &rig,
            &http,
            json!({"harness":{"name":"Existing synthetic override"}}),
        )
        .await?,
        PAGE_BYTES,
    )
    .await?;
    let before = get(&rig, &http, &format!("/agents/{}", rig.agent), PAGE_BYTES).await?;
    for invalid in [
        json!({"harness":0}),
        json!({"capabilities":"invalid"}),
        json!({"parameters":false}),
    ] {
        let refused = agent_patch(&rig, &http, invalid).await?;
        assert_eq!(refused.status(), reqwest::StatusCode::BAD_REQUEST);
        let after = get(&rig, &http, &format!("/agents/{}", rig.agent), PAGE_BYTES).await?;
        assert_eq!(after["revision"], before["revision"]);
        assert_eq!(after["descriptor"], before["descriptor"]);
    }
    let good = body(
        agent_patch(
            &rig,
            &http,
            json!({"harness":{"name":"Synthetic named harness"}}),
        )
        .await?,
        PAGE_BYTES,
    )
    .await?
    .0;
    assert_eq!(
        good["descriptor"]["harness"]["name"],
        "Synthetic named harness"
    );
    assert_eq!(
        good["descriptor"]["harness"]["version"],
        before["descriptor"]["harness"]["version"]
    );
    rig.server.engine.stop();
    rig.server.stop().await;
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn completed_failed_and_interrupted_native_turns_clear_running_before_bad_existing_accounting_and_replay_frozen_usage()
-> Proof {
    for (outcome, expected) in [
        (TurnOutcome::Completed, "completed"),
        (TurnOutcome::Failed, "failed"),
        (TurnOutcome::Interrupted, "interrupted"),
    ] {
        let rig = Rig::tuned(|tuning| tuning.sweep_every = std::time::Duration::ZERO).await;
        native(&rig).await?;
        let http = reqwest::Client::builder().timeout(PATIENCE).build()?;
        let thread = rig.thread("Synthetic terminal recovery").await;
        let diagnostics = terminal_work_diagnostics::Watch::new(thread)?;
        let before = get(
            &rig,
            &http,
            &format!("/threads/{thread}/sync?limit=20"),
            PAGE_BYTES,
        )
        .await?;
        let seq = before["seq"].as_i64().ok_or("initial sequence absent")?;
        let mut socket = subscribe(&rig, thread, seq).await?;
        let mut seen = Observed::new(seq);
        let (start, original_input) = input(&rig, &http, thread, 0, "terminal-original").await?;
        rig.begin_message(thread, &start, "terminal-answer").await;
        report(&rig, thread, &start, 1).await;
        rig.wait_turn(start.turn_id, "running").await;
        if outcome == TurnOutcome::Completed {
            visible(&mut socket, &mut seen, &rig, thread, true).await?;
        }
        // Exact legacy persisted configuration admitted by older validation. New API
        // writes are tested separately; this must not be repaired by hiding the trigger.
        sqlx::query("UPDATE agents SET descriptor_overrides=$2 WHERE id=$1")
            .bind(rig.agent)
            .bind(json!({"harness":0}))
            .execute(rig.pool.pool())
            .await?;
        rig.say(thread, realtime_support::turn_ended(start.turn_id, outcome))
            .await;
        seen.terminal(&mut socket, &rig, thread, start.turn_id, expected)
            .await?;
        assert!(seen.max_frame < PAGE_BYTES);
        let original = retained(&rig, start.turn_id).await?;
        assert_eq!(original["usage"], json!(usage(1)));
        assert!(!original["at"].is_null());
        if outcome == TurnOutcome::Completed {
            assert_eq!(original["watching_author"], json!(rig.ada));
            // The person leaves before replay; suppression belongs to the actual end,
            // rather than whichever page happens to be visible during retry.
            visible(&mut socket, &mut seen, &rig, thread, false).await?;
        }
        assert_eq!(
            ledger(&rig, start.turn_id).await?,
            (0, 0),
            "failed accounting remains due rather than disappearing"
        );
        let state: (String,Option<Id>,String) = sqlx::query_as("SELECT t.state,t.current_turn_id,u.state FROM threads t JOIN turns u ON u.id=$2 WHERE t.node_id=$1")
            .bind(thread).bind(start.turn_id).fetch_one(rig.pool.pool()).await?;
        assert!(matches!(state.0.as_str(), "idle" | "blocked"));
        assert_eq!(state.1, None);
        assert_eq!(state.2, expected);
        if state.0 == "blocked" {
            let cause: String =
                sqlx::query_scalar("SELECT blocked_detail FROM threads WHERE node_id=$1")
                    .bind(thread)
                    .fetch_one(rig.pool.pool())
                    .await?;
            assert_eq!(
                cause,
                "The last turn ended, but its usage and notifications could not be saved. Check the agent profile or storage."
            );
        }
        // Repeated native terminal delivery must preserve the exact pending snapshot.
        rig.say(thread, realtime_support::turn_ended(start.turn_id, outcome))
            .await;
        assert_eq!(retained(&rig, start.turn_id).await?, original);
        // A later report remains an observation; it cannot replace the frozen original
        // usage in a work item whose ledger receipt is the original turn identity.
        let later = orch_link_contract::events::UsageUpdated {
            turn_id: start.turn_id,
            turn: usage(2),
            thread: usage(2),
            context_window: None,
            context_used: None,
            report: None,
        };
        rig.say(thread, Event::UsageUpdated(later)).await;
        usage_committed(&rig, start.turn_id, usage(2).total_tokens).await;
        assert_eq!(retained(&rig, start.turn_id).await?, original);
        reserve_cap(&rig, start.turn_id).await?;
        // Direct HTTP admission cannot pass the previous turn's pending accounting.
        let pending = body(http.post(url(&rig, &format!("/threads/{thread}/inputs"))).bearer_auth(&rig.token)
            .json(&json!({"content":[{"type":"text","text":"Stay queued"}],"delivery":"queue","client_id":"terminal-next"}))
            .send().await?, PAGE_BYTES).await?.0;
        let queued: Id = serde_json::from_value(pending["input"]["id"].clone())?;
        let count: i64 = sqlx::query_scalar("SELECT count(*) FROM turns WHERE thread_id=$1")
            .bind(thread)
            .fetch_one(rig.pool.pool())
            .await?;
        assert_eq!(count, 1);
        let cancelled = http
            .delete(url(&rig, &format!("/threads/{thread}/inputs/{queued}")))
            .bearer_auth(&rig.token)
            .send()
            .await?;
        body(cancelled, PAGE_BYTES).await?;
        // A fresh API engine must recover the durable work, not an in-memory callback.
        diagnostics
            .checkpoint(&rig, start.turn_id, &original, "before_old_engine_stop")
            .await;
        rig.server.engine.stop();
        let recovered = Server::over(
            &rig.pool,
            &rig.clock,
            quick(&orch_config::ServerConfig::default()),
        )
        .await;
        recovered.engine.start();
        diagnostics
            .checkpoint(&rig, start.turn_id, &original, "after_recovered_start")
            .await;
        let current = get(&rig, &http, &format!("/agents/{}", rig.agent), PAGE_BYTES).await?;
        let fixed = http
            .patch(format!(
                "http://{}/api/v1/agents/{}",
                recovered.addr, rig.agent
            ))
            .bearer_auth(&rig.token)
            .json(&json!({"expected_revision":current["revision"],"descriptor_overrides":{}}))
            .send()
            .await?;
        body(fixed, PAGE_BYTES).await?;
        diagnostics
            .checkpoint(&rig, start.turn_id, &original, "after_valid_profile_patch")
            .await;
        diagnostics.accounted(&rig, start.turn_id, &original).await;
        assert_eq!(
            ledger(&rig, start.turn_id).await?,
            (1, usage(1).total_tokens)
        );
        if outcome == TurnOutcome::Completed {
            let notices:i64=sqlx::query_scalar("SELECT count(*) FROM notifications WHERE user_id=$1 AND thread_id=$2 AND kind='turn.completed'")
                .bind(rig.ada).bind(thread).fetch_one(rig.pool.pool()).await?;
            assert_eq!(
                notices, 0,
                "a visible completion stays suppressed after the person leaves during retry"
            );
        }
        let unchanged_input: String =
            sqlx::query_scalar("SELECT state FROM thread_inputs WHERE id=$1")
                .bind(original_input)
                .fetch_one(rig.pool.pool())
                .await?;
        assert_eq!(unchanged_input, "delivered");
        let at: chrono::DateTime<chrono::Utc> =
            sqlx::query_scalar("SELECT at FROM usage_events WHERE turn_id=$1 AND counted")
                .bind(start.turn_id)
                .fetch_one(rig.pool.pool())
                .await?;
        let frozen_at: chrono::DateTime<chrono::Utc> =
            serde_json::from_value(original["at"].clone())?;
        assert_eq!(
            at.timestamp_micros(),
            frozen_at.timestamp_micros(),
            "the ledger preserves the frozen end at PostgreSQL timestamp precision"
        );
        let row = get(
            &rig,
            &http,
            &format!("/threads/{thread}/sync?limit=20"),
            PAGE_BYTES,
        )
        .await?;
        assert_eq!(row["thread"]["state"], "idle");
        assert!(row["thread"]["current_turn_id"].is_null());
        let mut reconnected = Client::signed_in(recovered.addr, &rig.token).await;
        reconnected
            .sub(1, &format!("thread:{thread}"), Some(seq))
            .await;
        assert_eq!(reconnected.expect("subscribed").await["mode"], "delta");
        reconnected.expect("presence").await;
        let mut replay = Observed::new(seq);
        replay
            .terminal(&mut reconnected, &rig, thread, start.turn_id, expected)
            .await?;
        // Replay and another recovery owner cannot double-count a successful receipt.
        recovered.engine.wake(thread);
        accounted(&rig, thread).await;
        assert_eq!(
            ledger(&rig, start.turn_id).await?,
            (1, usage(1).total_tokens)
        );
        recovered.engine.stop();
        recovered.stop().await;
        rig.server.stop().await;
    }
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_existing_terminal_current_pointer_is_repaired_even_while_its_bad_profile_keeps_followup_due()
-> Proof {
    let rig = Rig::new().await;
    native(&rig).await?;
    let http = reqwest::Client::builder().timeout(PATIENCE).build()?;
    let thread = rig.thread("Synthetic old partial end").await;
    let (start, _) = input(&rig, &http, thread, 0, "old-partial").await?;
    rig.begin_message(thread, &start, "old-answer").await;
    report(&rig, thread, &start, 1).await;
    rig.wait_turn(start.turn_id, "running").await;
    rig.server.engine.stop();
    // Reconstruct exactly the old committed partial end, without a new work marker.
    sqlx::query("UPDATE agents SET descriptor_overrides=$2 WHERE id=$1")
        .bind(rig.agent)
        .bind(json!({"harness":0}))
        .execute(rig.pool.pool())
        .await?;
    sqlx::query("UPDATE turns SET state='completed',completed_at=$2,native_end_confirmed=true,end_work=NULL WHERE id=$1")
        .bind(start.turn_id).bind(rig.clock.now()).execute(rig.pool.pool()).await?;
    let original: (chrono::DateTime<chrono::Utc>, i64) =
        sqlx::query_as("SELECT completed_at,completed_seq FROM turns WHERE id=$1")
            .bind(start.turn_id)
            .fetch_one(rig.pool.pool())
            .await?;
    let before = get(
        &rig,
        &http,
        &format!("/threads/{thread}/sync?limit=20"),
        PAGE_BYTES,
    )
    .await?;
    assert_eq!(before["thread"]["state"], "running");
    assert!(before["thread"]["sides"]["mode"].is_null());
    assert_eq!(
        before["thread"]["sides"]["reason"],
        "This agent profile's overrides do not match its harness declaration."
    );
    let seq = before["seq"].as_i64().ok_or("sequence absent")?;
    let repaired = Server::over(
        &rig.pool,
        &rig.clock,
        quick(&orch_config::ServerConfig::default()),
    )
    .await;
    let mut socket = Client::signed_in(repaired.addr, &rig.token).await;
    socket.sub(1, &format!("thread:{thread}"), Some(seq)).await;
    socket.expect("subscribed").await;
    socket.expect("presence").await;
    repaired.engine.start();
    let mut seen = Observed::new(seq);
    seen.terminal(&mut socket, &rig, thread, start.turn_id, "completed")
        .await?;
    let after: (chrono::DateTime<chrono::Utc>, i64, i64) =
        sqlx::query_as("SELECT completed_at,completed_seq,seq_updated FROM turns WHERE id=$1")
            .bind(start.turn_id)
            .fetch_one(rig.pool.pool())
            .await?;
    assert_eq!((after.0, after.1), original);
    assert!(after.2 > seq);
    assert_eq!(
        retained(&rig, start.turn_id).await?["usage"],
        json!(usage(1))
    );
    assert_eq!(ledger(&rig, start.turn_id).await?, (0, 0));
    repaired.engine.stop();
    repaired.stop().await;
    rig.server.stop().await;
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_notification_write_failure_rolls_back_accounting_and_retries_one_frozen_outcome() -> Proof
{
    let rig = Rig::new().await;
    native(&rig).await?;
    let http = reqwest::Client::builder().timeout(PATIENCE).build()?;
    let thread = rig.thread("Synthetic notification failure").await;
    let before = get(
        &rig,
        &http,
        &format!("/threads/{thread}/sync?limit=20"),
        PAGE_BYTES,
    )
    .await?;
    let seq = before["seq"].as_i64().ok_or("sequence absent")?;
    let mut socket = subscribe(&rig, thread, seq).await?;
    let (start, _) = input(&rig, &http, thread, 0, "notification-original").await?;
    rig.begin_message(thread, &start, "notification-answer")
        .await;
    report(&rig, thread, &start, 1).await;
    // PostgreSQL sequences are not rolled back: this proves the failure point was
    // reached, instead of merely asserting before a supervisor got to the work.
    sqlx::query("CREATE SEQUENCE test_terminal_notification_attempt")
        .execute(rig.pool.pool())
        .await?;
    sqlx::query("CREATE FUNCTION test_terminal_notification_failure() RETURNS trigger LANGUAGE plpgsql AS $$
        BEGIN IF NEW.kind='turn.failed' THEN PERFORM nextval('test_terminal_notification_attempt');
            RAISE EXCEPTION 'synthetic notification write failure' USING ERRCODE='40001'; END IF; RETURN NEW; END $$")
        .execute(rig.pool.pool()).await?;
    sqlx::query(
        "CREATE TRIGGER test_terminal_notification_failure BEFORE INSERT ON notifications
        FOR EACH ROW EXECUTE FUNCTION test_terminal_notification_failure()",
    )
    .execute(rig.pool.pool())
    .await?;
    rig.say(
        thread,
        realtime_support::turn_ended(start.turn_id, TurnOutcome::Failed),
    )
    .await;
    let mut seen = Observed::new(seq);
    seen.terminal(&mut socket, &rig, thread, start.turn_id, "failed")
        .await?;
    realtime_support::eventually("the actual notification failure point", || async {
        sqlx::query_scalar::<_, bool>("SELECT is_called FROM test_terminal_notification_attempt")
            .fetch_one(rig.pool.pool())
            .await
            .unwrap_or(false)
    })
    .await;
    let saved = retained(&rig, start.turn_id).await?;
    assert_eq!(saved["usage"], json!(usage(1)));
    assert_eq!(ledger(&rig, start.turn_id).await?, (0, 0));
    let parallel = Server::over(
        &rig.pool,
        &rig.clock,
        quick(&orch_config::ServerConfig::default()),
    )
    .await;
    parallel.engine.start();
    sqlx::query("DROP TRIGGER test_terminal_notification_failure ON notifications")
        .execute(rig.pool.pool())
        .await?;
    accounted(&rig, thread).await;
    assert_eq!(
        ledger(&rig, start.turn_id).await?,
        (1, usage(1).total_tokens)
    );
    let notices: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM notifications WHERE thread_id=$1 AND kind='turn.failed'",
    )
    .bind(thread)
    .fetch_one(rig.pool.pool())
    .await?;
    assert_eq!(notices, 1);
    rig.say(
        thread,
        realtime_support::turn_ended(start.turn_id, TurnOutcome::Failed),
    )
    .await;
    assert_eq!(
        ledger(&rig, start.turn_id).await?,
        (1, usage(1).total_tokens)
    );
    parallel.engine.stop();
    parallel.stop().await;
    rig.server.engine.stop();
    rig.server.stop().await;
    Ok(())
}
