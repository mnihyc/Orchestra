//! Actual scoped standalone provisioning keeps its own current origin without a created Thread.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]
#[path = "../../engine/tests/common/mod.rs"]
mod common;
#[path = "../../engine/tests/hosting/mod.rs"]
mod hosting;

use orch_api_contract::{
    binding::NewEnvironment,
    environment::{EnvironmentCreate, EnvironmentCreateResponse},
};
use orch_api_core::{
    middleware::authentication::{API_TOKEN_PREFIX, new_token, token_hash},
    registry::Registry,
    testing::TestApp,
};
use orch_db::queries::identity::{self, NewApiToken};
use orch_link_contract::commands::CommandKind;
use orch_types::{Id, new_id};
use serde_json::json;
use std::sync::Arc;

#[tokio::test]
async fn standalone_scope_provisions_the_real_unit_and_a_closed_origin_cannot_start_more_work() {
    for closed in [false, true] {
        let clock = orch_testkit::clock::manual();
        let world = common::World::with_clock(Arc::new(clock.clone())).await;
        hosting::serve(&world);
        let host = hosting::ScriptedHost::on(&world, "Scoped standalone", 32768).await;
        let template = hosting::template(&world).await;
        let mut registry = Registry::new();
        orch_api_core::routes::mount(&mut registry);
        let app = TestApp::with_config(
            orch_db::Db::from_pool(world.pool.pool().clone()),
            clock.clone(),
            registry,
            orch_config::ServerConfig::default(),
        );
        let id = new_id();
        let secret = new_token(API_TOKEN_PREFIX);
        let scope = json!({"rules":[
            {"target":{"kind":"org","id":world.root,"reach":"self"},"resources":["environment"],"allow":[{"operation":"EnvironmentCreate","actions":["manage"]}]},
            {"target":{"kind":"template","id":template,"reach":"self"},"resources":["template"],"allow":[{"operation":"EnvironmentCreate","actions":["read"]}]},
            {"target":{"kind":"host","id":host.id,"reach":"self"},"resources":["host"],"allow":[{"operation":"EnvironmentCreate","actions":["write"]}]}
        ]});
        identity::create_api_token_scoped(
            &mut world.pool.acquire().await.unwrap(),
            &NewApiToken {
                id,
                user_id: world.ada,
                name: "Standalone native source",
                token_hash: &token_hash(&secret),
                scopes: &[],
                node_id: None,
                at: world.clock.now(),
                expires_at: Some(world.clock.now() + chrono::Duration::hours(1)),
            },
            Some(&scope),
        )
        .await
        .unwrap();
        if closed {
            host.added
                .machine
                .handler
                .hold(CommandKind::NodeRuntimeCreate);
        }
        let reply = app
            .call(EnvironmentCreate, &[])
            .token(&secret)
            .body(&NewEnvironment {
                name: Some("Standalone native proof".into()),
                home_node_id: Some(world.root),
                template_id: Some(template),
                host_id: Some(Some(host.id)),
                ..NewEnvironment::default()
            })
            .send()
            .await;
        assert_eq!(
            reply.status.as_u16(),
            201,
            "{}",
            String::from_utf8_lossy(&reply.raw)
        );
        let made = match reply.ok() {
            EnvironmentCreateResponse::Receipt(receipt) => receipt,
            EnvironmentCreateResponse::Full(_) => {
                panic!("Manage alone cannot disclose the new singleton")
            }
        };
        let stored: serde_json::Value = world
            .scalar("SELECT plan FROM jobs WHERE id=$1", made.job_id)
            .await;
        assert_eq!(stored["creator_provision"]["standalone"], true);
        assert!(stored["creator_provision"].get("thread").is_none());
        assert_eq!(
            stored["creator_provision"]["origin"]["operation"],
            "EnvironmentCreate"
        );
        if closed {
            host.wait_began(CommandKind::NodeRuntimeCreate, 1).await;
            sqlx::query("UPDATE api_tokens SET revoked_at=$2 WHERE id=$1")
                .bind(id)
                .bind(world.clock.now())
                .execute(world.pool.pool())
                .await
                .unwrap();
            host.added.machine.handler.release();
            let (state, _, error) = hosting::wait_over(&world, made.job_id).await;
            assert_eq!(state, "failed", "{error:?}");
            assert_eq!(
                host.count(CommandKind::NodeRuntimeStart),
                0,
                "the closed origin cannot launch its next native effect"
            );
        } else {
            host.wait_for(CommandKind::NodeRuntimeStart, 1).await;
            let runtime = host.started().unwrap();
            let _unit = hosting::enroll(&world, runtime).await;
            let (state, _, error) = hosting::wait_over(&world, made.job_id).await;
            assert_eq!(state, "succeeded", "{error:?}");
            let actual: Id = world
                .scalar(
                    "SELECT machine_id FROM environments WHERE id=$1",
                    made.environment_id,
                )
                .await;
            assert!(!actual.is_nil());
        }
    }
}
