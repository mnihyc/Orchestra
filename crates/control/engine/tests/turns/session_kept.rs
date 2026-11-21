//! A restarted server verifies a retained native session once; unchanged later turns keep that live session.
use super::*;

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_restarted_server_verifies_the_retained_session_once_and_later_turns_keep_it() {
    let world = World::new().await;
    let thread = world.thread("Kept session").await;
    let ada = Caller::session(world.ada);
    world
        .engine()
        .submit_input(&ada, thread, queued("First"))
        .await
        .unwrap();
    let first = world.run_turn(thread, 0, "One.").await;
    world.wait_state(thread, "idle").await;
    assert_eq!(world.ran(CommandKind::SessionOpen), 1);
    let segment = world.segment_of(thread).await;
    let native: String = world
        .scalar(
            "SELECT native_session_ref FROM segments WHERE id=$1",
            segment,
        )
        .await;
    world.restart_api().await;
    world
        .engine()
        .submit_input(&Caller::session(world.ada), thread, queued("Second"))
        .await
        .unwrap();
    let second = world.turn_start(thread, 1).await;
    assert_ne!(first.turn_id, second.turn_id);
    assert_eq!(
        world.ran(CommandKind::SessionOpen),
        2,
        "the restarted server verifies the retained native conversation through resume"
    );
    assert_eq!(world.segment_of(thread).await, segment);
    let verified: serde_json::Value = world.scalar(
        "SELECT (SELECT payload->'data'->'outcome' FROM items WHERE thread_id=$1 AND payload->>'marker'='session_reopened' ORDER BY seq_created DESC LIMIT 1)", thread).await;
    assert_eq!(verified["kind"], "verified_resume");
    assert_eq!(verified["coverage"]["recorded_turns"], "not_applicable");
    assert!(
        verified["carried_method"].is_null(),
        "verified native reuse is not a reconstruction claim"
    );
    assert_eq!(
        world
            .scalar::<String>(
                "SELECT native_session_ref FROM segments WHERE id=$1",
                segment
            )
            .await,
        native
    );
    world.answer(thread, &second, "second-reply", "Two.").await;
    world
        .say(thread, turn_ended(second.turn_id, TurnOutcome::Completed))
        .await;
    world.wait_state(thread, "idle").await;
    world
        .engine()
        .submit_input(&ada, thread, queued("Third"))
        .await
        .unwrap();
    world.turn_start(thread, 2).await;
    assert_eq!(
        world.ran(CommandKind::SessionOpen),
        2,
        "ordinary turns keep the verified session"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_successfully_carried_conversation_records_reconstruction_instead_of_native_resume() {
    use orch_link_contract::commands::{
        CommandError, CommandResult, ErrorCode, FsMaterializeResult,
    };
    let world = World::new().await;
    let directory = tempfile::tempdir().unwrap();
    let store = std::sync::Arc::new(
        orch_blob_local::LocalBlobStore::open(directory.path())
            .await
            .unwrap(),
    );
    world.engine().attach_blobs(store.clone());
    let thread = world.thread("Recorded reconstruction outcome").await;
    let caller = Caller::session(world.ada);
    world
        .engine()
        .submit_input(&caller, thread, queued("Remember the first message"))
        .await
        .unwrap();
    world.run_turn(thread, 0, "The recorded first answer").await;
    world.wait_state(thread, "idle").await;
    world.restart_api().await;
    world.engine().attach_blobs(store);
    world.machine.handler.on(CommandKind::FsMaterialize, |_| {
        Ok(CommandResult::FsMaterialize(FsMaterializeResult {
            files: 1,
            bytes: 1,
            skipped: 0,
        }))
    });
    world.refuse(
        CommandKind::SessionOpen,
        vec![CommandError::new(
            ErrorCode::SessionNotFound,
            "the native conversation is absent",
        )],
    );
    world
        .engine()
        .submit_input(&caller, thread, queued("Continue from that conversation"))
        .await
        .unwrap();
    let start = world.turn_start(thread, 1).await;
    assert!(
        start.text.contains("Remember the first message")
            && start.text.contains("The recorded first answer")
    );
    common::eventually("the actual history delivery has a typed outcome", || async {
        world.scalar::<bool>("SELECT EXISTS(SELECT 1 FROM items WHERE thread_id=$1 AND payload->>'marker'='agent_history' AND payload->'data'->'outcome'->>'kind'='reconstructed')", thread).await
    }).await;
    let outcome: serde_json::Value = world.scalar("SELECT payload->'data'->'outcome' FROM items WHERE thread_id=$1 AND payload->>'marker'='agent_history' ORDER BY seq_created DESC LIMIT 1", thread).await;
    assert!(outcome["carried_method"].as_str().is_some());
    assert!(outcome["at"].as_str().is_some());
    assert_ne!(outcome["kind"], "verified_resume");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_fresh_conversation_returned_for_resume_receives_the_recorded_history_before_the_input() {
    use orch_link_contract::commands::{
        CommandResult, FsMaterializeResult, SessionMode, SessionOpenResult,
    };
    use std::sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    };
    let world = World::new().await;
    let directory = tempfile::tempdir().unwrap();
    let blobs = Arc::new(
        orch_blob_local::LocalBlobStore::open(directory.path())
            .await
            .unwrap(),
    );
    world.engine().attach_blobs(blobs.clone());
    let thread = world.thread("A fresh answer to resume").await;
    let caller = Caller::session(world.ada);
    world
        .engine()
        .submit_input(&caller, thread, queued("Retain the earlier authored words"))
        .await
        .unwrap();
    world
        .run_turn(thread, 0, "Retain the recorded answer too")
        .await;
    world.wait_state(thread, "idle").await;
    world.restart_api().await;
    world.engine().attach_blobs(blobs);
    world.machine.handler.on(CommandKind::FsMaterialize, |_| {
        Ok(CommandResult::FsMaterialize(FsMaterializeResult {
            files: 1,
            bytes: 1,
            skipped: 0,
        }))
    });
    let replaced = Arc::new(AtomicBool::new(false));
    world
        .machine
        .handler
        .on(CommandKind::SessionOpen, move |command| {
            let Command::SessionOpen(open) = command else {
                unreachable!("the handler receives its declared kind")
            };
            let replacement =
                open.mode == SessionMode::Resume && !replaced.swap(true, Ordering::SeqCst);
            Ok(CommandResult::SessionOpen(SessionOpenResult {
                mcp: Some(open.mcp.iter().map(|server| server.name.clone()).collect()),
                native_session_ref: if replacement {
                    "a-fresh-conversation".to_owned()
                } else {
                    open.native_session_ref
                        .clone()
                        .unwrap_or_else(|| format!("native-{}", open.segment_id))
                },
                resumed: !replacement && open.mode == SessionMode::Resume,
                import_report: None,
                instruction_sources: Vec::new(),
                approval_reviewer: None,
            }))
        });
    world
        .engine()
        .submit_input(&caller, thread, queued("Continue with those words"))
        .await
        .unwrap();
    let start = world.turn_start(thread, 1).await;
    assert!(
        start.text.contains("Retain the earlier authored words")
            && start.text.contains("Retain the recorded answer too"),
        "{}",
        start.text
    );
    common::eventually("a fresh native conversation is recorded as reconstructed", || async {
        world.scalar::<bool>("SELECT EXISTS(SELECT 1 FROM items WHERE thread_id=$1 AND payload->>'marker'='agent_history' AND payload->'data'->'outcome'->>'kind'='reconstructed')",thread).await
    }).await;
    assert!(!world.scalar::<bool>("SELECT EXISTS(SELECT 1 FROM items WHERE thread_id=$1 AND payload->>'marker'='session_reopened' AND payload->'data'->'outcome'->>'kind'='verified_resume')",thread).await);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_session_that_is_opened_again_for_a_change_says_why_on_the_thread() {
    let world = World::new().await;
    let thread = world.thread("Reopened session").await;
    let ada = Caller::session(world.ada);
    world
        .engine()
        .submit_input(&ada, thread, queued("First"))
        .await
        .unwrap();
    world.run_turn(thread, 0, "One.").await;
    world.wait_state(thread, "idle").await;
    // A change that only a new session takes.
    world
        .set(
            world.topic,
            "agents.subagents_at_once",
            serde_json::json!(3),
        )
        .await;
    world
        .engine()
        .submit_input(&ada, thread, queued("Second"))
        .await
        .unwrap();
    world.turn_start(thread, 1).await;
    assert_eq!(world.ran(CommandKind::SessionOpen), 2);
    let marked: i64 = world
        .count(
            "SELECT count(*) FROM items WHERE thread_id = $1 AND kind = 'marker' AND payload->>'marker' = 'session_reopened'",
            thread,
        )
        .await;
    assert_eq!(
        marked, 1,
        "the thread says that the session was opened again"
    );
    let reason: String = world
        .scalar(
            "SELECT payload->'data'->>'reason' FROM items WHERE thread_id = $1 AND kind = 'marker' AND payload->>'marker' = 'session_reopened'",
            thread,
        )
        .await;
    assert_eq!(reason, "limits");
}
