//! A summary's source read leaves Send, Stop and the caller's account available.
use super::*;
use orch_api_contract::threads::{ThreadInterrupt, ThreadInterruptBody};
use orch_api_core::{dispatch::StoredCommand, testing::admit_stored_command};
use orch_engine::summaries::resource::command::observation;
use std::{
    sync::atomic::{AtomicUsize, Ordering},
    time::Duration,
};
use tokio::sync::Semaphore;

// This is a deadlock watchdog: the source read stays paused until the competing act answers.
const WATCHDOG: Duration = Duration::from_secs(5);

fn stored(command: orch_engine::commands::Outgoing) -> StoredCommand {
    StoredCommand {
        id: Some(command.id),
        machine: command.machine,
        kind: command.kind.as_str().into(),
        key: command.key.wire(),
        deadline: command.deadline,
        body: command.body,
        authority: command.authority,
    }
}

async fn pending(s: &Scene, thread: Id, provider: Id) -> StoredCommand {
    let pending = s
        .rig
        .commands
        .stop_next_issue(CommandKind::ProviderComplete);
    s.rig
        .world
        .app
        .call(SummaryRefresh, &[&"thread", &thread])
        .token(&s.ada.token)
        .body(&SummaryRefreshBody {
            client_id: new_id(),
            model: Some(SummaryModelChoice {
                provider_id: provider,
                model: MODEL.into(),
                params: None,
            }),
            range: None,
        })
        .send()
        .await
        .ok();
    s.rig.engine.summarize_resources().await.unwrap();
    stored(
        tokio::time::timeout(WATCHDOG, pending)
            .await
            .unwrap()
            .unwrap(),
    )
}

async fn send_and_stop_during_read(wire: bool) {
    let s = Scene::new().await;
    let provider = api_provider(&s).await;
    script(&s, false);
    let thread = s.thread().await.base.id;
    s.exchange(thread, 0, "Read the source", "The initial work is ready")
        .await;
    s.say(
        &s.ada,
        thread,
        "Keep checking",
        Delivery::Queue,
        "summary-running-turn",
    )
    .await
    .ok();
    let turn = s.rig.turn_start(thread, 1).await;
    s.rig.say(thread, turn_began(turn.turn_id)).await;
    s.rig.wait_turn(turn.turn_id, "running").await;
    s.rig.engine.stop();
    let command = pending(&s, thread, provider).await;
    let id = command.id.unwrap();
    let app = s.rig.world.app.app.clone();
    if wire {
        app.link
            .dispatcher()
            .issue_stored(command.clone())
            .await
            .unwrap();
    }
    let reached = Arc::new(Semaphore::new(0));
    let release = Arc::new(Semaphore::new(0));
    let reads = Arc::new(AtomicUsize::new(0));
    let (at, resume, seen) = (reached.clone(), release.clone(), reads.clone());
    let worker = tokio::spawn(observation::follow(
        move |request| {
            let (at, resume, seen) = (at.clone(), resume.clone(), seen.clone());
            Box::pin(async move {
                assert_eq!(request, id);
                if seen.fetch_add(1, Ordering::SeqCst) == 0 {
                    at.add_permits(1);
                    resume.acquire().await.unwrap().forget();
                }
            })
        },
        async move {
            if wire {
                admit_stored_command(&app, id).await.map(|_| ())
            } else {
                app.link
                    .dispatcher()
                    .issue_stored(command)
                    .await
                    .map(|_| ())
                    .map_err(|error| error.to_string())
            }
        },
    ));
    tokio::time::timeout(WATCHDOG, reached.acquire())
        .await
        .unwrap()
        .unwrap()
        .forget();
    let sent = tokio::time::timeout(
        WATCHDOG,
        s.say(
            &s.ada,
            thread,
            "A new message while the summary reads",
            Delivery::Queue,
            "during-summary-read",
        ),
    )
    .await;
    let stopped = tokio::time::timeout(
        WATCHDOG,
        s.rig
            .world
            .app
            .call(ThreadInterrupt, &[&thread])
            .token(&s.ada.token)
            .body(&ThreadInterruptBody {
                turn_id: turn.turn_id,
            })
            .send(),
    )
    .await;
    release.add_permits(1);
    let result = tokio::time::timeout(WATCHDOG, worker)
        .await
        .unwrap()
        .unwrap();
    sent.expect("Send answers while the source read remains paused")
        .ok();
    stopped
        .expect("Stop answers while the source read remains paused")
        .ok();
    assert!(
        result.is_err(),
        "words that changed during the read cannot authorize the old command"
    );
    assert!(
        reads.load(Ordering::SeqCst) >= 2,
        "the source is read again after it moves"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn send_and_stop_are_answered_while_a_summary_reads_their_thread_before_insertion() {
    send_and_stop_during_read(false).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn send_and_stop_are_answered_while_a_summary_reads_their_thread_before_transmission() {
    send_and_stop_during_read(true).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_summary_reads_the_source_again_when_its_rows_change_even_if_their_values_return() {
    for wire in [false, true] {
        let s = Scene::new().await;
        let provider = api_provider(&s).await;
        script(&s, false);
        let thread = s.thread().await.base.id;
        s.exchange(thread, 0, "Read the source", "The initial work is ready")
            .await;
        s.rig.engine.stop();
        let command = pending(&s, thread, provider).await;
        let id = command.id.unwrap();
        let app = s.rig.world.app.app.clone();
        if wire {
            app.link
                .dispatcher()
                .issue_stored(command.clone())
                .await
                .unwrap();
        }
        let pool = s.rig.db.pool().clone();
        let user = s.ada.id();
        let ancestor = s.tree.topic.base.id;
        let reads = Arc::new(AtomicUsize::new(0));
        let seen = reads.clone();
        let result = observation::follow(
            move |_| {
                let (pool, seen) = (pool.clone(), seen.clone());
                Box::pin(async move {
                    if seen.fetch_add(1, Ordering::SeqCst) == 0 {
                        let mut tx = pool.begin().await.unwrap();
                        // NOWAIT proves each row is free without measuring the speed of the runner.
                        for (query, row) in [
                            ("SELECT id FROM users WHERE id=$1 FOR UPDATE NOWAIT", user),
                            (
                                "SELECT id FROM nodes WHERE id=$1 FOR UPDATE NOWAIT",
                                ancestor,
                            ),
                            (
                                "SELECT id FROM providers WHERE id=$1 FOR UPDATE NOWAIT",
                                provider,
                            ),
                        ] {
                            sqlx::query(query)
                                .bind(row)
                                .execute(&mut *tx)
                                .await
                                .unwrap();
                        }
                        sqlx::query(
                            "SELECT node_id FROM threads WHERE node_id=$1 FOR UPDATE NOWAIT",
                        )
                        .bind(thread)
                        .execute(&mut *tx)
                        .await
                        .unwrap();
                        sqlx::query(
                            "UPDATE threads SET activity_seq=activity_seq WHERE node_id=$1",
                        )
                        .bind(thread)
                        .execute(&mut *tx)
                        .await
                        .unwrap();
                        tx.commit().await.unwrap();
                    }
                })
            },
            async {
                if wire {
                    admit_stored_command(&app, id).await.map(|_| ())
                } else {
                    app.link
                        .dispatcher()
                        .issue_stored(command)
                        .await
                        .map(|_| ())
                        .map_err(|error| error.to_string())
                }
            },
        )
        .await;
        assert!(
            result.is_ok(),
            "a fresh read of the unchanged words is admitted: {result:?}"
        );
        assert!(
            reads.load(Ordering::SeqCst) >= 2,
            "changing a row back does not hide the intervening write"
        );
    }
}
