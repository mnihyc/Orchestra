//! The files of an environment's workspace over HTTP: listed, described, brought to the store and read in ranges, written on the version the writer saw, made, renamed, copied and deleted; each as a command to the machine after the caller's right was checked, and answered from what was kept while the machine is away.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]

mod places;
mod support;
mod workspace;

use async_trait::async_trait;
use chrono::Duration;
use http::StatusCode;
use orch_api_contract::common::ErrorCode;
use orch_api_contract::environment::ThroughThreadQuery;
use orch_api_contract::forks::FilesSource;
use orch_api_contract::nodes::JobState;
use orch_api_contract::workspace::{
    DirEntryKind, EnvironmentFileList, EnvironmentFileListQuery, EnvironmentUploadWrite,
    EnvironmentUploadWriteBody, EnvironmentUploadWriteResult, FetchStatus, FetchUnavailable,
    FileListSource, Job, JobCancel, JobGet,
};
use orch_api_core::testing::Machine;
use orch_link_contract::commands::{CommandError, CommandKind, ErrorCode as MachineCode};
use orch_types::{Hash, Id, Json, new_id};
use serde_json::json;
use std::sync::Arc;
use workspace::{Site, ask, eventually};

/// Bytes that do not repeat inside a piece, so that a piece in the wrong place is noticed.
fn bytes_of(len: usize) -> Vec<u8> {
    (0..len)
        .map(|at| u8::try_from((at * 37 + at / 199) % 256).unwrap())
        .collect()
}

async fn browser_upload(site: &Site, bytes: &[u8]) -> Hash {
    use orch_api_contract::threads::{
        UploadSessionComplete, UploadSessionCreate, UploadSessionCreateBody, UploadSessionPut,
        UploadSessionPutQuery,
    };
    let upload = site
        .app()
        .call(UploadSessionCreate, &[])
        .token(&site.ada.token)
        .body(&UploadSessionCreateBody {
            name: "upload.txt".to_owned(),
            size: bytes.len() as i64,
            media_type: None,
        })
        .send()
        .await
        .ok();
    let mut offset = 0;
    for chunk in bytes.chunks(upload.chunk_bytes as usize) {
        let received = site
            .app()
            .call(UploadSessionPut, &[&upload.upload_id])
            .token(&site.ada.token)
            .query(&UploadSessionPutQuery { offset })
            .bytes(chunk)
            .send()
            .await
            .ok();
        offset = received.received;
    }
    site.app()
        .call(UploadSessionComplete, &[&upload.upload_id])
        .token(&site.ada.token)
        .send()
        .await
        .ok()
        .blob
}

struct HeldUpload {
    disk: Arc<workspace::Disk>,
    entered: tokio::sync::Semaphore,
    command: std::sync::Mutex<Option<Id>>,
    cancelled: tokio_util::sync::CancellationToken,
    release: tokio_util::sync::CancellationToken,
}

#[async_trait]
impl Machine for HeldUpload {
    fn cancel(&self, command: Id) {
        self.disk.cancel(command);
        if *self.command.lock().unwrap() == Some(command) {
            self.cancelled.cancel();
        }
    }

    async fn run(&self, command: &orch_link_contract::commands::Cmd) -> Result<Json, CommandError> {
        if command.kind != CommandKind::FsWrite.as_str() {
            return self.disk.run(command).await;
        }
        *self.command.lock().unwrap() = Some(command.id);
        self.entered.add_permits(1);
        self.release.cancelled().await;
        if self.cancelled.is_cancelled() {
            Err(CommandError::new(
                MachineCode::Conflict,
                "the fixture's writer stopped and cleaned up",
            ))
        } else {
            self.disk.run(command).await
        }
    }
}

async fn upload_job(site: &Site, id: Id) -> Job {
    site.app()
        .call(JobGet, &[&id])
        .token(&site.ada.token)
        .send()
        .await
        .ok()
}

async fn upload_pins(site: &Site, blob: Hash) -> i32 {
    orch_db::queries::storage::blob(&mut *site.world.conn().await, &blob)
        .await
        .unwrap()
        .unwrap()
        .pin_count
}

#[tokio::test]
async fn cancelling_a_dispatched_upload_keeps_its_command_and_content_until_the_machine_has_returned()
 {
    cancelled_upload(false).await;
}

#[tokio::test]
async fn a_controller_failure_during_an_upload_is_not_a_native_stop_receipt() {
    cancelled_upload(true).await;
}

async fn cancelled_upload(controller_failed: bool) {
    let mut site = live_site().await;
    let machine = Arc::new(HeldUpload {
        disk: site.disk.clone(),
        entered: tokio::sync::Semaphore::new(0),
        command: std::sync::Mutex::new(None),
        cancelled: Default::default(),
        release: Default::default(),
    });
    site.power_on_with(machine.clone());
    let bytes = b"unfinished native copy";
    let blob = browser_upload(&site, bytes).await;
    let before = upload_pins(&site, blob).await;
    site.app().app.engine.start();
    let id = new_id();
    site.app()
        .call(EnvironmentUploadWrite, &[&site.environment, &id])
        .token(&site.ada.token)
        .query(&ThroughThreadQuery::default())
        .body(&EnvironmentUploadWriteBody {
            path: "web/pending.txt".into(),
            blob,
            expected_hash: Some("none".into()),
            upload_batch: new_id(),
        })
        .send()
        .await
        .ok();
    tokio::time::timeout(workspace::PATIENCE, machine.entered.acquire())
        .await
        .unwrap()
        .unwrap()
        .forget();
    let command = machine.command.lock().unwrap().unwrap();
    // The last bytes can be reported before the handler's final version check and result.
    // That observation cannot fill the job's bar or release its content ownership.
    let mut tx = site.world.app.app.db.begin().await.unwrap();
    site.world
        .app
        .app
        .engine
        .apply_stream_events(
            &mut tx,
            site.machine,
            &orch_link_contract::streams::Stream::Env {
                environment_id: site.environment,
            },
            &[orch_link::streams::DeliveredEvent {
                seq: 1,
                at: orch_types::Time::new(site.world.now()),
                kind: "job.progress".into(),
                body: json!({"command_id":command,"job_id":id,"log":null,
                    "progress":{"phase":"files","phases":[],"percent":100,
                    "files_done":1,"files_total":1,"bytes_done":bytes.len(),"bytes_total":bytes.len(),
                    "rate_bps":null,"eta_seconds":null,"message":null}}),
            }],
        )
        .await
        .unwrap();
    tx.commit().await.unwrap();
    let copying = upload_job(&site, id).await;
    assert_eq!(copying.state, JobState::Running);
    assert_eq!(copying.progress.percent, 99);
    assert_eq!(copying.progress.files_done, 1);
    assert_eq!(upload_pins(&site, blob).await, before + 1);
    let first_error = json!({
        "code":"unacknowledged",
        "detail":"the controller has no native acknowledgement",
        "retryable":true,
        "details":null
    });
    let terminal = if controller_failed {
        sqlx::query("UPDATE commands SET state='failed',error=$2 WHERE id=$1")
            .bind(command)
            .bind(&first_error)
            .execute(&mut *site.world.conn().await)
            .await
            .unwrap();
        eventually(
            "the controller failure is retained for native cleanup",
            || async {
                orch_db::queries::jobs::job(&mut *site.world.conn().await, id)
                    .await
                    .unwrap()
                    .unwrap()
                    .plan
                    .get("abort")
                    .is_some_and(|reason| !reason.is_null())
            },
        )
        .await;
        JobState::Failed
    } else {
        let stopping = site
            .app()
            .call(JobCancel, &[&id])
            .token(&site.ada.token)
            .send()
            .await
            .ok();
        assert_eq!(stopping.state, JobState::Running);
        assert!(stopping.progress.activity.unwrap().stopping);
        JobState::Cancelled
    };
    tokio::time::timeout(workspace::PATIENCE, machine.cancelled.cancelled())
        .await
        .unwrap();
    // The actual stop delivery is the barrier; two ordinary runner polls still cannot finish cleanup.
    for _ in 0..2 {
        tokio::time::sleep(orch_engine::hosting::WATCH_EVERY).await;
        assert_eq!(upload_job(&site, id).await.state, JobState::Running);
        let mut conn = site.world.conn().await;
        let row = orch_db::queries::jobs::job(&mut conn, id)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(row.plan["active_command"]["id"], json!(command));
        let acknowledged: bool =
            sqlx::query_scalar("SELECT stop_confirmed_at IS NOT NULL FROM commands WHERE id=$1")
                .bind(command)
                .fetch_one(&mut *conn)
                .await
                .unwrap();
        assert!(!acknowledged);
        drop(conn);
        assert_eq!(upload_pins(&site, blob).await, before + 1);
    }
    machine.release.cancel();
    eventually("the upload's actual stopped acknowledgement", || async {
        upload_job(&site, id).await.state == terminal
    })
    .await;
    assert_eq!(upload_pins(&site, blob).await, before);
    let mut conn = site.world.conn().await;
    let acknowledged: bool =
        sqlx::query_scalar("SELECT stop_confirmed_at IS NOT NULL FROM commands WHERE id=$1")
            .bind(command)
            .fetch_one(&mut *conn)
            .await
            .unwrap();
    assert!(acknowledged);
    if controller_failed {
        let row = orch_db::queries::link::command(&mut conn, command)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(
            row.state, "failed",
            "the controller's first outcome stays unchanged"
        );
        assert_eq!(row.error, Some(first_error));
    }
    assert!(
        !site
            .disk
            .paths()
            .iter()
            .any(|path| path == "web/pending.txt")
    );
    site.app().app.engine.stop();
}

#[tokio::test]
async fn a_completed_upload_write_replays_one_result_and_one_audit_without_repinning_or_writing_again()
 {
    let site = live_site().await;
    let blob = browser_upload(&site, b"confirmed native file").await;
    let before = upload_pins(&site, blob).await;
    let id = new_id();
    let body = EnvironmentUploadWriteBody {
        path: "web/completed.txt".into(),
        blob,
        expected_hash: Some("none".into()),
        upload_batch: new_id(),
    };
    site.app().app.engine.start();
    site.app()
        .call(EnvironmentUploadWrite, &[&site.environment, &id])
        .token(&site.ada.token)
        .query(&ThroughThreadQuery::default())
        .body(&body)
        .send()
        .await
        .ok();
    eventually("the confirmed upload result", || async {
        upload_job(&site, id).await.state == JobState::Succeeded
    })
    .await;
    let stat = site
        .app()
        .call(EnvironmentUploadWriteResult, &[&site.environment, &id])
        .token(&site.ada.token)
        .query(&ThroughThreadQuery::default())
        .send()
        .await
        .ok();
    assert_eq!(stat.entry.path, body.path);
    assert_eq!(stat.hash, Some(blob));
    assert_eq!(upload_pins(&site, blob).await, before);
    let replay = site
        .app()
        .call(EnvironmentUploadWrite, &[&site.environment, &id])
        .token(&site.ada.token)
        .query(&ThroughThreadQuery::default())
        .body(&body)
        .send()
        .await
        .ok();
    assert_eq!(replay.id, id);
    assert_eq!(replay.state, JobState::Succeeded);
    assert_eq!(site.sent(CommandKind::FsWrite), 1);
    assert_eq!(upload_pins(&site, blob).await, before);
    let mut conn = site.world.conn().await;
    let count: i64 = sqlx::query_scalar("SELECT count(*) FROM audit_log WHERE environment_id=$1 AND action='workspace.file_written' AND details->>'path'=$2")
        .bind(site.environment).bind(&body.path).fetch_one(&mut *conn).await.unwrap();
    assert_eq!(count, 1);
    drop(conn);
    let changed = EnvironmentUploadWriteBody {
        path: "web/different.txt".into(),
        ..body
    };
    assert_eq!(
        site.app()
            .call(EnvironmentUploadWrite, &[&site.environment, &id])
            .token(&site.ada.token)
            .query(&ThroughThreadQuery::default())
            .body(&changed)
            .send()
            .await
            .status,
        StatusCode::CONFLICT
    );
    assert_eq!(site.sent(CommandKind::FsWrite), 1);
    site.app().app.engine.stop();
}

struct CompletedUpload {
    disk: Arc<workspace::Disk>,
    entered: tokio::sync::Semaphore,
    cancelled: tokio_util::sync::CancellationToken,
    release: tokio_util::sync::CancellationToken,
}

#[async_trait]
impl Machine for CompletedUpload {
    fn cancel(&self, command: Id) {
        self.disk.cancel(command);
        self.cancelled.cancel();
    }

    async fn run(&self, command: &orch_link_contract::commands::Cmd) -> Result<Json, CommandError> {
        let written = self.disk.run(command).await?;
        if command.kind == CommandKind::FsWrite.as_str() {
            // The native operation has really completed; only its response is held.
            self.entered.add_permits(1);
            self.release.cancelled().await;
        }
        Ok(written)
    }
}

#[tokio::test]
async fn a_native_write_completed_before_cancel_keeps_its_receipt_even_when_its_reply_arrives_after_cancel()
 {
    let mut site = live_site().await;
    let machine = Arc::new(CompletedUpload {
        disk: site.disk.clone(),
        entered: tokio::sync::Semaphore::new(0),
        cancelled: Default::default(),
        release: Default::default(),
    });
    site.power_on_with(machine.clone());
    let blob = browser_upload(&site, b"the completed native file survives cancellation").await;
    let before = upload_pins(&site, blob).await;
    let id = new_id();
    site.app().app.engine.start();
    site.app()
        .call(EnvironmentUploadWrite, &[&site.environment, &id])
        .token(&site.ada.token)
        .query(&ThroughThreadQuery::default())
        .body(&EnvironmentUploadWriteBody {
            path: "web/written-before-cancel.txt".into(),
            blob,
            expected_hash: Some("none".into()),
            upload_batch: new_id(),
        })
        .send()
        .await
        .ok();
    tokio::time::timeout(workspace::PATIENCE, machine.entered.acquire())
        .await
        .unwrap()
        .unwrap()
        .forget();
    assert!(
        site.disk
            .paths()
            .iter()
            .any(|path| path == "web/written-before-cancel.txt")
    );
    let stopping = site
        .app()
        .call(JobCancel, &[&id])
        .token(&site.ada.token)
        .send()
        .await
        .ok();
    assert_eq!(stopping.state, JobState::Running);
    assert!(stopping.progress.activity.unwrap().stopping);
    tokio::time::timeout(workspace::PATIENCE, machine.cancelled.cancelled())
        .await
        .unwrap();
    assert_eq!(upload_job(&site, id).await.state, JobState::Running);
    assert_eq!(upload_pins(&site, blob).await, before + 1);
    machine.release.cancel();
    eventually("the completed file's late native receipt", || async {
        upload_job(&site, id).await.state == JobState::Cancelled
    })
    .await;
    let stat = site
        .app()
        .call(EnvironmentUploadWriteResult, &[&site.environment, &id])
        .token(&site.ada.token)
        .query(&ThroughThreadQuery::default())
        .send()
        .await
        .ok();
    assert_eq!(stat.hash, Some(blob));
    assert_eq!(stat.entry.path, "web/written-before-cancel.txt");
    assert_eq!(upload_pins(&site, blob).await, before);
    assert_eq!(site.sent(CommandKind::FsWrite), 1);
    let mut conn = site.world.conn().await;
    let saved: i64 = sqlx::query_scalar("SELECT count(*) FROM audit_log WHERE environment_id=$1 AND action='workspace.file_written' AND details->>'path'=$2")
        .bind(site.environment).bind(&stat.entry.path).fetch_one(&mut *conn).await.unwrap();
    assert_eq!(saved, 1);
    let state: String =
        sqlx::query_scalar("SELECT state FROM commands WHERE body->'work'->>'job_id'=$1")
            .bind(id.to_string())
            .fetch_one(&mut *conn)
            .await
            .unwrap();
    assert_eq!(
        state, "cancelled",
        "retaining a receipt never rewrites the command's outcome"
    );
    site.app().app.engine.stop();
}

#[tokio::test]
async fn upload_write_admission_requires_current_files_management_and_the_callers_private_blob_authority()
 {
    let site = live_site().await;
    let blob = browser_upload(&site, format!("private {}", new_id()).as_bytes()).await;
    let before = upload_pins(&site, blob).await;
    let body = EnvironmentUploadWriteBody {
        path: "web/private.txt".into(),
        blob,
        expected_hash: Some("none".into()),
        upload_batch: new_id(),
    };
    let denied = new_id();
    assert_eq!(
        site.app()
            .call(EnvironmentUploadWrite, &[&site.environment, &denied])
            .token(&site.eve.token)
            .query(&ThroughThreadQuery::default())
            .body(&body)
            .send()
            .await
            .status,
        StatusCode::FORBIDDEN
    );
    let private = new_id();
    let inaccessible = site
        .app()
        .call(EnvironmentUploadWrite, &[&site.environment, &private])
        .token(&site.dev.token)
        .query(&ThroughThreadQuery::default())
        .body(&body)
        .send()
        .await;
    assert_eq!(inaccessible.status, StatusCode::NOT_FOUND);
    let unknown = new_id();
    let missing = site
        .app()
        .call(EnvironmentUploadWrite, &[&site.environment, &unknown])
        .token(&site.ada.token)
        .query(&ThroughThreadQuery::default())
        .body(&EnvironmentUploadWriteBody {
            blob: Hash::of(b"not stored"),
            ..body
        })
        .send()
        .await;
    assert_eq!(missing.status, inaccessible.status);
    assert_eq!(missing.problem().code, inaccessible.problem().code);
    for id in [denied, private, unknown] {
        assert!(
            orch_db::queries::jobs::job(&mut *site.world.conn().await, id)
                .await
                .unwrap()
                .is_none()
        );
    }
    assert_eq!(upload_pins(&site, blob).await, before);
    assert_eq!(site.sent(CommandKind::FsWrite), 0);
}

#[tokio::test]
async fn upload_history_records_the_actor_files_folder_sizes_outcome_and_replacements_once() {
    use orch_api_contract::workspace::{
        EnvironmentFileContentPut, EnvironmentFileContentPutQuery, EnvironmentUploadFinish,
    };
    let site = live_site().await;
    let batch = new_id();
    let blob = browser_upload(&site, b"upload bytes").await;
    for path in ["web/a.txt", "web/README.md"] {
        site.app()
            .call(EnvironmentFileContentPut, &[&site.environment])
            .token(&site.ada.token)
            .query(&EnvironmentFileContentPutQuery {
                thread: Some(site.thread),
                path: path.to_owned(),
                blob: Some(blob),
                expected_hash: if path.ends_with("README.md") {
                    None
                } else {
                    Some("none".to_owned())
                },
                upload_batch: Some(batch),
            })
            .send()
            .await
            .ok();
    }
    let body = json!({"folder":"web","outcome":"complete","files":[
        {"name":"a.txt","size":12,"upload_id":null,"blob":blob,"outcome":"complete"},
        {"name":"README.md","size":12,"upload_id":null,"blob":blob,"outcome":"complete"}
    ]});
    for _ in 0..2 {
        assert_eq!(
            site.app()
                .call(EnvironmentUploadFinish, &[&site.environment, &batch])
                .token(&site.ada.token)
                .raw_body(&body)
                .send()
                .await
                .status,
            StatusCode::NO_CONTENT
        );
    }
    let mut conn = site.world.conn().await;
    let rows: Vec<(orch_types::Id, chrono::DateTime<chrono::Utc>, Json)> = sqlx::query_as("SELECT actor_id,at,details FROM audit_log WHERE environment_id=$1 AND history AND details ? 'upload'")
        .bind(site.environment).fetch_all(&mut *conn).await.unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].0, site.ada.id());
    assert_eq!(rows[0].1, site.world.now());
    let report = &rows[0].2["upload"];
    assert_eq!(report["folder"], "web");
    assert_eq!(report["outcome"], "complete");
    assert_eq!(report["bytes"], 24);
    assert_eq!(report["replaced"], 1);
    let names = report["files"]
        .as_array()
        .unwrap()
        .iter()
        .map(|file| file["name"].as_str().unwrap())
        .collect::<Vec<_>>();
    assert!(names.contains(&"a.txt") && names.contains(&"README.md"));
    assert!(
        report["files"]
            .as_array()
            .unwrap()
            .iter()
            .all(|file| file["size"] == 12 && file["received"] == 12)
    );
}

#[tokio::test]
async fn a_cancelled_upload_history_uses_the_servers_partial_receipt_even_after_the_machine_goes_away()
 {
    use orch_api_contract::threads::{
        UploadSessionCreate, UploadSessionCreateBody, UploadSessionDelete, UploadSessionPut,
        UploadSessionPutQuery,
    };
    use orch_api_contract::workspace::EnvironmentUploadFinish;
    let site = live_site().await;
    let upload = site
        .app()
        .call(UploadSessionCreate, &[])
        .token(&site.ada.token)
        .body(&UploadSessionCreateBody {
            name: "partial.txt".to_owned(),
            size: 100,
            media_type: None,
        })
        .send()
        .await
        .ok();
    site.app()
        .call(UploadSessionPut, &[&upload.upload_id])
        .token(&site.ada.token)
        .query(&UploadSessionPutQuery { offset: 0 })
        .bytes(b"partial")
        .send()
        .await
        .ok();
    assert_eq!(
        site.app()
            .call(UploadSessionDelete, &[&upload.upload_id])
            .token(&site.ada.token)
            .send()
            .await
            .status,
        StatusCode::NO_CONTENT
    );
    site.set_remote_write(false).await;
    site.power_off().await;
    let batch = new_id();
    let body = json!({"folder":"web","outcome":"cancelled","files":[{"name":"partial.txt","size":100,"upload_id":upload.upload_id,"blob":null,"outcome":"cancelled"}]});
    assert_eq!(
        site.app()
            .call(EnvironmentUploadFinish, &[&site.environment, &batch])
            .token(&site.ada.token)
            .raw_body(&body)
            .send()
            .await
            .status,
        StatusCode::NO_CONTENT
    );
    let mut conn = site.world.conn().await;
    let details: Json = sqlx::query_scalar(
        "SELECT details FROM audit_log WHERE environment_id=$1 AND details->'upload'->>'id'=$2",
    )
    .bind(site.environment)
    .bind(batch.to_string())
    .fetch_one(&mut *conn)
    .await
    .unwrap();
    assert_eq!(details["upload"]["bytes"], 7);
    assert_eq!(details["upload"]["outcome"], "cancelled");
    assert_eq!(details["upload"]["files"][0]["name"], "partial.txt");
    assert_eq!(details["upload"]["files"][0]["size"], 100);
    assert_eq!(details["upload"]["files"][0]["received"], 7);
    assert!(site.disk.read("web/partial.txt").is_none());
}

#[tokio::test]
async fn cancelling_after_the_bytes_finished_keeps_the_receipt_without_keeping_upload_authority() {
    use orch_api_contract::threads::{
        ThreadAttachmentCreate, UploadBlobDelete, UploadBlobDeleteQuery, UploadSessionComplete,
        UploadSessionCreate, UploadSessionCreateBody, UploadSessionPut, UploadSessionPutQuery,
    };
    use orch_api_contract::workspace::EnvironmentUploadFinish;
    let site = live_site().await;
    let upload = site
        .app()
        .call(UploadSessionCreate, &[])
        .token(&site.ada.token)
        .body(&UploadSessionCreateBody {
            name: "ready.txt".to_owned(),
            size: 8,
            media_type: None,
        })
        .send()
        .await
        .ok();
    site.app()
        .call(UploadSessionPut, &[&upload.upload_id])
        .token(&site.ada.token)
        .query(&UploadSessionPutQuery { offset: 0 })
        .bytes(b"complete")
        .send()
        .await
        .ok();
    let done = site
        .app()
        .call(UploadSessionComplete, &[&upload.upload_id])
        .token(&site.ada.token)
        .send()
        .await
        .ok();
    site.app()
        .call(UploadBlobDelete, &[&done.blob])
        .token(&site.ada.token)
        .query(&UploadBlobDeleteQuery::default())
        .send()
        .await
        .ok();
    let refused = site
        .app()
        .call(ThreadAttachmentCreate, &[&site.thread])
        .token(&site.ada.token)
        .raw_body(&json!({"blob":done.blob,"name":"ready.txt"}))
        .send()
        .await
        .problem();
    assert_eq!(
        refused.code,
        ErrorCode::NotFound,
        "a receipt gives no authority over bytes"
    );
    let batch = new_id();
    site.app().call(EnvironmentUploadFinish, &[&site.environment, &batch]).token(&site.ada.token)
        .raw_body(&json!({"folder":"web","outcome":"cancelled","files":[{"name":"ready.txt","size":8,"upload_id":upload.upload_id,"blob":done.blob,"outcome":"cancelled"}]}))
        .send().await.ok();
    let mut conn = site.world.conn().await;
    let details: Json = sqlx::query_scalar(
        "SELECT details FROM audit_log WHERE environment_id=$1 AND details->'upload'->>'id'=$2",
    )
    .bind(site.environment)
    .bind(batch.to_string())
    .fetch_one(&mut *conn)
    .await
    .unwrap();
    assert_eq!(details["upload"]["bytes"], 8);
    assert_eq!(details["upload"]["files"][0]["received"], 8);
    assert_eq!(details["upload"]["outcome"], "cancelled");
    assert!(site.disk.read("web/ready.txt").is_none());
}

#[tokio::test]
async fn a_batch_that_failed_before_a_session_started_never_claims_an_upload_succeeded() {
    use orch_api_contract::workspace::EnvironmentUploadFinish;
    let site = live_site().await;
    let batch = new_id();
    site.app()
        .call(EnvironmentUploadFinish, &[&site.environment, &batch])
        .token(&site.ada.token)
        .raw_body(&json!({"folder":"web","outcome":"failed","files":[]}))
        .send()
        .await
        .ok();
    let mut conn = site.world.conn().await;
    let details: Json = sqlx::query_scalar(
        "SELECT details FROM audit_log WHERE environment_id=$1 AND details->'upload'->>'id'=$2",
    )
    .bind(site.environment)
    .bind(batch.to_string())
    .fetch_one(&mut *conn)
    .await
    .unwrap();
    assert_eq!(details["upload"]["outcome"], "failed");
    assert_eq!(details["upload"]["bytes"], 0);
    assert_eq!(details["upload"]["files"], json!([]));
}

async fn live_site() -> Site {
    let mut site = Site::new().await;
    site.disk.put("web/src/main.rs", b"fn main() {}\n");
    site.disk.put("web/src/lib.rs", b"pub fn lib() {}\n");
    site.disk.put("web/README.md", b"# Web\n");
    site.disk.put("web/Zebra.txt", b"stripes\n");
    site.disk.put("web/apple.txt", b"fruit\n");
    site.power_on();
    site
}

// ---------------------------------------------------------------- listing

#[tokio::test]
async fn a_directory_is_listed_from_the_machine_with_directories_first_and_names_in_order() {
    let site = live_site().await;
    let listed = ask::list(&site, &site.ada, "web", None).await.ok();
    assert_eq!(listed.source, FilesSource::Live);
    assert_eq!(listed.path, "web");
    assert!(!listed.truncated);
    let names: Vec<_> = listed
        .entries
        .iter()
        .map(|entry| entry.name.as_str())
        .collect();
    assert_eq!(
        names,
        ["src", "apple.txt", "README.md", "Zebra.txt"],
        "directories first, then by name without regard to case"
    );
    assert_eq!(listed.entries[0].kind, DirEntryKind::Dir);
    assert_eq!(listed.entries[1].size, 6);
    assert!(
        listed.entries.iter().all(|entry| !entry.cached),
        "nothing was read yet"
    );
}

#[tokio::test]
async fn the_root_of_the_workspace_is_listed_with_the_empty_path() {
    let site = live_site().await;
    let listed = ask::list(&site, &site.ada, "", None).await.ok();
    assert_eq!(listed.entries.len(), 1);
    assert_eq!(listed.entries[0].name, "web");
}

#[tokio::test]
async fn a_directory_that_is_not_there_is_not_found() {
    let site = live_site().await;
    let missing = ask::list(&site, &site.ada, "nowhere", None).await.problem();
    assert_eq!(missing.code, ErrorCode::NotFound);
}

#[tokio::test]
async fn a_listing_marks_the_files_whose_content_is_already_in_the_store() {
    let site = live_site().await;
    let fetched = ask::fetch(&site, &site.ada, "web/README.md", Some(5))
        .await
        .ok();
    assert_eq!(fetched.state, FetchStatus::Ready);
    let listed = ask::list(&site, &site.ada, "web", None).await.ok();
    let cached: Vec<_> = listed
        .entries
        .iter()
        .filter(|entry| entry.cached)
        .map(|entry| entry.name.as_str())
        .collect();
    assert_eq!(cached, ["README.md"]);

    // A file that changed since is another version, and its content is not in the store.
    site.disk.change("web/README.md", b"# Web, changed\n");
    let again = ask::list(&site, &site.ada, "web", None).await.ok();
    assert!(again.entries.iter().all(|entry| !entry.cached));
}

#[tokio::test]
async fn file_reads_never_attach_an_environment_or_start_a_background_measurement_watch() {
    let site = live_site().await;
    ask::list(&site, &site.ada, "web", None).await.ok();
    ask::stat(&site, &site.ada, "web/README.md").await.ok();
    ask::list(&site, &site.vic, "web/src", None).await.ok();

    assert_eq!(
        site.sent(CommandKind::EnvAttach),
        0,
        "the engine attaches an environment in each session of its machine, not a request"
    );
    assert!(site.disk.attaches().is_empty());
    assert_eq!(
        site.sent(CommandKind::WatchSet),
        0,
        "the visible live subscription owns a measurement watch; reading a file does not"
    );
}

#[tokio::test]
async fn an_environment_that_does_not_stand_attached_is_answered_as_offline_until_it_does() {
    let site = live_site().await;
    for status in ["provisioning", "stopped", "failed", "recreating"] {
        sqlx::query("UPDATE environments SET status = $2 WHERE id = $1")
            .bind(site.environment)
            .bind(status)
            .execute(&site.world.db.pool)
            .await
            .unwrap();
        let refused = ask::stat(&site, &site.ada, "web/README.md").await.problem();
        assert_eq!(refused.code, ErrorCode::SourceOffline, "{status}");
    }
    assert_eq!(
        site.sent(CommandKind::FsStat),
        0,
        "the machine was not asked about an environment it holds no more"
    );
    sqlx::query("UPDATE environments SET status = 'running' WHERE id = $1")
        .bind(site.environment)
        .execute(&site.world.db.pool)
        .await
        .unwrap();
    assert!(
        ask::stat(&site, &site.ada, "web/README.md")
            .await
            .status
            .is_success()
    );
}

#[tokio::test]
async fn a_directory_is_asked_again_once_the_engine_has_attached_it_in_the_new_session_of_its_machine()
 {
    let site = Site::on_a_directory().await;
    site.disk.put("web/README.md", b"# Web\n");
    assert!(
        ask::stat(&site, &site.ada, "web/README.md")
            .await
            .status
            .is_success()
    );
    // The machine comes back in a session of its own: the directory was attached in the one before.
    sqlx::query("UPDATE environments SET attached_session = '2000-01-01T00:00:00Z' WHERE id = $1")
        .bind(site.environment)
        .execute(&site.world.db.pool)
        .await
        .unwrap();
    sqlx::query("UPDATE machines SET last_seen_at = now() WHERE id = $1")
        .bind(site.machine)
        .execute(&site.world.db.pool)
        .await
        .unwrap();
    let refused = ask::stat(&site, &site.ada, "web/README.md").await.problem();
    assert_eq!(refused.code, ErrorCode::SourceOffline);
    assert_eq!(
        site.sent(CommandKind::EnvAttach),
        0,
        "a request does not attach it"
    );

    // The engine does, and from then on the machine is asked again.
    site.app().app.engine.start();
    eventually("the engine to attach the directory", || async {
        site.sent(CommandKind::EnvAttach) == 1
    })
    .await;
    eventually("the directory to be asked again", || async {
        ask::stat(&site, &site.ada, "web/README.md")
            .await
            .status
            .is_success()
    })
    .await;
    assert_eq!(site.sent(CommandKind::EnvAttach), 1, "once for the session");
}

// ---------------------------------------------------------------- who may

#[tokio::test]
async fn a_viewer_reads_the_files_and_cannot_write_them() {
    let site = live_site().await;
    let before = site.disk.paths();

    // What a viewer may do.
    assert!(
        ask::list(&site, &site.vic, "web", None)
            .await
            .status
            .is_success()
    );
    assert!(
        ask::stat(&site, &site.vic, "web/README.md")
            .await
            .status
            .is_success()
    );
    let fetched = ask::fetch(&site, &site.vic, "web/README.md", Some(5))
        .await
        .ok();
    let hash = fetched.hash.unwrap();
    let read = ask::content(&site, &site.vic, "web/README.md", &hash, None, false).await;
    assert_eq!(read.status, StatusCode::OK);

    // What a viewer may not.
    let writes = [
        ask::write(&site, &site.vic, "web/new.txt", Some("none"), b"x")
            .await
            .problem(),
        ask::mkdir(&site, &site.vic, "web/dir").await.problem(),
        ask::rename(&site, &site.vic, "web/README.md", "web/READ.md")
            .await
            .problem(),
        ask::copy(&site, &site.vic, "web/README.md", "web/COPY.md")
            .await
            .problem(),
        ask::delete(&site, &site.vic, "web/README.md", false)
            .await
            .problem(),
    ];
    for refused in &writes {
        assert_eq!(refused.code, ErrorCode::AuthzDenied, "{refused:?}");
        assert!(
            places::missing_permissions(refused).contains(&"files.manage".to_owned()),
            "it says what is missing: {refused:?}"
        );
    }
    assert_eq!(site.disk.paths(), before, "nothing on the disk changed");
    for kind in [
        CommandKind::FsWrite,
        CommandKind::FsMkdir,
        CommandKind::FsRename,
        CommandKind::FsCopy,
        CommandKind::FsRemove,
    ] {
        assert_eq!(site.sent(kind), 0, "{kind} was never sent");
    }
}

#[tokio::test]
async fn a_developer_of_the_org_writes_the_files_of_its_threads() {
    let site = live_site().await;
    let written = ask::write(&site, &site.dev, "web/by-dev.txt", Some("none"), b"hello").await;
    assert!(written.status.is_success(), "{:?}", written.body);
    assert_eq!(site.disk.read("web/by-dev.txt").unwrap(), b"hello");
}

#[tokio::test]
async fn somebody_who_belongs_to_nothing_there_reaches_nothing() {
    let site = live_site().await;
    let listed = ask::list(&site, &site.eve, "web", None).await;
    assert!(!listed.status.is_success());
    assert!(
        matches!(listed.status.as_u16(), 403 | 404),
        "{:?}",
        listed.status
    );
    let wrote = ask::write(&site, &site.eve, "web/x.txt", None, b"x").await;
    assert!(!wrote.status.is_success());
    assert_eq!(site.sent(CommandKind::FsList), 0);
    assert_eq!(site.sent(CommandKind::FsWrite), 0);
    assert_eq!(
        site.sent(CommandKind::WatchSet),
        0,
        "the machine was not asked anything"
    );
}

#[tokio::test]
async fn the_environments_files_are_given_on_its_own_page_and_a_thread_named_gives_nothing() {
    let site = live_site().await;
    let listing = |thread: Option<orch_types::Id>, token: &str| {
        site.app()
            .call(EnvironmentFileList, &[&site.environment])
            .token(token)
            .query(&EnvironmentFileListQuery {
                thread,
                path: "web".to_owned(),
                source: None,
                mount: None,
                cursor: None,
                limit: None,
            })
            .send()
    };
    // Its manager reaches it without naming a thread, and so does a viewer: files.view is a row of the
    // environment's page that ships to Viewers.
    let by_manager = listing(None, &site.ada.token).await;
    assert!(by_manager.status.is_success(), "{:?}", by_manager.body);
    let by_viewer = listing(None, &site.vic.token).await;
    assert!(by_viewer.status.is_success(), "{:?}", by_viewer.body);
    // Naming a thread changes nothing for either: it says beside which thread they work.
    assert!(
        listing(Some(site.thread), &site.vic.token)
            .await
            .status
            .is_success()
    );
    // Somebody who holds everything on the thread and nothing on its environment is refused, with what
    // is missing named on the environment.
    let guest = places::person(&site.world, "guest").await;
    site.app()
        .call(
            orch_api_contract::access::PermissionPageUpdate,
            &[&"nodes", &site.thread],
        )
        .token(&site.ada.token)
        .raw_body(&json!({ "name": [{
            "holders": [{ "kind": "user", "id": guest.id() }],
            "choice": "full_manage"
        }] }))
        .send()
        .await
        .ok();
    let through_the_thread = listing(Some(site.thread), &guest.token).await.problem();
    assert_eq!(through_the_thread.code, ErrorCode::AuthzDenied);
    assert!(
        places::missing_permissions(&through_the_thread).contains(&"files.view".to_owned()),
        "{through_the_thread:?}"
    );
    assert!(
        through_the_thread.extra["missing"]
            .to_string()
            .contains("environment"),
        "what is missing is on the environment: {through_the_thread:?}"
    );
    // A thread that the caller may not view, or that runs elsewhere, is not found, whoever they are.
    assert_eq!(
        listing(Some(new_id()), &site.ada.token)
            .await
            .problem()
            .code,
        ErrorCode::NotFound
    );
}

#[tokio::test]
async fn whoever_manages_the_environment_is_decided_by_the_access_engine_and_not_by_this_part() {
    use orch_engine::access::ResourceAccess;
    let site = live_site().await;
    let access = orch_api_core::thread_access::AuthzAccess::new(site.app().app.clock.clone());
    let mut conn = site.world.conn().await;
    for (who, expected) in [
        (&site.ada, true),
        (&site.dev, true),
        (&site.vic, false),
        (&site.eve, false),
    ] {
        let decision = access
            .environment_manage(&mut conn, who.id(), site.environment)
            .await
            .unwrap();
        assert_eq!(decision.is_allowed(), expected, "{}", who.person.username);
    }
}

#[tokio::test]
async fn a_thread_that_runs_in_no_such_environment_lets_nobody_in() {
    let site = live_site().await;
    let nowhere = site
        .app()
        .call(EnvironmentFileList, &[&new_id()])
        .token(&site.ada.token)
        .query(&EnvironmentFileListQuery {
            thread: Some(site.thread),
            path: String::new(),
            source: None,
            mount: None,
            cursor: None,
            limit: None,
        })
        .send()
        .await
        .problem();
    assert_eq!(
        nowhere.code,
        ErrorCode::NotFound,
        "the thread runs in another environment"
    );
}

// ---------------------------------------------------------------- paths

#[tokio::test]
async fn a_path_outside_the_workspace_is_refused_without_the_machine_being_asked() {
    let site = live_site().await;
    for path in [
        "../secrets",
        "web/../../etc/passwd",
        "/etc/passwd",
        "..",
        "web/../..",
    ] {
        let listed = ask::list(&site, &site.ada, path, None).await.problem();
        assert_eq!(listed.code, ErrorCode::NotFound, "list {path}");
        let stat = ask::stat(&site, &site.ada, path).await.problem();
        assert_eq!(stat.code, ErrorCode::NotFound, "stat {path}");
        let wrote = ask::write(&site, &site.ada, path, None, b"x")
            .await
            .problem();
        assert_eq!(wrote.code, ErrorCode::NotFound, "write {path}");
        let renamed = ask::rename(&site, &site.ada, "web/README.md", path)
            .await
            .problem();
        assert_eq!(renamed.code, ErrorCode::NotFound, "rename to {path}");
        let deleted = ask::delete(&site, &site.ada, path, true).await.problem();
        assert_eq!(deleted.code, ErrorCode::NotFound, "delete {path}");
    }
    assert!(site.disk.exists("web/README.md"));
    for kind in [
        CommandKind::FsList,
        CommandKind::FsStat,
        CommandKind::FsWrite,
        CommandKind::FsRename,
        CommandKind::FsRemove,
    ] {
        assert_eq!(site.sent(kind), 0, "{kind}");
    }
}

#[tokio::test]
async fn a_path_that_is_no_path_is_invalid() {
    let site = live_site().await;
    for path in ["web//src", "web/./src", "web/\u{0}x"] {
        let listed = ask::list(&site, &site.ada, path, None).await.problem();
        assert_eq!(listed.code, ErrorCode::RequestInvalid, "{path:?}");
    }
    let root_as_file = ask::write(&site, &site.ada, "", None, b"x").await.problem();
    assert_eq!(
        root_as_file.code,
        ErrorCode::RequestInvalid,
        "the root is no file"
    );
}

// ---------------------------------------------------------------- from what was kept

#[tokio::test]
async fn a_machine_that_is_away_answers_from_the_last_list_and_says_how_old_it_is() {
    let site = live_site().await;
    ask::list(&site, &site.ada, "web", None).await.ok();
    let taken = site.world.now();
    site.publish_manifest().await;
    site.power_off().await;
    site.world.advance(Duration::hours(3));

    let listed = ask::list(&site, &site.ada, "web", None).await.ok();
    assert_eq!(listed.source, FilesSource::Cached);
    assert_eq!(
        listed.as_of.get(),
        taken,
        "as of the moment the list was taken, not of now"
    );
    assert!(site.world.now() - listed.as_of.get() >= Duration::hours(3));
    let names: Vec<_> = listed
        .entries
        .iter()
        .map(|entry| entry.name.as_str())
        .collect();
    assert_eq!(names, ["src", "apple.txt", "README.md", "Zebra.txt"]);
    assert_eq!(listed.entries[2].size, 6);

    // Asking for the machine's own word, there is none.
    let live = ask::list(&site, &site.ada, "web", Some(FileListSource::Live))
        .await
        .problem();
    assert_eq!(live.code, ErrorCode::SourceOffline);
}

#[tokio::test]
async fn a_directory_of_the_list_is_found_below_a_directory_it_does_not_name_itself() {
    let site = live_site().await;
    site.publish_manifest().await;
    site.power_off().await;
    let nested = ask::list(&site, &site.ada, "web/src", None).await.ok();
    assert_eq!(nested.source, FilesSource::Cached);
    assert_eq!(nested.entries.len(), 2);
    assert_eq!(
        ask::list(&site, &site.ada, "web/nope", None)
            .await
            .problem()
            .code,
        ErrorCode::NotFound
    );
    assert_eq!(
        ask::list(&site, &site.ada, "web/README.md", None)
            .await
            .problem()
            .code,
        ErrorCode::NotFound,
        "a file is no directory"
    );
}

#[tokio::test]
async fn an_environment_that_never_sent_a_list_has_nothing_to_show_while_it_is_away() {
    let site = live_site().await;
    site.power_off().await;
    let listed = ask::list(&site, &site.ada, "web", None).await.problem();
    assert_eq!(listed.code, ErrorCode::SourceOffline);
}

#[tokio::test]
async fn the_cached_listing_can_be_asked_for_while_the_machine_is_there() {
    let site = live_site().await;
    site.publish_manifest().await;
    site.disk.put("web/new-since.txt", b"after the list");
    let cached = ask::list(&site, &site.ada, "web", Some(FileListSource::Cached))
        .await
        .ok();
    assert_eq!(cached.source, FilesSource::Cached);
    assert!(
        cached
            .entries
            .iter()
            .all(|entry| entry.name != "new-since.txt")
    );
    let live = ask::list(&site, &site.ada, "web", Some(FileListSource::Live))
        .await
        .ok();
    assert!(
        live.entries
            .iter()
            .any(|entry| entry.name == "new-since.txt")
    );
    assert_eq!(
        site.sent(CommandKind::FsList),
        1,
        "the cached one asked nothing"
    );
}

#[tokio::test]
async fn an_environment_whose_machine_has_not_sent_a_list_yet_says_so() {
    let site = live_site().await;
    let cached = ask::list(&site, &site.ada, "web", Some(FileListSource::Cached))
        .await
        .problem();
    assert_eq!(cached.code, ErrorCode::EnvironmentUnavailable);
}

#[tokio::test]
async fn a_tree_too_large_to_index_has_no_list_and_is_listed_live_or_from_what_was_opened() {
    let site = live_site().await;
    // The machine said at its attach that it keeps no list of this tree.
    ask::list(&site, &site.ada, "web", None).await.ok();
    sqlx::query(
        "UPDATE environments SET facts = jsonb_set(facts, '{index}', '\"live\"') WHERE id = $1",
    )
    .bind(site.environment)
    .execute(&site.world.db.pool)
    .await
    .unwrap();

    let asked = ask::list(&site, &site.ada, "web", Some(FileListSource::Cached))
        .await
        .problem();
    assert_eq!(asked.code, ErrorCode::WorkspaceNotIndexed);

    // What was opened before is what an environment that is away still shows.
    ask::fetch(&site, &site.ada, "web/README.md", Some(5))
        .await
        .ok();
    site.power_off().await;
    let away = ask::list(&site, &site.ada, "web", None).await.ok();
    assert_eq!(away.source, FilesSource::Cached);
    assert_eq!(away.entries.len(), 1);
    assert_eq!(away.entries[0].name, "README.md");
    assert!(away.entries[0].cached);
}

// ---------------------------------------------------------------- describing

#[tokio::test]
async fn a_file_is_described_with_its_hash_its_type_and_whether_it_is_text() {
    let site = live_site().await;
    let stat = ask::stat(&site, &site.ada, "web/src/main.rs").await.ok();
    assert_eq!(stat.entry.kind, DirEntryKind::File);
    assert_eq!(stat.entry.path, "web/src/main.rs");
    assert_eq!(stat.entry.size, 13);
    assert_eq!(stat.hash, Some(Hash::of(b"fn main() {}\n")));
    assert!(stat.is_text);
    let dir = ask::stat(&site, &site.ada, "web/src").await.ok();
    assert_eq!(dir.entry.kind, DirEntryKind::Dir);
    assert_eq!(dir.media_type, "inode/directory");
    assert_eq!(dir.hash, None);
    assert_eq!(
        ask::stat(&site, &site.ada, "web/none").await.problem().code,
        ErrorCode::NotFound
    );
}

#[tokio::test]
async fn a_path_the_ignore_rules_leave_out_does_not_exist_for_anybody() {
    let site = live_site().await;
    site.disk.refuse_next(
        CommandKind::FsStat,
        CommandError::new(MachineCode::Ignored, "left out by the ignore rules"),
    );
    assert_eq!(
        ask::stat(&site, &site.ada, "web/debug.log")
            .await
            .problem()
            .code,
        ErrorCode::NotFound
    );
}

#[tokio::test]
async fn a_machine_that_is_away_describes_a_path_from_its_last_list() {
    let site = live_site().await;
    site.publish_manifest().await;
    site.power_off().await;
    let file = ask::stat(&site, &site.ada, "web/README.md").await.ok();
    assert_eq!(file.hash, Some(Hash::of(b"# Web\n")));
    assert_eq!(file.entry.size, 6);
    let dir = ask::stat(&site, &site.ada, "web/src").await.ok();
    assert_eq!(dir.entry.kind, DirEntryKind::Dir);
    let root = ask::stat(&site, &site.ada, "").await.ok();
    assert_eq!(root.entry.kind, DirEntryKind::Dir);
    assert_eq!(
        ask::stat(&site, &site.ada, "web/none").await.problem().code,
        ErrorCode::NotFound
    );
}

// ---------------------------------------------------------------- reading the content

#[tokio::test]
async fn reading_a_file_is_a_fetch_and_then_its_content_by_the_hash_the_fetch_gave() {
    let site = live_site().await;
    let bytes = bytes_of(70_000);
    site.disk.put("web/data.bin", &bytes);

    let fetched = ask::fetch(&site, &site.ada, "web/data.bin", Some(5))
        .await
        .ok();
    assert_eq!(fetched.state, FetchStatus::Ready);
    assert_eq!(fetched.hash, Some(Hash::of(&bytes)));
    assert_eq!(fetched.size, Some(70_000));
    let hash = fetched.hash.unwrap();

    let read = ask::content(&site, &site.ada, "web/data.bin", &hash, None, false).await;
    assert_eq!(read.status, StatusCode::OK);
    assert_eq!(read.raw.as_ref(), bytes.as_slice());
    assert_eq!(
        read.headers["etag"].to_str().unwrap(),
        format!("\"{hash}\""),
        "the hash is the tag, so that a browser keeps it"
    );
    assert_eq!(read.headers["x-content-type-options"], "nosniff");
    assert_eq!(read.headers["accept-ranges"], "bytes");
    assert_eq!(read.headers["content-length"], "70000");
}

#[tokio::test]
async fn a_version_that_is_in_the_store_is_not_fetched_again() {
    let site = live_site().await;
    let first = ask::fetch(&site, &site.ada, "web/README.md", Some(5))
        .await
        .ok();
    let second = ask::fetch(&site, &site.vic, "web/README.md", None)
        .await
        .ok();
    assert_eq!(
        (first.state, second.state),
        (FetchStatus::Ready, FetchStatus::Ready)
    );
    assert_eq!(first.hash, second.hash);
    assert_eq!(site.sent(CommandKind::FsFetch), 1);

    // Another version is another fetch.
    site.disk.change("web/README.md", b"# Web, rewritten\n");
    let third = ask::fetch(&site, &site.ada, "web/README.md", Some(5))
        .await
        .ok();
    assert_eq!(third.hash, Some(Hash::of(b"# Web, rewritten\n")));
    assert_eq!(site.sent(CommandKind::FsFetch), 2);
}

#[tokio::test]
async fn a_fetch_that_is_not_waited_for_says_the_file_is_on_its_way() {
    let site = live_site().await;
    *site.disk.fetch_delay.lock().unwrap() = std::time::Duration::from_millis(400);
    let first = ask::fetch(&site, &site.ada, "web/README.md", None)
        .await
        .ok();
    assert_eq!(first.state, FetchStatus::Fetching);
    assert_eq!(first.size, Some(6));
    assert_eq!(first.hash, None);
    // Asking again waits for the same command and does not send another.
    let waited = ask::fetch(&site, &site.ada, "web/README.md", Some(10))
        .await
        .ok();
    assert_eq!(waited.state, FetchStatus::Ready);
    assert_eq!(site.sent(CommandKind::FsFetch), 1);
}

#[tokio::test]
async fn a_file_that_is_not_there_or_is_no_file_cannot_be_had() {
    let site = live_site().await;
    for path in ["web/none.txt", "web/src"] {
        let fetched = ask::fetch(&site, &site.ada, path, Some(1)).await.ok();
        assert_eq!(fetched.state, FetchStatus::Unavailable, "{path}");
        assert_eq!(fetched.reason, Some(FetchUnavailable::NotFound), "{path}");
    }
}

#[tokio::test]
async fn a_file_the_ignore_rules_leave_out_is_said_to_be_ignored() {
    let site = live_site().await;
    site.disk.refuse_next(
        CommandKind::FsStat,
        CommandError::new(MachineCode::Ignored, "left out"),
    );
    let fetched = ask::fetch(&site, &site.ada, "web/debug.log", None)
        .await
        .ok();
    assert_eq!(fetched.reason, Some(FetchUnavailable::Ignored));
}

#[tokio::test]
async fn a_file_larger_than_a_blob_may_be_is_listed_and_cannot_be_read() {
    let site = live_site().await;
    site.world.set("storage.max_blob_bytes", json!(10)).await;
    site.disk.put("web/huge.bin", &bytes_of(100));
    let fetched = ask::fetch(&site, &site.ada, "web/huge.bin", None)
        .await
        .ok();
    assert_eq!(fetched.state, FetchStatus::Unavailable);
    assert_eq!(fetched.reason, Some(FetchUnavailable::TooLarge));
    assert_eq!(
        site.sent(CommandKind::FsFetch),
        0,
        "the machine is not asked for what cannot be kept"
    );
}

#[tokio::test]
async fn a_fetch_the_machine_gave_up_on_is_tried_again_for_the_version_it_finds() {
    let site = live_site().await;
    site.disk.refuse_next(
        CommandKind::FsFetch,
        CommandError::new(MachineCode::Changed, "the file changed while it was read"),
    );
    let fetched = ask::fetch(&site, &site.ada, "web/README.md", Some(5))
        .await
        .ok();
    assert_eq!(fetched.state, FetchStatus::Ready);
    assert_eq!(site.sent(CommandKind::FsFetch), 2);
}

#[tokio::test]
async fn the_content_of_a_path_is_served_only_for_a_hash_that_belongs_to_it() {
    let site = live_site().await;
    let readme = ask::fetch(&site, &site.ada, "web/README.md", Some(5))
        .await
        .ok()
        .hash
        .unwrap();
    let apple = ask::fetch(&site, &site.ada, "web/apple.txt", Some(5))
        .await
        .ok()
        .hash
        .unwrap();
    let wrong_path = ask::content(&site, &site.ada, "web/README.md", &apple, None, false).await;
    assert_eq!(wrong_path.problem().code, ErrorCode::NotFound);
    let unknown = ask::content(
        &site,
        &site.ada,
        "web/README.md",
        &Hash::of(b"nothing"),
        None,
        false,
    )
    .await;
    assert_eq!(unknown.problem().code, ErrorCode::NotFound);
    let right = ask::content(&site, &site.ada, "web/README.md", &readme, None, false).await;
    assert_eq!(right.status, StatusCode::OK);
}

#[tokio::test]
async fn content_is_read_in_ranges_and_a_browser_that_has_it_is_told_so() {
    let site = live_site().await;
    let bytes = bytes_of(10_000);
    site.disk.put("web/ranged.bin", &bytes);
    let hash = ask::fetch(&site, &site.ada, "web/ranged.bin", Some(5))
        .await
        .ok()
        .hash
        .unwrap();

    let part = ask::content(
        &site,
        &site.ada,
        "web/ranged.bin",
        &hash,
        Some("bytes=100-199"),
        false,
    )
    .await;
    assert_eq!(part.status, StatusCode::PARTIAL_CONTENT);
    assert_eq!(part.raw.as_ref(), &bytes[100..200]);
    assert_eq!(part.headers["content-range"], "bytes 100-199/10000");
    let tail = ask::content(
        &site,
        &site.ada,
        "web/ranged.bin",
        &hash,
        Some("bytes=9990-"),
        false,
    )
    .await;
    assert_eq!(tail.raw.as_ref(), &bytes[9_990..]);
    let beyond = ask::content(
        &site,
        &site.ada,
        "web/ranged.bin",
        &hash,
        Some("bytes=50000-"),
        false,
    )
    .await;
    assert_eq!(beyond.status, StatusCode::RANGE_NOT_SATISFIABLE);
    assert_eq!(beyond.headers["content-range"], "bytes */10000");

    let mut call = site
        .app()
        .call(
            orch_api_contract::workspace::EnvironmentFileContentGet,
            &[&site.environment],
        )
        .token(&site.ada.token)
        .query(
            &orch_api_contract::workspace::EnvironmentFileContentGetQuery {
                thread: Some(site.thread),
                mount: None,
                path: "web/ranged.bin".to_owned(),
                hash,
                download: None,
                preview: None,
            },
        );
    call = call.header("if-none-match", &format!("\"{hash}\""));
    assert_eq!(call.send_bytes().await.status, StatusCode::NOT_MODIFIED);
}

#[tokio::test]
async fn a_page_is_never_shown_as_a_page_and_a_download_is_offered_as_one() {
    let site = live_site().await;
    site.disk
        .put("web/index.html", b"<script>alert(1)</script>");
    site.disk.put("web/notes.txt", b"plain notes");
    let page = ask::fetch(&site, &site.ada, "web/index.html", Some(5))
        .await
        .ok()
        .hash
        .unwrap();
    let shown = ask::content(&site, &site.ada, "web/index.html", &page, None, false).await;
    assert_eq!(shown.status, StatusCode::OK);
    assert_eq!(shown.headers["content-type"], "application/octet-stream");
    assert_eq!(shown.headers["x-content-type-options"], "nosniff");
    assert_eq!(shown.raw.as_ref(), b"<script>alert(1)</script>");
    assert!(
        shown.headers["content-disposition"]
            .to_str()
            .unwrap()
            .starts_with("attachment;")
    );
    assert!(
        shown.headers["content-security-policy"]
            .to_str()
            .unwrap()
            .contains("sandbox")
    );

    let notes = ask::fetch(&site, &site.ada, "web/notes.txt", Some(5))
        .await
        .ok()
        .hash
        .unwrap();
    let saved = ask::content(&site, &site.ada, "web/notes.txt", &notes, None, true).await;
    assert_eq!(saved.headers["content-type"], "application/octet-stream");
    let disposition = saved.headers["content-disposition"].to_str().unwrap();
    assert!(disposition.starts_with("attachment"), "{disposition}");
    assert!(disposition.contains("notes.txt"));
}

#[tokio::test]
async fn what_was_read_before_can_be_read_again_while_the_machine_is_away_and_what_was_not_cannot()
{
    let site = live_site().await;
    let readme = ask::fetch(&site, &site.ada, "web/README.md", Some(5))
        .await
        .ok()
        .hash
        .unwrap();
    site.publish_manifest().await;
    site.power_off().await;

    let again = ask::fetch(&site, &site.ada, "web/README.md", None)
        .await
        .ok();
    assert_eq!(
        (again.state, again.hash),
        (FetchStatus::Ready, Some(readme))
    );
    let read = ask::content(&site, &site.ada, "web/README.md", &readme, None, false).await;
    assert_eq!(read.raw.as_ref(), b"# Web\n");

    let never = ask::fetch(&site, &site.ada, "web/apple.txt", None)
        .await
        .ok();
    assert_eq!(never.state, FetchStatus::Unavailable);
    assert_eq!(never.reason, Some(FetchUnavailable::OfflineNotCached));
    let missing = ask::fetch(&site, &site.ada, "web/none.txt", None)
        .await
        .ok();
    assert_eq!(missing.reason, Some(FetchUnavailable::NotFound));
}

// ---------------------------------------------------------------- writing

#[tokio::test]
async fn a_file_written_through_the_api_is_on_the_machines_disk_and_audited() {
    let site = live_site().await;
    let stat = ask::write(
        &site,
        &site.ada,
        "web/src/new.rs",
        Some("none"),
        b"// new\n",
    )
    .await
    .ok();
    assert_eq!(stat.entry.path, "web/src/new.rs");
    assert_eq!(stat.entry.size, 7);
    assert_eq!(stat.hash, Some(Hash::of(b"// new\n")));
    assert_eq!(site.disk.read("web/src/new.rs").unwrap(), b"// new\n");

    let audit = site.world.audit().await;
    assert!(
        audit
            .iter()
            .any(|action| action == "workspace.file_written"),
        "{audit:?}"
    );
    // What was written is in the store for exactly that version.
    let listed = ask::list(&site, &site.ada, "web/src", None).await.ok();
    let entry = listed
        .entries
        .iter()
        .find(|entry| entry.name == "new.rs")
        .unwrap();
    assert!(entry.cached);
}

#[tokio::test]
async fn a_file_is_written_over_the_version_the_writer_saw_and_over_no_other() {
    let site = live_site().await;
    let seen = ask::fetch(&site, &site.ada, "web/README.md", Some(5))
        .await
        .ok()
        .hash
        .unwrap();
    let on_seen = ask::write(
        &site,
        &site.ada,
        "web/README.md",
        Some(&seen.to_string()),
        b"# Mine\n",
    )
    .await;
    assert!(on_seen.status.is_success(), "{:?}", on_seen.body);
    assert_eq!(site.disk.read("web/README.md").unwrap(), b"# Mine\n");

    // Somebody else changed it meanwhile; the writer still holds the old version.
    site.disk.change("web/README.md", b"# Theirs\n");
    let late = ask::write(
        &site,
        &site.ada,
        "web/README.md",
        Some(&seen.to_string()),
        b"# Mine again\n",
    )
    .await
    .problem();
    assert_eq!(late.code, ErrorCode::ConflictStaleRevision);
    assert_eq!(
        late.extra["current"]["hash"],
        json!(Hash::of(b"# Theirs\n")),
        "the file as it stands is returned with the refusal"
    );
    assert_eq!(late.extra["current"]["path"], "web/README.md");
    assert_eq!(
        site.disk.read("web/README.md").unwrap(),
        b"# Theirs\n",
        "nothing was written"
    );
}

#[tokio::test]
async fn a_stale_write_returns_the_stat_captured_by_the_host_even_if_the_file_changed_again() {
    let site = live_site().await;
    site.disk.change("web/README.md", b"captured version\n");
    let captured = ask::stat(&site, &site.ada, "web/README.md").await.ok();
    site.disk.refuse_next(
        CommandKind::FsWrite,
        CommandError::new(MachineCode::Conflict, "the expected file changed")
            .with_details(json!({"current": captured})),
    );
    // This is what an API-side second stat would see, rather than the version that refused the write.
    site.disk
        .change("web/README.md", b"a still later version\n");
    let error = ask::write(
        &site,
        &site.ada,
        "web/README.md",
        Some("none"),
        b"replacement",
    )
    .await
    .problem();
    assert_eq!(error.code, ErrorCode::ConflictStaleRevision);
    assert_eq!(
        error.extra["current"],
        serde_json::to_value(captured).unwrap()
    );
    assert_eq!(
        site.disk.read("web/README.md").unwrap(),
        b"a still later version\n"
    );
}

#[tokio::test]
async fn a_file_that_must_not_exist_yet_is_not_written_over_one_that_does() {
    let site = live_site().await;
    let refused = ask::write(&site, &site.ada, "web/README.md", Some("none"), b"x")
        .await
        .problem();
    assert_eq!(refused.code, ErrorCode::ConflictStaleRevision);
    assert_eq!(site.disk.read("web/README.md").unwrap(), b"# Web\n");
    // A hash of a version for a file that is not there is as stale.
    let gone = ask::write(
        &site,
        &site.ada,
        "web/gone.txt",
        Some(&Hash::of(b"x").to_string()),
        b"y",
    )
    .await
    .problem();
    assert_eq!(gone.code, ErrorCode::ConflictStaleRevision);
    assert!(!site.disk.exists("web/gone.txt"));
}

#[tokio::test]
async fn a_write_that_names_no_version_replaces_the_file_and_one_that_names_a_bad_one_is_invalid() {
    let site = live_site().await;
    assert!(
        ask::write(&site, &site.ada, "web/README.md", None, b"# Replaced\n")
            .await
            .status
            .is_success()
    );
    assert_eq!(site.disk.read("web/README.md").unwrap(), b"# Replaced\n");
    let bad = ask::write(&site, &site.ada, "web/README.md", Some("latest"), b"x")
        .await
        .problem();
    assert_eq!(bad.code, ErrorCode::RequestInvalid);
}

#[tokio::test]
async fn a_file_larger_than_one_piece_is_written_whole() {
    let site = live_site().await;
    let bytes = bytes_of(300_000);
    let stat = ask::write(&site, &site.ada, "web/big.bin", Some("none"), &bytes)
        .await
        .ok();
    assert_eq!(stat.entry.size, 300_000);
    assert_eq!(site.disk.read("web/big.bin").unwrap(), bytes);
}

#[tokio::test]
async fn a_file_may_be_written_from_an_upload_that_was_made_before() {
    use orch_api_contract::workspace::{EnvironmentFileContentPut, EnvironmentFileContentPutQuery};
    let site = live_site().await;
    let blob = browser_upload(&site, b"from an upload").await;
    let wrote = site
        .app()
        .call(EnvironmentFileContentPut, &[&site.environment])
        .token(&site.ada.token)
        .query(&EnvironmentFileContentPutQuery {
            thread: Some(site.thread),
            path: "web/uploaded.txt".to_owned(),
            expected_hash: Some("none".to_owned()),
            blob: Some(blob),
            upload_batch: None,
        })
        .send()
        .await;
    assert!(wrote.status.is_success(), "{:?}", wrote.body);
    assert_eq!(
        site.disk.read("web/uploaded.txt").unwrap(),
        b"from an upload"
    );
    // An upload the store does not hold is no content.
    let none = site
        .app()
        .call(EnvironmentFileContentPut, &[&site.environment])
        .token(&site.ada.token)
        .query(&EnvironmentFileContentPutQuery {
            thread: Some(site.thread),
            path: "web/other.txt".to_owned(),
            expected_hash: None,
            blob: Some(Hash::of(b"never uploaded")),
            upload_batch: None,
        })
        .send()
        .await
        .problem();
    assert_eq!(none.code, ErrorCode::NotFound);
}

#[tokio::test]
async fn a_resumable_workspace_upload_exceeds_a_raw_body_and_conflicts_do_not_replace_it() {
    use orch_api_contract::workspace::{EnvironmentFileContentPut, EnvironmentFileContentPutQuery};
    let site = live_site().await;
    let bytes = bytes_of(orch_api_contract::common::MAX_BODY_BYTES + 1);
    let blob = browser_upload(&site, &bytes).await;
    assert!(
        site.disk.read("web/upload.txt").is_none(),
        "uploading bytes alone never writes the workspace"
    );
    let query = EnvironmentFileContentPutQuery {
        thread: Some(site.thread),
        path: "web/upload.txt".to_owned(),
        expected_hash: Some("none".to_owned()),
        blob: Some(blob),
        upload_batch: None,
    };
    let wrote = site
        .app()
        .call(EnvironmentFileContentPut, &[&site.environment])
        .token(&site.ada.token)
        .query(&query)
        .send()
        .await
        .ok();
    assert_eq!(wrote.entry.size, bytes.len() as i64);
    assert_eq!(site.disk.read("web/upload.txt").unwrap(), bytes);
    let conflict = site
        .app()
        .call(EnvironmentFileContentPut, &[&site.environment])
        .token(&site.ada.token)
        .query(&query)
        .send()
        .await
        .problem();
    assert_eq!(conflict.code, ErrorCode::ConflictStaleRevision);
    assert_eq!(site.disk.read("web/upload.txt").unwrap(), bytes);
    let mut conn = site.world.conn().await;
    let history: i64 = sqlx::query_scalar("SELECT count(*) FROM audit_log WHERE environment_id=$1 AND action='workspace.file_written' AND details->>'path'='web/upload.txt'")
        .bind(site.environment).fetch_one(&mut *conn).await.unwrap();
    assert_eq!(history, 1, "one finished upload leaves one history line");
}

#[tokio::test]
async fn knowing_another_persons_upload_hash_does_not_allow_writing_its_bytes() {
    use orch_api_contract::workspace::{EnvironmentFileContentPut, EnvironmentFileContentPutQuery};
    let site = live_site().await;
    let meta = site
        .app()
        .app
        .blobs
        .put_bytes(bytes::Bytes::from_static(b"private bytes"))
        .await
        .unwrap();
    let refused = site
        .app()
        .call(EnvironmentFileContentPut, &[&site.environment])
        .token(&site.ada.token)
        .query(&EnvironmentFileContentPutQuery {
            thread: Some(site.thread),
            path: "web/private.txt".to_owned(),
            expected_hash: Some("none".to_owned()),
            blob: Some(meta.hash),
            upload_batch: None,
        })
        .send()
        .await
        .problem();
    assert_eq!(refused.code, ErrorCode::NotFound);
    assert!(site.disk.read("web/private.txt").is_none());
}

#[tokio::test]
async fn files_are_not_written_into_an_environment_that_switched_writes_off() {
    let site = live_site().await;
    site.set_remote_write(false).await;
    let refused = ask::write(&site, &site.ada, "web/x.txt", None, b"x")
        .await
        .problem();
    assert_eq!(refused.code, ErrorCode::WorkspaceRemoteWriteDisabled);
    for refused in [
        ask::mkdir(&site, &site.ada, "web/d").await.problem(),
        ask::rename(&site, &site.ada, "web/README.md", "web/R.md")
            .await
            .problem(),
        ask::copy(&site, &site.ada, "web/README.md", "web/C.md")
            .await
            .problem(),
        ask::delete(&site, &site.ada, "web/README.md", false)
            .await
            .problem(),
    ] {
        assert_eq!(refused.code, ErrorCode::WorkspaceRemoteWriteDisabled);
    }
    assert_eq!(site.sent(CommandKind::FsWrite), 0);
    // Reading is still allowed.
    assert!(
        ask::list(&site, &site.ada, "web", None)
            .await
            .status
            .is_success()
    );
}

#[tokio::test]
async fn a_write_needs_the_machine_to_be_there() {
    let site = live_site().await;
    site.power_off().await;
    let refused = ask::write(&site, &site.ada, "web/x.txt", None, b"x")
        .await
        .problem();
    assert_eq!(refused.code, ErrorCode::SourceOffline);
    for refused in [
        ask::mkdir(&site, &site.ada, "web/d").await.problem(),
        ask::rename(&site, &site.ada, "web/README.md", "web/R.md")
            .await
            .problem(),
        ask::copy(&site, &site.ada, "web/README.md", "web/C.md")
            .await
            .problem(),
        ask::delete(&site, &site.ada, "web/README.md", false)
            .await
            .problem(),
    ] {
        assert_eq!(refused.code, ErrorCode::SourceOffline);
    }
}

#[tokio::test]
async fn what_the_machines_owner_does_not_allow_is_passed_on_as_the_problem_it_is() {
    let site = live_site().await;
    site.disk.refuse_next(
        CommandKind::FsWrite,
        CommandError::new(
            MachineCode::PolicyRemoteWriteDisabled,
            "this machine takes no writes from outside",
        ),
    );
    let refused = ask::write(&site, &site.ada, "web/x.txt", None, b"x")
        .await
        .problem();
    // The machine's own policy, as its code says, in the code's words.
    assert_eq!(refused.code, ErrorCode::PolicyRemoteWriteDisabled);
    assert_eq!(
        refused.detail,
        orch_api_contract::messages::for_code(ErrorCode::PolicyRemoteWriteDisabled)
    );
    site.disk.refuse_next(
        CommandKind::FsMkdir,
        CommandError::new(MachineCode::TooLarge, "no"),
    );
    assert_eq!(
        ask::mkdir(&site, &site.ada, "web/d").await.problem().code,
        ErrorCode::RequestTooLarge
    );
    site.disk.refuse_next(
        CommandKind::FsRename,
        CommandError::new(MachineCode::Unsupported, "no"),
    );
    assert_eq!(
        ask::rename(&site, &site.ada, "web/README.md", "web/R.md")
            .await
            .problem()
            .code,
        ErrorCode::CapabilityUnsupported
    );
    site.disk.refuse_next(
        CommandKind::FsRemove,
        CommandError::new(MachineCode::Internal, "disk full"),
    );
    assert_eq!(
        ask::delete(&site, &site.ada, "web/README.md", false)
            .await
            .problem()
            .status,
        500
    );
}

// ---------------------------------------------------------------- making, moving, copying, deleting

#[tokio::test]
async fn a_directory_is_made_on_the_disk() {
    let site = live_site().await;
    let made = ask::mkdir(&site, &site.ada, "web/docs").await.ok();
    assert_eq!(made.entry.kind, DirEntryKind::Dir);
    assert_eq!(made.entry.path, "web/docs");
    assert!(site.disk.exists("web/docs"));
    assert_eq!(site.sent(CommandKind::FsMkdir), 1);
}

#[tokio::test]
async fn a_file_is_renamed_and_so_is_a_directory_with_all_that_is_in_it() {
    let site = live_site().await;
    let moved = ask::rename(&site, &site.ada, "web/README.md", "web/READ-ME.md")
        .await
        .ok();
    assert_eq!(moved.entry.path, "web/READ-ME.md");
    assert!(!site.disk.exists("web/README.md"));
    assert_eq!(site.disk.read("web/READ-ME.md").unwrap(), b"# Web\n");

    ask::rename(&site, &site.ada, "web/src", "web/source")
        .await
        .ok();
    assert!(site.disk.exists("web/source/main.rs"));
    assert!(!site.disk.exists("web/src/main.rs"));
    let audit = site.world.audit().await;
    assert_eq!(
        audit
            .iter()
            .filter(|action| *action == "workspace.file_renamed")
            .count(),
        2
    );
}

#[tokio::test]
async fn a_rename_forgets_the_content_kept_for_the_old_name() {
    let site = live_site().await;
    ask::fetch(&site, &site.ada, "web/README.md", Some(5))
        .await
        .ok();
    ask::rename(&site, &site.ada, "web/README.md", "web/READ-ME.md")
        .await
        .ok();
    let mut conn = site.world.conn().await;
    let left = orch_db::queries::storage::cached_below(&mut conn, site.environment, "web")
        .await
        .unwrap();
    assert!(left.is_empty(), "{left:?}");
}

#[tokio::test]
async fn a_name_that_is_taken_is_not_taken_over_and_one_that_is_not_there_is_not_found() {
    let site = live_site().await;
    let taken = ask::rename(&site, &site.ada, "web/README.md", "web/apple.txt")
        .await
        .problem();
    assert_eq!(taken.code, ErrorCode::ConflictExists);
    let copy_taken = ask::copy(&site, &site.ada, "web/README.md", "web/apple.txt")
        .await
        .problem();
    assert_eq!(copy_taken.code, ErrorCode::ConflictExists);
    assert_eq!(site.disk.read("web/apple.txt").unwrap(), b"fruit\n");
    let nothing = ask::rename(&site, &site.ada, "web/nope", "web/else")
        .await
        .problem();
    assert_eq!(nothing.code, ErrorCode::NotFound);
    let nothing_to_copy = ask::copy(&site, &site.ada, "web/nope", "web/else")
        .await
        .problem();
    assert_eq!(nothing_to_copy.code, ErrorCode::NotFound);
}

#[tokio::test]
async fn a_path_is_not_moved_onto_itself_or_into_itself() {
    let site = live_site().await;
    for (from, to) in [("web/src", "web/src"), ("web/src", "web/src/inner")] {
        let renamed = ask::rename(&site, &site.ada, from, to).await.problem();
        assert_eq!(renamed.code, ErrorCode::RequestInvalid, "{from} -> {to}");
        let copied = ask::copy(&site, &site.ada, from, to).await.problem();
        assert_eq!(copied.code, ErrorCode::RequestInvalid, "{from} -> {to}");
    }
    assert_eq!(
        site.sent(CommandKind::FsRename) + site.sent(CommandKind::FsCopy),
        0
    );
}

#[tokio::test]
async fn a_file_and_a_directory_are_copied_and_the_original_stays() {
    let site = live_site().await;
    let copied = ask::copy(&site, &site.ada, "web/README.md", "web/README.bak")
        .await
        .ok();
    assert_eq!(copied.entry.path, "web/README.bak");
    assert_eq!(site.disk.read("web/README.bak").unwrap(), b"# Web\n");
    assert!(site.disk.exists("web/README.md"));
    ask::copy(&site, &site.ada, "web/src", "web/src-copy")
        .await
        .ok();
    assert_eq!(
        site.disk.read("web/src-copy/lib.rs").unwrap(),
        b"pub fn lib() {}\n"
    );
    assert!(site.disk.exists("web/src/lib.rs"));
    let audit = site.world.audit().await;
    assert_eq!(
        audit
            .iter()
            .filter(|action| *action == "workspace.file_copied")
            .count(),
        2
    );
}

#[tokio::test]
async fn a_file_is_deleted_and_a_directory_only_with_what_is_in_it_when_asked() {
    let site = live_site().await;
    ask::fetch(&site, &site.ada, "web/src/main.rs", Some(5))
        .await
        .ok();
    ask::delete(&site, &site.ada, "web/apple.txt", false)
        .await
        .ok();
    assert!(!site.disk.exists("web/apple.txt"));

    let full = ask::delete(&site, &site.ada, "web/src", false)
        .await
        .problem();
    assert_eq!(
        full.code,
        ErrorCode::ConflictExists,
        "a directory with files in it is not deleted by chance"
    );
    assert!(site.disk.exists("web/src/main.rs"));
    ask::delete(&site, &site.ada, "web/src", true).await.ok();
    assert!(!site.disk.exists("web/src"));
    assert!(!site.disk.exists("web/src/main.rs"));

    let mut conn = site.world.conn().await;
    let left = orch_db::queries::storage::cached_below(&mut conn, site.environment, "web/src")
        .await
        .unwrap();
    assert!(
        left.is_empty(),
        "the content kept for what is gone is forgotten"
    );
    let gone = ask::delete(&site, &site.ada, "web/src", true)
        .await
        .problem();
    assert_eq!(gone.code, ErrorCode::NotFound);
    let audit = site.world.audit().await;
    assert_eq!(
        audit
            .iter()
            .filter(|action| *action == "workspace.file_deleted")
            .count(),
        2
    );
}

#[tokio::test]
async fn changing_files_counts_as_the_activity_of_the_environment() {
    let site = live_site().await;
    let before: Option<chrono::DateTime<chrono::Utc>> =
        sqlx::query_scalar("SELECT last_activity_at FROM environments WHERE id = $1")
            .bind(site.environment)
            .fetch_one(&site.world.db.pool)
            .await
            .unwrap();
    site.world.advance(Duration::minutes(5));
    ask::mkdir(&site, &site.ada, "web/docs").await.ok();
    ask::write(&site, &site.ada, "web/new.txt", None, b"x")
        .await
        .ok();
    let after: Option<chrono::DateTime<chrono::Utc>> =
        sqlx::query_scalar("SELECT last_activity_at FROM environments WHERE id = $1")
            .bind(site.environment)
            .fetch_one(&site.world.db.pool)
            .await
            .unwrap();
    assert!(after > before, "{before:?} -> {after:?}");
}

#[tokio::test]
async fn the_latest_list_of_the_tree_is_kept_pinned_and_the_one_it_replaces_is_let_go() {
    let site = live_site().await;
    let first = site.publish_manifest().await;
    site.disk.put("web/new-since.txt", b"after the first list");
    site.world.advance(Duration::minutes(1));
    let second = site.publish_manifest().await;
    assert_ne!(first, second);

    let mut conn = site.world.conn().await;
    let pins = |hash: Hash| {
        let pool = site.world.db.pool.clone();
        async move {
            sqlx::query_scalar::<_, i32>("SELECT pin_count FROM blobs WHERE hash = $1")
                .bind(hash.as_bytes().as_slice())
                .fetch_one(&pool)
                .await
                .unwrap()
        }
    };
    assert_eq!(
        pins(first).await,
        0,
        "the list that was replaced is cache again"
    );
    assert_eq!(pins(second).await, 1, "the latest is kept");
    let kept: Option<Vec<u8>> =
        sqlx::query_scalar("SELECT manifest_blob FROM environments WHERE id = $1")
            .bind(site.environment)
            .fetch_one(&mut *conn)
            .await
            .unwrap();
    assert_eq!(kept.as_deref(), Some(second.as_bytes().as_slice()));
    drop(conn);
    // The same list told again changes nothing.
    site.publish_manifest().await;
    assert_eq!(pins(second).await, 1);
}
