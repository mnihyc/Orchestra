//! The ordinary rebind and its prepared sibling share validation and the first native stamp.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "native controls fail by panicking"
)]
mod common;
use common::{MODEL, World};
use orch_authz::{Caller, Principal};
use orch_engine::{EngineError, binding::Rebind};
use orch_types::{Id, Time};
use std::sync::atomic::{AtomicBool, Ordering};

type State = (i64, Option<Id>, Option<Id>, Option<Id>, Option<String>);
async fn state(conn: &mut orch_db::Conn, thread: Id) -> State {
    sqlx::query_as("SELECT next_seq,agent_id,provider_id,current_segment_id,arrangement FROM threads WHERE node_id=$1").bind(thread).fetch_one(conn).await.unwrap()
}
#[tokio::test(flavor = "multi_thread")]
async fn native_rebind_prepares_without_mutation_and_consumes_its_guard_before_the_original_stamp()
{
    let world = World::new().await;
    let engine = world.engine();
    let thread = world.thread("Native prepared binding").await;
    let caller = Caller::session(world.ada);
    let request = Rebind {
        agent: world.agent,
        provider: world.provider,
        arrangement: None,
        model: Some(MODEL.into()),
        effort: None,
    };
    let mut tx = engine.db().begin().await.unwrap();
    let principal = Principal::load(&mut tx, caller, Time::new(world.clock.now()))
        .await
        .unwrap();
    let before = state(&mut tx, thread).await;
    let kinds = ["agent.start", "unit.after_up", "machine.probe"];
    let commands = kinds.map(|kind| world.commands.issued_of(kind));
    let prepared = engine
        .preparing_rebind(&mut tx, &principal, thread, &request)
        .await
        .unwrap();
    assert_eq!(prepared.thread().node_id, thread);
    assert_eq!(prepared.agent().id, world.agent);
    assert_eq!(prepared.provider().id, world.provider);
    assert_eq!(state(&mut tx, thread).await, before);
    assert_eq!(kinds.map(|kind| world.commands.issued_of(kind)), commands);
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
