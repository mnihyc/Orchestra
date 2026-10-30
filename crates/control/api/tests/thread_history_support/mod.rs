//! Real HTTP inputs and sequenced socket observations; all content and usage are synthetic.

use crate::realtime_support::{PATIENCE, Rig};
use orch_api_contract::threads::TokenUsage;
use orch_link_contract::commands::TurnStart;
use orch_link_contract::events::{Event, UsageUpdated};
use orch_types::Id;
use serde_json::{Value, json};
use std::time::{Duration, Instant};

pub type Proof<T = ()> = Result<T, Box<dyn std::error::Error + Send + Sync>>;
pub const PAGE_BYTES: usize = 1024 * 1024;

/// Per-turn usage includes repeated prompts; its live context remains a separate small measurement.
pub fn usage(rounds: i64) -> TokenUsage {
    TokenUsage {
        input_tokens: 70_000_000 * rounds,
        cached_input_tokens: 20_000_000 * rounds,
        cache_write_tokens: 5_000_000 * rounds,
        output_tokens: 8_125_000 * rounds,
        reasoning_tokens: 2_000_000 * rounds,
        total_tokens: 78_125_000 * rounds,
    }
}

pub async fn native(rig: &Rig) -> Proof {
    // This fixture's subscription is kept by this exact harness/machine. No forwarder
    // lease or paid provider endpoint belongs to the native credential path.
    sqlx::query("UPDATE providers SET key_machine_id=machine_id WHERE id=$1")
        .bind(rig.provider)
        .execute(rig.pool.pool())
        .await?;
    Ok(())
}

/// Bounded decoded bodies, with status/size/timing only; credentials/bodies are never logged.
pub async fn body(mut response: reqwest::Response, max: usize) -> Proof<(Value, usize, Duration)> {
    let status = response.status();
    let path = response.url().path().to_owned();
    let began = Instant::now();
    let mut bytes = Vec::new();
    while let Some(chunk) = response.chunk().await? {
        assert!(
            chunk.len() <= max.saturating_sub(bytes.len()),
            "HTTP body exceeds the fixture's {max}-byte budget"
        );
        bytes.extend_from_slice(&chunk);
    }
    if !status.is_success() {
        let code = serde_json::from_slice::<Value>(&bytes)
            .ok()
            .and_then(|value| value["code"].as_str().map(str::to_owned));
        panic!("native HTTP route {path} refused with {status}, code={code:?}");
    }
    Ok((
        serde_json::from_slice(&bytes)?,
        bytes.len(),
        began.elapsed(),
    ))
}

pub fn url(rig: &Rig, path: &str) -> String {
    format!("http://{}/api/v1{path}", rig.server.addr)
}

pub async fn get(rig: &Rig, http: &reqwest::Client, path: &str, max: usize) -> Proof<Value> {
    let began = Instant::now();
    let (value, bytes, _) = body(
        http.get(url(rig, path))
            .bearer_auth(&rig.token)
            .send()
            .await?,
        max,
    )
    .await?;
    eprintln!(
        "synthetic history HTTP read: bytes={bytes} elapsed_ms={}",
        began.elapsed().as_millis()
    );
    Ok(value)
}

pub async fn input(
    rig: &Rig,
    http: &reqwest::Client,
    thread: Id,
    nth: usize,
    client_id: &str,
) -> Proof<(TurnStart, Id)> {
    let posted = body(
        http.post(url(rig, &format!("/threads/{thread}/inputs")))
            .bearer_auth(&rig.token)
            .json(
                &json!({"content":[{"type":"text","text":format!("Synthetic interaction {nth}")}],
            "delivery":"queue","client_id":client_id}),
            )
            .send()
            .await?,
        PAGE_BYTES,
    )
    .await?
    .0;
    let input: Id = serde_json::from_value(posted["input"]["id"].clone())?;
    let start = rig.turn_start(thread, nth).await;
    let lease: Option<Id> = sqlx::query_scalar("SELECT lease_id FROM turns WHERE id=$1")
        .bind(start.turn_id)
        .fetch_one(rig.pool.pool())
        .await?;
    assert!(
        lease.is_none(),
        "the synthetic native reports must use the real no-lease ledger path"
    );
    Ok((start, input))
}

pub async fn report(rig: &Rig, thread: Id, start: &TurnStart, rounds: i64) {
    rig.say(
        thread,
        Event::UsageUpdated(UsageUpdated {
            turn_id: start.turn_id,
            turn: usage(1),
            thread: usage(rounds),
            context_window: Some(128_000),
            context_used: Some(4_096),
            report: None,
        }),
    )
    .await;
    usage_committed(rig, start.turn_id, usage(1).total_tokens).await;
}

/// Synchronizes the fixture's native event receipt, not a browser refresh for thread state.
pub async fn usage_committed(rig: &Rig, turn: Id, total: i64) {
    crate::realtime_support::eventually("the native usage event commits", || async {
        sqlx::query_scalar::<_, Option<i64>>(
            "SELECT (usage->>'total_tokens')::bigint FROM turns WHERE id=$1",
        )
        .bind(turn)
        .fetch_one(rig.pool.pool())
        .await
        .is_ok_and(|seen| seen == Some(total))
    })
    .await;
}

/// Internal proof synchronization only; no HTTP refresh or application polling is used for status.
pub async fn accounted(rig: &Rig, thread: Id) {
    let result = tokio::time::timeout(PATIENCE, async {
        loop {
            if sqlx::query_scalar::<_, bool>("SELECT NOT EXISTS(SELECT 1 FROM turns WHERE thread_id=$1 AND end_work IS NOT NULL)")
                .bind(thread).fetch_one(rig.pool.pool()).await.unwrap_or(false) { return; }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    }).await;
    if result.is_err() {
        rig.native_diagnostics(
            thread,
            "durable terminal accounting/notification work did not settle",
        )
        .await;
        panic!(
            "gave up waiting for durable terminal accounting/notification work within {PATIENCE:?}"
        );
    }
}
