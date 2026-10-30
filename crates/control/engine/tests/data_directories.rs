//! Existing data roots survive an ordinary recreate and a move asks for an explicit copy before making a job.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]
mod common;
mod hosting;
use common::World;
use hosting::{ScriptedHost, running, serve, template, wait_over};
use orch_api_contract::environment::{EnvironmentMovePreflightBody, UpdatePreflightReason};
use orch_authz::{Caller, Principal};
use orch_db::queries::environments::hosting as envs;
use orch_engine::updates::Target;
use orch_link_contract::commands::{CommandKind, CommandResult, FsManifestResult};
use orch_types::Time;
use serde_json::json;

/// The unit's connector answers a move's listing of its workspace with a list it recorded in the store,
/// an empty one: the plan is about where the data roots lie, not about the files. The directory holds the
/// store, for as long as the test keeps it.
async fn lists_its_workspace(world: &World, unit: &hosting::Unit) -> tempfile::TempDir {
    let directory = tempfile::tempdir().unwrap();
    let store = std::sync::Arc::new(
        orch_blob_local::LocalBlobStore::open(directory.path().join("content"))
            .await
            .unwrap(),
    );
    world.engine().attach_blobs(store);
    let written = orch_engine::manifest::build(
        &orch_engine::manifest::Header {
            v: orch_engine::manifest::MANIFEST_VERSION,
            root: "/workspace".to_owned(),
            generated_at: Time::new(world.clock.now()),
            generation: 1,
            case_sensitive: true,
        },
        Vec::new(),
    )
    .unwrap();
    let mut conn = world.pool.acquire().await.unwrap();
    let manifest = world
        .engine()
        .put_manifest(&mut conn, written.bytes)
        .await
        .unwrap()
        .hash;
    unit.added
        .machine
        .handler
        .on(CommandKind::FsManifest, move |_| {
            Ok(CommandResult::FsManifest(FsManifestResult {
                manifest_blob: manifest,
                file_count: 0,
                total_bytes: 0,
            }))
        });
    directory
}

#[tokio::test]
async fn recreate_keeps_borrowed_roots_and_a_move_explains_the_copy_offer() {
    let world = World::new().await;
    serve(&world);
    let source = ScriptedHost::on(&world, "Source", 16384).await;
    let target = ScriptedHost::on(&world, "Target", 16384).await;
    let template = template(&world).await;
    // The unit's connector stays online: the move lists the workspace through it, and the recreated unit is
    // attached through it.
    let (environment, old, unit) = running(&world, &source, template, "Directory data").await;
    let _store = lists_its_workspace(&world, &unit).await;
    let mut tx = world.pool.begin().await.unwrap();
    let borrowed = json!({"path":"/srv/work/existing"});
    sqlx::query(
        "UPDATE runtimes SET volumes=jsonb_set(volumes,'{workspace,directory}',$2) WHERE id=$1",
    )
    .bind(old)
    .bind(&borrowed)
    .execute(&mut *tx)
    .await
    .unwrap();
    let principal = Principal::load(
        &mut tx,
        Caller::session(world.ada),
        Time::new(world.clock.now()),
    )
    .await
    .unwrap();
    let move_plan = world
        .engine()
        .draft_move(
            &mut tx,
            &principal,
            environment,
            &EnvironmentMovePreflightBody {
                host_id: Some(target.id),
                ..Default::default()
            },
            None,
        )
        .await
        .unwrap();
    let reason = move_plan
        .blockers
        .iter()
        .find(|blocker| blocker.code == "existing_directory_on_source_machine")
        .unwrap();
    assert!(
        reason.message.contains("Copy existing directories")
            && reason.message.contains("originals are kept")
    );
    let draft = world
        .engine()
        .draft_update(
            &mut tx,
            &principal,
            Target::Environment(environment),
            UpdatePreflightReason::Recreate,
            None,
        )
        .await
        .unwrap();
    assert!(
        draft.recipe.data_sources.is_none(),
        "recreate must retain the binding, independent of defaults"
    );
    let job = orch_engine::jobs::recreate::queue(&mut tx, draft, None, world.clock.now())
        .await
        .unwrap();
    tx.commit().await.unwrap();
    world.engine().kick();
    let (state, _, error) = wait_over(&world, job.id).await;
    assert_eq!(state, "succeeded", "{error:?}");
    let mut conn = world.pool.acquire().await.unwrap();
    let current = envs::live_runtime(&mut conn, environment)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(current.volumes["workspace"]["directory"], borrowed);
    assert!(
        source
            .calls()
            .iter()
            .filter(|call| call.kind == orch_link_contract::commands::CommandKind::NodeVolumeEnsure)
            .all(|call| call.body["name"] != current.volumes["workspace"]["name"])
    );
    world.engine().stop();
}
