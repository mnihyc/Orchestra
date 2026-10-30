//! The first thing a gateway has to do: a machine enrolls with a token and holds a session through it to the API.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]

mod common;

use common::{PATIENCE, World, eventually};
use orch_db::queries::resources;
use orch_link_contract::commands::{SerialKey, SessionClose};
use orch_link_contract::enrollment::Trust;
use orch_types::new_id;

#[tokio::test]
async fn a_machine_enrolls_with_a_token_and_holds_a_session_through_a_gateway_to_the_api() {
    let mut world = World::start().await;
    let enrolled = world.enroll("alice-laptop").await;
    let machine = world.start_machine(&enrolled, enrolled.endpoints(Trust::Pinned));
    tokio::time::timeout(PATIENCE, machine.link.connected())
        .await
        .expect("the machine reached the gateway");

    // The system knows it: online, with the address it came from.
    let link = world.api.link.clone();
    let id = machine.id;
    eventually("the API to see the machine online", || {
        let link = link.clone();
        async move { link.is_online(id) }
    })
    .await;
    let row = {
        let mut conn = world.db.acquire().await.unwrap();
        resources::machine(&mut conn, id).await.unwrap().unwrap()
    };
    assert_eq!(row.name, "alice-laptop");
    assert_eq!(row.owner_id, Some(world.user));
    assert_eq!(row.placement, "connected");
    assert!(row.is_active());
    eventually("the row to say online", || {
        let db = world.db.clone();
        async move {
            let mut conn = db.acquire().await.unwrap();
            let row = resources::machine(&mut conn, id).await.unwrap().unwrap();
            row.online && row.last_ip.as_deref() == Some("127.0.0.1")
        }
    })
    .await;
    assert_eq!(world.gateway.gateway.sessions_held(), 1);

    // And it answers a command the system gives it.
    let thread = new_id();
    world
        .api
        .link
        .dispatcher()
        .run(
            id,
            &SerialKey::Thread(thread),
            &SessionClose { thread_id: thread },
        )
        .await
        .unwrap();
    world.stop().await;
}
