//! Actual API, TLS gateways and machine links; these prove carrier behavior, not native tool effects.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    reason = "test assertions"
)]

mod common;

use common::{Machine, PATIENCE, World, eventually, reserve_port};
use orch_gateway_core::{Gateway, GatewayParts, Settings};
use orch_link::tunnels::channel::{IncomingTunnels, OpenError, Tunnel};
use orch_link_contract::gateways::TunnelPlan;
use orch_link_contract::session::Role;
use orch_link_contract::tunnels::{Service, TicketSource};
use orch_pki::ca::Holder;
use orch_pki::certificates::ServiceName;
use orch_pki::tickets::TicketRequest;
use orch_pki::verification::TlsIdentity;
use orch_types::{Time, new_id};
use std::sync::Arc;
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

struct Regional {
    gateway: Gateway,
    task: tokio::task::JoinHandle<()>,
}
impl Regional {
    async fn stop(self) {
        self.gateway.shutdown();
        tokio::time::timeout(PATIENCE, self.task)
            .await
            .unwrap()
            .unwrap();
    }
}

async fn world(idle: u64) -> (World, Regional) {
    let (world, mut regional) = world_with_gateways(idle, false).await;
    (world, regional.remove(0))
}

async fn world_with_gateways(idle: u64, third: bool) -> (World, Vec<Regional>) {
    world_with_count(idle, if third { 3 } else { 2 }).await
}

async fn world_with_count(idle: u64, count: usize) -> (World, Vec<Regional>) {
    let mut instances = vec![("asia", "Asia tools", "Asia", reserve_port())];
    if count >= 3 {
        instances.push(("us", "US common", "US", reserve_port()));
    }
    if count >= 4 {
        instances.push(("region-4", "Region four", "Region four", reserve_port()));
    }
    if count >= 5 {
        instances.push(("region-5", "Region five", "Region five", reserve_port()));
    }
    let configured = instances.clone();
    let world = World::start_with_config(axum::Router::new(), move |config| {
        config.link.tunnel_session_idle_secs = idle;
        config.gateway.instances = vec![orch_config::server::GatewayInstance {
            id: "test-gateway".into(),
            label: "Europe".into(),
            region: "Europe".into(),
            endpoints: config.gateway.endpoints.clone(),
        }];
        config
            .gateway
            .instances
            .extend(configured.iter().map(|(id, label, region, port)| {
                orch_config::server::GatewayInstance {
                    id: (*id).into(),
                    label: (*label).into(),
                    region: (*region).into(),
                    endpoints: vec![orch_config::server::GatewayEndpoint {
                        url: format!("wss://127.0.0.1:{port}"),
                        trust: "pinned".into(),
                    }],
                }
            }));
    })
    .await;
    let mut regional = Vec::new();
    for (id, _, _, port) in instances {
        regional.push(start_gateway(&world, id, port).await);
    }
    (world, regional)
}

async fn start_gateway(world: &World, id: &str, port: u16) -> Regional {
    let (key, certificate) = world
        .secrets
        .authority()
        .issue_with_new_key(
            &Holder::Service {
                service: ServiceName::Gateway,
                names: vec!["127.0.0.1".into()],
            },
            365,
            world.clock.as_ref(),
        )
        .unwrap();
    let mut settings = Settings::from_config(&world.config, "0.1.0");
    settings.gateway_id = id.into();
    settings.backplane_url = format!("wss://127.0.0.1:{}/internal/backplane", world.api.port);
    let gateway = Gateway::new(GatewayParts {
        settings,
        identity: TlsIdentity {
            chain: vec![certificate],
            key,
        },
        authority: world.secrets.ca_certificate(),
        clock: Arc::clone(&world.clock),
    })
    .unwrap();
    let pinned = tokio::net::TcpListener::bind(("127.0.0.1", port))
        .await
        .unwrap();
    let edge = tokio::net::TcpListener::bind(("127.0.0.1", 0))
        .await
        .unwrap();
    let running = gateway.clone();
    let task = tokio::spawn(async move {
        running.run(pinned, edge).await;
    });
    gateway.wait_for_backplane().await;
    Regional { gateway, task }
}

fn denied_urls(world: &World, ids: &[&str]) -> Vec<String> {
    world
        .config
        .gateway
        .advertised_gateways()
        .into_iter()
        .filter(|gateway| ids.contains(&gateway.id.as_str()))
        .flat_map(|gateway| gateway.endpoints.into_iter().map(|endpoint| endpoint.url))
        .collect()
}

async fn machines(world: &World) -> (Machine, Machine) {
    let source = world.enroll("regional-source").await;
    let destination = world.enroll("regional-executor").await;
    let source = world.start_machine_on_gateway(&source, "test-gateway", vec![Role::Agent]);
    let destination = world.start_machine_on_gateway(&destination, "asia", vec![Role::Environment]);
    tokio::time::timeout(PATIENCE, async {
        source.link.connected().await;
        destination.link.connected().await;
    })
    .await
    .unwrap();
    eventually("both primary routes", || {
        let link = world.api.link.clone();
        let (s, d) = (source.id, destination.id);
        async move {
            link.gateway_of(s).as_deref() == Some("test-gateway")
                && link.gateway_of(d).as_deref() == Some("asia")
        }
    })
    .await;
    (source, destination)
}

fn ticket(world: &World, source: &Machine, destination: &Machine) -> String {
    world
        .api
        .link
        .tickets()
        .issue(&TicketRequest {
            src: TicketSource::Machine(source.id),
            dst: destination.id,
            service: Service::Exec {
                executor: "carrier-fixture".into(),
            },
            thread_id: Some(new_id()),
            tool_set: None,
            expires_at: Time::new(world.clock.now() + chrono::Duration::hours(1)),
        })
        .unwrap()
}

async fn open(
    source: &Machine,
    incoming: &mut IncomingTunnels,
    ticket: &str,
    plan: &TunnelPlan,
) -> (Tunnel, Tunnel) {
    tokio::time::timeout(PATIENCE, async {
        let (opened, accepted) =
            tokio::join!(source.link.open_routed_tunnel(ticket, plan), async {
                incoming.next().await.unwrap().accept()
            });
        (opened.unwrap(), accepted.unwrap())
    })
    .await
    .unwrap()
}

#[tokio::test]
async fn auxiliary_does_not_replace_primary_and_tool_bytes_use_only_the_executor_gateway() {
    let (mut world, regional) = world(60).await;
    let (source, destination) = machines(&world).await;
    let ticket = ticket(&world, &source, &destination);
    let plan = world.api.link.tunnel_plan_for(source.id, &ticket).unwrap();
    assert_eq!(plan.preferred_gateway.id, "asia");
    let lease = source.link.retain_tunnel_plans(vec![plan.clone()]);
    let mut incoming = destination.link.incoming_tunnels();
    let before = world.gateway.gateway.bytes_relayed();
    let (mut near, mut far) = open(&source, &mut incoming, &ticket, &plan).await;
    near.write_all(b"tools beside executor").await.unwrap();
    let mut received = vec![0; 21];
    far.read_exact(&mut received).await.unwrap();
    assert_eq!(&received, b"tools beside executor");
    assert_eq!(world.gateway.gateway.bytes_relayed(), before);
    assert!(regional.gateway.bytes_relayed() >= 21);
    assert_eq!(
        world.api.link.gateway_of(source.id).as_deref(),
        Some("test-gateway")
    );
    assert_eq!(
        world.api.link.gateway_of(destination.id).as_deref(),
        Some("asia")
    );
    assert_eq!(source.link.gateway_id().as_deref(), Some("test-gateway"));
    assert_eq!(source.link.auxiliary_sessions(), 1);
    near.closer().close();
    far.closer().close();
    drop(lease);
    source.link.shutdown().await;
    destination.link.shutdown().await;
    regional.stop().await;
    world.stop().await;
}

#[tokio::test]
async fn only_unused_auxiliaries_close_after_the_operator_idle_interval() {
    let (mut world, regional) = world(1).await;
    let (source, destination) = machines(&world).await;
    let ticket = ticket(&world, &source, &destination);
    let plan = world.api.link.tunnel_plan_for(source.id, &ticket).unwrap();
    let lease = source.link.retain_tunnel_plans(vec![plan.clone()]);
    let mut incoming = destination.link.incoming_tunnels();
    let (near, far) = open(&source, &mut incoming, &ticket, &plan).await;
    near.closer().close();
    far.closer().close();
    tokio::time::sleep(Duration::from_secs(3)).await;
    assert_eq!(
        source.link.auxiliary_sessions(),
        1,
        "an open thread reference protects the carrier"
    );
    drop(lease);
    eventually("unused auxiliary closes", || {
        let link = source.link.clone();
        async move { link.auxiliary_sessions() == 0 }
    })
    .await;
    assert_eq!(
        world.api.link.gateway_of(source.id).as_deref(),
        Some("test-gateway")
    );
    source.link.shutdown().await;
    destination.link.shutdown().await;
    regional.stop().await;
    world.stop().await;
}

#[tokio::test]
async fn all_gateway_carriers_share_the_existing_channel_allowance() {
    let (mut world, regional) = world(60).await;
    let (source, destination) = machines(&world).await;
    let ticket = ticket(&world, &source, &destination);
    let plan = world.api.link.tunnel_plan_for(source.id, &ticket).unwrap();
    let mut local = plan.clone();
    local.plan_id = new_id();
    local.preferred_gateway = local
        .common_fallbacks
        .iter()
        .find(|gateway| gateway.id == "test-gateway")
        .unwrap()
        .clone();
    local.common_fallbacks.clear();
    let lease = source
        .link
        .retain_tunnel_plans(vec![plan.clone(), local.clone()]);
    let mut incoming = destination.link.incoming_tunnels();
    let mut pairs = Vec::new();
    for index in 0..orch_link_contract::version::MAX_CHANNELS {
        pairs.push(
            open(
                &source,
                &mut incoming,
                &ticket,
                if index % 2 == 0 { &plan } else { &local },
            )
            .await,
        );
    }
    assert!(matches!(
        source.link.open_routed_tunnel(&ticket, &plan).await,
        Err(OpenError::TooMany)
    ));
    let (near, far) = pairs.pop().unwrap();
    near.closer().close();
    far.closer().close();
    pairs.push(open(&source, &mut incoming, &ticket, &plan).await);
    for (near, far) in pairs {
        near.closer().close();
        far.closer().close();
    }
    drop(lease);
    source.link.shutdown().await;
    destination.link.shutdown().await;
    regional.stop().await;
    world.stop().await;
}

#[tokio::test]
async fn a_third_common_gateway_preserves_both_hand_picked_primaries() {
    let (mut world, gateways) = world_with_gateways(60, true).await;
    let enrolled_source = world.enroll("limited-source").await;
    let enrolled_destination = world.enroll("limited-executor").await;
    let source = world.start_machine_with_reachability(
        &enrolled_source,
        "test-gateway",
        vec![Role::Agent],
        denied_urls(&world, &["asia"]),
    );
    let destination = world.start_machine_with_reachability(
        &enrolled_destination,
        "asia",
        vec![Role::Environment],
        denied_urls(&world, &["test-gateway"]),
    );
    tokio::time::timeout(PATIENCE, async {
        source.link.connected().await;
        destination.link.connected().await;
    })
    .await
    .unwrap();
    eventually("primary registry", || {
        let link = world.api.link.clone();
        let (s, d) = (source.id, destination.id);
        async move { link.gateway_of(s).is_some() && link.gateway_of(d).is_some() }
    })
    .await;
    let ticket = ticket(&world, &source, &destination);
    let plan = world.api.link.tunnel_plan_for(source.id, &ticket).unwrap();
    let lease = source.link.retain_tunnel_plans(vec![plan.clone()]);
    let mut incoming = destination.link.incoming_tunnels();
    let (mut near, mut far) = open(&source, &mut incoming, &ticket, &plan).await;
    near.write_all(b"third gateway").await.unwrap();
    let mut bytes = [0; 13];
    far.read_exact(&mut bytes).await.unwrap();
    assert_eq!(&bytes, b"third gateway");
    assert!(gateways[1].gateway.bytes_relayed() >= 13);
    assert_eq!(gateways[0].gateway.bytes_relayed(), 0);
    assert_eq!(world.gateway.gateway.bytes_relayed(), 0);
    assert_eq!(source.link.gateway_id().as_deref(), Some("test-gateway"));
    assert_eq!(destination.link.gateway_id().as_deref(), Some("asia"));
    assert_eq!(source.link.auxiliary_sessions(), 1);
    assert_eq!(destination.link.auxiliary_sessions(), 1);
    near.closer().close();
    far.closer().close();
    drop(lease);
    source.link.shutdown().await;
    destination.link.shutdown().await;
    for gateway in gateways {
        gateway.stop().await;
    }
    world.stop().await;
}

#[tokio::test]
async fn no_common_gateway_refuses_with_labels_and_does_not_move_primaries() {
    let (mut world, regional) = world(60).await;
    let enrolled_source = world.enroll("isolated-source").await;
    let enrolled_destination = world.enroll("isolated-executor").await;
    let source = world.start_machine_with_reachability(
        &enrolled_source,
        "test-gateway",
        vec![Role::Agent],
        denied_urls(&world, &["asia"]),
    );
    let destination = world.start_machine_with_reachability(
        &enrolled_destination,
        "asia",
        vec![Role::Environment],
        denied_urls(&world, &["test-gateway"]),
    );
    tokio::time::timeout(PATIENCE, async {
        source.link.connected().await;
        destination.link.connected().await;
    })
    .await
    .unwrap();
    eventually("primary registry", || {
        let link = world.api.link.clone();
        let (s, d) = (source.id, destination.id);
        async move { link.gateway_of(s).is_some() && link.gateway_of(d).is_some() }
    })
    .await;
    let ticket = ticket(&world, &source, &destination);
    let plan = world.api.link.tunnel_plan_for(source.id, &ticket).unwrap();
    let result = tokio::time::timeout(PATIENCE, source.link.open_routed_tunnel(&ticket, &plan))
        .await
        .unwrap();
    let Err(OpenError::Refused { code, detail }) = result else {
        panic!("expected unreachable refusal")
    };
    assert_eq!(
        code,
        orch_link_contract::tunnels::OpenErrorCode::GatewayUnreachable
    );
    assert!(
        detail.contains("Asia tools") && detail.contains("Europe"),
        "{detail}"
    );
    assert!(
        detail.contains("source") && detail.contains("peer"),
        "{detail}"
    );
    assert!(!detail.contains(&ticket));
    assert_eq!(source.link.gateway_id().as_deref(), Some("test-gateway"));
    assert_eq!(destination.link.gateway_id().as_deref(), Some("asia"));
    source.link.shutdown().await;
    destination.link.shutdown().await;
    regional.stop().await;
    world.stop().await;
}

#[tokio::test]
async fn an_api_outage_preserves_existing_carriers_but_refuses_new_admission() {
    let (mut world, regional) = world(60).await;
    let late = world.enroll("late-machine").await;
    let (source, destination) = machines(&world).await;
    let ticket = ticket(&world, &source, &destination);
    let plan = world.api.link.tunnel_plan_for(source.id, &ticket).unwrap();
    let lease = source.link.retain_tunnel_plans(vec![plan.clone()]);
    let mut incoming = destination.link.incoming_tunnels();
    let (mut near, mut far) = open(&source, &mut incoming, &ticket, &plan).await;
    world.api.stop().await;
    eventually("gateway sees stopped API", || {
        let gateway = regional.gateway.clone();
        async move { !gateway.backplane_up() }
    })
    .await;
    near.write_all(b"api offline").await.unwrap();
    let mut bytes = [0; 11];
    tokio::time::timeout(PATIENCE, far.read_exact(&mut bytes))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(&bytes, b"api offline");
    // A previously issued ticket can open another stream on a carrier that already stands.
    let (extra, peer) = open(&source, &mut incoming, &ticket, &plan).await;
    extra.closer().close();
    peer.closer().close();
    let late = world.start_machine_on_gateway(&late, "asia", vec![Role::Agent]);
    let mut status = late.link.status();
    tokio::time::timeout(PATIENCE, async {
        loop {
            if matches!(
                *status.borrow_and_update(),
                orch_link::machine::LinkStatus::Refused {
                    code: orch_link_contract::version::RejectCode::Busy,
                    ..
                }
            ) {
                break;
            }
            status.changed().await.unwrap();
        }
    })
    .await
    .unwrap();
    late.link.shutdown().await;
    near.closer().close();
    far.closer().close();
    drop(lease);
    source.link.shutdown().await;
    destination.link.shutdown().await;
    // The gateway has no independent machine-admission authority while the API is absent.
    assert!(!regional.gateway.backplane_up());
    regional.stop().await;
    world.stop().await;
}

#[tokio::test]
async fn a_gateway_process_loss_resets_the_old_channel_before_the_next_open() {
    let (mut world, regional) = world(60).await;
    let (source, destination) = machines(&world).await;
    let ticket = ticket(&world, &source, &destination);
    let plan = world.api.link.tunnel_plan_for(source.id, &ticket).unwrap();
    let lease = source.link.retain_tunnel_plans(vec![plan.clone()]);
    let mut incoming = destination.link.incoming_tunnels();
    let (mut near, far) = open(&source, &mut incoming, &ticket, &plan).await;
    regional.stop().await;
    let endpoint = &plan.preferred_gateway.endpoints[0];
    let port = url::Url::parse(&endpoint.url).unwrap().port().unwrap();
    let restarted = start_gateway(&world, "asia", port).await;
    let mut byte = [0];
    assert!(
        tokio::time::timeout(PATIENCE, near.read_exact(&mut byte))
            .await
            .unwrap()
            .is_err(),
        "a new gateway process has no channel state to resume"
    );
    far.closer().close();
    tokio::time::timeout(PATIENCE, destination.link.connected())
        .await
        .unwrap();
    let (fresh, peer) = open(&source, &mut incoming, &ticket, &plan).await;
    fresh.closer().close();
    peer.closer().close();
    drop(lease);
    source.link.shutdown().await;
    destination.link.shutdown().await;
    restarted.stop().await;
    world.stop().await;
}

#[tokio::test]
async fn an_auxiliary_cannot_publish_control_or_receive_a_content_token() {
    use orch_link::reconnect::Dialer;
    let (mut world, regional) = world(60).await;
    let enrolled = world.enroll("control-isolation").await;
    let source = world.start_machine_on_gateway(&enrolled, "test-gateway", vec![Role::Agent]);
    tokio::time::timeout(PATIENCE, source.link.connected())
        .await
        .unwrap();
    eventually("primary registry", || {
        let link = world.api.link.clone();
        let id = source.id;
        async move { link.gateway_of(id).as_deref() == Some("test-gateway") }
    })
    .await;
    let endpoint = world
        .config
        .gateway
        .advertised_gateways()
        .into_iter()
        .find(|gateway| gateway.id == "asia")
        .unwrap()
        .endpoints
        .remove(0);
    let dialer = orch_link::websocket::TlsDialer::new(Some(world.pinned_config(&enrolled)), None);
    let connection = dialer.dial_tunnel(&endpoint).await.unwrap();
    let identity = orch_link::session::ClientIdentity {
        purpose: orch_link_contract::gateways::SessionPurpose::Tunnel,
        gateway_id: Some("asia".into()),
        endpoints: None,
        hosted_endpoints: None,
        machine_id: source.id,
        connector_version: "0.1.0".into(),
        platform: orch_link_contract::session::Platform {
            os: orch_link_contract::session::Os::Linux,
            arch: orch_link_contract::session::Arch::X86_64,
        },
        roles: vec![Role::Agent],
        proof: Some((
            enrolled.identity.key.duplicate().unwrap(),
            enrolled.identity.certificate.clone(),
        )),
    };
    let mut auxiliary = orch_link::session::connect(connection, &identity, world.clock.as_ref())
        .await
        .unwrap();
    assert!(auxiliary.ready.session_token.is_empty() && auxiliary.ready.cursors.is_empty());
    assert!(auxiliary.ready.endpoints.is_none() && auxiliary.ready.gateways.is_empty());
    auxiliary
        .sender
        .send(orch_link_contract::commands::CmdAck { id: new_id() })
        .unwrap();
    let ended = tokio::time::timeout(PATIENCE, async {
        loop {
            if let Some(orch_link::session::SessionEvent::Ended(reason)) =
                auxiliary.inbox.next().await
            {
                break reason;
            }
        }
    })
    .await
    .unwrap();
    assert_eq!(
        ended,
        orch_link::session::EndReason::Bye(orch_link_contract::version::ByeCode::ProtocolError)
    );
    assert_eq!(
        world.api.link.gateway_of(source.id).as_deref(),
        Some("test-gateway")
    );
    source.link.shutdown().await;
    regional.stop().await;
    world.stop().await;
}

#[tokio::test]
async fn an_api_restart_recovers_pairing_capabilities_without_moving_either_primary() {
    let (mut world, regional) = world(60).await;
    let (source, destination) = machines(&world).await;
    let source_session = source.link.status().borrow().clone();
    let destination_session = destination.link.status().borrow().clone();
    world.restart_api().await;
    eventually("both replayed primary routes", || {
        let link = world.api.link.clone();
        let (s, d) = (source.id, destination.id);
        async move {
            link.gateway_of(s).as_deref() == Some("test-gateway")
                && link.gateway_of(d).as_deref() == Some("asia")
        }
    })
    .await;
    let ticket = ticket(&world, &source, &destination);
    let plan = world.api.link.tunnel_plan_for(source.id, &ticket).unwrap();
    assert_eq!(plan.preferred_gateway.id, "asia");
    assert!(
        plan.common_fallbacks
            .iter()
            .any(|gateway| gateway.id == "test-gateway"),
        "the API recovered the destination's ability to attach an auxiliary"
    );
    assert_eq!(*source.link.status().borrow(), source_session);
    assert_eq!(*destination.link.status().borrow(), destination_session);
    let mut incoming = destination.link.incoming_tunnels();
    let (near, far) = open(&source, &mut incoming, &ticket, &plan).await;
    near.closer().close();
    far.closer().close();
    source.link.shutdown().await;
    destination.link.shutdown().await;
    regional.stop().await;
    world.stop().await;
}

#[tokio::test]
async fn revocation_kicks_the_primary_and_every_auxiliary_without_an_auxiliary_routing_table() {
    let (mut world, regional) = world(60).await;
    let (source, destination) = machines(&world).await;
    let ticket = ticket(&world, &source, &destination);
    let plan = world.api.link.tunnel_plan_for(source.id, &ticket).unwrap();
    let lease = source.link.retain_tunnel_plans(vec![plan.clone()]);
    let mut incoming = destination.link.incoming_tunnels();
    let (near, far) = open(&source, &mut incoming, &ticket, &plan).await;
    assert_eq!(regional.gateway.sessions_held(), 2);
    world
        .api
        .link
        .certificates()
        .revoke(
            source.id,
            orch_db::audit::Actor::User(world.user),
            &orch_db::audit::Origin::default(),
        )
        .await
        .unwrap();
    world.api.link.let_go(source.id).await;
    eventually("primary and auxiliary kicked", || {
        let primary = world.gateway.gateway.clone();
        let auxiliary = regional.gateway.clone();
        async move { primary.sessions_held() == 0 && auxiliary.sessions_held() == 1 }
    })
    .await;
    assert_eq!(destination.link.gateway_id().as_deref(), Some("asia"));
    near.closer().close();
    far.closer().close();
    drop(lease);
    source.link.shutdown().await;
    destination.link.shutdown().await;
    regional.stop().await;
    world.stop().await;
}

#[tokio::test]
async fn five_gateways_keep_one_transport_each_while_concurrent_threads_share_the_pool() {
    let (mut world, gateways) = world_with_count(60, 5).await;
    let enrolled = world.enroll("five-region-source").await;
    let source = world.start_machine_on_gateway(&enrolled, "test-gateway", vec![Role::Agent]);
    let mut peers = Vec::new();
    for gateway in world.config.gateway.advertised_gateways() {
        let enrolled = world.enroll(&format!("executor-{}", gateway.id)).await;
        peers.push(world.start_machine_on_gateway(&enrolled, &gateway.id, vec![Role::Environment]));
    }
    tokio::time::timeout(PATIENCE, async {
        source.link.connected().await;
        for peer in &peers {
            peer.link.connected().await;
        }
    })
    .await
    .unwrap();
    let mut ids = vec![source.id];
    ids.extend(peers.iter().map(|peer| peer.id));
    eventually("five-region primary registry", || {
        let link = world.api.link.clone();
        let ids = ids.clone();
        async move { ids.iter().all(|id| link.gateway_of(*id).is_some()) }
    })
    .await;
    let primary = source.link.status().borrow().clone();
    let mut plans = Vec::new();
    let mut tasks = Vec::new();
    for peer in &peers {
        let ticket = ticket(&world, &source, peer);
        let plan = world.api.link.tunnel_plan_for(source.id, &ticket).unwrap();
        plans.push(plan.clone());
        let link = source.link.clone();
        let mut incoming = peer.link.incoming_tunnels();
        tasks.push(tokio::spawn(async move {
            let (opened, accepted) = tokio::join!(
                async {
                    tokio::join!(
                        link.open_routed_tunnel(&ticket, &plan),
                        link.open_routed_tunnel(&ticket, &plan)
                    )
                },
                async {
                    vec![
                        incoming.next().await.unwrap().accept().unwrap(),
                        incoming.next().await.unwrap().accept().unwrap(),
                    ]
                },
            );
            (vec![opened.0.unwrap(), opened.1.unwrap()], accepted)
        }));
    }
    let lease = source.link.retain_tunnel_plans(plans);
    let mut held = Vec::new();
    for task in tasks {
        held.push(tokio::time::timeout(PATIENCE, task).await.unwrap().unwrap());
    }
    let observed_gateways = std::iter::once(world.gateway.gateway.clone())
        .chain(gateways.iter().map(|gateway| gateway.gateway.clone()))
        .collect::<Vec<_>>();
    eventually("all five source attachments to be ready", || {
        let source = source.link.clone();
        let gateways = observed_gateways.clone();
        async move {
            source.auxiliary_sessions() == 4
                && gateways.iter().all(|gateway| gateway.sessions_held() == 2)
        }
    })
    .await;
    assert_eq!(source.link.auxiliary_sessions(), 4);
    assert_eq!(*source.link.status().borrow(), primary);
    assert_eq!(
        world.gateway.gateway.sessions_held(),
        2,
        "source and the local executor use their primaries"
    );
    assert!(
        gateways
            .iter()
            .all(|gateway| gateway.gateway.sessions_held() == 2),
        "two concurrent opens must share each source auxiliary"
    );
    for (opened, accepted) in held {
        for tunnel in opened.into_iter().chain(accepted) {
            tunnel.closer().close();
        }
    }
    drop(lease);
    source.link.shutdown().await;
    for peer in peers {
        peer.link.shutdown().await;
    }
    for gateway in gateways {
        gateway.stop().await;
    }
    world.stop().await;
}
