//! New threads start with a mode declared by their program while inherited permissions remain in force.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]

mod common;

use common::{World, queued};
use orch_agent_contract::descriptor::ApprovalMode;
use orch_authz::Caller;
use orch_settings_contract::registry::keys;
use serde_json::json;

async fn modes(world: &World, modes: &[&str]) {
    let mut descriptor = common::descriptor();
    descriptor["approval_modes"] = json!(modes);
    world.declares(descriptor).await;
}

async fn approval(world: &World, thread: orch_types::Id) -> ApprovalMode {
    let mut conn = world.pool.acquire().await.unwrap();
    orch_engine::resolved::load(&mut conn, thread)
        .await
        .unwrap()
        .approval_mode
}

#[tokio::test]
async fn a_program_with_reduced_modes_starts_with_its_own_default_when_the_places_mode_is_unsupported()
 {
    let world = World::observed().await;
    modes(&world, &["ask", "bypass"]).await;
    let thread = world.thread("A program with two approval modes").await;
    assert_eq!(approval(&world, thread).await, ApprovalMode::Ask);
    world
        .engine()
        .submit_input(&Caller::session(world.ada), thread, queued("Go"))
        .await
        .unwrap();
    let started = world.turn_start(thread, 0).await;
    assert_eq!(
        started.approval_mode,
        orch_link_contract::commands::ApprovalMode::Ask
    );
}

#[tokio::test]
async fn a_supported_places_mode_stays_inherited_instead_of_being_replaced_by_the_program_default()
{
    let world = World::observed().await;
    modes(&world, &["on_request", "ask"]).await;
    world
        .set(world.topic, keys::APPROVAL_MODE, json!("ask"))
        .await;
    let thread = world.thread("Keep the supported place preference").await;
    assert_eq!(approval(&world, thread).await, ApprovalMode::Ask);
    world
        .set(world.topic, keys::APPROVAL_MODE, json!("on_request"))
        .await;
    assert_eq!(approval(&world, thread).await, ApprovalMode::OnRequest);
}

#[tokio::test]
async fn choosing_a_supported_default_does_not_widen_the_places_allowed_modes() {
    let world = World::observed().await;
    modes(&world, &["ask"]).await;
    world
        .set(
            world.topic,
            keys::APPROVAL_ALLOWED_MODES,
            json!(["on_request"]),
        )
        .await;
    let thread = world.thread("The policy still governs the default").await;
    assert_eq!(approval(&world, thread).await, ApprovalMode::Ask);
    world
        .engine()
        .submit_input(&Caller::session(world.ada), thread, queued("Go"))
        .await
        .unwrap();
    world.wait_blocked(thread, "binding_invalid").await;
    let detail: Option<String> =
        sqlx::query_scalar("SELECT blocked_detail FROM threads WHERE node_id=$1")
            .bind(thread)
            .fetch_one(world.pool.pool())
            .await
            .unwrap();
    assert_eq!(
        detail.as_deref(),
        Some(orch_engine::sessions::APPROVAL_NOT_ALLOWED)
    );
    let mut conn = world.pool.acquire().await.unwrap();
    let resolved = orch_engine::resolved::load(&mut conn, thread)
        .await
        .unwrap();
    assert_eq!(resolved.allowed_modes, vec![ApprovalMode::OnRequest]);
}

#[tokio::test]
async fn an_unsupported_locked_place_mode_is_refused_without_changing_the_lock() {
    let world = World::observed().await;
    modes(&world, &["ask"]).await;
    let mut conn = world.pool.acquire().await.unwrap();
    orch_settings::write::write(
        &mut conn,
        &orch_settings::write::Write {
            actor: orch_db::audit::Actor::User(world.ada),
            target: orch_settings::Target::Node(world.topic),
            key: keys::APPROVAL_MODE,
            value: json!("on_request"),
            locked: Some(true),
            expected_revision: None,
            may_lock: true,
            origin: &orch_db::audit::Origin::default(),
        },
        world.clock.now(),
    )
    .await
    .unwrap();
    drop(conn);
    let made = world
        .engine()
        .create_thread(
            &Caller::session(world.ada),
            orch_engine::binding::NewThread {
                topic: world.topic,
                name: "Keep the inherited lock".to_owned(),
                agent: world.agent,
                provider: world.provider,
                environment: world.environment,
                directory: "web".to_owned(),
                arrangement: None,
                model: Some(common::MODEL.to_owned()),
                effort: None,
            },
        )
        .await;
    assert!(matches!(
        made,
        Err(orch_engine::binding::CreateThreadError::Setting(_))
    ));
    assert_eq!(approval(&world, world.topic).await, ApprovalMode::OnRequest);
}

#[tokio::test]
async fn an_explicit_birth_mode_is_kept_only_when_the_program_offers_it() {
    let world = World::observed().await;
    modes(&world, &["bypass", "ask"]).await;
    for mode in ["ask", "on_request"] {
        let made = world
            .engine()
            .create_thread_with(
                &Caller::session(world.ada),
                orch_engine::binding::NewThread {
                    topic: world.topic,
                    name: "An explicit supported approval choice".to_owned(),
                    agent: world.agent,
                    provider: world.provider,
                    environment: world.environment,
                    directory: "web".to_owned(),
                    arrangement: None,
                    model: Some(common::MODEL.to_owned()),
                    effort: None,
                },
                orch_engine::binding::Birth {
                    settings: vec![(keys::APPROVAL_MODE.to_owned(), json!(mode))],
                    ..Default::default()
                },
            )
            .await;
        if mode == "ask" {
            assert_eq!(
                approval(&world, made.unwrap().thread).await,
                ApprovalMode::Ask
            );
        } else {
            assert!(matches!(
                made,
                Err(orch_engine::binding::CreateThreadError::Setting(reason))
                    if reason == orch_engine::sessions::APPROVAL_UNSUPPORTED
            ));
        }
    }
}
