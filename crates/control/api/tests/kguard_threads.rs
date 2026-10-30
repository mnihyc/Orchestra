//! A thread's changing requests ask for their authority again in the transaction that writes: what is sent to an agent, a turn stopped, and a thread made, each refused whole when its sender was signed out while its body came, and done once when they were not.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]

mod places;
mod rig;
mod support;

use axum::body::Body;
use bytes::Bytes;
use chrono::Duration;
use futures::stream;
use http::{Request, StatusCode};
use orch_api_core::middleware::authentication::token_hash;
use orch_api_core::registry::Registry;
use orch_db::queries::identity;
use orch_types::Id;
use places::Who;
use rig::Scene;
use serde_json::json;
use std::future::Future;
use support::World;
use tower::ServiceExt;

/// Sends a request whose body arrives only after `change` has happened: the request is let in at its door
/// first, and whatever it writes is decided after the body came.
async fn send_after(
    world: &World,
    who: &Who,
    path: &str,
    value: serde_json::Value,
    change: impl Future<Output = ()> + Send + 'static,
) -> axum::response::Response {
    let body = Body::from_stream(stream::once(async move {
        change.await;
        Ok::<_, std::io::Error>(Bytes::from(serde_json::to_vec(&value).unwrap()))
    }));
    let mut registry = Registry::new();
    orch_api_core::routes::mount(&mut registry);
    registry
        .into_router()
        .with_state(world.app.app.clone())
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(path)
                .header("authorization", format!("Bearer {}", who.token))
                .header("content-type", "application/json")
                .body(body)
                .unwrap(),
        )
        .await
        .unwrap()
}

/// The browser session `who` is signed in with.
async fn session_of(world: &World, who: &Who) -> Id {
    let now = world.now();
    identity::session_by_access_token(&mut *world.conn().await, &token_hash(&who.token), now)
        .await
        .unwrap()
        .unwrap()
        .session_id
}

/// Signs `who` out of `session`, as another tab or an admin would while a body is on its way.
fn signing_out(world: &World, who: &Who, session: Id) -> impl Future<Output = ()> + Send + 'static {
    let pool = world.db.pool.clone();
    let user = who.id();
    let now = world.now();
    async move {
        let mut tx = pool.begin().await.unwrap();
        identity::revoke_session(&mut tx, session, user, now)
            .await
            .unwrap();
        tx.commit().await.unwrap();
    }
}

/// Time passes while the body comes, and nothing else changes.
fn waiting(world: &World) -> impl Future<Output = ()> + Send + 'static {
    let clock = world.app.clock.clone();
    async move {
        clock.advance(Duration::seconds(2));
    }
}

async fn inputs_of(world: &World, thread: Id) -> i64 {
    sqlx::query_scalar("SELECT count(*) FROM thread_inputs WHERE thread_id = $1")
        .bind(thread)
        .fetch_one(&world.db.pool)
        .await
        .unwrap()
}

async fn threads_in(world: &World, topic: Id) -> i64 {
    sqlx::query_scalar("SELECT count(*) FROM nodes WHERE parent_id = $1 AND kind = 'thread'")
        .bind(topic)
        .fetch_one(&world.db.pool)
        .await
        .unwrap()
}

fn words(client: &str) -> serde_json::Value {
    json!({ "content": [{ "type": "text", "text": "Late words" }], "delivery": "queue", "client_id": client })
}

fn a_thread(s: &Scene, name: &str) -> serde_json::Value {
    json!({
        "name": name,
        "binding": { "agent_id": s.on.agent, "provider_id": s.on.provider },
        "model": rig::MODEL,
        "environment": { "use": s.on.environment },
        "directory": "web",
    })
}

#[tokio::test]
async fn an_input_whose_sender_was_signed_out_while_its_body_came_is_not_taken() {
    let s = Scene::new().await;
    let thread = s.thread().await.base.id;
    let session = session_of(&s.rig.world, &s.ada).await;
    let refused = send_after(
        &s.rig.world,
        &s.ada,
        &format!("/threads/{thread}/inputs"),
        words("signed-out"),
        signing_out(&s.rig.world, &s.ada, session),
    )
    .await;
    assert_eq!(refused.status(), StatusCode::UNAUTHORIZED);
    assert_eq!(inputs_of(&s.rig.world, thread).await, 0);
}

#[tokio::test]
async fn an_input_by_somebody_still_signed_in_after_a_slow_body_is_taken_once() {
    let s = Scene::new().await;
    let thread = s.thread().await.base.id;
    let taken = send_after(
        &s.rig.world,
        &s.ada,
        &format!("/threads/{thread}/inputs"),
        words("slow"),
        waiting(&s.rig.world),
    )
    .await;
    assert!(taken.status().is_success(), "{}", taken.status());
    assert_eq!(inputs_of(&s.rig.world, thread).await, 1);
}

#[tokio::test]
async fn a_thread_whose_maker_was_signed_out_while_its_body_came_is_not_made() {
    let s = Scene::new().await;
    let topic = s.tree.topic.base.id;
    let session = session_of(&s.rig.world, &s.ada).await;
    let refused = send_after(
        &s.rig.world,
        &s.ada,
        &format!("/topics/{topic}/threads"),
        a_thread(&s, "Late"),
        signing_out(&s.rig.world, &s.ada, session),
    )
    .await;
    assert_eq!(refused.status(), StatusCode::UNAUTHORIZED);
    assert_eq!(threads_in(&s.rig.world, topic).await, 0);
}

#[tokio::test]
async fn a_thread_made_after_a_slow_body_is_made_once() {
    let s = Scene::new().await;
    let topic = s.tree.topic.base.id;
    let made = send_after(
        &s.rig.world,
        &s.ada,
        &format!("/topics/{topic}/threads"),
        a_thread(&s, "Slow"),
        waiting(&s.rig.world),
    )
    .await;
    assert!(made.status().is_success(), "{}", made.status());
    assert_eq!(threads_in(&s.rig.world, topic).await, 1);
}
