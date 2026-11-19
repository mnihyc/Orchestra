//! One peer receives a hundred independent turns without account-wide lease queues.
use super::*;
use futures::future::join_all;
use orch_link::commands::Progress;
use orch_link_contract::events::Event;
use orch_link_contract::streams::Stream;
use orch_testkit::doubles::link::{Direction, Shape};
use std::sync::Arc;

struct Release(Arc<orch_testkit::doubles::machine::ScriptedHandler>);
impl Drop for Release {
    fn drop(&mut self) {
        self.0.release();
    }
}

/// Observe the whole cohort without making a hundred competing fixture readers.
async fn wait_cohort(
    world: &World,
    query: &'static str,
    identities: &[orch_types::Id],
    state: &str,
) {
    let began = std::time::Instant::now();
    loop {
        let states: Vec<(String, i64)> = sqlx::query_as(query)
            .bind(identities)
            .fetch_all(world.pool.pool())
            .await
            .unwrap();
        if states.len() == 1
            && states[0].0 == state
            && states[0].1 == i64::try_from(identities.len()).unwrap()
        {
            return;
        }
        assert!(
            began.elapsed() < common::PATIENCE,
            "all {} turns reach {state}; their states are {states:?}",
            identities.len()
        );
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn a_hundred_threads_on_ten_providers_prepare_together_and_pipeline_inputs_over_a_long_line()
{
    // Keep real domain time: a frozen clock would hide queue-induced command expiry.
    let world = World::on_a_key("scripted-scale-key-not-a-model-credential").await;
    world.machine.handler.on(CommandKind::FsMkdir, |command| {
        let Command::FsMkdir(body) = command else {
            panic!("the directory request arrives");
        };
        Ok(orch_link_contract::commands::CommandResult::FsMkdir(
            serde_json::from_value(serde_json::json!({
                "name":body.path.rsplit('/').next().unwrap_or_default(),
                "path":body.path,"type":"dir","size":0,
                "mtime":"2026-10-08T14:00:00Z","mode":493,"link_target":null,
                "cached":false,"hash":null,"media_type":"inode/directory","is_text":false
            }))
            .unwrap(),
        ))
    });
    let mut providers = vec![world.provider];
    for number in 1..10 {
        providers.push(
            world
                .api_provider(&format!("Scripted provider {number}"))
                .await,
        );
    }
    let mut people = vec![world.ada];
    for number in 1..20 {
        let person = world.person(&format!("sender-{number}")).await;
        world.give(person, "interactor").await;
        people.push(person);
    }
    let mut threads = Vec::new();
    for number in 0..100 {
        threads.push(
            world
                .thread_on_provider(
                    world.ada,
                    providers[number % providers.len()],
                    &format!("Concurrent thread {number}"),
                )
                .await,
        );
    }
    world.machine.handler.hold(CommandKind::AgentStart);
    world.machine.handler.hold(CommandKind::SessionOpen);
    let _release = Release(world.machine.handler.clone());
    for direction in [Direction::Down, Direction::Up] {
        world.hub.faults().shape(
            direction,
            Some(Shape {
                delay: Duration::from_millis(700),
                bytes_per_second: u64::MAX,
                loss: 0.0,
            }),
        );
    }
    let shared = &world;
    let sent = join_all(threads.iter().copied().enumerate().map(|(at, thread)| {
        let sender = people[at % people.len()];
        async move {
            shared
                .engine()
                .submit_input(&Caller::session(sender), thread, queued("Begin together"))
                .await
                .expect("each sender's input is accepted")
        }
    }))
    .await;
    assert_eq!(sent.len(), 100);
    tokio::time::timeout(
        common::PATIENCE,
        world
            .machine
            .handler
            .started_kind(CommandKind::AgentStart, 10),
    )
    .await
    .unwrap();
    // Every independent lease/directory preparation is in flight while all ten
    // harness answers are held. An API wait on a harness would prevent this witness.
    eventually(
        "all preparations are issued before any harness answer",
        || {
            let ready = world.commands.issued_of("lease.open") >= 100
                && world.commands.issued_of("fs.mkdir") >= 100;
            async move { ready }
        },
    )
    .await;
    world.machine.handler.release_kind(CommandKind::AgentStart);
    tokio::time::timeout(
        common::PATIENCE,
        world
            .machine
            .handler
            .started_kind(CommandKind::SessionOpen, 100),
    )
    .await
    .unwrap();
    // Every actual native input is durably queued before any opening returns.
    // This is the third wave of the same start graph proved by round_trips.
    eventually(
        "all inputs reach their native prerequisite queues before opening replies",
        || {
            let inputs: Vec<_> = world
                .commands
                .issued_commands("turn.start")
                .into_iter()
                .chain(world.commands.issued_commands("turn.start.fenced"))
                .collect();
            let ready = inputs.len() == 100
                && inputs.iter().all(|(_, command)| {
                    world.commands.native_progress(world.machine_id, command.id)
                        == Some(Progress::Accepted)
                });
            async move { ready }
        },
    )
    .await;
    assert_eq!(
        world
            .machine
            .handler
            .executions()
            .iter()
            .filter(|entry| {
                matches!(
                    entry.kind,
                    CommandKind::TurnStart | CommandKind::TurnStartFenced
                )
            })
            .count(),
        0,
        "queued inputs must not cross held opening receipts"
    );
    world.machine.handler.release_kind(CommandKind::SessionOpen);

    let starts = join_all(threads.iter().copied().map(|thread| async move {
        let start = shared.turn_start(thread, 0).await;
        shared
            .machine
            .publish(
                &Stream::Session {
                    thread_id: thread,
                    segment_id: shared.segment_of(thread).await,
                },
                &common::turn_began(start.turn_id),
            )
            .await;
        (thread, start)
    }))
    .await;
    wait_cohort(
        &world,
        "SELECT state,count(*) FROM threads WHERE node_id=ANY($1) GROUP BY state",
        &threads,
        "running",
    )
    .await;
    let mut expired = Vec::new();
    for kind in [
        "fs.record",
        "fs.mkdir",
        "provider.start",
        "agent.start",
        "lease.open",
        "session.open",
        "turn.start",
        "turn.start.fenced",
    ] {
        for (_, command) in world.commands.issued_commands(kind) {
            let progress = world.commands.native_progress(world.machine_id, command.id);
            if matches!(progress, Some(Progress::Expired))
                || matches!(&progress, Some(Progress::Failed(error))
                    if error.known() == Some(orch_link_contract::commands::ErrorCode::Timeout))
            {
                expired.push((kind, command.id));
            }
        }
    }
    let mut blocked = Vec::new();
    for thread in &threads {
        let state = world.state(*thread).await;
        if state.0 == "blocked" {
            blocked.push((*thread, state));
        }
    }
    // Each input has a real separate opening prerequisite, retaining the three-wave start plan.
    let pipelined: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM commands WHERE kind IN ('turn.start','turn.start.fenced')
         AND body->>'thread_id'=ANY($1) AND after_command_id IS NOT NULL",
    )
    .bind(threads.iter().map(ToString::to_string).collect::<Vec<_>>())
    .fetch_one(world.pool.pool())
    .await
    .unwrap();
    for (thread, start) in &starts {
        world
            .machine
            .publish(
                &Stream::Session {
                    thread_id: *thread,
                    segment_id: world.segment_of(*thread).await,
                },
                &Event::TurnCompleted(orch_link_contract::events::TurnCompleted {
                    turn_id: start.turn_id,
                    status: TurnOutcome::Completed,
                    error: None,
                }),
            )
            .await;
    }
    let turns: Vec<_> = starts.iter().map(|(_, start)| start.turn_id).collect();
    wait_cohort(
        &world,
        "SELECT state,count(*) FROM turns WHERE id=ANY($1) GROUP BY state",
        &turns,
        "completed",
    )
    .await;
    world.hub.faults().clear();
    world.shutdown().await;
    eprintln!(
        "100 threads,20 people,10 providers,700ms each way: preparations and opening/input cohorts were in flight together"
    );
    assert!(
        expired.is_empty(),
        "no command expires in an unrelated queue: {expired:?}"
    );
    assert!(
        blocked.is_empty(),
        "no thread stays blocked after simultaneous starts: {blocked:?}"
    );
    assert_eq!(
        pipelined, 100,
        "all first inputs retain their local opening dependency"
    );
}
