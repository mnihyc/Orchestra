//! What goes wrong on the way to a turn is told in the thread, kept apart from the inputs it concerns, and tried again only where trying again can help.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]

mod common;

use common::{World, meanwhile, queued, said, turn_ended};
use orch_authz::Caller;
use orch_engine::commands::{
    Commands, CommandsError, Info, Outcome, Outgoing, RetainedCommand, Stage,
};
use orch_link_contract::commands::{CommandError, CommandKind, ErrorCode};
use orch_link_contract::events::TurnOutcome;
use orch_types::{Clock, Id};
use std::sync::{
    Arc,
    atomic::{AtomicBool, AtomicUsize, Ordering},
};
use std::time::Duration;

/// Models a dispatcher that sees a durable queued row before it has adopted it.
struct AdmissionGap {
    inner: Arc<dyn Commands>,
    db: orch_db::Db,
    clock: Arc<dyn orch_types::Clock>,
    reject: Arc<AtomicBool>,
    attempts: Arc<AtomicUsize>,
}

#[async_trait::async_trait]
impl Commands for AdmissionGap {
    async fn issue(&self, command: Outgoing) -> Result<(), CommandsError> {
        if matches!(
            command.kind,
            CommandKind::TurnStart | CommandKind::TurnStartFenced
        ) && self.reject.load(Ordering::SeqCst)
        {
            self.attempts.fetch_add(1, Ordering::SeqCst);
            return Err(CommandsError::Stopping);
        }
        self.inner.issue(command).await
    }

    async fn status(&self, id: Id) -> Result<Option<Info>, CommandsError> {
        if let Some(info) = self.inner.status(id).await? {
            return Ok(Some(info));
        }
        let mut conn = self
            .db
            .acquire()
            .await
            .map_err(|error| CommandsError::Other(error.to_string()))?;
        let row = orch_db::queries::link::command(&mut conn, id)
            .await
            .map_err(|error| CommandsError::Other(error.to_string()))?;
        Ok(row
            .filter(|row| {
                row.state == "queued"
                    && matches!(row.kind.as_str(), "turn.start" | "turn.start.fenced")
            })
            .map(|row| Info {
                stage: Stage::Queued,
                issued_at: row.created_at,
                outcome: None,
            }))
    }

    async fn wait(&self, id: Id) -> Result<Outcome, CommandsError> {
        self.inner.wait(id).await
    }
    async fn cancel(&self, id: Id) -> Result<bool, CommandsError> {
        if self.inner.cancel(id).await? {
            return Ok(true);
        }
        let mut conn = self
            .db
            .acquire()
            .await
            .map_err(|error| CommandsError::Other(error.to_string()))?;
        orch_db::queries::link::finish_command(
            &mut conn,
            id,
            &orch_db::queries::link::Ending::Cancelled,
            self.clock.now(),
        )
        .await
        .map_err(|error| CommandsError::Other(error.to_string()))
    }
    async fn observe(&self, command: Outgoing) -> Result<Outcome, CommandsError> {
        self.inner.observe(command).await
    }
    async fn retained_body(&self, id: Id) -> Result<Option<serde_json::Value>, CommandsError> {
        self.inner.retained_body(id).await
    }
    async fn retained_command(&self, id: Id) -> Result<Option<RetainedCommand>, CommandsError> {
        self.inner.retained_command(id).await
    }
    async fn stopped(&self, id: Id) -> Result<bool, CommandsError> {
        self.inner.stopped(id).await
    }
    fn hears_behind(&self, machine: Id, stream: &orch_link_contract::streams::Stream) -> bool {
        self.inner.hears_behind(machine, stream)
    }
}

async fn turns_of(world: &World, thread: Id) -> Vec<(i32, String)> {
    sqlx::query_as("SELECT ordinal, state FROM turns WHERE thread_id = $1 ORDER BY ordinal")
        .bind(thread)
        .fetch_all(world.pool.pool())
        .await
        .unwrap()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_unexpected_session_exit_interrupts_the_running_turn_and_retains_its_reason_after_restart()
 {
    let world = World::new().await;
    let thread = world.thread("Server exit").await;
    let input = world
        .engine()
        .submit_input(&Caller::session(world.ada), thread, queued("work"))
        .await
        .unwrap();
    let turn = world.turn_start(thread, 0).await;
    world.say(thread, common::turn_began(turn.turn_id)).await;
    world.wait_state(thread, "running").await;
    let reason = "app-server exited with code 7: updater shutdown";
    world
        .say(
            thread,
            orch_link_contract::events::Event::SessionLost(
                orch_link_contract::events::SessionLost {
                    reason: reason.into(),
                },
            ),
        )
        .await;
    world.wait_state(thread, "idle").await;
    assert_eq!(
        turns_of(&world, thread).await,
        [(1, "interrupted".to_owned())]
    );
    assert_eq!(world.input_state(input.input.id).await.0, "delivered");
    let message: String = world
        .scalar(
            "SELECT payload->>'message' FROM items WHERE thread_id=$1 AND kind='error'",
            thread,
        )
        .await;
    assert!(message.contains(reason));
    let session_reason: Option<String> = sqlx::query_scalar("SELECT last_error_detail FROM harnesses WHERE provider_id=$1 AND agent_id=$2 AND machine_id=$3")
        .bind(world.provider).bind(world.agent).bind(world.machine_id).fetch_one(world.pool.pool()).await.unwrap();
    assert_eq!(
        session_reason.as_deref(),
        Some(reason),
        "a loss report is sufficient even without a whole-profile error"
    );
    for (status, detail) in [("error", Some(reason.to_owned())), ("online", None)] {
        world
            .say_of_harness_taken_in(orch_link_contract::events::Event::HarnessStatus(
                orch_link_contract::events::HarnessStatus {
                    status: status.into(),
                    detail,
                    harness_version: Some("1.0.0".into()),
                    sessions_open: 0,
                },
            ))
            .await;
    }
    let retained: (String,Option<String>,Option<chrono::DateTime<chrono::Utc>>) = sqlx::query_as("SELECT status,last_error_detail,last_error_at FROM harnesses WHERE provider_id=$1 AND agent_id=$2 AND machine_id=$3")
        .bind(world.provider).bind(world.agent).bind(world.machine_id).fetch_one(world.pool.pool()).await.unwrap();
    assert_eq!(retained.0, "online");
    assert_eq!(retained.1.as_deref(), Some(reason));
    assert!(retained.2.is_some());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_session_lost_before_the_turn_begins_keeps_its_input_for_retry() {
    let world = World::new().await;
    let thread = world.thread("Session lost at start").await;
    let caller = Caller::session(world.ada);
    let first = world
        .engine()
        .submit_input(&caller, thread, queued("First"))
        .await
        .unwrap();
    let lost = world.turn_start(thread, 0).await;

    // The command was taken but no turn-started event was heard. The session
    // may have seen the input, so no automatic repeat is safe.
    world
        .say(
            thread,
            orch_link_contract::events::Event::SessionLost(
                orch_link_contract::events::SessionLost {
                    reason: "the session exited".into(),
                },
            ),
        )
        .await;
    world.wait_turn(lost.turn_id, "failed").await;
    world.wait_blocked(thread, "error").await;
    assert_eq!(world.input_state(first.input.id).await.0, "pending");
    meanwhile(Duration::from_millis(300)).await;
    assert_eq!(world.turn_starts(thread).len(), 1);

    // A subsequent send retries the original input before the new one.
    let second = world
        .engine()
        .submit_input(&caller, thread, queued("Second"))
        .await
        .unwrap();
    let retried = world.turn_start(thread, 1).await;
    assert_eq!(common::words_after_where(&retried.text), "First");
    world
        .say(thread, turn_ended(retried.turn_id, TurnOutcome::Completed))
        .await;
    let next = world.turn_start(thread, 2).await;
    assert_eq!(common::words_after_where(&next.text), "Second");
    world
        .say(thread, turn_ended(next.turn_id, TurnOutcome::Completed))
        .await;
    world.wait_state(thread, "idle").await;
    assert_eq!(world.input_state(first.input.id).await.0, "delivered");
    assert_eq!(world.input_state(second.input.id).await.0, "delivered");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_restart_waits_for_the_admitted_start_even_if_instructions_change() {
    let world = World::new().await;
    let thread = world.thread("Start retained across restart").await;
    world.machine.handler.hold(CommandKind::TurnStart);
    let input = world
        .engine()
        .submit_input(
            &Caller::session(world.ada),
            thread,
            queued("Original words"),
        )
        .await
        .unwrap();
    world.wait_began(CommandKind::TurnStart, 1).await;
    let sent = world.commands.issued_bodies("turn.start")[0].1.clone();
    world
        .set(
            thread,
            "agent.instructions",
            serde_json::json!("Instructions changed during startup"),
        )
        .await;
    world.restart_api().await;
    world.machine.handler.release();
    let start = world.turn_start(thread, 0).await;
    assert_eq!(start.text, sent["text"].as_str().unwrap());
    assert_eq!(world.commands.issued_of("turn.start"), 1);
    world.say(thread, common::turn_began(start.turn_id)).await;
    world
        .say(thread, turn_ended(start.turn_id, TurnOutcome::Completed))
        .await;
    world.wait_input(input.input.id, "delivered").await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_admitted_but_unissued_start_is_adopted_after_restart_with_its_original_body() {
    let world = World::new().await;
    let reject = Arc::new(AtomicBool::new(true));
    let attempts = Arc::new(AtomicUsize::new(0));
    let db = world.pool.clone();
    world
        .restart_api_using(|inner| {
            Arc::new(AdmissionGap {
                inner,
                db: db.clone(),
                clock: world.clock.clone(),
                reject: reject.clone(),
                attempts: attempts.clone(),
            })
        })
        .await;
    let thread = world.thread("Admitted before restart").await;
    let submitted = world
        .engine()
        .submit_input(&Caller::session(world.ada), thread, queued("Original work"))
        .await
        .unwrap();
    common::eventually("the start was admitted but not issued", || async {
        attempts.load(Ordering::SeqCst) > 0
    })
    .await;
    world.engine().stop();
    let (id, body, deadline): (Id, serde_json::Value, chrono::DateTime<chrono::Utc>) = sqlx::query_as(
        "SELECT id,body,deadline_at FROM commands WHERE kind='turn.start' AND body->>'thread_id'=$1::text AND state='queued'",
    )
    .bind(thread)
    .fetch_one(world.pool.pool())
    .await
    .unwrap();
    assert_eq!(world.commands.issued_of("turn.start"), 0);
    world
        .set(
            thread,
            "agent.instructions",
            serde_json::json!("Changed after admission"),
        )
        .await;
    world
        .restart_api_using(|inner| {
            Arc::new(AdmissionGap {
                inner,
                db: db.clone(),
                clock: world.clock.clone(),
                reject: reject.clone(),
                attempts: attempts.clone(),
            })
        })
        .await;
    reject.store(false, Ordering::SeqCst);
    world.engine().wake(thread);
    let start = world.turn_start(thread, 0).await;
    let issued = world.commands.issued_commands("turn.start");
    assert_eq!(issued.len(), 1);
    assert_eq!(issued[0].1.id, id);
    assert_eq!(issued[0].1.body, body);
    assert_eq!(issued[0].1.deadline.get(), deadline);
    assert_eq!(world.began(CommandKind::TurnStart), 1);
    world.say(thread, common::turn_began(start.turn_id)).await;
    world
        .say(thread, turn_ended(start.turn_id, TurnOutcome::Completed))
        .await;
    world.wait_input(submitted.input.id, "delivered").await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_admitted_but_unissued_start_is_adopted_without_reconnecting_the_machine() {
    let world = World::new().await;
    let reject = Arc::new(AtomicBool::new(true));
    let attempts = Arc::new(AtomicUsize::new(0));
    let db = world.pool.clone();
    world
        .restart_api_using(|inner| {
            Arc::new(AdmissionGap {
                inner,
                db: db.clone(),
                clock: world.clock.clone(),
                reject: reject.clone(),
                attempts: attempts.clone(),
            })
        })
        .await;
    let thread = world.thread("Admitted while linked").await;
    let submitted = world
        .engine()
        .submit_input(&Caller::session(world.ada), thread, queued("Original work"))
        .await
        .unwrap();
    common::eventually("the start was admitted but not issued", || async {
        attempts.load(Ordering::SeqCst) > 0
    })
    .await;
    let (id, body): (Id, serde_json::Value) = sqlx::query_as(
        "SELECT id,body FROM commands WHERE kind='turn.start' AND body->>'thread_id'=$1::text AND state='queued'",
    )
    .bind(thread)
    .fetch_one(world.pool.pool())
    .await
    .unwrap();
    reject.store(false, Ordering::SeqCst);
    world.engine().wake(thread);
    let start = world.turn_start(thread, 0).await;
    let issued = world.commands.issued_commands("turn.start");
    assert_eq!(issued.len(), 1);
    assert_eq!(issued[0].1.id, id);
    assert_eq!(issued[0].1.body, body);
    assert_eq!(world.began(CommandKind::TurnStart), 1);
    world.say(thread, common::turn_began(start.turn_id)).await;
    world
        .say(thread, turn_ended(start.turn_id, TurnOutcome::Completed))
        .await;
    world.wait_input(submitted.input.id, "delivered").await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_unissued_start_past_its_engine_deadline_is_cancelled_without_adoption() {
    let clock = Arc::new(orch_testkit::clock::manual());
    let world = World::with_clock(clock.clone()).await;
    world.set_config(|waits| waits.start_timeout = Duration::from_secs(2));
    let reject = Arc::new(AtomicBool::new(true));
    let attempts = Arc::new(AtomicUsize::new(0));
    let db = world.pool.clone();
    world
        .restart_api_using(|inner| {
            Arc::new(AdmissionGap {
                inner,
                db: db.clone(),
                clock: world.clock.clone(),
                reject: reject.clone(),
                attempts: attempts.clone(),
            })
        })
        .await;
    let thread = world.thread("Expired admission").await;
    let submitted = world
        .engine()
        .submit_input(&Caller::session(world.ada), thread, queued("Original work"))
        .await
        .unwrap();
    common::eventually("the start was admitted but not issued", || async {
        attempts.load(Ordering::SeqCst) > 0
    })
    .await;
    world.engine().stop();
    let turn: Id = world
        .scalar(
            "SELECT current_turn_id FROM threads WHERE node_id=$1",
            thread,
        )
        .await;
    let command: Id = sqlx::query_scalar(
        "SELECT id FROM commands WHERE kind='turn.start' AND body->>'turn_id'=$1::text",
    )
    .bind(turn)
    .fetch_one(world.pool.pool())
    .await
    .unwrap();
    // The link's retained command deadline is still ahead, while the engine's
    // shorter test start timeout has elapsed.
    clock.advance(chrono::Duration::seconds(3));
    let retained_deadline_is_ahead: bool =
        sqlx::query_scalar("SELECT deadline_at > $2 FROM commands WHERE id=$1")
            .bind(command)
            .bind(clock.now())
            .fetch_one(world.pool.pool())
            .await
            .unwrap();
    assert!(retained_deadline_is_ahead);
    world
        .restart_api_using(|inner| {
            Arc::new(AdmissionGap {
                inner,
                db: db.clone(),
                clock: world.clock.clone(),
                reject: reject.clone(),
                attempts: attempts.clone(),
            })
        })
        .await;
    reject.store(false, Ordering::SeqCst);
    world.engine().wake(thread);
    world.wait_turn(turn, "failed").await;
    world.wait_input(submitted.input.id, "pending").await;
    let error: Option<String> = sqlx::query_scalar("SELECT error->>'code' FROM turns WHERE id=$1")
        .bind(turn)
        .fetch_one(world.pool.pool())
        .await
        .unwrap();
    let state: String = sqlx::query_scalar("SELECT state FROM commands WHERE id=$1")
        .bind(command)
        .fetch_one(world.pool.pool())
        .await
        .unwrap();
    assert_eq!(error.as_deref(), Some("start_timeout"));
    assert_eq!(state, "cancelled");
    assert_eq!(world.commands.issued_of("turn.start"), 0);
    assert_eq!(world.began(CommandKind::TurnStart), 0);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn stopping_an_admitted_but_unissued_start_cancels_it_before_recovery() {
    let world = World::new().await;
    let reject = Arc::new(AtomicBool::new(true));
    let attempts = Arc::new(AtomicUsize::new(0));
    let db = world.pool.clone();
    world
        .restart_api_using(|inner| {
            Arc::new(AdmissionGap {
                inner,
                db: db.clone(),
                clock: world.clock.clone(),
                reject: reject.clone(),
                attempts: attempts.clone(),
            })
        })
        .await;
    let thread = world.thread("Stop before start issue").await;
    let caller = Caller::session(world.ada);
    let submitted = world
        .engine()
        .submit_input(&caller, thread, queued("Original work"))
        .await
        .unwrap();
    common::eventually("the start was admitted but not issued", || async {
        attempts.load(Ordering::SeqCst) > 0
    })
    .await;
    let old: Id = world
        .scalar(
            "SELECT current_turn_id FROM threads WHERE node_id=$1",
            thread,
        )
        .await;
    let old_command: Id = sqlx::query_scalar(
        "SELECT id FROM commands WHERE kind='turn.start' AND body->>'turn_id'=$1::text",
    )
    .bind(old)
    .fetch_one(world.pool.pool())
    .await
    .unwrap();
    world
        .engine()
        .interrupt(&caller, thread, old)
        .await
        .unwrap();
    world.wait_turn(old, "interrupted").await;
    world.wait_input(submitted.input.id, "pending").await;
    let state: String = sqlx::query_scalar("SELECT state FROM commands WHERE id=$1")
        .bind(old_command)
        .fetch_one(world.pool.pool())
        .await
        .unwrap();
    assert_eq!(state, "cancelled");
    reject.store(false, Ordering::SeqCst);
    world
        .restart_api_using(|inner| {
            Arc::new(AdmissionGap {
                inner,
                db: db.clone(),
                clock: world.clock.clone(),
                reject: reject.clone(),
                attempts: attempts.clone(),
            })
        })
        .await;
    meanwhile(Duration::from_millis(300)).await;
    assert_eq!(world.commands.issued_of("turn.start"), 0);
    let queue = world.sync(thread).await.thread.queue;
    assert!(queue.held);
    world
        .engine()
        .resume_queue(&caller, thread, queue.revision)
        .await
        .unwrap();
    let next = world.turn_start(thread, 0).await;
    assert_ne!(next.turn_id, old);
    assert_ne!(
        world.commands.issued_commands("turn.start")[0].1.id,
        old_command
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_idle_session_exit_still_leaves_its_reason_in_the_conversation() {
    let world = World::new().await;
    let thread = world.thread("Idle exit").await;
    world
        .engine()
        .submit_input(&Caller::session(world.ada), thread, queued("work"))
        .await
        .unwrap();
    let turn = world.turn_start(thread, 0).await;
    world.say(thread, common::turn_began(turn.turn_id)).await;
    world
        .say(thread, turn_ended(turn.turn_id, TurnOutcome::Completed))
        .await;
    world.wait_state(thread, "idle").await;
    world
        .say(
            thread,
            orch_link_contract::events::Event::SessionLost(
                orch_link_contract::events::SessionLost {
                    reason: "idle server exited unexpectedly".into(),
                },
            ),
        )
        .await;
    common::eventually("the idle failure is visible", || async {
        world.count("SELECT count(*) FROM items WHERE thread_id=$1 AND kind='error' AND payload->>'message' LIKE '%idle server exited unexpectedly%'",thread).await == 1
    }).await;
    assert_eq!(
        turns_of(&world, thread).await,
        [(1, "completed".to_owned())]
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_start_the_machine_refuses_for_good_fails_the_turn_and_its_input_and_blocks_the_thread() {
    let world = World::new().await;
    let thread = world.thread("Refused").await;
    world.refuse(
        CommandKind::TurnStart,
        vec![CommandError::new(
            ErrorCode::PolicyAutoApprovalNotAllowed,
            "this machine does not run unattended",
        )],
    );
    let submitted = world
        .engine()
        .submit_input(
            &Caller::session(world.ada),
            thread,
            queued("Run unattended"),
        )
        .await
        .unwrap();

    world.wait_told_blocked(thread, "binding_invalid").await;
    assert_eq!(
        world.input_state(submitted.input.id).await,
        ("failed".to_owned(), Some("start_failed".to_owned())),
        "a start that cannot succeed by being tried again is not tried again"
    );
    assert_eq!(turns_of(&world, thread).await, [(1, "failed".to_owned())]);
    let errors = world
        .count(
            "SELECT count(*) FROM items WHERE thread_id = $1 AND kind = 'error'",
            thread,
        )
        .await;
    assert_eq!(errors, 1, "the thread says what happened");
    let said_in_the_timeline: String = world
        .scalar(
            "SELECT payload->>'message' FROM items WHERE thread_id = $1 AND kind = 'error'",
            thread,
        )
        .await;
    assert!(
        said_in_the_timeline.contains("unattended"),
        "{said_in_the_timeline}"
    );
    meanwhile(Duration::from_millis(400)).await;
    assert_eq!(world.began(CommandKind::TurnStart), 1);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_start_that_may_pass_gives_the_input_one_more_try_and_then_gives_it_up() {
    let world = World::new().await;
    let thread = world.thread("Flaky").await;
    let ada = Caller::session(world.ada);
    let slow = || CommandError::new(ErrorCode::Timeout, "the harness was slow").retryable();
    world.refuse(CommandKind::TurnStart, vec![slow(), slow()]);

    let first = world
        .engine()
        .submit_input(&ada, thread, queued("First"))
        .await
        .unwrap();
    world.wait_blocked(thread, "error").await;
    assert_eq!(
        world.input_state(first.input.id).await.0,
        "pending",
        "it goes back to waiting, once"
    );
    assert_eq!(turns_of(&world, thread).await, [(1, "failed".to_owned())]);
    meanwhile(Duration::from_millis(500)).await;
    assert_eq!(
        world.began(CommandKind::TurnStart),
        1,
        "a thread blocked by an error waits for a person"
    );

    // A person sends another input: the first still goes first, and fails again.
    let second = world
        .engine()
        .submit_input(&ada, thread, queued("Second"))
        .await
        .unwrap();
    world.wait_input(first.input.id, "failed").await;
    assert_eq!(
        world.input_state(first.input.id).await.1.as_deref(),
        Some("start_failed")
    );
    assert_eq!(world.input_state(second.input.id).await.0, "pending");
    assert_eq!(world.began(CommandKind::TurnStart), 2);
    world.wait_blocked(thread, "error").await;

    // And the next person's input is served, since the machine answers now.
    let third = world
        .engine()
        .submit_input(&ada, thread, queued("Third"))
        .await
        .unwrap();
    let start = world.turn_start(thread, 2).await;
    assert_eq!(
        common::words_after_where(&start.text),
        "Second",
        "what waited longest goes first"
    );
    world.answer(thread, &start, "m3", "Done.").await;
    world
        .say(thread, turn_ended(start.turn_id, TurnOutcome::Completed))
        .await;
    let last = world.turn_start(thread, 3).await;
    assert_eq!(common::words_after_where(&last.text), "Third");
    world
        .say(thread, turn_ended(last.turn_id, TurnOutcome::Completed))
        .await;
    world.wait_state(thread, "idle").await;
    assert_eq!(world.input_state(third.input.id).await.0, "delivered");
    let states: Vec<String> = turns_of(&world, thread)
        .await
        .into_iter()
        .map(|(_, state)| state)
        .collect();
    assert_eq!(states, ["failed", "failed", "completed", "completed"]);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_turn_start_not_taken_up_in_time_fails_with_a_timeout_and_the_harness_is_asked_to_stop() {
    let world = World::new().await;
    let thread = world.thread("Too slow").await;
    world.machine.handler.hold(CommandKind::TurnStart);
    let submitted = world
        .engine()
        .submit_input(
            &Caller::session(world.ada),
            thread,
            queued("Take your time"),
        )
        .await
        .unwrap();

    world.wait_blocked(thread, "error").await;
    let code: Option<String> =
        sqlx::query_scalar("SELECT error->>'code' FROM turns WHERE thread_id = $1")
            .bind(thread)
            .fetch_one(world.pool.pool())
            .await
            .unwrap();
    assert_eq!(code.as_deref(), Some("start_timeout"));
    assert_eq!(
        world.input_state(submitted.input.id).await.0,
        "pending",
        "a timeout may pass: the input waits"
    );
    world.wait_began_or_issued("turn.interrupt").await;
    world.machine.handler.release();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_conversation_the_harness_no_longer_holds_is_carried_into_a_new_segment() {
    let world = World::new().await;
    let directory = tempfile::tempdir().unwrap();
    let store = std::sync::Arc::new(
        orch_blob_local::LocalBlobStore::open(directory.path())
            .await
            .unwrap(),
    );
    world.engine().attach_blobs(store.clone());
    let thread = world.thread("Lost").await;
    let ada = Caller::session(world.ada);
    world
        .engine()
        .submit_input(&ada, thread, queued("One"))
        .await
        .unwrap();
    world.run_turn(thread, 0, "First.").await;
    world.wait_state(thread, "idle").await;
    let before = world.segment_of(thread).await;

    // The API restarts, and the harness has forgotten the conversation by the time it is asked.
    world.restart_api().await;
    world.engine().attach_blobs(store);
    world.machine.handler.on(CommandKind::FsMaterialize, |_| {
        Ok(orch_link_contract::commands::CommandResult::FsMaterialize(
            orch_link_contract::commands::FsMaterializeResult {
                files: 1,
                bytes: 1,
                skipped: 0,
            },
        ))
    });
    world.refuse(
        CommandKind::SessionOpen,
        vec![CommandError::new(
            ErrorCode::SessionNotFound,
            "the harness holds no such conversation",
        )],
    );
    world
        .engine()
        .submit_input(&ada, thread, queued("Two"))
        .await
        .unwrap();
    let start = world.turn_start(thread, 1).await;
    assert!(
        start.text.contains("One") && start.text.contains("First."),
        "the recorded conversation accompanies the pending input: {}",
        start.text
    );
    world.answer(thread, &start, "m2", "Second.").await;
    world
        .say(thread, turn_ended(start.turn_id, TurnOutcome::Completed))
        .await;
    world.wait_state(thread, "idle").await;

    let after = world.segment_of(thread).await;
    assert_ne!(
        before, after,
        "the conversation goes on in a segment of its own"
    );
    let ended: Option<String> = world
        .scalar("SELECT end_reason FROM segments WHERE id = $1", before)
        .await;
    assert_eq!(ended.as_deref(), Some("error"));
    assert_eq!(
        world
            .count("SELECT count(*) FROM segments WHERE thread_id = $1", thread)
            .await,
        2
    );
    let lost = world
        .count(
            "SELECT count(*) FROM items WHERE thread_id = $1 AND payload->>'marker'='agent_history'",
            thread,
        )
        .await;
    assert_eq!(
        lost, 1,
        "the timeline says how the conversation reached the new session"
    );
    let turn_segment: Id = world
        .scalar("SELECT segment_id FROM turns WHERE id = $1", start.turn_id)
        .await;
    assert_eq!(
        turn_segment, after,
        "the turn belongs to the segment it ran in"
    );
    assert!(said(&world.sync(thread).await).contains(&"agent: Second.".to_owned()));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_resource_side_can_ask_for_a_second_look_at_the_threads_what_it_changed_blocked() {
    let world = World::new().await;
    let thread = world.thread("Second look").await;
    let ada = Caller::session(world.ada);
    world.exec("UPDATE providers SET status = 'disabled'").await;
    world
        .engine()
        .submit_input(&ada, thread, queued("Provider first"))
        .await
        .unwrap();
    world.wait_blocked(thread, "provider_unavailable").await;

    world.exec("UPDATE providers SET status = 'ready'").await;
    assert_eq!(
        world
            .engine()
            .reevaluate_provider(world.provider)
            .await
            .unwrap(),
        1
    );
    let start = world.turn_start(thread, 0).await;
    world.answer(thread, &start, "m1", "Done.").await;
    world
        .say(thread, turn_ended(start.turn_id, TurnOutcome::Completed))
        .await;
    world.wait_state(thread, "idle").await;

    world
        .exec("UPDATE environments SET status = 'stopped'")
        .await;
    world
        .engine()
        .submit_input(&ada, thread, queued("Environment next"))
        .await
        .unwrap();
    world.wait_blocked(thread, "environment_unavailable").await;
    world
        .exec("UPDATE environments SET status = 'running'")
        .await;
    assert_eq!(
        world
            .engine()
            .reevaluate_environment(world.environment)
            .await
            .unwrap(),
        1
    );
    let second = world.turn_start(thread, 1).await;
    assert_eq!(common::words_after_where(&second.text), "Environment next");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_turn_whose_start_the_system_is_still_taking_in_is_not_given_up_at_its_due_time() {
    let world = World::tuned(|waits| {
        waits.start_timeout = Duration::from_secs(2);
        waits.started_wait = Duration::from_secs(1);
    })
    .await;
    let thread = world.thread("Taken, heard late").await;
    let ada = Caller::session(world.ada);
    world
        .engine()
        .submit_input(&ada, thread, queued("First"))
        .await
        .unwrap();

    // The machine answered the command, and what its harness said after it has reached the API and is not
    // taken in yet (C-turns: under load, a minute and more): the turn is not given up at its due time.
    world.hearing_behind(true);
    let taken = world.turn_start(thread, 0).await;
    meanwhile(Duration::from_secs(5)).await;
    assert_eq!(
        world.turn_state(taken.turn_id).await,
        "starting",
        "a start whose word is still being taken in waits for it"
    );

    // The word is taken in: the turn began.
    world.say(thread, common::turn_began(taken.turn_id)).await;
    world.wait_turn(taken.turn_id, "running").await;
    world.hearing_behind(false);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_turn_the_harness_took_and_never_began_is_given_up_and_the_next_start_runs_once() {
    let world = World::tuned(|waits| {
        waits.start_timeout = Duration::from_secs(2);
        waits.started_wait = Duration::from_secs(1);
    })
    .await;
    let thread = world.thread("Taken, never begun").await;
    let ada = Caller::session(world.ada);
    let first = world
        .engine()
        .submit_input(&ada, thread, queued("First"))
        .await
        .unwrap();

    // The machine answers the command, and the harness never says that the turn began.
    let taken = world.turn_start(thread, 0).await;
    world.wait_blocked(thread, "error").await;
    let code: Option<String> = sqlx::query_scalar("SELECT error->>'code' FROM turns WHERE id = $1")
        .bind(taken.turn_id)
        .fetch_one(world.pool.pool())
        .await
        .unwrap();
    assert_eq!(code.as_deref(), Some("start_timeout"));
    assert_eq!(world.turn_state(taken.turn_id).await, "failed");
    assert_eq!(
        world.input_state(first.input.id).await.0,
        "pending",
        "a turn that never began may be tried again: its input waits"
    );
    world.wait_began_or_issued("turn.interrupt").await;

    // A late word of the harness does not bring the turn back.
    world.say(thread, common::turn_began(taken.turn_id)).await;
    meanwhile(Duration::from_millis(300)).await;
    assert_eq!(world.turn_state(taken.turn_id).await, "failed");

    // The next input sets the thread going again: the input that waited runs, once, and then the new one.
    world
        .engine()
        .submit_input(&ada, thread, queued("Second"))
        .await
        .unwrap();
    let again = world.run_turn(thread, 1, "Done.").await;
    assert_eq!(common::words_after_where(&again.text), "First");
    let next = world.run_turn(thread, 2, "Done too.").await;
    assert_eq!(common::words_after_where(&next.text), "Second");
    world.wait_state(thread, "idle").await;
    meanwhile(Duration::from_millis(300)).await;
    assert_eq!(world.turn_starts(thread).len(), 3);
    assert_eq!(world.input_state(first.input.id).await.0, "delivered");
}
