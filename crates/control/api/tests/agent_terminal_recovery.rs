//! An actual profile repair retries original native accounting without moving the domain clock.

mod realtime_support;
mod thread_history_support;

use orch_link_contract::events::TurnOutcome;
use orch_types::Id;
use realtime_support::{PATIENCE, Rig, Server, quick};
use serde_json::{Value, json};
use thread_history_support::{
    PAGE_BYTES, Proof, accounted, body, get, input, native, report, url, usage,
};

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn repairing_the_recorded_agent_replays_frozen_terminal_usage_after_rebind_without_a_clock_tick()
-> Proof {
    let rig = Rig::with_time_held().await;
    native(&rig).await?;
    let held_at = rig.clock.now();
    let http = reqwest::Client::builder().timeout(PATIENCE).build()?;
    let thread = rig.thread("Synthetic profile repair event").await;
    let (start, original_input) = input(&rig, &http, thread, 0, "profile-repair-original").await?;
    rig.begin_message(thread, &start, "The original native answer")
        .await;
    report(&rig, thread, &start, 1).await;
    rig.wait_turn(start.turn_id, "running").await;
    // This exact invalid legacy row is the existing repair trigger, not a newly accepted API value.
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
    rig.wait_turn(start.turn_id, "completed").await;
    realtime_support::eventually(
        "the bad declaration actually failed terminal accounting",
        || async {
            sqlx::query_scalar::<_, bool>(
                "SELECT end_work IS NOT NULL AND end_work ? 'attempted_at' FROM turns WHERE id=$1",
            )
            .bind(start.turn_id)
            .fetch_one(rig.pool.pool())
            .await
            .unwrap_or(false)
        },
    )
    .await;
    let original: Value = sqlx::query_scalar("SELECT end_work->'snapshot' FROM turns WHERE id=$1")
        .bind(start.turn_id)
        .fetch_one(rig.pool.pool())
        .await?;
    assert_eq!(original["usage"], json!(usage(1)));
    let before: i64 =
        sqlx::query_scalar("SELECT count(*) FROM usage_events WHERE turn_id=$1 AND counted")
            .bind(start.turn_id)
            .fetch_one(rig.pool.pool())
            .await?;
    assert_eq!(before, 0);
    // A later binding choice cannot change the agent of the already recorded segment.
    let replacement = body(
        http.post(url(&rig, &format!("/orgs/{}/agents", rig.root)))
            .bearer_auth(&rig.token)
            .json(&json!({"name":"A later profile","harness_kind":"fake","profile":{}}))
            .send()
            .await?,
        PAGE_BYTES,
    )
    .await?
    .0;
    let replacement: Id = serde_json::from_value(replacement["id"].clone())?;
    sqlx::query("UPDATE threads SET agent_id=$2 WHERE node_id=$1")
        .bind(thread)
        .bind(replacement)
        .execute(rig.pool.pool())
        .await?;
    let original_agent: Id = sqlx::query_scalar(
        "SELECT s.agent_id FROM turns t JOIN segments s ON s.id=t.segment_id WHERE t.id=$1",
    )
    .bind(start.turn_id)
    .fetch_one(rig.pool.pool())
    .await?;
    assert_eq!(original_agent, rig.agent);
    assert_ne!(replacement, original_agent);
    rig.server.engine.stop();
    let recovered = Server::over(
        &rig.pool,
        &rig.clock,
        quick(&orch_config::ServerConfig::default()),
    )
    .await;
    recovered.engine.start();
    let current = get(&rig, &http, &format!("/agents/{}", rig.agent), PAGE_BYTES).await?;
    body(
        http.patch(format!(
            "http://{}/api/v1/agents/{}",
            recovered.addr, rig.agent
        ))
        .bearer_auth(&rig.token)
        .json(&json!({"expected_revision":current["revision"],"descriptor_overrides":{}}))
        .send()
        .await?,
        PAGE_BYTES,
    )
    .await?;
    // No test wake, clock advance, work reset or synthetic counted receipt can satisfy this condition.
    accounted(&rig, thread).await;
    let ledger:(i64,i64,Option<chrono::DateTime<chrono::Utc>>)=sqlx::query_as(
        "SELECT count(*),COALESCE(sum(total_tokens),0)::bigint,MIN(at) FROM usage_events WHERE turn_id=$1 AND counted")
        .bind(start.turn_id).fetch_one(rig.pool.pool()).await?;
    assert_eq!((ledger.0, ledger.1), (1, usage(1).total_tokens));
    let original_at: chrono::DateTime<chrono::Utc> =
        serde_json::from_value(original["at"].clone())?;
    assert_eq!(
        ledger.2.map(|at| at.timestamp_micros()),
        Some(original_at.timestamp_micros())
    );
    let state:(String,Option<Id>,String)=sqlx::query_as(
        "SELECT t.state,t.current_turn_id,i.state FROM threads t JOIN thread_inputs i ON i.id=$2 WHERE t.node_id=$1")
        .bind(thread).bind(original_input).fetch_one(rig.pool.pool()).await?;
    assert_eq!(state, ("idle".to_owned(), None, "delivered".to_owned()));
    assert_eq!(rig.turn_starts(thread).len(), 1);
    assert_eq!(
        rig.clock.now(),
        held_at,
        "repair is an event, not a mock-clock retry threshold"
    );
    // A duplicate successful PATCH stays a no-op for the original terminal receipt.
    let current = body(
        http.get(format!(
            "http://{}/api/v1/agents/{}",
            recovered.addr, rig.agent
        ))
        .bearer_auth(&rig.token)
        .send()
        .await?,
        PAGE_BYTES,
    )
    .await?
    .0;
    body(
        http.patch(format!(
            "http://{}/api/v1/agents/{}",
            recovered.addr, rig.agent
        ))
        .bearer_auth(&rig.token)
        .json(&json!({"expected_revision":current["revision"],"descriptor_overrides":{}}))
        .send()
        .await?,
        PAGE_BYTES,
    )
    .await?;
    let count: i64 =
        sqlx::query_scalar("SELECT count(*) FROM usage_events WHERE turn_id=$1 AND counted")
            .bind(start.turn_id)
            .fetch_one(rig.pool.pool())
            .await?;
    assert_eq!(count, 1);
    recovered.engine.stop();
    recovered.stop().await;
    rig.server.stop().await;
    Ok(())
}
