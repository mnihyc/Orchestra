//! A move keeps environment identity and verified data across declared machine lifecycles.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]

mod common;
mod hosting;

use common::{Added, World};
use orch_api_contract::environment::EnvironmentMovePreflightBody;
use orch_authz::{Caller, Principal};
use orch_blob_contract::store::{BlobStore, stream_of};
use orch_blob_local::LocalBlobStore;
use orch_db::queries::{environments::hosting as envs, storage};
use orch_driver_contract::descriptor::{DriverDescriptor, EnvironmentDataStorage, MadeDataKind};
use orch_engine::{
    jobs::machine_move,
    manifest::{self, Entry, EntryKind, Header},
};
use orch_link_contract::{
    commands::{
        Command, CommandKind, CommandResult, FsManifestResult, FsMaterializeResult,
        FsSnapshotResult,
    },
    policy::{LocalPolicy, PolicyDefaults},
    source::{EnvironmentDataPart, FsDataMeasureResult, FsRecordVerifyResult},
};
use orch_types::{Hash, Id, Time};
use serde_json::json;
use std::sync::Arc;

struct Fixture {
    world: World,
    target: Added,
    thread: Id,
    workspace: FsSnapshotResult,
    home: FsSnapshotResult,
    recorded: FsSnapshotResult,
    _directory: tempfile::TempDir,
    _store: Arc<LocalBlobStore>,
}

async fn snapshot(
    world: &World,
    store: &LocalBlobStore,
    path: &str,
    bytes: &[u8],
) -> FsSnapshotResult {
    let hash = Hash::of(bytes);
    store
        .put(
            &hash,
            Some(i64::try_from(bytes.len()).unwrap()),
            stream_of(vec![bytes::Bytes::copy_from_slice(bytes)]),
        )
        .await
        .unwrap();
    let written = manifest::build(
        &Header {
            v: 1,
            root: "/workspace".into(),
            generated_at: Time::new(world.clock.now()),
            generation: 0,
            case_sensitive: true,
        },
        vec![Entry {
            path: path.into(),
            kind: EntryKind::File,
            size: i64::try_from(bytes.len()).unwrap(),
            mtime_ns: 0,
            mode: 0o600,
            hash: Some(hash),
            link: None,
        }],
    )
    .unwrap();
    let mut conn = world.pool.acquire().await.unwrap();
    storage::register_blob(
        &mut conn,
        &hash,
        i64::try_from(bytes.len()).unwrap(),
        world.clock.now(),
    )
    .await
    .unwrap();
    let kept = world
        .engine()
        .put_manifest(&mut conn, written.bytes)
        .await
        .unwrap();
    FsSnapshotResult {
        manifest_blob: kept.hash,
        file_count: 1,
        total_bytes: i64::try_from(bytes.len()).unwrap(),
        uploaded_bytes: 0,
        missing: 0,
    }
}

async fn fixture(borrowed: bool) -> Fixture {
    let world = World::new().await;
    let thread = world.thread("Keep the conversation").await;
    let target = hosting::connector(&world, "Destination directory", &["environment"])
        .await
        .added;
    let policy = LocalPolicy {
        environment_roots: vec!["/allowed".into()],
        ..LocalPolicy::installed(PolicyDefaults::Managed)
    };
    let descriptor = DriverDescriptor::registered_directories(
        "1",
        EnvironmentDataStorage {
            made_by_orch: Some(MadeDataKind::Directory),
            existing_directory: true,
            reason: None,
        },
    );
    let mut registration = world.pool.begin().await.unwrap();
    sqlx::query("UPDATE machines SET owner_id=$2, home_id=$4, org_id=$4, facts=facts || $3 WHERE id=ANY($1)")
        .bind(vec![world.machine_id,target.id]).bind(world.ada).bind(json!({"policy":policy,"environment_descriptor":descriptor,
            "storages":[{"name":"default","default":true,"capacity_gb":100,"free_gb":80,"private_views":false}]}))
        .bind(world.root).execute(&mut *registration).await.unwrap();
    for machine in [world.machine_id, target.id] {
        orch_authz::ownership::start_resource(
            &mut registration,
            orch_access_contract::decisions::ObjectRef {
                kind: orch_access_contract::permissions::ObjectKind::Machine,
                id: machine,
            },
            world.ada,
            orch_authz::ownership::NewAccess::Defaults,
            Time::new(world.clock.now()),
        )
        .await
        .unwrap();
    }
    registration.commit().await.unwrap();
    let sources = if borrowed {
        json!({"workspace":{"kind":"existing_directory","path":"/workspace"},"home":{"kind":"made_by_orch"}})
    } else {
        json!({"workspace":{"kind":"made_by_orch"},"home":{"kind":"made_by_orch"}})
    };
    sqlx::query("UPDATE environments SET placement='connected',template_id=NULL,generation=0,config=jsonb_set(config,'{env.data_sources}',$2),facts=jsonb_set(facts,'{suspend}','false') WHERE id=$1")
        .bind(world.environment).bind(sources).execute(world.pool.pool()).await.unwrap();
    sqlx::query("UPDATE threads SET arrangement='separated',executor='orch-exec' WHERE node_id=$1")
        .bind(thread)
        .execute(world.pool.pool())
        .await
        .unwrap();
    let directory = tempfile::tempdir().unwrap();
    let store = Arc::new(
        LocalBlobStore::open(directory.path().join("blobs"))
            .await
            .unwrap(),
    );
    world.engine().attach_blobs(store.clone());
    let workspace = snapshot(&world, &store, "src/kept.bin", b"workspace\0\xff").await;
    let home = snapshot(&world, &store, "config/kept", b"home bytes").await;
    let recorded = snapshot(&world, &store, "git/objects/ab/kept", b"recorded\0\xff").await;
    world.machine.handler.on(CommandKind::FsManifest, move |_| {
        Ok(CommandResult::FsManifest(FsManifestResult {
            manifest_blob: workspace.manifest_blob,
            file_count: workspace.file_count,
            total_bytes: workspace.total_bytes,
        }))
    });
    world.machine.handler.on(CommandKind::FsSnapshot, move |_| {
        Ok(CommandResult::FsSnapshot(workspace))
    });
    world
        .machine
        .handler
        .on(CommandKind::FsDataMeasure, move |command| {
            let Command::FsDataMeasure(body) = command else {
                unreachable!()
            };
            Ok(CommandResult::FsDataMeasure(FsDataMeasureResult {
                files: 1,
                bytes: if body.part == EnvironmentDataPart::Home {
                    home.total_bytes
                } else {
                    recorded.total_bytes
                },
                recorded: (body.part == EnvironmentDataPart::FileStates).then_some(
                    FsRecordVerifyResult {
                        roots: 1,
                        objects: 1,
                        journals: 1,
                    },
                ),
            }))
        });
    world
        .machine
        .handler
        .on(CommandKind::FsDataExport, move |command| {
            let Command::FsDataExport(body) = command else {
                unreachable!()
            };
            Ok(CommandResult::FsDataExport(
                if body.part == EnvironmentDataPart::Home {
                    home
                } else {
                    recorded
                },
            ))
        });
    target
        .machine
        .handler
        .on(CommandKind::EnvAttach, |command| {
            let Command::EnvAttach(body) = command else {
                unreachable!()
            };
            let mut facts = hosting::facts();
            facts["workspace_root"] = json!(format!(
                "/allowed/target/generation-{}/workspace",
                body.data_generation.unwrap()
            ));
            facts["home"] = json!("/allowed/target/home");
            facts["generation"] = json!(body.data_generation.unwrap());
            Ok(CommandResult::EnvAttach(
                serde_json::from_value(facts).unwrap(),
            ))
        });
    target
        .machine
        .handler
        .on(CommandKind::FsMaterialize, move |command| {
            let Command::FsMaterialize(body) = command else {
                unreachable!()
            };
            assert_eq!(body.manifest_blob, workspace.manifest_blob);
            Ok(CommandResult::FsMaterialize(FsMaterializeResult {
                files: 1,
                bytes: workspace.total_bytes,
                skipped: 0,
            }))
        });
    target
        .machine
        .handler
        .on(CommandKind::GitWorktreeRepair, |command| {
            let Command::GitWorktreeRepair(body) = command else {
                unreachable!()
            };
            assert_eq!(body.previous_root, "/workspace");
            Ok(CommandResult::GitWorktreeRepair(
                orch_link_contract::source::GitWorktreeRepairResult { repaired: 0 },
            ))
        });
    target
        .machine
        .handler
        .on(CommandKind::FsDataImport, move |command| {
            let Command::FsDataImport(body) = command else {
                unreachable!()
            };
            let expected = if body.part == EnvironmentDataPart::Home {
                home
            } else {
                recorded
            };
            assert_eq!(body.manifest_blob, expected.manifest_blob);
            Ok(CommandResult::FsDataImport(FsMaterializeResult {
                files: 1,
                bytes: expected.total_bytes,
                skipped: 0,
            }))
        });
    target.machine.handler.on(CommandKind::FsRecordVerify, |_| {
        Ok(CommandResult::FsRecordVerify(FsRecordVerifyResult {
            roots: 1,
            objects: 1,
            journals: 1,
        }))
    });
    Fixture {
        world,
        target,
        thread,
        workspace,
        home,
        recorded,
        _directory: directory,
        _store: store,
    }
}

async fn principal(world: &World) -> Principal {
    Principal::load(
        &mut world.pool.acquire().await.unwrap(),
        Caller::session(world.ada),
        Time::new(world.clock.now()),
    )
    .await
    .unwrap()
}

fn request(f: &Fixture, copy: bool) -> EnvironmentMovePreflightBody {
    EnvironmentMovePreflightBody {
        machine_id: Some(f.target.id),
        copy_directories: Some(copy),
        home: Some(true),
        ..Default::default()
    }
}

async fn queue(f: &Fixture) -> orch_db::queries::jobs::JobRow {
    let principal = principal(&f.world).await;
    let draft = hosting::ready_machine_move(
        &f.world,
        &principal,
        f.world.environment,
        &request(f, true),
        None,
    )
    .await
    .unwrap();
    assert!(draft.blockers.is_empty(), "{:?}", draft.blockers);
    assert_eq!(draft.recipe.files.path_map.from, "/workspace");
    assert_eq!(
        draft.recipe.files.bytes_to_download,
        f.workspace.total_bytes + f.home.total_bytes + f.recorded.total_bytes
    );
    let mut tx = f.world.pool.begin().await.unwrap();
    let row = machine_move::queue(&mut tx, &principal, draft, None, f.world.clock.now())
        .await
        .unwrap();
    tx.commit().await.unwrap();
    f.world.engine().kick();
    row
}

async fn assert_job_state(world: &World, job: Id, expected: &str) {
    let (state, progress, error) = hosting::wait_over(world, job).await;
    assert_eq!(state, expected, "progress {progress}, error {error:?}");
}

#[tokio::test]
async fn owned_data_moves_to_a_directory_machine_with_identity_owner_conversation_and_recorded_states_kept()
 {
    owned_directory_move(false).await;
}

#[tokio::test]
async fn a_registered_root_alias_moves_with_its_canonical_files_and_recorded_states() {
    owned_directory_move(true).await;
}

async fn owned_directory_move(root_alias: bool) {
    let f = fixture(false).await;
    if root_alias {
        // The machine reports its physical root while registration retains its alias.
        let mut conn = f.world.pool.acquire().await.unwrap();
        let source = envs::environment(&mut conn, f.world.environment)
            .await
            .unwrap()
            .unwrap();
        let mut facts = source.facts.clone();
        facts["workspace_root"] = json!(source.workspace_root);
        facts["generation"] = json!(source.generation);
        sqlx::query("UPDATE environments SET workspace_root=$2, facts=$3 WHERE id=$1")
            .bind(f.world.environment)
            .bind(format!("{}-registered-alias", source.workspace_root))
            .bind(facts)
            .execute(&mut *conn)
            .await
            .unwrap();
    }
    let job = queue(&f).await;
    assert_job_state(&f.world, job.id, "succeeded").await;
    let mut conn = f.world.pool.acquire().await.unwrap();
    let current = envs::environment(&mut conn, f.world.environment)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(current.owner_id, f.world.ada);
    assert_eq!(current.machine_id, Some(f.target.id));
    assert_eq!(current.generation, 1);
    assert!(current.workspace_root.starts_with("/allowed/target/"));
    assert_eq!(
        current.config["env.data_sources"]["workspace"]["kind"],
        "made_by_orch"
    );
    let thread = orch_db::queries::threads::thread(&mut conn, f.thread)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(thread.environment_id, Some(f.world.environment));
    let saved = orch_db::queries::jobs::job(&mut conn, job.id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(saved.plan["file_states_content_verified"], true);
    drop(conn);
    common::eventually(
        "the old connection closes only after verification",
        || async { f.world.began(CommandKind::EnvDetach) > 0 },
    )
    .await;
    let calls = f.target.machine.handler.executions();
    let at = |kind| calls.iter().position(|call| call.kind == kind).unwrap();
    assert!(at(CommandKind::EnvAttach) < at(CommandKind::ExecStart));
    assert!(at(CommandKind::ExecStart) < at(CommandKind::FsMaterialize));
    assert!(at(CommandKind::ExecStart) < at(CommandKind::FsDataImport));
    assert_eq!(
        calls
            .iter()
            .filter(|run| run.kind == CommandKind::FsDataImport)
            .count(),
        2
    );
    f.world.engine().stop();
}

#[tokio::test]
async fn an_environment_manager_can_move_to_an_offered_machine_without_managing_its_policy() {
    let f = fixture(false).await;
    let owner = f.world.person("machine-owner").await;
    let mut conn = f.world.pool.begin().await.unwrap();
    let current_owner = Principal::load(
        &mut conn,
        Caller::session(f.world.ada),
        Time::new(f.world.clock.now()),
    )
    .await
    .unwrap();
    orch_authz::ownership::transfer(
        &mut conn,
        &current_owner,
        orch_access_contract::decisions::ObjectRef {
            kind: orch_access_contract::permissions::ObjectKind::Machine,
            id: f.target.id,
        },
        owner,
        false,
        &orch_authz::usable::ResourcePolicy::default(),
        &orch_db::audit::Origin::default(),
    )
    .await
    .unwrap();
    orch_db::queries::access::testing::add_entry(
        &mut conn,
        &orch_db::queries::access::PageKey::new("machine", f.target.id, "live"),
        "any",
        "machine.use",
        ("user", Some(f.world.ada), None),
        None,
        f.world.clock.now(),
    )
    .await
    .unwrap();
    let principal = Principal::load(
        &mut conn,
        Caller::session(f.world.ada),
        Time::new(f.world.clock.now()),
    )
    .await
    .unwrap();
    assert!(
        !orch_authz::resources::check_machine(
            &mut conn,
            &principal,
            f.target.id,
            orch_access_contract::permissions::Permission::MachineManage,
            orch_authz::Policy::default()
        )
        .await
        .unwrap()
        .is_allowed()
    );
    conn.commit().await.unwrap();
    let job = queue(&f).await;
    assert_job_state(&f.world, job.id, "succeeded").await;
    f.world.engine().stop();
}

#[tokio::test]
async fn an_existing_directory_is_refused_until_copy_is_chosen_without_moving_or_replacing_the_original()
 {
    let f = fixture(true).await;
    let principal = principal(&f.world).await;
    let draft = hosting::ready_machine_move(
        &f.world,
        &principal,
        f.world.environment,
        &request(&f, false),
        None,
    )
    .await
    .unwrap();
    assert!(
        draft
            .blockers
            .iter()
            .any(|blocker| blocker.code == "existing_directory_on_source_machine")
    );
    let mut conn = f.world.pool.acquire().await.unwrap();
    assert!(
        machine_move::queue(&mut conn, &principal, draft, None, f.world.clock.now())
            .await
            .is_err()
    );
    assert_eq!(
        f.target
            .machine
            .handler
            .executions()
            .iter()
            .filter(|run| run.kind == CommandKind::EnvAttach)
            .count(),
        0
    );
    drop(conn);
    let job = queue(&f).await;
    assert_job_state(&f.world, job.id, "succeeded").await;
    assert_eq!(f.world.began(CommandKind::FsRemove), 0);
    f.world.engine().stop();
}

#[tokio::test]
async fn a_legacy_directory_does_not_silently_select_the_persons_undeclared_home() {
    let f = fixture(false).await;
    sqlx::query("UPDATE environments SET config=config-'env.data_sources' WHERE id=$1")
        .bind(f.world.environment)
        .execute(f.world.pool.pool())
        .await
        .unwrap();
    let principal = principal(&f.world).await;
    let request = EnvironmentMovePreflightBody {
        machine_id: Some(f.target.id),
        copy_directories: Some(true),
        ..Default::default()
    };
    let draft =
        hosting::ready_machine_move(&f.world, &principal, f.world.environment, &request, None)
            .await
            .unwrap();
    assert!(
        draft.recipe.files.home.is_none(),
        "an undeclared native home is never implicitly selected"
    );
    assert_eq!(
        f.world.began(CommandKind::FsDataMeasure),
        1,
        "only the mandatory recorded-state store is measured"
    );
    let explicit = EnvironmentMovePreflightBody {
        home: Some(true),
        ..request
    };
    let draft =
        hosting::ready_machine_move(&f.world, &principal, f.world.environment, &explicit, None)
            .await
            .unwrap();
    assert_eq!(
        draft.recipe.files.home.unwrap().total_bytes,
        f.home.total_bytes
    );
    f.world.engine().stop();
}

#[tokio::test]
async fn the_destination_free_space_observation_can_change_while_its_data_is_copied() {
    let f = fixture(false).await;
    f.target.machine.handler.hold(CommandKind::FsDataImport);
    let job = queue(&f).await;
    common::eventually("the candidate begins data import", || async {
        f.target
            .machine
            .handler
            .executions()
            .iter()
            .any(|run| run.kind == CommandKind::FsDataImport)
    })
    .await;
    sqlx::query(
        "UPDATE machines SET facts=jsonb_set(facts,'{storages,0,free_gb}','79') WHERE id=$1",
    )
    .bind(f.target.id)
    .execute(f.world.pool.pool())
    .await
    .unwrap();
    f.target.machine.handler.release();
    let (state, _, error) = hosting::wait_over(&f.world, job.id).await;
    assert_eq!(
        state, "succeeded",
        "a changing observation is not a changed policy: {error:?}"
    );
    f.world.engine().stop();
}

#[tokio::test]
async fn a_destination_policy_changed_during_copy_rolls_back_without_publishing_or_detaching_the_source()
 {
    let f = fixture(false).await;
    f.target.machine.handler.hold(CommandKind::FsDataImport);
    let job = queue(&f).await;
    common::eventually("the candidate waits during data import", || async {
        f.target
            .machine
            .handler
            .executions()
            .iter()
            .any(|run| run.kind == CommandKind::FsDataImport)
    })
    .await;
    sqlx::query("UPDATE machines SET facts=jsonb_set(facts,'{policy,environment.remote_write}','false') WHERE id=$1")
        .bind(f.target.id).execute(f.world.pool.pool()).await.unwrap();
    f.target.machine.handler.release();
    assert_job_state(&f.world, job.id, "failed").await;
    let mut conn = f.world.pool.acquire().await.unwrap();
    let current = envs::environment(&mut conn, f.world.environment)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(current.machine_id, Some(f.world.machine_id));
    assert_eq!(current.generation, 0);
    assert_eq!(current.owner_id, f.world.ada);
    assert_eq!(f.world.began(CommandKind::EnvDetach), 0);
    drop(conn);
    common::eventually("terminal move pins are released once", || async {
        let mut conn = f.world.pool.acquire().await.unwrap();
        orch_db::queries::jobs::job(&mut conn, job.id)
            .await
            .unwrap()
            .unwrap()
            .plan["move_pins_released"]
            == true
    })
    .await;
    // Purging a terminal job's recipe must not let another move reuse its failed directory.
    sqlx::query("UPDATE jobs SET plan='{}' WHERE id=$1")
        .bind(job.id)
        .execute(f.world.pool.pool())
        .await
        .unwrap();
    sqlx::query("UPDATE machines SET facts=jsonb_set(facts,'{policy,environment.remote_write}','true') WHERE id=$1")
        .bind(f.target.id).execute(f.world.pool.pool()).await.unwrap();
    let next = hosting::ready_machine_move(
        &f.world,
        &principal(&f.world).await,
        f.world.environment,
        &request(&f, true),
        None,
    )
    .await
    .unwrap();
    assert!(
        next.recipe.next_generation > 1,
        "a purged receipt never reuses a candidate namespace"
    );
    f.world.engine().stop();
}

#[tokio::test]
async fn the_same_owned_directory_data_moves_into_a_host_unit_through_the_common_move_job() {
    Box::pin(directory_to_unit_move(false, false)).await;
}

#[tokio::test]
async fn a_portable_unit_move_keeps_its_destination_host_use_reservation() {
    Box::pin(directory_to_unit_move(true, false)).await;
}

#[tokio::test]
async fn a_registration_claiming_the_last_slot_during_portable_move_copy_keeps_the_source_intact() {
    Box::pin(directory_to_unit_move(true, true)).await;
}

async fn directory_to_unit_move(limited: bool, fill_slot: bool) {
    let f = fixture(false).await;
    hosting::serve(&f.world);
    let host = hosting::ScriptedHost::on(&f.world, "Unit destination", 16_384).await;
    let entry = if limited {
        Some(
            hosting::limited_physical_use(
                &f.world,
                orch_access_contract::permissions::ResourceKind::Host,
                host.id,
                1,
            )
            .await,
        )
    } else {
        None
    };
    let template = hosting::template(&f.world).await;
    let principal = principal(&f.world).await;
    let request = EnvironmentMovePreflightBody {
        host_id: Some(host.id),
        template_id: Some(template),
        home: Some(true),
        ..Default::default()
    };
    let draft =
        hosting::ready_machine_move(&f.world, &principal, f.world.environment, &request, None)
            .await
            .unwrap();
    assert!(draft.blockers.is_empty(), "{:?}", draft.blockers);
    let mut tx = f.world.pool.begin().await.unwrap();
    let job = machine_move::queue(&mut tx, &principal, draft, None, f.world.clock.now())
        .await
        .unwrap();
    tx.commit().await.unwrap();
    f.world.engine().kick();
    host.wait_for(CommandKind::NodeRuntimeStart, 1).await;
    let unit = hosting::enroll_holding(
        &f.world,
        host.started().unwrap(),
        CommandKind::FsMaterialize,
    )
    .await;
    let workspace = f.workspace;
    let home = f.home;
    let recorded = f.recorded;
    unit.added
        .machine
        .handler
        .on(CommandKind::FsMaterialize, move |command| {
            let Command::FsMaterialize(body) = command else {
                unreachable!()
            };
            assert_eq!(body.manifest_blob, workspace.manifest_blob);
            Ok(CommandResult::FsMaterialize(FsMaterializeResult {
                files: 1,
                bytes: workspace.total_bytes,
                skipped: 0,
            }))
        });
    unit.added
        .machine
        .handler
        .on(CommandKind::FsDataImport, move |command| {
            let Command::FsDataImport(body) = command else {
                unreachable!()
            };
            let expected = if body.part == EnvironmentDataPart::Home {
                home
            } else {
                recorded
            };
            assert_eq!(body.manifest_blob, expected.manifest_blob);
            Ok(CommandResult::FsDataImport(FsMaterializeResult {
                files: 1,
                bytes: expected.total_bytes,
                skipped: 0,
            }))
        });
    unit.added
        .machine
        .handler
        .on(CommandKind::FsRecordVerify, |_| {
            Ok(CommandResult::FsRecordVerify(FsRecordVerifyResult {
                roots: 1,
                objects: 1,
                journals: 1,
            }))
        });
    let competitor = if fill_slot {
        common::eventually("the portable move is held during copy", || async {
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
            template,
            "Slot competitor",
            Some(host.id),
            &json!({}),
        )
        .await
        .unwrap();
        let competitor_runtime =
            hosting::runtime_started_for_job(&f.world, &host, made.job.id).await;
        let _unit = hosting::enroll(&f.world, competitor_runtime).await;
        let (state, _, error) = hosting::wait_over(&f.world, made.job.id).await;
        assert_eq!(
            state, "succeeded",
            "the admitted reservation provisions: {error:?}"
        );
        Some((made.id, _unit))
    } else {
        None
    };
    unit.let_go();
    let (state, _, error) = hosting::wait_over(&f.world, job.id).await;
    if let Some((competitor, _competitor_unit)) = competitor {
        assert_eq!(
            state, "failed",
            "the already committed registration owns the last slot: {error:?}"
        );
        let current = envs::environment(
            &mut f.world.pool.acquire().await.unwrap(),
            f.world.environment,
        )
        .await
        .unwrap()
        .unwrap();
        assert_eq!(current.machine_id, Some(f.world.machine_id));
        assert_eq!(current.generation, 0);
        let registered: Vec<Id> = sqlx::query_scalar(
            "SELECT id FROM environments WHERE host_entry_id=$1 AND deleted_at IS NULL",
        )
        .bind(entry.unwrap())
        .fetch_all(f.world.pool.pool())
        .await
        .unwrap();
        assert_eq!(registered, [competitor]);
        assert_eq!(f.world.began(CommandKind::EnvDetach), 0);
        f.world.engine().stop();
        return;
    }
    assert_eq!(state, "succeeded", "{error:?}");
    let mut conn = f.world.pool.acquire().await.unwrap();
    let environment = envs::environment(&mut conn, f.world.environment)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(environment.machine_id, Some(unit.id));
    assert_eq!(environment.owner_id, f.world.ada);
    assert_eq!(environment.template_id, Some(template));
    assert_eq!(environment.generation, 1);
    assert_eq!(environment.id, f.world.environment);
    if let Some(entry) = entry {
        let recorded: Option<Id> =
            sqlx::query_scalar("SELECT host_entry_id FROM environments WHERE id=$1")
                .bind(environment.id)
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
    f.world.engine().stop();
}

#[tokio::test]
async fn a_directory_move_obeys_the_machine_use_environment_cap_before_native_copy() {
    let f = fixture(false).await;
    hosting::limited_physical_use(
        &f.world,
        orch_access_contract::permissions::ResourceKind::Machine,
        f.target.id,
        0,
    )
    .await;
    let actor = principal(&f.world).await;
    let draft = hosting::ready_machine_move(
        &f.world,
        &actor,
        f.world.environment,
        &request(&f, true),
        None,
    )
    .await
    .unwrap();
    assert!(
        draft
            .blockers
            .iter()
            .any(|blocker| blocker.message.contains("environment capacity")),
        "a zero-slot Use entry must name its capacity refusal: {:?}",
        draft.blockers
    );
    assert_eq!(
        envs::environment(
            &mut f.world.pool.acquire().await.unwrap(),
            f.world.environment
        )
        .await
        .unwrap()
        .unwrap()
        .machine_id,
        Some(f.world.machine_id)
    );
    assert!(
        !f.target
            .machine
            .handler
            .executions()
            .iter()
            .any(|run| matches!(
                run.kind,
                CommandKind::EnvAttach | CommandKind::FsMaterialize
            ))
    );
    f.world.engine().stop();
}

#[tokio::test]
async fn a_directory_move_records_its_exact_machine_use_reservation() {
    let f = fixture(false).await;
    let entry = hosting::limited_physical_use(
        &f.world,
        orch_access_contract::permissions::ResourceKind::Machine,
        f.target.id,
        1,
    )
    .await;
    let job = queue(&f).await;
    assert_job_state(&f.world, job.id, "succeeded").await;
    let recorded: Option<Id> =
        sqlx::query_scalar("SELECT host_entry_id FROM environments WHERE id=$1")
            .bind(f.world.environment)
            .fetch_one(f.world.pool.pool())
            .await
            .unwrap();
    assert_eq!(recorded, Some(entry));
    assert_eq!(
        orch_db::queries::environments::environments_made_through(
            &mut f.world.pool.acquire().await.unwrap(),
            &[entry]
        )
        .await
        .unwrap(),
        1
    );
    f.world.engine().stop();
}

#[tokio::test]
async fn finishing_an_informational_home_count_does_not_change_a_reviewed_move_choice() {
    let f = fixture(false).await;
    let actor = principal(&f.world).await;
    let draft = hosting::ready_machine_move(
        &f.world,
        &actor,
        f.world.environment,
        &request(&f, true),
        None,
    )
    .await
    .unwrap();
    let mut counting = draft.recipe.clone();
    let home = counting.files.home.as_mut().unwrap();
    home.measured = false;
    home.file_count = 0;
    home.total_bytes = 0;
    home.measurement.as_mut().unwrap().measured_at = None;
    assert!(counting.same_choice(&draft.recipe));
    let states = counting.files.file_states.as_mut().unwrap();
    states.measurement.as_mut().unwrap().measured_at = None;
    assert!(
        counting.same_choice(&draft.recipe),
        "only the observation time changed"
    );
    counting.files.file_states.as_mut().unwrap().roots += 1;
    assert!(
        !counting.same_choice(&draft.recipe),
        "integrity counts still decide confirmation"
    );
    let mut different = draft.recipe.clone();
    different.files.home = None;
    assert!(
        !different.same_choice(&draft.recipe),
        "leaving the home behind is another choice"
    );
    different = draft.recipe.clone();
    different.source_revision += 1;
    assert!(
        !different.same_choice(&draft.recipe),
        "the frozen source revision must still match"
    );
    f.world.engine().stop();
}
