//! Before-state preparation precedes every turn and lease; watcher end facts name actual changes.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]

mod common;

use common::{World, queued};
use futures::StreamExt;
use orch_authz::Caller;
use orch_blob_contract::store::{BlobStore, stream_of};
use orch_blob_local::LocalBlobStore;
use orch_db::queries::{storage, threads};
use orch_engine::manifest::{self, Entry, EntryKind, Header, Written};
use orch_link_contract::commands::{Command, CommandKind, CommandResult};
use orch_link_contract::events::{Event, TurnCompleted, TurnOutcome};
use orch_link_contract::source::{FsRecordResult, RecordState, WorkspaceObserveResult};
use orch_types::{Hash, Time};
use serde_json::json;
use std::sync::Arc;

fn fallback() -> FsRecordResult {
    FsRecordResult {
        state: RecordState::Unavailable,
        tree_ref: None,
        repository_head: None,
        manifest_blob: None,
        files: 0,
        bytes: 0,
        expires_at: None,
        reason: Some("not_ready".to_owned()),
    }
}

#[tokio::test]
async fn the_latest_requested_status_wins_with_a_held_clock_and_intervening_changes_stay_stale() {
    let world = World::new().await;
    let thread = world.thread("Requested work").await;
    let mut conn = world.pool.acquire().await.unwrap();
    let row = threads::thread(&mut conn, thread).await.unwrap().unwrap();
    drop(conn);
    let environment = row.environment_id.unwrap();
    let directory = row.workspace_dir;
    let older = orch_types::new_id();
    let newer = orch_types::new_id();
    let at = Time::new(world.clock.now());
    let clear = orch_api_contract::workspace::GitStatus {
        available: true,
        reason: None,
        repository_dir: Some(directory.clone()),
        status_hash: None,
        remotes: Vec::new(),
        upstream: Some("origin/work".into()),
        branch: Some("work".into()),
        head: Some("kept".into()),
        ahead: Some(0),
        behind: Some(0),
        changes: Vec::new(),
    };
    let mut changed = clear.clone();
    changed
        .changes
        .push(orch_api_contract::workspace::GitChange {
            path: "code".into(),
            index: ".".into(),
            worktree: "M".into(),
        });
    world
        .engine()
        .begin_git_read(environment, &directory, older, at)
        .await
        .unwrap();
    world
        .engine()
        .begin_git_read(environment, &directory, newer, at)
        .await
        .unwrap();
    world
        .engine()
        .finish_git_read(environment, &directory, newer, at, &changed)
        .await
        .unwrap();
    world
        .engine()
        .finish_git_read(environment, &directory, older, at, &clear)
        .await
        .unwrap();
    let work = world.sync(thread).await.thread.work.unwrap();
    assert_eq!(work.uncommitted, Some(true));
    assert_eq!(work.changed_files, Some(1));
    assert_eq!(work.as_of, at);
    let request = orch_types::new_id();
    world
        .engine()
        .begin_git_read(environment, &directory, request, at)
        .await
        .unwrap();
    let mut tx = world.pool.begin().await.unwrap();
    let mut unit = orch_engine::unit::Unit::new(&mut tx, world.clock.now());
    let seq = unit.stamp(thread).await.unwrap();
    orch_db::queries::source::stale_work(&mut *unit.conn, thread, world.clock.now(), seq)
        .await
        .unwrap();
    unit.finish().await.unwrap();
    tx.commit().await.unwrap();
    world
        .engine()
        .finish_git_read(environment, &directory, request, at, &clear)
        .await
        .unwrap();
    let work = world.sync(thread).await.thread.work.unwrap();
    assert!(work.stale);
    assert_eq!(work.reason.as_deref(), Some("files_changed"));
    assert!(
        !serde_json::to_string(&work).unwrap().contains("_requested"),
        "private ordering never appears in a thread's facts"
    );
    let request = orch_types::new_id();
    world
        .engine()
        .begin_git_read(environment, &directory, request, at)
        .await
        .unwrap();
    let mut tx = world.pool.begin().await.unwrap();
    let mut unit = orch_engine::unit::Unit::new(&mut tx, world.clock.now());
    let seq = unit.stamp(thread).await.unwrap();
    orch_db::queries::source::stale_work_reason(
        &mut *unit.conn,
        thread,
        world.clock.now(),
        seq,
        "coverage_unknown",
    )
    .await
    .unwrap();
    unit.finish().await.unwrap();
    tx.commit().await.unwrap();
    world
        .engine()
        .finish_git_read(environment, &directory, request, at, &clear)
        .await
        .unwrap();
    let work = world.sync(thread).await.thread.work.unwrap();
    assert!(work.stale);
    assert_eq!(work.reason.as_deref(), Some("coverage_unknown"));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_slow_recording_creates_no_turn_or_lease_and_unknown_coverage_is_explained() {
    let world = World::new().await;
    world.machine.handler.on(CommandKind::FsRecord, |_| {
        Ok(CommandResult::FsRecord(fallback()))
    });
    world.machine.handler.hold(CommandKind::FsRecord);
    let thread = world.thread("Before files").await;
    let sent = world
        .engine()
        .submit_input(
            &Caller::session(world.ada),
            thread,
            queued("Keep this input"),
        )
        .await
        .unwrap();
    world.wait_began(CommandKind::FsRecord, 1).await;
    assert_eq!(
        world
            .count("SELECT count(*) FROM turns WHERE thread_id = $1", thread)
            .await,
        0
    );
    assert_eq!(
        world
            .count("SELECT count(*) FROM leases WHERE thread_id = $1", thread)
            .await,
        0
    );
    assert_eq!(world.input_state(sent.input.id).await.0, "pending");
    assert_eq!(world.began(CommandKind::SessionOpen), 0);
    world.machine.handler.release();
    let start = world.turn_start(thread, 0).await;
    let mut conn = world.pool.acquire().await.unwrap();
    let turn = threads::turn(&mut conn, start.turn_id)
        .await
        .unwrap()
        .unwrap();
    let before = storage::snapshot(&mut conn, turn.before_snapshot_id.unwrap())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(before.state, "unavailable");
    assert_eq!(before.unavailable_reason.as_deref(), Some("not_ready"));
    assert!(before.manifest_blob.is_none() && before.tree_ref.is_none());
    assert_eq!(before.storage, "environment");
    assert_eq!(common::words_after_where(&start.text), "Keep this input");
    let ran = world.machine.handler.executions();
    assert!(
        ran.iter()
            .position(|run| run.kind == CommandKind::FsRecord)
            .unwrap()
            < ran
                .iter()
                .position(|run| run.kind == CommandKind::SessionOpen)
                .unwrap()
    );
}

#[tokio::test]
async fn a_recording_that_expires_before_its_answer_is_kept_as_an_expired_fallback_without_recapture()
-> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let world = World::new().await;
    let expires = Time::new(world.clock.now() - chrono::Duration::seconds(1));
    world.machine.handler.on(CommandKind::FsRecord, move |_| {
        Ok(CommandResult::FsRecord(FsRecordResult {
            state: RecordState::Complete,
            tree_ref: Some("synthetic-original-tree".to_owned()),
            repository_head: None,
            manifest_blob: Some(Hash::of(b"expired original manifest")),
            files: 1,
            bytes: 8,
            expires_at: Some(expires),
            reason: None,
        }))
    });
    let thread = world.thread("Expired recording").await;
    world
        .engine()
        .submit_input(
            &Caller::session(world.ada),
            thread,
            queued("Run with the explained fallback"),
        )
        .await?;
    let start = world.turn_start(thread, 0).await;
    let mut conn = world.pool.acquire().await?;
    let turn = threads::turn(&mut conn, start.turn_id)
        .await?
        .ok_or("turn absent")?;
    let before = storage::snapshot(
        &mut conn,
        turn.before_snapshot_id.ok_or("before-state absent")?,
    )
    .await?
    .ok_or("snapshot absent")?;
    assert_eq!(before.state, "unavailable");
    assert_eq!(before.unavailable_reason.as_deref(), Some("expired"));
    assert!(before.manifest_blob.is_none() && before.tree_ref.is_none());
    assert_eq!(world.began(CommandKind::FsRecord), 1);
    Ok(())
}

fn file(path: &str, content: &[u8]) -> Entry {
    Entry {
        path: path.to_owned(),
        kind: EntryKind::File,
        size: content.len() as i64,
        mtime_ns: 0,
        mode: 0o644,
        hash: Some(Hash::of(content)),
        link: None,
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_pending_or_failed_source_retains_the_first_input_until_explicit_preparation_succeeds() {
    use orch_api_contract::source::{SourceInfo, SourceKind, SourceState};
    for state in [SourceState::Pending, SourceState::Failed] {
        let world = World::new().await;
        let thread = world.thread("Source waits").await;
        let directory: String = world
            .scalar(
                "SELECT workspace_dir FROM threads WHERE node_id = $1",
                thread,
            )
            .await;
        let source = SourceInfo {
            kind: SourceKind::Repository,
            directory,
            repository_url: Some("https://example.test/team/repo.git".to_owned()),
            branch: Some("work".to_owned()),
            head: None,
            state,
            job: None,
            reason: Some("authentication_failed".to_owned()),
        };
        sqlx::query("UPDATE threads SET source = $2 WHERE node_id = $1")
            .bind(thread)
            .bind(serde_json::to_value(&source).unwrap())
            .execute(world.pool.pool())
            .await
            .unwrap();
        let sent = world
            .engine()
            .submit_input(
                &Caller::session(world.ada),
                thread,
                queued("Retain these words"),
            )
            .await
            .unwrap();
        if state == SourceState::Failed {
            world.wait_blocked(thread, "error").await;
            world.engine().wake(thread);
            let outcome = world.engine().preflight(thread).await.unwrap();
            assert!(
                matches!(outcome, orch_engine::preflight::Outcome::Blocked(_)),
                "a failed source does not restart on an unrelated wake"
            );
        } else {
            world.wait_state(thread, "starting").await;
        }
        assert_eq!(
            world
                .count("SELECT count(*) FROM turns WHERE thread_id = $1", thread)
                .await,
            0
        );
        assert_eq!(world.began(CommandKind::FsRecord), 0);
        assert_eq!(world.began(CommandKind::SessionOpen), 0);
        let ready = SourceInfo {
            state: SourceState::Ready,
            reason: None,
            ..source
        };
        sqlx::query("UPDATE threads SET source = $2 WHERE node_id = $1")
            .bind(thread)
            .bind(serde_json::to_value(ready).unwrap())
            .execute(world.pool.pool())
            .await
            .unwrap();
        world.machine.handler.on(CommandKind::FsRecord, |_| {
            Ok(CommandResult::FsRecord(fallback()))
        });
        world.engine().wake(thread);
        let start = world.turn_start(thread, 0).await;
        assert_eq!(common::words_after_where(&start.text), "Retain these words");
        let input: orch_types::Id = world
            .scalar("SELECT input_id FROM turns WHERE id = $1", start.turn_id)
            .await;
        assert_eq!(input, sent.input.id);
        assert_eq!(
            world
                .count(
                    "SELECT count(*) FROM thread_inputs WHERE thread_id = $1",
                    thread
                )
                .await,
            1
        );
    }
}

async fn keep(store: &LocalBlobStore, written: &Written) {
    store
        .put(
            &written.hash,
            Some(written.bytes.len() as i64),
            stream_of(vec![bytes::Bytes::copy_from_slice(&written.bytes)]),
        )
        .await
        .unwrap();
}

#[tokio::test]
async fn terminal_export_pins_are_recovered_once_and_an_independent_content_pin_remains() {
    use orch_db::queries::jobs;
    let world = World::new().await;
    let thread = world.thread("Kept content").await;
    let directory = tempfile::tempdir().unwrap();
    let store = Arc::new(
        LocalBlobStore::open(directory.path().join("content"))
            .await
            .unwrap(),
    );
    world.engine().attach_blobs(store.clone());
    let bytes = bytes::Bytes::from_static(b"retained raw binary\0\xff");
    let hash = Hash::of(&bytes);
    store
        .put(
            &hash,
            Some(bytes.len() as i64),
            stream_of(vec![bytes.clone()]),
        )
        .await
        .unwrap();
    let mut tx = world.pool.begin().await.unwrap();
    storage::register_blob(&mut tx, &hash, bytes.len() as i64, world.clock.now())
        .await
        .unwrap();
    storage::pin_blobs(&mut tx, &[hash]).await.unwrap(); // another retained association
    storage::pin_blobs(&mut tx, &[hash]).await.unwrap(); // this export's temporary association
    let progress = serde_json::to_value(orch_engine::jobs::runner::initial_progress(
        &orch_engine::jobs::source::PHASES,
    ))
    .unwrap();
    let row = jobs::insert(
        &mut tx,
        &jobs::NewJob {
            id: orch_types::new_id(),
            kind: orch_engine::jobs::fork::KIND,
            node_id: Some(thread),
            environment_id: Some(world.environment),
            provider_id: None,
            plan: &json!({ "fork_file_pins": [hash] }),
            progress: &progress,
            created_by: Some(world.ada),
        },
        world.clock.now(),
    )
    .await
    .unwrap();
    jobs::finish(
        &mut tx,
        row.id,
        "failed",
        &progress,
        None,
        None,
        world.clock.now(),
    )
    .await
    .unwrap();
    tx.commit().await.unwrap();
    // A startup sweep may already own the lease. An explicit call is then a no-op;
    // the durable receipt, not the number of calls, says that recovery completed.
    common::eventually("the terminal export's pin release is committed", || async {
        world.engine().run_retention().await.unwrap();
        let mut conn = world.pool.acquire().await.unwrap();
        jobs::job(&mut conn, row.id).await.unwrap().unwrap().plan["fork_file_pins_released"] == true
    })
    .await;
    world.engine().run_retention().await.unwrap();
    let mut conn = world.pool.acquire().await.unwrap();
    let count: i32 = sqlx::query_scalar("SELECT pin_count FROM blobs WHERE hash = $1")
        .bind(hash.as_bytes().as_slice())
        .fetch_one(&mut *conn)
        .await
        .unwrap();
    assert_eq!(
        count, 1,
        "only the terminal export's temporary pin is released"
    );
    let receipt = jobs::job(&mut conn, row.id).await.unwrap().unwrap();
    assert_eq!(receipt.plan["fork_file_pins_released"], true);
    let read = store.get(&hash, None).await.unwrap();
    let kept = read
        .body
        .collect::<Vec<_>>()
        .await
        .into_iter()
        .flat_map(Result::unwrap)
        .collect::<Vec<_>>();
    assert_eq!(
        kept,
        bytes.as_ref(),
        "the remaining association still reads its original bytes"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn failure_attention_uses_acceptance_order_even_when_clock_and_later_row_updates_match() {
    use orch_api_contract::source::{SourceInfo, SourceKind, SourceState};
    // The wall clock is held, so that the new input is made at the very moment the turn ended.
    let world = World::with_clock(Arc::new(orch_testkit::clock::manual())).await;
    world.machine.handler.on(CommandKind::FsRecord, |_| {
        Ok(CommandResult::FsRecord(fallback()))
    });
    let thread = world.thread("Failure attention").await;
    let sent = world
        .engine()
        .submit_input(
            &Caller::session(world.ada),
            thread,
            queued("Original words"),
        )
        .await
        .unwrap();
    let start = world.turn_start(thread, 0).await;
    world
        .say(
            thread,
            Event::TurnCompleted(TurnCompleted {
                turn_id: start.turn_id,
                status: TurnOutcome::Failed,
                error: None,
            }),
        )
        .await;
    world.wait_turn(start.turn_id, "failed").await;
    assert!(world.sync(thread).await.thread.attention.failed);

    // These are real thread stamps, but they describe an older input edit and a late
    // turn update. Neither is a newly accepted input, even at this held clock.
    let mut tx = world.pool.begin().await.unwrap();
    let mut unit = orch_engine::unit::Unit::new(&mut tx, world.clock.now());
    let edited = unit.stamp(thread).await.unwrap();
    sqlx::query("UPDATE thread_inputs SET seq_updated = $2 WHERE id = $1")
        .bind(sent.input.id)
        .bind(edited)
        .execute(&mut *unit.conn)
        .await
        .unwrap();
    let late = unit.stamp(thread).await.unwrap();
    sqlx::query("UPDATE turns SET seq_updated = $2 WHERE id = $1")
        .bind(start.turn_id)
        .bind(late)
        .execute(&mut *unit.conn)
        .await
        .unwrap();
    let row = threads::thread(&mut *unit.conn, thread)
        .await
        .unwrap()
        .unwrap();
    let source = SourceInfo {
        kind: SourceKind::Repository,
        directory: row.workspace_dir,
        repository_url: Some("https://example.test/team/repo.git".to_owned()),
        branch: None,
        head: None,
        state: SourceState::Pending,
        job: None,
        reason: None,
    };
    let seq = unit.stamp(thread).await.unwrap();
    sqlx::query("UPDATE threads SET source = $2, seq_state = $3 WHERE node_id = $1")
        .bind(thread)
        .bind(serde_json::to_value(source).unwrap())
        .bind(seq)
        .execute(&mut *unit.conn)
        .await
        .unwrap();
    unit.finish().await.unwrap();
    tx.commit().await.unwrap();
    assert!(
        world.sync(thread).await.thread.attention.failed,
        "an old edit did not answer the failure"
    );

    let newer = world
        .engine()
        .submit_input(
            &Caller::session(world.ada),
            thread,
            queued("A new response"),
        )
        .await
        .unwrap();
    let same_time: bool = sqlx::query_scalar("SELECT i.created_at = t.completed_at FROM thread_inputs i JOIN turns t ON t.id = $2 WHERE i.id = $1")
        .bind(newer.input.id).bind(start.turn_id).fetch_one(world.pool.pool()).await.unwrap();
    assert!(
        same_time,
        "the regression deliberately holds the wall clock"
    );
    assert_eq!(
        world
            .count("SELECT count(*) FROM turns WHERE thread_id = $1", thread)
            .await,
        1,
        "the new response still waits for source, so another turn did not clear the flag"
    );
    assert!(
        !world.sync(thread).await.thread.attention.failed,
        "a newly accepted input clears the old failure independently of its hold"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_successful_empty_recording_survives_store_validation_and_is_kept_for_the_turn() {
    let world = World::new().await;
    let directory = tempfile::tempdir().unwrap();
    let store = Arc::new(LocalBlobStore::open(directory.path()).await.unwrap());
    world.engine().attach_blobs(store.clone());
    let thread = world.thread("An empty before-state is complete").await;
    let (root, relative, generation): (String, String, i32) = sqlx::query_as(
        "SELECT e.workspace_root,t.workspace_dir,e.generation FROM threads t JOIN environments e ON e.id=t.environment_id WHERE t.node_id=$1")
        .bind(thread).fetch_one(world.pool.pool()).await.unwrap();
    let before = manifest::build(
        &Header {
            v: manifest::MANIFEST_VERSION,
            root: orch_engine::sessions::absolute_directory(&root, &relative),
            generated_at: Time::new(world.clock.now()),
            generation: i64::from(generation),
            case_sensitive: true,
        },
        Vec::new(),
    )
    .unwrap();
    keep(&store, &before).await;
    let hash = before.hash;
    let expires = Time::new(world.clock.now() + chrono::Duration::days(7));
    world.machine.handler.on(CommandKind::FsRecord, move |_| {
        Ok(CommandResult::FsRecord(FsRecordResult {
            state: RecordState::Complete,
            tree_ref: Some("4b825dc642cb6eb9a060e54bf8d69288fbee4904".into()),
            repository_head: None,
            manifest_blob: Some(hash),
            files: 0,
            bytes: 0,
            expires_at: Some(expires),
            reason: None,
        }))
    });
    world
        .engine()
        .submit_input(
            &Caller::session(world.ada),
            thread,
            queued("Start from the empty directory"),
        )
        .await
        .unwrap();
    let start = world.turn_start(thread, 0).await;
    let mut conn = world.pool.acquire().await.unwrap();
    let turn = threads::turn(&mut conn, start.turn_id)
        .await
        .unwrap()
        .unwrap();
    let saved = storage::snapshot(&mut conn, turn.before_snapshot_id.unwrap())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(saved.state, "complete");
    assert_eq!(saved.unavailable_reason, None);
    assert_eq!(saved.manifest_blob, Some(hash));
    assert_eq!(saved.file_count, 0);
    assert_eq!(saved.total_bytes, 0);
    assert_eq!(
        saved.tree_ref.as_deref(),
        Some("4b825dc642cb6eb9a060e54bf8d69288fbee4904")
    );
    let id = saved.id;
    drop(conn);
    // Simulate the old server losing only its validation/store attachment at capture time.
    // The native command and original immutable manifest remain successful and unchanged.
    sqlx::query("UPDATE snapshots SET state='unavailable',unavailable_reason='store_unavailable',tree_ref=NULL,manifest_blob=NULL WHERE id=$1")
        .bind(id).execute(world.pool.pool()).await.unwrap();
    assert!(world.engine().recover_recording(id).await.unwrap());
    assert!(
        !world.engine().recover_recording(id).await.unwrap(),
        "revalidation is idempotent"
    );
    assert_eq!(
        world.began(CommandKind::FsRecord),
        1,
        "recovery must never record today's files"
    );
    let mut conn = world.pool.acquire().await.unwrap();
    let restored = storage::snapshot(&mut conn, id).await.unwrap().unwrap();
    assert_eq!(restored.state, "complete");
    assert_eq!(restored.manifest_blob, Some(hash));
    let pins: i32 = sqlx::query_scalar("SELECT pin_count FROM blobs WHERE hash=$1")
        .bind(hash.as_bytes().as_slice())
        .fetch_one(&mut *conn)
        .await
        .unwrap();
    assert_eq!(
        pins, 1,
        "the retained manifest has one logical snapshot pin"
    );
    drop(conn);
    sqlx::query("UPDATE snapshots SET state='unavailable',unavailable_reason='store_unavailable',env_generation=env_generation+1 WHERE id=$1")
        .bind(id).execute(world.pool.pool()).await.unwrap();
    assert!(
        !world.engine().recover_recording(id).await.unwrap(),
        "another generation is never repaired from this capture"
    );
    assert_eq!(world.began(CommandKind::FsRecord), 1);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn complete_before_and_watcher_end_states_make_one_bounded_actual_change_history() {
    let world = World::new().await;
    let directory = tempfile::tempdir().unwrap();
    let store = Arc::new(
        LocalBlobStore::open(directory.path().join("content"))
            .await
            .unwrap(),
    );
    world.engine().attach_blobs(store.clone());
    let thread = world.thread("Observed files").await;
    let peer = world.thread("The same files").await;
    let separate = world.thread("Different files").await;
    sqlx::query("UPDATE threads SET workspace_dir = 'elsewhere' WHERE node_id = $1")
        .bind(separate)
        .execute(world.pool.pool())
        .await
        .unwrap();
    for id in [peer, separate] {
        sqlx::query("UPDATE threads SET work_observed = $2 WHERE node_id = $1").bind(id)
            .bind(json!({ "uncommitted": false, "unpushed": null, "changed_files": 0, "ahead": null,
                "as_of": Time::new(world.clock.now()), "stale": false, "reason": "upstream_unknown" }))
            .execute(world.pool.pool()).await.unwrap();
    }
    let relative: String = world
        .scalar(
            "SELECT workspace_dir FROM threads WHERE node_id = $1",
            thread,
        )
        .await;
    let header = Header {
        v: manifest::MANIFEST_VERSION,
        root: orch_engine::sessions::absolute_directory("/workspace", &relative),
        generated_at: Time::new(world.clock.now()),
        generation: 1,
        case_sensitive: true,
    };
    let before = manifest::build(
        &header,
        vec![file("code", b"before"), file("removed", b"was here")],
    )
    .unwrap();
    let mut after = vec![file("code", b"after")];
    for number in 0..25 {
        after.push(file(&format!("added-{number:02}"), b"new"));
    }
    let end = manifest::build(&header, after).unwrap();
    keep(&store, &before).await;
    keep(&store, &end).await;
    let before_hash = before.hash;
    let end_hash = end.hash;
    let expires = Time::new(world.clock.now() + chrono::Duration::days(7));
    world
        .machine
        .handler
        .on(CommandKind::FsRecord, move |command| {
            let Command::FsRecord(_) = command else {
                panic!("unexpected command")
            };
            Ok(CommandResult::FsRecord(FsRecordResult {
                state: RecordState::Complete,
                tree_ref: Some("an-owned-tree".to_owned()),
                repository_head: None,
                manifest_blob: Some(before_hash),
                files: 2,
                bytes: 14,
                expires_at: Some(expires),
                reason: None,
            }))
        });
    world
        .machine
        .handler
        .on(CommandKind::WorkspaceObserve, move |_| {
            Ok(CommandResult::WorkspaceObserve(WorkspaceObserveResult {
                end_manifest_blob: Some(end_hash),
            }))
        });
    world
        .engine()
        .submit_input(&Caller::session(world.ada), thread, queued("Change code"))
        .await
        .unwrap();
    let start = world.turn_start(thread, 0).await;
    world
        .say(
            thread,
            Event::TurnCompleted(TurnCompleted {
                turn_id: start.turn_id,
                status: TurnOutcome::Completed,
                error: None,
            }),
        )
        .await;
    world.wait_turn(start.turn_id, "completed").await;
    // The machine is asked after the commit that ended the turn, with no transaction open.
    common::eventually(
        "the end of the turn is observed after its commit",
        || async {
            !world
                .scalar::<bool>(
                    "SELECT files_end_due FROM turns WHERE id = $1",
                    start.turn_id,
                )
                .await
        },
    )
    .await;
    let mut conn = world.pool.acquire().await.unwrap();
    let turn = threads::turn(&mut conn, start.turn_id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(turn.end_manifest_blob, Some(end_hash.as_bytes().to_vec()));
    let end_pins: i32 = sqlx::query_scalar("SELECT pin_count FROM blobs WHERE hash = $1")
        .bind(end_hash.as_bytes().as_slice())
        .fetch_one(&mut *conn)
        .await
        .unwrap();
    assert_eq!(
        end_pins, 1,
        "the turn retains its verified end manifest for later comparisons"
    );
    let end_files = turn.files_end_manifest.as_ref().unwrap();
    assert_eq!(
        end_files["code"]["h"],
        serde_json::to_value(Hash::of(b"after")).unwrap()
    );
    assert_eq!(end_files["code"]["t"], "f");
    assert_eq!(end_files["code"]["x"], 0);
    assert!(
        !end_files.to_string().contains("after"),
        "stored end facts contain hashes, not raw file bytes"
    );
    let snapshot = storage::snapshot(&mut conn, turn.before_snapshot_id.unwrap())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(snapshot.manifest_blob, Some(before_hash));
    let details: serde_json::Value = sqlx::query_scalar("SELECT details FROM audit_log WHERE action = 'workspace.changed_by_agent' AND node_id = $1")
        .bind(thread).fetch_one(&mut *conn).await.unwrap();
    assert_eq!(details["files"], 27);
    assert_eq!(details["added"], 25);
    assert_eq!(details["removed"], 1);
    assert_eq!(details["paths"].as_array().unwrap().len(), 20);
    assert_eq!(details["more"], 7);
    assert_eq!(details["turn_id"], start.turn_id.to_string());
    assert!(
        !details.to_string().contains("before"),
        "history contains names and counts, never file content"
    );
    assert_eq!(
        world.began(CommandKind::GitStatus),
        0,
        "turn end does not invoke Git"
    );
    assert!(
        threads::thread(&mut conn, thread)
            .await
            .unwrap()
            .unwrap()
            .work_observed
            .is_none(),
        "watcher hashes do not manufacture Git publication facts"
    );
    drop(conn);
    common::eventually(
        "the overlapping observation is invalidated after the turn commits",
        || async {
            world
                .sync(peer)
                .await
                .thread
                .work
                .is_some_and(|work| work.stale)
        },
    )
    .await;
    let peer_work = world.sync(peer).await.thread.work.unwrap();
    assert_eq!(
        peer_work.uncommitted,
        Some(false),
        "an included change does not invent a Git count"
    );
    assert_eq!(peer_work.reason.as_deref(), Some("files_changed"));
    assert!(!world.sync(separate).await.thread.work.unwrap().stale);
    // A crash after the turn commit but before its follow-up retains the pending bit.
    sqlx::query("UPDATE turns SET files_work_pending = true WHERE id = $1")
        .bind(start.turn_id)
        .execute(world.pool.pool())
        .await
        .unwrap();
    sqlx::query("UPDATE threads SET work_observed = jsonb_set(work_observed, '{stale}', 'false') WHERE node_id = $1")
        .bind(peer).execute(world.pool.pool()).await.unwrap();
    world.engine().run_retention().await.unwrap();
    assert!(
        world.sync(peer).await.thread.work.unwrap().stale,
        "startup/retention replays a committed invalidation"
    );
    assert!(
        !world
            .scalar::<bool>(
                "SELECT files_work_pending FROM turns WHERE id = $1",
                start.turn_id
            )
            .await
    );
    world
        .say(
            thread,
            Event::TurnCompleted(TurnCompleted {
                turn_id: start.turn_id,
                status: TurnOutcome::Completed,
                error: None,
            }),
        )
        .await;
    assert_eq!(world.count("SELECT count(*) FROM audit_log WHERE action = 'workspace.changed_by_agent' AND node_id = $1", thread).await, 1);

    // Expiry keeps the turn's menu and raw end facts, while dropping only the associations
    // that owned its two content-store pins. Repeating the sweep cannot release them twice.
    let mut conn = world.pool.acquire().await.unwrap();
    sqlx::query("UPDATE snapshots SET expires_at = $2 WHERE id = $1")
        .bind(snapshot.id)
        .bind(world.clock.now() - chrono::Duration::seconds(1))
        .execute(&mut *conn)
        .await
        .unwrap();
    drop(conn);
    world.machine.handler.on(CommandKind::FsRecordPrune, |_| {
        Ok(CommandResult::FsRecordPrune(
            orch_link_contract::source::FsRecordPruneResult { removed: 1 },
        ))
    });
    world.engine().run_retention().await.unwrap();
    world.engine().run_retention().await.unwrap();
    let mut conn = world.pool.acquire().await.unwrap();
    let kept_turn = threads::turn(&mut conn, start.turn_id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(kept_turn.before_snapshot_id, Some(snapshot.id));
    assert_eq!(kept_turn.files_end_manifest, turn.files_end_manifest);
    assert!(kept_turn.end_manifest_blob.is_none());
    let expired = storage::snapshot(&mut conn, snapshot.id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(expired.state, "expired");
    assert_eq!(expired.unavailable_reason.as_deref(), Some("expired"));
    assert!(expired.manifest_blob.is_none());
    for hash in [before_hash, end_hash] {
        let pins: i32 = sqlx::query_scalar("SELECT pin_count FROM blobs WHERE hash = $1")
            .bind(hash.as_bytes().as_slice())
            .fetch_one(&mut *conn)
            .await
            .unwrap();
        assert_eq!(pins, 0, "expiry releases each old source association once");
    }
}
