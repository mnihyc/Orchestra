//! A machine's session stream changes only the session it hosts.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]

mod common;
mod hosting;

use common::{MODEL, World, eventually, message_began, message_done, queued, turn_ended};
use orch_agent_contract::descriptor::ArrangementType;
use orch_authz::Caller;
use orch_db::queries::link as link_queries;
use orch_engine::binding::NewThread;
use orch_link::streams::DeliveredEvent;
use orch_link_contract::commands::{Command, CommandKind, OrchTool, OrchToolResult};
use orch_link_contract::events::{
    Event, OrchToolCall, RawFrame, SessionLost, TurnOutcome, UsageUpdated,
};
use orch_link_contract::streams::Stream;
use orch_types::{Id, Json, new_id};
use serde_json::json;

fn lost() -> Event {
    Event::SessionLost(SessionLost {
        reason: "the session closed".to_owned(),
    })
}

async fn wait_ack(world: &World, machine: Id, stream: &Stream, through: i64) {
    eventually("the link event to be taken in", || async {
        world.commands.acknowledged(machine, stream) >= through
    })
    .await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn environment_and_harness_reports_are_taken_only_from_the_machine_that_holds_them() {
    let world = World::new().await;
    let other = world
        .add_machine("foreign-reporter", &["agent", "environment"])
        .await;
    let mut facts = hosting::facts();
    facts["user"] = json!("source-user");
    let mut declaration = common::descriptor();
    declaration["x_source"] = json!("rightful");
    let declaration: orch_agent_contract::descriptor::AgentDescriptor =
        serde_json::from_value(declaration).unwrap();
    let expected_declaration = serde_json::to_value(&declaration).unwrap();
    let reports = [
        (
            Stream::Env {
                environment_id: world.environment,
            },
            Event::decode("env.facts", facts).unwrap(),
        ),
        (
            Stream::Harness {
                provider_id: world.provider,
                agent_id: world.agent,
            },
            Event::HarnessDescriptor(declaration),
        ),
    ];
    let before_facts: Json = world
        .scalar(
            "SELECT facts FROM environments WHERE id = $1",
            world.environment,
        )
        .await;
    let before_descriptor: Json = world
        .scalar("SELECT descriptor FROM agents WHERE id = $1", world.agent)
        .await;
    for (stream, event) in &reports {
        other.machine.publish(stream, event).await;
        wait_ack(&world, other.id, stream, 1).await;
        assert_eq!(cursor(&world, other.id, stream).await, None);
    }
    assert_eq!(
        world
            .scalar::<Json>(
                "SELECT facts FROM environments WHERE id = $1",
                world.environment
            )
            .await,
        before_facts
    );
    assert_eq!(
        world
            .scalar::<Json>("SELECT descriptor FROM agents WHERE id = $1", world.agent)
            .await,
        before_descriptor
    );
    for (stream, event) in &reports {
        world.machine.publish(stream, event).await;
        wait_ack(&world, world.machine_id, stream, 1).await;
        assert_eq!(cursor(&world, world.machine_id, stream).await, Some(1));
    }
    assert_eq!(
        world
            .scalar::<Json>(
                "SELECT facts FROM environments WHERE id = $1",
                world.environment
            )
            .await["user"],
        "source-user"
    );
    assert_eq!(
        world
            .scalar::<Json>("SELECT descriptor FROM agents WHERE id = $1", world.agent)
            .await,
        expected_declaration
    );
}

async fn cursor(world: &World, machine: Id, stream: &Stream) -> Option<i64> {
    link_queries::cursors(&mut world.pool.acquire().await.unwrap(), machine)
        .await
        .unwrap()
        .get(&stream.to_string())
        .copied()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_machine_that_hosts_a_session_can_report_its_end() {
    let world = World::new().await;
    let thread = world.thread("Its own session").await;
    let (start, _) = world.running_turn(thread, "Work on this").await;
    world.say(thread, lost()).await;
    world.wait_turn(start.turn_id, "interrupted").await;
    assert_eq!(world.turn_state(start.turn_id).await, "interrupted");
    let error: Json = world
        .scalar("SELECT error FROM turns WHERE id = $1", start.turn_id)
        .await;
    assert_eq!(error["code"], "session_lost");
    assert_eq!(error["retryable"], true);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn another_authenticated_agent_machine_cannot_end_a_session_it_does_not_host() {
    let world = World::new().await;
    let thread = world.thread("Another machine's session").await;
    let (start, _) = world.running_turn(thread, "Work on this").await;
    let segment = world.segment_of(thread).await;
    let other = world.add_machine("separate-agent", &["agent"]).await;
    let stream = Stream::Session {
        thread_id: thread,
        segment_id: segment,
    };
    other.machine.publish(&stream, &lost()).await;
    wait_ack(&world, other.id, &stream, 1).await;
    assert_eq!(world.turn_state(start.turn_id).await, "running");
    assert_eq!(cursor(&world, other.id, &stream).await, None);
}

fn usage(turn: Id) -> Event {
    let figures = json!({"input_tokens":13,"output_tokens":5,"total_tokens":18});
    Event::UsageUpdated(UsageUpdated {
        turn_id: turn,
        turn: serde_json::from_value(figures.clone()).unwrap(),
        thread: serde_json::from_value(figures).unwrap(),
        context_window: Some(200_000),
        context_used: Some(100),
        report: None,
    })
}

fn tool(turn: Id, call_ref: &str) -> Event {
    Event::OrchToolCall(OrchToolCall {
        turn_id: turn,
        call_ref: call_ref.to_owned(),
        attempt: None,
        tool: OrchTool::ListShared,
        arguments: json!({}),
    })
}

fn raw() -> Event {
    Event::RawFrame(RawFrame {
        n: 1,
        direction: "in".to_owned(),
        payload: json!({"message":"a private harness frame"}),
    })
}

fn tool_results(world: &World, call_ref: &str) -> Vec<OrchToolResult> {
    world
        .seen
        .of_kind(CommandKind::OrchToolResult)
        .into_iter()
        .filter_map(|command| match command {
            Command::OrchToolResult(result) if result.call_ref == call_ref => Some(result),
            _ => None,
        })
        .collect()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_foreign_session_cannot_write_items_usage_raw_or_call_an_orch_tool_and_the_source_can() {
    let world = World::new().await;
    world
        .exec(r#"UPDATE agents SET descriptor_overrides = '{"capabilities":{"orch_tools":"mcp"}}'"#)
        .await;
    let thread = world.thread("Source checked before effects").await;
    let (start, _) = world.running_turn(thread, "Work").await;
    let segment = world.segment_of(thread).await;
    let other = world.add_machine("foreign-agent", &["agent"]).await;
    let stream = Stream::Session {
        thread_id: thread,
        segment_id: segment,
    };
    let call_ref = new_id().to_string();
    let events = [
        message_began(start.turn_id, "injected-item"),
        message_done("injected-item", "injected private text"),
        usage(start.turn_id),
        tool(start.turn_id, &call_ref),
        lost(),
        raw(),
    ];
    let before_usage: Json = world
        .scalar(
            "SELECT coalesce(usage, '{}'::jsonb) FROM turns WHERE id = $1",
            start.turn_id,
        )
        .await;
    let before_commands = world.commands.kinds().len();
    // An internal caller must observe the same source boundary as the API's link.
    let delivered: Vec<DeliveredEvent> = events
        .iter()
        .enumerate()
        .map(|(seq, event)| DeliveredEvent {
            seq: seq as i64 + 1,
            at: world.clock.time(),
            kind: event.kind().as_str().to_owned(),
            body: event.body().unwrap(),
        })
        .collect();
    let mut tx = world.pool.begin().await.unwrap();
    world
        .engine()
        .apply_stream_events(&mut tx, other.id, &stream, &delivered)
        .await
        .unwrap();
    tx.commit().await.unwrap();
    for event in &events {
        other.machine.publish(&stream, event).await;
    }
    wait_ack(&world, other.id, &stream, events.len() as i64).await;
    assert_eq!(world.turn_state(start.turn_id).await, "running");
    assert_eq!(
        world
            .count(
                "SELECT count(*) FROM items WHERE thread_id = $1 AND native_ref = 'injected-item'",
                thread
            )
            .await,
        0
    );
    let after_usage: Json = world
        .scalar(
            "SELECT coalesce(usage, '{}'::jsonb) FROM turns WHERE id = $1",
            start.turn_id,
        )
        .await;
    assert_eq!(after_usage, before_usage);
    assert_eq!(
        world
            .count(
                "SELECT count(*) FROM raw_events WHERE segment_id = $1",
                segment
            )
            .await,
        0
    );
    assert_eq!(world.commands.kinds().len(), before_commands);
    assert!(other.seen.of_kind(CommandKind::OrchToolResult).is_empty());
    assert_eq!(cursor(&world, other.id, &stream).await, None);

    // The same item, usage and call identity are accepted from the recorded source.
    for event in &events[..4] {
        world.say(thread, event.clone()).await;
    }
    world.say(thread, raw()).await;
    eventually("the rightful usage, item, raw frame and tool answer", || async {
        let used: Json = world.scalar("SELECT coalesce(usage, '{}'::jsonb) FROM turns WHERE id = $1", start.turn_id).await;
        used["total_tokens"] == 18
            && world.count("SELECT count(*) FROM items WHERE thread_id = $1 AND native_ref = 'injected-item'", thread).await == 1
            && world.count("SELECT count(*) FROM raw_events WHERE segment_id = $1", segment).await == 1
            && !tool_results(&world, &call_ref).is_empty()
    }).await;
    assert!(tool_results(&world, &call_ref)[0].error.is_none());
    world.say(thread, lost()).await;
    world.wait_turn(start.turn_id, "interrupted").await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_foreign_report_does_not_restore_a_thread_blocked_because_its_source_is_offline() {
    let world = World::new().await;
    let thread = world.thread("Offline source").await;
    let (start, _) = world.running_turn(thread, "Work").await;
    let segment = world.segment_of(thread).await;
    let other = world.add_machine("foreign-online", &["agent"]).await;
    let stream = Stream::Session {
        thread_id: thread,
        segment_id: segment,
    };
    // Its three reports must be taken before it is marked offline; queued rightful reports
    // would themselves prove its return independently of the foreign report under test.
    wait_ack(&world, world.machine_id, &stream, 3).await;
    world.set_machine_online(world.machine_id, false).await;
    world
        .engine()
        .machine_offline(world.machine_id)
        .await
        .unwrap();
    world.wait_blocked(thread, "agent_offline").await;
    other
        .machine
        .publish(&stream, &message_began(start.turn_id, "offline-injection"))
        .await;
    wait_ack(&world, other.id, &stream, 1).await;
    assert_eq!(world.state(thread).await.0, "blocked");
    assert_eq!(
        world.state(thread).await.1.as_deref(),
        Some("agent_offline")
    );
    assert_eq!(cursor(&world, other.id, &stream).await, None);
    world.set_machine_online(world.machine_id, true).await;
    world
        .say(thread, message_began(start.turn_id, "offline-injection"))
        .await;
    world.wait_state(thread, "running").await;
    eventually("the source's item to be taken", || async {
        world.count("SELECT count(*) FROM items WHERE thread_id = $1 AND native_ref = 'offline-injection'", thread).await == 1
    }).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_ended_segment_keeps_its_old_source_after_relocation_and_the_new_segment_takes_the_new_machine()
 {
    let world = World::on_a_key("test-session-source-key").await;
    world
        .exec(r#"UPDATE env_templates SET spec = '{"colocated_harnesses":["fake"]}'"#)
        .await;
    let thread = world
        .engine()
        .create_thread(
            &Caller::session(world.ada),
            NewThread {
                topic: world.topic,
                name: "Relocated harness".to_owned(),
                agent: world.agent,
                provider: world.provider,
                environment: world.environment,
                directory: "web".to_owned(),
                arrangement: Some(ArrangementType::KeySeparated),
                model: Some(MODEL.to_owned()),
                effort: None,
            },
        )
        .await
        .unwrap()
        .thread;
    let (first, _) = world.running_turn(thread, "Before relocation").await;
    let old_segment = world.segment_of(thread).await;
    world
        .say(thread, turn_ended(first.turn_id, TurnOutcome::Completed))
        .await;
    world.wait_state(thread, "idle").await;
    let target = world
        .add_machine("relocated-harness", &["agent", "environment"])
        .await;
    sqlx::query(
        "UPDATE environments SET machine_id = $2, generation = generation + 1 WHERE id = $1",
    )
    .bind(world.environment)
    .bind(target.id)
    .execute(world.pool.pool())
    .await
    .unwrap();
    world
        .engine()
        .submit_input(
            &Caller::session(world.ada),
            thread,
            queued("After relocation"),
        )
        .await
        .unwrap();
    eventually("the new machine to start the next turn", || async {
        target.seen.of_kind(CommandKind::TurnStart).iter().any(
            |command| matches!(command, Command::TurnStart(start) if start.thread_id == thread),
        )
    })
    .await;
    let second = target
        .seen
        .of_kind(CommandKind::TurnStart)
        .into_iter()
        .find_map(|command| match command {
            Command::TurnStart(start) if start.thread_id == thread => Some(start),
            _ => None,
        })
        .unwrap();
    let new_segment = world.segment_of(thread).await;
    assert_ne!(old_segment, new_segment);
    let old_stream = Stream::Session {
        thread_id: thread,
        segment_id: old_segment,
    };
    let new_stream = Stream::Session {
        thread_id: thread,
        segment_id: new_segment,
    };
    target
        .machine
        .publish(&new_stream, &common::turn_began(second.turn_id))
        .await;
    world.wait_turn(second.turn_id, "running").await;
    wait_ack(&world, target.id, &new_stream, 1).await;
    target.machine.publish(&old_stream, &raw()).await;
    wait_ack(&world, target.id, &old_stream, 1).await;
    assert_eq!(cursor(&world, target.id, &old_stream).await, None);
    assert_eq!(
        world
            .count(
                "SELECT count(*) FROM raw_events WHERE segment_id = $1",
                old_segment
            )
            .await,
        0
    );
    world.set_machine_online(target.id, false).await;
    world.engine().machine_offline(target.id).await.unwrap();
    world.wait_blocked(thread, "agent_offline").await;
    world.machine.publish(&old_stream, &raw()).await;
    eventually("a late report from the old source", || async {
        world
            .count(
                "SELECT count(*) FROM raw_events WHERE segment_id = $1",
                old_segment,
            )
            .await
            == 1
    })
    .await;
    assert_eq!(world.state(thread).await.0, "blocked");
    assert_eq!(
        world.state(thread).await.1.as_deref(),
        Some("agent_offline")
    );
    world.set_machine_online(target.id, true).await;
    target.machine.publish(&new_stream, &raw()).await;
    world.wait_state(thread, "running").await;
    eventually("the new source's raw frame", || async {
        world
            .count(
                "SELECT count(*) FROM raw_events WHERE segment_id = $1",
                new_segment,
            )
            .await
            == 1
    })
    .await;
    assert_eq!(cursor(&world, target.id, &new_stream).await, Some(2));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_former_harness_machine_cannot_receive_a_cached_answer_of_a_later_machine() {
    let world = World::on_a_key("test-cached-answer-source-key").await;
    world
        .exec(r#"UPDATE env_templates SET spec = '{"colocated_harnesses":["fake"]}'"#)
        .await;
    world
        .exec(r#"UPDATE agents SET descriptor_overrides = '{"capabilities":{"orch_tools":"mcp"}}'"#)
        .await;
    // Keep the former harness distinct from the provider's credential custodian as well as its successor.
    let former = world
        .add_machine("former-tool-source", &["agent", "environment"])
        .await;
    sqlx::query("UPDATE environments SET machine_id=$2 WHERE id=$1")
        .bind(world.environment)
        .bind(former.id)
        .execute(world.pool.pool())
        .await
        .unwrap();
    let thread = world
        .engine()
        .create_thread(
            &Caller::session(world.ada),
            NewThread {
                topic: world.topic,
                name: "A moved tool source".to_owned(),
                agent: world.agent,
                provider: world.provider,
                environment: world.environment,
                directory: "web".to_owned(),
                arrangement: Some(ArrangementType::KeySeparated),
                model: Some(MODEL.to_owned()),
                effort: None,
            },
        )
        .await
        .unwrap()
        .thread;
    world
        .engine()
        .submit_input(
            &Caller::session(world.ada),
            thread,
            queued("Before the move"),
        )
        .await
        .unwrap();
    eventually("the former harness to start", || async {
        former.seen.of_kind(CommandKind::TurnStart).iter().any(
            |command| matches!(command, Command::TurnStart(start) if start.thread_id == thread),
        )
    })
    .await;
    let first = former
        .seen
        .of_kind(CommandKind::TurnStart)
        .into_iter()
        .find_map(|command| match command {
            Command::TurnStart(start) if start.thread_id == thread => Some(start),
            _ => None,
        })
        .unwrap();
    let old_segment = world.segment_of(thread).await;
    let old_stream = Stream::Session {
        thread_id: thread,
        segment_id: old_segment,
    };
    former
        .machine
        .publish(&old_stream, &common::turn_began(first.turn_id))
        .await;
    world.wait_turn(first.turn_id, "running").await;
    former
        .machine
        .publish(
            &old_stream,
            &turn_ended(first.turn_id, TurnOutcome::Completed),
        )
        .await;
    world.wait_state(thread, "idle").await;
    let target = world
        .add_machine("new-tool-source", &["agent", "environment"])
        .await;
    sqlx::query("UPDATE environments SET machine_id=$2,generation=generation+1 WHERE id=$1")
        .bind(world.environment)
        .bind(target.id)
        .execute(world.pool.pool())
        .await
        .unwrap();
    world
        .engine()
        .submit_input(
            &Caller::session(world.ada),
            thread,
            queued("After the move"),
        )
        .await
        .unwrap();
    eventually("the new harness to start", || async {
        target.seen.of_kind(CommandKind::TurnStart).iter().any(
            |command| matches!(command, Command::TurnStart(start) if start.thread_id == thread),
        )
    })
    .await;
    let second = target
        .seen
        .of_kind(CommandKind::TurnStart)
        .into_iter()
        .find_map(|command| match command {
            Command::TurnStart(start) if start.thread_id == thread => Some(start),
            _ => None,
        })
        .unwrap();
    let current = Stream::Session {
        thread_id: thread,
        segment_id: world.segment_of(thread).await,
    };
    target
        .machine
        .publish(&current, &common::turn_began(second.turn_id))
        .await;
    world.wait_turn(second.turn_id, "running").await;
    let marker = "description-written-after-relocation";
    sqlx::query("UPDATE nodes SET description=$2 WHERE id=$1")
        .bind(world.topic)
        .bind(marker)
        .execute(world.pool.pool())
        .await
        .unwrap();
    let reference = new_id().to_string();
    target
        .machine
        .publish(&current, &tool(second.turn_id, &reference))
        .await;
    let current_answers = || {
        target
            .seen
            .of_kind(CommandKind::OrchToolResult)
            .into_iter()
            .filter_map(|command| match command {
                Command::OrchToolResult(answer) if answer.call_ref == reference => Some(answer),
                _ => None,
            })
            .collect::<Vec<_>>()
    };
    eventually("the current harness's answer", || async {
        !current_answers().is_empty()
    })
    .await;
    let answer = current_answers().pop().unwrap();
    assert!(
        answer.error.is_none(),
        "the rightful current call succeeds: {answer:?}"
    );
    assert!(
        answer
            .result
            .as_ref()
            .is_some_and(|body| body.to_string().contains(marker))
    );
    assert!(former
        .seen
        .of_kind(CommandKind::OrchToolResult)
        .iter()
        .all(|command| !matches!(command, Command::OrchToolResult(reply) if reply.call_ref == reference)),
        "the old harness has not received the new answer");

    let attempt = new_id();
    let repeated = OrchToolCall {
        turn_id: first.turn_id,
        call_ref: reference.clone(),
        attempt: Some(attempt),
        tool: OrchTool::ListShared,
        arguments: json!({}),
    };
    let body = serde_json::to_value(&repeated).unwrap();
    let mut tx = world.pool.pool().begin().await.unwrap();
    assert!(
        orch_db::queries::machine_reports::accepts(
            &mut tx,
            former.id,
            &old_stream,
            &[("orch.tool_call", &body)],
        )
        .await
        .unwrap(),
        "a late report of the former machine's own segment is still its report"
    );
    tx.rollback().await.unwrap();
    let through = world.commands.acknowledged(former.id, &old_stream) + 1;
    former
        .machine
        .publish(&old_stream, &Event::OrchToolCall(repeated))
        .await;
    wait_ack(&world, former.id, &old_stream, through).await;
    // Await the exact same logical ingress too, so that absence of a reply is not a timing-based
    // negative. Both attempts share the native frame's immutable delivery identity.
    match world
        .engine()
        .answer_tool_call(&orch_engine::tools::CallContext {
            machine: former.id,
            thread,
            turn: first.turn_id,
            call_ref: reference,
            attempt: Some(attempt),
            tool: orch_agent_contract::events::OrchTool::ListShared,
            arguments: json!({}),
        })
        .await
    {
        Ok(())
        | Err(
            orch_engine::EngineError::StaleToolSource | orch_engine::EngineError::ToolCallIdentity,
        ) => {}
        Err(error) => panic!("the replay failed for a reason other than its source: {error}"),
    }
    let deliveries = world.commands.issued_commands("orch.tool_result");
    let leaked: Vec<_> = deliveries
        .iter()
        .filter(|(_, command)| {
            command.body.get("attempt") == Some(&json!(attempt))
                && command
                    .body
                    .get("result")
                    .is_some_and(|result| !result.is_null())
        })
        .collect();
    assert!(
        leaked.is_empty(),
        "a fresh retry envelope must not carry another machine's retained answer: {leaked:?}"
    );
}
