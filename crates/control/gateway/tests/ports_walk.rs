//! A person and the agent backend walk port admission through a real database, gateway and connector.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]
#![cfg(target_os = "linux")]
mod ports_support;

use async_trait::async_trait;
use orch_api_contract::environment::{MachineEnvironmentCreate, MachineEnvironmentCreateBody};
use orch_api_contract::nodes::{OrgCreate, OrgCreateBody, ProjectCreate, ProjectCreateBody};
use orch_api_contract::ports::*;
use orch_api_core::backplane::{ApplyError, EventApplier};
use orch_api_core::registry::Registry;
use orch_api_core::testing::TestApp;
use orch_connector_core::connector::{ConnectorBuilder, ConnectorParams};
use orch_connector_core::roles::environment::EnvironmentParts;
use orch_link::streams::Delivery;
use orch_link_contract::session::{Arch, Os, Platform, Role};
use orch_types::{Id, new_id};
use std::io::{BufRead, BufReader};
use std::process::{Child, Command, Stdio};
use std::sync::Arc;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio_util::sync::CancellationToken;

struct Apply {
    engine: orch_api_core::state::App,
}

// Named failure double for the application read hook only; transport, carrier rows,
// environment ownership and successful assertion signing elsewhere in this test are real.
struct UnavailablePortReads;
#[async_trait]
impl orch_api_core::machine_requests::Handler for UnavailablePortReads {
    async fn import(
        &self,
        _machine: Id,
        request: orch_link_contract::mcp_import::McpImportAsk,
    ) -> orch_link_contract::mcp_import::McpImportAnswer {
        orch_link_contract::mcp_import::McpImportAnswer {
            request_id: request.request_id,
            server_id: None,
            code: Some("unavailable".into()),
            detail: "the test application hook is unavailable".into(),
        }
    }
}
#[async_trait]
impl EventApplier for Apply {
    async fn apply(
        &self,
        tx: &mut orch_db::Conn,
        machine: Id,
        delivery: &Delivery,
    ) -> Result<(), ApplyError> {
        self.engine
            .engine
            .apply_events(tx, machine, delivery)
            .await
            .map_err(|error| ApplyError::Other(error.to_string()))
    }
}
const PROGRAM: &str = r#"
import socket,threading,json,sys,random
fixed=int(sys.argv[1])
for p in ([fixed] if fixed else random.sample([p for p in range(41000,42000) if not 41280<=p<=41299],980)):
    s=socket.socket()
    try: s.setsockopt(socket.SOL_SOCKET,socket.SO_REUSEADDR,1); s.bind(('127.0.0.1',p)); s.listen(); break
    except OSError:
        s.close()
        if fixed: raise
def one(c):
    while True:
        b=c.recv(65536)
        if not b: break
        c.sendall(b)
    c.close()
def run():
    while True:
        c,_=s.accept(); threading.Thread(target=one,args=(c,),daemon=True).start()
u=socket.socket(socket.AF_INET,socket.SOCK_DGRAM); u.bind(('127.0.0.1',p))
def datagrams():
    while True:
        b,peer=u.recvfrom(65536); u.sendto(b,peer)
threading.Thread(target=datagrams,daemon=True).start()
threading.Thread(target=run,daemon=True).start(); print(json.dumps({'port':p}),flush=True); sys.stdin.readline()
"#;
struct Program {
    child: Child,
    port: u16,
}
impl Program {
    fn start(environment: &orch_envhost::environment::Environment, port: u16) -> Self {
        let mut command = Command::new("python3");
        command
            .args(["-I", "-B", "-u", "-c", PROGRAM])
            .arg(port.to_string())
            .stdin(Stdio::piped())
            .stdout(Stdio::piped());
        let mut child = environment
            .processes()
            .spawn_tracked(&mut command, orch_envhost::processes::RootKind::Executor)
            .unwrap();
        let mut line = String::new();
        BufReader::new(child.stdout.take().unwrap())
            .read_line(&mut line)
            .unwrap();
        let port = serde_json::from_str::<serde_json::Value>(&line).unwrap()["port"]
            .as_u64()
            .unwrap() as u16;
        Self { child, port }
    }
}
impl Drop for Program {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}
fn request(port: u16, place: Id) -> PortOpeningRequest {
    PortOpeningRequest {
        port,
        kind: PortOpeningKind::Place,
        place_id: Some(place),
        label: None,
        transports: Some(vec![PortTransport::Tcp]),
        entry_methods: None,
        until: None,
        host_port: None,
        program: None,
        reason: "let the project's people try the application".into(),
        replacement_of: None,
        close_replaced: None,
    }
}

fn whois_path(inside: &PortConnectionInside, port: u16, transport: &str) -> String {
    let mut query = url::form_urlencoded::Serializer::new(String::new());
    let peer = std::net::SocketAddr::new(inside.address.parse().unwrap(), inside.port);
    query
        .append_pair("peer", &peer.to_string())
        .append_pair("listener_port", &port.to_string())
        .append_pair("transport", transport);
    format!("/whois?{}", query.finish())
}

#[tokio::test]
async fn rules_2_3_6_a_manager_admits_a_colleague_the_connector_stops_starts_and_the_same_request_stays_one_act()
 {
    let mut world = ports_support::World::start().await;
    let old = world.api.app.clone();
    let mut config = (*old.config).clone();
    config.edge.ports_domain = Some("ports.test".into());
    let app = Arc::new(orch_api_core::state::AppState {
        host_ports: old.host_ports.clone(),
        builtin_sources: old.builtin_sources.clone(),
        db: old.db.clone(),
        config: Arc::new(config),
        clock: old.clock.clone(),
        link: old.link.clone(),
        engine: old.engine.clone(),
        realtime: old.realtime.clone(),
        build: old.build,
        blobs: old.blobs.clone(),
        cache: old.cache.clone(),
    });
    app.link.set_applier(Arc::new(Apply {
        engine: app.clone(),
    }));
    let collecting = app.cache.collect_samples(&app.link);
    let stop = CancellationToken::new();
    let upkeep = orch_api_core::ports_runtime::start(&app, &stop);
    let mut registry = Registry::new();
    orch_api_core::routes::mount(&mut registry);
    let client = TestApp::over(app.clone(), orch_testkit::clock::manual(), registry);
    let manager = world.user;
    let colleague = new_id();
    let manager_token = orch_api_core::middleware::authentication::new_token(
        orch_api_core::middleware::authentication::ACCESS_TOKEN_PREFIX,
    );
    let colleague_token = orch_api_core::middleware::authentication::new_token(
        orch_api_core::middleware::authentication::ACCESS_TOKEN_PREFIX,
    );
    {
        let mut conn = world.db.acquire().await.unwrap();
        sqlx::query("UPDATE users SET is_system_admin=true WHERE id=$1")
            .bind(manager)
            .execute(&mut *conn)
            .await
            .unwrap();
        // As Orch ships, a team org is made by a system admin in a browser session or by somebody
        // holding an unused invitation to make one: the manager, who calls with a token, is given one.
        sqlx::query(
            "UPDATE users SET org_invited_by = id, org_invited_at = $2
              WHERE id = $1 AND org_invited_at IS NULL",
        )
        .bind(manager)
        .bind(world.clock.now())
        .execute(&mut *conn)
        .await
        .unwrap();
        orch_db::queries::identity::testing::insert_user(&mut conn, colleague, "colleague")
            .await
            .unwrap();
        for (user, token) in [(manager, &manager_token), (colleague, &colleague_token)] {
            orch_db::queries::identity::testing::insert_session(
                &mut conn,
                new_id(),
                user,
                &orch_api_core::middleware::authentication::token_hash(token),
                world.clock.now() + chrono::Duration::days(1),
            )
            .await
            .unwrap();
        }
        let page = orch_db::queries::access::PageKey::new(
            "system",
            orch_db::queries::access::SYSTEM_ID,
            "defaults",
        );
        for entry in orch_access_contract::pages::shipped() {
            orch_db::queries::access::testing::add_entry(
                &mut conn,
                &page,
                entry.profile.as_str(),
                entry.permission.as_str(),
                ("role", None, Some(entry.role.as_str())),
                None,
                world.clock.now(),
            )
            .await
            .unwrap();
        }
    }
    let org = client
        .call(OrgCreate, &[])
        .token(&manager_token)
        .body(&OrgCreateBody {
            name: "Ports team".into(),
            slug: "ports-walk".into(),
        })
        .send()
        .await
        .ok()
        .base
        .id;
    let project = client
        .call(ProjectCreate, &[&org])
        .token(&manager_token)
        .body(&ProjectCreateBody {
            name: "Preview".into(),
            description: None,
            access: None,
        })
        .send()
        .await
        .ok()
        .base
        .id;
    {
        let mut conn = world.db.acquire().await.unwrap();
        orch_db::queries::access::testing::insert_member(&mut conn, org, colleague, org)
            .await
            .unwrap();
        orch_db::queries::access::testing::give_role(
            &mut conn, org, "user", colleague, "viewer", manager, None,
        )
        .await
        .unwrap();
    }
    let enrolled = world.enroll("ports connector").await;
    let machine = enrolled.identity.machine_id;
    sqlx::query("UPDATE machines SET owner_id=$2 WHERE id=$1")
        .bind(machine)
        .bind(manager)
        .execute(world.db.pool())
        .await
        .unwrap();
    let work = tempfile::tempdir().unwrap();
    let data = tempfile::tempdir().unwrap();
    let mut cfg = orch_config::ConnectorConfig::default();
    cfg.environment.roots = vec![work.path().display().to_string()];
    cfg.ports.allowed = vec![
        orch_api_contract::ports::PortOpeningKind::Place,
        orch_api_contract::ports::PortOpeningKind::Link,
    ];
    // These are the owner's explicit values, as a configuration file naming the keys would record them.
    cfg.policy.local_keys = Some(vec![
        orch_link_contract::policy::PolicyKey::EnvironmentRoots.to_string(),
        orch_link_contract::policy::PolicyKey::Ports.to_string(),
    ]);
    let pinned = world.pinned_config(&enrolled);
    let params = || ConnectorParams {
        follow_system_endpoints: true,
        hosted_endpoints: None,
        version: "0.1.0".into(),
        data_dir: data.path().to_owned(),
        config: cfg.clone(),
        identity: orch_pki::enrollment::MachineIdentity {
            machine_id: enrolled.identity.machine_id,
            key: enrolled.identity.key.duplicate().unwrap(),
            certificate: enrolled.identity.certificate.clone(),
            authority: enrolled.identity.authority.clone(),
        },
        endpoints: enrolled.reply.endpoints.clone(),
        roles: vec![Role::Environment],
        managed: false,
        dialer: Arc::new(orch_link::websocket::TlsDialer::new(
            Some(pinned.clone()),
            None,
        )),
        clock: world.clock.clone(),
        platform: Platform {
            os: Os::Linux,
            arch: Arch::X86_64,
        },
        adapters: orch_connector_core::harnesses::AdapterRegistry::new(),
        tools_dir: None,
        blobs: None,
    };
    let mut connector = ConnectorBuilder::new(params())
        .environment(EnvironmentParts::default())
        .start()
        .await
        .unwrap();
    tokio::time::timeout(ports_support::PATIENCE, connector.link().connected())
        .await
        .unwrap();
    app.engine.start();
    let roots = serde_json::to_value(&cfg.environment.roots).unwrap();
    ports_support::eventually("the connector reports its owner's offered directories", || async {
        sqlx::query_scalar::<_, bool>(
            "SELECT COALESCE(facts#>'{policy,environment.roots}'=$2::jsonb,false) FROM machines WHERE id=$1",
        )
        .bind(machine)
        .bind(&roots)
        .fetch_one(world.db.pool())
        .await
        .unwrap()
    })
    .await;
    let env = client
        .call(MachineEnvironmentCreate, &[&machine])
        .token(&manager_token)
        .body(&MachineEnvironmentCreateBody {
            data_sources: None,
            home_id: org,
            name: "Preview service".into(),
            root: work.path().display().to_string(),
            home_node_id: None,
            remote_write: None,
            terminal: None,
            access: Some(orch_api_contract::binding::NewEnvironmentAccess::OnlyMe),
        })
        .send()
        .await
        .ok()
        .base
        .id;
    ports_support::eventually("the actual attached environment is running", || async {
        sqlx::query_scalar::<_, String>("SELECT status FROM environments WHERE id=$1")
            .bind(env)
            .fetch_one(world.db.pool())
            .await
            .unwrap()
            == "running"
    })
    .await;
    let port = ports_support::reserve_port();
    let before = client
        .call(EnvironmentPortGet, &[&env, &port.to_string()])
        .token(&manager_token)
        .query(&EnvironmentPortGetQuery { look: Some(true) })
        .send()
        .await
        .ok();
    assert_eq!(before.state, PortTargetState::NotListening);
    assert_eq!(before.revision, 0);
    assert!(before.address.is_some());
    let plan = client
        .call(EnvironmentPortOpeningPreflight, &[&env, &port.to_string()])
        .token(&manager_token)
        .body(&request(port, project))
        .send()
        .await
        .ok();
    let body = EnvironmentPortOpeningCreateBody {
        client_id: new_id(),
        preflight_id: plan.preflight_id,
    };
    let first = client
        .call(EnvironmentPortOpeningCreate, &[&env, &port.to_string()])
        .token(&manager_token)
        .body(&body)
        .send()
        .await
        .ok();
    let repeated = client
        .call(EnvironmentPortOpeningCreate, &[&env, &port.to_string()])
        .token(&manager_token)
        .body(&body)
        .send()
        .await
        .ok();
    assert_eq!(first.opening.id, repeated.opening.id);
    let native = connector
        .environment_role()
        .unwrap()
        .host()
        .environment(env)
        .unwrap();
    let program = Program::start(&native, port);
    assert_eq!(program.port, port);
    let target = client
        .call(EnvironmentPortGet, &[&env, &port.to_string()])
        .token(&manager_token)
        .query(&EnvironmentPortGetQuery { look: Some(true) })
        .send()
        .await
        .ok();
    assert_eq!(target.state, PortTargetState::Observed);
    assert_eq!(target.address, before.address);
    let local_url = native.variables()["ORCH_PORTS_URL"].clone();
    let local_token =
        orch_plugin_contract::Secret::new(native.variables()["ORCH_PORTS_TOKEN"].clone());
    assert!(matches!(
        orch_connector_core::local_ports::read_local(&local_url, None, "/ports").await,
        Err(orch_connector_core::local_ports::LocalReadError::Refused(
            401
        ))
    ));
    let local_target = orch_connector_core::local_ports::read_local(
        &local_url,
        Some(&local_token),
        &format!("/ports/{port}"),
    )
    .await
    .unwrap();
    let local_target: PortTarget = serde_json::from_value(local_target).unwrap();
    assert_eq!(local_target.address, target.address);
    let local_list = orch_connector_core::local_ports::read_local(
        &local_url,
        Some(&local_token),
        &format!("/ports?port={port}&limit=20"),
    )
    .await
    .unwrap();
    assert!(!local_list.to_string().contains("linkproof"));
    let keys: PortAssertionKeys = serde_json::from_value(
        orch_connector_core::local_ports::read_local(&local_url, None, "/ports/assertion-keys")
            .await
            .unwrap(),
    )
    .unwrap();
    assert!(!keys.keys.is_empty());
    // A live machine cannot use this protocol to read an unrelated environment or an old generation.
    for (environment_id, generation) in [
        (new_id(), native.facts().generation),
        (env, native.facts().generation + 1),
    ] {
        let refused = connector
            .link()
            .local_ports(orch_link_contract::local_ports::PortLocalAsk {
                request_id: new_id(),
                environment_id,
                generation,
                read: orch_link_contract::local_ports::PortLocalRead::Address { port },
            })
            .await
            .unwrap();
        assert!(refused.data.is_none());
        assert!(refused.failure.is_some());
    }
    let usage = client
        .call(EnvironmentPortUseCreate, &[&env, &port.to_string()])
        .token(&colleague_token)
        .body(&EnvironmentPortUseCreateBody {
            transport: PortTransport::Tcp,
            opening_id: Some(first.opening.id),
        })
        .send()
        .await
        .ok();
    let ingress = tokio::net::TcpListener::bind(("127.0.0.1", ports_support::reserve_port()))
        .await
        .unwrap();
    let mut outside_stream = tokio::net::TcpStream::connect(ingress.local_addr().unwrap())
        .await
        .unwrap();
    let (mut accepted, outside) = ingress.accept().await.unwrap();
    let mut channel = app
        .cache
        .ports()
        .open_channel(usage.id, &usage.ticket, outside)
        .await
        .unwrap();
    let inside = channel.inside_peer().unwrap();
    let closer = channel.closer();
    let forwarding = tokio::spawn(async move {
        let _ = tokio::io::copy_bidirectional(&mut accepted, &mut channel).await;
    });
    outside_stream
        .write_all(b"colleague entered")
        .await
        .unwrap();
    let mut answer = vec![0; 17];
    outside_stream.read_exact(&mut answer).await.unwrap();
    assert_eq!(answer, b"colleague entered");
    let live = client
        .call(EnvironmentPortConnectionList, &[&env])
        .token(&manager_token)
        .query(&EnvironmentPortConnectionListQuery::default())
        .send()
        .await
        .ok();
    assert_eq!(live.counts.active, 1);
    assert_eq!(live.connections.as_ref().unwrap()[0].inside, inside);
    assert_eq!(
        live.connections.as_ref().unwrap()[0].outside.port,
        outside.port()
    );
    // These are the actual native peer and actual TLS machine/API path, not a scripted signer.
    // Concurrent reads of one peer must share its one fresh assertion, including its JWS id.
    let mut peers = tokio::task::JoinSet::new();
    for _ in 0..16 {
        let (base, token, path) = (
            local_url.clone(),
            local_token.clone(),
            whois_path(&inside, port, "tcp"),
        );
        peers.spawn(async move {
            orch_connector_core::local_ports::read_local(&base, Some(&token), &path)
                .await
                .unwrap()
        });
    }
    let mut assertion = None;
    while let Some(answer) = peers.join_next().await {
        let peer: PortPeer = serde_json::from_value(answer.unwrap()).unwrap();
        assert_eq!(peer.connection.id, live.connections.as_ref().unwrap()[0].id);
        assert_eq!(peer.connection.inside, inside);
        assert!(peer.assertion.len() < orch_link_contract::local_ports::PEER_REPLY_BYTES);
        let claims = orch_pki::port_assertions::verify(
            &peer.assertion,
            &keys.keys[0].x,
            &keys.issuer,
            env,
            port,
            orch_types::Time::new(world.clock.now()),
        )
        .unwrap();
        assert_eq!(claims["person"]["id"], colleague.to_string());
        assert_eq!(claims["outside"]["port"], outside.port());
        if let Some(first) = &assertion {
            assert_eq!(first, &peer.assertion);
        } else {
            assertion = Some(peer.assertion);
        }
    }
    let mut stranger = inside.clone();
    stranger.port = if inside.port == u16::MAX {
        inside.port - 1
    } else {
        inside.port + 1
    };
    assert!(
        orch_connector_core::local_ports::read_local(
            &local_url,
            Some(&local_token),
            &whois_path(&stranger, port, "tcp")
        )
        .await
        .unwrap()
        .is_null()
    );
    assert!(
        orch_connector_core::local_ports::read_local(
            &local_url,
            Some(&local_token),
            &whois_path(&inside, port, "udp")
        )
        .await
        .unwrap()
        .is_null()
    );
    let own = client
        .call(EnvironmentPortUseCreate, &[&env, &port.to_string()])
        .token(&manager_token)
        .body(&EnvironmentPortUseCreateBody {
            transport: PortTransport::Tcp,
            opening_id: None,
        })
        .send()
        .await
        .ok();
    let mut manager_stream = tokio::net::TcpStream::connect(ingress.local_addr().unwrap())
        .await
        .unwrap();
    let (mut manager_accepted, manager_peer) = ingress.accept().await.unwrap();
    let mut own_channel = app
        .cache
        .ports()
        .open_channel(own.id, &own.ticket, manager_peer)
        .await
        .unwrap();
    let own_closer = own_channel.closer();
    let own_inside = own_channel.inside_peer().unwrap();
    assert_ne!(own_channel.inside_peer().unwrap(), inside);
    let own_forwarding = tokio::spawn(async move {
        let _ = tokio::io::copy_bidirectional(&mut manager_accepted, &mut own_channel).await;
    });
    manager_stream
        .write_all(b"manager alongside")
        .await
        .unwrap();
    let mut answer = [0; 17];
    manager_stream.read_exact(&mut answer).await.unwrap();
    assert_eq!(&answer, b"manager alongside");
    let two = client
        .call(EnvironmentPortConnectionList, &[&env])
        .token(&manager_token)
        .query(&EnvironmentPortConnectionListQuery::default())
        .send()
        .await
        .ok();
    assert_eq!(two.counts.active, 2);
    assert_eq!(two.totals.as_ref().unwrap().len(), 2);

    // The inside tuple is known, but no assertion for it is cached yet. Losing the signer
    // must be 503, while the already admitted native stream keeps its separate lifetime.
    orch_api_core::testing::machine_request_handler(&app, Arc::new(UnavailablePortReads));
    assert!(matches!(
        orch_connector_core::local_ports::read_local(
            &local_url,
            Some(&local_token),
            &whois_path(&own_inside, port, "tcp")
        )
        .await,
        Err(orch_connector_core::local_ports::LocalReadError::Refused(
            503
        ))
    ));
    orch_api_core::machine_requests::install(&app);
    ports_support::eventually(
        "the inside reader can refresh after the API returns",
        || async {
            orch_connector_core::local_ports::read_local(
                &local_url,
                Some(&local_token),
                &whois_path(&own_inside, port, "tcp"),
            )
            .await
            .is_ok_and(|value| !value.is_null())
        },
    )
    .await;

    // A TCP receiver prefix must never change UDP admission or datagram bytes.
    let udp_port = ports_support::reserve_port();
    let udp_program = Program::start(&native, udp_port);
    client
        .call(EnvironmentPortProgramUpdate, &[&env, &udp_port.to_string()])
        .token(&manager_token)
        .body(&EnvironmentPortProgramUpdateBody {
            expected_revision: 0,
            program: PortProgram {
                proxy_protocol: true,
                ..Default::default()
            },
        })
        .send()
        .await
        .ok();
    client
        .call(EnvironmentPortGet, &[&env, &udp_port.to_string()])
        .token(&manager_token)
        .query(&EnvironmentPortGetQuery { look: Some(true) })
        .send()
        .await
        .ok();
    let udp_use = client
        .call(EnvironmentPortUseCreate, &[&env, &udp_port.to_string()])
        .token(&manager_token)
        .body(&EnvironmentPortUseCreateBody {
            transport: PortTransport::Udp,
            opening_id: None,
        })
        .send()
        .await
        .ok();
    let udp_ingress = tokio::net::UdpSocket::bind(("127.0.0.1", ports_support::reserve_port()))
        .await
        .unwrap();
    let udp_outside = tokio::net::UdpSocket::bind(("127.0.0.1", ports_support::reserve_port()))
        .await
        .unwrap();
    udp_outside
        .send_to(b"UDP has no prefix", udp_ingress.local_addr().unwrap())
        .await
        .unwrap();
    let mut packet = [0; 128];
    let (length, peer) = udp_ingress.recv_from(&mut packet).await.unwrap();
    let udp_channel = app
        .cache
        .ports()
        .open_channel(udp_use.id, &udp_use.ticket, peer)
        .await
        .unwrap();
    assert!(
        udp_channel
            .send_datagram(bytes::Bytes::copy_from_slice(&packet[..length]))
            .unwrap()
    );
    let reply = tokio::time::timeout(ports_support::PATIENCE, udp_channel.recv_datagram())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(reply.as_ref(), b"UDP has no prefix");
    let udp_inside = udp_channel.inside_peer().unwrap();
    let udp_peer: PortPeer = serde_json::from_value(
        orch_connector_core::local_ports::read_local(
            &local_url,
            Some(&local_token),
            &whois_path(&udp_inside, udp_port, "udp"),
        )
        .await
        .unwrap(),
    )
    .unwrap();
    assert_eq!(udp_peer.connection.inside, udp_inside);
    assert_eq!(udp_peer.connection.transport, PortTransport::Udp);
    drop((udp_channel, udp_program, udp_ingress, udp_outside));

    connector.stop().await;
    assert!(
        orch_connector_core::local_ports::read_local(&local_url, Some(&local_token), "/ports")
            .await
            .is_err()
    );
    ports_support::eventually("the disconnected environment is offline", || async {
        sqlx::query_scalar::<_, String>("SELECT status FROM environments WHERE id=$1")
            .bind(env)
            .fetch_one(world.db.pool())
            .await
            .unwrap()
            == "offline"
    })
    .await;
    ports_support::eventually(
        "connector stop closes both current port channels",
        || async { closer.is_closed() && own_closer.is_closed() },
    )
    .await;
    let stopped = client
        .call(PortOpeningGet, &[&first.opening.id])
        .token(&manager_token)
        .send()
        .await
        .ok();
    assert_eq!(stopped.state, PortOpeningState::Stopped);
    connector = ConnectorBuilder::new(params())
        .environment(EnvironmentParts::default())
        .start()
        .await
        .unwrap();
    tokio::time::timeout(ports_support::PATIENCE, connector.link().connected())
        .await
        .unwrap();
    ports_support::eventually("the engine reattaches the restarted connector", || async {
        sqlx::query_scalar::<_, String>("SELECT status FROM environments WHERE id=$1")
            .bind(env)
            .fetch_one(world.db.pool())
            .await
            .unwrap()
            == "running"
    })
    .await;
    drop(program);
    let native = connector
        .environment_role()
        .unwrap()
        .host()
        .environment(env)
        .unwrap();
    let restarted = Program::start(&native, port);
    assert_ne!(native.variables()["ORCH_PORTS_TOKEN"], local_token.expose());
    let fresh_url = native.variables()["ORCH_PORTS_URL"].clone();
    let fresh_token =
        orch_plugin_contract::Secret::new(native.variables()["ORCH_PORTS_TOKEN"].clone());
    client
        .call(EnvironmentPortGet, &[&env, &port.to_string()])
        .token(&manager_token)
        .query(&EnvironmentPortGetQuery { look: Some(true) })
        .send()
        .await
        .ok();
    let usage = client
        .call(EnvironmentPortUseCreate, &[&env, &port.to_string()])
        .token(&colleague_token)
        .body(&EnvironmentPortUseCreateBody {
            transport: PortTransport::Tcp,
            opening_id: Some(first.opening.id),
        })
        .send()
        .await
        .ok();
    let mut after_stream = tokio::net::TcpStream::connect(ingress.local_addr().unwrap())
        .await
        .unwrap();
    let (mut after_accepted, after_peer) = ingress.accept().await.unwrap();
    let mut after = app
        .cache
        .ports()
        .open_channel(usage.id, &usage.ticket, after_peer)
        .await
        .unwrap();
    let after_closer = after.closer();
    let after_inside = after.inside_peer().unwrap();
    let after_forwarding = tokio::spawn(async move {
        let _ = tokio::io::copy_bidirectional(&mut after_accepted, &mut after).await;
    });
    after_stream.write_all(b"after start").await.unwrap();
    let mut bytes = [0; 11];
    after_stream.read_exact(&mut bytes).await.unwrap();
    assert_eq!(&bytes, b"after start");
    let retained = client
        .call(PortOpeningGet, &[&first.opening.id])
        .token(&manager_token)
        .send()
        .await
        .ok();
    assert_eq!(retained.id, first.opening.id);
    let closed = client
        .call(PortOpeningDelete, &[&first.opening.id])
        .token(&manager_token)
        .body(&PortRevisionBody {
            expected_revision: first.opening.revision,
        })
        .send()
        .await
        .ok();
    assert_eq!(closed.state, PortOpeningState::Closed);
    assert!(after_closer.is_closed());
    ports_support::eventually("a closed peer is no longer an inside identity", || async {
        orch_connector_core::local_ports::read_local(
            &fresh_url,
            Some(&fresh_token),
            &whois_path(&after_inside, port, "tcp"),
        )
        .await
        .is_ok_and(|value| value.is_null())
    })
    .await;
    drop((
        outside_stream,
        manager_stream,
        after_stream,
        ingress,
        restarted,
    ));
    forwarding.await.unwrap();
    own_forwarding.await.unwrap();
    after_forwarding.await.unwrap();
    stop.cancel();
    upkeep.await.unwrap();
    collecting.abort();
    app.engine.stop();
    connector.stop().await;
    world.stop().await;
}
