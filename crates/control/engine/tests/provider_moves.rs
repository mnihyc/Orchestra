//! A provider's runtime whose host is changed: the plan says before anything is done that it moves, what it carries (its sign-ins and harness homes) and what it leaves on the old host, and nothing blocks it any more.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]

mod common;
mod hosting;
mod runtimes;

use common::World;
use hosting::{ScriptedHost, serve};
use orch_api_contract::environment::UpdatePreflightReason;
use orch_authz::{Caller, Principal};
use orch_engine::updates::Target;
use orch_types::{Id, Time};

async fn draft(world: &World, provider: Id) -> orch_engine::updates::Draft {
    let mut tx = world.pool.begin().await.unwrap();
    let principal = Principal::load(
        &mut tx,
        Caller::session(world.ada),
        Time::new(world.clock.now()),
    )
    .await
    .unwrap();
    let draft = world
        .engine()
        .draft_update(
            &mut tx,
            &principal,
            Target::Provider(provider),
            UpdatePreflightReason::Update,
            None,
        )
        .await
        .unwrap();
    // Only a plan is read; nothing is kept.
    drop(tx);
    draft
}

#[tokio::test]
async fn changing_a_providers_host_plans_a_move_that_carries_its_sign_ins_and_says_what_stays_behind()
 {
    let world = World::new().await;
    serve(&world);
    let build = ScriptedHost::on(&world, "build", 16_384).await;
    let elsewhere = ScriptedHost::on(&world, "elsewhere", 16_384).await;
    runtimes::provider_template(&world).await;
    let provider = runtimes::managed_subscription(&world, "Service").await;
    let (old, _unit) = runtimes::made(&world, &build, provider).await;

    // On the same host nothing moves.
    let same = draft(&world, provider).await;
    assert_eq!(same.recipe.host, None);

    sqlx::query(
        "UPDATE providers SET runtime_config = runtime_config || jsonb_build_object('runtime.host_id', $2::text) WHERE id = $1",
    )
    .bind(provider)
    .bind(elsewhere.id.to_string())
    .execute(world.pool.pool())
    .await
    .unwrap();
    let moving = draft(&world, provider).await;
    assert_eq!(moving.recipe.old, old);
    assert_eq!(
        moving.recipe.host,
        Some(elsewhere.id),
        "the new unit is made on the selected host"
    );
    assert!(
        moving
            .blockers
            .iter()
            .all(|blocker| blocker.code != "provider_host_transfer_unavailable"),
        "{:?}",
        moving.blockers
    );
    assert!(
        moving.updates.iter().any(|update| update.name == "Host"
            && update.from.as_deref() == Some("build")
            && update.to.as_deref() == Some("elsewhere")),
        "{:?}",
        moving.updates
    );
    assert!(
        moving
            .kept
            .iter()
            .any(|line| line.contains("Sign-ins") && line.contains("elsewhere")),
        "{:?}",
        moving.kept
    );
    assert!(
        moving
            .lost
            .iter()
            .any(|line| line.contains("build") && line.contains("kept")),
        "what stays on the old host is said before: {:?}",
        moving.lost
    );

    // A host that is gone blocks the move before anything is done.
    sqlx::query(
        "UPDATE providers SET runtime_config = runtime_config || jsonb_build_object('runtime.host_id', $2::text) WHERE id = $1",
    )
    .bind(provider)
    .bind(orch_types::new_id().to_string())
    .execute(world.pool.pool())
    .await
    .unwrap();
    let gone = draft(&world, provider).await;
    assert!(
        gone.blockers
            .iter()
            .any(|blocker| blocker.code == "no_host"),
        "{:?}",
        gone.blockers
    );
    assert_eq!(gone.recipe.host, None);
    world.engine().stop();
}
