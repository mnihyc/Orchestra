//! A fork measures each owned selector on the old host, carries only those volumes through blobs, and imports before startup.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]

mod common;
mod hosting;

use common::World;
use hosting::{ScriptedHost, bind, running, serve, template};
use orch_access_contract::{decisions::ObjectRef, permissions::ResourceKind};
use orch_authz::{
    Caller,
    ownership::{NewAccess, start_resource},
};
use orch_db::queries::{
    resources::mounts::{self as mounts, LocationRow, MountRow},
    storage,
};
use orch_link_contract::commands::{
    BlobRef, Command, CommandKind, CommandResult, Empty, ExportedVolume,
    NodeRuntimeVolumesExportResult,
};
use orch_types::{Hash, Time, new_id};
use serde_json::json;

#[tokio::test]
async fn a_home_and_private_view_fork_measures_then_carries_whole_volumes_before_the_target_unit_is_created()
 {
    let world = World::new().await;
    serve(&world);
    let source = ScriptedHost::on(&world, "Source volumes", 16_384).await;
    let target = ScriptedHost::on(&world, "Target volumes", 16_384).await;
    let template = template(&world).await;
    for host in [&source, &target] {
        sqlx::query("UPDATE hosts SET drivers=jsonb_set(jsonb_set(drivers,'{docker,mounts}','\"placed\"'::jsonb),'{docker,private_views}','true'::jsonb),storages=$2 WHERE id=$1")
            .bind(host.id).bind(json!([{"name":"default","default":true,"capacity_gb":1000,"free_gb":900,"private_views":true}])).execute(world.pool.pool()).await.unwrap();
    }
    let (environment, source_runtime, _) =
        running(&world, &source, template, "Volume source").await;
    let mount = new_id();
    let mut conn = world.pool.acquire().await.unwrap();
    mounts::insert(
        &mut conn,
        &MountRow {
            id: mount,
            name: "large-data".into(),
            description: String::new(),
            org_id: Some(world.root),
            home_id: Some(world.root),
            owner_id: Some(world.ada),
            target: "/data".into(),
            read_only: true,
            enabled: true,
            default_fate: "as_is".into(),
            revision: 1,
        },
        world.clock.now(),
    )
    .await
    .unwrap();
    start_resource(
        &mut conn,
        ObjectRef {
            kind: ResourceKind::Mount.into(),
            id: mount,
        },
        world.ada,
        NewAccess::OnlyMe,
        Time::new(world.clock.now()),
    )
    .await
    .unwrap();
    for host in [&source, &target] {
        mounts::set_location(
            &mut conn,
            &LocationRow {
                mount_id: mount,
                machine_id: host.machine,
                host_id: Some(host.id),
                host_name: Some("Volumes".into()),
                kind: "path".into(),
                source: "/host/two-hundred-gigabytes".into(),
                storage: None,
                data_kind: None,
                created_by: world.ada,
                created_at: world.clock.now(),
            },
        )
        .await
        .unwrap();
    }
    sqlx::query("UPDATE runtimes SET volumes=volumes || $2 WHERE id=$1").bind(source_runtime).bind(json!({format!("over:{mount}"):{"name":format!("orch-over-{environment}-{mount}"),"storage":null}})).execute(&mut *conn).await.unwrap();
    let hash = Hash::of(b"whole volume fixture");
    storage::register_blob(&mut conn, &hash, 20, world.clock.now())
        .await
        .unwrap();
    drop(conn);
    world
        .set(
            world.topic,
            "env.mounts",
            json!({mount.to_string():"private"}),
        )
        .await;
    let thread = world.thread("Whole volumes").await;
    bind(&world, thread, environment).await;
    source.trace_calls(&world.ordered);
    target.trace_calls(&world.ordered);
    let source_machine = source.machine;
    let exporting = world.ordered.clone();
    source.added.machine.handler.on(
        CommandKind::NodeRuntimeVolumesExport,
        move |command: &Command| {
            exporting
                .lock()
                .unwrap()
                .push((source_machine, command.clone()));
            let Command::NodeRuntimeVolumesExport(body) = command else {
                unreachable!()
            };
            Ok(CommandResult::NodeRuntimeVolumesExport(
                NodeRuntimeVolumesExportResult {
                    volumes: body
                        .volumes
                        .iter()
                        .map(|volume| ExportedVolume {
                            selector: volume.selector,
                            archive: orch_link_contract::commands::VolumeArchive {
                                parts: vec![BlobRef { hash, size: 20 }, BlobRef { hash, size: 20 }],
                                size: 40,
                            },
                        })
                        .collect(),
                },
            ))
        },
    );
    let target_machine = target.machine;
    let importing = world.ordered.clone();
    target.added.machine.handler.on(
        CommandKind::NodeRuntimeVolumeImport,
        move |command: &Command| {
            importing
                .lock()
                .unwrap()
                .push((target_machine, command.clone()));
            Ok(CommandResult::NodeRuntimeVolumeImport(Empty {}))
        },
    );
    let request = serde_json::from_value(json!({"result":"new","history":{"mode":"none"},"environment":{"new":{"name":"Carried volumes","template_id":template,"host_id":target.id,"files":{"mode":"none"},"home":true}}})).unwrap();
    let caller = Caller::session(world.ada);
    let draft = world
        .engine()
        .draft_fork(&caller, thread, &request)
        .await
        .unwrap();
    hosting::wait_counts(&world, &serde_json::to_value(&draft.plan).unwrap()).await;
    let draft = world
        .engine()
        .draft_fork(&caller, thread, &request)
        .await
        .unwrap();
    assert!(draft.plan.blockers.is_empty(), "{:?}", draft.plan.blockers);
    let files = draft.plan.files.as_ref().unwrap();
    assert_eq!(
        (
            files.home.as_ref().unwrap().measured,
            files.home.as_ref().unwrap().file_count,
            files.home.as_ref().unwrap().total_bytes
        ),
        (true, 3, 8192)
    );
    assert_eq!(
        (
            files.private_views[0].measured,
            files.private_views[0].total_bytes,
            files.private_views[0].carried
        ),
        (true, 8192, true)
    );
    assert_eq!(files.total_bytes, 0, "the workspace selection is empty");
    assert_eq!(
        files.bytes_to_upload, 16384,
        "whole owned volumes are in the transfer total"
    );
    assert_eq!(files.bytes_to_download, 16384);
    let measured = world.ordered.lock().unwrap().clone();
    assert!(
        measured
            .iter()
            .any(|(machine, command)| *machine == source.machine
                && command.kind() == CommandKind::NodeRuntimeVolumesMeasure)
    );
    assert!(!measured.iter().any(|(_, command)| matches!(
        command.kind(),
        CommandKind::NodeRuntimeCreate
            | CommandKind::NodeRuntimeVolumesExport
            | CommandKind::NodeRuntimeVolumeImport
    )));
    let kept = world.engine().keep_fork_plan(&caller, draft).await.unwrap();
    let job = world
        .engine()
        .confirm_fork(
            &caller,
            thread,
            &orch_api_contract::forks::ForkCreateBody {
                notify: None,
                preflight_id: kept.preflight_id,
                options: None,
            },
        )
        .await
        .unwrap();
    target.wait_for(CommandKind::NodeRuntimeStart, 1).await;
    let _unit = hosting::enroll(&world, target.started().unwrap()).await;
    let (state, _, error) = hosting::wait_over(&world, job.id).await;
    assert_eq!(state, "succeeded", "{error:?}");
    let ordered = world.ordered.lock().unwrap().clone();
    let export = ordered
        .iter()
        .position(|(machine, command)| {
            *machine == source.machine && command.kind() == CommandKind::NodeRuntimeVolumesExport
        })
        .unwrap();
    let imports: Vec<_> = ordered
        .iter()
        .enumerate()
        .filter(|(_, (machine, command))| {
            *machine == target.machine && command.kind() == CommandKind::NodeRuntimeVolumeImport
        })
        .collect();
    assert_eq!(imports.len(), 2);
    let create = ordered
        .iter()
        .position(|(machine, command)| {
            *machine == target.machine && command.kind() == CommandKind::NodeRuntimeCreate
        })
        .unwrap();
    let start = ordered
        .iter()
        .position(|(machine, command)| {
            *machine == target.machine && command.kind() == CommandKind::NodeRuntimeStart
        })
        .unwrap();
    for (index, (_, command)) in imports {
        assert!(export < index && index < create && create < start);
        let body = command.body().unwrap();
        assert_ne!(
            body["selected"]["volume"]["name"],
            format!("orch-home-{environment}")
        );
        assert!(!body.to_string().contains("two-hundred-gigabytes"));
        assert_eq!(body["archive"]["parts"].as_array().unwrap().len(), 2);
        assert_eq!(body["archive"]["size"], 40);
    }
    let Command::NodeRuntimeVolumesExport(body) = &ordered[export].1 else {
        unreachable!()
    };
    assert_eq!(body.runtime_id, source_runtime);
    assert_eq!(body.volumes.len(), 2);
    assert!(
        body.volumes
            .iter()
            .all(|volume| volume.volume.name.starts_with("orch-home-")
                || volume.volume.name.starts_with("orch-over-"))
    );
    let until = tokio::time::Instant::now() + std::time::Duration::from_secs(10);
    loop {
        let remaining: i64 =
            sqlx::query_scalar("SELECT pin_count::bigint FROM blobs WHERE hash=$1")
                .bind(hash.as_bytes().as_slice())
                .fetch_one(world.pool.pool())
                .await
                .unwrap();
        if remaining == 0 {
            break;
        }
        assert!(
            tokio::time::Instant::now() < until,
            "transient archives were not unpinned after provisioning finished"
        );
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }
    world.engine().stop();
}
