//! What the system knows of its machines from their gateways: facts, presence, and what each side does while the other is away.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]

mod common;

use common::{PATIENCE, World, eventually};
use orch_db::queries::resources;
use orch_link::machine::LinkStatus;
use orch_link_contract::enrollment::Trust;
use orch_link_contract::events::Event;
use orch_link_contract::session::{MachineFacts, Role};
use orch_link_contract::streams::Stream;
use orch_link_contract::tunnels::{Service, TicketSource};
use orch_link_contract::version::RejectCode;
use orch_pki::tickets::TicketRequest;
use orch_types::{Id, Time};
use serde_json::json;
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

fn facts(hostname: &str, up_bps: i64, down_bps: i64) -> MachineFacts {
    serde_json::from_value(json!({
        "hostname": hostname, "os_version": "Debian 13", "cpus": 8, "memory_bytes": 16_000_000_000u64,
        "harnesses": [], "adapters": [], "plugins": [], "policy": null,
        "rates": { "up_bps": up_bps, "down_bps": down_bps, "round_trip_ms": 42 }, "clock_offset_ms": 0
    }))
    .unwrap()
}

async fn row(world: &World, id: Id) -> resources::MachineRow {
    let mut conn = world.db.acquire().await.unwrap();
    resources::machine(&mut conn, id).await.unwrap().unwrap()
}

#[tokio::test]
async fn what_a_machine_says_about_itself_is_kept_beside_what_the_system_keeps_of_its_certificate()
{
    let mut world = World::start().await;
    let enrolled = world.enroll("alice-laptop").await;
    let machine = world.start_machine(&enrolled, enrolled.endpoints(Trust::Pinned));
    tokio::time::timeout(PATIENCE, machine.link.connected())
        .await
        .unwrap();
    let key_before = row(&world, machine.id).await.link().public_key;
    assert!(!key_before.is_empty());

    machine
        .link
        .publish(
            &Stream::Machine,
            &Event::MachineFacts(facts("alice-laptop", 1_000, 4_000)),
        )
        .await
        .unwrap();
    machine
        .link
        .publish(
            &Stream::Machine,
            &Event::MachineFacts(facts("alice-desktop", 2_000, 0)),
        )
        .await
        .unwrap();
    let id = machine.id;
    eventually("the facts to be written", || {
        let db = world.db.clone();
        async move {
            let mut conn = db.acquire().await.unwrap();
            let row = resources::machine(&mut conn, id).await.unwrap().unwrap();
            row.facts["hostname"] == "alice-desktop"
        }
    })
    .await;
    let after = row(&world, id).await;
    assert_eq!(
        after.link().public_key,
        key_before,
        "the key secrets are sealed to is still there"
    );
    assert_eq!(
        after.rate_up_bps,
        Some(1_250),
        "an average, a quarter of the way to each new report"
    );
    assert_eq!(
        after.rate_down_bps,
        Some(4_000),
        "and a rate of 0 is no measurement"
    );
    world.stop().await;
}

#[tokio::test]
async fn a_gateway_that_goes_away_takes_its_sessions_with_it_and_the_machines_are_online_again_when_it_is_back()
 {
    let mut world = World::start().await;
    let enrolled = world.enroll("alice-laptop").await;
    let machine = world.start_machine(&enrolled, enrolled.endpoints(Trust::Pinned));
    tokio::time::timeout(PATIENCE, machine.link.connected())
        .await
        .unwrap();
    let id = machine.id;
    eventually("the machine to be online", || {
        let link = world.api.link.clone();
        async move { link.is_online(id) }
    })
    .await;

    world.gateway.stop().await;
    eventually("the system to see the machine go", || {
        let (link, db) = (world.api.link.clone(), world.db.clone());
        async move {
            let mut conn = db.acquire().await.unwrap();
            !link.is_online(id)
                && !resources::machine(&mut conn, id)
                    .await
                    .unwrap()
                    .unwrap()
                    .online
        }
    })
    .await;
    assert_eq!(world.api.link.gateways_connected(), 0);

    world.restart_gateway().await;
    eventually("the machine to be online again", || {
        let (link, db) = (world.api.link.clone(), world.db.clone());
        async move {
            let mut conn = db.acquire().await.unwrap();
            link.is_online(id)
                && resources::machine(&mut conn, id)
                    .await
                    .unwrap()
                    .unwrap()
                    .online
        }
    })
    .await;
    assert_eq!(world.api.link.gateways_connected(), 1);
    world.stop().await;
}

#[tokio::test]
async fn while_the_system_is_away_new_sessions_are_told_to_wait_and_tunnels_and_sessions_go_on() {
    let mut world = World::start().await;
    let (agent, environment, latecomer) = (
        world.enroll("agent-box").await,
        world.enroll("environment-box").await,
        world.enroll("latecomer").await,
    );
    let near = world.start_machine_as(&agent, agent.endpoints(Trust::Pinned), vec![Role::Agent]);
    let far = world.start_machine_as(
        &environment,
        environment.endpoints(Trust::Pinned),
        vec![Role::Environment],
    );
    tokio::time::timeout(PATIENCE, async {
        near.link.connected().await;
        far.link.connected().await;
    })
    .await
    .unwrap();
    let ticket = world
        .api
        .link
        .tickets()
        .issue(&TicketRequest {
            src: TicketSource::Machine(near.id),
            dst: far.id,
            service: Service::Exec {
                executor: "codex-exec-server".to_owned(),
            },
            thread_id: None,
            tool_set: None,
            expires_at: Time::new(chrono::Utc::now() + chrono::Duration::hours(1)),
        })
        .unwrap();
    let mut incoming = far.link.incoming_tunnels();
    let (opened, accepted) = tokio::join!(near.link.open_tunnel(&ticket), async {
        incoming.next().await.unwrap().accept()
    });
    let (mut here, mut there) = (opened.unwrap(), accepted.unwrap());

    world.api_goes_away().await;
    eventually("the gateway to notice", || {
        let gateway = world.gateway.gateway.clone();
        async move { !gateway.backplane_up() }
    })
    .await;

    // The sessions that stand are held, and the tunnel between the two goes on without the system.
    assert_eq!(world.gateway.gateway.sessions_held(), 2);
    here.write_all(b"still here").await.unwrap();
    let mut got = vec![0u8; 10];
    there.read_exact(&mut got).await.unwrap();
    assert_eq!(&got, b"still here");
    there.write_all(b"and here").await.unwrap();
    let mut back = vec![0u8; 8];
    here.read_exact(&mut back).await.unwrap();
    assert_eq!(&back, b"and here");

    // A machine that comes now is told that the system cannot be asked, and to try again.
    let waiting = world.start_machine(&latecomer, latecomer.endpoints(Trust::Pinned));
    let mut status = waiting.link.status();
    let told = tokio::time::timeout(PATIENCE, async {
        loop {
            if let LinkStatus::Refused { code, .. } = &*status.borrow_and_update() {
                return *code;
            }
            status.changed().await.unwrap();
        }
    })
    .await
    .unwrap();
    assert_eq!(told, RejectCode::Busy);

    // The system comes back; the gateway tells it what it holds, and the latecomer gets in.
    world.api_comes_back().await;
    tokio::time::timeout(Duration::from_secs(60), waiting.link.connected())
        .await
        .expect("the machine that was told to wait to get in");
    let ids = [near.id, far.id, waiting.id];
    eventually("the system to know all three", || {
        let link = world.api.link.clone();
        async move { ids.iter().all(|id| link.is_online(*id)) }
    })
    .await;
    // And the tunnel is the same tunnel.
    here.write_all(b"after").await.unwrap();
    let mut after = vec![0u8; 5];
    there.read_exact(&mut after).await.unwrap();
    assert_eq!(&after, b"after");
    world.stop().await;
}
