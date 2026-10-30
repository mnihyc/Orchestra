//! A thread's directory kept as of a moment, over HTTP: taken as a job that the machine carries out and reports on, listed, read file by file, downloaded as one stream, renamed and deleted; and the job read and cancelled by whoever may; with the files it holds pinned for as long as it exists.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]

mod places;
mod support;
mod workspace;

use chrono::Duration;
use http::StatusCode;
use orch_api_contract::common::{ErrorCode, PageQuery};
use orch_api_contract::nodes::JobState;
use orch_api_contract::workspace::{
    DirEntryKind, EnvironmentFilesExport, Job, JobCancel, JobGet, PhaseState, Snapshot,
    SnapshotArchiveFormat, SnapshotArchiveGet, SnapshotArchiveGetQuery, SnapshotDelete,
    SnapshotFileContentGet, SnapshotFileList, SnapshotGet, SnapshotPathQuery, SnapshotState,
    SnapshotUpdate, SnapshotUpdateBody, ThreadSnapshotCreate, ThreadSnapshotCreateBody,
    ThreadSnapshotCreateSelection, ThreadSnapshotList,
};
use orch_api_core::testing::Reply;
use orch_db::queries::storage::{self, NewSnapshot, SnapshotEnd, snapshot_state};
use orch_link_contract::commands::CommandKind;
use orch_types::{Hash, Id, new_id};
use places::Who;
use serde_json::json;
use std::io::Read;
use std::sync::Arc;
use workspace::{Site, eventually};

async fn live_site() -> Site {
    let mut site = Site::new().await;
    site.disk.put("web/src/main.rs", b"fn main() {}\n");
    site.disk.put("web/src/lib.rs", b"pub fn lib() {}\n");
    site.disk.put("web/README.md", b"# Web\n");
    site.disk.put("web/docs/guide.txt", b"A guide.\n");
    site.disk
        .put("other/elsewhere.txt", b"not in the thread's directory\n");
    site.power_on();
    site.app().app.engine.start();
    site
}

#[tokio::test]
async fn a_background_count_is_readable_to_its_resource_reader_and_its_maker_can_stop_it() {
    let mut site = Site::new().await;
    let mut conn = site.world.conn().await;
    let id = new_id();
    let progress = serde_json::to_value(orch_engine::jobs::runner::initial_progress(
        &orch_engine::jobs::measurement::PHASES,
    ))
    .unwrap();
    orch_db::queries::jobs::insert(&mut conn, &orch_db::queries::jobs::NewJob {
        id, kind: "measurement", node_id: None, environment_id: None, provider_id: None,
        plan: &json!({"subject":{"environment_id":site.environment},"keep_until":site.world.now()}),
        progress: &progress, created_by: Some(site.ada.id()),
    }, site.world.now()).await.unwrap();
    drop(conn);
    let counted = job(&site, &site.ada, id).await.ok();
    assert_eq!(counted.kind, "measurement");
    assert_eq!(counted.state, JobState::Queued);
    assert!(!job(&site, &site.eve, id).await.status.is_success());
    assert!(
        !site
            .app()
            .call(JobCancel, &[&id])
            .token(&site.eve.token)
            .send()
            .await
            .status
            .is_success()
    );
    let stopped = site
        .app()
        .call(JobCancel, &[&id])
        .token(&site.ada.token)
        .send()
        .await
        .ok();
    assert!(stopped.progress.activity.unwrap().stopping);
    assert_eq!(
        stopped.state,
        JobState::Queued,
        "accepting a stop must not pretend that cleanup has ended"
    );
    site.power_on();
    let dispatcher = site.app().app.link.dispatcher();
    let issued = dispatcher
        .issue(
            site.machine,
            &orch_link_contract::commands::SerialKey::Environment(site.environment),
            &orch_link_contract::commands::FsSnapshot {
                work: Some(orch_link_contract::commands::WorkReport {
                    job_id: id,
                    phase: "measure".into(),
                    timeout_secs: 86400,
                    files_total: None,
                    bytes_total: None,
                }),
                environment_id: site.environment,
                job_id: id,
                selection: Default::default(),
            },
        )
        .await
        .unwrap();
    assert_eq!(
        dispatcher.status(issued.id).await.unwrap().unwrap().state,
        "cancelled",
        "a cancelled parent admitted new machine I/O"
    );
    assert!(
        dispatcher.stopped(issued.id).await.unwrap(),
        "a never-admitted command needs no remote cleanup"
    );
    assert_eq!(site.sent(CommandKind::FsSnapshot), 0);
}

async fn create(site: &Site, by: &Who, body: ThreadSnapshotCreateBody) -> Reply<Job> {
    site.app()
        .call(ThreadSnapshotCreate, &[&site.thread])
        .token(&by.token)
        .body(&body)
        .send()
        .await
}

async fn job(site: &Site, by: &Who, id: Id) -> Reply<Job> {
    site.app()
        .call(JobGet, &[&id])
        .token(&by.token)
        .send()
        .await
}

async fn snapshot(site: &Site, by: &Who, id: Id) -> Reply<Snapshot> {
    site.app()
        .call(SnapshotGet, &[&id])
        .token(&by.token)
        .send()
        .await
}

/// Waits until the job is over, and gives it.
async fn over(site: &Site, id: Id) -> Job {
    eventually("the job to end", || async {
        !matches!(
            job(site, &site.ada, id).await.ok().state,
            JobState::Queued | JobState::Running
        )
    })
    .await;
    job(site, &site.ada, id).await.ok()
}

/// Takes a snapshot of the thread's directory and waits for it.
async fn taken(site: &Site) -> Snapshot {
    let begun = create(site, &site.ada, ThreadSnapshotCreateBody::default())
        .await
        .ok();
    let ended = over(site, begun.id).await;
    assert_eq!(ended.state, JobState::Succeeded, "{ended:?}");
    let id = ended.result.unwrap()["snapshot_id"]
        .as_str()
        .unwrap()
        .parse()
        .unwrap();
    snapshot(site, &site.ada, id).await.ok()
}

fn named(label: &str) -> ThreadSnapshotCreateBody {
    ThreadSnapshotCreateBody {
        label: Some(label.to_owned()),
        selection: None,
    }
}

// ---------------------------------------------------------------- taking one

#[tokio::test]
async fn preflight_measures_generated_files_and_refuses_an_oversized_selection_before_a_job_exists()
{
    use orch_api_contract::workspace::{
        SnapshotSelectionRefusal, ThreadSnapshotPreflight, ThreadSnapshotPreflightBody,
    };
    let site = live_site().await;
    for index in 0..250_001 {
        site.disk.put(&format!("generated/large/{index}"), b"x");
    }
    site.disk.put("generated/small/kept", b"kept");
    let selection = ThreadSnapshotCreateSelection {
        include: vec!["generated".into()],
        exclude: None,
    };
    let before: i64 =
        sqlx::query_scalar("SELECT count(*) FROM jobs WHERE kind = 'snapshot' AND node_id = $1")
            .bind(site.thread)
            .fetch_one(&mut *site.world.conn().await)
            .await
            .unwrap();
    let measured = site
        .app()
        .call(ThreadSnapshotPreflight, &[&site.thread])
        .token(&site.ada.token)
        .body(&ThreadSnapshotPreflightBody {
            selection: Some(selection.clone()),
        })
        .send()
        .await
        .ok();
    assert!(!measured.allowed);
    assert_eq!(measured.refusal, Some(SnapshotSelectionRefusal::EntryLimit));
    assert!(measured.reason.unwrap().contains("250,000"));
    assert_eq!(measured.file_count, 250_002);
    assert_eq!(measured.total_bytes, 250_005);
    assert_eq!(measured.largest_directories[0].path, "generated");
    assert_eq!(
        create(
            &site,
            &site.ada,
            ThreadSnapshotCreateBody {
                label: None,
                selection: Some(selection)
            }
        )
        .await
        .status,
        StatusCode::CONFLICT
    );
    let after: i64 =
        sqlx::query_scalar("SELECT count(*) FROM jobs WHERE kind = 'snapshot' AND node_id = $1")
            .bind(site.thread)
            .fetch_one(&mut *site.world.conn().await)
            .await
            .unwrap();
    assert_eq!(before, after, "refusal happens before a job is made");
    let narrow = site
        .app()
        .call(ThreadSnapshotPreflight, &[&site.thread])
        .token(&site.ada.token)
        .body(&ThreadSnapshotPreflightBody {
            selection: Some(ThreadSnapshotCreateSelection {
                include: vec!["generated/small".into()],
                exclude: None,
            }),
        })
        .send()
        .await
        .ok();
    assert!(narrow.allowed);
    assert_eq!(narrow.file_count, 1);
    assert_eq!(narrow.total_bytes, 4);
    assert!(narrow.estimate.seconds_high > 0);
}

#[tokio::test]
async fn a_snapshot_is_a_job_that_ends_with_the_files_of_the_threads_directory_kept() {
    let site = live_site().await;
    let begun = create(&site, &site.ada, named("Before the refactor")).await;
    assert_eq!(begun.status, StatusCode::ACCEPTED);
    let begun = begun.ok();
    assert_eq!(begun.kind, "snapshot");
    assert_eq!(begun.environment_id, Some(site.environment));
    assert_eq!(begun.node_id, Some(site.thread));
    assert!(matches!(begun.state, JobState::Queued | JobState::Running));
    assert_eq!(
        begun
            .progress
            .phases
            .iter()
            .map(|phase| phase.name.as_str())
            .collect::<Vec<_>>(),
        ["scan", "hash", "upload", "manifest", "finalize"]
    );
    assert_eq!(
        begun
            .progress
            .phases
            .iter()
            .map(|phase| phase.weight)
            .sum::<u32>(),
        100
    );

    let ended = over(&site, begun.id).await;
    assert_eq!(ended.state, JobState::Succeeded, "{ended:?}");
    assert_eq!(ended.progress.percent, 100);
    assert!(
        ended
            .progress
            .phases
            .iter()
            .all(|phase| phase.state == PhaseState::Done)
    );
    assert!(ended.started_at.is_some() && ended.finished_at.is_some());
    let id: Id = ended.result.as_ref().unwrap()["snapshot_id"]
        .as_str()
        .unwrap()
        .parse()
        .unwrap();

    let kept = snapshot(&site, &site.ada, id).await.ok();
    assert_eq!(kept.state, SnapshotState::Complete);
    assert_eq!(kept.label, "Before the refactor");
    assert_eq!(kept.thread_id, Some(site.thread));
    assert_eq!(kept.environment.id, site.environment);
    assert_eq!(kept.file_count, 4, "the files of web, and not the others");
    assert_eq!(kept.total_bytes, 13 + 16 + 6 + 9);
    assert_eq!(kept.missing_count, 0);
    assert_eq!(kept.selection.include, ["web"]);
    assert!(kept.completed_at.is_some());

    // The machine was asked for the directory of the thread, under the job that reports on it.
    let asked = site
        .played
        .as_ref()
        .unwrap()
        .seen_of(CommandKind::FsSnapshot);
    assert_eq!(asked.len(), 1);
    assert_eq!(asked[0].body["selection"]["include"], json!(["web"]));
    assert_eq!(asked[0].body["job_id"], json!(begun.id));
    let audit = site.world.audit().await;
    assert!(
        audit
            .iter()
            .any(|action| action == "workspace.snapshot_created"),
        "{audit:?}"
    );
}

#[tokio::test]
async fn the_files_a_snapshot_holds_are_pinned_for_as_long_as_it_exists() {
    let site = live_site().await;
    let kept = taken(&site).await;
    let mut conn = site.world.conn().await;
    let row = storage::snapshot(&mut conn, kept.id)
        .await
        .unwrap()
        .unwrap();
    let manifest = row.manifest_blob.unwrap();
    for content in [b"fn main() {}\n".as_slice(), b"# Web\n"] {
        let blob = storage::blob(&mut conn, &Hash::of(content))
            .await
            .unwrap()
            .unwrap();
        assert_eq!(blob.pin_count, 1, "{}", String::from_utf8_lossy(content));
    }
    assert_eq!(
        storage::blob(&mut conn, &manifest)
            .await
            .unwrap()
            .unwrap()
            .pin_count,
        1
    );
    drop(conn);

    // They survive any length of time that nobody asked for them.
    site.world.advance(Duration::hours(24 * 30));
    let gone = orch_api_core::cache::collect_garbage(
        &site.app().app.db,
        &site.app().app.blobs,
        site.app().app.clock.as_ref(),
        &site.app().app.config,
    )
    .await
    .unwrap();
    assert_eq!(gone.blobs, 0);
}

#[tokio::test]
async fn what_the_machine_reports_moves_the_bar_forward_and_never_back() {
    let site = live_site().await;
    let gate = Arc::new(tokio::sync::Notify::new());
    *site.disk.gate.lock().unwrap() = Some(Arc::clone(&gate));
    let begun = create(&site, &site.ada, ThreadSnapshotCreateBody::default())
        .await
        .ok();
    eventually("the machine to take the snapshot up", || async {
        site.sent(CommandKind::FsSnapshot) == 1
    })
    .await;

    site.machine_reports(begun.id, 40, "upload").await;
    let first = job(&site, &site.ada, begun.id).await.ok();
    assert_eq!(first.state, JobState::Running);
    assert_eq!(first.progress.percent, 40);
    assert_eq!(first.progress.phase, "upload");
    assert_eq!(first.progress.files_done, 3);
    assert_eq!(first.progress.message.as_deref(), Some("reading"));

    site.machine_reports(begun.id, 25, "upload").await;
    assert_eq!(
        job(&site, &site.ada, begun.id).await.ok().progress.percent,
        40,
        "never back"
    );
    site.machine_reports(begun.id, 100, "manifest").await;
    assert!(
        job(&site, &site.ada, begun.id).await.ok().progress.percent < 100,
        "the last phase is the system's own: the bar is full only when the snapshot is kept"
    );

    gate.notify_one();
    let ended = over(&site, begun.id).await;
    assert_eq!(
        (ended.state, ended.progress.percent),
        (JobState::Succeeded, 100)
    );
}

#[tokio::test]
async fn a_machine_that_calls_every_phase_done_does_not_take_the_snapshot_past_the_systems_own() {
    let site = live_site().await;
    let gate = Arc::new(tokio::sync::Notify::new());
    *site.disk.gate.lock().unwrap() = Some(Arc::clone(&gate));
    let begun = create(&site, &site.ada, ThreadSnapshotCreateBody::default())
        .await
        .ok();
    eventually("the machine to take the snapshot up", || async {
        site.sent(CommandKind::FsSnapshot) == 1
    })
    .await;

    // The program on a machine has done all it does, and says so, the last phase too.
    site.machine_reports_phases(begun.id, 100, "finalize", ["done"; 5])
        .await;
    let said = job(&site, &site.ada, begun.id).await.ok();
    assert_eq!(said.state, JobState::Running);
    assert!(said.progress.percent < 100);

    gate.notify_one();
    let ended = over(&site, begun.id).await;
    assert_eq!(ended.state, JobState::Succeeded, "{ended:?}");
    let id = ended.result.unwrap()["snapshot_id"]
        .as_str()
        .unwrap()
        .parse()
        .unwrap();
    assert_eq!(
        snapshot(&site, &site.ada, id).await.ok().state,
        SnapshotState::Complete,
        "the snapshot was kept, not left running: the system's own phase was carried out"
    );
}

#[tokio::test]
async fn files_that_could_not_be_read_make_a_partial_snapshot_that_lists_them() {
    let site = live_site().await;
    site.disk.make_unreadable("web/README.md");
    let kept = taken(&site).await;
    assert_eq!(kept.state, SnapshotState::Partial);
    assert_eq!(kept.missing_count, 1);
    assert_eq!(kept.file_count, 4);
    let listed = site
        .app()
        .call(SnapshotFileList, &[&kept.id])
        .token(&site.ada.token)
        .query(&SnapshotPathQuery {
            path: String::new(),
        })
        .send()
        .await
        .ok();
    let readme = listed
        .entries
        .iter()
        .find(|entry| entry.name == "README.md")
        .unwrap();
    assert!(!readme.cached, "it is listed, and has no content");
    // Its content is not there to be read.
    let content = site
        .app()
        .call(SnapshotFileContentGet, &[&kept.id])
        .token(&site.ada.token)
        .query(&SnapshotPathQuery {
            path: "README.md".to_owned(),
        })
        .send_bytes()
        .await;
    assert_eq!(content.problem().code, ErrorCode::NotFound);
}

#[tokio::test]
async fn a_selection_names_the_paths_that_are_taken_and_the_ones_left_out() {
    let site = live_site().await;
    let begun = create(
        &site,
        &site.ada,
        ThreadSnapshotCreateBody {
            label: None,
            selection: Some(ThreadSnapshotCreateSelection {
                include: vec!["web/src".to_owned()],
                exclude: Some(vec!["web/src/lib.rs".to_owned()]),
            }),
        },
    )
    .await
    .ok();
    let ended = over(&site, begun.id).await;
    let id: Id = ended.result.unwrap()["snapshot_id"]
        .as_str()
        .unwrap()
        .parse()
        .unwrap();
    let kept = snapshot(&site, &site.ada, id).await.ok();
    assert_eq!(kept.file_count, 1);
    assert_eq!(kept.selection.include, ["web/src"]);
    assert_eq!(kept.selection.exclude, ["web/src/lib.rs"]);
    assert!(
        kept.label.starts_with("Snapshot "),
        "a snapshot that was given no label is named for the moment: {}",
        kept.label
    );
}

#[tokio::test]
async fn a_selection_must_name_a_path_inside_the_workspace() {
    let site = live_site().await;
    let empty = create(
        &site,
        &site.ada,
        ThreadSnapshotCreateBody {
            label: None,
            selection: Some(ThreadSnapshotCreateSelection {
                include: Vec::new(),
                exclude: None,
            }),
        },
    )
    .await
    .problem();
    assert_eq!(empty.code, ErrorCode::RequestInvalid);
    for outside in ["../x", "/etc"] {
        let refused = create(
            &site,
            &site.ada,
            ThreadSnapshotCreateBody {
                label: None,
                selection: Some(ThreadSnapshotCreateSelection {
                    include: vec![outside.to_owned()],
                    exclude: None,
                }),
            },
        )
        .await
        .problem();
        assert_eq!(refused.code, ErrorCode::NotFound, "{outside}");
    }
    let malformed = create(
        &site,
        &site.ada,
        ThreadSnapshotCreateBody {
            label: None,
            selection: Some(ThreadSnapshotCreateSelection {
                include: vec!["web/a**b".to_owned()],
                exclude: None,
            }),
        },
    )
    .await
    .problem();
    assert_eq!(malformed.code, ErrorCode::RequestInvalid);
    let long = create(&site, &site.ada, named(&"x".repeat(121)))
        .await
        .problem();
    assert_eq!(long.code, ErrorCode::RequestInvalid);
    assert_eq!(site.sent(CommandKind::FsSnapshot), 0);
}

#[tokio::test]
async fn a_tree_too_large_to_index_is_snapshotted_by_a_selection_only() {
    let site = live_site().await;
    sqlx::query("UPDATE environments SET facts = '{\"index\": \"live\"}' WHERE id = $1")
        .bind(site.environment)
        .execute(&site.world.db.pool)
        .await
        .unwrap();
    let refused = create(&site, &site.ada, ThreadSnapshotCreateBody::default())
        .await
        .problem();
    assert_eq!(refused.code, ErrorCode::WorkspaceNotIndexed);
    let with_selection = create(
        &site,
        &site.ada,
        ThreadSnapshotCreateBody {
            label: None,
            selection: Some(ThreadSnapshotCreateSelection {
                include: vec!["web/docs".to_owned()],
                exclude: None,
            }),
        },
    )
    .await;
    assert!(
        with_selection.status.is_success(),
        "{:?}",
        with_selection.body
    );
}

#[tokio::test]
async fn a_snapshot_is_not_begun_while_the_machine_is_away() {
    let site = live_site().await;
    site.power_off().await;
    let refused = create(&site, &site.ada, ThreadSnapshotCreateBody::default())
        .await
        .problem();
    assert_eq!(refused.code, ErrorCode::SourceOffline);
    let mut conn = site.world.conn().await;
    let rows: i64 = sqlx::query_scalar("SELECT count(*) FROM snapshots")
        .fetch_one(&mut *conn)
        .await
        .unwrap();
    assert_eq!(rows, 0, "nothing was begun");
}

#[tokio::test]
async fn a_viewer_reads_snapshots_and_cannot_take_them() {
    let site = live_site().await;
    let kept = taken(&site).await;
    assert!(
        snapshot(&site, &site.vic, kept.id)
            .await
            .status
            .is_success()
    );
    let refused = create(&site, &site.vic, ThreadSnapshotCreateBody::default())
        .await
        .problem();
    assert_eq!(refused.code, ErrorCode::AuthzDenied);
    assert!(places::missing_permissions(&refused).contains(&"files.manage".to_owned()));
    let stranger = create(&site, &site.eve, ThreadSnapshotCreateBody::default()).await;
    assert!(!stranger.status.is_success());
    assert_eq!(
        site.sent(CommandKind::FsSnapshot),
        1,
        "only the first reached the machine"
    );
    // A developer takes them.
    let by_dev = create(&site, &site.dev, named("by dev")).await;
    assert!(by_dev.status.is_success(), "{:?}", by_dev.body);
}

// ---------------------------------------------------------------- reading one

#[tokio::test]
async fn a_snapshot_is_read_directory_by_directory_and_file_by_file_with_no_machine_needed() {
    let site = live_site().await;
    let kept = taken(&site).await;
    site.power_off().await;

    let root = site
        .app()
        .call(SnapshotFileList, &[&kept.id])
        .token(&site.vic.token)
        .query(&SnapshotPathQuery {
            path: String::new(),
        })
        .send()
        .await
        .ok();
    let names: Vec<_> = root
        .entries
        .iter()
        .map(|entry| (entry.name.as_str(), entry.kind))
        .collect();
    assert_eq!(
        names,
        [
            ("docs", DirEntryKind::Dir),
            ("src", DirEntryKind::Dir),
            ("README.md", DirEntryKind::File),
        ]
    );
    assert!(root.entries[2].cached, "a file with content that is kept");
    let src = site
        .app()
        .call(SnapshotFileList, &[&kept.id])
        .token(&site.vic.token)
        .query(&SnapshotPathQuery {
            path: "src".to_owned(),
        })
        .send()
        .await
        .ok();
    assert_eq!(src.entries.len(), 2);

    let read = site
        .app()
        .call(SnapshotFileContentGet, &[&kept.id])
        .token(&site.vic.token)
        .query(&SnapshotPathQuery {
            path: "src/main.rs".to_owned(),
        })
        .send_bytes()
        .await;
    assert_eq!(read.status, StatusCode::OK);
    assert_eq!(read.raw.as_ref(), b"fn main() {}\n");
    assert_eq!(
        read.headers["etag"].to_str().unwrap(),
        format!("\"{}\"", Hash::of(b"fn main() {}\n"))
    );

    let missing = site
        .app()
        .call(SnapshotFileContentGet, &[&kept.id])
        .token(&site.vic.token)
        .query(&SnapshotPathQuery {
            path: "src".to_owned(),
        })
        .send_bytes()
        .await;
    assert_eq!(
        missing.problem().code,
        ErrorCode::NotFound,
        "a directory has no content"
    );
    for outside in ["../x", "/etc/passwd"] {
        let refused = site
            .app()
            .call(SnapshotFileList, &[&kept.id])
            .token(&site.vic.token)
            .query(&SnapshotPathQuery {
                path: outside.to_owned(),
            })
            .send()
            .await
            .problem();
        assert_eq!(refused.code, ErrorCode::NotFound, "{outside}");
    }
}

#[tokio::test]
async fn a_snapshot_is_for_whoever_may_read_the_files_of_its_thread() {
    let site = live_site().await;
    let kept = taken(&site).await;
    let stranger = snapshot(&site, &site.eve, kept.id).await;
    assert!(!stranger.status.is_success());
    let listed = site
        .app()
        .call(SnapshotFileList, &[&kept.id])
        .token(&site.eve.token)
        .query(&SnapshotPathQuery {
            path: String::new(),
        })
        .send()
        .await;
    assert!(!listed.status.is_success());
    let archive = site
        .app()
        .call(SnapshotArchiveGet, &[&kept.id])
        .token(&site.eve.token)
        .query(&SnapshotArchiveGetQuery::default())
        .send_bytes()
        .await;
    assert!(!archive.status.is_success());
    assert_eq!(
        snapshot(&site, &site.ada, new_id()).await.problem().code,
        ErrorCode::NotFound
    );
}

#[tokio::test]
async fn recorded_states_keep_their_owning_environment_door_after_a_thread_moves() {
    let site = live_site().await;
    let manual = taken(&site).await;
    let recorded = new_id();
    let elsewhere = new_id();
    let mut conn = site.world.conn().await;
    sqlx::query("INSERT INTO environments (id, name, owner_id, org_id, placement, home_node_id, workspace_root, machine_id, generation, status)
        VALUES ($1, 'Elsewhere', $2, $3, 'connected', $3, '/elsewhere', $4, 1, 'running')")
        .bind(elsewhere).bind(site.eve.id()).bind(site.eve.person.personal_org).bind(site.machine).execute(&mut *conn).await.unwrap();
    orch_db::queries::access::testing::add_entry(
        &mut conn,
        &orch_db::queries::access::PageKey::new("environment", elsewhere, "live"),
        "any",
        "access.own",
        ("user", Some(site.eve.id()), None),
        None,
        site.world.now(),
    )
    .await
    .unwrap();
    sqlx::query("INSERT INTO snapshots (id, thread_id, environment_id, env_generation, kind, storage, directory, tree_ref,
            label, selection, state, file_count, total_bytes, manifest_blob, created_at, completed_at)
        SELECT $1, thread_id, environment_id, env_generation, 'turn', 'environment', 'web',
            '4b825dc642cb6eb9a060e54bf8d69288fbee4904', 'Before a turn', selection, state, file_count, total_bytes,
            manifest_blob, created_at, completed_at FROM snapshots WHERE id = $2")
        .bind(recorded).bind(manual.id).execute(&mut *conn).await.unwrap();
    sqlx::query("UPDATE threads SET environment_id = $2 WHERE node_id = $1")
        .bind(site.thread)
        .bind(elsewhere)
        .execute(&mut *conn)
        .await
        .unwrap();
    drop(conn);
    assert_eq!(
        snapshot(&site, &site.vic, recorded)
            .await
            .ok()
            .environment
            .id,
        site.environment
    );
    assert!(
        !snapshot(&site, &site.vic, manual.id)
            .await
            .status
            .is_success()
    );
    assert!(
        snapshot(&site, &site.eve, manual.id)
            .await
            .status
            .is_success()
    );
    assert!(
        !snapshot(&site, &site.eve, recorded)
            .await
            .status
            .is_success()
    );
    for (person, visible) in [(&site.vic, recorded), (&site.eve, manual.id)] {
        let page = site
            .app()
            .call(ThreadSnapshotList, &[&site.thread])
            .token(&person.token)
            .query(&PageQuery {
                limit: Some(1),
                cursor: None,
            })
            .send()
            .await
            .ok();
        assert_eq!(
            page.items.iter().map(|state| state.id).collect::<Vec<_>>(),
            [visible]
        );
        assert!(
            page.next_cursor.is_none(),
            "an inaccessible state does not make another page"
        );
    }
}

#[tokio::test]
async fn the_snapshots_of_a_thread_are_listed_newest_first_a_page_at_a_time() {
    let site = live_site().await;
    let mut ids = Vec::new();
    {
        let mut conn = site.world.conn().await;
        for n in 0..5 {
            let id = new_id();
            storage::begin_snapshot(
                &mut conn,
                &NewSnapshot {
                    id,
                    thread_id: Some(site.thread),
                    environment_id: site.environment,
                    env_generation: 1,
                    label: &format!("snapshot {n}"),
                    selection: &json!({ "include": ["web"], "exclude": [] }),
                    created_by: Some(site.ada.id()),
                    at: site.world.now(),
                },
            )
            .await
            .unwrap();
            storage::end_snapshot(
                &mut conn,
                id,
                &SnapshotEnd {
                    state: snapshot_state::COMPLETE,
                    file_count: n,
                    total_bytes: 0,
                    missing_count: 0,
                    manifest_blob: None,
                    at: site.world.now(),
                },
            )
            .await
            .unwrap();
            ids.push(id);
            site.world.advance(Duration::minutes(1));
        }
    }
    let first = site
        .app()
        .call(ThreadSnapshotList, &[&site.thread])
        .token(&site.vic.token)
        .query(&PageQuery {
            limit: Some(2),
            cursor: None,
        })
        .send()
        .await
        .ok();
    assert_eq!(
        first.items.iter().map(|each| each.id).collect::<Vec<_>>(),
        [ids[4], ids[3]]
    );
    let second = site
        .app()
        .call(ThreadSnapshotList, &[&site.thread])
        .token(&site.vic.token)
        .query(&PageQuery {
            limit: Some(2),
            cursor: first.next_cursor.clone(),
        })
        .send()
        .await
        .ok();
    assert_eq!(
        second.items.iter().map(|each| each.id).collect::<Vec<_>>(),
        [ids[2], ids[1]]
    );
    let last = site
        .app()
        .call(ThreadSnapshotList, &[&site.thread])
        .token(&site.vic.token)
        .query(&PageQuery {
            limit: Some(2),
            cursor: second.next_cursor.clone(),
        })
        .send()
        .await
        .ok();
    assert_eq!(
        last.items.iter().map(|each| each.id).collect::<Vec<_>>(),
        [ids[0]]
    );
    assert_eq!(last.next_cursor, None);
    let stranger = site
        .app()
        .call(ThreadSnapshotList, &[&site.thread])
        .token(&site.eve.token)
        .query(&PageQuery::default())
        .send()
        .await;
    assert!(!stranger.status.is_success());
}

// ---------------------------------------------------------------- as one stream

#[tokio::test]
async fn a_snapshot_is_downloaded_as_a_zstd_tar_below_a_directory_named_for_it() {
    let site = live_site().await;
    let kept = taken(&site).await;
    let response = site
        .app()
        .call(SnapshotArchiveGet, &[&kept.id])
        .token(&site.vic.token)
        .query(&SnapshotArchiveGetQuery::default())
        .send_bytes()
        .await;
    assert_eq!(response.status, StatusCode::OK);
    assert_eq!(response.headers["content-type"], "application/zstd");
    let disposition = response.headers["content-disposition"]
        .to_str()
        .unwrap()
        .to_owned();
    assert!(
        disposition.starts_with("attachment; filename=\""),
        "{disposition}"
    );
    assert!(disposition.ends_with(".tar.zst\""), "{disposition}");

    let tar_bytes = zstd::decode_all(response.raw.as_ref()).unwrap();
    let mut archive = tar::Archive::new(tar_bytes.as_slice());
    let mut found = std::collections::BTreeMap::new();
    for entry in archive.entries().unwrap() {
        let mut entry = entry.unwrap();
        let path = entry.path().unwrap().to_string_lossy().into_owned();
        let mut content = Vec::new();
        entry.read_to_end(&mut content).unwrap();
        found.insert(path, content);
    }
    let top = orch_api_core::media::safe_name(&kept.label);
    assert!(
        found.contains_key(&format!("{top}/")),
        "{:?}",
        found.keys().collect::<Vec<_>>()
    );
    assert_eq!(found[&format!("{top}/src/main.rs")], b"fn main() {}\n");
    assert_eq!(found[&format!("{top}/docs/guide.txt")], b"A guide.\n");
    assert_eq!(found[&format!("{top}/README.md")], b"# Web\n");
}

#[tokio::test]
async fn a_snapshot_may_be_downloaded_as_a_zip_that_every_desktop_opens() {
    let site = live_site().await;
    let kept = taken(&site).await;
    let response = site
        .app()
        .call(SnapshotArchiveGet, &[&kept.id])
        .token(&site.ada.token)
        .query(&SnapshotArchiveGetQuery {
            format: Some(SnapshotArchiveFormat::Zip),
        })
        .send_bytes()
        .await;
    assert_eq!(response.status, StatusCode::OK);
    assert_eq!(response.headers["content-type"], "application/zip");
    assert!(
        response.headers["content-disposition"]
            .to_str()
            .unwrap()
            .ends_with(".zip\"")
    );
    let mut zip = zip::ZipArchive::new(std::io::Cursor::new(response.raw.to_vec())).unwrap();
    let top = orch_api_core::media::safe_name(&kept.label);
    let mut content = String::new();
    zip.by_name(&format!("{top}/src/lib.rs"))
        .unwrap()
        .read_to_string(&mut content)
        .unwrap();
    assert_eq!(content, "pub fn lib() {}\n");
    let names: Vec<String> = (0..zip.len())
        .map(|at| zip.by_index(at).unwrap().name().unwrap().into_owned())
        .collect();
    assert!(names.contains(&format!("{top}/docs/")), "{names:?}");
}

#[tokio::test]
async fn an_environment_export_copies_the_entire_workspace_without_a_thread_and_stays_owner_only_after_removal()
 {
    let site = live_site().await;
    let before: i64 = sqlx::query_scalar("SELECT count(*) FROM threads")
        .fetch_one(&site.world.db.pool)
        .await
        .unwrap();
    let begun = site
        .app()
        .call(EnvironmentFilesExport, &[&site.environment])
        .token(&site.ada.token)
        .send()
        .await
        .ok();
    assert_eq!(begun.node_id, None);
    let ended = over(&site, begun.id).await;
    assert_eq!(ended.state, JobState::Succeeded, "{ended:?}");
    let id: Id = ended.result.unwrap()["snapshot_id"]
        .as_str()
        .unwrap()
        .parse()
        .unwrap();
    let kept = snapshot(&site, &site.ada, id).await.ok();
    assert_eq!(kept.thread_id, None);
    let after: i64 = sqlx::query_scalar("SELECT count(*) FROM threads")
        .fetch_one(&site.world.db.pool)
        .await
        .unwrap();
    assert_eq!(before, after, "an export creates no hidden conversation");
    // Even a colleague who reads this environment does not inherit the private export.
    assert_eq!(
        snapshot(&site, &site.vic, id).await.status,
        StatusCode::NOT_FOUND
    );
    sqlx::query("UPDATE environments SET deleted_at=$2,status='removed' WHERE id=$1")
        .bind(site.environment)
        .bind(site.world.now())
        .execute(&site.world.db.pool)
        .await
        .unwrap();
    let downloaded = site
        .app()
        .call(SnapshotArchiveGet, &[&id])
        .token(&site.ada.token)
        .query(&SnapshotArchiveGetQuery {
            format: Some(SnapshotArchiveFormat::Zip),
        })
        .send_bytes()
        .await;
    assert_eq!(downloaded.status, StatusCode::OK);
    let mut zip = zip::ZipArchive::new(std::io::Cursor::new(downloaded.raw.to_vec())).unwrap();
    let top = orch_api_core::media::safe_name(&kept.label);
    for (path, expected) in [
        ("web/src/main.rs", "fn main() {}\n"),
        ("other/elsewhere.txt", "not in the thread's directory\n"),
    ] {
        let mut content = String::new();
        zip.by_name(&format!("{top}/{path}"))
            .unwrap()
            .read_to_string(&mut content)
            .unwrap();
        assert_eq!(content, expected);
    }
    let denied = site
        .app()
        .call(SnapshotArchiveGet, &[&id])
        .token(&site.eve.token)
        .query(&SnapshotArchiveGetQuery::default())
        .send_bytes()
        .await;
    assert_eq!(denied.status, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn a_reader_cannot_export_an_environment_or_leave_a_snapshot_job_behind() {
    let site = live_site().await;
    let before: i64 = sqlx::query_scalar("SELECT count(*) FROM snapshots")
        .fetch_one(&site.world.db.pool)
        .await
        .unwrap();
    let refused = site
        .app()
        .call(EnvironmentFilesExport, &[&site.environment])
        .token(&site.vic.token)
        .send()
        .await
        .problem();
    assert_eq!(refused.code, ErrorCode::AuthzDenied);
    let after: i64 = sqlx::query_scalar("SELECT count(*) FROM snapshots")
        .fetch_one(&site.world.db.pool)
        .await
        .unwrap();
    assert_eq!(before, after);
    assert_eq!(site.sent(CommandKind::FsSnapshot), 0);
}

#[tokio::test]
async fn a_partial_snapshot_downloads_the_files_it_holds_and_not_those_it_could_not_take() {
    let site = live_site().await;
    site.disk.make_unreadable("web/README.md");
    let kept = taken(&site).await;
    let response = site
        .app()
        .call(SnapshotArchiveGet, &[&kept.id])
        .token(&site.ada.token)
        .query(&SnapshotArchiveGetQuery::default())
        .send_bytes()
        .await;
    let tar_bytes = zstd::decode_all(response.raw.as_ref()).unwrap();
    let mut archive = tar::Archive::new(tar_bytes.as_slice());
    let names: Vec<String> = archive
        .entries()
        .unwrap()
        .map(|entry| {
            entry
                .unwrap()
                .path()
                .unwrap()
                .to_string_lossy()
                .into_owned()
        })
        .collect();
    assert!(names.iter().any(|name| name.ends_with("src/main.rs")));
    assert!(
        !names.iter().any(|name| name.ends_with("README.md")),
        "{names:?}"
    );
}

// ---------------------------------------------------------------- renaming and deleting

#[tokio::test]
async fn a_snapshot_is_renamed_by_whoever_may_write_the_files() {
    let site = live_site().await;
    let kept = taken(&site).await;
    let rename = |by: &Who, label: &str| {
        let call = site
            .app()
            .call(SnapshotUpdate, &[&kept.id])
            .token(&by.token)
            .body(&SnapshotUpdateBody {
                label: label.to_owned(),
            });
        async move { call.send().await }
    };
    let renamed = rename(&site.ada, "  Before the move  ").await.ok();
    assert_eq!(renamed.label, "Before the move");
    assert_eq!(
        rename(&site.ada, "   ").await.problem().code,
        ErrorCode::RequestInvalid
    );
    assert_eq!(
        rename(&site.vic, "mine").await.problem().code,
        ErrorCode::AuthzDenied
    );
    assert_eq!(
        snapshot(&site, &site.ada, kept.id).await.ok().label,
        "Before the move"
    );
}

#[tokio::test]
async fn deleting_a_snapshot_lets_go_of_the_files_it_held() {
    let site = live_site().await;
    let kept = taken(&site).await;
    let mut conn = site.world.conn().await;
    let manifest = storage::snapshot(&mut conn, kept.id)
        .await
        .unwrap()
        .unwrap()
        .manifest_blob
        .unwrap();
    let main = Hash::of(b"fn main() {}\n");
    assert_eq!(
        storage::blob(&mut conn, &main)
            .await
            .unwrap()
            .unwrap()
            .pin_count,
        1
    );
    drop(conn);

    let delete = |by: &Who| {
        let call = site
            .app()
            .call(SnapshotDelete, &[&kept.id])
            .token(&by.token);
        async move { call.send().await }
    };
    assert_eq!(
        delete(&site.vic).await.problem().code,
        ErrorCode::AuthzDenied
    );
    assert!(
        snapshot(&site, &site.ada, kept.id)
            .await
            .status
            .is_success()
    );
    let gone = delete(&site.ada).await;
    assert_eq!(gone.status, StatusCode::NO_CONTENT);
    assert_eq!(
        snapshot(&site, &site.ada, kept.id).await.problem().code,
        ErrorCode::NotFound
    );

    let mut conn = site.world.conn().await;
    assert_eq!(
        storage::blob(&mut conn, &main)
            .await
            .unwrap()
            .unwrap()
            .pin_count,
        0
    );
    assert_eq!(
        storage::blob(&mut conn, &manifest)
            .await
            .unwrap()
            .unwrap()
            .pin_count,
        0
    );
    drop(conn);
    // The content is cache again and goes after its grace.
    site.world.advance(Duration::hours(25));
    let collected = orch_api_core::cache::collect_garbage(
        &site.app().app.db,
        &site.app().app.blobs,
        site.app().app.clock.as_ref(),
        &site.app().app.config,
    )
    .await
    .unwrap();
    assert!(collected.blobs >= 4, "{collected:?}");
    assert!(
        site.app()
            .app
            .blobs
            .store()
            .size(&main)
            .await
            .unwrap()
            .is_none()
    );
}

#[tokio::test]
async fn a_snapshot_that_is_being_taken_is_not_deleted_before_its_job_is_cancelled() {
    let site = live_site().await;
    let gate = Arc::new(tokio::sync::Notify::new());
    *site.disk.gate.lock().unwrap() = Some(Arc::clone(&gate));
    let begun = create(&site, &site.ada, ThreadSnapshotCreateBody::default())
        .await
        .ok();
    let snapshots = site
        .app()
        .call(ThreadSnapshotList, &[&site.thread])
        .token(&site.ada.token)
        .query(&PageQuery::default())
        .send()
        .await
        .ok();
    assert_eq!(snapshots.items.len(), 1);
    assert_eq!(snapshots.items[0].state, SnapshotState::Running);
    let refused = site
        .app()
        .call(SnapshotDelete, &[&snapshots.items[0].id])
        .token(&site.ada.token)
        .send()
        .await
        .problem();
    assert_eq!(refused.code, ErrorCode::ResourceInUse);
    gate.notify_one();
    over(&site, begun.id).await;
}

// ---------------------------------------------------------------- the job

#[tokio::test]
async fn a_job_is_read_by_whoever_may_see_the_thread_it_is_about() {
    let site = live_site().await;
    let begun = create(&site, &site.ada, ThreadSnapshotCreateBody::default())
        .await
        .ok();
    assert!(job(&site, &site.vic, begun.id).await.status.is_success());
    assert!(job(&site, &site.dev, begun.id).await.status.is_success());
    let stranger = job(&site, &site.eve, begun.id).await;
    assert!(!stranger.status.is_success());
    assert_eq!(
        job(&site, &site.ada, new_id()).await.problem().code,
        ErrorCode::NotFound
    );
    over(&site, begun.id).await;
}

#[tokio::test]
async fn a_job_under_way_is_cancelled_and_the_snapshot_it_was_taking_fails() {
    let site = live_site().await;
    let gate = Arc::new(tokio::sync::Notify::new());
    *site.disk.gate.lock().unwrap() = Some(Arc::clone(&gate));
    let begun = create(&site, &site.ada, ThreadSnapshotCreateBody::default())
        .await
        .ok();
    eventually("the machine to take the snapshot up", || async {
        site.sent(CommandKind::FsSnapshot) == 1
    })
    .await;

    let refused = site
        .app()
        .call(JobCancel, &[&begun.id])
        .token(&site.vic.token)
        .send()
        .await
        .problem();
    assert_eq!(
        refused.code,
        ErrorCode::AuthzDenied,
        "whoever could have started it may cancel it"
    );
    let asked = site
        .app()
        .call(JobCancel, &[&begun.id])
        .token(&site.ada.token)
        .send()
        .await;
    assert!(asked.status.is_success(), "{:?}", asked.body);
    let stopping = asked.ok();
    assert_eq!(stopping.state, JobState::Running);
    assert!(stopping.progress.activity.unwrap().stopping);
    // The machine still owns the held command: cancellation is a visible request until it answers.
    assert_eq!(
        job(&site, &site.ada, begun.id).await.ok().state,
        JobState::Running
    );
    let command = site
        .played
        .as_ref()
        .unwrap()
        .seen_of(CommandKind::FsSnapshot)[0]
        .id;
    eventually(
        "the machine to receive the stop before its held work is released",
        || async { site.disk.saw_stop(command) },
    )
    .await;
    gate.notify_one();
    let ended = over(&site, begun.id).await;
    assert_eq!(ended.state, JobState::Cancelled);
    let snapshots = site
        .app()
        .call(ThreadSnapshotList, &[&site.thread])
        .token(&site.ada.token)
        .query(&PageQuery::default())
        .send()
        .await
        .ok();
    assert_eq!(snapshots.items[0].state, SnapshotState::Failed);
    // The original command's result cannot make the cancelled job succeed later.
    gate.notify_one();
    tokio::time::sleep(std::time::Duration::from_millis(300)).await;
    assert_eq!(
        job(&site, &site.ada, begun.id).await.ok().state,
        JobState::Cancelled
    );
    // A job that is over is shown as it is, and nothing is asked of anybody.
    let late = site
        .app()
        .call(JobCancel, &[&begun.id])
        .token(&site.ada.token)
        .send()
        .await
        .ok();
    assert_eq!(late.state, JobState::Cancelled);
}

#[tokio::test]
async fn a_job_that_was_asked_to_be_cancelled_before_it_began_never_does() {
    // The engine is not running here, so nothing takes the job up until it is started.
    let mut site = Site::new().await;
    site.disk.put("web/a.txt", b"a");
    site.power_on();
    let begun = create(&site, &site.ada, ThreadSnapshotCreateBody::default())
        .await
        .ok();
    assert_eq!(begun.state, JobState::Queued);
    let asked = site
        .app()
        .call(JobCancel, &[&begun.id])
        .token(&site.ada.token)
        .send()
        .await;
    assert!(asked.status.is_success(), "{:?}", asked.body);
    site.app().app.engine.start();
    let ended = over(&site, begun.id).await;
    assert_eq!(ended.state, JobState::Cancelled);
    let snapshots = site
        .app()
        .call(ThreadSnapshotList, &[&site.thread])
        .token(&site.ada.token)
        .query(&PageQuery::default())
        .send()
        .await
        .ok();
    assert_eq!(snapshots.items[0].state, SnapshotState::Failed);
    assert_eq!(site.sent(CommandKind::FsSnapshot), 0);
}

#[tokio::test]
async fn a_snapshot_whose_machine_is_gone_when_the_job_runs_fails_with_why() {
    let site = live_site().await;
    sqlx::query("UPDATE environments SET machine_id = NULL WHERE id = $1")
        .bind(site.environment)
        .execute(&site.world.db.pool)
        .await
        .unwrap();
    // The machine was there when the snapshot was asked for; the environment lost it since.
    let mut conn = site.world.conn().await;
    let place = orch_db::queries::environments::workspace::thread_place(&mut conn, site.thread)
        .await
        .unwrap()
        .unwrap();
    drop(conn);
    let begun = site
        .app()
        .app
        .engine
        .begin_snapshot(
            &mut *site.world.conn().await,
            &orch_engine::jobs::snapshot::Asked {
                thread: Some(site.thread),
                environment: site.environment,
                generation: 1,
                label: "lost".to_owned(),
                include: vec![place.workspace_dir],
                exclude: Vec::new(),
                created_by: Some(site.ada.id()),
            },
        )
        .await
        .unwrap();
    let ended = over(&site, begun.job.id).await;
    assert_eq!(ended.state, JobState::Failed);
    let error = ended.error.unwrap();
    assert_eq!(
        error.code,
        orch_engine::jobs::snapshot::ENVIRONMENT_UNAVAILABLE
    );
    assert_eq!(error.phase, "scan");
    assert!(error.undone);
    assert_eq!(
        snapshot(&site, &site.ada, begun.snapshot).await.ok().state,
        SnapshotState::Failed
    );
}
