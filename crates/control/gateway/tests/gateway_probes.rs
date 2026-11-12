//! A real TLS probe proves the gateway without a machine or API connection.
#![allow(clippy::unwrap_used, clippy::expect_used, reason = "test assertions")]

mod common;

use common::{World, request};
use orch_connector_core::gateway_selection::HttpsGatewayProber;
use orch_link::gateways::GatewayProber;
use orch_link_contract::enrollment::{Endpoint, Trust};
use orch_link_contract::gateways::PROBE_PATH;
use orch_pki::certificates::ServiceName;
use orch_pki::verification::{ServerTrust, pinned_client_config};
use std::sync::Arc;

#[tokio::test]
async fn a_verified_three_sample_probe_needs_neither_machine_admission_nor_the_api() {
    let mut world = World::start().await;
    world.api_goes_away().await;
    let prober = HttpsGatewayProber::for_enrollment(
        &world.secrets.ca_certificate(),
        Arc::clone(&world.clock),
    )
    .unwrap();
    let endpoint = Endpoint {
        url: format!("wss://{}", world.gateway.pinned),
        trust: Trust::Pinned,
    };
    let result = prober.measure(&endpoint).await.unwrap();
    assert_eq!(result.gateway_id, "test-gateway");
    assert_eq!(result.endpoint, endpoint);
    assert_eq!(world.gateway.gateway.sessions_held(), 0);
    world.stop().await;
}

#[tokio::test]
async fn a_probe_without_a_fresh_nonce_is_refused_before_any_machine_session() {
    let mut world = World::start().await;
    let tls = pinned_client_config(
        ServerTrust::Authority(Box::new(world.secrets.ca_certificate())),
        Some(ServiceName::Gateway),
        None,
        Arc::clone(&world.clock),
    )
    .unwrap();
    let (status, _) = request(world.gateway.pinned, tls, "GET", PROBE_PATH, None, &[]).await;
    assert_eq!(status, 400);
    assert_eq!(world.gateway.gateway.sessions_held(), 0);
    world.stop().await;
}
