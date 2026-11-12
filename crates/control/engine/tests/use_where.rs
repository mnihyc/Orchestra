//! Where an entry lets work run, as the engine asks for it: a thread is started, and each of its turns is run, in the environment it is asked about, and the answer is the same refusal as for a withdrawn Use; a thread is never moved to another environment or provider quietly.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]

mod common;

use common::{MODEL, World, eventually, meanwhile, queued};
use orch_access_contract::permissions::{ObjectKind, Permission};
use orch_authz::Caller;
use orch_engine::binding::{CreateThreadError, NewThread};
use orch_types::new_id;
use std::time::Duration;

fn new_thread(world: &World, name: &str) -> NewThread {
    NewThread {
        topic: world.topic,
        name: name.to_owned(),
        agent: world.agent,
        provider: world.provider,
        environment: world.environment,
        directory: "web".to_owned(),
        arrangement: None,
        model: Some(MODEL.to_owned()),
        effort: None,
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_thread_is_started_in_an_environment_the_entry_allows_and_refused_in_one_it_leaves_out() {
    let world = World::new().await;
    world.access.use_only_in(world.ada, &[new_id()]);
    let refused = world
        .engine()
        .create_thread(&Caller::session(world.ada), new_thread(&world, "Away"))
        .await;
    let Err(CreateThreadError::Denied(refusal)) = refused else {
        panic!("a refusal of the use of the provider was expected");
    };
    assert_eq!(refusal.required, Permission::ProviderUse);
    assert_eq!(refusal.object.kind, ObjectKind::Provider);
    assert_eq!(refusal.object.id, world.provider);
    let made: i64 = sqlx::query_scalar("SELECT count(*) FROM threads")
        .fetch_one(world.pool.pool())
        .await
        .unwrap();
    assert_eq!(made, 0, "nothing is made, and nothing is made elsewhere");

    world.access.use_only_in(world.ada, &[world.environment]);
    world
        .engine()
        .create_thread(&Caller::session(world.ada), new_thread(&world, "Here"))
        .await
        .expect("the entry allows the environment");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_thread_whose_entry_was_narrowed_after_it_started_waits_at_its_next_turn_as_for_a_withdrawn_use()
 {
    let world = World::new().await;
    let thread = world.thread("Narrowed").await;
    let bob = world.person("bob").await;
    world.give(bob, "interactor").await;
    world.access.use_only_in(bob, &[new_id()]);
    let held = world
        .engine()
        .submit_input(&Caller::session(bob), thread, queued("Held"))
        .await
        .unwrap();
    assert_eq!(
        held.input.state_detail.as_deref(),
        Some("model_use_required"),
        "the same wait as for a person who may not use the model"
    );
    world.wait_state(thread, "idle").await;
    meanwhile(Duration::from_millis(300)).await;
    assert_eq!(
        world.turn_starts(thread).len(),
        0,
        "it runs on nobody's access"
    );
    // Somebody whose entry reaches the environment is not held.
    let served = world
        .engine()
        .submit_input(&Caller::session(world.ada), thread, queued("Served"))
        .await
        .unwrap();
    assert_ne!(
        served.input.state_detail.as_deref(),
        Some("model_use_required")
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_turn_that_waited_for_its_environment_is_held_at_its_pre_flight_when_the_entry_leaves_the_place_out()
 {
    let world = World::new().await;
    let thread = world.thread("Waiting").await;
    let bob = world.person("bob").await;
    world.give(bob, "interactor").await;
    // The input is taken while the environment is down, and then the entry is narrowed: the sender
    // check at submission passed, so it is the pre-flight of the turn that has to ask with the place.
    world
        .exec("UPDATE environments SET status = 'failed', status_detail = 'the unit is gone'")
        .await;
    let waiting = world
        .engine()
        .submit_input(&Caller::session(bob), thread, queued("From bob"))
        .await
        .unwrap();
    world.wait_blocked(thread, "environment_unavailable").await;
    world.access.use_only_in(bob, &[new_id()]);
    eventually(
        "his input waits for somebody whose entry reaches the environment",
        || async {
            world.input_state(waiting.input.id).await
                == ("pending".to_owned(), Some("model_use_required".to_owned()))
        },
    )
    .await;
    assert_eq!(
        world.turn_starts(thread).len(),
        0,
        "it runs on nobody's access"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_models_a_reviewer_may_be_chosen_from_are_those_the_authors_entry_lets_run_in_the_environment()
 {
    let world = World::new().await;
    let thread = world.thread("Reviewed").await;
    let bob = world.person("bob").await;
    world.give(bob, "interactor").await;
    let mut conn = world.pool.acquire().await.unwrap();
    let row = orch_db::queries::threads::thread(&mut conn, thread)
        .await
        .unwrap()
        .unwrap();
    let settings = orch_engine::resolved::load(&mut conn, thread)
        .await
        .unwrap();
    world.access.use_only_in(bob, &[world.environment]);
    let offered = world
        .engine()
        .review_models(&mut conn, &row, world.provider, Some(bob), &settings)
        .await
        .unwrap();
    assert!(
        offered.iter().any(|model| model == MODEL),
        "the entry names the environment: {offered:?}"
    );
    world.access.use_only_in(bob, &[new_id()]);
    let offered = world
        .engine()
        .review_models(&mut conn, &row, world.provider, Some(bob), &settings)
        .await
        .unwrap();
    assert!(
        offered.is_empty(),
        "the entry leaves the environment out: {offered:?}"
    );
}
