//! A provider's borrowed home survives recreate and crosses hosts only through an explicit whole-home copy.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]
mod common;
mod hosting;
mod runtimes;

use common::World;
use hosting::{ScriptedHost, serve, wait_over};
use orch_api_contract::environment::UpdatePreflightReason;
use orch_authz::{Caller, Principal};
use orch_db::queries::{environments::hosting as rows, storage};
use orch_engine::updates::Target;
use orch_link_contract::commands::{
    BlobRef, Command, CommandKind as K, CommandResult, Empty, ExportedVolume,
    NodeRuntimeExportResult, NodeRuntimeVolumesExportResult, ProviderRefreshResult, VolumeArchive,
};
use orch_types::{Hash, Id, Time};
use serde_json::json;

/// The provider's state, which a replacement that kept its home keeps.
async fn provider_status(world: &World, provider: Id) -> String {
    world
        .scalar("SELECT status FROM providers WHERE id = $1", provider)
        .await
}

/// The candidate's connector answers the replacement's check of the sign-in as a provider's runtime does,
/// with the state the provider is in.
fn answers_refresh(candidate: &hosting::Unit, status: String) {
    candidate
        .added
        .machine
        .handler
        .on(K::ProviderRefresh, move |_| {
            Ok(CommandResult::ProviderRefresh(ProviderRefreshResult {
                source: None,
                status: status.clone(),
                facts: None,
            }))
        });
}

async fn borrowed(world: &World, host: &ScriptedHost) -> orch_types::Id {
    runtimes::provider_template(world).await;
    world
        .set(
            world.root,
            "runtime.home",
            json!({"kind":"existing_directory","path":"/srv/harness/existing"}),
        )
        .await;
    let policy = orch_link_contract::policy::LocalPolicy {
        environment_roots: vec!["/srv/harness".into()],
        ..Default::default()
    };
    sqlx::query("UPDATE machines SET facts=jsonb_set(facts,'{policy}',$2) WHERE id=$1")
        .bind(host.machine)
        .bind(json!(policy))
        .execute(world.pool.pool())
        .await
        .unwrap();
    sqlx::query(
        "UPDATE hosts SET drivers=jsonb_set(drivers,'{docker,data_storage}',$2) WHERE id=$1",
    )
    .bind(host.id)
    .bind(json!({"made_by_orch":"volume","existing_directory":true,"reason":null}))
    .execute(world.pool.pool())
    .await
    .unwrap();
    runtimes::managed_subscription(world, "Borrowed harness home").await
}

#[tokio::test]
async fn a_borrowed_home_is_separate_from_private_state_and_recreate_keeps_its_binding() {
    let world = World::new().await;
    serve(&world);
    let host = ScriptedHost::on(&world, "Home host", 16384).await;
    let provider = borrowed(&world, &host).await;
    let (old, _unit) = runtimes::made(&world, &host, provider).await;
    let first = host
        .calls()
        .into_iter()
        .find(|call| call.kind == K::NodeRuntimeCreate)
        .unwrap();
    assert_eq!(
        first.body["volumes"]["home"]["directory"]["path"],
        "/srv/harness/existing"
    );
    assert!(first.body["volumes"]["state"].get("directory").is_none());
    assert_eq!(
        host.count(K::NodeVolumeEnsure),
        1,
        "only private state is allocated; the borrowed directory is never created or chowned"
    );
    let mut tx = world.pool.begin().await.unwrap();
    let principal = Principal::load(
        &mut tx,
        Caller::session(world.ada),
        Time::new(world.clock.now()),
    )
    .await
    .unwrap();
    let draft = world
        .engine()
        .draft_update(
            &mut tx,
            &principal,
            Target::Provider(provider),
            UpdatePreflightReason::Recreate,
            None,
        )
        .await
        .unwrap();
    assert!(draft.blockers.is_empty(), "{:?}", draft.blockers);
    assert!(draft.recipe.data_sources.is_none());
    let job = orch_engine::jobs::recreate::queue(&mut tx, draft, None, world.clock.now())
        .await
        .unwrap();
    tx.commit().await.unwrap();
    world.engine().kick();
    host.wait_for(K::NodeRuntimeStart, 2).await;
    let next = host.started().unwrap();
    let status = provider_status(&world, provider).await;
    let candidate = runtimes::enrol(&world, next).await;
    answers_refresh(&candidate, status);
    let (state, _, error) = wait_over(&world, job.id).await;
    assert_eq!(state, "succeeded", "{error:?}");
    let mut conn = world.pool.acquire().await.unwrap();
    let replacement = rows::runtime(&mut conn, next).await.unwrap().unwrap();
    let original = rows::runtime(&mut conn, old).await.unwrap().unwrap();
    assert_eq!(replacement.volumes["home"], original.volumes["home"]);
    assert_eq!(replacement.volumes["state"], original.volumes["state"]);
    world.engine().stop();
}

#[tokio::test]
async fn a_provider_home_copy_is_reviewed_and_imported_before_the_destination_unit_exists() {
    let world = World::new().await;
    serve(&world);
    let source = ScriptedHost::on(&world, "Source", 16384).await;
    let provider = borrowed(&world, &source).await;
    let (old, _unit) = runtimes::made(&world, &source, provider).await;
    let target = ScriptedHost::on(&world, "Destination", 16384).await;
    sqlx::query("UPDATE providers SET runtime_config=runtime_config || $2 WHERE id=$1")
        .bind(provider)
        .bind(json!({"runtime.host_id":target.id}))
        .execute(world.pool.pool())
        .await
        .unwrap();
    let mut conn = world.pool.acquire().await.unwrap();
    let principal = Principal::load(
        &mut conn,
        Caller::session(world.ada),
        Time::new(world.clock.now()),
    )
    .await
    .unwrap();
    let refused = world
        .engine()
        .draft_provider_update_with_copy(
            &mut conn,
            &principal,
            provider,
            UpdatePreflightReason::Update,
            false,
        )
        .await
        .unwrap();
    assert!(refused.blockers.iter().any(|blocker| blocker.code
        == "existing_directory_on_source_machine"
        && blocker.message.contains("original directory will be kept")));
    assert_eq!(
        source.count(K::NodeRuntimeStop),
        0,
        "review changes nothing on the running source"
    );
    let draft = world
        .engine()
        .draft_provider_update_with_copy(
            &mut conn,
            &principal,
            provider,
            UpdatePreflightReason::Update,
            true,
        )
        .await
        .unwrap();
    assert!(draft.blockers.is_empty(), "{:?}", draft.blockers);
    assert!(
        draft
            .recipe
            .data_sources
            .as_ref()
            .unwrap()
            .home
            .directory()
            .is_none()
    );
    assert_eq!(
        draft.recipe.volume_carry.as_ref().unwrap()["selectors"],
        json!(["home"])
    );
    let hash = Hash::of(b"provider whole-home fixture");
    storage::register_blob(&mut conn, &hash, 20, world.clock.now())
        .await
        .unwrap();
    drop(conn);
    source.answer(K::NodeRuntimeVolumesExport, move |command| {
        let Command::NodeRuntimeVolumesExport(body) = command else {
            unreachable!()
        };
        assert_eq!(body.runtime_id, old);
        assert_eq!(body.volumes.len(), 1);
        assert_eq!(
            body.volumes[0].volume.directory.as_ref().unwrap().path,
            "/srv/harness/existing"
        );
        Ok(CommandResult::NodeRuntimeVolumesExport(
            NodeRuntimeVolumesExportResult {
                volumes: vec![ExportedVolume {
                    selector: body.volumes[0].selector,
                    archive: VolumeArchive {
                        parts: vec![BlobRef { hash, size: 20 }],
                        size: 20,
                    },
                }],
            },
        ))
    });
    source.answer(K::NodeRuntimeExport, |_| {
        Ok(CommandResult::NodeRuntimeExport(NodeRuntimeExportResult {
            archive: None,
        }))
    });
    target.answer(K::NodeRuntimeVolumeImport, move |command| {
        let Command::NodeRuntimeVolumeImport(body) = command else {
            unreachable!()
        };
        assert_eq!(body.provider_id, Some(provider));
        assert!(body.selected.volume.directory.is_none());
        assert_eq!(body.archive.parts[0].hash, hash);
        Ok(CommandResult::NodeRuntimeVolumeImport(Empty {}))
    });
    let mut tx = world.pool.begin().await.unwrap();
    let job = orch_engine::jobs::recreate::queue(&mut tx, draft, None, world.clock.now())
        .await
        .unwrap();
    tx.commit().await.unwrap();
    world.engine().kick();
    target.wait_for(K::NodeRuntimeStart, 1).await;
    let next = target.started().unwrap();
    let status = provider_status(&world, provider).await;
    let candidate = runtimes::enrol(&world, next).await;
    answers_refresh(&candidate, status);
    let (state, _, error) = wait_over(&world, job.id).await;
    assert_eq!(state, "succeeded", "{error:?}");
    let kinds = target.kinds();
    assert!(
        kinds
            .iter()
            .position(|kind| *kind == K::NodeRuntimeVolumeImport)
            .unwrap()
            < kinds
                .iter()
                .position(|kind| *kind == K::NodeRuntimeCreate)
                .unwrap()
    );
    let mut conn = world.pool.acquire().await.unwrap();
    let runtime = rows::runtime(&mut conn, next).await.unwrap().unwrap();
    assert!(runtime.volumes["home"].get("directory").is_none());
    let selected: serde_json::Value =
        sqlx::query_scalar("SELECT runtime_config->'runtime.home' FROM providers WHERE id=$1")
            .bind(provider)
            .fetch_one(&mut *conn)
            .await
            .unwrap();
    assert_eq!(selected, json!({"kind":"made_by_orch"}));
    assert!(
        source
            .calls()
            .iter()
            .filter(|call| call.kind == K::NodeVolumeRemove)
            .all(|call| call.body["name"] != json!(format!("orch-home-{provider}")))
    );
    world.engine().stop();
}
