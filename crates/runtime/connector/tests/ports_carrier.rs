//! Real connector roles carry owned native TCP and UDP through their admitted certificate-bound link.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]
#[cfg(target_os = "linux")]
mod rig_environment;

#[cfg(target_os = "linux")]
use bytes::Bytes;
#[cfg(target_os = "linux")]
use orch_api_contract::ports::{PortCaller, PortConnectionOutside, PortTransport};
#[cfg(target_os = "linux")]
use orch_envhost::processes::{Processes, RootKind};
#[cfg(target_os = "linux")]
use orch_link_contract::ports::PortTicketClaims;
#[cfg(target_os = "linux")]
use orch_link_contract::tunnels::{TicketClaims, TicketSource, TunnelKind};
#[cfg(target_os = "linux")]
use orch_types::new_id;
#[cfg(target_os = "linux")]
use rig_environment::{EnvRig, PATIENCE};
#[cfg(target_os = "linux")]
use std::io::{BufRead, BufReader};
#[cfg(target_os = "linux")]
use std::process::{Child, Command, Stdio};
#[cfg(target_os = "linux")]
use tokio::io::{AsyncReadExt, AsyncWriteExt};

#[cfg(target_os = "linux")]
const SERVER: &str = r#"
import socket,threading,json,sys,random
fixed=int(sys.argv[1]); tag=sys.argv[2].encode()
for port in ([fixed] if fixed else random.sample([p for p in range(41000,42000) if not 41280<=p<=41299],980)):
    tcp=socket.socket(); udp=socket.socket(socket.AF_INET,socket.SOCK_DGRAM)
    try:
        tcp.setsockopt(socket.SOL_SOCKET,socket.SO_REUSEADDR,1)
        tcp.bind(('127.0.0.1',port)); udp.bind(('127.0.0.1',port)); tcp.listen(); break
    except OSError:
        tcp.close(); udp.close()
        if fixed: raise
def stream(c):
    c.sendall(tag)
    while True:
        b=c.recv(65536)
        if not b: break
        c.sendall(b)
    c.shutdown(socket.SHUT_WR); c.close()
def streams():
    while True:
        c,_=tcp.accept(); threading.Thread(target=stream,args=(c,),daemon=True).start()
def packets():
    while True:
        b,peer=udp.recvfrom(65536); udp.sendto(b,peer)
threading.Thread(target=streams,daemon=True).start(); threading.Thread(target=packets,daemon=True).start()
print(json.dumps({'port':port}),flush=True); sys.stdin.readline()
"#;
#[cfg(target_os = "linux")]
struct Service {
    child: Child,
    port: u16,
}
#[cfg(target_os = "linux")]
impl Service {
    fn start(processes: Option<&Processes>, port: u16, tag: &str) -> Self {
        let mut command = Command::new("python3");
        command
            .args(["-I", "-B", "-u", "-c", SERVER])
            .arg(port.to_string())
            .arg(tag)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped());
        let mut child = if let Some(processes) = processes {
            processes
                .spawn_tracked(&mut command, RootKind::Executor)
                .unwrap()
        } else {
            orch_envhost::platform::prepare_group(&mut command);
            command.spawn().unwrap()
        };
        let mut line = String::new();
        BufReader::new(child.stdout.take().unwrap())
            .read_line(&mut line)
            .unwrap();
        let row: serde_json::Value = serde_json::from_str(&line).unwrap();
        Self {
            child,
            port: row["port"].as_u64().unwrap() as u16,
        }
    }
    fn stop(&mut self) {
        self.child.kill().unwrap();
        self.child.wait().unwrap();
    }
}
#[cfg(target_os = "linux")]
impl Drop for Service {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}
#[cfg(target_os = "linux")]
fn ticket(rig: &EnvRig, src: orch_types::Id, port: u16, transport: PortTransport) -> String {
    ticket_for(
        rig,
        src,
        port,
        transport,
        Some(orch_api_contract::ports::PortOpeningKind::Place),
    )
}

#[cfg(target_os = "linux")]
fn ticket_for(
    rig: &EnvRig,
    src: orch_types::Id,
    port: u16,
    transport: PortTransport,
    audience: Option<orch_api_contract::ports::PortOpeningKind>,
) -> String {
    let id = new_id();
    let generation = rig
        .connector
        .as_ref()
        .unwrap()
        .environment_role()
        .unwrap()
        .host()
        .environment(rig.environment)
        .unwrap()
        .facts()
        .generation;
    rig.signer
        .sign(&TicketClaims {
            jti: id,
            kind: TunnelKind::Port,
            src: TicketSource::Machine(src),
            dst: rig.machine_id,
            service: format!("port/{id}"),
            thread_id: None,
            provider_id: None,
            tool_set: None,
            tool_environment_id: None,
            tool_environment_generation: None,
            exp: rig.deadline().unix_millis() / 1000,
            port: Some(PortTicketClaims {
                environment_id: rig.environment,
                generation,
                port,
                transport,
                opening_id: Some(new_id()),
                audience,
                authority_revision: 1,
                allocation_revision: None,
                host_template: None,
                source: PortConnectionOutside {
                    address: "127.0.0.1".into(),
                    port: 41001,
                    caller: PortCaller::Address,
                },
            }),
        })
        .unwrap()
}

#[cfg(target_os = "linux")]
async fn opener_ready(rig: &EnvRig) -> rig_environment::tunnels::Opener {
    let opener = rig.opener().await;
    let mut current = opener.machine.link.port_control();
    tokio::time::timeout(PATIENCE, current.wait_for(|available| *available))
        .await
        .unwrap()
        .unwrap();
    opener
}

#[cfg(target_os = "linux")]
#[tokio::test]
async fn a_host_accept_writes_proxy_at_the_owned_receiver_before_caller_supplied_prefix_bytes() {
    let mut rig = EnvRig::new();
    rig.config.ports.allowed = vec![orch_api_contract::ports::PortOpeningKind::Host];
    rig.config.ports.publish = Some(orch_api_contract::ports::HostPorts {
        incoming_address: "vm.example.test".into(),
        listen: "127.0.0.1".into(),
        ranges: vec![orch_api_contract::ports::PortRange {
            from: 41000,
            to: 41999,
        }],
    });
    rig.start().await;
    let directory = rig.directory("host-prefix");
    rig.attach(&directory).await;
    let env = rig
        .connector
        .as_ref()
        .unwrap()
        .environment_role()
        .unwrap()
        .host()
        .environment(rig.environment)
        .unwrap();
    let service = Service::start(Some(env.processes()), 0, "prefix:");
    let opener = opener_ready(&rig).await;
    let ordinary = ticket(&rig, opener.machine_id, service.port, PortTransport::Tcp);
    let mut claims = orch_link_contract::tunnels::Ticket::parse(&ordinary)
        .unwrap()
        .claims()
        .clone();
    let port = claims.port.as_mut().unwrap();
    port.allocation_revision = Some(1);
    port.audience = Some(orch_api_contract::ports::PortOpeningKind::Host);
    port.host_template = Some(orch_link_contract::ports::HostPortTemplate {
        host_id: new_id(),
        proxy_protocol: true,
    });
    let template = rig.signer.sign(&claims).unwrap();
    assert!(
        opener.open(&template).await.is_err(),
        "a template alone supplies no observed outside socket"
    );
    let peer = orch_link_contract::ports::HostPortPeer {
        use_id: new_id(),
        peer: orch_api_contract::ports::PortConnectionInside {
            address: "192.0.2.41".into(),
            port: 41001,
        },
    };
    assert!(
        opener
            .machine
            .link
            .open_host_port(&ordinary, peer.clone())
            .await
            .is_err(),
        "an ordinary single-use ticket cannot change its signed peer"
    );
    let mut stream = opener
        .machine
        .link
        .open_host_port(&template, peer)
        .await
        .unwrap();
    let caller_bytes = b"PROXY caller-supplied bytes remain data\r\n";
    stream.write_all(caller_bytes).await.unwrap();
    stream.shutdown().await.unwrap();
    let mut received = Vec::new();
    tokio::time::timeout(PATIENCE, stream.read_to_end(&mut received))
        .await
        .unwrap()
        .unwrap();
    assert!(received.starts_with(b"prefix:"));
    let prefix = &received[b"prefix:".len()..];
    assert_eq!(&prefix[..12], b"\r\n\r\n\0\r\nQUIT\n");
    assert_eq!(prefix[12], 0x21);
    assert_eq!(prefix[13], 0x11);
    assert_eq!(u16::from_be_bytes([prefix[14], prefix[15]]), 12);
    assert_eq!(&prefix[16..20], &[192, 0, 2, 41]);
    assert_eq!(&prefix[20..24], &[127, 0, 0, 1]);
    assert_eq!(u16::from_be_bytes([prefix[24], prefix[25]]), 41001);
    assert_eq!(u16::from_be_bytes([prefix[26], prefix[27]]), service.port);
    assert_eq!(&prefix[28..], caller_bytes);
    rig.stop().await;
}

#[cfg(target_os = "linux")]
#[tokio::test]
async fn an_environment_only_machine_publishes_its_directory_and_withdraws_live_sockets_under_the_same_ports_key()
 {
    use orch_api_contract::ports::{HostPorts, PortOpeningKind, PortRange};
    use orch_link_contract::policy::PolicyKey;
    use orch_link_contract::ports::{HostPortTemplate, PortPublish, PortPublishTicket};
    let probe = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let outside = probe.local_addr().unwrap();
    drop(probe);
    let mut rig = EnvRig::new();
    rig.config.ports.allowed = vec![PortOpeningKind::Host];
    rig.config.ports.publish = Some(HostPorts {
        incoming_address: "vm.example.test".into(),
        listen: "127.0.0.1".into(),
        ranges: vec![PortRange {
            from: outside.port(),
            to: outside.port(),
        }],
    });
    assert_eq!(
        rig.roles,
        vec![orch_link_contract::session::Role::Environment]
    );
    rig.start().await;
    let directory = rig.directory("published-directory");
    rig.attach(&directory).await;
    let connector = rig.connector.as_ref().unwrap();
    let env = connector
        .environment_role()
        .unwrap()
        .host()
        .environment(rig.environment)
        .unwrap();
    let service = Service::start(Some(env.processes()), 0, "");
    let mut available = connector.link().port_control();
    tokio::time::timeout(PATIENCE, available.wait_for(|ready| *ready))
        .await
        .unwrap()
        .unwrap();
    let opening = new_id();
    let mut claims = orch_link_contract::tunnels::Ticket::parse(&ticket_for(
        &rig,
        rig.machine_id,
        service.port,
        PortTransport::Tcp,
        Some(PortOpeningKind::Host),
    ))
    .unwrap()
    .claims()
    .clone();
    let port = claims.port.as_mut().unwrap();
    port.opening_id = Some(opening);
    port.allocation_revision = Some(1);
    port.host_template = Some(HostPortTemplate {
        host_id: rig.machine_id,
        proxy_protocol: false,
    });
    let body = PortPublish {
        opening_id: opening,
        environment_id: rig.environment,
        environment_machine_id: rig.machine_id,
        generation: env.facts().generation,
        port: service.port,
        transports: vec![PortTransport::Tcp],
        incoming_address: "vm.example.test".into(),
        listen: "127.0.0.1".into(),
        host_port: outside.port(),
        revision: 1,
        allocation_revision: 1,
        proxy_protocol: false,
        tickets: vec![PortPublishTicket {
            transport: PortTransport::Tcp,
            ticket: rig.signer.sign(&claims).unwrap(),
        }],
    };
    assert_eq!(rig.succeed(&body).await["state"], "open");
    let mut caller = tokio::net::TcpStream::connect(outside).await.unwrap();
    caller.write_all(b"directory").await.unwrap();
    let mut received = [0; 9];
    tokio::time::timeout(PATIENCE, caller.read_exact(&mut received))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(&received, b"directory");
    connector
        .services()
        .policy
        .set_here(
            PolicyKey::Ports,
            &serde_json::json!({"allowed":[],"publish":null}),
        )
        .unwrap();
    assert!(
        rig.controller
            .wait_until(PATIENCE, |controller| controller
                .applied("machine")
                .iter()
                .any(|event| event.kind == "port.state"))
            .await
    );
    let mut byte = [0; 1];
    let ended = tokio::time::timeout(PATIENCE, caller.read(&mut byte))
        .await
        .unwrap();
    assert!(
        ended.is_err() || ended.unwrap() == 0,
        "a kept ports refusal closes an existing caller"
    );
    assert!(tokio::net::TcpStream::connect(outside).await.is_err());
    let (_, reason) = rig.failure(&body).await;
    assert!(
        reason.contains("ports") && reason.contains("environment") && reason.contains("operator"),
        "{reason}"
    );
    rig.stop().await;
}

#[cfg(target_os = "linux")]
#[tokio::test]
async fn the_carrier_rechecks_the_signed_audience_and_refuses_an_unlabelled_wider_opening() {
    use orch_api_contract::ports::PortOpeningKind as Audience;
    let mut rig = EnvRig::new();
    rig.config.ports.allowed = vec![Audience::Place];
    rig.start().await;
    let directory = rig.directory("audiences");
    rig.attach(&directory).await;
    let env = rig
        .connector
        .as_ref()
        .unwrap()
        .environment_role()
        .unwrap()
        .host()
        .environment(rig.environment)
        .unwrap();
    let server = Service::start(Some(env.processes()), 0, "");
    let opener = opener_ready(&rig).await;
    let refused = opener
        .open(&ticket_for(
            &rig,
            opener.machine_id,
            server.port,
            PortTransport::Tcp,
            Some(Audience::Link),
        ))
        .await
        .unwrap_err();
    assert!(refused.to_string().contains("ports"), "{refused}");
    assert!(
        opener
            .open(&ticket_for(
                &rig,
                opener.machine_id,
                server.port,
                PortTransport::Tcp,
                None
            ))
            .await
            .is_err()
    );
    let mut place = opener
        .open(&ticket_for(
            &rig,
            opener.machine_id,
            server.port,
            PortTransport::Tcp,
            Some(Audience::Place),
        ))
        .await
        .unwrap();
    rig.connector
        .as_ref()
        .unwrap()
        .services()
        .policy
        .set_here(
            orch_link_contract::policy::PolicyKey::Ports,
            &serde_json::json!({"allowed":["link"],"publish":null}),
        )
        .unwrap();
    let mut buffer = [0; 1];
    let ended = tokio::time::timeout(PATIENCE, place.read(&mut buffer))
        .await
        .unwrap();
    assert!(ended.is_err() || ended.unwrap() == 0);
    let link = opener
        .open(&ticket_for(
            &rig,
            opener.machine_id,
            server.port,
            PortTransport::Tcp,
            Some(Audience::Link),
        ))
        .await
        .unwrap();
    drop(link);
    opener.machine.link.shutdown().await;
    rig.stop().await;
}

#[cfg(target_os = "linux")]
#[tokio::test]
async fn rule_2_tcp_half_close_udp_empty_two_senders_and_owned_replacement_use_the_same_real_connector_link()
 {
    let mut rig = EnvRig::new();
    rig.config.ports.allowed = vec![
        orch_api_contract::ports::PortOpeningKind::Place,
        orch_api_contract::ports::PortOpeningKind::Link,
    ];
    rig.start().await;
    let directory = rig.directory("ports");
    rig.attach(&directory).await;
    let env = rig
        .connector
        .as_ref()
        .unwrap()
        .environment_role()
        .unwrap()
        .host()
        .environment(rig.environment)
        .unwrap();
    let mut server = Service::start(Some(env.processes()), 0, "first:");
    let port = server.port;
    let opener = opener_ready(&rig).await;
    let mut tcp = opener
        .open(&ticket(&rig, opener.machine_id, port, PortTransport::Tcp))
        .await
        .unwrap();
    assert!(tcp.inside_peer().is_some());
    tcp.write_all(b"hello").await.unwrap();
    tcp.shutdown().await.unwrap();
    let mut bytes = Vec::new();
    tokio::time::timeout(PATIENCE, tcp.read_to_end(&mut bytes))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(bytes, b"first:hello");
    let udp1 = opener
        .open(&ticket(&rig, opener.machine_id, port, PortTransport::Udp))
        .await
        .unwrap();
    let udp2 = opener
        .open(&ticket(&rig, opener.machine_id, port, PortTransport::Udp))
        .await
        .unwrap();
    assert_ne!(udp1.inside_peer(), udp2.inside_peer());
    assert!(udp1.send_datagram(Bytes::new()).unwrap());
    assert!(udp2.send_datagram(Bytes::from_static(b"second")).unwrap());
    assert_eq!(
        tokio::time::timeout(PATIENCE, udp1.recv_datagram())
            .await
            .unwrap()
            .unwrap(),
        Bytes::new()
    );
    assert_eq!(
        tokio::time::timeout(PATIENCE, udp2.recv_datagram())
            .await
            .unwrap()
            .unwrap(),
        Bytes::from_static(b"second")
    );
    server.stop();
    let mut replacement = Service::start(Some(env.processes()), port, "replacement:");
    let mut tcp = opener
        .open(&ticket(&rig, opener.machine_id, port, PortTransport::Tcp))
        .await
        .unwrap();
    tcp.write_all(b"next").await.unwrap();
    tcp.shutdown().await.unwrap();
    let mut bytes = Vec::new();
    tokio::time::timeout(PATIENCE, tcp.read_to_end(&mut bytes))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(bytes, b"replacement:next");
    replacement.stop();
    let foreign = Service::start(None, port, "foreign:");
    assert!(
        opener
            .open(&ticket(&rig, opener.machine_id, port, PortTransport::Tcp))
            .await
            .is_err()
    );
    drop((foreign, udp1, udp2));
    opener.machine.link.shutdown().await;
    rig.stop().await;
}

#[cfg(target_os = "linux")]
#[tokio::test]
async fn rule_2_loss_of_current_control_closes_tcp_and_udp_and_local_policy_refuses_new_ingress() {
    let mut rig = EnvRig::new();
    rig.config.ports.allowed = vec![
        orch_api_contract::ports::PortOpeningKind::Place,
        orch_api_contract::ports::PortOpeningKind::Link,
    ];
    rig.config.environment.terminal = true;
    rig.start().await;
    let directory = rig.directory("ports");
    rig.attach(&directory).await;
    let env = rig
        .connector
        .as_ref()
        .unwrap()
        .environment_role()
        .unwrap()
        .host()
        .environment(rig.environment)
        .unwrap();
    let server = Service::start(Some(env.processes()), 0, "");
    let opener = opener_ready(&rig).await;
    let mut tcp = opener
        .open(&ticket(
            &rig,
            opener.machine_id,
            server.port,
            PortTransport::Tcp,
        ))
        .await
        .unwrap();
    let udp = opener
        .open(&ticket(
            &rig,
            opener.machine_id,
            server.port,
            PortTransport::Udp,
        ))
        .await
        .unwrap();
    let terminal_id = new_id();
    rig.succeed(&orch_link_contract::commands::PtyOpen {
        environment_id: rig.environment,
        terminal_id,
        cols: 80,
        rows: 24,
        cwd: String::new(),
        record: false,
    })
    .await;
    let shell_ticket = rig.ticket(
        TicketSource::Api,
        orch_link_contract::tunnels::Service::Pty { terminal_id },
        None,
        None,
    );
    let mut shell = rig
        .hub
        .terminals()
        .open(rig.machine_id, &shell_ticket)
        .await
        .unwrap();
    rig.hub.set_api_down(true);
    tokio::time::timeout(PATIENCE, async {
        while !tcp.is_closed() || !udp.is_closed() {
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();
    assert!(tcp.write_all(b"no").await.is_err());
    shell
        .write_all(b"echo preserved-$((20+22))\n")
        .await
        .unwrap();
    tokio::time::timeout(PATIENCE, async {
        let mut seen = Vec::new();
        let mut buffer = [0; 1024];
        while !String::from_utf8_lossy(&seen).contains("preserved-42") {
            let n = shell.read(&mut buffer).await.unwrap();
            assert!(n > 0);
            seen.extend_from_slice(&buffer[..n]);
        }
    })
    .await
    .unwrap();
    rig.hub.set_api_down(false);
    tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    let policy = rig.connector.as_ref().unwrap().services().policy.clone();
    policy
        .set_here(
            orch_link_contract::policy::PolicyKey::Ports,
            &serde_json::json!({"allowed":[],"publish":null}),
        )
        .unwrap();
    assert!(
        opener
            .open(&ticket(
                &rig,
                opener.machine_id,
                server.port,
                PortTransport::Tcp
            ))
            .await
            .is_err()
    );
    drop((server, shell));
    opener.machine.link.shutdown().await;
    rig.stop().await;
}

#[cfg(target_os = "linux")]
#[tokio::test]
async fn rule_2_native_udp_sender_limit_and_sixty_second_idle_are_shared_across_targets() {
    let mut rig = EnvRig::new();
    rig.config.ports.allowed = vec![
        orch_api_contract::ports::PortOpeningKind::Place,
        orch_api_contract::ports::PortOpeningKind::Link,
    ];
    rig.start().await;
    let directory = rig.directory("ports");
    rig.attach(&directory).await;
    let env = rig
        .connector
        .as_ref()
        .unwrap()
        .environment_role()
        .unwrap()
        .host()
        .environment(rig.environment)
        .unwrap();
    let server = Service::start(Some(env.processes()), 0, "");
    let opener = opener_ready(&rig).await;
    use futures::StreamExt;
    let tickets = (0..orch_link::ports::UDP_SENDERS)
        .map(|_| ticket(&rig, opener.machine_id, server.port, PortTransport::Udp))
        .collect::<Vec<_>>();
    let associations = futures::stream::iter(tickets)
        .map(|ticket| {
            let opener = &opener;
            async move { opener.open(&ticket).await }
        })
        .buffer_unordered(16)
        .collect::<Vec<_>>()
        .await
        .into_iter()
        .map(Result::unwrap)
        .collect::<Vec<_>>();
    assert!(
        associations.iter().all(|channel| !channel.is_closed()),
        "all 128 current sender sockets are live together"
    );
    assert!(
        opener
            .open(&ticket(
                &rig,
                opener.machine_id,
                server.port,
                PortTransport::Udp
            ))
            .await
            .is_err()
    );
    // A real sixty seconds of silence must release every last-hop socket.
    tokio::time::timeout(std::time::Duration::from_secs(65), async {
        while associations.iter().any(|channel| !channel.is_closed()) {
            tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        }
    })
    .await
    .unwrap();
    let fresh = opener
        .open(&ticket(
            &rig,
            opener.machine_id,
            server.port,
            PortTransport::Udp,
        ))
        .await
        .unwrap();
    assert!(
        fresh
            .send_datagram(Bytes::from_static(b"after idle"))
            .unwrap()
    );
    assert_eq!(
        tokio::time::timeout(PATIENCE, fresh.recv_datagram())
            .await
            .unwrap()
            .unwrap(),
        Bytes::from_static(b"after idle")
    );
    drop((fresh, associations, server));
    opener.machine.link.shutdown().await;
    rig.stop().await;
}

#[cfg(target_os = "linux")]
#[tokio::test]
async fn rule_2_sixty_four_real_tcp_connections_and_sixteen_waiters_share_one_environment_and_one_deadline()
 {
    use futures::StreamExt;
    let mut rig = EnvRig::new();
    rig.config.ports.allowed = vec![
        orch_api_contract::ports::PortOpeningKind::Place,
        orch_api_contract::ports::PortOpeningKind::Link,
    ];
    rig.start().await;
    let directory = rig.directory("ports");
    rig.attach(&directory).await;
    let env = rig
        .connector
        .as_ref()
        .unwrap()
        .environment_role()
        .unwrap()
        .host()
        .environment(rig.environment)
        .unwrap();
    let server = Service::start(Some(env.processes()), 0, "");
    let opener = opener_ready(&rig).await;
    let tickets = (0..orch_link::ports::TCP_CONNECTIONS)
        .map(|_| ticket(&rig, opener.machine_id, server.port, PortTransport::Tcp))
        .collect::<Vec<_>>();
    let mut live = futures::stream::iter(tickets)
        .map(|ticket| {
            let opener = &opener;
            async move { opener.open(&ticket).await }
        })
        .buffer_unordered(16)
        .collect::<Vec<_>>()
        .await
        .into_iter()
        .map(Result::unwrap)
        .collect::<Vec<_>>();
    let mut waiting = tokio::task::JoinSet::new();
    for _ in 0..orch_link::ports::TCP_WAITING {
        let link = opener.machine.link.clone();
        let ticket = ticket(&rig, opener.machine_id, server.port, PortTransport::Tcp);
        waiting.spawn(async move { link.open_tunnel(&ticket).await });
    }
    tokio::time::timeout(PATIENCE, async {
        while rig
            .hub
            .with_relay(|relay| relay.channels_of(rig.machine_id))
            != 80
        {
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();
    let excess = opener
        .open(&ticket(
            &rig,
            opener.machine_id,
            server.port,
            PortTransport::Tcp,
        ))
        .await
        .unwrap_err();
    assert!(matches!(
        excess,
        orch_link::tunnels::channel::OpenError::Refused {
            code: orch_link_contract::tunnels::OpenErrorCode::TooManyChannels,
            ..
        }
    ));
    drop(live.pop());
    let next = tokio::time::timeout(PATIENCE, waiting.join_next())
        .await
        .unwrap()
        .unwrap()
        .unwrap()
        .unwrap();
    assert!(next.inside_peer().is_some());
    // Every remaining accepted waiter ends at the ordinary ten-second open deadline.
    let began = tokio::time::Instant::now();
    while let Some(result) = waiting.join_next().await {
        assert!(result.unwrap().is_err());
    }
    assert!(began.elapsed() < std::time::Duration::from_secs(12));
    drop((next, live, server));
    opener.machine.link.shutdown().await;
    rig.stop().await;
}

#[cfg(target_os = "linux")]
#[tokio::test]
async fn rule_2_a_hundred_mebibytes_in_both_tcp_directions_are_byte_exact_with_bounded_shared_credit()
 {
    let mut rig = EnvRig::new();
    rig.config.ports.allowed = vec![
        orch_api_contract::ports::PortOpeningKind::Place,
        orch_api_contract::ports::PortOpeningKind::Link,
    ];
    rig.start().await;
    let directory = rig.directory("ports");
    rig.attach(&directory).await;
    let env = rig
        .connector
        .as_ref()
        .unwrap()
        .environment_role()
        .unwrap()
        .host()
        .environment(rig.environment)
        .unwrap();
    let server = Service::start(Some(env.processes()), 0, "");
    let opener = opener_ready(&rig).await;
    let tcp = opener
        .open(&ticket(
            &rig,
            opener.machine_id,
            server.port,
            PortTransport::Tcp,
        ))
        .await
        .unwrap();
    let (mut read, mut write) = tokio::io::split(tcp);
    const TOTAL: usize = 100 * 1024 * 1024;
    const CHUNK: usize = 64 * 1024;
    let sender = async {
        for n in 0..TOTAL / CHUNK {
            write
                .write_all(&vec![(n % 251) as u8; CHUNK])
                .await
                .unwrap();
        }
        write.shutdown().await.unwrap();
    };
    let receiver = async {
        let mut position = 0;
        let mut buffer = vec![0; CHUNK];
        loop {
            let n = read.read(&mut buffer).await.unwrap();
            if n == 0 {
                break;
            }
            for (i, b) in buffer[..n].iter().enumerate() {
                assert_eq!(*b, (((position + i) / CHUNK) % 251) as u8);
            }
            position += n;
        }
        assert_eq!(position, TOTAL);
    };
    tokio::time::timeout(PATIENCE, async {
        tokio::join!(sender, receiver);
    })
    .await
    .unwrap();
    drop(server);
    opener.machine.link.shutdown().await;
    rig.stop().await;
}

#[cfg(target_os = "linux")]
async fn slow_native_transfer(up: u64, down: u64) {
    use orch_testkit::doubles::link::{Direction, Shape};
    use std::time::Duration;
    let mut rig = EnvRig::new();
    rig.config.ports.allowed = vec![
        orch_api_contract::ports::PortOpeningKind::Place,
        orch_api_contract::ports::PortOpeningKind::Link,
    ];
    rig.start().await;
    let directory = rig.directory("ports");
    rig.attach(&directory).await;
    let env = rig
        .connector
        .as_ref()
        .unwrap()
        .environment_role()
        .unwrap()
        .host()
        .environment(rig.environment)
        .unwrap();
    let server = Service::start(Some(env.processes()), 0, "");
    let opener = opener_ready(&rig).await;
    let tcp = opener
        .open(&ticket(
            &rig,
            opener.machine_id,
            server.port,
            PortTransport::Tcp,
        ))
        .await
        .unwrap();
    let udp = opener
        .open(&ticket(
            &rig,
            opener.machine_id,
            server.port,
            PortTransport::Udp,
        ))
        .await
        .unwrap();
    rig.hub.faults().shape(
        Direction::Up,
        Some(Shape {
            delay: Duration::from_millis(150),
            bytes_per_second: up,
            loss: 0.01,
        }),
    );
    rig.hub.faults().shape(
        Direction::Down,
        Some(Shape {
            delay: Duration::from_millis(150),
            bytes_per_second: down,
            loss: 0.01,
        }),
    );
    // Only the transport clock is accelerated. The connector and the owned echo sockets are real.
    tokio::time::pause();
    let (mut read, mut write) = tokio::io::split(tcp);
    const TOTAL: usize = 100 * 1024 * 1024;
    const CHUNK: usize = 64 * 1024;
    let sender = async {
        for n in 0..TOTAL / CHUNK {
            write
                .write_all(&vec![(n % 251) as u8; CHUNK])
                .await
                .unwrap();
        }
        write.shutdown().await.unwrap();
    };
    let receiver = async {
        let mut position = 0;
        let mut buffer = vec![0; CHUNK];
        loop {
            let n = read.read(&mut buffer).await.unwrap();
            if n == 0 {
                break;
            }
            for (i, b) in buffer[..n].iter().enumerate() {
                assert_eq!(*b, (((position + i) / CHUNK) % 251) as u8);
            }
            position += n;
        }
        assert_eq!(position, TOTAL);
    };
    let control = async {
        tokio::time::sleep(Duration::from_secs(5)).await;
        let mut dropped = 0;
        for _ in 0..1000 {
            if !udp.send_datagram(Bytes::from(vec![4; 4000])).unwrap() {
                dropped += 1;
            }
        }
        assert!(dropped > 0, "UDP saturation takes no credit wait");
        assert!(udp.datagram_budget().unwrap().dropped().full > 0);
        let id = new_id();
        rig.controller.issue_with_id(
            id,
            &orch_link_contract::commands::SerialKey::Machine,
            rig.deadline(),
            &orch_link_contract::commands::MachineProbe {},
        );
        let sent = tokio::time::Instant::now();
        tokio::time::timeout(Duration::from_secs(2), async {
            while rig.controller.progress(id).is_none_or(|p| {
                matches!(
                    p,
                    orch_link::commands::Progress::Queued | orch_link::commands::Progress::Sent
                )
            }) {
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
        assert!(
            sent.elapsed() <= Duration::from_millis(1500),
            "control beside saturated UDP and TCP: {:?}",
            sent.elapsed()
        );
    };
    let mut moved = Box::pin(async {
        tokio::join!(sender, receiver, control);
    });
    let begun = tokio::time::Instant::now();
    let maximum = Duration::from_secs((TOTAL as u64 / up) * 4 + 120);
    loop {
        tokio::select! {
            ()=&mut moved=>break,
            ()=async {tokio::task::yield_now().await;std::thread::sleep(Duration::from_micros(100));tokio::time::advance(Duration::from_millis(10)).await;}=>{}
        }
        assert!(
            begun.elapsed() < maximum,
            "the owned slow transfer did not complete: {:?}",
            begun.elapsed()
        );
    }
    assert!(
        begun.elapsed() >= Duration::from_secs(TOTAL as u64 / up),
        "the transport obeys its rate"
    );
    assert!(
        rig.hub.faults().losses() > 0,
        "one-percent retransmission delay was exercised"
    );
    drop(moved);
    tokio::time::resume();
    drop((udp, server));
    opener.machine.link.shutdown().await;
    rig.stop().await;
}

#[cfg(target_os = "linux")]
#[tokio::test]
async fn rule_2_owned_tcp_100_mib_each_way_and_control_beside_full_udp_on_the_slow_profile() {
    slow_native_transfer(93_750, 200_000).await;
}
#[cfg(target_os = "linux")]
#[tokio::test]
async fn rule_2_owned_tcp_100_mib_each_way_and_control_beside_full_udp_at_a_tenth_rate() {
    slow_native_transfer(9_375, 20_000).await;
}

#[cfg(target_os = "linux")]
#[tokio::test]
async fn rule_2_a_private_unit_carries_loopback_tcp_and_udp_without_an_independent_outbound_route()
{
    const CASE: &str =
        "rule_2_a_private_unit_carries_loopback_tcp_and_udp_without_an_independent_outbound_route";
    if std::env::var("ORCH_PORTS_RESTRICTED_FIXTURE").as_deref() != Ok("1") {
        // The same test binary runs in an actual private network namespace, with no published port.
        let executable = std::env::current_exe().unwrap();
        let result = Command::new("docker")
            .args([
                "run",
                "--rm",
                "--name",
                "orchprobe-ports-door-restricted",
                "--network",
                "none",
                "--mount",
            ])
            .arg(format!(
                "type=bind,source={},target=/ports-test,readonly",
                executable.display()
            ))
            .args([
                "--env",
                "ORCH_PORTS_RESTRICTED_FIXTURE=1",
                "--entrypoint",
                "/ports-test",
                "python:3.13-slim-trixie@sha256:70729b46c69b4f1e97c4822c1af3df53a1476cf5ddc6c087c0c10bc3a5678c2f",
                "--exact",
                CASE,
                "--nocapture",
            ])
            .output()
            .unwrap();
        assert!(
            result.status.success(),
            "private unit: {}{}",
            String::from_utf8_lossy(&result.stdout),
            String::from_utf8_lossy(&result.stderr)
        );
        return;
    }
    let outbound: std::net::SocketAddr = "192.0.2.1:41055".parse().unwrap();
    assert!(
        std::net::TcpStream::connect_timeout(&outbound, std::time::Duration::from_millis(250))
            .is_err()
    );
    let outside = std::net::UdpSocket::bind("127.0.0.1:0").unwrap();
    assert!(outside.connect(outbound).is_err());
    let mut rig = EnvRig::new();
    rig.config.ports.allowed = vec![
        orch_api_contract::ports::PortOpeningKind::Place,
        orch_api_contract::ports::PortOpeningKind::Link,
    ];
    rig.start().await;
    let directory = rig.directory("private-ports");
    rig.attach(&directory).await;
    let env = rig
        .connector
        .as_ref()
        .unwrap()
        .environment_role()
        .unwrap()
        .host()
        .environment(rig.environment)
        .unwrap();
    let server = Service::start(Some(env.processes()), 0, "");
    let opener = opener_ready(&rig).await;
    let mut tcp = opener
        .open(&ticket(
            &rig,
            opener.machine_id,
            server.port,
            PortTransport::Tcp,
        ))
        .await
        .unwrap();
    tcp.write_all(b"inside the unit").await.unwrap();
    tcp.shutdown().await.unwrap();
    let mut bytes = Vec::new();
    tokio::time::timeout(PATIENCE, tcp.read_to_end(&mut bytes))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(&bytes, b"inside the unit");
    let udp = opener
        .open(&ticket(
            &rig,
            opener.machine_id,
            server.port,
            PortTransport::Udp,
        ))
        .await
        .unwrap();
    assert!(
        udp.send_datagram(Bytes::from_static(b"inside UDP"))
            .unwrap()
    );
    assert_eq!(
        tokio::time::timeout(PATIENCE, udp.recv_datagram())
            .await
            .unwrap()
            .unwrap(),
        Bytes::from_static(b"inside UDP")
    );
    drop((udp, server));
    opener.machine.link.shutdown().await;
    rig.stop().await;
}
