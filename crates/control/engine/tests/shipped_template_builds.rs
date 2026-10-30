//! Environments made from the built-in templates send their Dockerfiles to the selected host.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]

mod common;
mod hosting;

use common::World;
use hosting::{ScriptedHost, enroll, make, runtime_started_for_job, serve, wait_over};
use orch_db::seeds::{self, DEFAULT_TEMPLATE_ID, DEVELOPER_TEMPLATE_ID, Release};
use orch_link_contract::commands::CommandKind;
use serde_json::json;

#[tokio::test]
async fn each_built_in_template_starts_an_environment_by_building_its_recipe_on_the_host() {
    let world = World::new().await;
    serve(&world);
    let host = ScriptedHost::on(&world, "build-host", 16_384).await;
    let mut tx = world.pool.begin().await.unwrap();
    seeds::seed(
        &mut tx,
        &Release {
            version: "0.1.0",
            adapters: &[],
        },
        world.clock.now(),
    )
    .await
    .unwrap();
    tx.commit().await.unwrap();

    let base = include_str!("../../../../images/orch-env-base/Dockerfile");
    let developer = include_str!("../../../../images/orch-env-dev/Dockerfile");
    for (index, (template, recipe)) in [
        (DEFAULT_TEMPLATE_ID, base.to_owned()),
        (DEVELOPER_TEMPLATE_ID, format!("{base}\n{developer}")),
    ]
    .into_iter()
    .enumerate()
    {
        let made = make(&world, template, &format!("built-{index}")).await;
        let runtime = runtime_started_for_job(&world, &host, made.job.id).await;
        let _unit = enroll(&world, runtime).await;
        let (state, _, error) = wait_over(&world, made.job.id).await;
        assert_eq!(state, "succeeded", "{error:?}");
        let builds: Vec<_> = host
            .calls()
            .into_iter()
            .filter(|call| call.kind == CommandKind::NodeImageEnsure)
            .collect();
        let source = &builds[index].body["spec"];
        assert!(source["image"].is_null());
        assert_eq!(source["build"]["dockerfile"], recipe);
        assert!(source["build"]["context"].is_null());
        assert_eq!(source["build"]["args"], json!({"VERSION": "0.1.0"}));
    }
}
