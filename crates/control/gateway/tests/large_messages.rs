//! A message of any size crosses the whole system, from the database through the API, the gateway and the
//! machine's link and back: the answer of a machine, the command of the system. What a session cannot carry is
//! failed at once, with its size and the setting, and the command row says so.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]

mod common;

use common::{PATIENCE, World, eventually};
use orch_api_core::dispatch::CommandOutcome;
use orch_link_contract::commands::{
    Command, CommandError, CommandKind, CommandResult, ErrorCode, GitDiff, GitDiffResult, ProcList,
    ProviderCompleteResult, SerialKey,
};
use orch_link_contract::enrollment::Trust;
use orch_link_contract::frames::Frame;
use orch_link_contract::session::Role;
use orch_link_contract::version::Limits;
use orch_testkit::doubles::link::large_messages::example;
use orch_types::{Id, Time, new_id};
use std::sync::Arc;

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

#[tokio::test]
async fn lowering_the_message_limit_after_an_api_restart_renews_old_sessions_and_answers_commands()
{
    let mut world = World::start_with_config(axum::Router::new(), |_| {}).await;
    let enrolled = world.enroll("alice-laptop").await;
    let machine = world.start_machine(&enrolled, enrolled.endpoints(Trust::Pinned));
    machine
        .handler
        .on(CommandKind::GitDiff, answer_as_long_as_the_path_says);
    tokio::time::timeout(PATIENCE, machine.link.connected())
        .await
        .unwrap();
    let first = ask(&world, machine.id, MIB.to_string()).await;
    assert!(matches!(
        outcome(&world, first).await,
        CommandOutcome::Succeeded(_)
    ));
    let old_session = world.api.link.session_of(machine.id).unwrap();
    let mut config = (*world.config).clone();
    config.link.max_message_bytes = 512 * 1024;
    world.config = Arc::new(config);
    world.restart_api().await;
    eventually(
        "the retained machine to renegotiate the new message limit",
        || {
            let link = world.api.link.clone();
            let machine = machine.id;
            async move {
                link.session_of(machine)
                    .is_some_and(|session| session != old_session)
            }
        },
    )
    .await;
    let large = ask(&world, machine.id, MIB.to_string()).await;
    let CommandOutcome::Failed(error) = outcome(&world, large).await else {
        panic!("the new limit was not applied to the machine's answer");
    };
    assert_eq!(error.code, "too_large");
    assert!(error.detail.contains("524288"), "{}", error.detail);
    assert_eq!(machine.handler.runs_of(large), 1);
    let small = ask(&world, machine.id, "10".to_owned()).await;
    let CommandOutcome::Succeeded(body) = outcome(&world, small).await else {
        panic!("a small answer did not arrive after the limit change");
    };
    assert_eq!(body["text"].as_str().unwrap().len(), 10);
    world.stop().await;
}

#[tokio::test]
async fn a_model_request_past_one_message_reaches_the_machine_through_the_gateway() {
    let mut world = World::start_with_config(axum::Router::new(), |config| {
        config.link.max_message_bytes = 512 * 1024;
    })
    .await;
    let enrolled = world.enroll("fixture-machine").await;
    let machine = world.start_machine_as(
        &enrolled,
        enrolled.endpoints(Trust::Pinned),
        vec![Role::Provider],
    );
    machine
        .handler
        .on(CommandKind::ProviderComplete, |command| {
            let Command::ProviderComplete(body) = command else {
                panic!("a completion")
            };
            Ok(CommandResult::ProviderComplete(ProviderCompleteResult {
                text: body.input.len().to_string(),
                timing: Default::default(),
                usage: Default::default(),
                request_ref: None,
            }))
        });
    tokio::time::timeout(PATIENCE, machine.link.connected())
        .await
        .unwrap();
    let size = Limits::default().message_bytes() + 1;
    let Frame::Cmd(cmd) = example(CommandKind::ProviderComplete, size) else {
        panic!("a request")
    };
    let Command::ProviderComplete(mut body) = cmd.command().unwrap() else {
        panic!("a completion")
    };
    {
        let mut conn = world.db.acquire().await.unwrap();
        let home: Id = sqlx::query_scalar("SELECT personal_org_id FROM users WHERE id=$1")
            .bind(world.user)
            .fetch_one(&mut *conn)
            .await
            .unwrap();
        let provider = orch_db::queries::resources::access_fixtures::insert_resource(
            &mut conn,
            "provider",
            Some(world.user),
            Some(home),
            "Fixture provider",
        )
        .await
        .unwrap();
        sqlx::query(
            "UPDATE providers SET machine_id=$2,protocols=ARRAY['openai-responses'] WHERE id=$1",
        )
        .bind(provider)
        .bind(machine.id)
        .execute(&mut *conn)
        .await
        .unwrap();
        orch_db::queries::resources::access_fixtures::insert_model(
            &mut conn,
            provider,
            &body.model_name,
            &[],
            true,
            world.user,
        )
        .await
        .unwrap();
        body.provider_id = provider;
        body.protocol = "openai-responses".into();
    }
    let id = new_id();
    world
        .api
        .link
        .dispatcher()
        .issue_as(id, machine.id, &SerialKey::None, later(300), &body)
        .await
        .unwrap();
    let CommandOutcome::Succeeded(body) = outcome(&world, id).await else {
        panic!("the paged request did not succeed")
    };
    assert_eq!(body["text"], size.to_string());
    assert_eq!(machine.handler.runs_of(id), 1);
    world.stop().await;
}

#[tokio::test]
async fn a_process_list_past_one_message_arrives_whole_and_survives_an_api_restart() {
    let mut world = World::start_with_config(axum::Router::new(), |_| {}).await;
    let enrolled = world.enroll("fixture-machine").await;
    let machine = world.start_machine(&enrolled, enrolled.endpoints(Trust::Pinned));
    machine.handler.on(CommandKind::ProcList, |_| {
        let Frame::CmdRes(result) =
            example(CommandKind::ProcList, Limits::default().message_bytes() + 1)
        else {
            panic!("a reply")
        };
        Ok(CommandResult::decode(CommandKind::ProcList, result.outcome.unwrap()).unwrap())
    });
    tokio::time::timeout(PATIENCE, machine.link.connected())
        .await
        .unwrap();
    let id = new_id();
    world
        .api
        .link
        .dispatcher()
        .issue_as(
            id,
            machine.id,
            &SerialKey::None,
            later(300),
            &ProcList {
                environment_id: new_id(),
            },
        )
        .await
        .unwrap();
    let CommandOutcome::Succeeded(body) = outcome(&world, id).await else {
        panic!("the paged answer did not succeed")
    };
    assert!(serde_json::to_vec(&body).unwrap().len() > Limits::default().message_bytes());
    let first = serde_json::to_vec(&body).unwrap();
    world.restart_api().await;
    let CommandOutcome::Succeeded(body) = outcome(&world, id).await else {
        panic!("the completed paged result was not durable")
    };
    assert_eq!(serde_json::to_vec(&body).unwrap(), first);
    assert_eq!(machine.handler.runs_of(id), 1);
    world.stop().await;
}
