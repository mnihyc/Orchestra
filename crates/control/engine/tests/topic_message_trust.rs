//! Sibling messaging uses current explicit grants; human input keeps its separate access.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]

mod common;
mod shared;

use common::{World, queued};
use orch_access_contract::{decisions::Decision, permissions::Permission};
use orch_authz::{Act, Caller, Policy, Principal, nodes};
use orch_db::queries::access::{PageKey, testing as access_testing};
use orch_engine::inputs::InputError;
use orch_link_contract::commands::{OrchTool, OrchToolResult};
use orch_types::{Id, Time};
use serde_json::json;
use shared::{a_world_whose_agent_takes_tools, call_as, ok, refused, share};

/// Items, accepted inputs, deferred notices and targeted announcements are independent write effects.
async fn target_effects(world: &World, thread: Id) -> (i64, i64, i64, i64) {
    sqlx::query_as(
        "SELECT (SELECT count(*) FROM items WHERE thread_id=$1),
            (SELECT count(*) FROM thread_inputs WHERE thread_id=$1),
            (SELECT count(*) FROM thread_pending_notices WHERE thread_id=$1),
            (SELECT count(*) FROM announcements WHERE to_thread_id=$1)",
    )
    .bind(thread)
    .fetch_one(world.pool.pool())
    .await
    .unwrap()
}

async fn send(world: &World, from: Id, to: Id, call_ref: &str, text: &str) -> OrchToolResult {
    call_as(
        world,
        from,
        OrchTool::SendMessage,
        json!({"target":{"kind":"thread","path":format!("id:{to}")},"text":text}),
        call_ref,
    )
    .await
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn driving_a_keeps_human_input_separate_and_only_current_explicit_grants_reach_b() {
    let world = a_world_whose_agent_takes_tools().await;
    let a = world.thread("A").await;
    let b = world.thread("B").await;
    let person = world.person("requester").await;
    let caller = Caller::session(person);
    let mut conn = world.pool.acquire().await.unwrap();
    for (kind, object, permission) in [
        ("node", a, "thread.interact"),
        ("node", b, "thread.view"),
        ("provider", world.provider, "provider.use"),
        ("environment", world.environment, "environment.use"),
    ] {
        access_testing::add_entry(
            &mut conn,
            &PageKey::new(kind, object, "live"),
            "any",
            permission,
            ("user", Some(person), None),
            None,
            world.clock.now(),
        )
        .await
        .unwrap();
    }
    let principal = Principal::load(&mut conn, caller.clone(), Time::new(world.clock.now()))
        .await
        .unwrap();
    assert!(matches!(
        nodes::check(
            &mut conn,
            &principal,
            b,
            Permission::ThreadView,
            Act::Use,
            Policy::default()
        )
        .await
        .unwrap(),
        Decision::Allowed { .. }
    ));
    drop(conn);
    let before_direct = target_effects(&world, b).await;
    let direct = world
        .engine()
        .submit_input(&caller, b, queued("Direct input to B"))
        .await;
    assert!(matches!(direct, Err(InputError::Denied(ref refusal))
        if refusal.required == Permission::ThreadInteract && refusal.object.id == b));
    assert_eq!(target_effects(&world, b).await, before_direct);

    let accepted = world
        .engine()
        .submit_input(&caller, a, queued("Ask the other thread for help"))
        .await
        .unwrap();
    let start = world.turn_start(a, 0).await;
    world
        .answer(a, &start, "requester-work", "Working on the accepted input")
        .await;
    world.wait_turn(start.turn_id, "running").await;
    let source: (Id, Option<Id>) =
        sqlx::query_as("SELECT input_id,audit_for_user_id FROM turns WHERE id=$1")
            .bind(start.turn_id)
            .fetch_one(world.pool.pool())
            .await
            .unwrap();
    assert_eq!(source, (accepted.input.id, Some(person)));
    let implicit: i64 = sqlx::query_scalar("SELECT count(*) FROM permission_entries WHERE object_kind='node' AND object_id=$1 AND permission='thread.interact' AND holder_kind='agents' AND holder_id=$2 AND removed_at IS NULL")
        .bind(b).bind(a).fetch_one(world.pool.pool()).await.unwrap();
    assert_eq!(
        implicit, 0,
        "the fixture starts without an agent control contribution"
    );
    let before = target_effects(&world, b).await;
    let denied = send(
        &world,
        a,
        b,
        "default-off",
        "The default must not inject this",
    )
    .await;
    assert!(
        denied.error.is_some(),
        "an untrusted sibling message was accepted"
    );
    assert_eq!(refused(&denied).0, "not_found");
    assert_eq!(target_effects(&world, b).await, before);

    let grant = share(&world, "node", b, "live", "thread.interact", "any", a).await;
    let sent = ok(send(
        &world,
        a,
        b,
        "explicit-topic",
        "An explicitly permitted sibling message",
    )
    .await);
    assert_eq!(sent["status"], "queued");
    assert_eq!(sent["delivery"], "deferred");
    let message: (Option<Id>, Option<Id>, String) = sqlx::query_as(
        "SELECT from_thread_id,to_thread_id,body FROM announcements WHERE from_thread_id=$1 AND to_thread_id=$2",
    )
    .bind(a)
    .bind(b)
    .fetch_one(world.pool.pool())
    .await
    .unwrap();
    assert_eq!(
        message,
        (
            Some(a),
            Some(b),
            "An explicitly permitted sibling message".into()
        )
    );
    let read = ok(call_as(
        &world,
        a,
        OrchTool::ReadShared,
        json!({"path":format!("id:{b}")}),
        "interact-includes-view",
    )
    .await);
    assert_eq!(
        read["entry"]["id"],
        json!(b),
        "an explicit Interact contribution also includes View"
    );
    let mut conn = world.pool.acquire().await.unwrap();
    orch_db::queries::access::remove_entry(&mut conn, grant, world.ada, world.clock.now())
        .await
        .unwrap();
    drop(conn);
    let before = target_effects(&world, b).await;
    let denied = send(
        &world,
        a,
        b,
        "off-again",
        "Removing the contribution stops further messages",
    )
    .await;
    assert_eq!(refused(&denied).0, "not_found");
    assert_eq!(target_effects(&world, b).await, before);
    share(&world, "node", b, "live", "thread.interact", "any", a).await;
    let sent = ok(send(
        &world,
        a,
        b,
        "explicit-grant",
        "An explicit grant still works",
    )
    .await);
    assert_eq!(sent["status"], "queued");
    let read = ok(call_as(
        &world,
        a,
        OrchTool::ReadShared,
        json!({"path":format!("id:{b}")}),
        "interact-keeps-view",
    )
    .await);
    assert_eq!(read["entry"]["id"], json!(b));
    assert!(matches!(
        world.engine().submit_input(&caller, b, queued("Still view-only for the person")).await,
        Err(InputError::Denied(ref refusal)) if refusal.required == Permission::ThreadInteract && refusal.object.id == b
    ));
    world.shutdown().await;
}
