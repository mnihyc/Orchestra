//! A real database preserves only reviewed End targets and truthful outcomes, with live native facts kept out of replay.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]
mod common;
use common::{World, eventually};
use orch_api_contract::background::{BackgroundEndAll, BackgroundEndRequest, BackgroundEndResult};
use orch_authz::{Caller, Principal};
use orch_engine::background::Identity;
use orch_link_contract::background::{
    BackgroundEndState, BackgroundPlace, BackgroundState, NativeBackground,
    NativeBackgroundCommand, NativeBackgroundEnd,
};
use orch_link_contract::commands::{Command, CommandKind, CommandResult};
use orch_types::{Json, new_id};
use std::collections::HashSet;
use std::sync::{Arc, Mutex};

fn native(id: &str) -> NativeBackgroundCommand {
    NativeBackgroundCommand {
        id: id.into(),
        native_item_ref: None,
        turn_id: None,
        command: Some("PRIVATE-ROSTER-COMMAND".into()),
        command_truncated: false,
        cwd: None,
        place: BackgroundPlace::Environment,
        since: None,
        cpu_percent: None,
        memory_bytes: None,
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn end_all_keeps_only_the_two_hundred_fifty_reviewed_identities_and_reload_reads_new_native_facts()
 {
    let world = World::new().await;
    let thread = world.thread("A retained service").await;
    world.running_turn(thread, "Start").await;
    let commands = Arc::new(Mutex::new(
        (0..250)
            .map(|index| native(&index.to_string()))
            .collect::<Vec<_>>(),
    ));
    let reading = Arc::clone(&commands);
    world
        .machine
        .handler
        .on(CommandKind::SessionBackground, move |_| {
            let items = reading.lock().unwrap().clone();
            let total = items.len() as i64;
            Ok(CommandResult::SessionBackground(NativeBackground {
                supported: true,
                as_of: None,
                state: BackgroundState::Live,
                reason: None,
                items,
                next_cursor: None,
                total: Some(total),
            }))
        });
    let ended = Arc::new(Mutex::new(HashSet::<String>::new()));
    let endings = Arc::clone(&ended);
    let live = Arc::clone(&commands);
    world
        .machine
        .handler
        .on(CommandKind::SessionBackgroundEnd, move |command| {
            let Command::SessionBackgroundEnd(body) = command else {
                panic!("an exact native end was expected")
            };
            let fresh = endings.lock().unwrap().insert(body.command_id.clone());
            live.lock()
                .unwrap()
                .retain(|command| command.id != body.command_id);
            Ok(CommandResult::SessionBackgroundEnd(NativeBackgroundEnd {
                id: body.command_id.clone(),
                state: if fresh {
                    BackgroundEndState::Ended
                } else {
                    BackgroundEndState::AlreadyEnded
                },
                reason: None,
            }))
        });
    let credential = new_id();
    let caller = Caller::session(world.ada);
    let identity = Identity {
        credential_id: credential,
        token: false,
    };
    sqlx::query("INSERT INTO sessions (id,user_id,refresh_hash,expires_at) VALUES ($1,$2,$3,$4)")
        .bind(credential)
        .bind(world.ada)
        .bind(vec![7_u8; 32])
        .bind(world.clock.now() + chrono::Duration::hours(1))
        .execute(world.pool.pool())
        .await
        .unwrap();
    let first = world
        .engine()
        .background(&caller, identity, thread, None, None)
        .await
        .unwrap();
    assert_eq!(first.items.len(), 50);
    assert_eq!(first.total, Some(250));
    assert!(first.can_end);
    let mut last = first.clone();
    let mut transferred = first.items.len();
    while let Some(cursor) = last.next_cursor.clone() {
        last = world
            .engine()
            .background(&caller, identity, thread, Some(&cursor), None)
            .await
            .unwrap();
        transferred += last.items.len();
        assert_eq!(last.list_ref, first.list_ref);
    }
    assert_eq!(transferred, 250);
    assert_eq!(
        world.commands.issued_of("session.background"),
        0,
        "a native roster never enters durable command storage"
    );
    commands.lock().unwrap().push(native("later"));
    let mut tx = world.pool.begin().await.unwrap();
    let principal = Principal::load(&mut tx, caller.clone(), world.clock.time())
        .await
        .unwrap();
    let row = world
        .engine()
        .end_background_guarded(
            tx,
            principal,
            identity,
            thread,
            BackgroundEndRequest::All(BackgroundEndAll {
                list_ref: last.list_ref.unwrap(),
                all: true,
                count: 250,
            }),
        )
        .await
        .unwrap();
    // End is deliberately serial and checkpoints every reviewed identity. This case proves
    // 250 mutations, not that their aggregate fits one unrelated eventual assertion window.
    // Each committed checkpoint must make progress within the same unchanged patience bound.
    for completed in 1_i64..=250 {
        eventually(
            &format!("captured native End checkpoint {completed}"),
            || async {
                let (state, outcomes, error): (String, i64, Option<Json>) = sqlx::query_as(
                    "SELECT state, COALESCE(jsonb_array_length(plan->'outcomes'),0)::bigint, error
                   FROM jobs WHERE id=$1",
                )
                .bind(row.id)
                .fetch_one(world.pool.pool())
                .await
                .unwrap();
                assert!(
                    !matches!(state.as_str(), "failed" | "cancelled"),
                    "{state}: {error:?}"
                );
                outcomes >= completed
            },
        )
        .await;
    }
    eventually("the captured native End job finishes", || async {
        world
            .scalar::<String>("SELECT state FROM jobs WHERE id=$1", row.id)
            .await
            == "succeeded"
    })
    .await;
    assert_eq!(ended.lock().unwrap().len(), 250);
    assert!(!ended.lock().unwrap().contains("later"));
    let result: Json = world
        .scalar("SELECT result FROM jobs WHERE id=$1", row.id)
        .await;
    let result: BackgroundEndResult = serde_json::from_value(result).unwrap();
    assert_eq!(result.results.len(), 250);
    assert!(
        result
            .results
            .iter()
            .all(|result| result.state == orch_api_contract::background::BackgroundEndState::Ended)
    );
    let plan: Json = world
        .scalar("SELECT plan FROM jobs WHERE id=$1", row.id)
        .await;
    assert!(!plan.to_string().contains("PRIVATE-ROSTER-COMMAND"));
    assert!(plan.get("items").is_none());
    assert!(plan.get("load").is_none());
    assert_eq!(
        commands.lock().unwrap().len(),
        1,
        "a later command is left running"
    );
    let targets: Vec<Json> = serde_json::from_value(plan["targets"].clone()).unwrap();
    assert!(targets.iter().all(|target| target.get("command").is_none()
        && target.get("cwd").is_none()
        && target.get("cpu_percent").is_none()));
    let reloaded = world
        .engine()
        .background(&caller, identity, thread, None, None)
        .await
        .unwrap();
    assert_eq!(
        reloaded.state,
        orch_api_contract::background::BackgroundState::Live
    );
    assert_eq!(reloaded.total, Some(1));
    assert_eq!(
        reloaded.items.len(),
        1,
        "reload observes the later command rather than the captured roster"
    );
    assert_ne!(reloaded.list_ref, first.list_ref);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_offline_existing_session_is_unknown_and_another_callers_review_is_refused() {
    let world = World::new().await;
    let thread = world.thread("Offline native facts").await;
    world.running_turn(thread, "Start").await;
    let identity = Identity {
        credential_id: new_id(),
        token: false,
    };
    let caller = Caller::session(world.ada);
    let read = world
        .engine()
        .background(&caller, identity, thread, None, None)
        .await
        .unwrap();
    assert_ne!(
        read.state,
        orch_api_contract::background::BackgroundState::Live
    );
    assert_eq!(read.total, None);
    assert!(!read.can_end);
    let cursor = format!("{}/0", read.list_ref.unwrap());
    let replaced = Identity {
        credential_id: new_id(),
        token: false,
    };
    assert!(matches!(
        world
            .engine()
            .background(&caller, replaced, thread, Some(&cursor), None)
            .await,
        Err(orch_engine::background::BackgroundError::Stale)
    ));
    assert_eq!(world.commands.issued_of("session.background"), 0);
}
