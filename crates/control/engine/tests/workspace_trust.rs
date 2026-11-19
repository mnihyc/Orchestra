//! A workspace's own harness configuration: refused by the harness while nobody has answered for it, asked once of
//! the people who manage the environment while every thread that meets it waits, and carried by the sessions
//! opened after the answer.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]
mod common;
use common::{World, queued};
use orch_api_contract::trust::ConfigTrustAnswer;
use orch_authz::{Caller, Principal};
use orch_link_contract::commands::{
    Command, CommandError, CommandKind, ConfigTrust, WorkspaceConfiguration,
};
use orch_types::{Json, Time};

#[tokio::test]
async fn a_workspace_configuration_is_asked_about_once_and_the_answer_reaches_every_session_after_it()
 {
    let world = World::new().await;
    let thread = world.thread("Configured workspace").await;
    let other = world.thread("Same workspace").await;
    world.refuse(
        CommandKind::SessionOpen,
        vec![CommandError {
            code: "workspace_configuration".to_owned(),
            detail: "this workspace holds a configuration of Codex's own; the people who manage the environment decide whether it is used\nfiles abc123\n.codex".to_owned(),
            retryable: false,
            details: None,
        }],
    );
    let submitted = world
        .engine()
        .submit_input(&Caller::session(world.ada), thread, queued("first"))
        .await
        .unwrap();
    world.wait_blocked(thread, "workspace_trust").await;
    let (state, _) = world.input_state(submitted.input.id).await;
    assert_eq!(state, "pending", "the input waits again where it stood");
    assert_eq!(
        world
            .count(
                "SELECT count(*) FROM items WHERE thread_id = $1 AND kind = 'error'",
                thread
            )
            .await,
        0,
        "a question is no error of the thread's"
    );
    let harness: String = world
        .scalar("SELECT harness_kind FROM agents WHERE id = $1", world.agent)
        .await;
    let kept: Json = world
        .scalar(
            "SELECT config_trust FROM environments WHERE id = $1",
            world.environment,
        )
        .await;
    assert_eq!(kept[&harness]["answer"], Json::Null, "the question waits");
    assert_eq!(kept[&harness]["files"], "abc123");
    assert_eq!(kept[&harness]["found"], serde_json::json!([".codex"]));

    // Another thread of the same environment waits for the same question, and nothing is opened for it.
    let opened_before = world.began(CommandKind::SessionOpen);
    world
        .engine()
        .submit_input(&Caller::session(world.ada), other, queued("second"))
        .await
        .unwrap();
    world.wait_blocked(other, "workspace_trust").await;
    assert_eq!(world.began(CommandKind::SessionOpen), opened_before);
    assert_eq!(
        world
            .count(
                "SELECT count(*) FROM jsonb_object_keys((SELECT config_trust FROM environments WHERE id = $1))",
                world.environment
            )
            .await,
        1,
        "asked once"
    );

    // Kept out by a person who manages the environment: remembered for it, and the thread goes on.
    let mut tx = world.pool.begin().await.unwrap();
    let principal = Principal::load(
        &mut tx,
        Caller::session(world.ada),
        Time::new(world.clock.now()),
    )
    .await
    .unwrap();
    world
        .engine()
        .set_config_trust_guarded(
            tx,
            principal,
            world.environment,
            &harness,
            ConfigTrustAnswer::KeepOut,
        )
        .await
        .unwrap();
    let kept: Json = world
        .scalar(
            "SELECT config_trust FROM environments WHERE id = $1",
            world.environment,
        )
        .await;
    assert_eq!(kept[&harness]["answer"], "keep_out");
    assert_eq!(
        kept[&harness]["files"], "abc123",
        "the answer holds for the files it was asked about"
    );
    world.run_turn(thread, 0, "completed").await;
    let opened = world.seen.of_kind(CommandKind::SessionOpen);
    let Some(Command::SessionOpen(last)) = opened.last() else {
        panic!("a session open")
    };
    assert_eq!(
        last.workspace_configuration,
        Some(WorkspaceConfiguration {
            answer: ConfigTrust::KeepOut,
            files: "abc123".to_owned(),
        }),
        "the session after the answer carries it, with the files it holds for"
    );
}
