//! One peer receives a hundred independent turns without account-wide lease queues.
use super::*;
use futures::future::join_all;
use orch_link::commands::Progress;
use orch_link_contract::events::Event;
use orch_link_contract::streams::Stream;

#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn a_hundred_threads_on_one_machine_all_start_without_expiry_or_a_leftover_provider_block() {
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
    let mut people = vec![world.ada];
    for number in 1..20 {
        let person = world.person(&format!("sender-{number}")).await;
        world.give(person, "interactor").await;
        people.push(person);
    }
    let mut threads = Vec::new();
    for number in 0..100 {
        threads.push(world.thread(&format!("Concurrent thread {number}")).await);
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
    let starts = join_all(
        threads
            .iter()
            .copied()
            .map(|thread| async move { (thread, shared.turn_start(thread, 0).await) }),
    )
    .await;
    for (thread, start) in &starts {
        let segment = world.segment_of(*thread).await;
        world
            .machine
            .publish(
                &Stream::Session {
                    thread_id: *thread,
                    segment_id: segment,
                },
                &common::turn_began(start.turn_id),
            )
            .await;
    }
    join_all(
        threads
            .iter()
            .copied()
            .map(|thread| async move { shared.wait_state(thread, "running").await }),
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
            if world.commands.native_progress(world.machine_id, command.id)
                == Some(Progress::Expired)
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
    join_all(
        starts
            .iter()
            .map(|(_, start)| async move { shared.wait_turn(start.turn_id, "completed").await }),
    )
    .await;
    world.shutdown().await;
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
