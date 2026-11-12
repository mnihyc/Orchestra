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

/// The parts of a use that are counted apart: the input, the part of it read from a cache and the part written to
/// one, and the output.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
struct Classes {
    input: i64,
    cached: i64,
    written: i64,
    output: i64,
}

impl Classes {
    fn total(self) -> i64 {
        self.input + self.output
    }
}

/// What the ledger counts for the turns of the case: the rows of requests and the closing of each turn's use.
async fn counted_over_the_turns(proof: &Bridge) -> Classes {
    let mut counted = Classes::default();
    for turn in [Some(proof.origin), proof.wake].into_iter().flatten() {
        for row in proof
            .world
            .ledger_of(turn)
            .await
            .iter()
            .filter(|row| row.counted)
        {
            counted.input += row.input_tokens;
            counted.cached += row.cached_input_tokens;
            counted.written += row.cache_write_tokens;
            counted.output += row.output_tokens;
        }
    }
    counted
}

/// The child's two requests are rows of the original input, and what the thread counted over its turns is what the model
/// server answered, part by part, each request once.
async fn the_children_rows_and_the_whole_are_conserved(proof: &Bridge) {
    let rows = proof.world.ledger_of(proof.origin).await;
    let child: Vec<_> = rows
        .iter()
        .filter(|row| row.counted && row.timing().subagent)
        .collect();
    assert_eq!(
        child.iter().map(|row| row.total_tokens).sum::<i64>(),
        180,
        "both actual child model requests, including the one after the barrier, belong to the original input\n  {}\n  {}",
        proof.accounting_facts().await,
        proof.request_digest()
    );
    assert!(child.iter().all(|row| row.user_id == Some(proof.author)));
    // Conservation: what the thread counted, over all its turns, is what the model server answered, each request
    // once. A request of the child that the wake's turn grew the program's totals by belongs to the original input
    // by its row, and must not be counted again in the wake's.
    let answers: Vec<_> = proof
        .native
        .requests()
        .into_iter()
        .filter_map(|request| {
            request.usage.map(|usage| {
                (
                    request.seq,
                    request.path,
                    Classes {
                        input: usage.input,
                        cached: usage.cached,
                        written: usage.cache_write,
                        output: usage.output,
                    },
                )
            })
        })
        .collect();
    let answered = answers
        .iter()
        .fold(Classes::default(), |sum, (_, _, each)| Classes {
            input: sum.input + each.input,
            cached: sum.cached + each.cached,
            written: sum.written + each.written,
            output: sum.output + each.output,
        });
    // The closing of the turns' use is work done after they end: wait for the ledger to hold what was answered, and
    // then look again, so that more than that would show.
    let began = std::time::Instant::now();
    while counted_over_the_turns(proof).await.total() < answered.total()
        && began.elapsed() < std::time::Duration::from_secs(30)
    {
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    }
    common::meanwhile(std::time::Duration::from_millis(500)).await;
    let counted = counted_over_the_turns(proof).await;
    // Each part of the use is conserved, not only the whole of it: the engine counts the input, the cache parts and
    // the output apart.
    assert_eq!(
        counted, answered,
        "the thread counted {counted:?} over its turns and the model server answered {answered:?}: {answers:?}\n  {}",
        proof.accounting_facts().await
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_real_claude_child_wake_preserves_its_origin_and_precedes_output() {
    background_wake_bridge::run_case("Claude child wake and original accounting", async {
    let mut proof = Bridge::start(Kind::Claude, true, true).await;
    proof.finish().await;
    // The real flow, as the pinned program runs it: the parent starts the sub-agent and then runs a command that only
    // waits for a file (`while [ ! -e … ]; do sleep …; done`), which the program allows by itself, so it is no
    // question. The one question a person is asked is the sub-agent's command, which makes the file the parent
    // waits for. The person allows it, the command begins, the parent's wait ends, and so does the parent's turn: no
    // question of the sub-agent was open when the turn ended.
    assert!(
        proof.child_command_begun(),
        "the sub-agent's command was allowed and began"
    );
    let allowed = proof.allowed_commands().await;
    assert!(
        allowed.len() == 1 && allowed[0].starts_with("touch "),
        "the one question put to a person was the sub-agent's command: {allowed:?}"
    );
    assert_eq!(
        proof.questions_asked().await,
        1,
        "no other question was put to a person: the parent's wait ran without one"
    );
    assert!(
        proof.state.taken_back.is_empty(),
        "no question was taken back: {:?}",
        proof.state.taken_back
    );
    assert!(
        proof
            .state
            .finished
            .iter()
            .any(|task| task.kind == BackgroundTaskKind::Subagent)
    );
    assert!(proof.state.checked_markers.contains(&proof.wake.unwrap()));
    the_children_rows_and_the_whole_are_conserved(&proof).await;
    proof.close().await;
    }).await;
}

/// The order the case is named for, from the frames of the program that the case saw: the sub-agent's first message is
/// printed block by block, the parent's result comes between its first block and its last, so that the totals of that
/// result cannot hold the message's cost (the program adds it when the message ends).
fn assert_the_message_was_unended_at_the_parents_result(proof: &Bridge) {
    let trace = &proof.state.trace;
    let result = trace.iter().position(|line| line.starts_with("result/"));
    let blocks: Vec<(usize, &str)> = trace
        .iter()
        .enumerate()
        .filter(|(_, line)| line.starts_with("assistant/") && !line.contains("parent=-"))
        .filter_map(|(at, line)| {
            line.split_whitespace()
                .find_map(|word| word.strip_prefix("message="))
                .map(|message| (at, message))
        })
        .collect();
    let seen = || format!("the frames the case saw:\n  {}", trace.join("\n  "));
    let (Some(result), Some(&(first, message))) = (result, blocks.first()) else {
        panic!("a result and a sub-agent message were both seen: {}", seen());
    };
    let last = blocks
        .iter()
        .filter(|(_, each)| *each == message)
        .map(|(at, _)| *at)
        .max()
        .unwrap_or(first);
    assert!(
        first < result && result < last,
        "the sub-agent's message {message} was printed (first block at frame {first}, last at {last}) around the parent's result (frame {result}): it was not unended at the result, and the case does not test what it is named for (the model server holds the parent's last message until the case has seen the first block, and the sub-agent's message after that block until the case releases it, so neither order is a matter of time: the program printed the message whole, or printed it later than the frames the case saw): {}",
        seen()
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_real_claude_child_message_the_program_has_not_ended_at_the_parents_result_is_counted_in_the_window_that_holds_its_cost() {
    background_wake_bridge::run_case("Claude child message unfinished at the parent's result", async {
        // The parent starts the sub-agent and ends; the sub-agent's first message is still being written (a text, a
        // pause, a call) when the parent's result comes, so the totals of that result do not hold it. Its cost enters
        // the next window, the wake's, which names it with the sub-agent's second request: both are rows of the original
        // input, and neither is counted again in the wake's turn.
        let mut proof = Bridge::start_streaming_child().await;
        // The parent's turn is over (the start waits for it) and the sub-agent's message is held after its first block:
        // it ends when the case lets it.
        proof.release_the_child();
        proof.finish().await;
        assert!(proof.state.checked_markers.contains(&proof.wake.unwrap()));
        assert_the_message_was_unended_at_the_parents_result(&proof);
        // The sub-agent reads a file of the workspace, which the pinned program allows by itself: that is the program's
        // part. The product's part is that no question was put to a person (nobody was asked for a word that was not
        // needed); if the program one day asks for a read, this says so and the case needs another call for the
        // sub-agent.
        assert_eq!(
            proof.questions_asked().await,
            0,
            "no question was put to a person (the pinned program allows a read in the workspace by itself)"
        );
        the_children_rows_and_the_whole_are_conserved(&proof).await;
        proof.close().await;
    })
    .await;
}

async fn refused_native_followup(child: bool) {
    let mut proof = Bridge::start(Kind::Claude, child, true).await;
    if child {
        assert!(
            proof.child_command_begun(),
            "the sub-agent's command was allowed and began"
        );
    }
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
    // The pinned Codex keeps a finished background terminal's output out of its model's history, and an agent
    // reads an Orch item only through tools a person has to enable, so the controller reports the completion with
    // the next input in the words it would have used for a continuation of its own, which name the item that holds
    // the output, followed by the end of that output.
    let cause = proof
        .state
        .finished
        .iter()
        .find(|task| task.kind == BackgroundTaskKind::Command)
        .unwrap();
    let item: orch_types::Id =
        sqlx::query_scalar("SELECT id FROM items WHERE thread_id=$1 AND native_ref=$2")
            .bind(proof.thread)
            .bind(&cause.native_item_ref)
            .fetch_one(proof.world.pool.pool())
            .await
            .unwrap();
    let words = format!(
        "finished (exit {}). Its output is in the item {item}.",
        cause
            .exit_code
            .expect("the native terminal reports its actual exit")
    );
    // What the item of the call keeps of the output, to settle at once whether the output reached the item before
    // the completion was reported or the carrying left it out.
    let kept: Option<serde_json::Value> =
        sqlx::query_scalar("SELECT payload->'tool'->'output' FROM items WHERE id=$1")
            .bind(item)
            .fetch_one(proof.world.pool.pool())
            .await
            .unwrap();
    assert!(
        serde_json::to_string(&last.body)
            .unwrap()
            .contains(&receipt),
        "the actual model input receives output generated after the original prompt\n  kept in the item={kept:?}\n  finished={:?}\n  {}\n  {}",
        proof.state.finished,
        proof.request_digest(),
        proof.state.trace.join("\n  ")
    );
    assert!(
        serde_json::to_string(&last.body).unwrap().contains(&words),
        "the actual model input receives the completion generated after the original prompt, in the controller's words {words:?}\n  finished={:?}\n  {}\n  {}",
        proof.state.finished,
        proof.request_digest(),
        proof.state.trace.join("\n  ")
    );
    assert!(proof.state.config_off);
    assert!(proof.wake.is_none());
    proof.close().await;
}
