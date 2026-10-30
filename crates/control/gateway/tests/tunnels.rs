//! A tunnel between two machines goes through the gateway without the system, carries bytes both ways, and survives a machine's reconnect.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]

mod common;

use common::{PATIENCE, World};
use orch_link::machine::LinkStatus;
use orch_link::tunnels::channel::Tunnel;
use orch_link_contract::enrollment::Trust;
use orch_link_contract::session::Role;
use orch_link_contract::tunnels::{OpenErrorCode, Service, TicketSource};
use orch_pki::tickets::TicketRequest;
use orch_types::{Id, Time};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

/// Bytes that depend on their place, so that a byte in the wrong place is a different byte.
fn pattern(seed: u64, from: u64, len: usize) -> Vec<u8> {
    (0..len as u64)
        .map(|i| {
            let x = (from + i).wrapping_mul(0x9E37_79B9_7F4A_7C15) ^ seed;
            (x >> 29) as u8
        })
        .collect()
}

fn ticket(world: &World, opener: Id, destination: Id) -> String {
    world
        .api
        .link
        .tickets()
        .issue(&TicketRequest {
            src: TicketSource::Machine(opener),
            dst: destination,
            service: Service::Exec {
                executor: "codex-exec-server".to_owned(),
            },
            thread_id: None,
            tool_set: None,
            expires_at: Time::new(chrono::Utc::now() + chrono::Duration::hours(1)),
        })
        .unwrap()
}

async fn write_pattern(mut tunnel: tokio::io::WriteHalf<Tunnel>, seed: u64, total: u64) {
    let mut at = 0u64;
    while at < total {
        let n = usize::try_from(16 * 1024u64.min(total - at))
            .unwrap()
            .min(16 * 1024);
        let n = n.min(usize::try_from(total - at).unwrap());
        tunnel.write_all(&pattern(seed, at, n)).await.unwrap();
        at += n as u64;
    }
    tunnel.shutdown().await.unwrap();
}

async fn read_pattern(
    mut tunnel: tokio::io::ReadHalf<Tunnel>,
    seed: u64,
    total: u64,
    progress: Arc<AtomicU64>,
) {
    let mut at = 0u64;
    let mut buffer = vec![0u8; 32 * 1024];
    loop {
        let n = tunnel.read(&mut buffer).await.unwrap();
        if n == 0 {
            break;
        }
        assert_eq!(
            &buffer[..n],
            &pattern(seed, at, n)[..],
            "bytes differ at {at}"
        );
        at += n as u64;
        progress.store(at, Ordering::SeqCst);
        // A reader that is not in a hurry, so that the transfer is still under way when the line is cut.
        if at % (128 * 1024) < n as u64 {
            tokio::time::sleep(Duration::from_millis(2)).await;
        }
    }
    assert_eq!(at, total, "the stream ended early or late");
}

#[tokio::test]
async fn a_tunnel_between_two_machines_carries_bytes_both_ways_and_survives_a_reconnect() {
    const TOTAL: u64 = 4 * 1024 * 1024;
    let mut world = World::start().await;
    let (agent, environment) = (
        world.enroll("agent-box").await,
        world.enroll("environment-box").await,
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
    let LinkStatus::Connected {
        session_id: first_session,
        ..
    } = near.link.status().borrow().clone()
    else {
        panic!("the machine holds a session");
    };
    let mut incoming = far.link.incoming_tunnels();
    let ticket = ticket(&world, near.id, far.id);
    let (opened, accepted) = tokio::join!(near.link.open_tunnel(&ticket), async {
        let wanted = incoming.next().await.unwrap();
        assert_eq!(wanted.service, "exec/codex-exec-server");
        wanted.accept()
    });
    let (mut here, mut there) = (opened.unwrap(), accepted.unwrap());
    assert_eq!(here.cid(), there.cid(), "both ends name the tunnel alike");

    // A few words each way first.
    here.write_all(b"hello, executor").await.unwrap();
    let mut got = vec![0u8; 15];
    there.read_exact(&mut got).await.unwrap();
    assert_eq!(&got, b"hello, executor");
    there.write_all(b"hello, harness").await.unwrap();
    let mut back = vec![0u8; 14];
    here.read_exact(&mut back).await.unwrap();
    assert_eq!(&back, b"hello, harness");

    // Then a good deal of data both ways, and the line of one machine cut while it goes.
    let (here_read, here_write) = tokio::io::split(here);
    let (there_read, there_write) = tokio::io::split(there);
    let progress = Arc::new(AtomicU64::new(0));
    let transfers = vec![
        tokio::spawn(write_pattern(here_write, 7, TOTAL)),
        tokio::spawn(read_pattern(there_read, 7, TOTAL, Arc::clone(&progress))),
        tokio::spawn(write_pattern(there_write, 11, TOTAL)),
        tokio::spawn(read_pattern(
            here_read,
            11,
            TOTAL,
            Arc::new(AtomicU64::new(0)),
        )),
    ];
    let waited = tokio::time::timeout(PATIENCE, async {
        while progress.load(Ordering::SeqCst) < TOTAL / 4 {
            tokio::time::sleep(Duration::from_millis(1)).await;
        }
    })
    .await;
    assert!(waited.is_ok(), "the transfer started");
    assert!(
        world.gateway.gateway.drop_connection(near.id),
        "the line was cut"
    );
    for transfer in transfers {
        tokio::time::timeout(Duration::from_secs(60), transfer)
            .await
            .expect("the transfer to end")
            .unwrap();
    }
    assert!(progress.load(Ordering::SeqCst) >= TOTAL);
    let LinkStatus::Connected {
        session_id: second_session,
        ..
    } = near.link.status().borrow().clone()
    else {
        panic!("the machine holds a session again");
    };
    assert_ne!(
        first_session, second_session,
        "the machine had to come back on a session of its own"
    );
    assert!(
        world.gateway.gateway.bytes_relayed() >= 2 * TOTAL,
        "and the gateway carried it"
    );
    world.stop().await;
}

#[tokio::test]
async fn a_tunnel_needs_a_ticket_the_system_signed_for_the_one_who_opens_it_and_a_destination_that_is_there()
 {
    let mut world = World::start().await;
    let (agent, environment) = (
        world.enroll("agent-box").await,
        world.enroll("environment-box").await,
    );
    let near = world.start_machine_as(&agent, agent.endpoints(Trust::Pinned), vec![Role::Agent]);
    tokio::time::timeout(PATIENCE, near.link.connected())
        .await
        .unwrap();
    let refused = |result: Result<Tunnel, orch_link::tunnels::channel::OpenError>| match result {
        Err(orch_link::tunnels::channel::OpenError::Refused { code, .. }) => code,
        Err(other) => panic!("a refusal was expected, got {other}"),
        Ok(_) => panic!("a refusal was expected"),
    };
    // The destination has no session.
    let to_nobody = ticket(&world, near.id, environment.identity.machine_id);
    assert_eq!(
        refused(near.link.open_tunnel(&to_nobody).await),
        OpenErrorCode::PeerOffline
    );
    // The ticket is another machine's.
    let others = ticket(&world, environment.identity.machine_id, near.id);
    assert_eq!(
        refused(near.link.open_tunnel(&others).await),
        OpenErrorCode::BadTicket
    );
    // Past its time.
    let late = world
        .api
        .link
        .tickets()
        .issue(&TicketRequest {
            src: TicketSource::Machine(near.id),
            dst: environment.identity.machine_id,
            service: Service::Exec {
                executor: "x".to_owned(),
            },
            thread_id: None,
            tool_set: None,
            expires_at: Time::new(chrono::Utc::now() - chrono::Duration::seconds(5)),
        })
        .unwrap();
    assert_eq!(
        refused(near.link.open_tunnel(&late).await),
        OpenErrorCode::Expired
    );
    world.stop().await;
}

#[tokio::test]
async fn a_terminal_opened_by_the_system_reaches_a_shell_on_a_machine_through_the_backplane_and_ends_with_its_session()
 {
    let mut world = World::start().await;
    let enrolled = world.enroll("environment-box").await;
    let machine = world.start_machine_as(
        &enrolled,
        enrolled.endpoints(Trust::Pinned),
        vec![Role::Environment],
    );
    tokio::time::timeout(PATIENCE, machine.link.connected())
        .await
        .unwrap();
    let mut incoming = machine.link.incoming_tunnels();
    let terminal = orch_types::new_id();
    let ticket = world
        .api
        .link
        .tickets()
        .terminal(
            machine.id,
            Service::Pty {
                terminal_id: terminal,
            },
        )
        .unwrap();
    let terminals = world.api.link.terminals().clone();
    let (opened, accepted) = tokio::join!(terminals.open(machine.id, &ticket), async {
        let wanted = incoming.next().await.unwrap();
        assert_eq!(wanted.service, format!("pty/{terminal}"));
        wanted.accept()
    });
    let (mut browser, mut shell) = (opened.unwrap(), accepted.unwrap());

    // What a person types reaches the shell, and what it prints reaches the person.
    browser.write_all(b"ls\n").await.unwrap();
    let mut typed = vec![0u8; 3];
    shell.read_exact(&mut typed).await.unwrap();
    assert_eq!(&typed, b"ls\n");
    shell.write_all(b"Cargo.toml\n").await.unwrap();
    let mut printed = vec![0u8; 11];
    browser.read_exact(&mut printed).await.unwrap();
    assert_eq!(&printed, b"Cargo.toml\n");

    // A terminal is one attachment, and does not outlive the session it went through.
    assert!(world.gateway.gateway.drop_connection(machine.id));
    let mut rest = Vec::new();
    let ended = tokio::time::timeout(Duration::from_secs(20), browser.read_to_end(&mut rest)).await;
    assert!(
        ended.is_ok(),
        "the person's end is closed when the session ends"
    );
    world.stop().await;
}

#[tokio::test]
async fn a_terminal_to_a_machine_with_no_session_is_refused_at_once() {
    let mut world = World::start().await;
    let enrolled = world.enroll("environment-box").await;
    let ticket = world
        .api
        .link
        .tickets()
        .terminal(
            enrolled.identity.machine_id,
            Service::Pty {
                terminal_id: orch_types::new_id(),
            },
        )
        .unwrap();
    let started = std::time::Instant::now();
    let opened = world
        .api
        .link
        .terminals()
        .open(enrolled.identity.machine_id, &ticket)
        .await;
    assert!(matches!(
        opened,
        Err(orch_link::tunnels::channel::OpenError::Refused {
            code: OpenErrorCode::PeerOffline,
            ..
        })
    ));
    assert!(
        started.elapsed() < Duration::from_secs(5),
        "not after the time an opening is waited for"
    );
    world.stop().await;
}
