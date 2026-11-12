//! A resumed tunnel remains bound to the authority that opened it.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]

use chrono::TimeZone;
use orch_link::machine::LinkStatus;
use orch_link_contract::frames::Frame;
use orch_link_contract::session::Role;
use orch_link_contract::tunnels::{Resume, Service, TicketSource};
use orch_pki::tickets::{TicketRequest, TicketSigner};
use orch_testkit::doubles::controller::ScriptedController;
use orch_testkit::doubles::link::{Hub, TestPki};
use orch_testkit::doubles::machine::{MachineSpec, ScriptedMachine};
use orch_types::{Clock, ManualClock, Time, new_id};
use std::sync::Arc;
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

#[tokio::test]
async fn a_different_tool_ticket_cannot_take_over_an_existing_tunnel() {
    let clock: Arc<dyn Clock> = Arc::new(ManualClock::new(
        chrono::Utc
            .with_ymd_and_hms(2026, 10, 8, 14, 0, 0)
            .single()
            .unwrap(),
    ));
    let pki = TestPki::new(Arc::clone(&clock));
    let hub = Hub::new(Arc::clone(&clock), pki.certificate());
    let signer = TicketSigner::generate();
    hub.set_ticket_verifier(signer.verifier());
    let (source, destination, thread) = (new_id(), new_id(), new_id());
    let _controllers = [
        ScriptedController::attach(&hub, Arc::clone(&clock), source, vec![Role::Agent]),
        ScriptedController::attach(
            &hub,
            Arc::clone(&clock),
            destination,
            vec![Role::Environment],
        ),
    ];
    let near = ScriptedMachine::start(
        &hub,
        &clock,
        &pki.machine(source),
        &MachineSpec {
            roles: vec![Role::Agent],
            ..MachineSpec::default()
        },
    );
    let far = ScriptedMachine::start(
        &hub,
        &clock,
        &pki.machine(destination),
        &MachineSpec {
            roles: vec![Role::Environment],
            ..MachineSpec::default()
        },
    );
    let patience = Duration::from_secs(30);
    tokio::time::timeout(patience, async {
        near.link.connected().await;
        far.link.connected().await;
    })
    .await
    .unwrap();
    let request = TicketRequest {
        src: TicketSource::Machine(source),
        dst: destination,
        service: Service::Mcp {
            server: "files".into(),
        },
        thread_id: Some(thread),
        tool_set: None,
        expires_at: Time::new(clock.now() + chrono::Duration::hours(1)),
    };
    let first = signer.issue_for_tool(&request, new_id(), 0).unwrap();
    let other = signer.issue_for_tool(&request, new_id(), 0).unwrap();
    let mut incoming = far.link.incoming_tunnels();
    let (opened, accepted) = tokio::join!(near.link.open_tunnel(&first), async {
        incoming.next().await.unwrap().accept()
    });
    let (mut original, mut target) = (opened.unwrap(), accepted.unwrap());
    let LinkStatus::Connected { session_id, .. } = near.link.status().borrow().clone() else {
        panic!("the opener must be connected");
    };
    hub.with_relay(|relay| {
        relay.on_frame(
            source,
            session_id,
            Frame::Resume(Resume {
                epoch: 0,
                recovering: false,
                host_peer: None,
                channel: u32::MAX - 2,
                ticket: other,
                jti_channel: original.cid(),
                recv: 0,
            }),
        )
    });
    original.write_all(b"still mine").await.unwrap();
    let mut received = [0; 10];
    tokio::time::timeout(patience, target.read_exact(&mut received))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(&received, b"still mine");
    near.link.shutdown().await;
    far.link.shutdown().await;
}

#[cfg(feature = "test-observers")]
#[tokio::test]
async fn a_new_relay_map_requires_the_exact_retained_ticket_before_resuming_a_socket() {
    let clock: Arc<dyn Clock> = Arc::new(ManualClock::new(
        "2026-11-08T00:00:00Z".parse::<Time>().unwrap().get(),
    ));
    let pki = TestPki::new(clock.clone());
    let hub = Hub::new(clock.clone(), pki.certificate());
    let signer = TicketSigner::generate();
    hub.set_ticket_verifier(signer.verifier());
    let (source, destination, thread) = (new_id(), new_id(), new_id());
    let _controllers = [
        ScriptedController::attach(&hub, clock.clone(), source, vec![Role::Agent]),
        ScriptedController::attach(&hub, clock.clone(), destination, vec![Role::Environment]),
    ];
    let near = ScriptedMachine::start(
        &hub,
        &clock,
        &pki.machine(source),
        &MachineSpec {
            roles: vec![Role::Agent],
            ..MachineSpec::default()
        },
    );
    let far = ScriptedMachine::start(
        &hub,
        &clock,
        &pki.machine(destination),
        &MachineSpec {
            roles: vec![Role::Environment],
            ..MachineSpec::default()
        },
    );
    let patience = Duration::from_secs(30);
    tokio::time::timeout(patience, async {
        near.link.connected().await;
        far.link.connected().await;
    })
    .await
    .unwrap();
    let request = TicketRequest {
        src: TicketSource::Machine(source),
        dst: destination,
        service: Service::Mcp {
            server: "files".into(),
        },
        thread_id: Some(thread),
        tool_set: None,
        expires_at: Time::new(clock.now() + chrono::Duration::hours(1)),
    };
    let first = signer.issue_for_tool(&request, new_id(), 0).unwrap();
    let other = signer.issue_for_tool(&request, new_id(), 0).unwrap();
    let mut incoming = far.link.incoming_tunnels();
    let (opened, accepted) = tokio::join!(near.link.open_tunnel(&first), async {
        incoming.next().await.unwrap().accept()
    });
    let (mut original, mut target) = (opened.unwrap(), accepted.unwrap());
    let LinkStatus::Connected { session_id, .. } = near.link.status().borrow().clone() else {
        panic!("source must be connected");
    };
    // Model loss of only the relay's volatile route memory, retaining the actual native endpoints.
    hub.with_relay(|relay| relay.lose_route_memory_for_test());
    let claims = orch_link_contract::tunnels::Ticket::parse(&first)
        .unwrap()
        .claims()
        .clone();
    let mut expired = claims.clone();
    expired.exp = clock.time().unix_millis().div_euclid(1000) - 1;
    let mut foreign = claims;
    foreign.src = TicketSource::Machine(new_id());
    let refused = [
        (other, original.cid()),
        (signer.sign(&expired).unwrap(), original.cid()),
        (signer.sign(&foreign).unwrap(), original.cid()),
        (first.clone(), new_id()),
    ];
    for (index, (ticket, cid)) in refused.into_iter().enumerate() {
        let channel = u32::MAX - 2 * (u32::try_from(index).unwrap() + 1);
        hub.with_relay(|relay| {
            relay.on_frame(
                source,
                session_id,
                Frame::Resume(Resume {
                    epoch: 0,
                    recovering: false,
                    host_peer: None,
                    channel,
                    ticket,
                    jti_channel: cid,
                    recv: 0,
                }),
            )
        });
        let mut poll = tokio::time::interval(Duration::from_millis(10));
        tokio::time::timeout(patience, async {
            while hub.with_relay(|relay| relay.channels_of(source)) != 0 {
                poll.tick().await;
            }
        })
        .await
        .unwrap();
    }
    target.write_all(b"still retained").await.unwrap();
    // Reconnect the real opener; its own ordinary resume carries the correct original ticket.
    near.link
        .select_gateway(orch_link_contract::gateways::GatewayChoice::default());
    let mut reply = [0; 14];
    tokio::time::timeout(patience, original.read_exact(&mut reply))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(&reply, b"still retained");
    assert_eq!(original.cid(), target.cid());
    assert!(
        tokio::time::timeout(Duration::ZERO, incoming.next())
            .await
            .is_err()
    );
    near.link.shutdown().await;
    far.link.shutdown().await;
}
