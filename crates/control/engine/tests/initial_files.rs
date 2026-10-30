//! Initial workspace delivery precedes running publication and cancellation collects its fresh copy and pins.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]

mod common;
mod hosting;

use common::World;
use orch_api_contract::{binding::NewEnvironmentFiles, forks::FileSelection};
use orch_authz::{Caller, Principal};
use orch_blob_contract::store::{BlobStore, read_all, stream_of};
use orch_blob_local::LocalBlobStore;
use orch_db::queries::{environments::thread_engine as envs, jobs, storage, threads};
use orch_engine::{
    environments::{self, Request},
    initial_files,
    manifest::{self, Entry, EntryKind, Header},
};
use orch_link_contract::commands::{
    Command, CommandKind, CommandResult, FsMaterializeResult, FsSnapshotResult,
};
use orch_types::{Hash, Id, Time};
use serde_json::json;
use std::sync::{Arc, Mutex};

const SOURCE_BYTES: &[u8] = b"the same uploaded and shared bytes";

struct Prepared {
    directory: tempfile::TempDir,
    store: Arc<LocalBlobStore>,
    thread: Id,
    source: Id,
    hash: Hash,
    manifest: Hash,
}
async fn source(world: &World) -> Prepared {
    source_with_root_alias(world, false).await
}

async fn source_with_root_alias(world: &World, root_alias: bool) -> Prepared {
    let directory = tempfile::tempdir().unwrap();
    let store = Arc::new(
        LocalBlobStore::open(directory.path().to_path_buf())
            .await
            .unwrap(),
    );
    world.engine().attach_blobs(store.clone());
    let thread = world.thread("Initial file source").await;
    let mut conn = world.pool.acquire().await.unwrap();
    let row = threads::thread(&mut conn, thread).await.unwrap().unwrap();
    let source = row.environment_id.unwrap();
    let environment = envs::environment(&mut conn, source).await.unwrap().unwrap();
    if root_alias {
        // The native connector reports its resolved root while a directory keeps its registration.
        let mut facts = environment.facts.clone();
        facts["workspace_root"] = json!(environment.workspace_root);
        facts["generation"] = json!(environment.generation);
        sqlx::query("UPDATE environments SET workspace_root=$2, facts=$3 WHERE id=$1")
            .bind(source)
            .bind(format!("{}/registered-root", environment.workspace_root))
            .bind(facts)
            .execute(&mut *conn)
            .await
            .unwrap();
    }
    let bytes = SOURCE_BYTES;
    let hash = Hash::of(bytes);
    store
        .put(
            &hash,
            Some(i64::try_from(bytes.len()).unwrap()),
            stream_of(vec![bytes::Bytes::copy_from_slice(bytes)]),
        )
        .await
        .unwrap();
    storage::register_blob(
        &mut conn,
        &hash,
        i64::try_from(bytes.len()).unwrap(),
        world.clock.now(),
    )
    .await
    .unwrap();
    let entry = |path: &str| Entry {
        path: path.into(),
        kind: EntryKind::File,
        size: i64::try_from(bytes.len()).unwrap(),
        mtime_ns: 0,
        mode: 0o600,
        hash: Some(hash),
        link: None,
    };
    let written = manifest::build(
        &Header {
            v: 1,
            root: orch_engine::sessions::absolute_directory(
                &environment.workspace_root,
                &row.workspace_dir,
            ),
            generated_at: Time::new(world.clock.now()),
            generation: i64::from(environment.generation),
            case_sensitive: true,
        },
        vec![
            entry("uploads/result.txt"),
            entry("shares/reference.md"),
            entry("unselected/private.txt"),
        ],
    )
    .unwrap();
    let kept = world
        .engine()
        .put_manifest(&mut conn, written.bytes)
        .await
        .unwrap();
    drop(conn);
    world
        .machine
        .handler
        .on(CommandKind::FsSnapshot, move |command| {
            let Command::FsSnapshot(body) = command else {
                panic!("snapshot command");
            };
            assert_eq!(body.environment_id, source);
            assert!(
                body.selection
                    .include
                    .iter()
                    .any(|path| path.ends_with("uploads"))
            );
            Ok(CommandResult::FsSnapshot(FsSnapshotResult {
                manifest_blob: kept.hash,
                file_count: 3,
                total_bytes: 3 * i64::try_from(bytes.len()).unwrap(),
                uploaded_bytes: 0,
                missing: 0,
            }))
        });
    Prepared {
        directory,
        store,
        thread,
        source,
        hash,
        manifest: kept.hash,
    }
}

async fn queue(
    world: &World,
    prepared: &Prepared,
    host: &hosting::ScriptedHost,
) -> environments::Made {
    queue_files(
        world,
        host,
        &NewEnvironmentFiles::FromThread {
            from_thread: prepared.thread,
            selection: Some(FileSelection::Selection {
                include: vec!["uploads".into(), "shares".into()],
                exclude: None,
            }),
        },
    )
    .await
}

async fn queue_files(
    world: &World,
    host: &hosting::ScriptedHost,
    files: &NewEnvironmentFiles,
) -> environments::Made {
    let template = hosting::template(world).await;
    let mut tx = world.pool.begin().await.unwrap();
    let principal = Principal::load(
        &mut tx,
        Caller::session(world.ada),
        Time::new(world.clock.now()),
    )
    .await
    .unwrap();
    let files = initial_files::plan(&mut tx, &principal, files)
        .await
        .unwrap();
    let made = environments::create(
        &mut tx,
        world.clock.now(),
        &principal,
        &Request {
            owner: world.ada,
            name: Some("With initial files"),
            home: world.topic,
            template: Some(template),
            host: Some(Some(host.id)),
            config: &json!({}),
        },
    )
    .await
    .unwrap();
    jobs::merge_plan(
        &mut tx,
        made.job.id,
        &json!({"initial_files":files}),
        world.clock.now(),
    )
    .await
    .unwrap();
    tx.commit().await.unwrap();
    world.engine().kick();
    made
}

async fn enrolled(
    world: &World,
    host: &hosting::ScriptedHost,
    made: &environments::Made,
    materialized_files: i64,
) -> (hosting::Unit, Arc<Mutex<Option<Hash>>>) {
    host.wait_for(CommandKind::NodeRuntimeStart, 1).await;
    let unit =
        hosting::enroll_holding(world, host.started().unwrap(), CommandKind::FsMaterialize).await;
    let captured = Arc::new(Mutex::new(None));
    let written = Arc::clone(&captured);
    let environment = made.id;
    // The native acknowledgement is held until the test permits delivery. The actual stored
    // manifest and file bytes are inspected below, so a successful empty acknowledgement is insufficient.
    unit.added
        .machine
        .handler
        .on(CommandKind::FsMaterialize, move |command| {
            let Command::FsMaterialize(body) = command else {
                panic!("materialize command");
            };
            assert_eq!(body.environment_id, environment);
            *written.lock().unwrap() = Some(body.manifest_blob);
            Ok(CommandResult::FsMaterialize(FsMaterializeResult {
                files: materialized_files,
                bytes: materialized_files * i64::try_from(SOURCE_BYTES.len()).unwrap(),
                skipped: 0,
            }))
        });
    (unit, captured)
}

#[tokio::test]
async fn a_selected_initial_copy_is_complete_before_the_environment_becomes_running() {
    selected_initial_copy(false).await;
}

#[tokio::test]
async fn a_selected_initial_copy_keeps_the_reported_root_of_a_registered_alias() {
    selected_initial_copy(true).await;
}

async fn selected_initial_copy(root_alias: bool) {
    let world = World::new().await;
    hosting::serve(&world);
    let host = hosting::ScriptedHost::on(&world, "initial-files", 16384).await;
    let prepared = source_with_root_alias(&world, root_alias).await;
    let made = queue(&world, &prepared, &host).await;
    let (unit, captured) = enrolled(&world, &host, &made, 2).await;
    common::eventually("the initial delivery command is held", || async {
        let began = unit.began(CommandKind::FsMaterialize) > 0;
        if !began {
            let mut conn = world.pool.acquire().await.unwrap();
            let row = jobs::job(&mut conn, made.job.id).await.unwrap().unwrap();
            assert!(
                matches!(row.state.as_str(), "queued" | "running"),
                "initial delivery ended before materialization: state {}, error {:?}",
                row.state,
                row.error
            );
        }
        began
    })
    .await;
    assert_eq!(hosting::status(&world, made.id).await.0, "provisioning");
    assert_eq!(world.commands.issued_of("turn.start"), 0);
    unit.let_go();
    assert_eq!(hosting::wait_over(&world, made.job.id).await.0, "succeeded");
    assert_eq!(hosting::status(&world, made.id).await.0, "running");
    let hash = captured.lock().unwrap().unwrap();
    let manifest = world.engine().read_manifest(&hash).await.unwrap();
    let paths: Vec<_> = manifest
        .entries()
        .iter()
        .filter(|entry| entry.kind == EntryKind::File)
        .map(|entry| entry.path.as_str())
        .collect();
    assert_eq!(paths, ["shares/reference.md", "uploads/result.txt"]);
    for file in manifest
        .entries()
        .iter()
        .filter(|entry| entry.kind == EntryKind::File)
    {
        assert_eq!(file.hash, Some(prepared.hash));
        let actual = read_all(
            prepared
                .store
                .get(&file.hash.unwrap(), None)
                .await
                .unwrap()
                .body,
        )
        .await
        .unwrap();
        assert_eq!(Hash::of(&actual), prepared.hash);
        assert_eq!(
            actual.as_ref(),
            b"the same uploaded and shared bytes".as_slice()
        );
    }
    let mut conn = world.pool.acquire().await.unwrap();
    let row = jobs::job(&mut conn, made.job.id).await.unwrap().unwrap();
    assert!(row.plan["initial_files_pins"].is_null());
    assert_eq!(
        storage::blob(&mut conn, &hash)
            .await
            .unwrap()
            .unwrap()
            .pin_count,
        0
    );
    world.engine().stop();
    let _ = prepared.directory;
}

#[tokio::test]
async fn cancellation_removes_the_fresh_unit_and_volumes_and_releases_its_copy_pins() {
    let world = World::new().await;
    hosting::serve(&world);
    let host = hosting::ScriptedHost::on(&world, "cancel-initial-files", 16384).await;
    let prepared = source(&world).await;
    let made = queue(&world, &prepared, &host).await;
    let (unit, _) = enrolled(&world, &host, &made, 2).await;
    common::eventually("the copy is pinned before its delivery", || async {
        let mut conn = world.pool.acquire().await.unwrap();
        jobs::job(&mut conn, made.job.id)
            .await
            .unwrap()
            .unwrap()
            .plan["initial_files_pins"]
            .is_array()
    })
    .await;
    world
        .engine()
        .cancel_job(made.job.id, world.ada)
        .await
        .unwrap();
    unit.let_go();
    assert_eq!(hosting::wait_over(&world, made.job.id).await.0, "cancelled");
    assert!(host.count(CommandKind::NodeRuntimeRemove) > 0);
    assert!(host.count(CommandKind::NodeVolumeRemove) > 0);
    assert_ne!(hosting::status(&world, made.id).await.0, "running");
    let mut conn = world.pool.acquire().await.unwrap();
    let row = jobs::job(&mut conn, made.job.id).await.unwrap().unwrap();
    assert!(row.plan["initial_files_pins"].is_null());
    assert_eq!(
        storage::blob(&mut conn, &prepared.hash)
            .await
            .unwrap()
            .unwrap()
            .pin_count,
        0
    );
    assert_eq!(
        envs::environment(&mut conn, prepared.source)
            .await
            .unwrap()
            .unwrap()
            .status,
        "running"
    );
    world.engine().stop();
}

#[tokio::test]
async fn a_saved_snapshot_keeps_its_contents_after_the_live_source_identity_changes() {
    copy_saved_snapshot(true, 3).await;
}

#[tokio::test]
async fn a_saved_snapshot_with_the_wrong_manifest_generation_is_not_delivered() {
    copy_saved_snapshot(false, 3).await;
}

#[tokio::test]
async fn a_saved_snapshot_with_incomplete_materialization_is_not_published() {
    copy_saved_snapshot(true, 2).await;
}

async fn copy_saved_snapshot(matching_generation: bool, materialized_files: i64) {
    let world = World::new().await;
    hosting::serve(&world);
    let host = hosting::ScriptedHost::on(&world, "saved-files", 16384).await;
    let prepared = source(&world).await;
    let snapshot = orch_types::new_id();
    let mut tx = world.pool.begin().await.unwrap();
    let original = envs::environment(&mut tx, prepared.source)
        .await
        .unwrap()
        .unwrap();
    let thread = threads::thread(&mut tx, prepared.thread)
        .await
        .unwrap()
        .unwrap();
    let manifest = world
        .engine()
        .read_manifest(&prepared.manifest)
        .await
        .unwrap();
    storage::begin_snapshot(
        &mut tx,
        &storage::NewSnapshot {
            id: snapshot,
            thread_id: Some(prepared.thread),
            environment_id: prepared.source,
            env_generation: if matching_generation {
                original.generation
            } else {
                original.generation + 1
            },
            label: "Saved before the move",
            selection: &json!({"include":[thread.workspace_dir],"exclude":[]}),
            created_by: Some(world.ada),
            at: world.clock.now(),
        },
    )
    .await
    .unwrap();
    sqlx::query("UPDATE snapshots SET directory=$2 WHERE id=$1")
        .bind(snapshot)
        .bind(&thread.workspace_dir)
        .execute(&mut *tx)
        .await
        .unwrap();
    assert!(
        storage::end_snapshot(
            &mut tx,
            snapshot,
            &storage::SnapshotEnd {
                state: "complete",
                file_count: 3,
                total_bytes: manifest.entries().iter().map(|file| file.size).sum(),
                missing_count: 0,
                manifest_blob: Some(prepared.manifest),
                at: world.clock.now(),
            },
        )
        .await
        .unwrap()
    );
    assert_eq!(
        storage::pin_snapshot(&mut tx, snapshot, &[prepared.manifest, prepared.hash])
            .await
            .unwrap(),
        2
    );
    let next_generation = original.generation + 1;
    let next_root = format!("{}-moved", original.workspace_root);
    let mut facts = original.facts.clone();
    facts["generation"] = json!(next_generation);
    facts["workspace_root"] = json!(next_root);
    sqlx::query("UPDATE environments SET generation=$2, workspace_root=$3, facts=$4 WHERE id=$1")
        .bind(prepared.source)
        .bind(next_generation)
        .bind(&next_root)
        .bind(facts)
        .execute(&mut *tx)
        .await
        .unwrap();
    tx.commit().await.unwrap();
    let made = queue_files(
        &world,
        &host,
        &NewEnvironmentFiles::Snapshot {
            snapshot_id: snapshot,
        },
    )
    .await;
    let (unit, captured) = enrolled(&world, &host, &made, materialized_files).await;
    if !matching_generation {
        let (state, _, error) = hosting::wait_over(&world, made.job.id).await;
        assert_eq!(state, "failed");
        let error = error.unwrap();
        assert_eq!(error["code"], "initial_files_unavailable");
        assert_eq!(
            error["message"],
            "the initial manifest belongs to another source generation"
        );
        assert_eq!(unit.began(CommandKind::FsMaterialize), 0);
        let mut conn = world.pool.acquire().await.unwrap();
        assert_eq!(
            storage::blob(&mut conn, &prepared.hash)
                .await
                .unwrap()
                .unwrap()
                .pin_count,
            1
        );
        assert_eq!(
            storage::snapshot(&mut conn, snapshot)
                .await
                .unwrap()
                .unwrap()
                .state,
            "complete"
        );
        world.engine().stop();
        return;
    }
    common::eventually("the saved files reach initial materialization", || async {
        let began = unit.began(CommandKind::FsMaterialize) > 0;
        if !began {
            let mut conn = world.pool.acquire().await.unwrap();
            let row = jobs::job(&mut conn, made.job.id).await.unwrap().unwrap();
            assert!(
                matches!(row.state.as_str(), "queued" | "running"),
                "saved-file delivery ended before materialization: state {}, error {:?}",
                row.state,
                row.error
            );
        }
        began
    })
    .await;
    assert_eq!(hosting::status(&world, made.id).await.0, "provisioning");
    unit.let_go();
    let (state, _, error) = hosting::wait_over(&world, made.job.id).await;
    if materialized_files == 3 {
        assert_eq!(state, "succeeded", "{error:?}");
        assert_eq!(hosting::status(&world, made.id).await.0, "running");
    } else {
        assert_eq!(state, "failed");
        let error = error.unwrap();
        assert_eq!(error["code"], "initial_files_unavailable");
        assert_eq!(
            error["message"],
            "the new environment did not receive every selected file"
        );
        assert_eq!(error["undone"], true);
        assert_ne!(hosting::status(&world, made.id).await.0, "running");
    }
    let copied_hash = captured.lock().unwrap().unwrap();
    let copied = world.engine().read_manifest(&copied_hash).await.unwrap();
    assert_eq!(copied.entries(), manifest.entries());
    assert_eq!(copied.header().generation, i64::from(original.generation));
    assert_eq!(
        world.commands.issued_of("fs.snapshot"),
        0,
        "saved files are not recaptured"
    );
    assert_eq!(
        world.commands.issued_of("fs.record.read"),
        0,
        "the blob-store snapshot needs no live export"
    );
    let mut conn = world.pool.acquire().await.unwrap();
    let after = envs::environment(&mut conn, prepared.source)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(after.workspace_root, next_root);
    assert_eq!(after.generation, next_generation);
    assert_eq!(
        storage::blob(&mut conn, &prepared.hash)
            .await
            .unwrap()
            .unwrap()
            .pin_count,
        1,
        "delivery releases its temporary pin while the saved snapshot keeps its content"
    );
    assert!(
        jobs::job(&mut conn, made.job.id)
            .await
            .unwrap()
            .unwrap()
            .plan["initial_files_pins"]
            .is_null()
    );
    let content = read_all(prepared.store.get(&prepared.hash, None).await.unwrap().body)
        .await
        .unwrap();
    assert_eq!(Hash::of(&content), prepared.hash);
    world.engine().stop();
}
