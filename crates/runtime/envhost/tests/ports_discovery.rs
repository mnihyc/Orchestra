//! Real TCP and UDP bindings keep current scoped ownership, attribution, first-seen times and Internal endpoints.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]
#![cfg(target_os = "linux")]

use std::io::{BufRead, BufReader, Read, Write};
use std::net::{SocketAddr, TcpStream, UdpSocket};
use std::process::{Child, Command, Stdio};
use std::sync::Arc;
use std::time::Duration;

use orch_api_contract::ports::{PortBinding, PortDiscovery, PortTransport};
use orch_envhost::ports::{PortReadError, PortScope, Ports};
use orch_envhost::ports_sockets;
use orch_envhost::processes::{Processes, RootKind, ScopeKind};
use orch_executor_contract::descriptor::Attribution;
use orch_types::{SystemClock, new_id};

// These trusted fixtures listen only in the run's allowed port interval. Binding itself chooses
// a free number atomically, avoiding a separate check which another worker could race.
const PROGRAM: &str = r#"
import json, random, socket, sys, threading
addresses = json.loads(sys.argv[1])
fixed = int(sys.argv[2])
mode = sys.argv[3]
tag = sys.argv[4].encode()
for number in ([fixed] if fixed else random.sample(range(41000,42000),1000)):
    opened=[]
    try:
        for address in addresses:
            family = socket.AF_INET6 if ':' in address else socket.AF_INET
            for transport in ([socket.SOCK_STREAM, socket.SOCK_DGRAM] if mode=='both' else [socket.SOCK_DGRAM if mode=='udp' else socket.SOCK_STREAM]):
                server=socket.socket(family,transport)
                opened.append(server)
                if family==socket.AF_INET6: server.setsockopt(socket.IPPROTO_IPV6,socket.IPV6_V6ONLY,1)
                server.setsockopt(socket.SOL_SOCKET,socket.SO_REUSEADDR,1)
                server.bind((address,number))
                if transport==socket.SOCK_STREAM: server.listen()
        break
    except OSError:
        for server in opened: server.close()
        if fixed: raise
else: raise RuntimeError('no free fixture number in the allowed interval')
def serve(server):
    if server.type==socket.SOCK_DGRAM:
        while True:
            data,peer=server.recvfrom(65535)
            server.sendto(data,peer)
    else:
        def connection(client):
            data=b''
            while True:
                chunk=client.recv(65535)
                if not chunk: break
                data+=chunk
            client.sendall(tag+data)
            client.shutdown(socket.SHUT_WR)
            client.close()
        while True:
            client,peer=server.accept()
            threading.Thread(target=connection,args=(client,),daemon=True).start()
for server in opened: threading.Thread(target=serve,args=(server,),daemon=True).start()
print(json.dumps({'port':number}),flush=True)
sys.stdin.readline()
"#;

struct Service {
    child: Child,
    port: u16,
}

impl Service {
    fn start(
        processes: Option<&Processes>,
        addresses: &[&str],
        port: u16,
        transport: &str,
        tag: &str,
        thread: Option<orch_types::Id>,
        item: Option<orch_types::Id>,
    ) -> Self {
        let mut command = Command::new("python3");
        command.args(["-I", "-B", "-u", "-c", PROGRAM]);
        command
            .arg(serde_json::to_string(addresses).unwrap())
            .arg(port.to_string())
            .arg(transport)
            .arg(tag)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        if let Some(thread) = thread {
            command.env("ORCH_THREAD_ID", thread.to_string());
        }
        if let Some(item) = item {
            command.env("ORCH_ITEM_ID", item.to_string());
        }
        let mut child = match processes {
            Some(processes) => processes
                .spawn_tracked(&mut command, RootKind::Executor)
                .unwrap(),
            None => {
                orch_envhost::platform::prepare_group(&mut command);
                command.spawn().unwrap()
            }
        };
        let mut stdout = BufReader::new(child.stdout.take().unwrap());
        let mut line = String::new();
        stdout.read_line(&mut line).unwrap();
        if line.is_empty() {
            let mut error = String::new();
            child
                .stderr
                .take()
                .unwrap()
                .read_to_string(&mut error)
                .unwrap();
            child.wait().unwrap();
            panic!("fixture did not bind: {error}");
        }
        let row: serde_json::Value = serde_json::from_str(&line).unwrap();
        Self {
            child,
            port: u16::try_from(row["port"].as_u64().unwrap()).unwrap(),
        }
    }

    fn stop(&mut self) {
        self.child.kill().unwrap();
        self.child.wait().unwrap();
    }
}

impl Drop for Service {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

fn reader() -> (Processes, Ports) {
    let clock = Arc::new(SystemClock);
    let processes = Processes::new(ScopeKind::Started, clock.clone());
    processes.add_attribution(Attribution {
        thread_id: "ORCH_THREAD_ID".into(),
        tool_call_id: "ORCH_ITEM_ID".into(),
    });
    let ports = Ports::new(new_id(), 3, PortScope::Processes, processes.clone(), clock);
    (processes, ports)
}

#[test]
fn current_tcp_listeners_and_bound_udp_are_native_scoped_ipv4_and_ipv6_observations() {
    let (processes, ports) = reader();
    let (thread, item) = (new_id(), new_id());
    let owned = Service::start(
        Some(&processes),
        &["127.0.0.1", "::1"],
        0,
        "both",
        "",
        Some(thread),
        Some(item),
    );
    let foreign = Service::start(None, &["127.0.0.1"], 0, "both", "", None, None);
    let snapshot = ports.look();
    assert_eq!(snapshot.discovery, PortDiscovery::Native);
    assert!(snapshot.reason.is_none());
    assert!(
        !snapshot
            .listeners
            .iter()
            .any(|row| row.port == foreign.port)
    );
    let rows: Vec<_> = snapshot
        .listeners
        .iter()
        .filter(|row| row.port == owned.port)
        .collect();
    assert_eq!(rows.len(), 4);
    for row in rows {
        assert_eq!(row.generation, 3);
        assert_eq!(row.first_seen_at, snapshot.as_of);
        let program = row.process.as_ref().unwrap();
        assert_eq!(program.pid, owned.child.id());
        assert_eq!(program.thread_id, Some(thread));
        assert_eq!(program.item_id, Some(item));
        assert!(!row.internal);
        assert!(row.unavailable_reason.is_none());
        assert_eq!(
            row.binding,
            match row.transport {
                PortTransport::Tcp => PortBinding::Listening,
                PortTransport::Udp => PortBinding::BoundUnknown,
            }
        );
    }
    let again = ports.look();
    for before in snapshot
        .listeners
        .iter()
        .filter(|row| row.port == owned.port)
    {
        let after = again
            .listeners
            .iter()
            .find(|row| row.observation_id == before.observation_id)
            .unwrap();
        assert_eq!(
            before.first_seen_at, after.first_seen_at,
            "a look does not invent a socket creation time"
        );
    }
}

#[test]
fn owned_replacements_on_the_same_port_work_and_a_foreign_replacement_is_refused() {
    let (processes, ports) = reader();
    let mut original = Service::start(
        Some(&processes),
        &["127.0.0.1"],
        0,
        "tcp",
        "first:",
        None,
        None,
    );
    let number = original.port;
    let proof = ports.owned(number, PortTransport::Tcp).unwrap();
    assert_eq!(exchange(proof.address, b"abc"), b"first:abc");
    original.stop();
    let mut replacement = Service::start(
        Some(&processes),
        &["127.0.0.1"],
        number,
        "tcp",
        "next:",
        None,
        None,
    );
    assert!(matches!(ports.recheck(&proof), Err(PortReadError::Changed)));
    let current = ports.owned(number, PortTransport::Tcp).unwrap();
    assert_eq!(exchange(current.address, b"abc"), b"next:abc");
    replacement.stop();
    let foreign = Service::start(None, &["127.0.0.1"], number, "tcp", "foreign:", None, None);
    assert!(matches!(
        ports.owned(number, PortTransport::Tcp),
        Err(PortReadError::NotListening)
    ));
    assert!(
        !ports
            .look()
            .listeners
            .iter()
            .any(|row| row.port == foreign.port)
    );
}

fn exchange(address: SocketAddr, bytes: &[u8]) -> Vec<u8> {
    let mut stream = TcpStream::connect(address).unwrap();
    stream
        .set_read_timeout(Some(Duration::from_secs(5)))
        .unwrap();
    stream.write_all(bytes).unwrap();
    stream.shutdown(std::net::Shutdown::Write).unwrap();
    let mut reply = Vec::new();
    stream.read_to_end(&mut reply).unwrap();
    reply
}

#[test]
fn every_caller_prefers_owned_ipv4_loopback_before_ipv6_and_reaches_an_owned_wildcard_by_loopback()
{
    let (processes, ports) = reader();
    let both = Service::start(
        Some(&processes),
        &["127.0.0.1", "::1"],
        0,
        "both",
        "",
        None,
        None,
    );
    for transport in [PortTransport::Tcp, PortTransport::Udp] {
        assert_eq!(
            ports.owned(both.port, transport).unwrap().address,
            SocketAddr::from(([127, 0, 0, 1], both.port))
        );
    }
    let wildcard = Service::start(
        Some(&processes),
        &["0.0.0.0"],
        0,
        "tcp",
        "wild:",
        None,
        None,
    );
    let current = ports.owned(wildcard.port, PortTransport::Tcp).unwrap();
    assert_eq!(current.address.ip().to_string(), "127.0.0.1");
    assert_eq!(exchange(current.address, b"ok"), b"wild:ok");
}

#[test]
fn internal_control_and_carrier_sockets_cannot_be_opened_and_their_registration_ends_with_the_socket()
 {
    let (processes, ports) = reader();
    let server = Service::start(Some(&processes), &["127.0.0.1"], 0, "both", "", None, None);
    let tcp = ports.internal_port(server.port, PortTransport::Tcp);
    let udp = ports.internal_port(server.port, PortTransport::Udp);
    for transport in [PortTransport::Tcp, PortTransport::Udp] {
        assert!(matches!(
            ports.owned(server.port, transport),
            Err(PortReadError::Internal)
        ));
    }
    for row in ports
        .look()
        .listeners
        .iter()
        .filter(|row| row.port == server.port)
    {
        assert!(row.internal);
        assert!(row.unavailable_reason.is_some());
    }
    drop((tcp, udp));
    assert!(ports.owned(server.port, PortTransport::Tcp).is_ok());
    assert!(ports.owned(server.port, PortTransport::Udp).is_ok());
    processes.protect(server.child.id()).unwrap();
    assert!(matches!(
        ports.owned(server.port, PortTransport::Udp),
        Err(PortReadError::Internal)
    ));
}

#[test]
fn two_real_udp_senders_keep_distinct_inside_peers_and_zero_length_datagrams_remain_valid() {
    let (processes, ports) = reader();
    let server = Service::start(Some(&processes), &["127.0.0.1"], 0, "udp", "", None, None);
    let target = ports.owned(server.port, PortTransport::Udp).unwrap();
    // The test's senders are fixture sockets, in the allowed interval, chosen by atomic binds.
    let bind_sender = || {
        (41000..42000)
            .find_map(|port| UdpSocket::bind(("127.0.0.1", port)).ok())
            .unwrap()
    };
    let first = bind_sender();
    let second = bind_sender();
    for socket in [&first, &second] {
        socket.connect(target.address).unwrap();
        socket
            .set_read_timeout(Some(Duration::from_secs(5)))
            .unwrap();
    }
    assert_ne!(first.local_addr().unwrap(), second.local_addr().unwrap());
    first.send(b"one").unwrap();
    second.send(b"two").unwrap();
    let mut buffer = [0u8; 16];
    assert_eq!(first.recv(&mut buffer).unwrap(), 3);
    assert_eq!(&buffer[..3], b"one");
    assert_eq!(second.recv(&mut buffer).unwrap(), 3);
    assert_eq!(&buffer[..3], b"two");
    first.send(&[]).unwrap();
    assert_eq!(first.recv(&mut buffer).unwrap(), 0);
}

#[tokio::test]
async fn a_fresh_look_and_two_live_readers_share_current_native_bindings_and_first_seen_times() {
    let (processes, ports) = reader();
    let mut server = Service::start(Some(&processes), &["127.0.0.1"], 0, "both", "", None, None);
    let mut first = ports.watch();
    let mut second = ports.watch();
    first.changed().await.unwrap();
    second.changed().await.unwrap();
    let measured = first.borrow_and_update().clone().unwrap();
    assert_eq!(Some(&measured), second.borrow_and_update().as_ref());
    let looked = ports.look();
    for before in measured
        .listeners
        .iter()
        .filter(|row| row.port == server.port)
    {
        let after = looked
            .listeners
            .iter()
            .find(|row| row.observation_id == before.observation_id)
            .unwrap();
        assert_eq!(before.first_seen_at, after.first_seen_at);
    }
    server.stop();
    tokio::time::timeout(Duration::from_secs(15), first.changed())
        .await
        .unwrap()
        .unwrap();
    let gone = first.borrow_and_update().clone().unwrap();
    assert_eq!(gone.discovery, PortDiscovery::Native);
    assert!(!gone.listeners.iter().any(|row| row.port == server.port));
    assert!(gone.as_of.get() - measured.as_of.get() >= chrono::Duration::seconds(4));
    second.changed().await.unwrap();
    assert_eq!(Some(&gone), second.borrow_and_update().as_ref());
    drop((first, second));
}

#[tokio::test]
async fn the_inside_socket_factories_use_current_owned_programs_and_preserve_half_close_and_datagrams()
 {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let (processes, ports) = reader();
    let ports = Arc::new(ports);
    let mut server = Service::start(
        Some(&processes),
        &["127.0.0.1"],
        0,
        "both",
        "tcp:",
        None,
        None,
    );
    let mut stream = ports_sockets::tcp(&ports, server.port).await.unwrap();
    stream.write_all(b"whole bytes").await.unwrap();
    stream.shutdown().await.unwrap();
    let mut reply = Vec::new();
    tokio::time::timeout(Duration::from_secs(5), stream.read_to_end(&mut reply))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(reply, b"tcp:whole bytes");

    let first = ports_sockets::udp(ports.clone(), server.port)
        .await
        .unwrap();
    let second = ports_sockets::udp(ports.clone(), server.port)
        .await
        .unwrap();
    assert_ne!(first.inside_peer().unwrap(), second.inside_peer().unwrap());
    first.send(b"sender one").await.unwrap();
    second.send(b"sender two").await.unwrap();
    let mut buffer = [0u8; 32];
    let count = tokio::time::timeout(Duration::from_secs(5), first.recv(&mut buffer))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(&buffer[..count], b"sender one");
    let count = tokio::time::timeout(Duration::from_secs(5), second.recv(&mut buffer))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(&buffer[..count], b"sender two");
    first.send(&[]).await.unwrap();
    assert_eq!(
        tokio::time::timeout(Duration::from_secs(5), first.recv(&mut buffer))
            .await
            .unwrap()
            .unwrap(),
        0
    );
    assert!(
        ports
            .look()
            .listeners
            .iter()
            .any(|row| row.port == first.inside_peer().unwrap().port()
                && row.transport == PortTransport::Udp
                && row.internal)
    );

    let port = server.port;
    server.stop();
    let mut replacement = Service::start(
        Some(&processes),
        &["127.0.0.1"],
        port,
        "both",
        "new:",
        None,
        None,
    );
    let mut current = ports_sockets::tcp(&ports, port).await.unwrap();
    current.write_all(b"ok").await.unwrap();
    current.shutdown().await.unwrap();
    let mut reply = Vec::new();
    tokio::time::timeout(Duration::from_secs(5), current.read_to_end(&mut reply))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(reply, b"new:ok");
    first.send(b"owned replacement").await.unwrap();
    let count = tokio::time::timeout(Duration::from_secs(5), first.recv(&mut buffer))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(&buffer[..count], b"owned replacement");
    replacement.stop();
    let _foreign = Service::start(None, &["127.0.0.1"], port, "both", "foreign:", None, None);
    assert!(ports_sockets::tcp(&ports, port).await.is_err());
    assert!(
        first
            .send(b"must not reach the foreign program")
            .await
            .is_err()
    );
}
