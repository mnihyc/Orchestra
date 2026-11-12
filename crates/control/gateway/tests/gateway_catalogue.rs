//! The catalogue uses the actual verified enrollment and session entrances.
#![allow(clippy::unwrap_used, clippy::expect_used, reason = "test assertions")]

mod common;

use common::{World, request, try_request};
use orch_link::reconnect::Dialer;
use orch_link::session::{ClientIdentity, connect};
use orch_link::websocket::TlsDialer;
use orch_link_contract::enrollment::{EnrollmentToken, Trust};
use orch_link_contract::gateways::{DISCOVERY_PATH, GatewayDiscovery};
use orch_link_contract::session::{Arch, Os, Platform, Role};
use orch_pki::certificates::ServiceName;
use orch_pki::verification::{ServerTrust, pinned_client_config};
use std::sync::Arc;

#[tokio::test]
async fn an_admin_catalogue_refresh_reaches_a_primary_without_substituting_or_reconnecting_it() {
    use orch_api_contract::gateways::{
        GatewayCatalogueEntry, GatewayCatalogueGet, GatewayCatalogueUpdate,
        GatewayCatalogueUpdateBody, GatewayEndpoint, GatewayEndpointTrust,
    };
    use orch_api_core::middleware::authentication::{API_TOKEN_PREFIX, new_token, token_hash};
    let mut world = World::start().await;
    let enrolled = world.enroll("fixed-primary").await;
    let machine = world.start_machine_on_gateway(&enrolled, "test-gateway", vec![Role::Agent]);
    tokio::time::timeout(common::PATIENCE, machine.link.connected())
        .await
        .unwrap();
    common::eventually("initial primary route", || {
        let link = world.api.link.clone();
        let id = machine.id;
        async move { link.session_of(id).is_some() }
    })
    .await;
    let original_session = world.api.link.session_of(machine.id);
    sqlx::query("UPDATE users SET is_system_admin=true WHERE id=$1")
        .bind(world.user)
        .execute(world.db.pool())
        .await
        .unwrap();
    let token = new_token(API_TOKEN_PREFIX);
    let scope: orch_access_contract::key_scope::KeyScope = serde_json::from_value(serde_json::json!({"rules":[{"target":{"kind":"system","reach":"self"},"allow":[{"operation":"GatewayCatalogueGet","actions":["read"]},{"operation":"GatewayCatalogueUpdate","actions":["manage"]}]}]})).unwrap();
    orch_db::queries::identity::create_api_token_scoped(
        &mut world.db.acquire().await.unwrap(),
        &orch_db::queries::identity::NewApiToken {
            id: orch_types::new_id(),
            user_id: world.user,
            name: "Gateway metadata fixture",
            token_hash: &token_hash(&token),
            scopes: &[],
            node_id: None,
            at: world.clock.now(),
            expires_at: None,
        },
        Some(&serde_json::to_value(scope).unwrap()),
    )
    .await
    .unwrap();
    let mut registry = orch_api_core::registry::Registry::new();
    orch_api_core::routes::mount(&mut registry);
    let http = orch_api_core::testing::TestApp::over(
        world.api.app.clone(),
        orch_types::ManualClock::new(world.clock.now()),
        registry,
    );
    let prior = http
        .call(GatewayCatalogueGet, &[])
        .token(&token)
        .send()
        .await
        .ok();
    let mut gateways = prior.gateways;
    gateways[0].label = "Europe control".into();
    gateways.push(GatewayCatalogueEntry {
        id: "us".into(),
        label: "US tools".into(),
        region: "US".into(),
        endpoints: vec![GatewayEndpoint {
            url: "wss://us.orch.test:9443".into(),
            trust: GatewayEndpointTrust::Pinned,
        }],
    });
    http.call(GatewayCatalogueUpdate, &[])
        .token(&token)
        .body(&GatewayCatalogueUpdateBody {
            expected_revision: prior.revision,
            gateways,
        })
        .send()
        .await
        .ok();
    common::eventually("primary metadata refresh", || {
        let link = machine.link.clone();
        async move {
            let gateways = link.gateway_catalogue();
            gateways.len() == 2 && gateways[0].label == "Europe control"
        }
    })
    .await;
    assert_eq!(world.api.link.session_of(machine.id), original_session);
    assert_eq!(machine.link.gateway_id().as_deref(), Some("test-gateway"));
    assert_eq!(
        machine.link.gateway_status().choice.gateway_id.as_deref(),
        Some("test-gateway")
    );
    assert_eq!(machine.link.auxiliary_sessions(), 0);
    let newly_enrolled = world.enroll("new-metadata").await;
    assert_eq!(newly_enrolled.reply.gateways[0].label, "Europe control");
    assert_eq!(newly_enrolled.reply.gateways.len(), 2);
    machine.link.shutdown().await;
    world.stop().await;
}

#[tokio::test]
async fn discovery_enrollment_and_ready_advertise_the_same_labelled_gateways() {
    let mut world = World::start_with_config(axum::Router::new(), |config| {
        config.gateway.instances = vec![
            orch_config::server::GatewayInstance {
                id: config.gateway.gateway_id.clone(),
                label: "Europe".into(),
                region: "Europe".into(),
                endpoints: config.gateway.endpoints.clone(),
            },
            orch_config::server::GatewayInstance {
                id: "asia".into(),
                label: "Asia tools".into(),
                region: "Asia".into(),
                endpoints: vec![orch_config::server::GatewayEndpoint {
                    url: "wss://asia.test:9443".into(),
                    trust: "pinned".into(),
                }],
            },
        ];
    })
    .await;
    let token_text = world.token().await;
    let token: EnrollmentToken = token_text.parse().unwrap();
    let tls = pinned_client_config(
        ServerTrust::Pin(*token.pin()),
        Some(ServiceName::Gateway),
        None,
        Arc::clone(&world.clock),
    )
    .unwrap();
    let (status, body) = request(world.gateway.pinned, tls, "GET", DISCOVERY_PATH, None, &[]).await;
    assert_eq!(status, 200);
    let discovered: GatewayDiscovery = serde_json::from_slice(&body).unwrap();
    assert_eq!(
        discovered.gateways,
        world.config.gateway.advertised_gateways()
    );
    assert_eq!(
        discovered.ca_pem,
        world.secrets.authority().certificate_pem()
    );
    assert_eq!(world.gateway.gateway.sessions_held(), 0);
    let enrolled = world
        .enroll_with(&token_text, "catalogue-machine")
        .await
        .unwrap();
    assert_eq!(enrolled.reply.gateways, discovered.gateways);
    assert_eq!(
        enrolled.reply.endpoints,
        world.config.gateway.advertised_endpoints()
    );
    assert!(matches!(
        world.enroll_with(&token_text, "token-already-spent").await,
        Err((410, _))
    ));
    let dialer = TlsDialer::new(Some(world.pinned_config(&enrolled)), None);
    let connection = dialer
        .dial(&enrolled.endpoints(Trust::Pinned)[0])
        .await
        .unwrap();
    let identity = ClientIdentity {
        purpose: Default::default(),
        gateway_id: None,
        endpoints: None,
        hosted_endpoints: None,
        machine_id: enrolled.identity.machine_id,
        connector_version: "0.1.0".into(),
        platform: Platform {
            os: Os::Linux,
            arch: Arch::X86_64,
        },
        roles: vec![Role::Agent, Role::Environment],
        proof: Some((
            enrolled.identity.key.duplicate().unwrap(),
            enrolled.identity.certificate.clone(),
        )),
    };
    let established = connect(connection, &identity, world.clock.as_ref())
        .await
        .unwrap();
    assert_eq!(established.ready.gateways, discovered.gateways);
    assert_eq!(established.ready.endpoints, Some(enrolled.reply.endpoints));
    established.sender.end();
    world.stop().await;
}

#[tokio::test]
async fn discovery_needs_the_right_authority_but_neither_a_machine_nor_a_token() {
    let mut world = World::start().await;
    let token: EnrollmentToken = world.token().await.parse().unwrap();
    let tls = pinned_client_config(
        ServerTrust::Pin(*token.pin()),
        Some(ServiceName::Gateway),
        None,
        Arc::clone(&world.clock),
    )
    .unwrap();
    let (status, body) = request(world.gateway.pinned, tls, "GET", DISCOVERY_PATH, None, &[]).await;
    assert_eq!(status, 200);
    let discovery: GatewayDiscovery = serde_json::from_slice(&body).unwrap();
    assert_eq!(discovery.gateways.len(), 1);
    assert_eq!(world.gateway.gateway.sessions_held(), 0);
    let wrong = pinned_client_config(
        ServerTrust::Pin([0; 32]),
        Some(ServiceName::Gateway),
        None,
        Arc::clone(&world.clock),
    )
    .unwrap();
    assert!(
        try_request(
            world.gateway.pinned,
            wrong,
            "GET",
            DISCOVERY_PATH,
            None,
            &[]
        )
        .await
        .is_err()
    );
    world.stop().await;
}

#[tokio::test]
async fn anonymous_discovery_redacts_legacy_url_secrets_without_rewriting_authenticated_addresses()
{
    let world = World::start_with_config(axum::Router::new(), |config| {
        config.gateway.endpoints[0].url =
            config.gateway.endpoints[0]
                .url
                .replacen("://", "://fixture-user:fixture-password@", 1)
                + "?fixture-token=private#private";
    })
    .await;
    let token_text = world.token().await;
    let token: EnrollmentToken = token_text.parse().unwrap();
    let tls = pinned_client_config(
        ServerTrust::Pin(*token.pin()),
        Some(ServiceName::Gateway),
        None,
        Arc::clone(&world.clock),
    )
    .unwrap();
    let (status, body) = request(world.gateway.pinned, tls, "GET", DISCOVERY_PATH, None, &[]).await;
    assert_eq!(status, 200);
    let text = String::from_utf8(body.clone()).unwrap();
    assert!(
        !text.contains("fixture-user")
            && !text.contains("fixture-password")
            && !text.contains("fixture-token")
    );
    let discovered: GatewayDiscovery = serde_json::from_slice(&body).unwrap();
    assert!(!discovered.gateways[0].endpoints.is_empty());
    let enrolled = world
        .enroll_with(&token_text, "legacy-address-machine")
        .await
        .unwrap();
    assert_eq!(
        enrolled.reply.endpoints,
        world.config.gateway.advertised_endpoints()
    );
}
