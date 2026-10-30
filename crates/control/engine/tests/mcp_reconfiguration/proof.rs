//! Ordered native openings and bounded metadata-only diagnostics for MCP jobs.

use super::{Command, CommandKind, Id, World};
use orch_types::Json;
use std::time::Duration;

// ScriptedCommands deliberately returns its issue map in no order. Compare the actual
// session-opening executions, whose recorder preserves native admission order instead.
pub(super) fn session_openings(world: &World) -> Vec<Json> {
    let opened: Vec<_> = world
        .seen
        .of_kind(CommandKind::SessionOpen)
        .into_iter()
        .map(|command| {
            let Command::SessionOpen(body) = command else {
                unreachable!("the recorder filtered session.open")
            };
            serde_json::to_value(body).unwrap()
        })
        .collect();
    assert_eq!(
        opened.len(),
        world.commands.issued_of("session.open"),
        "every issued session opening must have reached the scripted native peer"
    );
    opened
}

type Observation = (String, Json, Option<Json>, Option<Json>);

async fn observe(world: &World, job: Id) -> Observation {
    sqlx::query_as("SELECT state,progress,error,plan->'runner' FROM jobs WHERE id=$1")
        .bind(job)
        .fetch_one(world.pool.pool())
        .await
        .unwrap()
}

fn command_counts(world: &World) -> String {
    let executed = world.machine.handler.executions();
    let counts: Vec<_> = [
        CommandKind::McpStart,
        CommandKind::McpRestart,
        CommandKind::SessionMcpReload,
        CommandKind::SessionOpen,
        CommandKind::TurnStart,
        CommandKind::TurnInterrupt,
    ]
    .into_iter()
    .map(|kind| {
        (
            kind,
            world.commands.issued_of(kind.as_str()),
            executed.iter().filter(|run| run.kind == kind).count(),
        )
    })
    .collect();
    format!("kind/issued/admitted {counts:?}")
}

pub(super) async fn restart_ends(world: &World, job: Id, expected: &str) {
    restart_ends_within(world, job, expected, super::common::PATIENCE).await;
}

pub(super) async fn restart_ends_within(
    world: &World,
    job: Id,
    expected: &str,
    patience: Duration,
) {
    let mut observed = None;
    let watching = async {
        loop {
            let row = observe(world, job).await;
            if matches!(row.0.as_str(), "succeeded" | "failed" | "cancelled") {
                assert_eq!(
                    row.0,
                    expected,
                    "MCP job {job}; state/progress/error/runner {row:?}; {}",
                    command_counts(world)
                );
                return;
            }
            observed = Some(row);
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    };
    let finished = tokio::time::timeout(patience, watching).await.is_ok();
    assert!(
        finished,
        "MCP job {job} did not finish within {patience:?}; last state/progress/error/runner {observed:?}; {}",
        command_counts(world)
    );
}

pub(super) async fn reload_is_held(world: &World, job: Id) {
    let mut observed = None;
    let watching = async {
        loop {
            if world
                .machine
                .handler
                .executions()
                .iter()
                .any(|run| run.kind == CommandKind::SessionMcpReload)
            {
                return;
            }
            let row = observe(world, job).await;
            assert!(
                matches!(row.0.as_str(), "queued" | "running"),
                "MCP job {job} ended before admitting the held reload; state/progress/error/runner {row:?}; {}",
                command_counts(world)
            );
            observed = Some(row);
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    };
    let patience = super::common::PATIENCE;
    let admitted = tokio::time::timeout(patience, watching).await.is_ok();
    assert!(
        admitted,
        "MCP job {job} did not admit the held reload within {patience:?}; last state/progress/error/runner {observed:?}; {}",
        command_counts(world)
    );
}
