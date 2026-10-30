//! Workspace moves publish a checked target atomically and restore the source when copying fails.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]

mod common;
mod hosting;

use common::World;
use hosting::{ScriptedHost, Unit, running, serve, template, wait_over};
use orch_api_contract::environment::{EnvironmentMovePreflightBody, UpdatePreflightReason};
use orch_authz::{Caller, Principal};
use orch_blob_contract::store::{BlobStore, stream_of};
use orch_blob_local::LocalBlobStore;
use orch_db::queries::environments::hosting as envs;
use orch_engine::updates::Target;
use orch_link_contract::commands::{
    Command, CommandError, CommandKind, CommandResult, ErrorCode, FsManifestResult,
    FsMaterializeResult, FsSnapshotResult, ProcHeldResult,
};
use orch_types::{Hash, Id, Time, new_id};
use serde_json::json;
use std::sync::{
    Arc, Mutex,
    atomic::{AtomicBool, Ordering},
};

struct Fixture {
    world: World,
    source: ScriptedHost,
    target: ScriptedHost,
    environment: Id,
    old: Id,
    source_unit: Unit,
    manifest: Hash,
    suspended: Arc<AtomicBool>,
    data: Vec<u8>,
    _store: Arc<LocalBlobStore>,
    clusters: Option<(
        Arc<orch_testkit::doubles::cluster::ClusterDriver>,
        Arc<orch_testkit::doubles::cluster::ClusterDriver>,
    )>,
}

async fn fixture() -> Fixture {
    fixture_with(World::new().await, false, false).await
}

async fn fixture_with(world: World, local: bool, cluster: bool) -> Fixture {
    serve(&world);
    let source = ScriptedHost::on(&world, "Source", 16_384).await;
    let target = ScriptedHost::on(&world, "Target", 16_384).await;
    let template = if local {
        hosting::template_of(&world, &["orch-exec"], &["fake"]).await
    } else {
        template(&world).await
    };
    let clusters = if cluster {
        let source_driver = hosting::cluster::install(&world, &source).await;
        let target_driver = hosting::cluster::install(&world, &target).await;
        sqlx::query(
            "UPDATE env_templates SET driver='cluster-test', spec=spec-'runtime' WHERE id=$1",
        )
        .bind(template)
        .execute(world.pool.pool())
        .await
        .unwrap();
        Some((source_driver, target_driver))
    } else {
        None
    };
    let (environment, initial_runtime, source_unit) = if cluster {
        let made = hosting::make_with(&world, template, "Main", Some(source.id), &json!({})).await;
        source.wait_for(CommandKind::NodeRuntimeStart, 1).await;
        let runtime = source.started().unwrap();
        let unit = hosting::enroll(&world, runtime).await;
        let (state, _, error) = wait_over(&world, made.job.id).await;
        assert_eq!(state, "succeeded", "{error:?}");
        (made.id, runtime, unit)
    } else {
        running(&world, &source, template, "Main").await
    };
    let mut tx = world.pool.begin().await.unwrap();
    // This connector belongs to a provisioned unit. Real enrollment records it as managed,
    // so recreating the unit transfers its persistent identity before the old unit is reaped.
    sqlx::query("UPDATE machines SET placement = 'managed' WHERE id = $1")
        .bind(source_unit.id)
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
    let draft = world
        .engine()
        .draft_update(
            &mut tx,
            &principal,
            Target::Environment(environment),
            UpdatePreflightReason::Update,
            None,
        )
        .await
        .unwrap();
    let job = orch_engine::jobs::recreate::queue(&mut tx, draft, None, world.clock.now())
        .await
        .unwrap();
    tx.commit().await.unwrap();
    world.engine().kick();
    assert_eq!(wait_over(&world, job.id).await.0, "succeeded");
    let hosting = world.engine().config().hosting;
    // Observe cleanup itself, rather than relying on planning the move before the reaper runs.
    // The observer covers scheduling and lease handover plus the stop and removal commands.
    hosting::await_event(
        "the replaced unit is removed",
        hosting.reaper_every + hosting.job_lease + hosting.stop_wait * 2,
        || async {
            envs::runtime(&mut world.pool.acquire().await.unwrap(), initial_runtime)
                .await
                .unwrap()
                .unwrap()
                .state
                == "removed"
        },
    )
    .await;
    let mut conn = world.pool.acquire().await.unwrap();
    let old = envs::live_runtime(&mut conn, environment)
        .await
        .unwrap()
        .unwrap()
        .id;
    let identity: (String, bool, Option<Id>) =
        sqlx::query_as("SELECT status, online, runtime_id FROM machines WHERE id = $1")
            .bind(source_unit.id)
            .fetch_one(&mut *conn)
            .await
            .unwrap();
    assert_eq!(identity, ("active".to_owned(), true, Some(old)));
    drop(conn);
    let root = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../../.tmp/upd-move-blobs")
        .join(new_id().to_string());
    let store = Arc::new(LocalBlobStore::open(root).await.unwrap());
    world.engine().attach_blobs(store.clone());
    let data = b"persistent workspace data".to_vec();
    let file = Hash::of(&data);
    store
        .put(
            &file,
            Some(data.len() as i64),
            stream_of(vec![data.clone().into()]),
        )
        .await
        .unwrap();
    let mut conn = world.pool.acquire().await.unwrap();
    orch_db::queries::storage::register_blob(
        &mut conn,
        &file,
        data.len() as i64,
        world.clock.now(),
    )
    .await
    .unwrap();
    let header = json!({"v":1,"root":"/workspace","generated_at":Time::new(world.clock.now()),"generation":2,"case_sensitive":true});
    let entry = json!({"p":"proof","t":"f","s":data.len(),"m":world.clock.now().timestamp_nanos_opt().unwrap(),"x":420,"h":file,"l":null});
    let manifest = world
        .engine()
        .put_manifest(
            &mut conn,
            zstd::stream::encode_all(format!("{header}\n{entry}\n").as_bytes(), 0).unwrap(),
        )
        .await
        .unwrap()
        .hash;
    sqlx::query("UPDATE environments SET facts = jsonb_set(facts,'{suspend}',$2) WHERE id = $1")
        .bind(environment)
        .bind(json!(!cluster))
        .execute(&mut *conn)
        .await
        .unwrap();
    drop(conn);
    let bytes = data.len() as i64;
    if let Some((source_driver, target_driver)) = &clusters {
        use orch_driver_contract::runtime::VolumeRole;
        use orch_driver_contract::volumes::OwnedVolumeSelector;
        use std::collections::BTreeMap;
        let home = source_driver.unit_spec(old).unwrap().volumes[&VolumeRole::Home]
            .name
            .clone();
        source_driver
            .write_volume(
                &home,
                ".orch/file-states/roots/original.json",
                b"original root metadata".to_vec(),
            )
            .unwrap();
        source_driver
            .write_volume(
                &home,
                ".orch/file-states/source-credential-interrupted/password",
                b"temporary credential".to_vec(),
            )
            .unwrap();
        source_driver
            .write_volume(&home, "unselected-home", b"left behind".to_vec())
            .unwrap();
        let mut archives = BTreeMap::new();
        let mut content = BTreeMap::new();
        for selector in [OwnedVolumeSelector::Home, OwnedVolumeSelector::FileStates] {
            let data = source_driver.volume_archive(&home, selector).unwrap();
            let hash = Hash::of(&data);
            let size = i64::try_from(data.len()).unwrap();
            store
                .put(&hash, Some(size), stream_of(vec![data.clone().into()]))
                .await
                .unwrap();
            orch_db::queries::storage::register_blob(
                &mut world.pool.acquire().await.unwrap(),
                &hash,
                size,
                world.clock.now(),
            )
            .await
            .unwrap();
            content.insert(hash, data);
            archives.insert(
                selector,
                orch_link_contract::commands::VolumeArchive {
                    parts: vec![orch_link_contract::commands::BlobRef { hash, size }],
                    size,
                },
            );
        }
        let source_driver = source_driver.clone();
        source.answer(CommandKind::NodeRuntimeVolumesExport, move |command| {
            let Command::NodeRuntimeVolumesExport(request) = command else {
                unreachable!()
            };
            assert_eq!(request.runtime_id, old);
            assert!(source_driver.unit_spec(old).is_some());
            Ok(CommandResult::NodeRuntimeVolumesExport(
                orch_link_contract::commands::NodeRuntimeVolumesExportResult {
                    volumes: request
                        .volumes
                        .iter()
                        .map(|selected| {
                            assert_eq!(selected.volume.name, home);
                            orch_link_contract::commands::ExportedVolume {
                                selector: selected.selector,
                                archive: archives[&selected.selector].clone(),
                            }
                        })
                        .collect(),
                },
            ))
        });
        let target_driver = target_driver.clone();
        target.answer(CommandKind::NodeRuntimeVolumeImport, move |command| {
            let Command::NodeRuntimeVolumeImport(request) = command else {
                unreachable!()
            };
            assert!(target_driver.unit_spec(request.runtime_id).is_none());
            let mut archive = Vec::new();
            for part in &request.archive.parts {
                let bytes = &content[&part.hash];
                assert_eq!(i64::try_from(bytes.len()).unwrap(), part.size);
                archive.extend_from_slice(bytes);
            }
            assert_eq!(i64::try_from(archive.len()).unwrap(), request.archive.size);
            target_driver
                .apply_volume_archive(
                    &request.selected.volume.name,
                    request.selected.selector,
                    &archive,
                )
                .map_err(|error| CommandError::new(ErrorCode::Conflict, error.to_string()))?;
            Ok(CommandResult::NodeRuntimeVolumeImport(
                orch_link_contract::commands::Empty {},
            ))
        });
    } else {
        // The move also carries its owned home/file-state selectors. Supply the exact
        // requested archive replies backed by a blob already present in this fixture;
        // these scripted replies prove transport/transaction behavior, not native decoding.
        let archive_part = orch_link_contract::commands::BlobRef {
            hash: file,
            size: bytes,
        };
        source.answer(CommandKind::NodeRuntimeVolumesExport, move |command| {
            let Command::NodeRuntimeVolumesExport(request) = command else {
                unreachable!()
            };
            assert_eq!(request.runtime_id, old);
            Ok(CommandResult::NodeRuntimeVolumesExport(
                orch_link_contract::commands::NodeRuntimeVolumesExportResult {
                    volumes: request
                        .volumes
                        .iter()
                        .map(|volume| orch_link_contract::commands::ExportedVolume {
                            selector: volume.selector,
                            archive: orch_link_contract::commands::VolumeArchive {
                                parts: vec![archive_part],
                                size: bytes,
                            },
                        })
                        .collect(),
                },
            ))
        });
        target.answer(CommandKind::NodeRuntimeVolumeImport, |command| {
            let Command::NodeRuntimeVolumeImport(_) = command else {
                unreachable!()
            };
            Ok(CommandResult::NodeRuntimeVolumeImport(
                orch_link_contract::commands::Empty {},
            ))
        });
    }
    source_unit
        .added
        .machine
        .handler
        .on(CommandKind::FsManifest, move |_| {
            Ok(CommandResult::FsManifest(FsManifestResult {
                manifest_blob: manifest,
                file_count: 1,
                total_bytes: bytes,
            }))
        });
    source_unit
        .added
        .machine
        .handler
        .on(CommandKind::FsSnapshot, move |_| {
            Ok(CommandResult::FsSnapshot(FsSnapshotResult {
                manifest_blob: manifest,
                file_count: 1,
                total_bytes: bytes,
                uploaded_bytes: 0,
                missing: 0,
            }))
        });
    let suspended = Arc::new(AtomicBool::new(false));
    let flag = suspended.clone();
    source_unit
        .added
        .machine
        .handler
        .on(CommandKind::ProcSuspend, move |_| {
            flag.store(true, Ordering::SeqCst);
            Ok(CommandResult::ProcSuspend(ProcHeldResult { processes: 1 }))
        });
    let flag = suspended.clone();
    source_unit
        .added
        .machine
        .handler
        .on(CommandKind::ProcResume, move |_| {
            flag.store(false, Ordering::SeqCst);
            Ok(CommandResult::ProcResume(ProcHeldResult { processes: 1 }))
        });
    Fixture {
        world,
        source,
        target,
        environment,
        old,
        source_unit,
        manifest,
        suspended,
        data,
        _store: store,
        clusters,
    }
}

async fn queue(f: &Fixture) -> orch_db::queries::jobs::JobRow {
    let mut tx = f.world.pool.begin().await.unwrap();
    let principal = Principal::load(
        &mut tx,
        Caller::session(f.world.ada),
        Time::new(f.world.clock.now()),
    )
    .await
    .unwrap();
    let draft = hosting::ready_move(
        &f.world,
        &mut tx,
        &principal,
        f.environment,
        &EnvironmentMovePreflightBody {
            machine_id: None,
            template_id: None,
            copy_directories: None,
            host_id: Some(f.target.id),
            home: None,
            private_views: None,
            files: None,
            check_fingerprints: None,
            fingerprint_request_id: None,
            mounts: None,
        },
        None,
    )
    .await
    .unwrap();
    assert!(draft.blockers.is_empty(), "{:?}", draft.blockers);
    let job = orch_engine::jobs::environment_move::queue(&mut tx, draft, None, f.world.clock.now())
        .await
        .unwrap();
    tx.commit().await.unwrap();
    f.world.engine().kick();
    job
}

#[tokio::test]
async fn a_move_reviews_and_carries_the_exact_kept_image_before_creation_then_releases_its_source_and_pin()
 {
    let f = fixture().await;
    let capture = new_id();
    let generation = envs::runtime(&mut f.world.pool.acquire().await.unwrap(), f.old)
        .await
        .unwrap()
        .unwrap()
        .generation;
    let image = json!({"system":"test","environment":f.environment,"generation":generation,"capture":capture,"reference":format!("orch-keep-{}:{generation}",f.environment),"bytes":64<<20,"total_bytes":72<<20,"layers":2});
    sqlx::query("UPDATE runtimes SET kept_image=$2 WHERE id=$1")
        .bind(f.old)
        .bind(image.clone())
        .execute(f.world.pool.pool())
        .await
        .unwrap();
    let archive = orch_link_contract::commands::BlobRef {
        hash: Hash::of(&f.data),
        size: f.data.len() as i64,
    };
    let exported = Arc::new(AtomicBool::new(false));
    let flag = exported.clone();
    let wanted = image.clone();
    f.source
        .added
        .machine
        .handler
        .on(CommandKind::NodeKeptExport, move |command| {
            let Command::NodeKeptExport(body) = command else {
                return Err(CommandError::new(
                    ErrorCode::Unsupported,
                    "not a kept-image export",
                ));
            };
            assert_eq!(serde_json::to_value(&body.image).unwrap(), wanted);
            flag.store(true, Ordering::SeqCst);
            Ok(CommandResult::NodeKeptExport(archive))
        });
    let mut conn = f.world.pool.acquire().await.unwrap();
    let principal = Principal::load(
        &mut conn,
        Caller::session(f.world.ada),
        Time::new(f.world.clock.now()),
    )
    .await
    .unwrap();
    let reviewed = hosting::ready_move(
        &f.world,
        &mut conn,
        &principal,
        f.environment,
        &EnvironmentMovePreflightBody {
            machine_id: None,
            template_id: None,
            copy_directories: None,
            host_id: Some(f.target.id),
            home: None,
            private_views: None,
            files: None,
            check_fingerprints: None,
            fingerprint_request_id: None,
            mounts: None,
        },
        None,
    )
    .await
    .unwrap();
    assert_eq!(reviewed.recipe.files.kept_image_bytes, Some(64 << 20));
    assert!(reviewed.recipe.files.bytes_to_download >= 72 << 20);
    drop(conn);
    let job = queue(&f).await;
    f.target.wait_for(CommandKind::NodeRuntimeStart, 1).await;
    let unit = hosting::enroll(&f.world, f.target.started().unwrap()).await;
    unit.added
        .machine
        .handler
        .on(CommandKind::FsMaterialize, |_| {
            Ok(CommandResult::FsMaterialize(FsMaterializeResult {
                files: 1,
                bytes: 25,
                skipped: 0,
            }))
        });
    let (state, _, error) = wait_over(&f.world, job.id).await;
    assert_eq!(state, "succeeded", "{error:?}");
    assert!(exported.load(Ordering::SeqCst));
    let calls = f.target.calls();
    let import = calls
        .iter()
        .position(|call| call.kind == CommandKind::NodeKeptImport)
        .unwrap();
    let create = calls
        .iter()
        .position(|call| call.kind == CommandKind::NodeRuntimeCreate)
        .unwrap();
    assert!(import < create);
    assert_eq!(calls[create].body["kept_image"], image);
    common::eventually("the old host's image copy is removed", || async {
        f.source.count(CommandKind::NodeKeptRemove) > 0
    })
    .await;
    common::eventually("the image archive pin is released", || async {
        sqlx::query_scalar::<_, i32>("SELECT pin_count FROM blobs WHERE hash=$1")
            .bind(Hash::of(&f.data).as_bytes().as_slice())
            .fetch_one(f.world.pool.pool())
            .await
            .unwrap()
            == 0
    })
    .await;
    assert_eq!(
        f.target.count(CommandKind::NodeKeptRemove),
        0,
        "success keeps the destination image"
    );
    f.world.engine().stop();
}

#[tokio::test]
async fn a_destination_without_kept_file_transfer_returns_its_reason_in_the_move_plan() {
    let f = fixture().await;
    let generation = envs::runtime(&mut f.world.pool.acquire().await.unwrap(), f.old)
        .await
        .unwrap()
        .unwrap()
        .generation;
    let image = json!({"system":"test","environment":f.environment,"generation":generation,"capture":new_id(),"reference":"owned-kept-image","bytes":64<<20,"total_bytes":72<<20,"layers":1});
    sqlx::query("UPDATE runtimes SET kept_image=$2 WHERE id=$1")
        .bind(f.old)
        .bind(image)
        .execute(f.world.pool.pool())
        .await
        .unwrap();
    sqlx::query(
        "UPDATE hosts SET drivers=jsonb_set(drivers,'{docker,kept_file_transfer}',$2) WHERE id=$1",
    )
    .bind(f.target.id)
    .bind(json!({"supported":false,"reason":"this destination accepts immutable images only"}))
    .execute(f.world.pool.pool())
    .await
    .unwrap();
    let mut conn = f.world.pool.acquire().await.unwrap();
    let principal = Principal::load(
        &mut conn,
        Caller::session(f.world.ada),
        Time::new(f.world.clock.now()),
    )
    .await
    .unwrap();
    let draft = hosting::ready_move(
        &f.world,
        &mut conn,
        &principal,
        f.environment,
        &EnvironmentMovePreflightBody {
            machine_id: None,
            template_id: None,
            copy_directories: None,
            host_id: Some(f.target.id),
            home: None,
            private_views: None,
            files: None,
            mounts: None,
            check_fingerprints: None,
            fingerprint_request_id: None,
        },
        None,
    )
    .await
    .unwrap();
    assert!(
        draft
            .blockers
            .iter()
            .any(|blocker| blocker.code == "kept_image_transfer_unavailable"
                && blocker.message.contains("immutable images only")),
        "{:?}",
        draft.blockers
    );
    assert_eq!(f.source.count(CommandKind::NodeKeptExport), 0);
    assert_eq!(f.target.count(CommandKind::NodeKeptImport), 0);
}

#[tokio::test]
async fn a_source_class_volume_limit_is_shown_alongside_existing_move_blockers() {
    use orch_driver_contract::isolation::{
        Boundary, RuntimeClass, RuntimeLimit, SUBJECT_VOLUME_CARRY,
    };
    let f = fixture().await;
    let mut class = RuntimeClass::new("runc", Boundary::SharedKernel);
    let sentence = "To take this environment to another host, copy its files first: Kubernetes volumes do not travel with a move.";
    class.limits.push(RuntimeLimit {
        key: "kubernetes_volume_move".into(),
        touches: vec![SUBJECT_VOLUME_CARRY.into()],
        todo: None,
        why: sentence.into(),
    });
    sqlx::query("UPDATE hosts SET drivers=jsonb_set(jsonb_set(drivers,'{docker,runtimes}',$2),'{docker,default_runtime}', '\"runc\"'::jsonb) WHERE id=$1")
        .bind(f.source.id)
        .bind(json!([class]))
        .execute(f.world.pool.pool())
        .await
        .unwrap();
    sqlx::query("UPDATE machines SET facts=jsonb_set(facts,'{policy,environment.remote_write}','false'::jsonb) WHERE id=(SELECT machine_id FROM hosts WHERE id=$1)")
        .bind(f.target.id)
        .execute(f.world.pool.pool())
        .await
        .unwrap();
    sqlx::query("UPDATE runtimes SET volumes=volumes-'home' WHERE id=$1")
        .bind(f.old)
        .execute(f.world.pool.pool())
        .await
        .unwrap();
    let mut conn = f.world.pool.acquire().await.unwrap();
    let principal = Principal::load(
        &mut conn,
        Caller::session(f.world.ada),
        Time::new(f.world.clock.now()),
    )
    .await
    .unwrap();
    let draft = hosting::ready_move(
        &f.world,
        &mut conn,
        &principal,
        f.environment,
        &EnvironmentMovePreflightBody {
            machine_id: None,
            template_id: None,
            copy_directories: None,
            host_id: Some(f.target.id),
            home: None,
            private_views: None,
            files: None,
            mounts: None,
            check_fingerprints: None,
            fingerprint_request_id: None,
        },
        None,
    )
    .await
    .unwrap();
    assert!(
        draft.blockers.iter().any(
            |blocker| blocker.code == "runtime_limit.kubernetes_volume_move"
                && blocker.message == sentence
        ),
        "{:?}",
        draft.blockers
    );
    assert!(
        draft
            .blockers
            .iter()
            .any(|blocker| blocker.code == "policy.remote_write_disabled"),
        "{:?}",
        draft.blockers
    );
    assert!(
        draft
            .blockers
            .iter()
            .any(|blocker| blocker.code == "file_states_copy_unavailable"),
        "{:?}",
        draft.blockers
    );
    assert_eq!(f.target.count(CommandKind::NodeRuntimeCreate), 0);
}

#[tokio::test]
async fn a_class_advisory_does_not_itself_forbid_a_supported_volume_carry() {
    use orch_driver_contract::isolation::{
        Boundary, RuntimeClass, RuntimeLimit, SUBJECT_VOLUME_CARRY,
    };
    let f = fixture().await;
    let mut class = RuntimeClass::new("runc", Boundary::SharedKernel);
    class.limits.push(RuntimeLimit {
        key: "slow_volume_copy".into(),
        touches: vec![SUBJECT_VOLUME_CARRY.into()],
        todo: None,
        why: "The copy can take time.".into(),
    });
    sqlx::query("UPDATE hosts SET drivers=jsonb_set(jsonb_set(drivers,'{docker,runtimes}',$2),'{docker,default_runtime}', '\"runc\"'::jsonb) WHERE id=$1")
        .bind(f.source.id)
        .bind(json!([class]))
        .execute(f.world.pool.pool())
        .await
        .unwrap();
    let mut conn = f.world.pool.acquire().await.unwrap();
    let principal = Principal::load(
        &mut conn,
        Caller::session(f.world.ada),
        Time::new(f.world.clock.now()),
    )
    .await
    .unwrap();
    let draft = hosting::ready_move(
        &f.world,
        &mut conn,
        &principal,
        f.environment,
        &EnvironmentMovePreflightBody {
            machine_id: None,
            template_id: None,
            copy_directories: None,
            host_id: Some(f.target.id),
            home: None,
            private_views: None,
            files: None,
            mounts: None,
            check_fingerprints: None,
            fingerprint_request_id: None,
        },
        None,
    )
    .await
    .unwrap();
    assert!(draft.blockers.is_empty(), "{:?}", draft.blockers);
}

#[tokio::test]
async fn a_workspace_move_keeps_its_environment_and_owner_and_materializes_the_promised_content() {
    Box::pin(workspace_move(false, false)).await;
}

#[tokio::test]
async fn a_legacy_managed_move_records_and_consumes_the_destination_host_use_entry() {
    Box::pin(workspace_move(true, false)).await;
}

#[tokio::test]
async fn a_registration_claiming_the_last_slot_during_legacy_move_copy_keeps_the_source_intact() {
    Box::pin(workspace_move(true, true)).await;
}

async fn workspace_move(limited: bool, fill_slot: bool) {
    workspace_move_with_cluster(limited, fill_slot, false).await;
}

#[tokio::test]
async fn an_image_only_cluster_passes_the_workspace_move_flow_without_kept_layers_or_native_suspension()
 {
    workspace_move_with_cluster(false, false, true).await;
}

async fn workspace_move_with_cluster(limited: bool, fill_slot: bool, cluster: bool) {
    let f = fixture_with(World::new().await, false, cluster).await;
    let entry = if limited {
        Some(
            hosting::limited_physical_use(
                &f.world,
                orch_access_contract::permissions::ResourceKind::Host,
                f.target.id,
                1,
            )
            .await,
        )
    } else {
        None
    };
    let job = queue(&f).await;
    let stored: orch_engine::jobs::environment_move::MoveRecipe =
        serde_json::from_value(job.plan.clone()).unwrap();
    assert!(
        stored.files.home.is_none(),
        "this move leaves the other home contents behind"
    );
    assert!(stored.files.file_states.as_ref().unwrap().complete);
    assert!(
        stored.files.bytes_to_download
            >= stored.files.total_bytes + stored.files.file_states.as_ref().unwrap().total_bytes
    );
    f.target.wait_for(CommandKind::NodeRuntimeStart, 1).await;
    let next = f.target.started().unwrap();
    let unit = if fill_slot {
        hosting::enroll_holding(&f.world, next, CommandKind::FsMaterialize).await
    } else {
        hosting::enroll(&f.world, next).await
    };
    let written = Arc::new(Mutex::new(Vec::<u8>::new()));
    let out = written.clone();
    let data = f.data.clone();
    let expected = f.manifest;
    let target_cluster = f.clusters.as_ref().map(|(_, target)| target.clone());
    unit.added
        .machine
        .handler
        .on(CommandKind::FsMaterialize, move |command| {
            let Command::FsMaterialize(body) = command else {
                return Err(CommandError::new(
                    ErrorCode::Unsupported,
                    "not a materialization",
                ));
            };
            if body.manifest_blob != expected {
                return Err(CommandError::new(
                    ErrorCode::Conflict,
                    "not the promised workspace",
                ));
            }
            *out.lock().unwrap() = data.clone();
            if let Some(driver) = &target_cluster {
                let volume = hosting::cluster::workspace(driver, next);
                driver.write_volume(&volume, "proof", data.clone()).unwrap();
            }
            Ok(CommandResult::FsMaterialize(FsMaterializeResult {
                files: 1,
                bytes: data.len() as i64,
                skipped: 0,
            }))
        });
    let competitor = if fill_slot {
        common::eventually("the legacy move is held during copy", || async {
            unit.added
                .machine
                .handler
                .executions()
                .iter()
                .any(|run| run.kind == CommandKind::FsMaterialize)
        })
        .await;
        let made = hosting::try_make(
            &f.world,
            stored.replacement.template,
            "Slot competitor",
            Some(f.target.id),
            &json!({}),
        )
        .await
        .unwrap();
        let competitor_runtime =
            hosting::runtime_started_for_job(&f.world, &f.target, made.job.id).await;
        let _unit = hosting::enroll(&f.world, competitor_runtime).await;
        let (state, _, error) = wait_over(&f.world, made.job.id).await;
        assert_eq!(
            state, "succeeded",
            "the admitted reservation provisions: {error:?}"
        );
        Some((made.id, _unit))
    } else {
        None
    };
    unit.let_go();
    let (state, _, error) = wait_over(&f.world, job.id).await;
    if let Some((competitor, _competitor_unit)) = competitor {
        assert_eq!(
            state, "failed",
            "the already committed registration owns the last slot: {error:?}"
        );
        let current = envs::live_runtime(&mut f.world.pool.acquire().await.unwrap(), f.environment)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(current.id, f.old);
        assert_eq!(current.host_id, f.source.id);
        let registered: Vec<Id> = sqlx::query_scalar(
            "SELECT id FROM environments WHERE host_entry_id=$1 AND deleted_at IS NULL",
        )
        .bind(entry.unwrap())
        .fetch_all(f.world.pool.pool())
        .await
        .unwrap();
        assert_eq!(registered, [competitor]);
        f.world.engine().stop();
        return;
    }
    assert_eq!(state, "succeeded", "{error:?}");
    let source = f.source.calls();
    assert!(
        source
            .iter()
            .any(|call| call.kind == CommandKind::NodeRuntimeVolumesExport
                && call.body["volumes"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .any(|volume| volume["selector"] == "file_states")),
        "the immutable store travels even when home is false"
    );
    let target_calls = f.target.calls();
    let import = target_calls
        .iter()
        .position(|call| {
            call.kind == CommandKind::NodeRuntimeVolumeImport
                && call.body["selected"]["selector"] == "file_states"
        })
        .unwrap();
    let create = target_calls
        .iter()
        .position(|call| call.kind == CommandKind::NodeRuntimeCreate)
        .unwrap();
    assert!(
        import < create,
        "the successor first starts with its original store already present"
    );
    assert_eq!(*written.lock().unwrap(), f.data);
    if let Some((_, driver)) = &f.clusters {
        let workspace = hosting::cluster::workspace(driver, next);
        assert_eq!(driver.volume_files(&workspace)["proof"], f.data);
        let home = driver.unit_spec(next).unwrap().volumes
            [&orch_driver_contract::runtime::VolumeRole::Home]
            .name
            .clone();
        assert_eq!(
            driver.volume_files(&home),
            std::collections::BTreeMap::from([(
                ".orch/file-states/roots/original.json".into(),
                b"original root metadata".to_vec()
            )]),
            "the immutable store is decoded into the successor without transient credentials or unrelated home files"
        );
        assert_eq!(f.source.count(CommandKind::NodeKeepFiles), 0);
        assert_eq!(f.target.count(CommandKind::NodeKeptImport), 0);
        assert_eq!(
            unit.added
                .machine
                .handler
                .executions()
                .iter()
                .filter(|run| run.kind == CommandKind::ProcSuspend)
                .count(),
            0
        );
    }
    let mut conn = f.world.pool.acquire().await.unwrap();
    let environment = envs::environment(&mut conn, f.environment)
        .await
        .unwrap()
        .unwrap();
    let current = envs::live_runtime(&mut conn, f.environment)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(environment.owner_id, f.world.ada);
    assert_eq!(environment.machine_id, Some(unit.id));
    assert_eq!(current.host_id, f.target.id);
    assert_eq!(current.id, next);
    assert_eq!(environment.status, "running");
    if let Some(entry) = entry {
        let recorded: Option<Id> =
            sqlx::query_scalar("SELECT host_entry_id FROM environments WHERE id=$1")
                .bind(f.environment)
                .fetch_one(&mut *conn)
                .await
                .unwrap();
        assert_eq!(recorded, Some(entry));
        assert_eq!(
            orch_db::queries::environments::environments_made_through(&mut conn, &[entry])
                .await
                .unwrap(),
            1
        );
    }
    let moved: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM audit_log WHERE environment_id = $1 AND action = 'environment.moved'",
    )
    .bind(f.environment)
    .fetch_one(&mut *conn)
    .await
    .unwrap();
    assert_eq!(moved, 1);
    assert_eq!(
        envs::runtime(&mut conn, f.old)
            .await
            .unwrap()
            .unwrap()
            .state,
        "retired"
    );
    assert!(f.source.calls().iter().any(|c|c.kind==CommandKind::NodeRuntimeStop && c.body["runtime_id"]==json!(f.old)));
    f.world.engine().stop();
}

#[tokio::test]
async fn a_move_that_fails_copying_removes_its_candidate_and_resumes_the_source() {
    let f = fixture().await;
    let before = envs::environment(&mut f.world.pool.acquire().await.unwrap(), f.environment)
        .await
        .unwrap()
        .unwrap();
    let job = queue(&f).await;
    f.target.wait_for(CommandKind::NodeRuntimeStart, 1).await;
    let next = f.target.started().unwrap();
    let unit = hosting::enroll(&f.world, next).await;
    unit.added
        .machine
        .handler
        .on(CommandKind::FsMaterialize, |_| {
            Err(CommandError::new(
                ErrorCode::Conflict,
                "the destination cannot write the files",
            ))
        });
    let (state, _, error) = wait_over(&f.world, job.id).await;
    assert_eq!(state, "failed");
    assert_eq!(error.unwrap()["undone"], true);
    assert!(!f.suspended.load(Ordering::SeqCst));
    let mut conn = f.world.pool.acquire().await.unwrap();
    let after = envs::environment(&mut conn, f.environment)
        .await
        .unwrap()
        .unwrap();
    let current = envs::live_runtime(&mut conn, f.environment)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(current.id, f.old);
    assert_eq!(current.host_id, f.source.id);
    assert_eq!(after.machine_id, Some(f.source_unit.id));
    assert_eq!(after.generation, before.generation);
    assert_eq!(after.owner_id, before.owner_id);
    assert_eq!(after.status, "running");
    assert_eq!(
        envs::runtime(&mut conn, next).await.unwrap().unwrap().state,
        "removed"
    );
    assert_eq!(f.target.count(CommandKind::NodeVolumeRemove), 3);
    f.world.engine().stop();
}

#[tokio::test]
async fn a_move_resumes_the_source_when_destination_cleanup_has_to_wait_for_the_reaper() {
    let f = fixture().await;
    f.target.refuse(
        CommandKind::NodeRuntimeRemove,
        CommandError::new(ErrorCode::Conflict, "the destination cannot remove it yet"),
    );
    let job = queue(&f).await;
    f.target.wait_for(CommandKind::NodeRuntimeStart, 1).await;
    let next = f.target.started().unwrap();
    let unit = hosting::enroll(&f.world, next).await;
    unit.added
        .machine
        .handler
        .on(CommandKind::FsMaterialize, |_| {
            Err(CommandError::new(
                ErrorCode::Conflict,
                "the destination cannot write the files",
            ))
        });
    let (state, _, error) = wait_over(&f.world, job.id).await;
    assert_eq!(state, "failed");
    assert_eq!(error.unwrap()["undone"], false);
    assert!(!f.suspended.load(Ordering::SeqCst));
    let mut conn = f.world.pool.acquire().await.unwrap();
    let current = envs::live_runtime(&mut conn, f.environment)
        .await
        .unwrap()
        .unwrap();
    let environment = envs::environment(&mut conn, f.environment)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(current.id, f.old);
    assert_eq!(environment.status, "running");
    assert_eq!(environment.machine_id, Some(f.source_unit.id));
    let candidate = envs::runtime(&mut conn, next).await.unwrap().unwrap();
    assert_eq!(candidate.state, "retired");
    assert!(candidate.remove_at.is_some());
    f.world.engine().stop();
}

/// A read-only mount of the world's maker, enabled where the environment lives, with a volume location on each
/// host named, and the default given.
async fn mount_at(f: &Fixture, name: &str, default_fate: &str, hosts: &[&ScriptedHost]) -> Id {
    use orch_db::queries::resources::mounts::{self as rows, LocationRow, MountRow};
    let id = new_id();
    let mut conn = f.world.pool.acquire().await.unwrap();
    rows::insert(
        &mut conn,
        &MountRow {
            id,
            name: name.into(),
            description: String::new(),
            org_id: Some(f.world.root),
            home_id: Some(f.world.root),
            owner_id: Some(f.world.ada),
            target: format!("/data/{name}"),
            read_only: true,
            enabled: true,
            default_fate: default_fate.into(),
            revision: 1,
        },
        f.world.clock.now(),
    )
    .await
    .unwrap();
    orch_authz::ownership::start_resource(
        &mut conn,
        orch_access_contract::decisions::ObjectRef {
            kind: orch_access_contract::permissions::ResourceKind::Mount.into(),
            id,
        },
        f.world.ada,
        orch_authz::ownership::NewAccess::OnlyMe,
        Time::new(f.world.clock.now()),
    )
    .await
    .unwrap();
    for host in hosts {
        rows::set_location(
            &mut conn,
            &LocationRow {
                mount_id: id,
                machine_id: host.machine,
                host_id: Some(host.id),
                host_name: None,
                kind: "volume".into(),
                source: format!("orch-mount-{id}"),
                storage: None,
                data_kind: None,
                created_by: f.world.ada,
                created_at: f.world.clock.now(),
            },
        )
        .await
        .unwrap();
    }
    for host in [&f.source, &f.target] {
        sqlx::query("UPDATE hosts SET drivers=jsonb_set(drivers,'{docker,mounts}','\"placed\"'::jsonb) WHERE id=$1")
            .bind(host.id)
            .execute(&mut *conn)
            .await
            .unwrap();
    }
    id
}

async fn plan_with(
    f: &Fixture,
    mounts: Option<Vec<orch_api_contract::forks::MountChoice>>,
) -> orch_engine::jobs::environment_move::MoveDraft {
    let mut tx = f.world.pool.begin().await.unwrap();
    let principal = Principal::load(
        &mut tx,
        Caller::session(f.world.ada),
        Time::new(f.world.clock.now()),
    )
    .await
    .unwrap();
    let draft = hosting::ready_move(
        &f.world,
        &mut tx,
        &principal,
        f.environment,
        &EnvironmentMovePreflightBody {
            machine_id: None,
            template_id: None,
            copy_directories: None,
            host_id: Some(f.target.id),
            home: None,
            private_views: None,
            files: None,
            check_fingerprints: None,
            fingerprint_request_id: None,
            mounts,
        },
        None,
    )
    .await
    .unwrap();
    tx.rollback().await.unwrap();
    draft
}

#[tokio::test]
async fn a_move_offers_each_mount_detach_as_is_or_copy_and_asks_before_a_copy_lands_on_data() {
    use orch_api_contract::forks::{
        CopyInto, MountAction, MountChoice, MountDetachedBecause, PlanMount,
    };
    use orch_api_contract::mounts::MountFate;
    let f = fixture().await;
    let notes = mount_at(&f, "notes", "as_is", &[&f.source]).await;
    let data = mount_at(&f, "data", "copy", &[&f.source, &f.target]).await;
    let enabled: serde_json::Map<String, serde_json::Value> = [notes, data]
        .iter()
        .map(|id| (id.to_string(), json!(true)))
        .collect();
    f.world
        .set(f.world.topic, "env.mounts", json!(enabled))
        .await;
    let row = |draft: &orch_engine::jobs::environment_move::MoveDraft, id: Id| -> PlanMount {
        draft
            .mounts
            .iter()
            .find(|mount| mount.mount.id == id)
            .unwrap()
            .clone()
    };
    let planned = plan_with(&f, None).await;
    let as_is = row(&planned, notes);
    assert_eq!(as_is.fate, MountFate::AsIs);
    assert_eq!(
        as_is.fates,
        vec![MountFate::Detach, MountFate::AsIs, MountFate::Copy]
    );
    assert_eq!(
        as_is.copy_unavailable, None,
        "Copy is available where it is"
    );
    assert_eq!(as_is.action, MountAction::Detach);
    assert_eq!(
        as_is.detached_because,
        Some(MountDetachedBecause::NoLocation),
        "as it is, without a location there, it stays detached"
    );
    let copied = row(&planned, data);
    assert_eq!(
        copied.fate,
        MountFate::Copy,
        "its manager's default is preselected"
    );
    let copy = copied.copy.clone().unwrap();
    assert!(copy.measured);
    assert_eq!((copy.file_count, copy.total_bytes), (3, 8192));
    let holds = copy.holds.clone().unwrap();
    assert_eq!((holds.file_count, holds.total_bytes), (3, 8192));
    assert!(
        copy.replace,
        "nothing on the target has the location attached"
    );
    assert!(
        planned
            .blockers
            .iter()
            .any(|blocker| blocker.code == "mount_copy_into_required"),
        "a copy into data asks first: {:?}",
        planned.blockers
    );
    assert_eq!(
        f.source
            .calls()
            .iter()
            .filter(|call| call.kind == CommandKind::NodeMountMeasure)
            .count(),
        1,
        "the source location is measured where it is"
    );
    let replace = plan_with(
        &f,
        Some(vec![MountChoice {
            mount_id: data,
            fate: MountFate::Copy,
            into: Some(CopyInto::Replace),
        }]),
    )
    .await;
    let replaced = row(&replace, data).copy.unwrap();
    assert!(
        !replaced.location.made,
        "a replace lands in the location there"
    );
    assert_eq!(
        replaced.location.source.as_deref(),
        Some(format!("orch-mount-{data}").as_str())
    );
    assert!(
        !replace
            .blockers
            .iter()
            .any(|blocker| blocker.code == "mount_copy_into_required")
    );
    let both = plan_with(
        &f,
        Some(vec![
            MountChoice {
                mount_id: data,
                fate: MountFate::Copy,
                into: Some(CopyInto::NewLocation),
            },
            MountChoice {
                mount_id: notes,
                fate: MountFate::Detach,
                into: None,
            },
        ]),
    )
    .await;
    let made = row(&both, data).copy.unwrap();
    assert!(made.location.made);
    let name = made.location.source.unwrap();
    let machine = f.target.machine.simple().to_string();
    assert_eq!(
        name,
        format!("orch-mount-{data}-{}", &machine[machine.len() - 12..]),
        "a new location has a name of the machine's own"
    );
    let off = row(&both, notes);
    assert_eq!(off.detached_because, Some(MountDetachedBecause::Asked));
    assert_eq!(
        both.recipe.mount_decisions.detached,
        vec![notes],
        "the job is told what the plan detached"
    );
    f.world.engine().stop();
}

async fn queue_with(
    f: &Fixture,
    mounts: Vec<orch_api_contract::forks::MountChoice>,
) -> orch_db::queries::jobs::JobRow {
    let mut tx = f.world.pool.begin().await.unwrap();
    let principal = Principal::load(
        &mut tx,
        Caller::session(f.world.ada),
        Time::new(f.world.clock.now()),
    )
    .await
    .unwrap();
    let draft = hosting::ready_move(
        &f.world,
        &mut tx,
        &principal,
        f.environment,
        &EnvironmentMovePreflightBody {
            machine_id: None,
            template_id: None,
            copy_directories: None,
            host_id: Some(f.target.id),
            home: None,
            private_views: None,
            files: None,
            check_fingerprints: None,
            fingerprint_request_id: None,
            mounts: Some(mounts),
        },
        None,
    )
    .await
    .unwrap();
    assert!(draft.blockers.is_empty(), "{:?}", draft.blockers);
    let job = orch_engine::jobs::environment_move::queue(&mut tx, draft, None, f.world.clock.now())
        .await
        .unwrap();
    tx.commit().await.unwrap();
    f.world.engine().kick();
    job
}

/// Answers the archive and unpack commands of a mount's copy with the fixture's stored blob.
fn copies_answered(f: &Fixture) {
    let part = orch_link_contract::commands::BlobRef {
        hash: Hash::of(&f.data),
        size: f.data.len() as i64,
    };
    for host in [&f.source, &f.target] {
        host.answer(CommandKind::NodeMountExport, move |_| {
            Ok(CommandResult::NodeMountExport(
                orch_link_contract::commands::VolumeArchive {
                    parts: vec![part],
                    size: part.size,
                },
            ))
        });
        host.answer(CommandKind::NodeMountImport, |_| {
            Ok(CommandResult::NodeMountImport(
                orch_link_contract::commands::Empty {},
            ))
        });
    }
}

fn tail(id: Id) -> String {
    let text = id.simple().to_string();
    text[text.len() - 12..].to_owned()
}

#[tokio::test]
async fn a_move_copies_a_mounts_data_into_a_new_location_before_its_unit_is_made_and_makes_it_the_hosts()
 {
    let f = fixture().await;
    let data = mount_at(&f, "data", "copy", &[&f.source]).await;
    let enabled: serde_json::Map<String, serde_json::Value> =
        std::iter::once((data.to_string(), json!(true))).collect();
    f.world
        .set(f.world.topic, "env.mounts", json!(enabled))
        .await;
    copies_answered(&f);
    let job = queue_with(&f, Vec::new()).await;
    f.target.wait_for(CommandKind::NodeRuntimeStart, 1).await;
    let unit = hosting::enroll(&f.world, f.target.started().unwrap()).await;
    unit.added
        .machine
        .handler
        .on(CommandKind::FsMaterialize, |_| {
            Ok(CommandResult::FsMaterialize(FsMaterializeResult {
                files: 1,
                bytes: 25,
                skipped: 0,
            }))
        });
    let (state, _, error) = wait_over(&f.world, job.id).await;
    assert_eq!(state, "succeeded", "{error:?}");
    let name = format!("orch-mount-{data}-{}", tail(f.target.machine));
    assert!(
        f.source
            .calls()
            .iter()
            .any(|call| call.kind == CommandKind::NodeMountExport
                && call.body["data"]["name"] == format!("orch-mount-{data}")),
        "the data is read where it lies, on the old host"
    );
    let calls = f.target.calls();
    let made = calls
        .iter()
        .position(|call| call.kind == CommandKind::NodeVolumeEnsure && call.body["name"] == name)
        .expect("the new location's volume is made");
    let landed = calls
        .iter()
        .position(|call| call.kind == CommandKind::NodeMountImport && call.body["name"] == name)
        .expect("the copy lands in it");
    let create = calls
        .iter()
        .position(|call| call.kind == CommandKind::NodeRuntimeCreate)
        .unwrap();
    assert!(
        made < landed && landed < create,
        "the copy lands before the unit is made"
    );
    assert!(
        calls[create].body["mounts"]
            .as_array()
            .unwrap()
            .iter()
            .any(|mount| mount["name"] == "data" && mount["source"] == name),
        "the new unit attaches the copy: {}",
        calls[create].body["mounts"]
    );
    let source: String = sqlx::query_scalar(
        "SELECT source FROM mount_locations WHERE mount_id=$1 AND machine_id=$2",
    )
    .bind(data)
    .bind(f.target.machine)
    .fetch_one(f.world.pool.pool())
    .await
    .unwrap();
    assert_eq!(
        source, name,
        "the copy is the host's location from the commit on"
    );
    common::eventually("the copy's archive parts are released", || async {
        sqlx::query_scalar::<_, i32>("SELECT pin_count FROM blobs WHERE hash=$1")
            .bind(Hash::of(&f.data).as_bytes().as_slice())
            .fetch_one(f.world.pool.pool())
            .await
            .unwrap()
            == 0
    })
    .await;
    f.world.engine().stop();
}

#[tokio::test]
async fn a_move_that_fails_after_replacing_a_locations_data_puts_back_what_it_held_and_removes_what_it_made()
 {
    use orch_api_contract::forks::{CopyInto, MountChoice};
    use orch_api_contract::mounts::MountFate;
    let f = fixture().await;
    let data = mount_at(&f, "data", "as_is", &[&f.source, &f.target]).await;
    let enabled: serde_json::Map<String, serde_json::Value> =
        std::iter::once((data.to_string(), json!(true))).collect();
    f.world
        .set(f.world.topic, "env.mounts", json!(enabled))
        .await;
    copies_answered(&f);
    let job = queue_with(
        &f,
        vec![MountChoice {
            mount_id: data,
            fate: MountFate::Copy,
            into: Some(CopyInto::Replace),
        }],
    )
    .await;
    f.target.wait_for(CommandKind::NodeRuntimeStart, 1).await;
    let unit = hosting::enroll(&f.world, f.target.started().unwrap()).await;
    unit.added
        .machine
        .handler
        .on(CommandKind::FsMaterialize, |_| {
            Err(CommandError::new(
                ErrorCode::Conflict,
                "the destination cannot write the files",
            ))
        });
    let (state, _, error) = wait_over(&f.world, job.id).await;
    assert_eq!(state, "failed");
    assert_eq!(error.unwrap()["undone"], true);
    let location = format!("orch-mount-{data}");
    let imports: Vec<String> = f
        .target
        .calls()
        .iter()
        .filter(|call| call.kind == CommandKind::NodeMountImport)
        .map(|call| call.body["name"].as_str().unwrap().to_owned())
        .collect();
    let aside = imports
        .iter()
        .find(|name| name.contains("-kept-"))
        .expect("what the location held is copied aside first")
        .clone();
    assert_eq!(
        imports.iter().filter(|name| **name == location).count(),
        2,
        "the copy replaced the location's data, and the undo put back what it held: {imports:?}"
    );
    assert!(
        f.target
            .calls()
            .iter()
            .any(|call| call.kind == CommandKind::NodeVolumeRemove && call.body["name"] == aside),
        "the aside volume the job made is removed"
    );
    let kept: i64 = sqlx::query_scalar("SELECT count(*) FROM mount_kept_data WHERE mount_id=$1")
        .bind(data)
        .fetch_one(f.world.pool.pool())
        .await
        .unwrap();
    assert_eq!(
        kept, 0,
        "nothing is listed as kept by a move that did not happen"
    );
    let source: String = sqlx::query_scalar(
        "SELECT source FROM mount_locations WHERE mount_id=$1 AND machine_id=$2",
    )
    .bind(data)
    .bind(f.target.machine)
    .fetch_one(f.world.pool.pool())
    .await
    .unwrap();
    assert_eq!(source, location, "the location stays what it was");
    f.world.engine().stop();
}

async fn local_turn(
    world: &World,
    unit: &Unit,
    thread: Id,
    index: usize,
) -> orch_link_contract::commands::TurnStart {
    let mut found = None;
    common::eventually("a local turn is admitted", || {
        found = unit
            .added
            .seen
            .all()
            .into_iter()
            .filter_map(|command| match command {
                Command::TurnStart(body) if body.thread_id == thread => Some(body),
                Command::TurnStartFenced(body) if body.thread_id == thread => Some(*body.turn),
                _ => None,
            })
            .nth(index);
        let ready = found.is_some();
        async move { ready }
    })
    .await;
    let turn = found.unwrap();
    let segment = world.segment_of(thread).await;
    world.open_session_for_reports(thread, segment).await;
    let stream = orch_link_contract::streams::Stream::Session {
        thread_id: thread,
        segment_id: segment,
    };
    for event in [
        common::turn_began(turn.turn_id),
        common::message_began(turn.turn_id, "local-reply"),
        common::message_done("local-reply", "The conversation continues"),
        common::turn_ended(
            turn.turn_id,
            orch_link_contract::events::TurnOutcome::Completed,
        ),
    ] {
        unit.added.machine.publish(&stream, &event).await;
    }
    world.wait_turn(turn.turn_id, "completed").await;
    turn
}

#[tokio::test]
async fn a_key_separated_agent_moves_with_its_home_and_requests_native_resume_or_carries_the_record_if_the_home_cannot_travel()
 {
    use orch_api_contract::forks::MoveThreadAction;
    for carry in [true, false] {
        let f = fixture_with(World::on_a_key("a-test-key").await, true, false).await;
        let created = f
            .world
            .engine()
            .create_thread(
                &Caller::session(f.world.ada),
                orch_engine::binding::NewThread {
                    topic: f.world.topic,
                    name: "Local conversation".into(),
                    agent: f.world.agent,
                    provider: f.world.provider,
                    environment: f.environment,
                    directory: "web".into(),
                    arrangement: Some(
                        orch_agent_contract::descriptor::ArrangementType::KeySeparated,
                    ),
                    model: Some(common::MODEL.into()),
                    effort: None,
                },
            )
            .await
            .unwrap();
        let thread = created.thread;
        f.world
            .engine()
            .submit_input(
                &Caller::session(f.world.ada),
                thread,
                common::queued("Keep this local history"),
            )
            .await
            .unwrap();
        local_turn(&f.world, &f.source_unit, thread, 0).await;
        let old_segment = f.world.segment_of(thread).await;
        let native = orch_db::queries::threads::segment(
            &mut f.world.pool.acquire().await.unwrap(),
            old_segment,
        )
        .await
        .unwrap()
        .unwrap()
        .native_session_ref
        .unwrap();
        if !carry {
            f.source
                .answer(CommandKind::NodeRuntimeVolumesMeasure, |command| {
                    let Command::NodeRuntimeVolumesMeasure(request) = command else {
                        unreachable!()
                    };
                    if request.volumes.iter().any(|volume| {
                        volume.selector == orch_driver_contract::volumes::OwnedVolumeSelector::Home
                    }) {
                        return Err(CommandError::new(
                            ErrorCode::Unsupported,
                            "This home cannot be carried",
                        ));
                    }
                    Ok(CommandResult::NodeRuntimeVolumesMeasure(
                        orch_link_contract::commands::NodeRuntimeVolumesMeasureResult {
                            volumes: request
                                .volumes
                                .iter()
                                .map(|volume| orch_link_contract::commands::MeasuredVolume {
                                    selector: volume.selector,
                                    measure: orch_driver_contract::volumes::VolumeMeasure {
                                        file_count: 3,
                                        total_bytes: 8192,
                                        file_states: Some(
                                            orch_driver_contract::volumes::FileStatesMeasure {
                                                roots: 0,
                                                objects: 0,
                                                total_bytes: 8192,
                                                journals: 0,
                                                complete: true,
                                            },
                                        ),
                                    },
                                })
                                .collect(),
                        },
                    ))
                });
        }
        let mut conn = f.world.pool.acquire().await.unwrap();
        let principal = Principal::load(
            &mut conn,
            Caller::session(f.world.ada),
            Time::new(f.world.clock.now()),
        )
        .await
        .unwrap();
        let draft = f
            .world
            .engine()
            .draft_move(
                &mut conn,
                &principal,
                f.environment,
                &EnvironmentMovePreflightBody {
                    host_id: Some(f.target.id),
                    machine_id: None,
                    template_id: None,
                    copy_directories: None,
                    home: None,
                    private_views: None,
                    files: None,
                    check_fingerprints: None,
                    fingerprint_request_id: None,
                    mounts: None,
                },
                None,
            )
            .await
            .unwrap();
        let unsupported = (!carry)
            .then(|| {
                draft
                    .recipe
                    .files
                    .home
                    .as_ref()
                    .and_then(|home| home.measurement.as_ref())
                    .map(|reference| reference.job_id)
            })
            .flatten();
        let mut counts = std::collections::BTreeSet::new();
        if let Some(reference) = draft
            .recipe
            .files
            .home
            .as_ref()
            .and_then(|home| home.measurement.as_ref())
        {
            counts.insert(reference.job_id);
        }
        if let Some(reference) = draft
            .recipe
            .files
            .file_states
            .as_ref()
            .and_then(|states| states.measurement.as_ref())
        {
            counts.insert(reference.job_id);
        }
        for id in counts {
            let (state, _, error) = wait_over(&f.world, id).await;
            if Some(id) == unsupported {
                assert_eq!(state, "failed");
                assert_eq!(
                    error.as_ref().and_then(|error| error["code"].as_str()),
                    Some("unsupported")
                );
            } else {
                assert_eq!(state, "succeeded", "{error:?}");
            }
        }
        let draft = f
            .world
            .engine()
            .draft_move(
                &mut conn,
                &principal,
                f.environment,
                &EnvironmentMovePreflightBody {
                    host_id: Some(f.target.id),
                    machine_id: None,
                    template_id: None,
                    copy_directories: None,
                    home: None,
                    private_views: None,
                    files: None,
                    check_fingerprints: None,
                    fingerprint_request_id: None,
                    mounts: None,
                },
                None,
            )
            .await
            .unwrap();
        assert!(draft.blockers.is_empty(), "{:?}", draft.blockers);
        assert_eq!(draft.recipe.files.home.is_some(), carry);
        assert!(
            draft
                .threads
                .iter()
                .any(|(id, action)| *id == thread && *action == MoveThreadAction::Reimport)
        );
        drop(conn);
        let mut tx = f.world.pool.begin().await.unwrap();
        let job =
            orch_engine::jobs::environment_move::queue(&mut tx, draft, None, f.world.clock.now())
                .await
                .unwrap();
        tx.commit().await.unwrap();
        f.world.engine().kick();
        f.target.wait_for(CommandKind::NodeRuntimeStart, 1).await;
        let runtime = f.target.started().unwrap();
        let target = hosting::enroll_holding(&f.world, runtime, CommandKind::EnvAttach).await;
        target
            .added
            .machine
            .handler
            .on(CommandKind::FsMaterialize, |_| {
                Ok(CommandResult::FsMaterialize(FsMaterializeResult {
                    files: 1,
                    bytes: 1,
                    skipped: 0,
                }))
            });
        target.let_go();
        let result = wait_over(&f.world, job.id).await;
        assert_eq!(result.0, "succeeded", "{result:?}");
        let mut conn = f.world.pool.acquire().await.unwrap();
        let segment =
            orch_db::queries::threads::segment(&mut conn, f.world.segment_of(thread).await)
                .await
                .unwrap()
                .unwrap();
        if carry {
            assert_eq!(segment.native_session_ref.as_deref(), Some(native.as_str()));
        } else {
            assert_eq!(segment.native_session_ref, None);
            let notices: serde_json::Value =
                sqlx::query_scalar("SELECT pending_notices FROM threads WHERE node_id=$1")
                    .bind(thread)
                    .fetch_one(&mut *conn)
                    .await
                    .unwrap();
            assert!(
                notices.to_string().contains("Keep this local history"),
                "the exact recorded Carry text is ready before the next turn"
            );
        }
        drop(conn);
        f.world
            .engine()
            .submit_input(
                &Caller::session(f.world.ada),
                thread,
                common::queued("Continue here"),
            )
            .await
            .unwrap();
        local_turn(&f.world, &target, thread, 0).await;
        let opened = target
            .added
            .seen
            .all()
            .into_iter()
            .find_map(|command| match command {
                Command::SessionOpen(body) => Some(body),
                Command::SessionOpenFenced(body) => Some(*body.session),
                _ => None,
            })
            .unwrap();
        assert_eq!(
            opened.mode,
            if carry {
                orch_link_contract::commands::SessionMode::Resume
            } else {
                orch_link_contract::commands::SessionMode::New
            }
        );
        if carry {
            assert_eq!(opened.native_session_ref.as_deref(), Some(native.as_str()));
        }
        assert_eq!(f.world.arrangement_of(thread).await, "key_separated");
    }
}

/// A move that brings the home draws, queues and ends the same way for any number of threads; it is the home that
/// travels, so the move tells no machine of any thread.
async fn move_carrying_the_home(f: &Fixture) -> orch_db::queries::jobs::JobRow {
    let mut conn = f.world.pool.acquire().await.unwrap();
    let principal = Principal::load(
        &mut conn,
        Caller::session(f.world.ada),
        Time::new(f.world.clock.now()),
    )
    .await
    .unwrap();
    let body = EnvironmentMovePreflightBody {
        host_id: Some(f.target.id),
        machine_id: None,
        template_id: None,
        copy_directories: None,
        home: None,
        private_views: None,
        files: None,
        check_fingerprints: None,
        fingerprint_request_id: None,
        mounts: None,
    };
    let draft = f
        .world
        .engine()
        .draft_move(&mut conn, &principal, f.environment, &body, None)
        .await
        .unwrap();
    let mut measured = std::collections::BTreeSet::new();
    if let Some(reference) = draft
        .recipe
        .files
        .home
        .as_ref()
        .and_then(|home| home.measurement.as_ref())
    {
        measured.insert(reference.job_id);
    }
    if let Some(reference) = draft
        .recipe
        .files
        .file_states
        .as_ref()
        .and_then(|states| states.measurement.as_ref())
    {
        measured.insert(reference.job_id);
    }
    for id in measured {
        let (state, _, error) = wait_over(&f.world, id).await;
        assert_eq!(state, "succeeded", "{error:?}");
    }
    let draft = f
        .world
        .engine()
        .draft_move(&mut conn, &principal, f.environment, &body, None)
        .await
        .unwrap();
    assert!(draft.blockers.is_empty(), "{:?}", draft.blockers);
    assert!(draft.recipe.files.home.is_some(), "the home travels");
    drop(conn);
    let mut tx = f.world.pool.begin().await.unwrap();
    let job = orch_engine::jobs::environment_move::queue(&mut tx, draft, None, f.world.clock.now())
        .await
        .unwrap();
    tx.commit().await.unwrap();
    f.world.engine().kick();
    job
}

#[tokio::test]
async fn a_move_that_brings_the_home_tells_no_machine_of_any_thread_whatever_their_number() {
    let mut kinds_of = Vec::new();
    for count in [2_usize, 12] {
        let f = fixture_with(World::on_a_key("a-test-key").await, true, false).await;
        let mut threads = Vec::new();
        let mut natives = Vec::new();
        for index in 0..count {
            let created = f
                .world
                .engine()
                .create_thread(
                    &Caller::session(f.world.ada),
                    orch_engine::binding::NewThread {
                        topic: f.world.topic,
                        name: format!("Local conversation {index}"),
                        agent: f.world.agent,
                        provider: f.world.provider,
                        environment: f.environment,
                        directory: "web".into(),
                        arrangement: Some(
                            orch_agent_contract::descriptor::ArrangementType::KeySeparated,
                        ),
                        model: Some(common::MODEL.into()),
                        effort: None,
                    },
                )
                .await
                .unwrap();
            f.world
                .engine()
                .submit_input(
                    &Caller::session(f.world.ada),
                    created.thread,
                    common::queued(&format!("History of thread {index}")),
                )
                .await
                .unwrap();
            local_turn(&f.world, &f.source_unit, created.thread, 0).await;
            let segment = f.world.segment_of(created.thread).await;
            let native = orch_db::queries::threads::segment(
                &mut f.world.pool.acquire().await.unwrap(),
                segment,
            )
            .await
            .unwrap()
            .unwrap()
            .native_session_ref
            .unwrap();
            threads.push(created.thread);
            natives.push(native);
        }
        let before = f.world.ordered().len();
        let job = move_carrying_the_home(&f).await;
        f.target.wait_for(CommandKind::NodeRuntimeStart, 1).await;
        let runtime = f.target.started().unwrap();
        let target = hosting::enroll_holding(&f.world, runtime, CommandKind::EnvAttach).await;
        target
            .added
            .machine
            .handler
            .on(CommandKind::FsMaterialize, |_| {
                Ok(CommandResult::FsMaterialize(FsMaterializeResult {
                    files: 1,
                    bytes: 1,
                    skipped: 0,
                }))
            });
        target.let_go();
        let result = wait_over(&f.world, job.id).await;
        assert_eq!(result.0, "succeeded", "{result:?}");

        // The commands the move gave to any machine speak of the environment and its volumes, never of a thread:
        // no session was opened or closed, no turn begun, no thread named.
        let moved: Vec<Command> = f
            .world
            .ordered()
            .into_iter()
            .skip(before)
            .map(|(_, command)| command)
            .collect();
        assert!(!moved.is_empty(), "the move gave the machines commands");
        for command in &moved {
            let body = serde_json::to_string(command).unwrap();
            for thread in &threads {
                assert!(
                    !body.contains(&thread.to_string()),
                    "{count} threads: {:?} names thread {thread}",
                    command.kind()
                );
            }
            assert!(
                !matches!(
                    command,
                    Command::SessionOpen(_)
                        | Command::SessionOpenFenced(_)
                        | Command::TurnStart(_)
                        | Command::TurnStartFenced(_)
                ),
                "{count} threads: the move gave {:?}",
                command.kind()
            );
        }
        kinds_of.push(
            moved
                .iter()
                .map(|command| command.kind().as_str())
                .collect::<std::collections::BTreeSet<_>>(),
        );

        // Every thread keeps the session it had; none was resumed.
        let mut conn = f.world.pool.acquire().await.unwrap();
        for (thread, native) in threads.iter().zip(&natives) {
            let segment =
                orch_db::queries::threads::segment(&mut conn, f.world.segment_of(*thread).await)
                    .await
                    .unwrap()
                    .unwrap();
            assert_eq!(segment.native_session_ref.as_deref(), Some(native.as_str()));
        }
        drop(conn);
        assert!(
            target
                .added
                .seen
                .of_kind(CommandKind::SessionOpen)
                .is_empty()
                && target
                    .added
                    .seen
                    .of_kind(CommandKind::SessionOpenFenced)
                    .is_empty(),
            "{count} threads: no thread was opened by the move"
        );

        // The next input of one thread opens that thread alone, in its own session.
        let last = *threads.last().unwrap();
        f.world
            .engine()
            .submit_input(
                &Caller::session(f.world.ada),
                last,
                common::queued("Continue here"),
            )
            .await
            .unwrap();
        local_turn(&f.world, &target, last, 0).await;
        let opened: Vec<_> = target
            .added
            .seen
            .all()
            .into_iter()
            .filter_map(|command| match command {
                Command::SessionOpen(body) => Some(body),
                Command::SessionOpenFenced(body) => Some(*body.session),
                _ => None,
            })
            .collect();
        assert_eq!(opened.len(), 1, "{count} threads: one thread was opened");
        assert_eq!(opened[0].thread_id, last);
        assert_eq!(
            opened[0].mode,
            orch_link_contract::commands::SessionMode::Resume
        );
        assert_eq!(
            opened[0].native_session_ref.as_deref(),
            natives.last().map(String::as_str)
        );
        f.world.engine().stop();
    }
    assert_eq!(
        kinds_of[0], kinds_of[1],
        "the move gives the machines the same kinds of command for 2 threads and for 12"
    );
}
