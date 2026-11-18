//! A message of any size crosses the whole system, from the database through the API, the gateway and the
//! machine's link and back: the answer of a machine, the command of the system. What a session cannot carry is
//! failed at once, with its size and the setting, and the command row says so.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]

mod common;

use common::{PATIENCE, World};
use orch_api_core::dispatch::CommandOutcome;
use orch_link_contract::commands::{
    Command, CommandError, CommandKind, CommandResult, ErrorCode, GitDiff, GitDiffResult, SerialKey,
};
use orch_link_contract::enrollment::Trust;
use orch_types::{Id, Time, new_id};

const MIB: usize = 1_048_576;

fn later(seconds: i64) -> Time {
    Time::new(chrono::Utc::now() + chrono::Duration::seconds(seconds))
}

/// A reading of the differences whose answer is as long as the path of the body says.
fn answer_as_long_as_the_path_says(command: &Command) -> Result<CommandResult, CommandError> {
    let Command::GitDiff(body) = command else {
        return Err(CommandError::new(ErrorCode::Internal, "not a diff"));
    };
    let size = body
        .path
        .as_deref()
        .and_then(|path| path.parse::<usize>().ok())
        .unwrap_or(0);
    Ok(CommandResult::GitDiff(GitDiffResult {
        text: "d".repeat(size),
        truncated: false,
    }))
}

/// The same command, answered with how many bytes its path has.
fn answer_with_the_length_of_the_path(command: &Command) -> Result<CommandResult, CommandError> {
    let Command::GitDiff(body) = command else {
        return Err(CommandError::new(ErrorCode::Internal, "not a diff"));
    };
    Ok(CommandResult::GitDiff(GitDiffResult {
        text: body.path.as_deref().map_or(0, str::len).to_string(),
        truncated: false,
    }))
}

async fn ask(world: &World, machine: Id, path: String) -> Id {
    let command = new_id();
    world
        .api
        .link
        .dispatcher()
        .issue_as(
            command,
            machine,
            &SerialKey::None,
            later(300),
            &GitDiff {
                environment_id: new_id(),
                path: Some(path),
                staged: None,
            },
        )
        .await
        .unwrap();
    command
}

async fn outcome(world: &World, command: Id) -> CommandOutcome {
    tokio::time::timeout(PATIENCE, world.api.link.dispatcher().wait(command))
        .await
        .expect("the command to be over")
        .unwrap()
}

#[tokio::test]
async fn the_answer_of_a_machine_larger_than_a_frame_reaches_the_system_through_the_gateway() {
    let mut world = World::start_with_config(axum::Router::new(), |_| {}).await;
    let enrolled = world.enroll("alice-laptop").await;
    let machine = world.start_machine(&enrolled, enrolled.endpoints(Trust::Pinned));
    machine
        .handler
        .on(CommandKind::GitDiff, answer_as_long_as_the_path_says);
    tokio::time::timeout(PATIENCE, machine.link.connected())
        .await
        .unwrap();
    // The size of the models list a real account gave, and then twice that of a megabyte.
    for size in [438_737, MIB] {
        let command = ask(&world, machine.id, size.to_string()).await;
        let CommandOutcome::Succeeded(body) = outcome(&world, command).await else {
            panic!("the answer did not arrive");
        };
        assert_eq!(body["text"].as_str().unwrap().len(), size);
    }
    world.stop().await;
}

#[tokio::test]
async fn a_command_larger_than_a_frame_reaches_the_machine_through_the_gateway_whole() {
    let mut world = World::start_with_config(axum::Router::new(), |_| {}).await;
    let enrolled = world.enroll("alice-laptop").await;
    let machine = world.start_machine(&enrolled, enrolled.endpoints(Trust::Pinned));
    machine
        .handler
        .on(CommandKind::GitDiff, answer_with_the_length_of_the_path);
    tokio::time::timeout(PATIENCE, machine.link.connected())
        .await
        .unwrap();
    let command = ask(&world, machine.id, "p".repeat(MIB)).await;
    let CommandOutcome::Succeeded(body) = outcome(&world, command).await else {
        panic!("the command did not end well");
    };
    assert_eq!(body["text"], MIB.to_string());
    assert_eq!(machine.handler.runs_of(command), 1);
    world.stop().await;
}

#[tokio::test]
async fn a_command_over_the_bound_fails_at_once_with_its_size_and_the_setting_and_never_reaches_the_machine()
 {
    let mut world = World::start_with_config(axum::Router::new(), |config| {
        config.link.max_message_bytes = 512 * 1024;
    })
    .await;
    let enrolled = world.enroll("alice-laptop").await;
    let machine = world.start_machine(&enrolled, enrolled.endpoints(Trust::Pinned));
    machine
        .handler
        .on(CommandKind::GitDiff, answer_with_the_length_of_the_path);
    tokio::time::timeout(PATIENCE, machine.link.connected())
        .await
        .unwrap();
    let command = ask(&world, machine.id, "p".repeat(MIB)).await;
    let CommandOutcome::Failed(error) = outcome(&world, command).await else {
        panic!("the command was not refused");
    };
    assert_eq!(error.code, "too_large");
    assert!(
        error.detail.contains("max_message_bytes") && error.detail.contains("524288"),
        "the words name the setting and its value: {}",
        error.detail
    );
    assert_eq!(machine.handler.runs_of(command), 0);
    // The session stands, and what fits goes through it.
    let small = ask(&world, machine.id, "p".to_owned()).await;
    let CommandOutcome::Succeeded(body) = outcome(&world, small).await else {
        panic!("a small command did not end well");
    };
    assert_eq!(body["text"], "1");
    world.stop().await;
}

#[tokio::test]
async fn the_answer_of_a_machine_over_the_bound_fails_the_command_with_its_size_and_the_setting() {
    let mut world = World::start_with_config(axum::Router::new(), |config| {
        config.link.max_message_bytes = 512 * 1024;
    })
    .await;
    let enrolled = world.enroll("alice-laptop").await;
    let machine = world.start_machine(&enrolled, enrolled.endpoints(Trust::Pinned));
    machine
        .handler
        .on(CommandKind::GitDiff, answer_as_long_as_the_path_says);
    tokio::time::timeout(PATIENCE, machine.link.connected())
        .await
        .unwrap();
    let command = ask(&world, machine.id, MIB.to_string()).await;
    let CommandOutcome::Failed(error) = outcome(&world, command).await else {
        panic!("the answer was not refused");
    };
    assert_eq!(error.code, "too_large");
    assert!(
        error.detail.contains("max_message_bytes") && error.detail.contains("524288"),
        "{}",
        error.detail
    );
    world.stop().await;
}
