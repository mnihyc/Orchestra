//! HTTP gateway metadata, write-only administration, persisted overrides and native status visibility.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    reason = "test assertions"
)]

mod places;
mod rig;
mod support;

use orch_access_contract::key_scope::KeyScope;
use orch_api_contract::common::ErrorCode;
use orch_api_contract::gateways::*;
use orch_api_core::middleware::authentication::{API_TOKEN_PREFIX, new_token, token_hash};
use orch_db::queries::{identity, resources::access_fixtures as fixtures};
use orch_types::{Id, Json, new_id};
use places::{Who, admin, person};
use serde_json::json;
use support::World;

fn gateways() -> Vec<GatewayCatalogueEntry> {
    vec![GatewayCatalogueEntry {
        id: "asia".into(),
        label: "Asia tools".into(),
        region: "Asia".into(),
        endpoints: vec![GatewayEndpoint {
            url: "wss://asia.orch.test:9443".into(),
            trust: GatewayEndpointTrust::Pinned,
        }],
    }]
}

async fn catalogue(world: &World, who: &Who) -> GatewayCatalogue {
    world
        .app
        .call(GatewayCatalogueGet, &[])
        .token(&who.token)
        .send()
        .await
        .ok()
}

async fn scoped_key(world: &World, who: &Who, operation: &str, actions: &[&str]) -> String {
    let scope: KeyScope = serde_json::from_value(json!({"rules":[{"target":{"kind":"system","reach":"self"},"allow":[{"operation":operation,"actions":actions}]}]})).unwrap();
    assert!(scope.validate().is_empty());
    let token = new_token(API_TOKEN_PREFIX);
    identity::create_api_token_scoped(
        &mut *world.conn().await,
        &identity::NewApiToken {
            id: new_id(),
            user_id: who.id(),
            name: "Gateway metadata control",
            token_hash: &token_hash(&token),
            scopes: &[],
            node_id: None,
            at: world.now(),
            expires_at: None,
        },
        Some(&serde_json::to_value(scope).unwrap()),
    )
    .await
    .unwrap();
    token
}

#[tokio::test]
async fn installation_metadata_is_readable_without_system_administration_or_token_creation() {
    let world = World::new().await;
    let owner = person(&world, "installer").await;
    let before: i64 = sqlx::query_scalar("SELECT count(*) FROM enrollment_tokens")
        .fetch_one(&world.db.pool)
        .await
        .unwrap();
    let shown = catalogue(&world, &owner).await;
    assert_eq!(shown.gateways.len(), 1);
    assert_eq!(shown.gateways[0].region, "Unspecified");
    assert_eq!(
        shown.gateways[0].endpoints[0].url,
        "wss://gateway.test:9443"
    );
    let after: i64 = sqlx::query_scalar("SELECT count(*) FROM enrollment_tokens")
        .fetch_one(&world.db.pool)
        .await
        .unwrap();
    assert_eq!(before, after);
    let key = scoped_key(&world, &owner, "GatewayCatalogueGet", &["read"]).await;
    let by_key = world
        .app
        .call(GatewayCatalogueGet, &[])
        .token(&key)
        .send()
        .await
        .ok();
    assert_eq!(shown, by_key);
}

#[tokio::test]
async fn a_manage_only_key_gets_only_the_catalogue_receipt_and_stale_writes_reveal_no_current_value()
 {
    let world = World::new().await;
    let root = admin(&world, "root").await;
    let prior = catalogue(&world, &root).await;
    let key = scoped_key(&world, &root, "GatewayCatalogueUpdate", &["manage"]).await;
    let body = GatewayCatalogueUpdateBody {
        expected_revision: prior.revision.clone(),
        gateways: gateways(),
    };
    let reply = world
        .app
        .call(GatewayCatalogueUpdate, &[])
        .token(&key)
        .body(&body)
        .send_bytes()
        .await;
    assert_eq!(reply.status, 200, "{}", String::from_utf8_lossy(&reply.raw));
    let receipt: Json = serde_json::from_slice(&reply.raw).unwrap();
    assert_eq!(receipt.as_object().unwrap().len(), 1);
    assert!(receipt["revision"].is_string());
    assert_eq!(
        world
            .app
            .call(GatewayCatalogueGet, &[])
            .token(&key)
            .send_bytes()
            .await
            .status,
        403
    );
    let changed = catalogue(&world, &root).await;
    assert_eq!(changed.gateways, gateways());
    assert_eq!(receipt["revision"], changed.revision);
    let stale = world
        .app
        .call(GatewayCatalogueUpdate, &[])
        .token(&key)
        .body(&body)
        .send_bytes()
        .await;
    assert_eq!(stale.status, 409);
    let problem = stale.problem();
    assert_eq!(problem.code, ErrorCode::ConflictStaleRevision);
    assert!(!problem.extra.contains_key("current"));
    assert!(!String::from_utf8_lossy(&stale.raw).contains("Asia tools"));
    let row = orch_db::queries::settings::row(&mut *world.conn().await, None, "gateway.instances")
        .await
        .unwrap()
        .unwrap();
    assert_eq!(row.updated_by, Some(root.id()));
    assert!(
        world
            .audit()
            .await
            .contains(&"system.settings_updated".into())
    );
}

#[tokio::test]
async fn ordinary_authority_and_shared_catalogue_validation_precede_every_write() {
    let world = World::new().await;
    let root = admin(&world, "root").await;
    let installer = person(&world, "installer").await;
    let prior = catalogue(&world, &root).await;
    let key = scoped_key(&world, &installer, "GatewayCatalogueUpdate", &["manage"]).await;
    let body = GatewayCatalogueUpdateBody {
        expected_revision: prior.revision.clone(),
        gateways: gateways(),
    };
    assert_eq!(
        world
            .app
            .call(GatewayCatalogueUpdate, &[])
            .token(&key)
            .body(&body)
            .send_bytes()
            .await
            .status,
        403
    );
    for mutation in 0..4 {
        let mut bad = body.clone();
        match mutation {
            0 => {
                bad.gateways[0].endpoints[0].url =
                    "wss://login:secret@gateway.test:9443/?token=secret".into()
            }
            1 => bad.gateways[0].label = "bad\nlabel".into(),
            2 => bad.gateways.push(bad.gateways[0].clone()),
            _ => bad.gateways.clear(),
        }
        assert_eq!(
            world
                .app
                .call(GatewayCatalogueUpdate, &[])
                .token(&root.token)
                .body(&bad)
                .send_bytes()
                .await
                .status,
            400
        );
    }
    assert_eq!(catalogue(&world, &root).await, prior);
}

#[tokio::test]
async fn the_operator_override_is_loaded_before_the_restarted_api_advertises_gateways() {
    let world = World::new().await;
    let root = admin(&world, "root").await;
    let prior = catalogue(&world, &root).await;
    world
        .app
        .call(GatewayCatalogueUpdate, &[])
        .token(&root.token)
        .body(&GatewayCatalogueUpdateBody {
            expected_revision: prior.revision,
            gateways: gateways(),
        })
        .send()
        .await
        .ok();
    let mut registry = orch_api_core::registry::Registry::new();
    orch_api_core::routes::mount(&mut registry);
    let restarted = orch_api_core::testing::TestApp::new(
        orch_db::Db::from_pool(world.db.pool.clone()),
        world.app.clock.clone(),
        registry,
    );
    restarted.app.link.start().await.unwrap();
    let shown = restarted
        .call(GatewayCatalogueGet, &[])
        .token(&root.token)
        .send()
        .await
        .ok();
    assert_eq!(shown.gateways, gateways());
    assert_eq!(
        restarted
            .app
            .link
            .certificates()
            .gateway_discovery()
            .gateways[0]
            .id,
        "asia"
    );
    restarted.app.link.shutdown();
}

#[tokio::test]
async fn an_offline_machine_keeps_its_last_selection_and_dated_samples_under_its_own_read_gate() {
    let world = World::new().await;
    let owner = person(&world, "owner").await;
    let stranger = person(&world, "stranger").await;
    let machine: Id = fixtures::insert_persons_machine(
        &mut *world.conn().await,
        owner.id(),
        "regional connector",
    )
    .await
    .unwrap();
    let measured_at = orch_types::Time::new(world.now());
    let report = json!({"choice":{"selection":"selected","gateway_id":"removed-gateway"},"measured_at":measured_at,"measurements":[{"gateway_id":"removed-gateway","endpoint":{"url":"wss://login:secret@regional.test:9443/private?token=secret","trust":"pinned"},"round_trip_ms":9}]});
    sqlx::query("UPDATE machines SET facts=jsonb_build_object('gateway',$2::jsonb) WHERE id=$1")
        .bind(machine)
        .bind(report)
        .execute(&world.db.pool)
        .await
        .unwrap();
    let reply = world
        .app
        .call(MachineGatewayGet, &[&machine])
        .token(&owner.token)
        .send_bytes()
        .await;
    assert_eq!(reply.status, 200);
    let shown: ConnectorGatewayStatus = serde_json::from_slice(&reply.raw).unwrap();
    assert_eq!(shown.selection, Some(ConnectorGatewaySelection::Selected));
    assert_eq!(
        shown.selected_gateway_id.as_deref(),
        Some("removed-gateway")
    );
    assert_eq!(shown.primary_gateway_id, None);
    assert!(!shown.connected);
    assert_eq!(shown.measured_at, Some(measured_at));
    assert_eq!(shown.measurements[0].round_trip_ms, 9);
    assert!(!String::from_utf8_lossy(&reply.raw).contains("secret"));
    assert_eq!(
        world
            .app
            .call(MachineGatewayGet, &[&machine])
            .token(&stranger.token)
            .send_bytes()
            .await
            .status,
        404
    );
}

#[tokio::test]
async fn a_thread_reports_colocation_or_unknown_and_does_not_expose_it_to_a_stranger() {
    let scene = rig::Scene::new().await;
    let thread = scene.thread().await.base.id;
    let app = &scene.rig.world.app;
    let local = app
        .call(ThreadTunnelGatewayGet, &[&thread])
        .token(&scene.ada.token)
        .send()
        .await
        .ok();
    assert_eq!(local.state, TunnelGatewayState::Local);
    assert!(local.attached_gateway_id.is_none() && local.plan.is_none());
    sqlx::query("UPDATE threads SET provider_id=NULL WHERE node_id=$1")
        .bind(thread)
        .execute(scene.rig.db.pool())
        .await
        .unwrap();
    let unknown = app
        .call(ThreadTunnelGatewayGet, &[&thread])
        .token(&scene.ada.token)
        .send()
        .await
        .ok();
    assert_eq!(unknown.state, TunnelGatewayState::Unknown);
    assert!(unknown.attached_gateway_id.is_none() && unknown.plan.is_none());
    assert_eq!(
        app.call(ThreadTunnelGatewayGet, &[&thread])
            .token(&scene.bea.token)
            .send_bytes()
            .await
            .status,
        404
    );
}
