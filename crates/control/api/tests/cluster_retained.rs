//! Cluster-shaped hosts expose optional-feature reasons and restore retained volumes through the ordinary API.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]
#[path = "../../engine/tests/common/mod.rs"]
mod common;
#[path = "../../engine/tests/hosting/mod.rs"]
mod hosting;

use orch_api_contract::environment::{
    EnvironmentDelete, EnvironmentDeleteQuery, EnvironmentGet, EnvironmentRestore,
    EnvironmentRestoreBody, EnvironmentStatus,
};
use orch_api_contract::ports::{EnvironmentPortList, EnvironmentPortListQuery};
use orch_api_contract::resources::HostGet;
use orch_api_core::middleware::authentication::{ACCESS_TOKEN_PREFIX, new_token, token_hash};
use orch_api_core::{registry::Registry, testing::TestApp};
use orch_driver_contract::driver::EnvironmentDriver;
use orch_link_contract::commands::CommandKind;
use orch_types::new_id;
use std::sync::Arc;

#[tokio::test]
async fn a_cluster_reports_its_limits_removes_only_its_unit_and_restores_the_same_files_stopped() {
    let clock = orch_testkit::clock::manual();
    let world = common::World::with_clock(Arc::new(clock.clone())).await;
    hosting::serve(&world);
    let host = hosting::ScriptedHost::on(&world, "Cluster pool", 32768).await;
    let driver = hosting::cluster::install(&world, &host).await;
    let template = hosting::template(&world).await;
    sqlx::query("UPDATE env_templates SET driver='cluster-test', spec=spec-'runtime' WHERE id=$1")
        .bind(template)
        .execute(world.pool.pool())
        .await
        .unwrap();
    let made = hosting::make(&world, template, "Retained pool files").await;
    host.wait_for(CommandKind::NodeRuntimeStart, 1).await;
    let original = host.started().unwrap();
    let _unit = hosting::enroll(&world, original).await;
    let (state, _, error) = hosting::wait_over(&world, made.job.id).await;
    assert_eq!(state, "succeeded", "{error:?}");
    let workspace = hosting::cluster::workspace(&driver, original);
    driver
        .write_volume(&workspace, "proof", b"kept across restore".to_vec())
        .unwrap();
    let volumes = driver.volumes();
    let mut registry = Registry::new();
    orch_api_core::routes::mount(&mut registry);
    let app = TestApp::with_config(
        orch_db::Db::from_pool(world.pool.pool().clone()),
        clock.clone(),
        registry,
        orch_config::ServerConfig::default(),
    );
    let token = new_token(ACCESS_TOKEN_PREFIX);
    orch_db::queries::identity::testing::insert_session(
        &mut world.pool.acquire().await.unwrap(),
        new_id(),
        world.ada,
        &token_hash(&token),
        world.clock.now() + chrono::Duration::days(400),
    )
    .await
    .unwrap();
    let shown = app
        .call(HostGet, &[&host.id])
        .token(&token)
        .send()
        .await
        .ok();
    let features = &shown
        .drivers
        .iter()
        .find(|driver| driver.name == "cluster-test")
        .unwrap()
        .availability;
    for feature in [
        "host_paths",
        "build",
        "archive",
        "kept_files",
        "private_views",
        "suspend",
        "host_ports",
        "live_mounts",
    ] {
        assert!(!features[feature].supported, "{feature}");
        assert!(
            features[feature]
                .reason
                .as_ref()
                .is_some_and(|reason| !reason.is_empty()),
            "{feature}"
        );
    }
    let ports = app
        .call(EnvironmentPortList, &[&made.id])
        .token(&token)
        .query(&EnvironmentPortListQuery::default())
        .send()
        .await
        .ok();
    assert!(ports.host_ports.is_none());
    let host_ports = ports
        .allowed
        .iter()
        .find(|audience| audience.kind == orch_api_contract::ports::PortOpeningKind::Host)
        .unwrap();
    assert!(!host_ports.allowed);
    assert_eq!(
        host_ports.reason.as_deref(),
        driver.describe().host_ports.reason.as_deref()
    );
    let accepted = app
        .call(EnvironmentDelete, &[&made.id])
        .token(&token)
        .query(&EnvironmentDeleteQuery {
            confirm: "Retained pool files".into(),
            at: None,
        })
        .send()
        .await
        .ok();
    let revision = accepted.removal.unwrap().revision;
    clock.advance(chrono::Duration::seconds(31));
    world.engine().kick();
    hosting::await_event(
        "the cluster unit is removed while its files remain",
        std::time::Duration::from_secs(60),
        || async {
            sqlx::query_scalar::<_, bool>(
                "SELECT removal_phase='kept' FROM environments WHERE id=$1",
            )
            .bind(made.id)
            .fetch_one(world.pool.pool())
            .await
            .unwrap()
        },
    )
    .await;
    assert!(driver.unit_spec(original).is_none());
    assert_eq!(driver.volumes().len(), volumes.len());
    assert_eq!(
        driver.volume_files(&workspace)["proof"],
        b"kept across restore"
    );
    let restore = app
        .call(EnvironmentRestore, &[&made.id])
        .token(&token)
        .body(&EnvironmentRestoreBody {
            expected_revision: revision,
        })
        .send()
        .await
        .ok();
    host.wait_for(CommandKind::NodeRuntimeStart, 2).await;
    let recovered = host.started().unwrap();
    assert_ne!(recovered, original);
    let _restored = hosting::enroll(&world, recovered).await;
    let (state, _, error) = hosting::wait_over(&world, restore.id).await;
    assert_eq!(state, "succeeded", "{error:?}");
    let shown = app
        .call(EnvironmentGet, &[&made.id])
        .token(&token)
        .send()
        .await
        .ok();
    assert_eq!(shown.base.status, EnvironmentStatus::Stopped);
    assert_eq!(shown.generation, 2);
    assert_eq!(hosting::cluster::workspace(&driver, recovered), workspace);
    assert_eq!(
        driver.volume_files(&workspace)["proof"],
        b"kept across restore"
    );
    assert_eq!(world.ran(CommandKind::TurnStart), 0);
    assert_eq!(host.count(CommandKind::NodeKeepFiles), 0);
    assert_eq!(host.count(CommandKind::NodeKeptImport), 0);
    assert!(
        driver
            .volumes()
            .iter()
            .all(|volume| volume.storage.as_deref() == Some("fast"))
    );
}
