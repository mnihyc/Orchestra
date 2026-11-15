//! The live control question is serialized with revocation and keeps unrelated tool members usable.
use super::*;
use orch_link_contract::backplane::BackplaneFrame;
use orch_link_contract::tool_sessions::{ToolSessionMember, ToolSessionReply, ToolSessionUpdate};
use orch_link_contract::tunnels::ToolSessionScope;
use orch_types::Id;

struct Rig {
    clock: Arc<dyn Clock>,
    hub: Hub,
    signer: TicketSigner,
    near: ScriptedMachine,
    far: ScriptedMachine,
    source: Id,
    destination: Id,
    scope: ToolSessionScope,
    controllers: Vec<ScriptedController>,
}

impl Rig {
    async fn new() -> Self {
        let clock: Arc<dyn Clock> = Arc::new(ManualClock::new(
            "2026-11-08T00:00:00Z".parse::<Time>().unwrap().get(),
        ));
        let pki = TestPki::new(clock.clone());
        let hub = Hub::new(clock.clone(), pki.certificate());
        let signer = TicketSigner::generate();
        hub.set_ticket_verifier(signer.verifier());
        hub.set_tool_authority(|query| async move {
            ToolSessionReply {
                n: query.n,
                allowed: true,
                revision: query.claims.tool_session.as_ref().unwrap().revision,
            }
        });
        let source = new_id();
        let destination = new_id();
        let controllers = vec![
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
        tokio::time::timeout(Duration::from_secs(30), async {
            near.link.connected().await;
            far.link.connected().await;
        })
        .await
        .unwrap();
        let scope = ToolSessionScope {
            thread_id: new_id(),
            segment_id: new_id(),
            opening_id: new_id(),
            member: format!("mcp:{}", new_id()),
            nonce: new_id(),
            revision: 1,
        };
        Self {
            clock,
            hub,
            signer,
            near,
            far,
            source,
            destination,
            scope,
            controllers,
        }
    }

    fn ticket(&self, scope: &ToolSessionScope) -> String {
        self.signer
            .issue_for_session(
                &TicketRequest {
                    src: TicketSource::Machine(self.source),
                    dst: self.destination,
                    service: Service::Mcp {
                        server: "files".into(),
                    },
                    thread_id: Some(scope.thread_id),
                    tool_set: None,
                    expires_at: Time::new(self.clock.now() + chrono::Duration::hours(1)),
                },
                Some((new_id(), 0)),
                None,
                scope,
            )
            .unwrap()
    }

    fn update(&self, live: bool, revision: i64, members: Vec<ToolSessionMember>) {
        self.hub.with_relay(|relay| {
            relay.on_api_frame(BackplaneFrame::ToolSessionUpdate(ToolSessionUpdate {
                thread_id: self.scope.thread_id,
                segment_id: self.scope.segment_id,
                opening_id: Some(self.scope.opening_id),
                revision,
                live,
                members,
            }))
        });
    }

    async fn stop(self) {
        self.near.link.shutdown().await;
        self.far.link.shutdown().await;
        drop(self.controllers);
    }
}

#[tokio::test]
async fn a_live_tool_opens_but_current_control_loss_refuses_its_next_open_and_resume() {
    let rig = Rig::new().await;
    let ticket = rig.ticket(&rig.scope);
    let mut incoming = rig.far.link.incoming_tunnels();
    let (opened, accepted) = tokio::join!(rig.near.link.open_tunnel(&ticket), async {
        incoming.next().await.unwrap().accept()
    });
    let (mut original, _target) = (opened.unwrap(), accepted.unwrap());
    rig.hub.set_api_down(true);
    assert!(
        tokio::time::timeout(Duration::from_secs(30), rig.near.link.open_tunnel(&ticket))
            .await
            .unwrap()
            .is_err()
    );
    assert_eq!(
        tokio::time::timeout(Duration::from_secs(30), original.read_u8())
            .await
            .unwrap()
            .unwrap_err()
            .kind(),
        std::io::ErrorKind::ConnectionReset
    );
    assert_eq!(rig.hub.with_relay(|relay| relay.channels_of(rig.source)), 0);
    rig.stop().await;
}

#[tokio::test]
async fn a_held_positive_answer_cannot_reopen_a_session_that_closed_while_it_was_asked() {
    let rig = Rig::new().await;
    let entered = Arc::new(tokio::sync::Notify::new());
    let release = Arc::new(tokio::sync::Notify::new());
    let (noticed, gate) = (entered.clone(), release.clone());
    rig.hub.set_tool_authority(move |query| {
        let noticed = noticed.clone();
        let gate = gate.clone();
        async move {
            noticed.notify_one();
            gate.notified().await;
            ToolSessionReply {
                n: query.n,
                allowed: true,
                revision: 1,
            }
        }
    });
    let ticket = rig.ticket(&rig.scope);
    {
        let opening = rig.near.link.open_tunnel(&ticket);
        tokio::pin!(opening);
        tokio::select! { _=entered.notified()=>{},result=&mut opening=>panic!("the real pending API question must be observed before completion: {result:?}") }
        rig.update(false, 2, Vec::new());
        release.notify_one();
        assert!(
            tokio::time::timeout(Duration::from_secs(30), &mut opening)
                .await
                .unwrap()
                .is_err()
        );
    }
    assert_eq!(rig.hub.with_relay(|relay| relay.channels_of(rig.source)), 0);
    rig.stop().await;
}

#[tokio::test]
async fn withdrawing_one_tool_member_closes_it_and_keeps_the_other_members_socket_usable() {
    let rig = Rig::new().await;
    let mut b = rig.scope.clone();
    b.member = format!("mcp:{}", new_id());
    b.nonce = new_id();
    let (ticket_a, ticket_b) = (rig.ticket(&rig.scope), rig.ticket(&b));
    let mut incoming = rig.far.link.incoming_tunnels();
    let (a, far_a) = tokio::join!(rig.near.link.open_tunnel(&ticket_a), async {
        incoming.next().await.unwrap().accept()
    });
    let (b_open, far_b) = tokio::join!(rig.near.link.open_tunnel(&ticket_b), async {
        incoming.next().await.unwrap().accept()
    });
    let (mut a, _far_a, mut b_open, mut far_b) =
        (a.unwrap(), far_a.unwrap(), b_open.unwrap(), far_b.unwrap());
    rig.update(
        true,
        2,
        vec![ToolSessionMember {
            member: b.member,
            nonce: b.nonce,
        }],
    );
    assert!(
        tokio::time::timeout(Duration::from_secs(30), a.read_u8())
            .await
            .unwrap()
            .is_err()
    );
    b_open.write_all(b"kept").await.unwrap();
    let mut bytes = [0; 4];
    tokio::time::timeout(Duration::from_secs(30), far_b.read_exact(&mut bytes))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(&bytes, b"kept");
    rig.stop().await;
}

#[tokio::test]
async fn a_resumed_tool_socket_asks_the_current_api_again_before_replaying_its_bytes() {
    use std::sync::atomic::{AtomicUsize, Ordering};
    let rig = Rig::new().await;
    let questions = Arc::new(AtomicUsize::new(0));
    let observed = questions.clone();
    rig.hub.set_tool_authority(move |query| {
        observed.fetch_add(1, Ordering::SeqCst);
        async move {
            ToolSessionReply {
                n: query.n,
                allowed: true,
                revision: query.claims.tool_session.as_ref().unwrap().revision,
            }
        }
    });
    let ticket = rig.ticket(&rig.scope);
    let mut incoming = rig.far.link.incoming_tunnels();
    let (opened, accepted) = tokio::join!(rig.near.link.open_tunnel(&ticket), async {
        incoming.next().await.unwrap().accept()
    });
    let (mut near, mut far) = (opened.unwrap(), accepted.unwrap());
    let before = questions.load(Ordering::SeqCst);
    assert_eq!(before, 1);
    let old_session = rig.hub.session_of(rig.source).unwrap();
    let mut status = rig.near.link.status();
    rig.hub.disconnect(rig.source);
    tokio::time::timeout(Duration::from_secs(30),async {
        loop {
            if matches!(*status.borrow_and_update(),LinkStatus::Connected {session_id,..} if session_id!=old_session) {break;}
            status.changed().await.unwrap();
        }
    }).await.unwrap();
    near.write_all(b"resumed").await.unwrap();
    let mut bytes = [0; 7];
    tokio::time::timeout(Duration::from_secs(30), far.read_exact(&mut bytes))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(&bytes, b"resumed");
    assert!(
        questions.load(Ordering::SeqCst) > before,
        "ordinary Resume must make another current admission question"
    );
    rig.stop().await;
}

#[tokio::test]
async fn a_tool_question_that_times_out_cannot_be_opened_by_its_later_positive_answer() {
    use orch_link_contract::tool_sessions::ToolSessionControl;
    let rig = Rig::new().await;
    let entered = Arc::new(tokio::sync::Notify::new());
    let release = Arc::new(tokio::sync::Notify::new());
    let (noticed, gate) = (entered.clone(), release.clone());
    rig.hub.set_tool_authority(move |query| {
        let (noticed, gate) = (noticed.clone(), gate.clone());
        async move {
            noticed.notify_one();
            gate.notified().await;
            ToolSessionReply {
                n: query.n,
                allowed: true,
                revision: 1,
            }
        }
    });
    rig.hub.with_relay(|relay| {
        relay.on_api_frame(BackplaneFrame::ToolSessionControl(ToolSessionControl {
            available: true,
            admission_timeout_ms: 1,
        }))
    });
    let ticket = rig.ticket(&rig.scope);
    {
        let opening = rig.near.link.open_tunnel(&ticket);
        tokio::pin!(opening);
        tokio::select! {_=entered.notified()=>{},answer=&mut opening=>panic!("the query must first reach its authority: {answer:?}")}
        rig.hub
            .with_relay(|relay| relay.tick(tokio::time::Instant::now() + Duration::from_millis(2)));
        assert!(
            tokio::time::timeout(Duration::from_secs(30), &mut opening)
                .await
                .unwrap()
                .is_err()
        );
        release.notify_one();
    }
    assert_eq!(rig.hub.with_relay(|relay| relay.channels_of(rig.source)), 0);
    rig.stop().await;
}
