//! Native wake, authority and usage controls share the ordinary Engine admission path.
#![cfg(target_os = "linux")]
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "native integration assertions"
)]
mod background_wake_bridge;
#[path = "../../../../extensions/agents/claude-code/tests/common/mod.rs"]
mod claude_native;
#[path = "../../../../extensions/agents/codex/tests/common/mod.rs"]
mod codex_native;
mod common;
use background_wake_bridge::{Bridge, Kind};
use orch_agent_contract::background::BackgroundTaskKind;
use orch_authz::Caller;
use orch_link_contract::commands::CommandKind;
use serde_json::json;

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_real_claude_command_wake_is_admitted_under_its_origin_and_counted_once() {
    background_wake_bridge::run_case("Claude command wake and accounting", async {
        let mut proof = Bridge::start(Kind::Claude, false, true).await;
        proof.finish().await;
        assert!(proof.state.native_wake);
        assert!(proof.state.checked_markers.contains(&proof.wake.unwrap()));
        proof.prove_replay_accounting().await;
        proof.close().await;
    })
    .await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_real_claude_child_wake_preserves_its_origin_and_precedes_output() {
    background_wake_bridge::run_case("Claude child wake and original accounting", async {
    let mut proof = Bridge::start(Kind::Claude, true, true).await;
    proof.finish().await;
    assert!(
        proof
            .state
            .finished
            .iter()
            .any(|task| task.kind == BackgroundTaskKind::Subagent)
    );
    assert!(proof.state.checked_markers.contains(&proof.wake.unwrap()));
    let rows = proof.world.ledger_of(proof.origin).await;
    let child: Vec<_> = rows
        .iter()
        .filter(|row| row.counted && row.timing().subagent)
        .collect();
    assert_eq!(
        child.iter().map(|row| row.total_tokens).sum::<i64>(),
        180,
        "both actual child model requests, including the one after the barrier, belong to the original input"
    );
    assert!(child.iter().all(|row| row.user_id == Some(proof.author)));
    proof.close().await;
    }).await;
}

async fn refused_native_followup(child: bool) {
    let mut proof = Bridge::start(Kind::Claude, child, true).await;
    proof.world.take(proof.author_role).await;
    tokio::fs::write(&proof.release, b"finish").await.unwrap();
    proof
        .pump_until(|state| state.interrupted && state.quiet)
        .await;
    proof.flush().await;
    let detail:String=sqlx::query_scalar("SELECT state_detail FROM thread_inputs WHERE thread_id=$1 AND source='system' AND client_id LIKE 'background:%'")
        .bind(proof.thread).fetch_one(proof.world.pool.pool()).await.unwrap();
    assert_eq!(detail, "author_lost_access");
    assert_eq!(proof.world.turn_starts(proof.thread).len(), 1);
    assert!(
        proof.state.checked_markers.is_empty(),
        "no continuation output is admitted under revoked authority"
    );
    proof.close().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn revoked_interact_interrupts_the_real_native_followup_and_holds_its_completion() {
    refused_native_followup(false).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn revoked_interact_interrupts_the_real_child_followup_and_holds_its_completion() {
    refused_native_followup(true).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn off_removes_background_arguments_from_the_real_claude_offer_and_keeps_commands_foreground()
{
    let mut proof = Bridge::start(Kind::Claude, false, false).await;
    proof.flush().await;
    let requests = proof.native.requests();
    let bash = requests
        .iter()
        .filter(|request| request.path.ends_with("/messages"))
        .filter_map(|request| request.body["tools"].as_array())
        .flatten()
        .find(|tool| tool["name"] == "Bash")
        .expect("the actual native tool offer includes foreground Bash");
    assert!(
        bash["input_schema"]["properties"]
            .get("run_in_background")
            .is_none()
    );
    for tool in requests
        .iter()
        .filter(|request| request.path.ends_with("/messages"))
        .filter_map(|request| request.body["tools"].as_array())
        .flatten()
        .filter(|tool| tool["name"] == "Agent")
    {
        assert!(
            tool["input_schema"]["properties"]
                .get("run_in_background")
                .is_none(),
            "Off cannot offer a background subagent argument either"
        );
    }
    let completed:bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM items WHERE thread_id=$1 AND kind='tool_call' AND payload#>>'{tool,kind}'='command' AND payload#>>'{tool,exit_code}'='0' AND payload#>>'{tool,background}'='false')")
        .bind(proof.thread).fetch_one(proof.world.pool.pool()).await.unwrap();
    assert!(
        completed,
        "Off executes the ordinary command in the foreground"
    );
    assert!(proof.state.running.is_empty());
    assert!(!proof.state.native_wake);
    assert_eq!(proof.world.turn_starts(proof.thread).len(), 1);
    proof.close().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_persons_stop_holds_the_real_followup_until_a_new_human_input() {
    let mut proof = Bridge::start(Kind::Claude, false, true).await;
    proof
        .world
        .engine()
        .interrupt(&Caller::session(proof.author), proof.thread, proof.origin)
        .await
        .unwrap();
    tokio::fs::write(&proof.release, b"finish").await.unwrap();
    proof
        .pump_until(|state| state.interrupted && state.quiet)
        .await;
    proof.flush().await;
    let held:bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM thread_inputs WHERE thread_id=$1 AND state_detail='background_stopped' AND state='pending')")
        .bind(proof.thread).fetch_one(proof.world.pool.pool()).await.unwrap();
    assert!(held);
    assert_eq!(proof.world.turn_starts(proof.thread).len(), 1);
    proof
        .world
        .engine()
        .submit_input(
            &Caller::session(proof.author),
            proof.thread,
            common::queued("Continue after the stopped wake"),
        )
        .await
        .unwrap();
    proof.pump_until(|state| state.completed.len() > 1).await;
    assert!(proof.wake.is_some());
    proof.close().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn actual_native_process_loss_marks_its_running_tasks_lost_without_a_wake() {
    let mut proof = Bridge::start(Kind::Claude, false, true).await;
    proof.native.kill_owned();
    // Session loss clears the native process; the recorded task update is checked after its actual event.
    proof.pump_until(|state| state.lost).await;
    proof.flush().await;
    let lost:bool=sqlx::query_scalar(r#"SELECT EXISTS(SELECT 1 FROM items WHERE thread_id=$1 AND payload @? '$.background_tasks.* ? (@.task.state == "lost" && exists(@.lost_at))')"#)
        .bind(proof.thread).fetch_one(proof.world.pool.pool()).await.unwrap();
    assert!(lost);
    assert_eq!(proof.world.turn_starts(proof.thread).len(), 1);
    proof.close().await;
}

async fn codex_completion(child: bool) {
    let mut proof = Bridge::start(Kind::Codex, child, true).await;
    proof.finish().await;
    assert!(
        proof.state.config_off,
        "the actual launched native thread disables experimental message wakes"
    );
    assert!(
        !proof.state.native_wake,
        "the controller is the only wake path"
    );
    let start = proof
        .world
        .turn_starts(proof.thread)
        .into_iter()
        .find(|turn| Some(turn.turn_id) == proof.wake)
        .unwrap();
    let cause = proof
        .state
        .finished
        .iter()
        .find(|task| {
            task.kind
                == if child {
                    BackgroundTaskKind::Subagent
                } else {
                    BackgroundTaskKind::Command
                }
        })
        .unwrap();
    let expected = if child {
        format!("Subagent {} finished.", json!(cause.label))
    } else {
        let item: orch_types::Id =
            sqlx::query_scalar("SELECT id FROM items WHERE thread_id=$1 AND native_ref=$2")
                .bind(proof.thread)
                .bind(&cause.native_item_ref)
                .fetch_one(proof.world.pool.pool())
                .await
                .unwrap();
        format!(
            "Background task {} finished (exit {}). Its output is in the item {item}.",
            json!(cause.label),
            cause
                .exit_code
                .expect("the native terminal reports its actual exit")
        )
    };
    assert_eq!(start.text, expected);
    assert!(
        proof
            .state
            .native_inputs
            .iter()
            .any(|text| text == &expected),
        "the controller's exact words reach the actual native turn/start"
    );
    assert_eq!(
        proof.world.seen.of_kind(CommandKind::TurnInterrupt).len(),
        0
    );
    proof.close().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_real_codex_terminal_completion_starts_the_exact_owned_controller_input() {
    background_wake_bridge::run_case(
        "Codex terminal controller continuation",
        codex_completion(false),
    )
    .await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_real_codex_child_completion_starts_the_exact_owned_controller_input() {
    background_wake_bridge::run_case(
        "Codex child controller continuation",
        codex_completion(true),
    )
    .await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_real_codex_terminal_failure_wakes_with_its_actual_exit_code() {
    let mut proof = Bridge::start(Kind::Codex, false, true).await;
    tokio::fs::write(&proof.release, b"fail").await.unwrap();
    proof.pump_until(|state| state.completed.len() > 1).await;
    proof.flush().await;
    let cause = proof
        .state
        .finished
        .iter()
        .find(|task| task.kind == BackgroundTaskKind::Command)
        .unwrap();
    assert_eq!(cause.exit_code, Some(7));
    assert_eq!(
        cause.state,
        orch_agent_contract::background::BackgroundTaskState::Failed
    );
    let start = proof
        .world
        .turn_starts(proof.thread)
        .into_iter()
        .find(|turn| Some(turn.turn_id) == proof.wake)
        .unwrap();
    assert!(
        start
            .text
            .contains(" finished (exit 7). Its output is in the item ")
    );
    assert!(proof.state.config_off);
    proof.close().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn off_keeps_a_real_codex_terminal_and_delivers_its_result_with_the_next_input() {
    let mut proof = Bridge::start(Kind::Codex, false, false).await;
    proof.pump_until(|state| !state.running.is_empty()).await;
    let receipt = format!("native-result-{}", orch_types::new_id());
    tokio::fs::write(&proof.release, receipt.as_bytes())
        .await
        .unwrap();
    proof.pump_until(|state| !state.finished.is_empty()).await;
    proof.flush().await;
    proof.world.engine().preflight(proof.thread).await.unwrap();
    let held:bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM thread_inputs WHERE thread_id=$1 AND state_detail='background_wake_off' AND state='pending')")
        .bind(proof.thread).fetch_one(proof.world.pool.pool()).await.unwrap();
    assert!(held);
    assert_eq!(proof.world.turn_starts(proof.thread).len(), 1);
    proof
        .world
        .engine()
        .submit_input(
            &Caller::session(proof.author),
            proof.thread,
            common::queued("Tell me the completed result"),
        )
        .await
        .unwrap();
    proof.pump_until(|state| state.completed.len() > 1).await;
    proof.flush().await;
    let pending:bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM thread_inputs WHERE thread_id=$1 AND state_detail='background_wake_off' AND state='pending')")
        .bind(proof.thread).fetch_one(proof.world.pool.pool()).await.unwrap();
    assert!(
        !pending,
        "reporting with admitted ordinary input consumes the held completion"
    );
    let requests = proof.native.requests();
    let last = requests
        .iter()
        .rev()
        .find(|request| request.path.ends_with("/responses"))
        .unwrap();
    assert!(
        serde_json::to_string(&last.body)
            .unwrap()
            .contains(&receipt),
        "the actual model input receives output generated after the original prompt"
    );
    assert!(proof.state.config_off);
    assert!(proof.wake.is_none());
    proof.close().await;
}
