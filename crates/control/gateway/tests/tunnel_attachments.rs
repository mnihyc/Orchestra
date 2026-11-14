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

async fn planned_ticket(
    world: &World,
    source: &Machine,
    destination: &Machine,
) -> (String, TunnelPlan) {
    let signed = ticket(world, source, destination);
    let plan = world
        .api
        .link
        .tunnel_plan_for(source.id, &signed)
        .await
        .unwrap();
    (signed, plan)
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
    let plan = world
        .api
        .link
        .tunnel_plan_for(source.id, &ticket)
        .await
        .unwrap();
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
    let plan = world
        .api
        .link
        .tunnel_plan_for(source.id, &ticket)
        .await
        .unwrap();
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
    let plan = world
        .api
        .link
        .tunnel_plan_for(source.id, &ticket)
        .await
        .unwrap();
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
    let plan = world
        .api
        .link
        .tunnel_plan_for(source.id, &ticket)
        .await
        .unwrap();
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
    let plan = world
        .api
        .link
        .tunnel_plan_for(source.id, &ticket)
        .await
        .unwrap();
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
    let plan = world
        .api
        .link
        .tunnel_plan_for(source.id, &ticket)
        .await
        .unwrap();
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
async fn a_restarted_gateway_can_rebuild_the_route_without_replacing_the_native_sockets() {
    let (mut world, regional) = world(60).await;
    let (source, destination) = machines(&world).await;
    let ticket = ticket(&world, &source, &destination);
    let plan = world
        .api
        .link
        .tunnel_plan_for(source.id, &ticket)
        .await
        .unwrap();
    let lease = source.link.retain_tunnel_plans(vec![plan.clone()]);
    let mut incoming = destination.link.incoming_tunnels();
    let (mut near, mut far) = open(&source, &mut incoming, &ticket, &plan).await;
    let cid = near.cid();
    regional.stop().await;
    let endpoint = &plan.preferred_gateway.endpoints[0];
    let port = url::Url::parse(&endpoint.url).unwrap().port().unwrap();
    let restarted = start_gateway(&world, "asia", port).await;
    far.write_all(b"retained").await.unwrap();
    let mut bytes = [0; 8];
    tokio::time::timeout(PATIENCE, near.read_exact(&mut bytes))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(&bytes, b"retained");
    assert_eq!(near.cid(), cid);
    assert_eq!(far.cid(), cid);
    near.closer().close();
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
        .send(orch_link_contract::commands::CmdAck {
            id: new_id(),
            key_phase: orch_link_contract::commands::CommandKeyPhase::Queued,
        })
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
    let plan = world
        .api
        .link
        .tunnel_plan_for(source.id, &ticket)
        .await
        .unwrap();
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
    let plan = world
        .api
        .link
        .tunnel_plan_for(source.id, &ticket)
        .await
        .unwrap();
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
async fn two_threads_share_one_cold_tunnel_attachment_without_changing_the_primary() {
    let (mut world, gateways) = world_with_gateways(60, false).await;
    let (source, destination) = machines(&world).await;
    let primary = source.link.status().borrow().clone();
    let first_ticket = ticket(&world, &source, &destination);
    let second_ticket = ticket(&world, &source, &destination);
    let first_plan = world
        .api
        .link
        .tunnel_plan_for(source.id, &first_ticket)
        .await
        .unwrap();
    let second_plan = world
        .api
        .link
        .tunnel_plan_for(source.id, &second_ticket)
        .await
        .unwrap();
    assert_ne!(first_plan.plan_id, second_plan.plan_id);
    assert_eq!(source.link.auxiliary_sessions(), 0);
    let lease = source
        .link
        .retain_tunnel_plans(vec![first_plan.clone(), second_plan.clone()]);
    let mut incoming = destination.link.incoming_tunnels();
    let (opened, mut accepted) = tokio::time::timeout(PATIENCE, async {
        tokio::join!(
            async {
                tokio::join!(
                    source.link.open_routed_tunnel(&first_ticket, &first_plan),
                    source.link.open_routed_tunnel(&second_ticket, &second_plan)
                )
            },
            async {
                [
                    incoming.next().await.unwrap().accept().unwrap(),
                    incoming.next().await.unwrap().accept().unwrap(),
                ]
            },
        )
    })
    .await
    .unwrap();
    let mut opened = [opened.0.unwrap(), opened.1.unwrap()];
    for tunnel in &mut opened {
        tunnel.write_all(b"pooled").await.unwrap();
    }
    for tunnel in &mut accepted {
        let mut bytes = [0; 6];
        tokio::time::timeout(PATIENCE, tunnel.read_exact(&mut bytes))
            .await
            .unwrap()
            .unwrap();
        assert_eq!(&bytes, b"pooled");
    }
    eventually("two threads share their one cold source attachment", || {
        let source = source.link.clone();
        let near = gateways[0].gateway.clone();
        async move { source.auxiliary_sessions() == 1 && near.sessions_held() == 2 }
    })
    .await;
    assert_eq!(*source.link.status().borrow(), primary);
    assert_eq!(world.gateway.gateway.sessions_held(), 1);
    for tunnel in opened.into_iter().chain(accepted) {
        tunnel.closer().close();
    }
    drop(lease);
    source.link.shutdown().await;
    destination.link.shutdown().await;
    for gateway in gateways {
        gateway.stop().await;
    }
    world.stop().await;
}

#[tokio::test]
async fn five_gateways_keep_one_transport_each_while_concurrent_threads_share_the_pool() {
    let (mut world, gateways) = world_with_count(60, 5).await;
    let enrolled = world.enroll("five-region-source").await;
    let source = world.start_machine_on_gateway(&enrolled, "test-gateway", vec![Role::Agent]);
    let mut expected_primaries = vec![(source.id, "test-gateway".to_owned())];
    let mut peers = Vec::new();
    for gateway in world.config.gateway.advertised_gateways() {
        let enrolled = world.enroll(&format!("executor-{}", gateway.id)).await;
        let peer = world.start_machine_on_gateway(&enrolled, &gateway.id, vec![Role::Environment]);
        expected_primaries.push((peer.id, gateway.id));
        peers.push(peer);
    }
    tokio::time::timeout(PATIENCE, async {
        source.link.connected().await;
        for peer in &peers {
            peer.link.connected().await;
        }
    })
    .await
    .unwrap();
    eventually("five-region exact primary registry", || {
        let link = world.api.link.clone();
        let expected = expected_primaries.clone();
        async move {
            expected
                .iter()
                .all(|(id, gateway)| link.gateway_of(*id).as_deref() == Some(gateway.as_str()))
        }
    })
    .await;
    let primary = source.link.status().borrow().clone();
    let mut retained = Vec::new();
    let mut warm_routes = Vec::new();
    let mut thread_routes = Vec::new();
    for peer in &peers {
        let warm = planned_ticket(&world, &source, peer).await;
        let threads = [
            planned_ticket(&world, &source, peer).await,
            planned_ticket(&world, &source, peer).await,
        ];
        assert_ne!(threads[0].1.plan_id, threads[1].1.plan_id);
        retained.push(warm.1.clone());
        retained.extend(threads.iter().map(|(_, plan)| plan.clone()));
        warm_routes.push(warm);
        thread_routes.push(threads);
    }
    let lease = source.link.retain_tunnel_plans(retained);
    let mut held = Vec::new();
    // The native incoming queue has one owner for the lifetime of each peer.
    let mut incoming = peers
        .iter()
        .map(|peer| peer.link.incoming_tunnels())
        .collect::<Vec<_>>();
    // Establish each real transport before measuring sharing of a five-entry pool.
    // A cold-attachment race is tested separately, without requiring five concurrent TLS handshakes.
    for ((signed, plan), incoming) in warm_routes.iter().zip(&mut incoming) {
        let (mut opened, mut accepted) = open(&source, incoming, signed, plan).await;
        opened.write_all(b"warm").await.unwrap();
        let mut request = [0; 4];
        tokio::time::timeout(PATIENCE, accepted.read_exact(&mut request))
            .await
            .unwrap()
            .unwrap();
        assert_eq!(&request, b"warm");
        accepted.write_all(b"ready").await.unwrap();
        let mut response = [0; 5];
        tokio::time::timeout(PATIENCE, opened.read_exact(&mut response))
            .await
            .unwrap()
            .unwrap();
        assert_eq!(&response, b"ready");
        held.push((vec![opened], vec![accepted]));
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
    let mut tasks = Vec::new();
    for (routes, mut incoming) in thread_routes.into_iter().zip(incoming) {
        let link = source.link.clone();
        tasks.push(tokio::spawn(async move {
            let [(first_ticket, first_plan), (second_ticket, second_plan)] = routes;
            let (opened, accepted) = tokio::join!(
                async {
                    tokio::join!(
                        link.open_routed_tunnel(&first_ticket, &first_plan),
                        link.open_routed_tunnel(&second_ticket, &second_plan)
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
    for task in tasks {
        held.push(tokio::time::timeout(PATIENCE, task).await.unwrap().unwrap());
    }
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
        "different concurrent thread plans must share each source auxiliary"
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

async fn gateway_samples(world: &World, machine: &Machine, values: &[(&str, u64)], at: Time) {
    use orch_link_contract::{events::Event, session::MachineFacts, streams::Stream};
    let catalogue = world.config.gateway.advertised_gateways();
    let measurements=values.iter().map(|(id,rtt)|{
        let gateway=catalogue.iter().find(|gateway|gateway.id==*id).unwrap();
        serde_json::json!({"gateway_id":id,"endpoint":{"url":gateway.endpoints[0].url,"trust":gateway.endpoints[0].trust},"round_trip_ms":rtt})
    }).collect::<Vec<_>>();
    let facts:MachineFacts=serde_json::from_value(serde_json::json!({"hostname":"placement-fixture","os_version":"fixture","cpus":1,"memory_bytes":1,"harnesses":[],"adapters":[],"plugins":[],"policy":null,"rates":{},"clock_offset_ms":0,"gateway":{"choice":machine.link.gateway_status().choice,"measured_at":at,"measurements":measurements}})).unwrap();
    machine
        .link
        .publish(&Stream::Machine, &Event::MachineFacts(facts))
        .await
        .unwrap();
}

#[tokio::test]
async fn fresh_two_sided_measurements_choose_one_common_gateway_without_moving_selected_primaries()
{
    use orch_api_contract::gateways::TunnelGatewayRule;
    let (mut world, regional) = world_with_count(60, 3).await;
    let (source, destination) = machines(&world).await;
    let issued = ticket(&world, &source, &destination);
    let missing = world
        .api
        .link
        .tunnel_plan_for(source.id, &issued)
        .await
        .unwrap();
    assert_eq!(missing.preferred_gateway.id, "asia");
    assert_eq!(
        missing.decision.rule,
        TunnelGatewayRule::ExecutorSamplesUnavailable
    );
    let now = world.clock.time();
    gateway_samples(
        &world,
        &source,
        &[("test-gateway", 5), ("asia", 190), ("us", 20)],
        now,
    )
    .await;
    gateway_samples(
        &world,
        &destination,
        &[("test-gateway", 180), ("asia", 5), ("us", 20)],
        now,
    )
    .await;
    eventually("both committed measurement vectors", || {
        let link = world.api.link.clone();
        let ticket = issued.clone();
        let id = source.id;
        async move {
            link.tunnel_plan_for(id, &ticket)
                .await
                .is_some_and(|plan| plan.preferred_gateway.id == "us")
        }
    })
    .await;
    let plan = world
        .api
        .link
        .tunnel_plan_for(source.id, &issued)
        .await
        .unwrap();
    assert_eq!(plan.decision.rule, TunnelGatewayRule::LowestCombinedRtt);
    assert_eq!(plan.decision.combined_round_trip_ms, Some(40));
    assert_eq!(plan.decision.source_measured_at, Some(now));
    assert_eq!(plan.decision.destination_measured_at, Some(now));
    let lease = source.link.retain_tunnel_plans(vec![plan.clone()]);
    let mut incoming = destination.link.incoming_tunnels();
    let europe_before = world.gateway.gateway.bytes_relayed();
    let asia_before = regional[0].gateway.bytes_relayed();
    let (mut near, mut far) = open(&source, &mut incoming, &issued, &plan).await;
    near.write_all(b"one gateway").await.unwrap();
    let mut bytes = [0; 11];
    far.read_exact(&mut bytes).await.unwrap();
    assert_eq!(&bytes, b"one gateway");
    assert_eq!(world.gateway.gateway.bytes_relayed(), europe_before);
    assert_eq!(regional[0].gateway.bytes_relayed(), asia_before);
    assert!(regional[1].gateway.bytes_relayed() >= 11);
    assert_eq!(source.link.gateway_id().as_deref(), Some("test-gateway"));
    assert_eq!(destination.link.gateway_id().as_deref(), Some("asia"));
    assert_eq!(source.link.auxiliary_sessions(), 1);
    assert_eq!(destination.link.auxiliary_sessions(), 1);
    near.closer().close();
    far.closer().close();
    drop(lease);
    let stale = Time::new(
        now.get()
            - chrono::Duration::seconds(
                i64::try_from(world.config.link.dead_after_secs).unwrap() + 1,
            ),
    );
    gateway_samples(&world, &source, &[("us", 0)], stale).await;
    eventually("stale vector cannot choose a gateway", || {
        let link = world.api.link.clone();
        let ticket = issued.clone();
        let id = source.id;
        async move {
            link.tunnel_plan_for(id, &ticket).await.is_some_and(|plan| {
                plan.preferred_gateway.id == "asia"
                    && plan.decision.rule == TunnelGatewayRule::ExecutorSamplesUnavailable
            })
        }
    })
    .await;
    source.link.shutdown().await;
    destination.link.shutdown().await;
    for gateway in regional {
        gateway.stop().await;
    }
    world.stop().await;
}

#[tokio::test]
async fn the_normal_inherited_operator_setting_pins_a_measured_pair_to_the_executor() {
    use orch_api_contract::gateways::TunnelGatewayRule;
    use orch_settings::write::{Write, write};
    let (mut world, regional) = world_with_count(60, 3).await;
    let (source, destination) = machines(&world).await;
    gateway_samples(&world, &source, &[("us", 20)], world.clock.time()).await;
    gateway_samples(&world, &destination, &[("us", 20)], world.clock.time()).await;
    let issued = ticket(&world, &source, &destination);
    eventually("measured default before the pin", || {
        let link = world.api.link.clone();
        let ticket = issued.clone();
        let id = source.id;
        async move {
            link.tunnel_plan_for(id, &ticket)
                .await
                .is_some_and(|plan| plan.preferred_gateway.id == "us")
        }
    })
    .await;
    let mut conn = world.db.acquire().await.unwrap();
    write(
        &mut conn,
        &Write {
            actor: orch_db::audit::Actor::System,
            target: orch_settings::Target::System,
            key: "gateways.tunnel_strategy",
            value: serde_json::json!("executor"),
            locked: None,
            expected_revision: None,
            may_lock: true,
            origin: &orch_db::audit::Origin::default(),
        },
        world.clock.now(),
    )
    .await
    .unwrap();
    drop(conn);
    let plan = world
        .api
        .link
        .tunnel_plan_for(source.id, &issued)
        .await
        .unwrap();
    assert_eq!(plan.preferred_gateway.id, "asia");
    assert_eq!(plan.decision.rule, TunnelGatewayRule::ExecutorPinned);
    source.link.shutdown().await;
    destination.link.shutdown().await;
    for gateway in regional {
        gateway.stop().await;
    }
    world.stop().await;
}

#[tokio::test]
async fn automatic_moves_a_healthy_idle_primary_after_a_better_gateway_appears_and_drains_its_auxiliary()
 {
    let (mut world, regional) = world(60).await;
    let enrolled = world.enroll("moving-automatic").await;
    let scores = Arc::new(std::sync::Mutex::new(std::collections::HashMap::from([
        ("test-gateway".into(), 10),
        ("asia".into(), 100),
    ])));
    let source = world.start_automatic_machine(&enrolled, scores.clone());
    let destination = world.enroll("moving-peer").await;
    let destination = world.start_machine_on_gateway(&destination, "asia", vec![Role::Environment]);
    source.link.connected().await;
    destination.link.connected().await;
    eventually("initial automatic primary", || {
        let link = world.api.link.clone();
        let id = source.id;
        async move { link.gateway_of(id).as_deref() == Some("test-gateway") }
    })
    .await;
    let original = source.link.status().borrow().clone();
    let issued = ticket(&world, &source, &destination);
    let plan = world
        .api
        .link
        .tunnel_plan_for(source.id, &issued)
        .await
        .unwrap();
    let lease = source.link.retain_tunnel_plans(vec![plan.clone()]);
    let mut incoming = destination.link.incoming_tunnels();
    let (mut near, mut far) = open(&source, &mut incoming, &issued, &plan).await;
    assert_eq!(source.link.auxiliary_sessions(), 1);
    let mut samples = source.link.gateway_changes();
    samples.borrow_and_update();
    scores.lock().unwrap().insert("asia".into(), 1);
    // Await the actual periodic verified probe, not a sleep or a primary disconnect.
    tokio::time::timeout(PATIENCE, samples.changed())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        *source.link.status().borrow(),
        original,
        "the live target auxiliary delays voluntary movement"
    );
    near.write_all(b"still live").await.unwrap();
    let mut bytes = [0; 10];
    far.read_exact(&mut bytes).await.unwrap();
    assert_eq!(&bytes, b"still live");
    near.closer().close();
    far.closer().close();
    let mut status = source.link.status();
    tokio::time::timeout(
        PATIENCE + Duration::from_secs(u64::from(orch_link_contract::session::HEARTBEAT_SECS)),
        async {
            loop {
                if source.link.gateway_id().as_deref() == Some("asia")
                    && matches!(
                        *status.borrow(),
                        orch_link::machine::LinkStatus::Connected { .. }
                    )
                {
                    break;
                }
                status.changed().await.unwrap();
            }
        },
    )
    .await
    .unwrap();
    eventually("one transport after idle purpose change", || {
        let gateway = regional.gateway.clone();
        let source = source.link.clone();
        let link = world.api.link.clone();
        let id = enrolled.identity.machine_id;
        async move {
            gateway.sessions_held() == 2
                && source.auxiliary_sessions() == 0
                && link.gateway_of(id).as_deref() == Some("asia")
        }
    })
    .await;
    assert_eq!(
        source.link.gateway_status().choice.selection,
        orch_link_contract::gateways::GatewaySelection::Automatic
    );
    assert_eq!(
        world.api.link.gateway_of(destination.id).as_deref(),
        Some("asia")
    );
    let next = ticket(&world, &source, &destination);
    let next_plan = world
        .api
        .link
        .tunnel_plan_for(source.id, &next)
        .await
        .unwrap();
    let (mut near, mut far) = open(&source, &mut incoming, &next, &next_plan).await;
    near.write_all(b"primary now").await.unwrap();
    let mut bytes = [0; 11];
    far.read_exact(&mut bytes).await.unwrap();
    assert_eq!(&bytes, b"primary now");
    assert_eq!(source.link.auxiliary_sessions(), 0);
    near.closer().close();
    far.closer().close();
    drop(lease);
    source.link.shutdown().await;
    destination.link.shutdown().await;
    regional.stop().await;
    world.stop().await;
}

#[tokio::test]
async fn losing_the_gateway_process_resumes_the_same_tool_sockets_and_replay_through_a_fallback() {
    retained_process_loss(false).await;
}

#[tokio::test]
async fn retained_tool_sockets_use_the_third_gateway_when_the_first_fallback_is_unreachable() {
    retained_process_loss(true).await;
}

async fn retained_process_loss(third: bool) {
    let (mut world, mut regionals) = world_with_count(60, if third { 3 } else { 2 }).await;
    let regional = regionals.remove(0);
    let (source, destination) = if third {
        let source = world.enroll("retained-source").await;
        let destination = world.enroll("retained-peer").await;
        let source = world.start_machine_on_gateway(&source, "test-gateway", vec![Role::Agent]);
        let destination = world.start_machine_with_reachability(
            &destination,
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
        eventually("retained original primaries", || {
            let link = world.api.link.clone();
            let (s, d) = (source.id, destination.id);
            async move {
                link.gateway_of(s).as_deref() == Some("test-gateway")
                    && link.gateway_of(d).as_deref() == Some("asia")
            }
        })
        .await;
        (source, destination)
    } else {
        machines(&world).await
    };
    let issued = ticket(&world, &source, &destination);
    let plan = world
        .api
        .link
        .tunnel_plan_for(source.id, &issued)
        .await
        .unwrap();
    let lease = source.link.retain_tunnel_plans(vec![plan.clone()]);
    let mut incoming = destination.link.incoming_tunnels();
    let (mut near, mut far) = open(&source, &mut incoming, &issued, &plan).await;
    let cid = near.cid();
    assert_eq!(far.cid(), cid);
    // More than a receive window, less than one replay store: the unread tail must be retained, not re-executed.
    let bytes = (0..6 * 1024 * 1024)
        .map(|index| (index % 251) as u8)
        .collect::<Vec<_>>();
    tokio::time::timeout(PATIENCE, near.write_all(&bytes))
        .await
        .unwrap()
        .unwrap();
    regional.stop().await;
    let mut received = vec![0; bytes.len()];
    tokio::time::timeout(
        PATIENCE + Duration::from_secs(u64::from(orch_link_contract::tunnels::RESUME_GRACE_SECS)),
        far.read_exact(&mut received),
    )
    .await
    .unwrap()
    .unwrap();
    assert_eq!(received, bytes);
    assert_eq!(near.cid(), cid);
    assert_eq!(far.cid(), cid);
    far.write_all(b"same native socket").await.unwrap();
    let mut response = [0; 18];
    tokio::time::timeout(PATIENCE, near.read_exact(&mut response))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(&response, b"same native socket");
    let expected = if third { "us" } else { "test-gateway" };
    let fallback = if third {
        regionals[0].gateway.clone()
    } else {
        world.gateway.gateway.clone()
    };
    eventually(
        "fallback attachments preserve the selected primaries",
        || {
            let gateway = fallback.clone();
            let source = source.link.clone();
            let destination = destination.link.clone();
            async move {
                gateway.sessions_held() == 2
                    && source.gateway_id().as_deref() == Some("test-gateway")
                    && destination.gateway_id().is_none()
                    && destination.auxiliary_sessions() == 1
            }
        },
    )
    .await;
    let thread = orch_link_contract::tunnels::Ticket::parse(&issued)
        .unwrap()
        .claims()
        .thread_id
        .unwrap();
    eventually("the API observes the resumed carrier", || {
        let link = world.api.link.clone();
        let source = source.id;
        async move { link.tool_gateway_for(thread, source).as_deref() == Some(expected) }
    })
    .await;
    assert_eq!(
        destination
            .link
            .gateway_status()
            .choice
            .gateway_id
            .as_deref(),
        Some("asia")
    );
    // There is still exactly the one destination acceptance; recovery never dispatches another OpenOffer.
    assert!(
        tokio::time::timeout(Duration::ZERO, incoming.next())
            .await
            .is_err()
    );
    near.closer().close();
    far.closer().close();
    drop(lease);
    source.link.shutdown().await;
    destination.link.shutdown().await;
    for regional in regionals {
        regional.stop().await;
    }
    world.stop().await;
}
