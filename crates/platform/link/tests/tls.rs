//! The handshake of each trust path over real TLS on a loopback socket: the pinned path under the system's
//! authority with a client certificate, and the public path through an edge with a signed handshake.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]

use async_trait::async_trait;
use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use chrono::{Duration as Days, TimeZone};
use orch_link::framing::Message;
use orch_link::reconnect::Dialer;
use orch_link::session::{
    ClientIdentity, GatewayConfig, HandshakeError, SessionAuthority, SessionEvent, accept, connect,
};
use orch_link::websocket::{TlsDialer, accept_tls};
use orch_link_contract::backplane::{AuthQuery, AuthReply};
use orch_link_contract::enrollment::{Endpoint, Trust};
use orch_link_contract::frames::{self, Frame, Text};
use orch_link_contract::session::{
    Arch, Auth, AuthProof, Os, Platform, Role, TlsMode, auth_signing_input,
};
use orch_link_contract::version::{ConnectorVersions, Limits, RejectCode};
use orch_pki::ca::{CA_VALIDITY_DAYS, CertificateAuthority, Holder};
use orch_pki::certificates::{Certificate, LeafKey, ServiceName};
use orch_pki::verification::{
    ServerTrust, TlsIdentity, pinned_client_config, pinned_server_config, plain_server_config,
    public_client_config, root_store,
};
use orch_types::{Clock, Id, ManualClock, new_id};
use std::collections::{BTreeMap, HashMap};
use std::net::SocketAddr;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::net::TcpListener;
use tokio::sync::mpsc;
use tokio_rustls::TlsAcceptor;

#[derive(Debug)]
enum Seen {
    Session {
        machine: Id,
        tls: TlsMode,
        host: Option<String>,
    },
    Refused(RejectCode),
    NeverSpoke(String),
}

struct Registry {
    known: Mutex<HashMap<Id, (bool, String)>>,
}

#[async_trait]
impl SessionAuthority for Registry {
    async fn authorize(&self, query: AuthQuery) -> AuthReply {
        let refuse = |code| AuthReply {
            endpoints: None,
            update_permit: None,
            n: query.n,
            ok: false,
            code: Some(code),
            cursors: None,
            placement: None,
        };
        match self.known.lock().unwrap().get(&query.machine_id) {
            None => refuse(RejectCode::UnknownMachine),
            Some((active, fingerprint)) if !active || fingerprint != &query.fingerprint => {
                refuse(RejectCode::Revoked)
            }
            Some(_) => AuthReply {
                endpoints: None,
                update_permit: None,
                n: query.n,
                ok: true,
                code: None,
                cursors: Some(BTreeMap::new()),
                placement: Some("connected".to_owned()),
            },
        }
    }
}

struct World {
    clock: ManualClock,
    system: CertificateAuthority,
    public: CertificateAuthority,
    registry: Arc<Registry>,
    machine_id: Id,
    machine_key: LeafKey,
    machine_certificate: Certificate,
}

fn world() -> World {
    let clock = ManualClock::new(
        chrono::Utc
            .with_ymd_and_hms(2026, 10, 8, 14, 0, 0)
            .single()
            .unwrap(),
    );
    let system = CertificateAuthority::create("Test", CA_VALIDITY_DAYS, &clock).unwrap();
    let public =
        CertificateAuthority::create("A public authority", CA_VALIDITY_DAYS, &clock).unwrap();
    let machine_id = new_id();
    let (machine_key, machine_certificate) = system
        .issue_with_new_key(&Holder::Machine(machine_id), 365, &clock)
        .unwrap();
    let registry = Arc::new(Registry {
        known: Mutex::new(HashMap::from([(
            machine_id,
            (true, machine_certificate.fingerprint()),
        )])),
    });
    World {
        clock,
        system,
        public,
        registry,
        machine_id,
        machine_key,
        machine_certificate,
    }
}

impl World {
    fn arc_clock(&self) -> Arc<dyn Clock> {
        Arc::new(self.clock.clone())
    }

    /// A service certificate good for `localhost`, outliving the tests that move the clock by years.
    fn server_identity(authority: &CertificateAuthority, clock: &ManualClock) -> TlsIdentity {
        let (key, certificate) = authority
            .issue_with_new_key(
                &Holder::Service {
                    service: ServiceName::Gateway,
                    names: vec!["localhost".to_owned(), "127.0.0.1".to_owned()],
                },
                4_000,
                clock,
            )
            .unwrap();
        TlsIdentity {
            chain: vec![certificate, authority.certificate().clone()],
            key,
        }
    }

    fn gateway_config(&self) -> GatewayConfig {
        GatewayConfig {
            gateway_id: "gw".to_owned(),
            limits: Limits::default(),
            versions: ConnectorVersions {
                min: "0.1.0".to_owned(),
                recommended: "0.1.0".to_owned(),
            },
            heartbeat_secs: 20,
            authority: self.system.certificate().clone(),
        }
    }

    /// Serves sessions on a loopback socket under `tls` and reports what each connection came to.
    async fn serve(
        &self,
        tls: Arc<rustls::ServerConfig>,
    ) -> (SocketAddr, mpsc::UnboundedReceiver<Seen>) {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let (seen_tx, seen_rx) = mpsc::unbounded_channel();
        let acceptor = TlsAcceptor::from(tls);
        let (gateway, registry, clock) = (
            self.gateway_config(),
            Arc::clone(&self.registry),
            self.clock.clone(),
        );
        tokio::spawn(async move {
            loop {
                let Ok((socket, _)) = listener.accept().await else {
                    return;
                };
                let (acceptor, gateway, registry, clock, seen) = (
                    acceptor.clone(),
                    gateway.clone(),
                    Arc::clone(&registry),
                    clock.clone(),
                    seen_tx.clone(),
                );
                tokio::spawn(async move {
                    let connection = match accept_tls(&acceptor, socket).await {
                        Ok(connection) => connection,
                        Err(error) => {
                            let _ = seen.send(Seen::NeverSpoke(error.to_string()));
                            return;
                        }
                    };
                    let host = connection.peer.host.clone();
                    match accept(connection, &gateway, registry.as_ref(), &clock).await {
                        Ok(mut accepted) => {
                            let _ = seen.send(Seen::Session {
                                machine: accepted.auth.machine_id,
                                tls: accepted.tls,
                                host,
                            });
                            while let Some(event) = accepted.inbox.next().await {
                                if matches!(event, SessionEvent::Ended(_)) {
                                    break;
                                }
                            }
                        }
                        Err(orch_link::session::AcceptError::Refused { code, .. }) => {
                            let _ = seen.send(Seen::Refused(code));
                        }
                        Err(other) => {
                            let _ = seen.send(Seen::NeverSpoke(other.to_string()));
                        }
                    }
                });
            }
        });
        (address, seen_rx)
    }

    fn machine(&self) -> ClientIdentity {
        ClientIdentity {
            hosted_endpoints: None,
            endpoints: None,
            machine_id: self.machine_id,
            connector_version: "0.1.0".to_owned(),
            platform: Platform {
                os: Os::Linux,
                arch: Arch::X86_64,
            },
            roles: vec![Role::Agent],
            proof: Some((
                self.machine_key.duplicate().unwrap(),
                self.machine_certificate.clone(),
            )),
        }
    }

    fn pinned_dialer(&self, identity: &TlsIdentity) -> TlsDialer {
        let config = pinned_client_config(
            ServerTrust::Authority(Box::new(self.system.certificate().clone())),
            Some(ServiceName::Gateway),
            Some(identity),
            self.arc_clock(),
        )
        .unwrap();
        TlsDialer::new(Some(config), None)
    }

    fn mutual_identity(&self) -> TlsIdentity {
        TlsIdentity {
            chain: vec![self.machine_certificate.clone()],
            key: self.machine_key.duplicate().unwrap(),
        }
    }

    fn public_dialer(&self) -> TlsDialer {
        let roots = root_store(&[self.public.certificate().clone()]).unwrap();
        TlsDialer::new(
            None,
            Some(public_client_config(roots, self.arc_clock()).unwrap()),
        )
    }
}

fn endpoint(address: SocketAddr, trust: Trust) -> Endpoint {
    Endpoint {
        url: format!("wss://localhost:{}", address.port()),
        trust,
    }
}

async fn next(seen: &mut mpsc::UnboundedReceiver<Seen>) -> Seen {
    tokio::time::timeout(Duration::from_secs(10), seen.recv())
        .await
        .expect("nothing came of the connection")
        .unwrap()
}

#[tokio::test]
async fn on_the_pinned_path_the_client_certificate_is_the_identity_and_both_sides_are_the_systems()
{
    let world = world();
    let server = World::server_identity(&world.system, &world.clock);
    let tls =
        pinned_server_config(&server, world.system.certificate(), 90, world.arc_clock()).unwrap();
    let (address, mut seen) = world.serve(tls).await;
    let dialer = world.pinned_dialer(&world.mutual_identity());
    let connection = dialer
        .dial(&endpoint(address, Trust::Pinned))
        .await
        .unwrap();
    let established = connect(connection, &world.machine(), &world.clock)
        .await
        .unwrap();
    assert_eq!(
        established.tls,
        TlsMode::Mutual,
        "no proof was needed: the connection showed the machine"
    );
    let Seen::Session { machine, tls, host } = next(&mut seen).await else {
        panic!("no session")
    };
    assert_eq!((machine, tls), (world.machine_id, TlsMode::Mutual));
    assert_eq!(
        host.as_deref(),
        Some("localhost"),
        "the gateway knows which host the machine asked for"
    );
}

#[tokio::test]
async fn on_the_public_path_the_machine_proves_itself_by_signing_the_handshake() {
    let world = world();
    let edge = World::server_identity(&world.public, &world.clock);
    let tls = plain_server_config(&edge, world.arc_clock()).unwrap();
    let (address, mut seen) = world.serve(tls).await;
    let dialer = world.public_dialer();
    let connection = dialer
        .dial(&endpoint(address, Trust::Public))
        .await
        .unwrap();
    let established = connect(connection, &world.machine(), &world.clock)
        .await
        .unwrap();
    assert_eq!(established.tls, TlsMode::Server);
    let Seen::Session { machine, tls, .. } = next(&mut seen).await else {
        panic!("no session")
    };
    assert_eq!((machine, tls), (world.machine_id, TlsMode::Server));
}

#[tokio::test]
async fn a_client_certificate_of_a_wrong_authority_does_not_get_as_far_as_a_handshake() {
    let world = world();
    let server = World::server_identity(&world.system, &world.clock);
    let tls =
        pinned_server_config(&server, world.system.certificate(), 90, world.arc_clock()).unwrap();
    let (address, mut seen) = world.serve(tls).await;
    let intruder = CertificateAuthority::create("Intruder", 3_650, &world.clock).unwrap();
    let (key, certificate) = intruder
        .issue_with_new_key(&Holder::Machine(world.machine_id), 365, &world.clock)
        .unwrap();
    let dialer = world.pinned_dialer(&TlsIdentity {
        chain: vec![certificate],
        key,
    });
    assert!(
        dialer
            .dial(&endpoint(address, Trust::Pinned))
            .await
            .is_err()
    );
    let Seen::NeverSpoke(why) = next(&mut seen).await else {
        panic!("the gateway heard a frame")
    };
    assert!(
        why.contains("TLS"),
        "it was the TLS handshake that failed: {why}"
    );
}

#[tokio::test]
async fn a_gateway_that_is_not_the_systems_is_not_believed_by_a_machine() {
    let world = world();
    let intruder = CertificateAuthority::create("Intruder", 3_650, &world.clock).unwrap();
    let server = World::server_identity(&intruder, &world.clock);
    let tls = pinned_server_config(&server, intruder.certificate(), 90, world.arc_clock()).unwrap();
    let (address, _seen) = world.serve(tls).await;
    let dialer = world.pinned_dialer(&world.mutual_identity());
    assert!(
        dialer
            .dial(&endpoint(address, Trust::Pinned))
            .await
            .is_err()
    );
}

#[tokio::test]
async fn an_edge_no_public_authority_vouches_for_is_not_believed_by_a_machine() {
    let world = world();
    let impostor = CertificateAuthority::create("Impostor", 3_650, &world.clock).unwrap();
    let edge = World::server_identity(&impostor, &world.clock);
    let tls = plain_server_config(&edge, world.arc_clock()).unwrap();
    let (address, _seen) = world.serve(tls).await;
    assert!(
        world
            .public_dialer()
            .dial(&endpoint(address, Trust::Public))
            .await
            .is_err()
    );
}

#[tokio::test]
async fn an_expired_certificate_is_heard_within_the_grace_but_given_no_session_and_is_not_heard_after_it()
 {
    let world = world();
    let server = World::server_identity(&world.system, &world.clock);
    let tls =
        pinned_server_config(&server, world.system.certificate(), 90, world.arc_clock()).unwrap();
    let (address, mut seen) = world.serve(tls).await;
    // A month after it ran out.
    world.clock.advance(Days::days(365 + 30));
    let dialer = world.pinned_dialer(&world.mutual_identity());
    let connection = dialer
        .dial(&endpoint(address, Trust::Pinned))
        .await
        .expect("the connection is made, to be told to renew");
    let HandshakeError::Rejected(reject) = connect(connection, &world.machine(), &world.clock)
        .await
        .unwrap_err()
    else {
        panic!("not refused")
    };
    assert_eq!(reject.code, RejectCode::CertificateExpired);
    assert!(matches!(
        next(&mut seen).await,
        Seen::Refused(RejectCode::CertificateExpired)
    ));
    // Four months after: the connection is not even made.
    world.clock.advance(Days::days(100));
    let dialer = world.pinned_dialer(&world.mutual_identity());
    assert!(
        dialer
            .dial(&endpoint(address, Trust::Pinned))
            .await
            .is_err()
    );
}

#[tokio::test]
async fn a_machine_the_system_does_not_know_or_has_revoked_is_refused_with_the_code_that_says_so() {
    let world = world();
    let edge = World::server_identity(&world.public, &world.clock);
    let (address, mut seen) = world
        .serve(plain_server_config(&edge, world.arc_clock()).unwrap())
        .await;
    let dialer = world.public_dialer();
    world
        .registry
        .known
        .lock()
        .unwrap()
        .get_mut(&world.machine_id)
        .unwrap()
        .0 = false;
    let connection = dialer
        .dial(&endpoint(address, Trust::Public))
        .await
        .unwrap();
    let HandshakeError::Rejected(reject) = connect(connection, &world.machine(), &world.clock)
        .await
        .unwrap_err()
    else {
        panic!("not refused")
    };
    assert_eq!(reject.code, RejectCode::Revoked);
    assert!(matches!(
        next(&mut seen).await,
        Seen::Refused(RejectCode::Revoked)
    ));
    world.registry.known.lock().unwrap().clear();
    let connection = dialer
        .dial(&endpoint(address, Trust::Public))
        .await
        .unwrap();
    let HandshakeError::Rejected(reject) = connect(connection, &world.machine(), &world.clock)
        .await
        .unwrap_err()
    else {
        panic!("not refused")
    };
    assert_eq!(reject.code, RejectCode::UnknownMachine);
}

fn auth_for(world: &World, nonce: &str, host: &str) -> Auth {
    let sent_at = world.clock.time();
    let signed = auth_signing_input(nonce, world.machine_id, host, sent_at);
    Auth {
        endpoints: None,
        hosted_endpoints: None,
        machine_id: world.machine_id,
        protocol_version: 1,
        connector_version: "0.1.0".to_owned(),
        platform: Platform {
            os: Os::Linux,
            arch: Arch::X86_64,
        },
        roles: vec![Role::Agent],
        sent_at,
        proof: Some(AuthProof {
            certificate_pem: world.machine_certificate.pem(),
            signature: URL_SAFE_NO_PAD.encode(world.machine_key.sign(signed.as_bytes())),
        }),
    }
}

async fn read_frame(connection: &mut orch_link::framing::Connection) -> Frame {
    let Some(Message::Text(text)) = connection.source.recv().await.unwrap() else {
        panic!("no frame")
    };
    let Text::Known(frame) = frames::decode::<Frame>(&text, 262_144).unwrap() else {
        panic!("unknown frame")
    };
    frame
}

#[tokio::test]
async fn over_real_tls_a_replayed_handshake_and_a_proof_for_another_host_are_refused() {
    let world = world();
    let edge = World::server_identity(&world.public, &world.clock);
    let (address, mut seen) = world
        .serve(plain_server_config(&edge, world.arc_clock()).unwrap())
        .await;
    let dialer = world.public_dialer();
    // A genuine handshake, recorded on the wire.
    let mut first = dialer
        .dial(&endpoint(address, Trust::Public))
        .await
        .unwrap();
    let Frame::Hello(hello) = read_frame(&mut first).await else {
        panic!("no hello")
    };
    let genuine = frames::encode(
        &Frame::Auth(auth_for(&world, &hello.nonce, "localhost")),
        262_144,
    )
    .unwrap();
    first
        .sink
        .send(Message::Text(genuine.clone()))
        .await
        .unwrap();
    assert!(matches!(read_frame(&mut first).await, Frame::Ready(_)));
    assert!(matches!(next(&mut seen).await, Seen::Session { .. }));
    // The same words on another connection.
    let mut second = dialer
        .dial(&endpoint(address, Trust::Public))
        .await
        .unwrap();
    let Frame::Hello(_) = read_frame(&mut second).await else {
        panic!("no hello")
    };
    second.sink.send(Message::Text(genuine)).await.unwrap();
    let Frame::Reject(reject) = read_frame(&mut second).await else {
        panic!("not refused")
    };
    assert_eq!(reject.code, RejectCode::BadProof);
    assert!(matches!(
        next(&mut seen).await,
        Seen::Refused(RejectCode::BadProof)
    ));
    // A proof signed for a host the machine did not connect to.
    let mut third = dialer
        .dial(&endpoint(address, Trust::Public))
        .await
        .unwrap();
    let Frame::Hello(hello) = read_frame(&mut third).await else {
        panic!("no hello")
    };
    let other_host = frames::encode(
        &Frame::Auth(auth_for(&world, &hello.nonce, "other.example.com")),
        262_144,
    )
    .unwrap();
    third.sink.send(Message::Text(other_host)).await.unwrap();
    let Frame::Reject(reject) = read_frame(&mut third).await else {
        panic!("not refused")
    };
    assert_eq!(reject.code, RejectCode::BadProof);
}
