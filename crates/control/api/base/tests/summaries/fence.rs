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
    pending_as(s, thread, provider, &s.ada.token).await
}

async fn pending_as(s: &Scene, thread: Id, provider: Id, token: &str) -> StoredCommand {
    let pending = s
        .rig
        .commands
        .stop_next_issue(CommandKind::ProviderComplete);
    s.rig
        .world
        .app
        .call(SummaryRefresh, &[&"thread", &thread])
        .token(token)
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
                turn_id: Some(turn.turn_id),
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

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_summary_reapplies_masking_when_an_environment_secret_changes_during_its_read() {
    use orch_api_contract::settings::{EnvironmentSecretSet, NodeSecretSetBody};
    // These values are synthetic source text and masking inputs, never a model credential.
    const BEFORE: &str = "synthetic-mask-before";
    const AFTER: &str = "synthetic-mask-after";
    for wire in [false, true] {
        let s = Scene::new().await;
        let provider = api_provider(&s).await;
        script(&s, false);
        let environment = s.on.environment;
        s.rig
            .world
            .app
            .call(EnvironmentSecretSet, &[&environment, &"SOURCE_MASK"])
            .token(&s.ada.token)
            .body(&NodeSecretSetBody {
                value: BEFORE.into(),
            })
            .send()
            .await
            .ok();
        let thread = s.thread().await.base.id;
        s.exchange(thread, 0, AFTER, "The initial work is ready")
            .await;
        s.rig.engine.stop();
        let command = pending(&s, thread, provider).await;
        assert!(command.body["input"].as_str().unwrap().contains(AFTER));
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
            move |_| {
                let (at, resume, seen) = (at.clone(), resume.clone(), seen.clone());
                Box::pin(async move {
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
        // The in-memory HTTP client stays on this test task; only the source observer is Send.
        let changed = tokio::time::timeout(
            WATCHDOG,
            s.rig
                .world
                .app
                .call(EnvironmentSecretSet, &[&environment, &"SOURCE_MASK"])
                .token(&s.ada.token)
                .body(&NodeSecretSetBody {
                    value: AFTER.into(),
                })
                .send(),
        )
        .await;
        release.add_permits(1);
        let result = tokio::time::timeout(WATCHDOG, worker)
            .await
            .unwrap()
            .unwrap();
        changed
            .expect("the masking input can change while source reading is paused")
            .ok();
        assert!(
            result.is_err(),
            "a prompt made before the new masking input cannot be published"
        );
        assert!(
            reads.load(Ordering::SeqCst) >= 2,
            "masking is reapplied in a fresh source read"
        );
    }
}

async fn summary_key(s: &Scene, thread: Id) -> (Id, String) {
    use orch_api_core::middleware::authentication::{API_TOKEN_PREFIX, new_token, token_hash};
    use orch_db::queries::identity::{self, NewApiToken};
    let scope: orch_access_contract::key_scope::KeyScope = serde_json::from_value(json!({
        "rules":[{"target":{"kind":"thread","id":thread,"reach":"self"},
            "resources":["thread"],"allow":[{"operation":"SummaryRefresh","actions":["read","write"]}]}]
    })).unwrap();
    assert!(scope.validate().is_empty());
    let bearer = new_token(API_TOKEN_PREFIX);
    let token = new_id();
    identity::create_api_token_scoped(
        &mut *s.rig.world.conn().await,
        &NewApiToken {
            id: token,
            user_id: s.ada.id(),
            name: "Thread summary key",
            token_hash: &token_hash(&bearer),
            scopes: &[],
            node_id: None,
            at: s.rig.world.now(),
            expires_at: None,
        },
        Some(&serde_json::to_value(scope).unwrap()),
    )
    .await
    .unwrap();
    (token, bearer)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_summary_asked_with_a_key_keeps_the_account_free_and_rechecks_the_key_after_reading() {
    use orch_db::queries::identity;
    for wire in [false, true] {
        for revoke in [false, true] {
            let s = Scene::new().await;
            let provider = api_provider(&s).await;
            script(&s, false);
            let thread = s.thread().await.base.id;
            s.exchange(thread, 0, "Read the source", "The initial work is ready")
                .await;
            s.rig.engine.stop();
            let (token, bearer) = summary_key(&s, thread).await;
            let command = pending_as(&s, thread, provider, &bearer).await;
            let id = command.id.unwrap();
            let app = s.rig.world.app.app.clone();
            if wire {
                app.link
                    .dispatcher()
                    .issue_stored(command.clone())
                    .await
                    .unwrap();
            }
            let (pool, user, now) = (s.rig.db.pool().clone(), s.ada.id(), s.rig.world.now());
            let result = observation::follow(
                move |_| {
                    let pool = pool.clone();
                    Box::pin(async move {
                        let mut tx = pool.begin().await.unwrap();
                        sqlx::query("SELECT id FROM users WHERE id=$1 FOR UPDATE NOWAIT")
                            .bind(user)
                            .execute(&mut *tx)
                            .await
                            .unwrap();
                        sqlx::query("SELECT id FROM api_tokens WHERE id=$1 FOR UPDATE NOWAIT")
                            .bind(token)
                            .execute(&mut *tx)
                            .await
                            .unwrap();
                        if revoke {
                            assert!(
                                identity::revoke_api_token(&mut tx, token, user, now)
                                    .await
                                    .unwrap()
                            );
                        }
                        tx.commit().await.unwrap();
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
            assert_eq!(
                result.is_err(),
                revoke,
                "the final locked check uses the key's current state: {result:?}"
            );
        }
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_summary_releases_its_write_fence_before_rereading_a_source_changed_during_a_lock_wait() {
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
        let mut held = s.rig.db.begin().await.unwrap();
        let blocker: i32 = sqlx::query_scalar("SELECT pg_backend_pid()")
            .fetch_one(&mut *held)
            .await
            .unwrap();
        sqlx::query("SELECT node_id FROM threads WHERE node_id=$1 FOR UPDATE")
            .bind(thread)
            .execute(&mut *held)
            .await
            .unwrap();
        let pool = s.rig.db.pool().clone();
        let user = s.ada.id();
        let ancestor = s.tree.topic.base.id;
        let reads = Arc::new(AtomicUsize::new(0));
        let seen = reads.clone();
        let worker = tokio::spawn(observation::follow(
            move |_| {
                let (pool, seen) = (pool.clone(), seen.clone());
                Box::pin(async move {
                    if seen.fetch_add(1, Ordering::SeqCst) > 0 {
                        let mut tx = pool.begin().await.unwrap();
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
                            (
                                "SELECT node_id FROM threads WHERE node_id=$1 FOR UPDATE NOWAIT",
                                thread,
                            ),
                        ] {
                            sqlx::query(query)
                                .bind(row)
                                .execute(&mut *tx)
                                .await
                                .unwrap();
                        }
                        tx.rollback().await.unwrap();
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
        // Observe the actual row wait, rather than assuming it began after an arbitrary sleep.
        eventually(
            "the summary reaches the held thread after its source read",
            || async {
                if reads.load(Ordering::SeqCst) == 0 {
                    return false;
                }
                sqlx::query_scalar::<_, bool>(
                "SELECT EXISTS(SELECT 1 FROM pg_stat_activity WHERE $1=ANY(pg_blocking_pids(pid)))"
            ).bind(blocker).fetch_one(s.rig.db.pool()).await.unwrap()
            },
        )
        .await;
        sqlx::query("UPDATE threads SET activity_seq=activity_seq WHERE node_id=$1")
            .bind(thread)
            .execute(&mut *held)
            .await
            .unwrap();
        held.commit().await.unwrap();
        let result = tokio::time::timeout(WATCHDOG, worker)
            .await
            .unwrap()
            .unwrap();
        assert!(
            result.is_ok(),
            "the new read of the same words is admitted: {result:?}"
        );
        assert!(reads.load(Ordering::SeqCst) >= 2);
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn two_writers_of_the_same_summary_receive_the_same_single_command() {
    let s = Scene::new().await;
    let provider = api_provider(&s).await;
    script(&s, false);
    let thread = s.thread().await.base.id;
    s.exchange(thread, 0, "Read the source", "The initial work is ready")
        .await;
    s.rig.engine.stop();
    let command = pending(&s, thread, provider).await;
    let id = command.id.unwrap();
    let reached = Arc::new(Semaphore::new(0));
    let release = Arc::new(Semaphore::new(0));
    let mut workers = Vec::new();
    // Two first insertions both read before either can take the source's short write fence.
    for _ in 0..2 {
        let (at, resume) = (reached.clone(), release.clone());
        let seen = Arc::new(AtomicUsize::new(0));
        let dispatcher = s.rig.world.app.app.link.dispatcher();
        let command = command.clone();
        workers.push(tokio::spawn(observation::follow(
            move |_| {
                let (at, resume, seen) = (at.clone(), resume.clone(), seen.clone());
                Box::pin(async move {
                    if seen.fetch_add(1, Ordering::SeqCst) == 0 {
                        at.add_permits(1);
                        resume.acquire().await.unwrap().forget();
                    }
                })
            },
            async move { dispatcher.issue_stored(command).await },
        )));
    }
    tokio::time::timeout(WATCHDOG, reached.acquire_many(2))
        .await
        .unwrap()
        .unwrap()
        .forget();
    release.add_permits(2);
    for worker in workers {
        let receipt = tokio::time::timeout(WATCHDOG, worker)
            .await
            .unwrap()
            .unwrap()
            .expect("a duplicate writer receives the existing summary command");
        assert_eq!(receipt.id, id);
    }
    let count: i64 = sqlx::query_scalar("SELECT count(*) FROM commands WHERE id=$1")
        .bind(id)
        .fetch_one(s.rig.db.pool())
        .await
        .unwrap();
    assert_eq!(count, 1);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_summary_keeps_the_shorter_key_lifetime_it_observed_when_its_command_was_inserted() {
    use orch_db::queries::link;
    use orch_engine::commands::CommandAuthority;
    use orch_types::Time;
    let s = Scene::new().await;
    let provider = api_provider(&s).await;
    script(&s, false);
    let thread = s.thread().await.base.id;
    s.exchange(thread, 0, "Read the source", "The initial work is ready")
        .await;
    s.rig.engine.stop();
    let (key, bearer) = summary_key(&s, thread).await;
    let command = pending_as(&s, thread, provider, &bearer).await;
    let id = command.id.unwrap();
    // A key shortened after preparation keeps that observed lifetime in the durable receipt,
    // even if its row later permits a longer life again.
    let until = s.rig.world.now() + chrono::Duration::minutes(5);
    sqlx::query("UPDATE api_tokens SET expires_at=$2 WHERE id=$1")
        .bind(key)
        .bind(until)
        .execute(s.rig.db.pool())
        .await
        .unwrap();
    s.rig
        .world
        .app
        .app
        .link
        .dispatcher()
        .issue_stored(command)
        .await
        .unwrap();
    sqlx::query("UPDATE api_tokens SET expires_at=NULL WHERE id=$1")
        .bind(key)
        .execute(s.rig.db.pool())
        .await
        .unwrap();
    let mut tx = s.rig.db.begin().await.unwrap();
    let authority: CommandAuthority =
        serde_json::from_value(link::command_authority(&mut tx, id).await.unwrap().unwrap())
            .unwrap();
    let delivered = authority
        .http_delivery(&mut tx, s.rig.world.app.app.clock.as_ref(), id)
        .await
        .unwrap();
    assert_eq!(delivered.until(), Some(Time::new(until)));
    tx.rollback().await.unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_unpaid_prompt_rebuild_wins_before_wire_admission_and_its_old_command_cannot_leave() {
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
    app.link.dispatcher().issue_stored(command).await.unwrap();
    let original = rows::request(&mut s.rig.db.pool().acquire().await.unwrap(), id)
        .await
        .unwrap()
        .unwrap();
    assert!(
        original.model_admitted_at.is_none(),
        "storing a command does not spend its model call"
    );
    let reached = Arc::new(Semaphore::new(0));
    let release = Arc::new(Semaphore::new(0));
    let seen = Arc::new(AtomicUsize::new(0));
    let (at, resume) = (reached.clone(), release.clone());
    let worker = tokio::spawn(observation::follow(
        move |_| {
            let (at, resume, seen) = (at.clone(), resume.clone(), seen.clone());
            Box::pin(async move {
                if seen.fetch_add(1, Ordering::SeqCst) == 0 {
                    at.add_permits(1);
                    resume.acquire().await.unwrap().forget();
                }
            })
        },
        async move { admit_stored_command(&app, id).await },
    ));
    tokio::time::timeout(WATCHDOG, reached.acquire())
        .await
        .unwrap()
        .unwrap()
        .forget();
    let mut tx = s.rig.db.begin().await.unwrap();
    let result = rows::rebuild(
        &mut tx,
        &original,
        original.lease_token.unwrap(),
        &original.accepted_source,
        3,
        s.rig.world.now(),
    )
    .await
    .unwrap();
    let rows::Rebuild::Ready {
        request: next,
        cancelled,
    } = result
    else {
        panic!("an unadmitted prompt can be rebuilt");
    };
    assert_eq!(next.id, original.id);
    assert_ne!(next.call_id, original.call_id);
    assert_eq!(next.read_attempt, 1);
    assert!(next.prepared.is_none());
    assert_eq!(cancelled, vec![id]);
    tx.commit().await.unwrap();
    release.add_permits(1);
    assert!(
        tokio::time::timeout(WATCHDOG, worker)
            .await
            .unwrap()
            .unwrap()
            .is_err()
    );
    s.rig
        .world
        .app
        .app
        .link
        .dispatcher()
        .cancel(id)
        .await
        .unwrap();
    let withdrawn = rows::withdrawn(&mut s.rig.db.pool().acquire().await.unwrap(), id)
        .await
        .unwrap();
    assert!(
        withdrawn
            .iter()
            .any(|call| call.id == id && call.input.is_some()),
        "native input settlement remains recoverable after the short rebuild commits"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_wire_admitted_summary_cannot_be_rebuilt_even_when_no_acknowledgement_was_received() {
    let s = Scene::new().await;
    let provider = api_provider(&s).await;
    script(&s, false);
    let thread = s.thread().await.base.id;
    s.exchange(thread, 0, "Read the source", "The initial work is ready")
        .await;
    s.rig.engine.stop();
    let command = pending(&s, thread, provider).await;
    let id = command.id.unwrap();
    let app = &s.rig.world.app.app;
    app.link.dispatcher().issue_stored(command).await.unwrap();
    assert!(admit_stored_command(app, id).await.unwrap());
    let original = rows::request(&mut s.rig.db.pool().acquire().await.unwrap(), id)
        .await
        .unwrap()
        .unwrap();
    assert!(original.model_admitted_at.is_some());
    let mut tx = s.rig.db.begin().await.unwrap();
    assert!(matches!(
        rows::rebuild(
            &mut tx,
            &original,
            original.lease_token.unwrap(),
            &original.accepted_source,
            3,
            s.rig.world.now()
        )
        .await
        .unwrap(),
        rows::Rebuild::Admitted
    ));
    tx.commit().await.unwrap();
    let retained = rows::request(&mut s.rig.db.pool().acquire().await.unwrap(), id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(retained.call_id, original.call_id);
    assert_eq!(retained.read_attempt, 0);
    assert_eq!(retained.prepared, original.prepared);
}
