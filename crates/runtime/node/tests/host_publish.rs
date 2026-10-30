//! Host TCP callers and UDP senders have distinct real inside sockets and bounded live numeric records.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]

use bytes::Bytes;
use orch_api_contract::ports::{
    EnvironmentPortConnectionListQuery, PortCaller, PortConnectionOutside, PortConnectionOwner,
    PortTransport,
};
use orch_connector_core::tunnels::Tunnels;
use orch_link_contract::ports::{
    HostPortTemplate, PortClose, PortPublish, PortPublishState, PortPublishTicket, PortTicketClaims,
};
use orch_link_contract::session::Role;
use orch_link_contract::tunnels::{TicketClaims, TicketSource, TunnelKind};
use orch_node_core::host_publish::Publisher;
use orch_pki::tickets::TicketSigner;
use orch_testkit::doubles::controller::ScriptedController;
use orch_testkit::doubles::link::{Hub, TestPki};
use orch_testkit::doubles::machine::{MachineSpec, ScriptedMachine};
use orch_types::{Clock, SystemClock, new_id};
use std::sync::Arc;
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

#[tokio::test]
async fn one_host_template_serves_two_tcp_callers_and_two_udp_senders_then_close_releases_the_number()
 {
    let clock: Arc<dyn Clock> = Arc::new(SystemClock);
    let pki = TestPki::new(clock.clone());
    let hub = Hub::new(clock.clone(), pki.certificate());
    let signer = TicketSigner::generate();
    hub.set_ticket_verifier(signer.verifier());
    let (node_id, unit_id, environment, opening, host) =
        (new_id(), new_id(), new_id(), new_id(), new_id());
    let _node_controller =
        ScriptedController::attach(&hub, clock.clone(), node_id, vec![Role::Node]);
    let _unit_controller =
        ScriptedController::attach(&hub, clock.clone(), unit_id, vec![Role::Environment]);
    let node = ScriptedMachine::start(
        &hub,
        &clock,
        &pki.machine(node_id),
        &MachineSpec {
            roles: vec![Role::Node],
            ..Default::default()
        },
    );
    let unit = ScriptedMachine::start(
        &hub,
        &clock,
        &pki.machine(unit_id),
        &MachineSpec {
            roles: vec![Role::Environment],
            ..Default::default()
        },
    );
    tokio::time::timeout(Duration::from_secs(30), async {
        node.link.connected().await;
        unit.link.connected().await;
    })
    .await
    .unwrap();
    let mut available = node.link.port_control();
    tokio::time::timeout(Duration::from_secs(30), available.wait_for(|ready| *ready))
        .await
        .unwrap()
        .unwrap();
    let publisher = Publisher::with_clock(Tunnels::new(node.link.clone()), clock.clone());
    let native = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let target = native.local_addr().unwrap();
    let native_udp = Arc::new(tokio::net::UdpSocket::bind(target).await.unwrap());
    // Named receiver double: actual sockets/peers and bytes, not the environment's native ownership reader.
    // The connector's ports_carrier suite separately proves that current owned-process boundary.
    let echoes = tokio::spawn(async move {
        loop {
            let (mut socket, _) = native.accept().await.unwrap();
            tokio::spawn(async move {
                let (mut read, mut write) = socket.split();
                let _ = tokio::io::copy(&mut read, &mut write).await;
            });
        }
    });
    let datagram_echo = tokio::spawn(async move {
        let mut buffer = vec![0; 65536];
        loop {
            let (size, peer) = native_udp.recv_from(&mut buffer).await.unwrap();
            native_udp.send_to(&buffer[..size], peer).await.unwrap();
        }
    });
    let mut incoming = unit.link.incoming_tunnels();
    let receivers = tokio::spawn(async move {
        while let Some(incoming) = incoming.next().await {
            tokio::spawn(async move {
                let transport = incoming.claims.port.as_ref().unwrap().transport;
                match transport {
                    PortTransport::Tcp => {
                        let mut socket = tokio::net::TcpStream::connect(target).await.unwrap();
                        let inside = socket.local_addr().unwrap();
                        if let Ok(mut tunnel) =
                            incoming.accept_port(orch_api_contract::ports::PortConnectionInside {
                                address: inside.ip().to_string(),
                                port: inside.port(),
                            })
                        {
                            let _ = tokio::io::copy_bidirectional(&mut tunnel, &mut socket).await;
                        }
                    }
                    PortTransport::Udp => {
                        let socket = tokio::net::UdpSocket::bind("127.0.0.1:0").await.unwrap();
                        socket.connect(target).await.unwrap();
                        let inside = socket.local_addr().unwrap();
                        let tunnel = incoming
                            .accept_port(orch_api_contract::ports::PortConnectionInside {
                                address: inside.ip().to_string(),
                                port: inside.port(),
                            })
                            .unwrap();
                        let mut buffer = vec![0; 65536];
                        loop {
                            tokio::select! {
                                packet=tunnel.recv_datagram()=>match packet {Ok(packet)=>{socket.send(&packet).await.unwrap();},Err(_)=>break},
                                packet=socket.recv(&mut buffer)=>{let size=packet.unwrap();if tunnel.send_datagram(Bytes::copy_from_slice(&buffer[..size])).is_err() {break;}}
                            }
                        }
                    }
                }
            });
        }
    });
    let probe = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let outside = probe.local_addr().unwrap();
    let udp_probe = tokio::net::UdpSocket::bind(outside).await.unwrap();
    drop(probe);
    drop(udp_probe);
    let ticket = |transport| {
        let jti = new_id();
        signer
            .sign(&TicketClaims {
                jti,
                kind: TunnelKind::Port,
                src: TicketSource::Machine(node_id),
                dst: unit_id,
                service: format!("port/{jti}"),
                thread_id: None,
                tool_set: None,
                tool_environment_id: None,
                tool_environment_generation: None,
                exp: (clock.time().unix_millis() / 1000) + 300,
                port: Some(PortTicketClaims {
                    environment_id: environment,
                    generation: 1,
                    port: target.port(),
                    transport,
                    opening_id: Some(opening),
                    authority_revision: 1,
                    audience: Some(orch_api_contract::ports::PortOpeningKind::Host),
                    allocation_revision: Some(1),
                    host_template: Some(HostPortTemplate {
                        host_id: host,
                        proxy_protocol: false,
                    }),
                    source: PortConnectionOutside {
                        address: "host.example".into(),
                        port: outside.port(),
                        caller: PortCaller::Address,
                    },
                }),
            })
            .unwrap()
    };
    let published = publisher
        .publish(PortPublish {
            opening_id: opening,
            environment_id: environment,
            environment_machine_id: unit_id,
            generation: 1,
            port: target.port(),
            transports: vec![PortTransport::Tcp, PortTransport::Udp],
            incoming_address: "host.example".into(),
            listen: "127.0.0.1".into(),
            host_port: outside.port(),
            revision: 1,
            allocation_revision: 1,
            proxy_protocol: false,
            tickets: vec![
                PortPublishTicket {
                    transport: PortTransport::Tcp,
                    ticket: ticket(PortTransport::Tcp),
                },
                PortPublishTicket {
                    transport: PortTransport::Udp,
                    ticket: ticket(PortTransport::Udp),
                },
            ],
        })
        .await;
    assert_eq!(
        published.state,
        PortPublishState::Open,
        "{:?}",
        published.reason
    );
    let mut first = tokio::net::TcpStream::connect(outside).await.unwrap();
    let mut second = tokio::net::TcpStream::connect(outside).await.unwrap();
    let first_peer = first.local_addr().unwrap();
    let second_peer = second.local_addr().unwrap();
    let udp1 = tokio::net::UdpSocket::bind("127.0.0.1:0").await.unwrap();
    let udp2 = tokio::net::UdpSocket::bind("127.0.0.1:0").await.unwrap();
    tokio::time::timeout(Duration::from_secs(30), async {
        first.write_all(b"one").await.unwrap();
        second.write_all(b"two").await.unwrap();
        let mut bytes = [0; 3];
        first.read_exact(&mut bytes).await.unwrap();
        assert_eq!(&bytes, b"one");
        second.read_exact(&mut bytes).await.unwrap();
        assert_eq!(&bytes, b"two");
        udp1.send_to(b"alpha", outside).await.unwrap();
        udp2.send_to(b"beta", outside).await.unwrap();
        let mut bytes = [0; 16];
        let (size, _) = udp1.recv_from(&mut bytes).await.unwrap();
        assert_eq!(&bytes[..size], b"alpha");
        let (size, _) = udp2.recv_from(&mut bytes).await.unwrap();
        assert_eq!(&bytes[..size], b"beta");
    })
    .await
    .unwrap();
    let records = publisher.records();
    let query = EnvironmentPortConnectionListQuery {
        limit: Some(200),
        ..Default::default()
    };
    tokio::time::timeout(Duration::from_secs(30), async {
        loop {
            let list = records.list(environment, 1, &query, true, clock.time());
            if list.counts.active == 4 && list.counts.bytes_in == 15 && list.counts.bytes_out == 15
            {
                break;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    let list = records.list(environment, 1, &query, true, clock.time());
    assert_eq!(list.owner, PortConnectionOwner::Node);
    let rows = list.connections.unwrap();
    let first_row = rows
        .iter()
        .find(|row| row.outside.port == first_peer.port() && row.transport == PortTransport::Tcp)
        .unwrap();
    let second_row = rows
        .iter()
        .find(|row| row.outside.port == second_peer.port() && row.transport == PortTransport::Tcp)
        .unwrap();
    assert_ne!(first_row.id, second_row.id);
    assert_ne!(first_row.inside, second_row.inside);
    assert_eq!(first_row.outside.address, first_peer.ip().to_string());
    assert!(rows.iter().all(
        |row| matches!(&row.outside.caller, PortCaller::Address) && row.http_requests.is_none()
    ));
    assert_eq!(
        rows.iter()
            .filter(|row| row.transport == PortTransport::Udp)
            .count(),
        2
    );
    assert_eq!(
        hub.with_relay(|relay| relay
            .port_records()
            .list(environment, 1, &query, true, clock.time())
            .counts
            .active),
        0,
        "the node owns raw-host rows; the gateway must not count the same sockets twice"
    );
    let old_node = hub.session_of(node_id).unwrap();
    let old_unit = hub.session_of(unit_id).unwrap();
    hub.faults().sever_now();
    tokio::time::timeout(Duration::from_secs(30), async {
        loop {
            if hub.session_of(node_id).is_some_and(|id| id != old_node)
                && hub.session_of(unit_id).is_some_and(|id| id != old_unit)
            {
                break;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    tokio::time::timeout(Duration::from_secs(30), async {
        first.write_all(b"again").await.unwrap();
        let mut bytes = [0; 5];
        first.read_exact(&mut bytes).await.unwrap();
        assert_eq!(&bytes, b"again");
        second.write_all(b"other").await.unwrap();
        second.read_exact(&mut bytes).await.unwrap();
        assert_eq!(&bytes, b"other");
    })
    .await
    .unwrap();
    let resumed = records.list(environment, 1, &query, true, clock.time());
    for before in rows
        .iter()
        .filter(|row| row.transport == PortTransport::Tcp)
    {
        let after = resumed
            .connections
            .as_ref()
            .unwrap()
            .iter()
            .find(|row| row.id == before.id)
            .unwrap();
        assert_eq!(
            after.inside, before.inside,
            "resuming a session keeps this exact accepted socket"
        );
        assert!(after.ended_at.is_none());
    }
    assert!(
        publisher
            .close(&PortClose {
                opening_id: opening,
                environment_id: environment,
                generation: 1,
                revision: 1
            })
            .await
            .closed
    );
    assert_eq!(
        records
            .list(environment, 1, &query, true, clock.time())
            .counts
            .active,
        0
    );
    let rebound = tokio::net::TcpListener::bind(outside).await.unwrap();
    let rebound_udp = tokio::net::UdpSocket::bind(outside).await.unwrap();
    drop(rebound);
    drop(rebound_udp);
    echoes.abort();
    datagram_echo.abort();
    receivers.abort();
    node.link.shutdown().await;
    unit.link.shutdown().await;
}
