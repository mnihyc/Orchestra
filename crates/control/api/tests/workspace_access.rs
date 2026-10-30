//! Every request that reaches an environment's workspace is refused to whoever does not hold what it needs: a person who is signed in as nobody gets no answer, a stranger is let into nothing, a viewer reads and writes nothing, and nothing that changes the machine reaches it on their account.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]

mod places;
mod support;
mod workspace;

use orch_api_contract::processes::{
    EnvironmentProcessList, EnvironmentProcessSignal, EnvironmentResourceList,
};
use orch_api_contract::routes::Endpoint;
use orch_api_contract::threads::{ThreadAttachmentCreate, ThreadBlobGet};
use orch_api_contract::workspace::{
    EnvironmentArchiveEntryGet, EnvironmentArchiveList, EnvironmentChangeList,
    EnvironmentFileContentGet, EnvironmentFileContentPut, EnvironmentFileCopy,
    EnvironmentFileDelete, EnvironmentFileFetch, EnvironmentFileList, EnvironmentFileMkdir,
    EnvironmentFileRename, EnvironmentFileSearch, EnvironmentFileStat, EnvironmentGitDiffGet,
    EnvironmentGitStatusGet, JobCancel, JobGet, SnapshotArchiveGet, SnapshotDelete,
    SnapshotFileContentGet, SnapshotFileList, SnapshotGet, SnapshotUpdate, ThreadSnapshotCreate,
    ThreadSnapshotList,
};
use orch_db::queries::storage::{self, NewSnapshot, SnapshotEnd, snapshot_state};
use orch_engine::jobs::snapshot::Asked;
use orch_link_contract::commands::CommandKind;
use orch_types::{Hash, Id, new_id};
use places::Who;
use serde_json::{Value, json};
use std::fmt::Display;
use workspace::Site;

/// The status a request got as `who`, or as nobody.
async fn status<E: Endpoint>(
    site: &Site,
    who: Option<&Who>,
    endpoint: E,
    params: &[&dyn Display],
    query: &str,
    body: Option<Value>,
) -> u16 {
    let mut call = site.app().call(endpoint, params).raw_query(query);
    if let Some(who) = who {
        call = call.token(&who.token);
    }
    if let Some(body) = body {
        call = call.raw_body(&body);
    }
    call.send_bytes().await.status.as_u16()
}

/// The requests of a workspace, each as a call that is made by `who`: its name, whether it changes
/// something, and the status it got.
async fn all_as(
    site: &Site,
    who: Option<&Who>,
    snapshot: Id,
    job: Id,
) -> Vec<(&'static str, bool, u16)> {
    let (env, thread) = (site.environment, site.thread);
    let through = format!("thread={thread}");
    let on = |extra: &str| format!("thread={thread}&{extra}");
    let hash = Hash::of(b"x");
    let started = "2026-10-08T17:04:05.123Z";
    let mut got = Vec::new();
    macro_rules! ask {
        ($name:literal, $writes:expr, $endpoint:expr, [$($param:expr),*], $query:expr, $body:expr) => {
            got.push((
                $name,
                $writes,
                status(site, who, $endpoint, &[$(&$param as &dyn Display),*], $query, $body).await,
            ));
        };
    }
    ask!(
        "EnvironmentFileList",
        false,
        EnvironmentFileList,
        [env],
        &on("path=web"),
        None
    );
    ask!(
        "EnvironmentFileStat",
        false,
        EnvironmentFileStat,
        [env],
        &on("path=web/a.txt"),
        None
    );
    ask!(
        "EnvironmentFileFetch",
        false,
        EnvironmentFileFetch,
        [env],
        &through,
        Some(json!({ "path": "web/a.txt" }))
    );
    ask!(
        "EnvironmentFileContentGet",
        false,
        EnvironmentFileContentGet,
        [env],
        &on(&format!("path=web/a.txt&hash={hash}")),
        None
    );
    ask!(
        "EnvironmentFileContentPut",
        true,
        EnvironmentFileContentPut,
        [env],
        &on("path=web/new.txt"),
        Some(json!("content"))
    );
    ask!(
        "EnvironmentFileMkdir",
        true,
        EnvironmentFileMkdir,
        [env],
        &through,
        Some(json!({ "path": "web/d" }))
    );
    ask!(
        "EnvironmentFileRename",
        true,
        EnvironmentFileRename,
        [env],
        &through,
        Some(json!({ "from": "web/a.txt", "to": "web/b.txt" }))
    );
    ask!(
        "EnvironmentFileCopy",
        true,
        EnvironmentFileCopy,
        [env],
        &through,
        Some(json!({ "from": "web/a.txt", "to": "web/c.txt" }))
    );
    ask!(
        "EnvironmentFileDelete",
        true,
        EnvironmentFileDelete,
        [env],
        &on("path=web/a.txt"),
        None
    );
    ask!(
        "EnvironmentChangeList",
        false,
        EnvironmentChangeList,
        [env],
        &through,
        None
    );
    ask!(
        "EnvironmentFileSearch",
        false,
        EnvironmentFileSearch,
        [env],
        &on("q=main&in=content"),
        None
    );
    ask!(
        "EnvironmentArchiveList",
        false,
        EnvironmentArchiveList,
        [env],
        &on("path=web/p.zip"),
        None
    );
    ask!(
        "EnvironmentArchiveEntryGet",
        false,
        EnvironmentArchiveEntryGet,
        [env],
        &on("path=web/p.zip&entry=a"),
        None
    );
    ask!(
        "EnvironmentGitStatusGet",
        false,
        EnvironmentGitStatusGet,
        [env],
        &through,
        None
    );
    ask!(
        "EnvironmentGitDiffGet",
        false,
        EnvironmentGitDiffGet,
        [env],
        &through,
        None
    );
    ask!(
        "EnvironmentProcessList",
        false,
        EnvironmentProcessList,
        [env],
        &through,
        None
    );
    ask!(
        "EnvironmentProcessSignal",
        true,
        EnvironmentProcessSignal,
        [env, 41_u32],
        &through,
        Some(json!({ "signal": "TERM", "started_at": started }))
    );
    ask!(
        "EnvironmentResourceList",
        false,
        EnvironmentResourceList,
        [env],
        &through,
        None
    );
    ask!(
        "ThreadSnapshotList",
        false,
        ThreadSnapshotList,
        [thread],
        "",
        None
    );
    ask!(
        "ThreadSnapshotCreate",
        true,
        ThreadSnapshotCreate,
        [thread],
        "",
        Some(json!({}))
    );
    ask!("SnapshotGet", false, SnapshotGet, [snapshot], "", None);
    ask!(
        "SnapshotFileList",
        false,
        SnapshotFileList,
        [snapshot],
        "path=",
        None
    );
    ask!(
        "SnapshotFileContentGet",
        false,
        SnapshotFileContentGet,
        [snapshot],
        "path=a",
        None
    );
    ask!(
        "SnapshotArchiveGet",
        false,
        SnapshotArchiveGet,
        [snapshot],
        "",
        None
    );
    ask!(
        "SnapshotUpdate",
        true,
        SnapshotUpdate,
        [snapshot],
        "",
        Some(json!({ "label": "x" }))
    );
    ask!("SnapshotDelete", true, SnapshotDelete, [snapshot], "", None);
    ask!("JobGet", false, JobGet, [job], "", None);
    ask!("JobCancel", true, JobCancel, [job], "", None);
    ask!(
        "ThreadAttachmentCreate",
        true,
        ThreadAttachmentCreate,
        [thread],
        "",
        Some(json!({ "blob": hash, "name": "x" }))
    );
    ask!(
        "ThreadBlobGet",
        false,
        ThreadBlobGet,
        [thread, hash],
        "",
        None
    );
    got
}

/// A snapshot that was taken, and a job that waits, to ask for.
async fn something_to_ask_for(site: &Site) -> (Id, Id) {
    let mut conn = site.world.conn().await;
    let snapshot = new_id();
    storage::begin_snapshot(
        &mut conn,
        &NewSnapshot {
            id: snapshot,
            thread_id: Some(site.thread),
            environment_id: site.environment,
            env_generation: 1,
            label: "kept",
            selection: &json!({ "include": ["web"], "exclude": [] }),
            created_by: Some(site.ada.id()),
            at: site.world.now(),
        },
    )
    .await
    .unwrap();
    storage::end_snapshot(
        &mut conn,
        snapshot,
        &SnapshotEnd {
            state: snapshot_state::COMPLETE,
            file_count: 0,
            total_bytes: 0,
            missing_count: 0,
            manifest_blob: None,
            at: site.world.now(),
        },
    )
    .await
    .unwrap();
    // The engine is not running, so the job stays where it was made.
    let begun = site
        .app()
        .app
        .engine
        .begin_snapshot(
            &mut conn,
            &Asked {
                thread: Some(site.thread),
                environment: site.environment,
                generation: 1,
                label: "waiting".to_owned(),
                include: vec!["web".to_owned()],
                exclude: Vec::new(),
                created_by: Some(site.ada.id()),
            },
        )
        .await
        .unwrap();
    (snapshot, begun.job.id)
}

fn writes_reached_the_machine(site: &Site) -> usize {
    [
        CommandKind::FsWrite,
        CommandKind::FsMkdir,
        CommandKind::FsRename,
        CommandKind::FsCopy,
        CommandKind::FsRemove,
        CommandKind::FsSnapshot,
        CommandKind::ProcSignal,
    ]
    .into_iter()
    .map(|kind| site.sent(kind))
    .sum()
}

#[tokio::test]
async fn nobody_who_is_not_signed_in_is_answered_by_any_request_of_the_workspace() {
    let mut site = Site::new().await;
    site.power_on();
    let (snapshot, job) = something_to_ask_for(&site).await;
    let got = all_as(&site, None, snapshot, job).await;
    assert_eq!(got.len(), 30);
    for (name, _, status) in got {
        assert_eq!(status, 401, "{name}");
    }
    assert_eq!(
        site.sent(CommandKind::FsList) + writes_reached_the_machine(&site),
        0
    );
}

#[tokio::test]
async fn a_stranger_is_let_into_nothing_of_the_workspace() {
    let mut site = Site::new().await;
    site.disk.put("web/a.txt", b"a");
    site.power_on();
    let (snapshot, job) = something_to_ask_for(&site).await;
    let got = all_as(&site, Some(&site.eve), snapshot, job).await;
    for (name, _, status) in got {
        assert!(matches!(status, 403 | 404), "{name}: {status}");
    }
    assert_eq!(
        site.played.as_ref().unwrap().seen().len(),
        0,
        "the machine was asked nothing on a stranger's account"
    );
}

#[tokio::test]
async fn a_viewer_reads_what_the_workspace_holds_and_changes_none_of_it() {
    let mut site = Site::new().await;
    site.disk.put("web/a.txt", b"a");
    site.power_on();
    let (snapshot, job) = something_to_ask_for(&site).await;
    let got = all_as(&site, Some(&site.vic), snapshot, job).await;
    for (name, writes, status) in &got {
        if *writes {
            assert_eq!(*status, 403, "{name} changes something");
        } else {
            assert!(!matches!(status, 401 | 403), "{name} only reads: {status}");
        }
    }
    assert_eq!(writes_reached_the_machine(&site), 0);
    assert!(site.disk.exists("web/a.txt") && !site.disk.exists("web/new.txt"));
}

#[tokio::test]
async fn the_owner_of_the_environment_is_refused_none_of_them() {
    let mut site = Site::new().await;
    site.disk.put("web/a.txt", b"a");
    site.power_on();
    let (snapshot, job) = something_to_ask_for(&site).await;
    let got = all_as(&site, Some(&site.ada), snapshot, job).await;
    for (name, _, status) in got {
        assert!(!matches!(status, 401 | 403), "{name}: {status}");
    }
}
