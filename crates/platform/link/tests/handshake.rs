//! The session handshake between a machine and a gateway, with the proof, the codes and the clocks.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]

use async_trait::async_trait;
use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use chrono::{Duration as Days, TimeZone};
use orch_link::framing::{Connection, Message, PeerInfo, memory_pair};
use orch_link::session::{
    AcceptError, ClientIdentity, GatewayConfig, HandshakeError, SessionAuthority, accept, connect,
};
use orch_link_contract::backplane::{AuthQuery, AuthReply};
use orch_link_contract::frames::{self, Frame, Text};
use orch_link_contract::session::{Arch, Auth, AuthProof, Os, Platform, Role, auth_signing_input};
use orch_link_contract::version::{ConnectorVersions, Limits, RejectCode};
use orch_pki::certificates::{Certificate, LeafKey};
use orch_testkit::doubles::link::{TestIdentity, TestPki};
use orch_types::{Clock, Id, ManualClock, Time, new_id};
use std::collections::{BTreeMap, HashMap};
use std::sync::{Arc, Mutex};
use std::time::Duration;

const HOST: &str = "orch.example.com";

struct Registry {
    known: Mutex<HashMap<Id, Known>>,
    silent: bool,
}

#[derive(Clone)]
struct Known {
    active: bool,
    fingerprint: String,
    update_idle: bool,
}

#[async_trait]
impl SessionAuthority for Registry {
    async fn authorize(&self, query: AuthQuery) -> AuthReply {
        if self.silent {
            std::future::pending::<()>().await;
        }
        let refuse = |code| AuthReply {
            endpoints: None,
            update_permit: None,
            n: query.n,
            ok: false,
            code: Some(code),
            cursors: None,
            placement: None,
        };
        let Some(known) = self.known.lock().unwrap().get(&query.machine_id).cloned() else {
            return refuse(RejectCode::UnknownMachine);
        };
        if !known.active || known.fingerprint != query.fingerprint {
            return refuse(RejectCode::Revoked);
        }
        AuthReply {
            endpoints: None,
            update_permit: known
                .update_idle
                .then(|| orch_link_contract::session::UpdatePermit {
                    recommended: "0.2.0".into(),
                }),
            n: query.n,
            ok: true,
            code: None,
            cursors: Some(BTreeMap::from([("machine".to_owned(), 12)])),
            placement: Some("connected".to_owned()),
        }
    }
}

struct Fixture {
    clock: ManualClock,
    pki: TestPki,
    gateway: GatewayConfig,
    registry: Arc<Registry>,
    identity: TestIdentity,
}

fn fixture(silent: bool) -> Fixture {
    let clock = ManualClock::new(
        chrono::Utc
            .with_ymd_and_hms(2026, 10, 8, 14, 0, 0)
            .single()
            .unwrap(),
    );
    let pki = TestPki::new(Arc::new(clock.clone()));
    let identity = pki.machine(new_id());
    let registry = Arc::new(Registry {
        known: Mutex::new(HashMap::from([(
            identity.machine_id,
            Known {
                active: true,
                update_idle: false,
                fingerprint: identity.certificate.fingerprint(),
            },
        )])),
        silent,
    });
    let gateway = GatewayConfig {
        gateway_id: "gw".to_owned(),
        limits: Limits::default(),
        versions: ConnectorVersions {
            min: "0.1.0".to_owned(),
            recommended: "0.2.0".to_owned(),
        },
        heartbeat_secs: 20,
        authority: pki.certificate(),
    };
    Fixture {
        clock,
        pki,
        gateway,
        registry,
        identity,
    }
}

impl Fixture {
    fn client(&self, version: &str, key: Option<LeafKey>) -> ClientIdentity {
        ClientIdentity {
            hosted_endpoints: None,
            endpoints: None,
            machine_id: self.identity.machine_id,
            connector_version: version.to_owned(),
            platform: Platform {
                os: Os::Linux,
                arch: Arch::X86_64,
            },
            roles: vec![Role::Agent],
            proof: Some((
                key.unwrap_or_else(|| self.identity.key.duplicate().unwrap()),
                self.identity.certificate.clone(),
            )),
        }
    }

    /// A connection pair: the gateway sees the machine as showing `shown` in TLS (mutual) or nothing (server), and
    /// the two have their own idea of the host.
    fn pair(
        &self,
        shown: Option<Certificate>,
        machine_host: &str,
        gateway_host: &str,
    ) -> (Connection, Connection) {
        memory_pair(
            PeerInfo {
                host: Some(machine_host.to_owned()),
                ..PeerInfo::default()
            },
            PeerInfo {
                remote_ip: Some("192.0.2.7".parse().unwrap()),
                host: Some(gateway_host.to_owned()),
                client_certificate: shown,
            },
        )
    }

    /// Runs both ends; the machine's clock is `machine_clock`.
    async fn run(
        &self,
        shown: Option<Certificate>,
        client: &ClientIdentity,
        machine_clock: &ManualClock,
        hosts: (&str, &str),
    ) -> (
        Result<orch_link::session::Accepted, AcceptError>,
        Result<orch_link::session::Established, HandshakeError>,
    ) {
        let (machine_side, gateway_side) = self.pair(shown, hosts.0, hosts.1);
        let gateway = self.gateway.clone();
        let registry = Arc::clone(&self.registry);
        let clock = self.clock.clone();
        let serving =
            tokio::spawn(
                async move { accept(gateway_side, &gateway, registry.as_ref(), &clock).await },
            );
        let connected = connect(machine_side, client, machine_clock).await;
        (serving.await.unwrap(), connected)
    }
}

fn rejected(result: Result<orch_link::session::Established, HandshakeError>) -> RejectCode {
    match result.unwrap_err() {
        HandshakeError::Rejected(reject) => reject.code,
        other => panic!("expected a refusal, got {other}"),
    }
}

#[tokio::test]
async fn a_machine_known_by_its_certificate_gets_a_session_with_the_cursors_of_its_streams() {
    let f = fixture(false);
    let client = f.client("0.1.0", None);
    let (served, connected) = f
        .run(
            Some(f.identity.certificate.clone()),
            &client,
            &f.clock,
            (HOST, HOST),
        )
        .await;
    let accepted = served.unwrap();
    let established = connected.unwrap();
    assert_eq!(accepted.session_id, established.ready.session_id);
    assert_eq!(established.ready.cursors["machine"], 12);
    assert_eq!(
        established.tls,
        orch_link_contract::session::TlsMode::Mutual
    );
    assert_eq!(established.ready.limits, Limits::default());
    assert_eq!(accepted.fingerprint, f.identity.certificate.fingerprint());
    assert_eq!(accepted.remote_ip, Some("192.0.2.7".parse().unwrap()));
}

#[tokio::test]
async fn a_machine_with_no_certificate_in_the_connection_proves_it_holds_the_key_of_its_certificate()
 {
    let f = fixture(false);
    let client = f.client("0.1.0", None);
    let (served, connected) = f.run(None, &client, &f.clock, (HOST, HOST)).await;
    served.unwrap();
    let established = connected.unwrap();
    assert_eq!(
        established.tls,
        orch_link_contract::session::TlsMode::Server
    );
}

#[tokio::test]
async fn a_proof_signed_with_another_key_is_refused_as_a_bad_proof() {
    let f = fixture(false);
    let client = f.client("0.1.0", Some(LeafKey::generate().unwrap()));
    let (_, connected) = f.run(None, &client, &f.clock, (HOST, HOST)).await;
    assert_eq!(rejected(connected), RejectCode::BadProof);
}

#[tokio::test]
async fn a_proof_signed_for_another_host_is_refused_as_a_bad_proof() {
    let f = fixture(false);
    let client = f.client("0.1.0", None);
    // The machine believes it connected to another host than the one the gateway answers as.
    let (_, connected) = f
        .run(None, &client, &f.clock, ("evil.example.com", HOST))
        .await;
    assert_eq!(rejected(connected), RejectCode::BadProof);
}

#[tokio::test]
async fn a_proof_with_a_clock_too_far_from_the_gateways_is_refused_as_clock_skew() {
    let f = fixture(false);
    let client = f.client("0.1.0", None);
    let ahead = ManualClock::new(f.clock.now() + Days::seconds(121));
    let (_, connected) = f.run(None, &client, &ahead, (HOST, HOST)).await;
    assert_eq!(rejected(connected), RejectCode::ClockSkew);
    let behind = ManualClock::new(f.clock.now() - Days::seconds(121));
    let (_, connected) = f.run(None, &client, &behind, (HOST, HOST)).await;
    assert_eq!(rejected(connected), RejectCode::ClockSkew);
    let slightly = ManualClock::new(f.clock.now() + Days::seconds(110));
    let (served, connected) = f.run(None, &client, &slightly, (HOST, HOST)).await;
    served.unwrap();
    connected.unwrap();
}

/// Reads the gateway's hello and answers with an auth the test made.
async fn answer_hello(
    connection: &mut Connection,
    make: impl FnOnce(&str) -> Auth,
) -> (String, String) {
    let Some(Message::Text(text)) = connection.source.recv().await.unwrap() else {
        panic!("no hello")
    };
    let Text::Known(Frame::Hello(hello)) = frames::decode::<Frame>(&text, 262_144).unwrap() else {
        panic!("not hello")
    };
    let auth = make(&hello.nonce);
    let text = frames::encode(&Frame::Auth(auth), 262_144).unwrap();
    connection
        .sink
        .send(Message::Text(text.clone()))
        .await
        .unwrap();
    (hello.nonce, text)
}

fn proof_auth(f: &Fixture, nonce: &str, sent_at: Time) -> Auth {
    let signed = auth_signing_input(nonce, f.identity.machine_id, HOST, sent_at);
    Auth {
        endpoints: None,
        hosted_endpoints: None,
        machine_id: f.identity.machine_id,
        protocol_version: 1,
        connector_version: "0.1.0".to_owned(),
        platform: Platform {
            os: Os::Linux,
            arch: Arch::X86_64,
        },
        roles: vec![Role::Agent],
        sent_at,
        proof: Some(AuthProof {
            certificate_pem: f.identity.certificate.pem(),
            signature: URL_SAFE_NO_PAD.encode(f.identity.key.sign(signed.as_bytes())),
        }),
    }
}

async fn reply_of(connection: &mut Connection) -> Frame {
    let Some(Message::Text(text)) = connection.source.recv().await.unwrap() else {
        panic!("no reply")
    };
    let Text::Known(frame) = frames::decode::<Frame>(&text, 262_144).unwrap() else {
        panic!("unknown reply")
    };
    frame
}

#[tokio::test]
async fn a_recorded_handshake_replayed_on_another_connection_is_refused_because_the_nonce_is_not_that_connections()
 {
    let f = fixture(false);
    // The first connection: a genuine handshake, recorded.
    let (mut machine, gateway_side) = f.pair(None, HOST, HOST);
    let (gateway, registry, clock) = (f.gateway.clone(), Arc::clone(&f.registry), f.clock.clone());
    let first =
        tokio::spawn(
            async move { accept(gateway_side, &gateway, registry.as_ref(), &clock).await },
        );
    let (_, recorded) =
        answer_hello(&mut machine, |nonce| proof_auth(&f, nonce, f.clock.time())).await;
    assert!(
        matches!(reply_of(&mut machine).await, Frame::Ready(_)),
        "the genuine handshake stands"
    );
    first.await.unwrap().unwrap();
    // The second connection: somebody who saw the first sends the same words.
    let (mut thief, gateway_side) = f.pair(None, HOST, HOST);
    let (gateway, registry, clock) = (f.gateway.clone(), Arc::clone(&f.registry), f.clock.clone());
    let second =
        tokio::spawn(
            async move { accept(gateway_side, &gateway, registry.as_ref(), &clock).await },
        );
    let Some(Message::Text(_hello)) = thief.source.recv().await.unwrap() else {
        panic!("no hello")
    };
    thief.sink.send(Message::Text(recorded)).await.unwrap();
    let Frame::Reject(reject) = reply_of(&mut thief).await else {
        panic!("not refused")
    };
    assert_eq!(reject.code, RejectCode::BadProof);
    assert!(matches!(
        second.await.unwrap(),
        Err(AcceptError::Refused {
            code: RejectCode::BadProof,
            ..
        })
    ));
}

#[tokio::test]
async fn the_system_decides_who_may_have_a_session_and_the_machine_is_told_why() {
    let f = fixture(false);
    let client = f.client("0.1.0", None);
    // A machine the system has never heard of.
    let stranger = f.pki.machine(new_id());
    let strange = ClientIdentity {
        hosted_endpoints: None,
        endpoints: None,
        machine_id: stranger.machine_id,
        proof: Some((
            stranger.key.duplicate().unwrap(),
            stranger.certificate.clone(),
        )),
        ..f.client("0.1.0", None)
    };
    let (_, connected) = f.run(None, &strange, &f.clock, (HOST, HOST)).await;
    assert_eq!(rejected(connected), RejectCode::UnknownMachine);
    // Revoked.
    f.registry
        .known
        .lock()
        .unwrap()
        .get_mut(&f.identity.machine_id)
        .unwrap()
        .active = false;
    let (_, connected) = f.run(None, &client, &f.clock, (HOST, HOST)).await;
    assert_eq!(rejected(connected), RejectCode::Revoked);
    // A certificate other than the one the system knows the machine by.
    f.registry
        .known
        .lock()
        .unwrap()
        .get_mut(&f.identity.machine_id)
        .unwrap()
        .active = true;
    f.registry
        .known
        .lock()
        .unwrap()
        .get_mut(&f.identity.machine_id)
        .unwrap()
        .fingerprint = "0".repeat(64);
    let (_, connected) = f.run(None, &client, &f.clock, (HOST, HOST)).await;
    assert_eq!(rejected(connected), RejectCode::Revoked);
}

#[tokio::test]
async fn a_certificate_that_ran_out_is_refused_a_session_with_the_code_that_says_to_renew() {
    let f = fixture(false);
    let client = f.client("0.1.0", None);
    f.clock.advance(Days::days(366));
    // Through a connection that shows it, and through one that does not.
    let (_, connected) = f
        .run(
            Some(f.identity.certificate.clone()),
            &client,
            &f.clock,
            (HOST, HOST),
        )
        .await;
    let code = rejected(connected);
    assert_eq!(code, RejectCode::CertificateExpired);
    assert_eq!(
        code.remedy(),
        Some(orch_link_contract::version::Remedy::Renew)
    );
    let (_, connected) = f.run(None, &client, &f.clock, (HOST, HOST)).await;
    assert_eq!(rejected(connected), RejectCode::CertificateExpired);
}

#[tokio::test]
async fn an_old_node_gets_no_session_and_only_a_proved_server_idle_grant_permits_its_update() {
    let f = fixture(false);
    let mut node = f.client("0.0.9", None);
    node.roles = vec![Role::Node];
    let (accepted, connected) = f.run(None, &node, &f.clock, (HOST, HOST)).await;
    assert!(matches!(
        accepted,
        Err(AcceptError::Refused {
            code: RejectCode::UpgradeRequired,
            ..
        })
    ));
    let Err(HandshakeError::Rejected(reject)) = connected else {
        panic!("old node admitted")
    };
    assert_eq!(reject.code, RejectCode::UpgradeRequired);
    assert!(
        reject.update_permit.is_none(),
        "a node with busy hosted turns must wait"
    );
    f.registry
        .known
        .lock()
        .unwrap()
        .get_mut(&f.identity.machine_id)
        .unwrap()
        .update_idle = true;
    let (_, connected) = f.run(None, &node, &f.clock, (HOST, HOST)).await;
    let Err(HandshakeError::Rejected(reject)) = connected else {
        panic!("old node admitted")
    };
    assert_eq!(reject.update_permit.unwrap().recommended, "0.2.0");
    let other = f.pki.machine(new_id());
    let mut forged = f.client("0.0.9", Some(other.key.duplicate().unwrap()));
    forged.roles = vec![Role::Node];
    let (_, connected) = f.run(None, &forged, &f.clock, (HOST, HOST)).await;
    let Err(HandshakeError::Rejected(reject)) = connected else {
        panic!("forged node admitted")
    };
    assert_eq!(reject.code, RejectCode::BadProof);
    assert!(
        reject.update_permit.is_none(),
        "no idle grant precedes identity proof"
    );
}

#[tokio::test]
async fn a_connector_older_than_the_system_accepts_is_told_to_update_and_a_newer_protocol_is_unsupported()
 {
    let f = fixture(false);
    let old = f.client("0.0.9", None);
    let (_, connected) = f.run(None, &old, &f.clock, (HOST, HOST)).await;
    let code = rejected(connected);
    assert_eq!(code, RejectCode::UpgradeRequired);
    assert_eq!(
        code.remedy(),
        Some(orch_link_contract::version::Remedy::Update)
    );
    // A machine that speaks only a protocol the gateway does not.
    let (mut machine, gateway_side) = f.pair(None, HOST, HOST);
    let (gateway, registry, clock) = (f.gateway.clone(), Arc::clone(&f.registry), f.clock.clone());
    let serving =
        tokio::spawn(
            async move { accept(gateway_side, &gateway, registry.as_ref(), &clock).await },
        );
    answer_hello(&mut machine, |nonce| Auth {
        endpoints: None,
        hosted_endpoints: None,
        protocol_version: 7,
        ..proof_auth(&f, nonce, f.clock.time())
    })
    .await;
    let Frame::Reject(reject) = reply_of(&mut machine).await else {
        panic!("not refused")
    };
    assert_eq!(reject.code, RejectCode::ProtocolUnsupported);
    assert!(serving.await.unwrap().is_err());
}

#[tokio::test(start_paused = true)]
async fn a_system_that_does_not_answer_in_five_seconds_makes_the_gateway_refuse_as_busy_with_a_delay()
 {
    let f = fixture(true);
    let client = f.client("0.1.0", None);
    let (_, connected) = f
        .run(
            Some(f.identity.certificate.clone()),
            &client,
            &f.clock,
            (HOST, HOST),
        )
        .await;
    let HandshakeError::Rejected(reject) = connected.unwrap_err() else {
        panic!("not refused")
    };
    assert_eq!(reject.code, RejectCode::Busy);
    assert_eq!(reject.retry_after_secs, Some(5));
}

#[tokio::test(start_paused = true)]
async fn a_session_is_given_up_when_nothing_is_heard_for_three_heartbeats_and_not_for_being_slow() {
    let f = fixture(false);
    let client = f.client("0.1.0", None);
    let (served, connected) = f
        .run(
            Some(f.identity.certificate.clone()),
            &client,
            &f.clock,
            (HOST, HOST),
        )
        .await;
    let mut gateway_end = served.unwrap();
    let machine_end = connected.unwrap();
    // Silence on both sides: pings keep the session alive for as long as they are answered.
    tokio::time::sleep(Duration::from_secs(300)).await;
    assert!(
        gateway_end.sender.is_open() && machine_end.sender.is_open(),
        "pings keep a quiet session alive"
    );
    let measured = machine_end.sender.meter().measurements();
    assert!(
        measured.round_trip_ms.is_some(),
        "the round trip is measured from pings"
    );
    // The machine goes away without a word: the gateway gives the session up after three heartbeats.
    drop(machine_end.inbox);
    machine_end.sender.end();
    let ended = tokio::time::timeout(Duration::from_secs(120), async {
        loop {
            match gateway_end.inbox.next().await {
                Some(orch_link::session::SessionEvent::Ended(reason)) => break reason,
                Some(_) => {}
                None => break orch_link::session::EndReason::PeerClosed,
            }
        }
    })
    .await
    .unwrap();
    assert!(
        matches!(
            ended,
            orch_link::session::EndReason::PeerClosed | orch_link::session::EndReason::Dead
        ),
        "{ended:?}"
    );
}
