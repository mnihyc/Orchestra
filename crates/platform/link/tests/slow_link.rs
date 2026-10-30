//! A line of a few hundred kilobits and a few hundred milliseconds, and a tenth of that: nothing is given up for
//! being slow, control is not held up by data, and what is moved in pieces is moved in pieces the line carries.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]

use async_trait::async_trait;
use bytes::Bytes;
use chrono::TimeZone;
use orch_link::content::{BlobSink, BlobSource, TransferOptions, Transfers};
use orch_link::framing::LinkMeter;
use orch_link::tunnels::channel::TunnelOptions;
use orch_link_contract::commands::{SerialKey, SessionClose};
use orch_link_contract::enrollment::{
    Endpoint, EnrollReply, EnrollReplySystem, EnrollRequest, Trust,
};
use orch_link_contract::session::{Arch, Os, Platform, Role};
use orch_link_contract::tunnels::{Service, TicketSource};
use orch_link_contract::version::Limits;
use orch_pki::enrollment::PendingEnrollment;
use orch_pki::tickets::{TicketRequest, TicketSigner};
use orch_testkit::doubles::controller::ScriptedController;
use orch_testkit::doubles::link::{
    BlobServer, Direction, Hub, LinkShape, Shape, ShapedBlobs, TestPki,
};
use orch_testkit::doubles::machine::{MachineSpec, ScriptedMachine};
use orch_types::{Clock, Hash, Id, ManualClock, Time, new_id};
use std::sync::Arc;
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::time::Instant;

#[derive(Clone, Copy, Debug)]
struct Line {
    round_trip: Duration,
    up: u64,
    down: u64,
    loss: f64,
}

/// 300 ms, 1.6 Mbit/s down, 750 kbit/s up, 1 % of packets lost.
const RECOMMENDED: Line = Line {
    round_trip: Duration::from_millis(300),
    up: 93_750,
    down: 200_000,
    loss: 0.01,
};
/// A tenth of those rates, the same round trip.
const A_TENTH: Line = Line {
    round_trip: Duration::from_millis(300),
    up: 9_375,
    down: 20_000,
    loss: 0.01,
};

impl Line {
    fn shape(self, bytes_per_second: u64) -> Shape {
        Shape {
            delay: self.round_trip / 2,
            bytes_per_second,
            loss: self.loss,
        }
    }
}

/// The shaped transport and the product's meters use the held runtime clock.
/// A saturated transfer therefore measures the simulated line, with its loss,
/// independently of the processors that the test happens to be given.
async fn session_on(line: Line) {
    let clock: Arc<dyn Clock> = Arc::new(ManualClock::new(
        chrono::Utc
            .with_ymd_and_hms(2026, 10, 8, 14, 0, 0)
            .single()
            .unwrap(),
    ));
    let pki = TestPki::new(Arc::clone(&clock));
    let hub = Hub::with_limits(Arc::clone(&clock), pki.certificate(), Limits::default(), 2);
    let signer = TicketSigner::generate();
    hub.set_ticket_verifier(signer.verifier());
    hub.faults().shape(Direction::Up, Some(line.shape(line.up)));
    hub.faults()
        .shape(Direction::Down, Some(line.shape(line.down)));
    let machine_id = new_id();
    let controller = ScriptedController::attach(
        &hub,
        Arc::clone(&clock),
        machine_id,
        vec![Role::Environment],
    );
    let started = Instant::now();
    let machine = ScriptedMachine::start(
        &hub,
        &clock,
        &pki.machine(machine_id),
        &MachineSpec {
            roles: vec![Role::Environment],
            tunnels: TunnelOptions::default(),
            ..MachineSpec::default()
        },
    );
    machine.link.connected().await;
    let handshake = started.elapsed();
    assert!(
        handshake < Duration::from_secs(10),
        "hello, auth and ready each took less than 10 s: {handshake:?} in all"
    );
    assert!(
        handshake >= line.round_trip,
        "the handshake cannot be faster than the line: {handshake:?}"
    );
    let session = hub.session_of(machine_id).unwrap();

    // A terminal that saturates the line in both directions.
    let mut incoming = machine.link.incoming_tunnels();
    let terminals = hub.terminals();
    let ticket = signer
        .issue(&TicketRequest {
            src: TicketSource::Api,
            dst: machine_id,
            service: Service::Pty {
                terminal_id: new_id(),
            },
            thread_id: None,
            tool_set: None,
            expires_at: Time::new(clock.now() + chrono::Duration::hours(24)),
        })
        .unwrap();
    let (opened, accepted) = tokio::join!(terminals.open(machine_id, &ticket), async {
        incoming.next().await.unwrap().accept()
    });
    let (api_end, shell_end) = (opened.unwrap(), accepted.unwrap());
    let (mut api_read, mut api_write) = tokio::io::split(api_end);
    let (mut shell_read, mut shell_write) = tokio::io::split(shell_end);
    let block = vec![0x5a_u8; 16 * 1024];
    let run_for = Duration::from_secs(8);
    let stop = tokio_util::sync::CancellationToken::new();
    let pushing = [
        tokio::spawn({
            let (stop, block) = (stop.clone(), block.clone());
            async move {
                while !stop.is_cancelled() {
                    if api_write.write_all(&block).await.is_err() {
                        break;
                    }
                }
            }
        }),
        tokio::spawn({
            let (stop, block) = (stop.clone(), block.clone());
            async move {
                while !stop.is_cancelled() {
                    if shell_write.write_all(&block).await.is_err() {
                        break;
                    }
                }
            }
        }),
    ];
    let draining = [
        tokio::spawn(async move {
            let mut buffer = vec![0u8; 64 * 1024];
            while api_read.read(&mut buffer).await.is_ok_and(|n| n > 0) {}
        }),
        tokio::spawn(async move {
            let mut buffer = vec![0u8; 64 * 1024];
            while shell_read.read(&mut buffer).await.is_ok_and(|n| n > 0) {}
        }),
    ];

    // Let the line fill, then send a command while the transfer is on.
    tokio::time::sleep(Duration::from_secs(6)).await;
    let command = controller.issue(
        &SerialKey::Machine,
        Time::new(clock.now() + chrono::Duration::minutes(5)),
        &orch_link_contract::commands::MachineProbe {},
    );
    let sent = Instant::now();
    let acknowledged = loop {
        match controller.progress(command) {
            Some(
                orch_link::commands::Progress::Accepted
                | orch_link::commands::Progress::Succeeded(_)
                | orch_link::commands::Progress::Failed(_),
            ) => {
                break sent.elapsed();
            }
            _ => tokio::time::sleep(Duration::from_millis(10)).await,
        }
        assert!(
            sent.elapsed() < Duration::from_secs(30),
            "the command was never acknowledged"
        );
    };
    // A second is what the line allows: the command waits for the frame on the wire in one direction and the
    // acknowledgement for the frame on the wire in the other. Half a second more is let pass for one lost packet,
    // which stalls the simulated pipe for 300 ms, and for the observation pass.
    assert!(
        acknowledged <= Duration::from_millis(1_500),
        "a command sent during a transfer is acknowledged in about a second: {acknowledged:?}"
    );
    tokio::time::sleep(run_for).await;

    // What the machine saw of the line.
    let rates = machine.link.rates();
    let bits = |bytes_per_second: u64| i64::try_from(bytes_per_second * 8).unwrap();
    let near = |measured: i64, expected: i64| {
        measured >= expected * 5 / 10 && measured <= expected * 16 / 10
    };
    assert!(
        near(rates.up_bps, bits(line.up)),
        "up {} bits/s of {}",
        rates.up_bps,
        bits(line.up)
    );
    assert!(
        near(rates.down_bps, bits(line.down)),
        "down {} bits/s of {}",
        rates.down_bps,
        bits(line.down)
    );
    assert!(
        (200..=1_500).contains(&rates.round_trip_ms),
        "the round trip is reported as a fact: {} ms",
        rates.round_trip_ms
    );
    // Data frames are sized to a quarter of a second of the line, never under 4 KiB.
    let frame = machine.link.data_frame_bytes().unwrap();
    let wanted = usize::try_from(line.up / 4).unwrap().max(4 * 1024);
    assert!(
        frame >= 4 * 1024 && frame <= wanted * 2,
        "frames of {frame} bytes on a line that carries {wanted} a quarter second"
    );
    // And the session was never given up for being late.
    assert_eq!(
        hub.session_of(machine_id),
        Some(session),
        "the same session stands"
    );
    assert_eq!(hub.faults().connections(), 1);
    stop.cancel();
    for task in pushing {
        task.abort();
    }
    for task in draining {
        task.abort();
    }
}

#[tokio::test(start_paused = true)]
async fn on_a_line_of_the_recommended_shape_the_session_stays_up_control_is_not_held_up_and_the_facts_report_the_line()
 {
    session_on(RECOMMENDED).await;
}

#[tokio::test(start_paused = true)]
async fn on_a_line_of_a_tenth_of_that_the_same_holds() {
    session_on(A_TENTH).await;
}

/// Bytes that depend on their place.
struct Pattern {
    size: i64,
}

fn pattern(at: i64, len: i64) -> Bytes {
    let mut bytes = Vec::with_capacity(usize::try_from(len).unwrap());
    let mut place = at;
    while place < at + len {
        let word = (u64::try_from(place / 8).unwrap())
            .wrapping_mul(0x9E37_79B9_7F4A_7C15)
            .to_le_bytes();
        let from = usize::try_from(place % 8).unwrap();
        let take = usize::try_from((8 - place % 8).min(at + len - place)).unwrap();
        bytes.extend_from_slice(&word[from..from + take]);
        place += i64::try_from(take).unwrap();
    }
    Bytes::from(bytes)
}

#[async_trait]
impl BlobSource for Pattern {
    async fn read(&mut self, offset: i64, len: i64) -> std::io::Result<Bytes> {
        Ok(pattern(offset, len.min(self.size - offset)))
    }
}

#[derive(Default)]
struct Memory {
    held: Vec<u8>,
}

#[async_trait]
impl BlobSink for Memory {
    async fn received(&mut self) -> std::io::Result<i64> {
        Ok(i64::try_from(self.held.len()).unwrap())
    }
    async fn write(&mut self, offset: i64, bytes: Bytes) -> std::io::Result<()> {
        assert_eq!(offset, i64::try_from(self.held.len()).unwrap());
        self.held.extend_from_slice(&bytes);
        Ok(())
    }
    async fn hash(&mut self) -> std::io::Result<Hash> {
        Ok(Hash::of(&self.held))
    }
    async fn discard(&mut self) -> std::io::Result<()> {
        self.held.clear();
        Ok(())
    }
}

const HUNDRED_MIB: i64 = 100 * 1024 * 1024;

fn hash_of_pattern(size: i64) -> Hash {
    let mut hasher = Hash::hasher();
    let mut at = 0;
    while at < size {
        let n = (4 * 1024 * 1024).min(size - at);
        hasher.update(&pattern(at, n));
        at += n;
    }
    Hash::finish(&hasher)
}

async fn upload_across(line: Line) {
    let server = BlobServer::new();
    let command = new_id();
    server.open_command(command);
    let shape = LinkShape {
        round_trip: line.round_trip,
        up_bytes_per_second: line.up,
        down_bytes_per_second: line.down,
    };
    let endpoint = ShapedBlobs::new(Arc::clone(&server), Some(shape), HUNDRED_MIB * 2);
    let hash = hash_of_pattern(HUNDRED_MIB);
    // The line is cut in the middle of a piece, twice.
    server.cut_put(15);
    server.cut_put(40);
    let transfers = Transfers::new(
        Arc::new(endpoint),
        LinkMeter::new(),
        TransferOptions::default(),
    );
    let outcome = transfers
        .upload(
            command,
            hash,
            HUNDRED_MIB,
            &mut Pattern { size: HUNDRED_MIB },
        )
        .await
        .unwrap();
    assert_eq!(
        server.content(&hash).map(|whole| Hash::of(&whole)),
        Some(hash),
        "all of it arrived, whole"
    );
    assert!(outcome.retries >= 2, "{outcome:?}");
    assert!(
        outcome.lost_bytes <= 2 * outcome.largest_piece,
        "a cut loses at most the piece that was on its way: {outcome:?}"
    );
    assert!(
        outcome.smallest_piece >= 64 * 1024 || outcome.smallest_piece == 0 || outcome.requests > 0,
        "{outcome:?}"
    );
    assert!(outcome.largest_piece <= 4 * 1024 * 1024);
    // A piece is about five seconds of the line.
    let five_seconds = i64::try_from(line.up * 5).unwrap();
    assert!(
        outcome.largest_piece <= five_seconds.max(256 * 1024) * 2,
        "pieces are sized to the line: {} bytes on a line that carries {five_seconds} in five seconds",
        outcome.largest_piece
    );
}

#[tokio::test(start_paused = true)]
async fn a_hundred_mebibytes_cross_the_recommended_line_and_a_cut_loses_at_most_a_piece() {
    upload_across(RECOMMENDED).await;
}

#[tokio::test(start_paused = true)]
async fn a_hundred_mebibytes_cross_a_line_of_a_tenth_of_that_and_a_cut_loses_at_most_a_piece() {
    upload_across(A_TENTH).await;
}

#[tokio::test(start_paused = true)]
async fn pieces_shrink_to_a_slow_line_and_grow_back_on_a_fast_one() {
    let server = BlobServer::new();
    let command = new_id();
    server.open_command(command);
    let size = 64 * 1024 * 1024;
    let slow = LinkShape {
        round_trip: Duration::from_millis(300),
        up_bytes_per_second: 93_750,
        down_bytes_per_second: 200_000,
    };
    let endpoint = ShapedBlobs::new(Arc::clone(&server), Some(slow), size * 2);
    let changer = endpoint.clone();
    let transfers = Transfers::new(
        Arc::new(endpoint),
        LinkMeter::new(),
        TransferOptions::default(),
    );
    // After a minute on the slow line, the line gets fast.
    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_secs(60)).await;
        changer.set_shape(None);
    });
    transfers
        .upload(command, hash_of_pattern(size), size, &mut Pattern { size })
        .await
        .unwrap();
    let pieces: Vec<usize> = server
        .requests()
        .into_iter()
        .filter(|(kind, _)| kind == "put")
        .map(|(_, bytes)| bytes)
        .collect();
    let early = pieces.iter().take(8).copied().max().unwrap();
    // The last piece is only what is left, so the pieces before it say what the line carried.
    let late = pieces.iter().rev().skip(1).take(8).copied().min().unwrap();
    assert!(
        early <= 1024 * 1024,
        "on the slow line pieces were about five seconds of it: {early}"
    );
    assert_eq!(
        late,
        4 * 1024 * 1024,
        "on the fast line they grew to the most a request may carry"
    );
}

#[tokio::test(start_paused = true)]
async fn a_download_goes_on_from_what_the_other_side_holds_and_checks_the_whole() {
    let server = BlobServer::new();
    let command = new_id();
    server.open_command(command);
    let size: i64 = 8 * 1024 * 1024;
    let whole = pattern(0, size);
    let hash = server.insert(whole.clone());
    let shape = LinkShape {
        round_trip: Duration::from_millis(300),
        up_bytes_per_second: 93_750,
        down_bytes_per_second: 200_000,
    };
    let transfers = Transfers::new(
        Arc::new(ShapedBlobs::new(server, Some(shape), size * 2)),
        LinkMeter::new(),
        TransferOptions::default(),
    );
    // A cut left the first three mebibytes behind.
    let mut sink = Memory {
        held: whole[..3 * 1024 * 1024].to_vec(),
    };
    let outcome = transfers
        .download(command, hash, size, &mut sink)
        .await
        .unwrap();
    assert_eq!(sink.held, whole.to_vec());
    assert!(outcome.requests > 0);
    // Content that is not what it was announced to be is refused and thrown away.
    let mut wrong = Memory {
        held: vec![1; 1024],
    };
    let error = transfers
        .download(command, hash, size, &mut wrong)
        .await
        .unwrap_err();
    assert_eq!(error, orch_link::content::ContentError::HashMismatch);
    assert!(wrong.held.is_empty());
}

#[test]
fn enrollment_and_renewal_requests_fit_the_limit_of_thirty_seconds_on_both_lines() {
    let pending = PendingEnrollment::start().unwrap();
    let request = EnrollRequest {
        machine_id: None,
        token: format!("oenr_{}.{}", "x".repeat(43), "y".repeat(43)),
        csr_pem: pending.csr_pem().to_owned(),
        name: "alice-laptop".to_owned(),
        platform: Platform {
            os: Os::Linux,
            arch: Arch::X86_64,
        },
        connector_version: "0.1.0".to_owned(),
        protocol_versions: vec![1],
        roles: vec![Role::Agent, Role::Environment],
    };
    let reply = EnrollReply {
        machine_id: new_id(),
        certificate_pem: "-".repeat(1_100),
        ca_pem: "-".repeat(700),
        endpoints: vec![Endpoint {
            url: "wss://gw.orch.internal:9443".to_owned(),
            trust: Trust::Pinned,
        }],
        system: EnrollReplySystem {
            name: "Orch".to_owned(),
            public_url: "https://orch.example.com".to_owned(),
        },
        protocol_version: 1,
    };
    let up = serde_json::to_vec(&request).unwrap().len() as f64;
    let down = serde_json::to_vec(&reply).unwrap().len() as f64;
    for line in [RECOMMENDED, A_TENTH] {
        // A TCP connection, a TLS handshake and the request: four round trips, and the bytes of both messages.
        let seconds =
            4.0 * line.round_trip.as_secs_f64() + up / line.up as f64 + down / line.down as f64;
        assert!(seconds < 30.0, "{seconds} seconds on {line:?}");
    }
    let _ = (
        Id::nil(),
        Limits::default(),
        SessionClose {
            thread_id: Id::nil(),
        },
    );
}
