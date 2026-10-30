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
