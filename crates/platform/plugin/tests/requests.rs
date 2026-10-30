//! Requests that overlap, are answered out of order, report progress, are cancelled or run out of time, with real
//! programs.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]

mod support;

use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use orch_plugin::requests::{CallError, Observer};
use orch_plugin::supervisor::Plugin;
use orch_plugin_contract::messages::{ProgressParams, RequestId};
use orch_plugin_contract::{PluginKind, PluginState};
use serde_json::json;
use support::{Work, advance, clock, options, registration, script, settle, wait_until};

/// A summarizer-shaped plugin that answers the methods of `responses`, started on a clock moved by hand.
async fn started(
    responses: &serde_json::Value,
    args: &[&str],
) -> (Plugin, Work, orch_types::ManualClock) {
    let clock = clock();
    let work = Work::new();
    let plugin = Plugin::start(
        &registration(
            PluginKind::Summarizer,
            "rules",
            &work,
            &script(
                PluginKind::Summarizer,
                &["describe", "summarize"],
                responses,
            ),
            args,
        ),
        options(&clock),
    )
    .await
    .unwrap();
    plugin.settled().await;
    assert_eq!(plugin.report().state, PluginState::Running);
    (plugin, work, clock)
}

#[derive(Default)]
struct Progress(Mutex<Vec<(RequestId, String, Option<String>)>>);

#[async_trait::async_trait]
impl Observer for Progress {
    fn progress(&self, request: RequestId, method: &str, progress: ProgressParams) {
        self.0
            .lock()
            .unwrap()
            .push((request, method.to_owned(), progress.message));
    }
}

#[tokio::test]
async fn two_requests_overlap_and_each_gets_its_own_answer_while_the_slower_one_is_still_working() {
    let (plugin, _work, _clock) = started(
        &json!({
            "slow": { "result": { "which": "slow" }, "delay_ms": 400 },
            "fast": { "result": { "which": "fast" } }
        }),
        &[],
    )
    .await;
    let began = Instant::now();
    let slow = plugin.start_call("slow", json!({})).unwrap();
    let fast = plugin.start_call("fast", json!({})).unwrap();
    assert_eq!((slow.id(), fast.id()), (1, 2), "the handshake was 0");
    let fast = tokio::spawn(async move { (fast.await, began.elapsed()) });
    let slow = tokio::spawn(async move { (slow.await, began.elapsed()) });
    let (fast_answer, fast_after) = fast.await.unwrap();
    let (slow_answer, slow_after) = slow.await.unwrap();
    assert_eq!(fast_answer.unwrap()["which"], "fast");
    assert_eq!(slow_answer.unwrap()["which"], "slow");
    assert!(
        fast_after < slow_after,
        "the quick one did not wait for the slow one"
    );
    assert!(slow_after >= Duration::from_millis(380));
    plugin.shutdown().await;
}

#[tokio::test]
async fn answers_that_come_in_the_opposite_order_are_matched_to_their_requests_by_id() {
    let (plugin, _work, _clock) = started(
        &json!({ "echo": { "result": { "n": { "$params": "n" } } } }),
        &["--out-of-order", "3"],
    )
    .await;
    let order = Arc::new(Mutex::new(Vec::new()));
    let mut waiting = Vec::new();
    for n in 1..=3 {
        let call = plugin.start_call("echo", json!({ "n": n })).unwrap();
        let order = Arc::clone(&order);
        waiting.push(tokio::spawn(async move {
            let answer = call.await.unwrap();
            order.lock().unwrap().push(n);
            assert_eq!(
                answer["n"], n,
                "request {n} got the answer to another request"
            );
        }));
    }
    for task in waiting {
        task.await.unwrap();
    }
    assert_eq!(
        *order.lock().unwrap(),
        [3, 2, 1],
        "the answers came last first"
    );
    plugin.shutdown().await;
}

#[tokio::test]
async fn a_request_that_is_cancelled_ends_at_once_and_the_plugin_is_told_to_stop_working_on_it() {
    let (plugin, work, _clock) = started(
        &json!({ "work": { "result": {}, "delay_ms": 60_000, "progress": [ { "message": "working" } ] } }),
        &[],
    )
    .await;
    let progress = Arc::new(Progress::default());
    plugin.set_observer(Arc::clone(&progress) as Arc<dyn Observer>);
    let call = plugin.start_call("work", json!({})).unwrap();
    let id = call.id();
    wait_until("the plugin reported progress", || {
        !progress.0.lock().unwrap().is_empty()
    })
    .await;
    assert_eq!(
        progress.0.lock().unwrap()[0],
        (id, "work".to_owned(), Some("working".to_owned())),
        "progress is shown where the operation is shown, with the request's id"
    );

    let began = Instant::now();
    call.cancel();
    assert_eq!(call.await, Err(CallError::Cancelled));
    assert!(
        began.elapsed() < Duration::from_secs(5),
        "the wait ended at once"
    );
    wait_until("the plugin received the cancel", || {
        work.received("cancel")
            .iter()
            .any(|line| line["params"]["id"] == id)
    })
    .await;
    settle().await;
    assert_eq!(
        plugin.report().state,
        PluginState::Running,
        "its own `cancelled` answer was no surprise"
    );
    plugin.shutdown().await;
}

#[tokio::test]
async fn a_request_that_is_given_up_is_cancelled_as_well() {
    let (plugin, work, _clock) = started(
        &json!({ "work": { "result": {}, "delay_ms": 60_000 } }),
        &[],
    )
    .await;
    let call = plugin.start_call("work", json!({})).unwrap();
    let id = call.id();
    wait_until("the plugin has the request", || {
        !work.received("work").is_empty()
    })
    .await;
    drop(call);
    wait_until("the plugin received the cancel", || {
        work.received("cancel")
            .iter()
            .any(|line| line["params"]["id"] == id)
    })
    .await;
    plugin.shutdown().await;
}

#[tokio::test]
async fn a_request_that_is_not_answered_by_its_deadline_fails_as_a_retryable_timeout_whatever_progress_it_reported()
 {
    let (plugin, work, clock) = started(
        &json!({ "work": { "result": {}, "delay_ms": 600_000, "progress": [ { "message": "still here" } ] } }),
        &[],
    )
    .await;
    let progress = Arc::new(Progress::default());
    plugin.set_observer(Arc::clone(&progress) as Arc<dyn Observer>);
    let call = plugin.start_call("work", json!({})).unwrap();
    let id = call.id();
    wait_until("the plugin reported progress", || {
        !progress.0.lock().unwrap().is_empty()
    })
    .await;
    advance(&clock, 59);
    settle().await;
    assert!(
        work.received("cancel").is_empty(),
        "sixty seconds are sixty seconds, progress or not"
    );
    advance(&clock, 1);
    let failed = call.await.unwrap_err();
    assert_eq!(
        failed,
        CallError::Timeout {
            method: "work".into()
        }
    );
    assert_eq!(failed.code(), "timeout");
    assert!(failed.retryable());
    wait_until("the plugin was told to stop", || {
        work.received("cancel")
            .iter()
            .any(|line| line["params"]["id"] == id)
    })
    .await;
    settle().await;
    assert_eq!(
        plugin.report().state,
        PluginState::Running,
        "a request that ran out of time does not take the plugin down with it"
    );
    plugin.shutdown().await;
}

#[tokio::test]
async fn lines_that_are_no_message_are_skipped_and_counted_and_the_answers_are_read_all_the_same() {
    let (plugin, _work, _clock) = started(
        &json!({ "echo": { "result": { "n": { "$params": "n" } } } }),
        &["--garbage", "3"],
    )
    .await;
    for n in [1, 2] {
        let answer = plugin.call("echo", json!({ "n": n })).await.unwrap();
        assert_eq!(answer["n"], n);
    }
    assert_eq!(
        plugin.report().malformed_lines,
        6,
        "three before each answer"
    );
    assert_eq!(plugin.report().state, PluginState::Running);
    plugin.shutdown().await;
}

#[tokio::test]
async fn a_plugin_that_is_down_fails_a_call_as_unavailable_with_its_name() {
    let clock = clock();
    let work = Work::new();
    let plugin = Plugin::start(
        &registration(
            PluginKind::Summarizer,
            "rules",
            &work,
            &script(
                PluginKind::Summarizer,
                &["describe", "summarize"],
                &json!({}),
            ),
            &["--exit-after-hello", "--exit-code", "3"],
        ),
        options(&clock),
    )
    .await
    .unwrap();
    wait_until("it went away", || {
        plugin.report().state == PluginState::Restarting && plugin.report().settled
    })
    .await;
    let refused = plugin.call("summarize", json!({})).await.unwrap_err();
    let CallError::Unavailable {
        plugin: name,
        detail,
    } = &refused
    else {
        panic!("unavailable, not {refused:?}");
    };
    assert_eq!(name, "rules");
    assert!(detail.contains("exit status: 3"), "{detail}");
    assert!(refused.retryable());
    plugin.shutdown().await;
}
