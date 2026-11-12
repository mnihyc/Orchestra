//! The ordinary rebind and its prepared sibling share validation and the first native stamp.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "native controls fail by panicking"
)]
mod common;
use common::{MODEL, World, config, meanwhile};
use orch_authz::{Caller, Principal};
use orch_engine::{EngineError, binding::Rebind};
use orch_types::{Id, Json, Time};
use std::sync::atomic::{AtomicBool, Ordering};

type State = (i64, Option<Id>, Option<Id>, Option<Id>, Option<String>);
async fn state(conn: &mut orch_db::Conn, thread: Id) -> State {
    sqlx::query_as("SELECT next_seq,agent_id,provider_id,current_segment_id,arrangement FROM threads WHERE node_id=$1").bind(thread).fetch_one(conn).await.unwrap()
}

/// What binding a thread could start on a machine: the harness of the binding, the unit hooks that
/// follow its start, and the probe of the machine it would run on.
const STARTS: [&str; 3] = ["agent.start", "unit.after_up", "machine.probe"];

fn rebinding(world: &World) -> Rebind {
    Rebind {
        agent: world.agent,
        provider: world.provider,
        arrangement: None,
        model: Some(MODEL.into()),
        effort: None,
    }
}

/// The commands of those kinds that the machines were asked, with their bodies, so that a failure
/// can say which.
fn asked(world: &World) -> Vec<(&'static str, Vec<(Id, Json)>)> {
    STARTS
        .iter()
        .map(|kind| (*kind, world.commands.issued_bodies(kind)))
        .collect()
}

#[tokio::test(flavor = "multi_thread")]
async fn native_rebind_prepares_without_mutation_and_consumes_its_guard_before_the_original_stamp()
{
    // A subscription that was never read is read by the engine's own sweep a moment after the world
    // stands, and that read starts its harness: that start is nobody's rebind. Observed beforehand,
    // the subscription is left alone while the case lasts, so a command of the kinds counted below
    // can only be the preparation's, wherever it falls between the counts.
    let world = World::observed().await;
    let engine = world.engine();
    let thread = world.thread("Native prepared binding").await;
    let caller = Caller::session(world.ada);
    let request = rebinding(&world);
    let mut tx = engine.db().begin().await.unwrap();
    let principal = Principal::load(&mut tx, caller, Time::new(world.clock.now()))
        .await
        .unwrap();
    let before = state(&mut tx, thread).await;
    let commands = STARTS.map(|kind| world.commands.issued_of(kind));
    let prepared = engine
        .preparing_rebind(&mut tx, &principal, thread, &request)
        .await
        .unwrap();
    assert_eq!(prepared.thread().node_id, thread);
    assert_eq!(prepared.agent().id, world.agent);
    assert_eq!(prepared.provider().id, world.provider);
    assert_eq!(state(&mut tx, thread).await, before);
    assert_eq!(
        STARTS.map(|kind| world.commands.issued_of(kind)),
        commands,
        "preparing a rebind starts nothing; the machines were asked: {:?}",
        asked(&world)
    );
    let called = AtomicBool::new(false);
    let refused = engine
        .write_rebind_before(&mut tx, &principal, &prepared, || {
            called.store(true, Ordering::Release);
            Err(EngineError::unreadable("native rebind guard control"))
        })
        .await;
    assert!(refused.is_err());
    assert!(called.load(Ordering::Acquire));
    assert_eq!(
        state(&mut tx, thread).await,
        before,
        "a declined first stamp writes no binding, setting or sequence"
    );
    let accepted = engine
        .write_rebind_before(&mut tx, &principal, &prepared, || Ok(()))
        .await
        .unwrap();
    assert_eq!(accepted.thread, thread);
    assert!(state(&mut tx, thread).await.0 > before.0);
    tx.commit().await.unwrap();
}

#[tokio::test(flavor = "multi_thread")]
async fn a_declined_native_rebind_that_is_rolled_back_leaves_nothing_to_start_afterwards() {
    let world = World::observed().await;
    let engine = world.engine();
    let thread = world.thread("Native declined binding").await;
    let caller = Caller::session(world.ada);
    let request = rebinding(&world);
    let commands = STARTS.map(|kind| world.commands.issued_of(kind));
    let mut tx = engine.db().begin().await.unwrap();
    let principal = Principal::load(&mut tx, caller, Time::new(world.clock.now()))
        .await
        .unwrap();
    let before = state(&mut tx, thread).await;
    let prepared = engine
        .preparing_rebind(&mut tx, &principal, thread, &request)
        .await
        .unwrap();
    let declined = engine
        .write_rebind_before(&mut tx, &principal, &prepared, || {
            Err(EngineError::unreadable("native rebind declined"))
        })
        .await;
    assert!(declined.is_err());
    tx.rollback().await.unwrap();
    // The transaction held the thread while it lasted, so whatever it had set going could not act
    // on it. Now it can: the engine looks again at everything that waits at least every
    // `recheck_every`, and is given two of those looks.
    meanwhile(config().recheck_every * 2).await;
    let mut tx = engine.db().begin().await.unwrap();
    assert_eq!(
        state(&mut tx, thread).await,
        before,
        "a declined and rolled back rebind leaves the thread as it was"
    );
    tx.rollback().await.unwrap();
    assert_eq!(
        STARTS.map(|kind| world.commands.issued_of(kind)),
        commands,
        "a declined rebind starts nothing, however late; the machines were asked: {:?}",
        asked(&world)
    );
}
