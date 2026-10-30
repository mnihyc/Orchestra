//! Tunnels between two machines through a gateway: bytes in order, within a window, across lost sessions.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]

use chrono::TimeZone;
use orch_link::tunnels::channel::{OpenError, Tunnel, TunnelOptions};
use orch_link::tunnels::flow::Gauge;
use orch_link_contract::session::Role;
use orch_link_contract::tunnels::{OpenErrorCode, Service, TicketSource};
use orch_pki::tickets::{TicketRequest, TicketSigner};
use orch_testkit::doubles::controller::ScriptedController;
use orch_testkit::doubles::link::{Hub, TestPki};
use orch_testkit::doubles::machine::{MachineSpec, ScriptedMachine};
use orch_types::{Clock, Id, ManualClock, Time, new_id};
use std::sync::Arc;
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

struct World {
    clock: Arc<dyn Clock>,
    hub: Hub,
    signer: TicketSigner,
    opener: ScriptedMachine,
    destination: ScriptedMachine,
    opener_id: Id,
    destination_id: Id,
    gauge: Arc<Gauge>,
    /// The tunnels others open to the destination: taken once, kept here.
    incoming: tokio::sync::Mutex<orch_link::tunnels::channel::IncomingTunnels>,
    _controllers: Vec<ScriptedController>,
}

fn clock() -> Arc<dyn Clock> {
    Arc::new(ManualClock::new(
        chrono::Utc
            .with_ymd_and_hms(2026, 10, 8, 14, 0, 0)
            .single()
            .unwrap(),
    ))
}

async fn world(grace: Duration) -> World {
    let clock = clock();
    let pki = TestPki::new(Arc::clone(&clock));
    let hub = Hub::new(Arc::clone(&clock), pki.certificate());
    let signer = TicketSigner::generate();
    hub.set_ticket_verifier(signer.verifier());
    hub.set_relay_grace(grace);
    let gauge = Arc::new(Gauge::new());
    let spec = |roles: Vec<Role>| MachineSpec {
        roles,
        tunnels: TunnelOptions {
            grace,
            ..TunnelOptions::default()
        },
        gauge: Some(Arc::clone(&gauge)),
        ..MachineSpec::default()
    };
    let (opener_id, destination_id) = (new_id(), new_id());
    let controllers = vec![
        ScriptedController::attach(&hub, Arc::clone(&clock), opener_id, vec![Role::Agent]),
        ScriptedController::attach(
            &hub,
            Arc::clone(&clock),
            destination_id,
            vec![Role::Environment],
        ),
    ];
    let opener = ScriptedMachine::start(
        &hub,
        &clock,
        &pki.machine(opener_id),
        &spec(vec![Role::Agent]),
    );
    let destination = ScriptedMachine::start(
        &hub,
        &clock,
        &pki.machine(destination_id),
        &spec(vec![Role::Environment]),
    );
    opener.link.connected().await;
    destination.link.connected().await;
    let incoming = tokio::sync::Mutex::new(destination.link.incoming_tunnels());
    World {
        clock,
        hub,
        signer,
        opener,
        destination,
        opener_id,
        destination_id,
        gauge,
        incoming,
        _controllers: controllers,
    }
}

impl World {
    fn ticket_for(
        &self,
        src: TicketSource,
        dst: Id,
        executor: &str,
        lasts: chrono::Duration,
    ) -> String {
        self.signer
            .issue(&TicketRequest {
                src,
                dst,
                service: Service::Exec {
                    executor: executor.to_owned(),
                },
                thread_id: None,
                tool_set: None,
                expires_at: Time::new(self.clock.now() + lasts),
            })
            .unwrap()
    }

    fn ticket(&self) -> String {
        self.ticket_for(
            TicketSource::Machine(self.opener_id),
            self.destination_id,
            "codex-exec-server",
            chrono::Duration::hours(1),
        )
    }

    /// Opens a tunnel from the opener to the destination and returns both ends.
    async fn tunnel(&self) -> (Tunnel, Tunnel) {
        let mut incoming = self.incoming.lock().await;
        let ticket = self.ticket();
        let (opened, accepted) = tokio::join!(self.opener.link.open_tunnel(&ticket), async {
            let wanted = incoming.next().await.unwrap();
            assert_eq!(wanted.service, "exec/codex-exec-server");
            wanted.accept()
        });
        (opened.unwrap(), accepted.unwrap())
    }
}

#[tokio::test]
async fn a_tunnel_carries_bytes_both_ways_and_each_end_sees_the_other_end_it_is() {
    let world = world(Duration::from_secs(60)).await;
    let (mut near, mut far) = world.tunnel().await;
    assert_eq!(near.cid(), far.cid(), "both ends name the tunnel alike");
    near.write_all(b"hello, executor").await.unwrap();
    let mut got = vec![0u8; 15];
    far.read_exact(&mut got).await.unwrap();
    assert_eq!(&got, b"hello, executor");
    far.write_all(b"hello, harness").await.unwrap();
    let mut back = vec![0u8; 14];
    near.read_exact(&mut back).await.unwrap();
    assert_eq!(&back, b"hello, harness");
    near.shutdown().await.unwrap();
    let mut rest = Vec::new();
    far.read_to_end(&mut rest).await.unwrap();
    assert!(
        rest.is_empty(),
        "the end of one direction is an end of file for the reader"
    );
    far.write_all(b"still open the other way").await.unwrap();
    let mut more = vec![0u8; 24];
    near.read_exact(&mut more).await.unwrap();
    assert_eq!(&more, b"still open the other way");
}

#[tokio::test]
async fn a_ticket_is_checked_for_who_presents_it_and_when() {
    let world = world(Duration::from_secs(60)).await;
    let refused = |error: Result<Tunnel, OpenError>| match error.unwrap_err() {
        OpenError::Refused { code, .. } => code,
        other => panic!("expected a refusal, got {other}"),
    };
    // Another machine's ticket.
    let strangers = world.ticket_for(
        TicketSource::Machine(new_id()),
        world.destination_id,
        "x",
        chrono::Duration::hours(1),
    );
    assert_eq!(
        refused(world.opener.link.open_tunnel(&strangers).await),
        OpenErrorCode::BadTicket
    );
    // A ticket the system did not sign.
    let forged = TicketSigner::generate()
        .issue(&TicketRequest {
            src: TicketSource::Machine(world.opener_id),
            dst: world.destination_id,
            service: Service::Exec {
                executor: "x".to_owned(),
            },
            thread_id: None,
            tool_set: None,
            expires_at: Time::new(world.clock.now() + chrono::Duration::hours(1)),
        })
        .unwrap();
    assert_eq!(
        refused(world.opener.link.open_tunnel(&forged).await),
        OpenErrorCode::BadTicket
    );
    // Past its time.
    let late = world.ticket_for(
        TicketSource::Machine(world.opener_id),
        world.destination_id,
        "x",
        chrono::Duration::seconds(-5),
    );
    assert_eq!(
        refused(world.opener.link.open_tunnel(&late).await),
        OpenErrorCode::Expired
    );
    // A destination that is not there.
    let nobody = world.ticket_for(
        TicketSource::Machine(world.opener_id),
        new_id(),
        "x",
        chrono::Duration::hours(1),
    );
    assert_eq!(
        refused(world.opener.link.open_tunnel(&nobody).await),
        OpenErrorCode::PeerOffline
    );
}

#[tokio::test]
async fn a_destination_that_refuses_a_tunnel_says_why() {
    let world = world(Duration::from_secs(60)).await;
    let mut incoming = world.incoming.lock().await;
    let ticket = world.ticket();
    let (opened, ()) = tokio::join!(world.opener.link.open_tunnel(&ticket), async {
        incoming
            .next()
            .await
            .unwrap()
            .refuse(OpenErrorCode::NoSuchService, "no such executor here");
    });
    match opened.unwrap_err() {
        OpenError::Refused { code, detail } => {
            assert_eq!(code, OpenErrorCode::NoSuchService);
            assert_eq!(detail, "no such executor here");
        }
        other => panic!("{other}"),
    }
}

fn pattern(seed: u64, from: u64, len: usize) -> Vec<u8> {
    // Bytes that depend on their place, so that a byte in the wrong place is a different byte.
    (0..len as u64)
        .map(|i| {
            let x = (from + i).wrapping_mul(0x9E37_79B9_7F4A_7C15) ^ seed;
            (x >> 29) as u8
        })
        .collect()
}

/// Writes `total` bytes of the pattern in pieces of `piece`, then shuts the direction down.
async fn write_pattern(
    mut tunnel: tokio::io::WriteHalf<Tunnel>,
    seed: u64,
    total: u64,
    piece: usize,
) {
    let mut at = 0u64;
    while at < total {
        let n = piece.min((total - at) as usize);
        tunnel
            .write_all(&pattern(seed, at, n))
            .await
            .expect("a write on a tunnel failed");
        at += n as u64;
    }
    tunnel.shutdown().await.expect("a shutdown failed");
}

/// Reads to the end and checks every byte against the pattern.
async fn read_pattern(mut tunnel: tokio::io::ReadHalf<Tunnel>, seed: u64, total: u64) {
    let mut at = 0u64;
    let mut buffer = vec![0u8; 64 * 1024];
    loop {
        let n = tunnel
            .read(&mut buffer)
            .await
            .expect("a read on a tunnel failed");
        if n == 0 {
            break;
        }
        assert_eq!(
            &buffer[..n],
            &pattern(seed, at, n)[..],
            "bytes differ at {at}"
        );
        at += n as u64;
    }
    assert_eq!(at, total, "the stream ended early or late");
}

#[tokio::test]
async fn a_hundred_mebibytes_each_way_under_a_one_mebibyte_window_arrive_identical_and_memory_stays_bounded()
 {
    const TOTAL: u64 = 100 * 1024 * 1024;
    const WINDOW: u64 = 1024 * 1024;
    const REPLAY: u64 = 8 * 1024 * 1024;
    let world = world(Duration::from_secs(60)).await;
    let (near, far) = world.tunnel().await;
    let (near_read, near_write) = tokio::io::split(near);
    let (far_read, far_write) = tokio::io::split(far);
    let work = async {
        tokio::join!(
            tokio::spawn(write_pattern(near_write, 11, TOTAL, 256 * 1024)),
            tokio::spawn(read_pattern(far_read, 11, TOTAL)),
            tokio::spawn(write_pattern(far_write, 22, TOTAL, 100 * 1024)),
            tokio::spawn(read_pattern(near_read, 22, TOTAL)),
        )
    };
    let (a, b, c, d) = tokio::time::timeout(Duration::from_secs(300), work)
        .await
        .expect("the transfer stood still");
    a.unwrap();
    b.unwrap();
    c.unwrap();
    d.unwrap();
    // Per direction an end keeps what it sent and was not acknowledged (at most the replay buffer) and the
    // receiver holds what arrived and was not read (at most the window). Two directions.
    let bound = 2 * (REPLAY + WINDOW);
    let peak = world.gauge.peak();
    assert!(
        peak <= bound,
        "the tunnels held {peak} bytes at the most, the bound is {bound}"
    );
    assert!(peak > 0);
}

#[tokio::test]
async fn a_reader_that_stops_reading_stops_the_writer_within_one_window() {
    const WINDOW: u64 = 1024 * 1024;
    let world = world(Duration::from_secs(60)).await;
    let (mut near, far) = world.tunnel().await;
    // The far end never reads. The near end writes until it is held up.
    let block = vec![7u8; 64 * 1024];
    let mut accepted = 0u64;
    let held_up = tokio::time::timeout(Duration::from_millis(1_500), async {
        loop {
            near.write_all(&block).await.unwrap();
            accepted += block.len() as u64;
        }
    })
    .await;
    assert!(held_up.is_err(), "the writer was not held up");
    // What the writer got rid of is what the reader's window and the replay buffer hold, no more.
    assert!(
        accepted <= WINDOW + 8 * 1024 * 1024 + block.len() as u64,
        "{accepted}"
    );
    assert!(world.gauge.current() <= 2 * 8 * 1024 * 1024 + WINDOW);
    drop(far);
}

#[tokio::test]
async fn a_tunnel_whose_session_is_lost_and_found_within_the_grace_never_shows_a_close_to_its_program()
 {
    const TOTAL: u64 = 24 * 1024 * 1024;
    let world = world(Duration::from_secs(60)).await;
    let (near, far) = world.tunnel().await;
    let (near_read, near_write) = tokio::io::split(near);
    let (far_read, far_write) = tokio::io::split(far);
    let writers = (
        tokio::spawn(write_pattern(near_write, 5, TOTAL, 128 * 1024)),
        tokio::spawn(write_pattern(far_write, 6, TOTAL, 64 * 1024)),
    );
    let readers = (
        tokio::spawn(read_pattern(far_read, 5, TOTAL)),
        tokio::spawn(read_pattern(near_read, 6, TOTAL)),
    );
    // Cut every session, twice, while the bytes flow.
    for _ in 0..2 {
        tokio::time::sleep(Duration::from_millis(150)).await;
        world.hub.faults().sever_now();
        tokio::time::sleep(Duration::from_millis(1_800)).await;
    }
    let all = async {
        writers.0.await.unwrap();
        writers.1.await.unwrap();
        readers.0.await.unwrap();
        readers.1.await.unwrap();
    };
    tokio::time::timeout(Duration::from_secs(120), all)
        .await
        .expect("the tunnel did not carry on");
    assert!(
        world.hub.faults().connections() >= 6,
        "both machines came back at least twice"
    );
}

#[tokio::test]
async fn a_tunnel_that_is_not_resumed_in_time_is_reset_at_both_ends() {
    let world = world(Duration::from_secs(2)).await;
    let (mut near, mut far) = world.tunnel().await;
    near.write_all(b"before").await.unwrap();
    let mut got = [0u8; 6];
    far.read_exact(&mut got).await.unwrap();
    // The gateway is not reachable for longer than the grace.
    world.hub.faults().refuse_connections(10_000);
    world.hub.faults().sever_now();
    let (near_result, far_result) = tokio::time::timeout(Duration::from_secs(15), async {
        let mut a = [0u8; 1];
        let mut b = [0u8; 1];
        tokio::join!(near.read(&mut a), far.read(&mut b))
    })
    .await
    .expect("the sockets were never closed");
    let kind = |result: std::io::Result<usize>| result.unwrap_err().kind();
    assert_eq!(kind(near_result), std::io::ErrorKind::ConnectionReset);
    assert_eq!(kind(far_result), std::io::ErrorKind::ConnectionReset);
    assert!(near.is_closed() && far.is_closed());
}

#[tokio::test]
async fn data_written_while_the_session_is_down_waits_in_the_replay_buffer_and_arrives_after() {
    let world = world(Duration::from_secs(60)).await;
    let (mut near, mut far) = world.tunnel().await;
    world.hub.faults().sever_now();
    tokio::time::sleep(Duration::from_millis(100)).await;
    assert!(near.is_suspended() || !near.is_closed());
    near.write_all(b"written while away").await.unwrap();
    let mut got = vec![0u8; 18];
    tokio::time::timeout(Duration::from_secs(20), far.read_exact(&mut got))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(&got, b"written while away");
}

impl World {
    fn terminal_ticket(&self, terminal: Id) -> String {
        self.signer
            .issue(&TicketRequest {
                src: TicketSource::Api,
                dst: self.destination_id,
                service: Service::Pty {
                    terminal_id: terminal,
                },
                thread_id: None,
                tool_set: None,
                expires_at: Time::new(self.clock.now() + chrono::Duration::hours(1)),
            })
            .unwrap()
    }

    /// Opens a terminal from the system to the destination machine.
    async fn terminal(&self) -> (Tunnel, Tunnel) {
        let mut incoming = self.incoming.lock().await;
        let ticket = self.terminal_ticket(new_id());
        let terminals = self.hub.terminals();
        let (opened, accepted) =
            tokio::join!(terminals.open(self.destination_id, &ticket), async {
                incoming.next().await.unwrap().accept()
            });
        (opened.unwrap(), accepted.unwrap())
    }
}

#[tokio::test]
async fn a_terminal_ends_at_the_system_and_carries_bytes_its_size_and_the_end_of_its_process() {
    let world = world(Duration::from_secs(60)).await;
    let (mut api, mut shell) = world.terminal().await;
    let mut api_events = api.take_events().unwrap();
    let mut shell_events = shell.take_events().unwrap();
    api.write_all(b"ls\n").await.unwrap();
    let mut typed = [0u8; 3];
    shell.read_exact(&mut typed).await.unwrap();
    assert_eq!(&typed, b"ls\n");
    shell.write_all(b"file.txt\n").await.unwrap();
    let mut shown = [0u8; 9];
    api.read_exact(&mut shown).await.unwrap();
    assert_eq!(&shown, b"file.txt\n");
    api.resize(120, 40);
    assert_eq!(
        tokio::time::timeout(Duration::from_secs(5), shell_events.recv())
            .await
            .unwrap(),
        Some(orch_link::tunnels::channel::TunnelEvent::Resize {
            cols: 120,
            rows: 40
        })
    );
    shell.exited(3);
    assert_eq!(
        tokio::time::timeout(Duration::from_secs(5), api_events.recv())
            .await
            .unwrap(),
        Some(orch_link::tunnels::channel::TunnelEvent::Exit { code: 3 })
    );
    shell.close();
    let mut rest = Vec::new();
    tokio::time::timeout(Duration::from_secs(5), api.read_to_end(&mut rest))
        .await
        .unwrap()
        .unwrap();
    assert!(rest.is_empty());
}

#[tokio::test]
async fn a_terminal_is_not_resumed_when_a_session_is_lost_the_browser_attaches_again() {
    let world = world(Duration::from_secs(60)).await;
    let (mut api, mut shell) = world.terminal().await;
    world.hub.faults().sever_now();
    let (mut api_byte, mut shell_byte) = ([0u8; 1], [0u8; 1]);
    let (api_result, shell_result) = tokio::time::timeout(Duration::from_secs(10), async {
        tokio::join!(api.read(&mut api_byte), shell.read(&mut shell_byte))
    })
    .await
    .expect("a terminal waits for nothing: its ends are reset at once");
    assert_eq!(
        api_result.unwrap_err().kind(),
        std::io::ErrorKind::ConnectionReset
    );
    assert_eq!(
        shell_result.unwrap_err().kind(),
        std::io::ErrorKind::ConnectionReset
    );
    // And a new attachment works once the machine is back.
    tokio::time::sleep(Duration::from_millis(2_500)).await;
    world.destination.link.connected().await;
    let (mut again, mut shell_again) = world.terminal().await;
    again.write_all(b"x").await.unwrap();
    let mut got = [0u8; 1];
    shell_again.read_exact(&mut got).await.unwrap();
    assert_eq!(&got, b"x");
}
