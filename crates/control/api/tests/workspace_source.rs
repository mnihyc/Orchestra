//! Source publication and observed work are served through current environment and thread doors.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]

mod places;
mod support;
mod workspace;

use orch_api_contract::common::ErrorCode;
use orch_api_contract::environment::ThroughThreadQuery;
use orch_api_contract::nodes::{Node, NodeGet, OrgTreeGet, OrgTreeGetQuery};
use orch_api_contract::search::{NameSearchList, NameSearchListQuery};
use orch_api_contract::source::{EnvironmentGitCommit, EnvironmentGitCommitBody};
use orch_api_contract::threads::{ThreadSyncGet, ThreadSyncGetQuery};
use orch_api_contract::workspace::{JobGet, JobPhase, JobProgress, PhaseState};
use orch_db::queries::jobs::{self, NewJob};
use orch_types::{Hash, Time, new_id};
use serde_json::json;
use workspace::Site;

struct RestoreReader {
    disk: std::sync::Arc<workspace::Disk>,
    content: workspace::Content,
    manifest: Hash,
    reads: std::sync::atomic::AtomicUsize,
}

#[async_trait::async_trait]
impl orch_api_core::testing::Machine for RestoreReader {
    async fn run(
        &self,
        command: &orch_link_contract::commands::Cmd,
    ) -> Result<serde_json::Value, orch_link_contract::commands::CommandError> {
        use orch_link_contract::commands::{CommandError, CommandKind, ErrorCode};
        if command.kind != CommandKind::FsRestorePlan.as_str() {
            return self.disk.run(command).await;
        }
        let request: orch_link_contract::source::FsRestorePlan =
            serde_json::from_value(command.body.clone())
                .map_err(|error| CommandError::new(ErrorCode::Internal, error.to_string()))?;
        self.reads.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        let bytes = serde_json::to_vec(&json!({ "snapshot_id": request.snapshot_id, "target_manifest": self.manifest,
            "directory": request.directory, "generation": 1, "namespace_fingerprint": Hash::of(b"complete native namespace"),
            "ignore_fingerprint": Hash::of(b"current native coverage"),
            "current": [{ "p": "code", "t": "f", "x": 0o644, "h": Hash::of(b"later"), "l": null }] })).unwrap();
        let plan = self
            .content
            .upload(self.disk.machine, command.id, &bytes, workspace::PIECE)
            .await;
        Ok(
            json!({ "plan_blob": plan, "paths": [{ "path": "code", "action": "change", "expected": Hash::of(b"later"),
            "later_change": null, "later_writers": [] }], "writers_active": false, "undo_available": true }),
        )
    }
}

#[tokio::test]
async fn restore_preflight_shows_unknown_later_changes_and_rechecks_generation_and_expiry_before_any_apply()
 {
    use orch_api_contract::source::{
        EnvironmentFilesRestore, EnvironmentFilesRestoreBody, EnvironmentRestorePreflight,
        EnvironmentRestorePreflightBody,
    };
    use orch_blob_contract::store::stream_of;
    use orch_db::queries::{source, storage};
    use orch_link_contract::source::{FsRecordResult, RecordState};
    let mut site = Site::live().await;
    let now = site.world.now();
    let snapshot = new_id();
    let manifest = orch_engine::manifest::build(
        &orch_engine::manifest::Header {
            v: orch_engine::manifest::MANIFEST_VERSION,
            root: "/workspace/web".into(),
            generated_at: Time::new(now),
            generation: 1,
            case_sensitive: true,
        },
        vec![orch_engine::manifest::Entry {
            path: "code".into(),
            kind: orch_engine::manifest::EntryKind::File,
            size: 6,
            mtime_ns: 0,
            mode: 0o644,
            hash: Some(Hash::of(b"before")),
            link: None,
        }],
    )
    .unwrap();
    site.world
        .app
        .app
        .blobs
        .store()
        .put(
            &manifest.hash,
            Some(manifest.bytes.len() as i64),
            stream_of(vec![bytes::Bytes::copy_from_slice(&manifest.bytes)]),
        )
        .await
        .unwrap();
    let mut conn = site.world.conn().await;
    storage::register_blob(&mut conn, &manifest.hash, manifest.bytes.len() as i64, now)
        .await
        .unwrap();
    source::begin_recorded(
        &mut conn,
        &storage::NewSnapshot {
            id: snapshot,
            thread_id: Some(site.thread),
            environment_id: site.environment,
            env_generation: 1,
            label: "Before the turn",
            selection: &json!({ "include": ["web"], "exclude": [] }),
            created_by: Some(site.ada.id()),
            at: now,
        },
        "web",
        "turn",
    )
    .await
    .unwrap();
    source::finish_recorded(
        &mut conn,
        snapshot,
        &FsRecordResult {
            state: RecordState::Complete,
            tree_ref: Some("retained-native-root".into()),
            repository_head: None,
            manifest_blob: Some(manifest.hash),
            files: 1,
            bytes: 6,
            expires_at: Some(Time::new(now + chrono::Duration::days(7))),
            reason: None,
        },
        now,
    )
    .await
    .unwrap();
    storage::pin_snapshot(&mut conn, snapshot, &[manifest.hash])
        .await
        .unwrap();
    drop(conn);
    let reader = std::sync::Arc::new(RestoreReader {
        disk: site.disk.clone(),
        content: site.content.clone(),
        manifest: manifest.hash,
        reads: std::sync::atomic::AtomicUsize::new(0),
    });
    site.power_on_with(reader.clone());
    let request = EnvironmentRestorePreflightBody {
        snapshot_id: snapshot,
        options: None,
    };
    assert_eq!(
        site.app()
            .call(EnvironmentRestorePreflight, &[&site.environment])
            .token(&site.vic.token)
            .body(&request)
            .send()
            .await
            .problem()
            .code,
        ErrorCode::AuthzDenied
    );
    assert_eq!(
        reader.reads.load(std::sync::atomic::Ordering::SeqCst),
        0,
        "a reader sends no native file plan"
    );
    let plan = site
        .app()
        .call(EnvironmentRestorePreflight, &[&site.environment])
        .token(&site.ada.token)
        .body(&request)
        .send()
        .await
        .ok();
    assert_eq!(plan.snapshot.id, snapshot);
    assert_eq!(plan.directory, "web");
    assert!(plan.undo_available);
    assert_eq!(
        plan.paths[0].later_change, None,
        "missing turn-end coverage is unknown"
    );
    assert!(plan.writers.iter().any(|writer| {
        writer
            .thread
            .as_ref()
            .is_some_and(|thread| thread.id == site.thread)
    }));
    let mut conn = site.world.conn().await;
    sqlx::query("UPDATE environments SET generation = generation + 1 WHERE id = $1")
        .bind(site.environment)
        .execute(&mut *conn)
        .await
        .unwrap();
    drop(conn);
    let confirmation = EnvironmentFilesRestoreBody {
        preflight_id: plan.preflight_id,
    };
    assert_eq!(
        site.app()
            .call(EnvironmentFilesRestore, &[&site.environment])
            .token(&site.ada.token)
            .body(&confirmation)
            .send()
            .await
            .problem()
            .code,
        ErrorCode::WorkspaceRestoreConflict
    );
    let mut conn = site.world.conn().await;
    sqlx::query("UPDATE environments SET generation = 1 WHERE id = $1")
        .bind(site.environment)
        .execute(&mut *conn)
        .await
        .unwrap();
    sqlx::query("UPDATE snapshots SET expires_at = $2 WHERE id = $1")
        .bind(snapshot)
        .bind(now - chrono::Duration::seconds(1))
        .execute(&mut *conn)
        .await
        .unwrap();
    let count: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM jobs WHERE kind = 'workspace_restore' AND environment_id = $1",
    )
    .bind(site.environment)
    .fetch_one(&mut *conn)
    .await
    .unwrap();
    drop(conn);
    assert_eq!(count, 0);
    let expired = site
        .app()
        .call(EnvironmentFilesRestore, &[&site.environment])
        .token(&site.ada.token)
        .body(&confirmation)
        .send()
        .await
        .problem();
    assert_eq!(expired.code, ErrorCode::WorkspaceRestoreUnavailable);
    assert_eq!(expired.extra["reason"], "expired");
    assert_eq!(
        reader.reads.load(std::sync::atomic::Ordering::SeqCst),
        1,
        "confirmation never substitutes a new plan or applies the stale one"
    );
}

#[tokio::test]
async fn source_retry_keeps_the_original_repository_branch_native_identity_and_destination_when_defaults_change()
 {
    use orch_api_contract::source::{
        EnvironmentSourcePrepare, EnvironmentSourcePrepareBody, RepositorySource,
        SourceDestination, SourceInfo, SourceKind, SourceState,
    };
    use orch_engine::jobs::source::{Action, Plan};
    let site = Site::live().await;
    let mut conn = site.world.conn().await;
    let environment =
        orch_db::queries::environments::workspace::environment(&mut conn, site.environment)
            .await
            .unwrap()
            .unwrap();
    let thread = orch_db::queries::threads::thread(&mut conn, site.thread)
        .await
        .unwrap()
        .unwrap();
    let effect = new_id();
    let frozen = RepositorySource {
        repository_url: Some("https://code.example.test/old/repository.git".into()),
        branch: Some("old-main".into()),
        secret_name: None,
    };
    let info = SourceInfo {
        kind: SourceKind::Repository,
        directory: thread.workspace_dir.clone(),
        repository_url: frozen.repository_url.clone(),
        branch: frozen.branch.clone(),
        head: None,
        state: SourceState::Failed,
        job: None,
        reason: Some("workspace.source_failed".into()),
    };
    let original = Plan {
        environment: site.environment,
        generation: environment.generation,
        machine: environment.machine_id.unwrap(),
        thread: Some(site.thread),
        caller: site.ada.id(),
        token: None,
        action: Action::Prepare {
            directory: thread.workspace_dir,
            source: frozen.clone(),
            credential: None,
            destination: SourceDestination::MissingDirectory,
            effect_id: Some(effect),
        },
    };
    sqlx::query("UPDATE threads SET source = $2 WHERE node_id = $1")
        .bind(site.thread)
        .bind(json!({ "info": info, "source": frozen, "prepare": true, "native_plan": original }))
        .execute(&mut *conn)
        .await
        .unwrap();
    let resolved = orch_engine::jobs::source::resolved(&mut conn, site.thread)
        .await
        .unwrap();
    assert_ne!(resolved, frozen);
    drop(conn);
    let job = site
        .app()
        .call(EnvironmentSourcePrepare, &[&site.environment])
        .token(&site.ada.token)
        .body(&EnvironmentSourcePrepareBody {
            thread_id: site.thread,
            source: None,
        })
        .send()
        .await
        .ok();
    let mut conn = site.world.conn().await;
    let stored = jobs::job(&mut conn, job.id).await.unwrap().unwrap();
    let action = &stored.plan["action"];
    assert_eq!(action["source"], serde_json::to_value(&frozen).unwrap());
    assert_eq!(action["effect_id"], effect.to_string());
    assert_eq!(
        action["destination"],
        json!({ "kind": "missing_directory" })
    );
    assert_ne!(
        job.id, effect,
        "the new progress job retries one original native effect"
    );
}

#[tokio::test]
async fn commit_jobs_freeze_confirmed_identity_and_refuse_a_reader_or_traversal() {
    let site = Site::live().await;
    let body = EnvironmentGitCommitBody {
        path: "web".to_owned(),
        paths: vec!["src/code.rs".to_owned()],
        message: "A reviewed change".to_owned(),
        expected_head: None,
        expected_status: Hash::of(b"reviewed status"),
    };
    let refused = site
        .app()
        .call(EnvironmentGitCommit, &[&site.environment])
        .token(&site.vic.token)
        .query(&ThroughThreadQuery {
            thread: Some(site.thread),
        })
        .body(&body)
        .send()
        .await
        .problem();
    assert_eq!(refused.code, ErrorCode::AuthzDenied);
    let escaped = EnvironmentGitCommitBody {
        path: "../private".to_owned(),
        ..body.clone()
    };
    assert_eq!(
        site.app()
            .call(EnvironmentGitCommit, &[&site.environment])
            .token(&site.ada.token)
            .query(&ThroughThreadQuery {
                thread: Some(site.thread)
            })
            .body(&escaped)
            .send()
            .await
            .problem()
            .code,
        ErrorCode::NotFound
    );
    let job = site
        .app()
        .call(EnvironmentGitCommit, &[&site.environment])
        .token(&site.ada.token)
        .query(&ThroughThreadQuery {
            thread: Some(site.thread),
        })
        .body(&body)
        .send()
        .await
        .ok();
    assert_eq!(job.kind, "git_commit");
    assert_eq!(job.node_id, Some(site.thread));
    assert_eq!(job.environment_id, Some(site.environment));
    let mut conn = site.world.conn().await;
    let stored = jobs::job(&mut conn, job.id).await.unwrap().unwrap();
    let author = &stored.plan["action"]["body"]["author"];
    let person = orch_db::queries::identity::user_by_id(&mut conn, site.ada.id())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(author["name"], person.display_name);
    assert_eq!(
        author["email"],
        if person.email_confirmed() {
            person.email.unwrap()
        } else {
            format!("{}@orch.invalid", person.id)
        }
    );
    assert_eq!(
        stored.plan["action"]["body"]["expected_status"],
        json!(body.expected_status)
    );
    assert_eq!(stored.plan["action"]["body"]["paths"], json!(body.paths));
}

#[tokio::test]
async fn weaker_job_readers_get_no_file_result_error_log_or_transfer_counts() {
    let site = Site::live().await;
    let mut tx = site.world.app.app.db.begin().await.unwrap();
    let progress = JobProgress {
        activity: None,
        phase: "commit".to_owned(),
        phases: vec![JobPhase {
            name: "commit".to_owned(),
            weight: 100,
            state: PhaseState::Done,
        }],
        percent: 100,
        files_done: 7,
        files_total: Some(7),
        bytes_done: 19,
        bytes_total: Some(19),
        rate_bps: None,
        eta_seconds: None,
        message: Some("private/file".to_owned()),
    };
    let mut value = serde_json::to_value(progress).unwrap();
    value["log"] = json!("PRIVATE_PROGRAM_LOG");
    let job = jobs::insert(
        &mut tx,
        &NewJob {
            id: new_id(),
            kind: jobs::kind::GIT_COMMIT,
            node_id: Some(site.thread),
            environment_id: Some(site.environment),
            provider_id: None,
            plan: &json!({}),
            progress: &value,
            created_by: Some(site.ada.id()),
        },
        site.world.now(),
    )
    .await
    .unwrap();
    jobs::finish(&mut tx, job.id, "failed", &value, Some(&json!({ "private/file": "PRIVATE_FILE_RESULT" })),
        Some(&json!({ "code": "git.failed", "message": "PRIVATE_FAILURE", "phase": "commit", "undone": true })), site.world.now()).await.unwrap();
    tx.commit().await.unwrap();
    // A reader of the thread who holds nothing of its environment. (A Viewer of the org holds the files
    // of the environments in it, as shipped, and reads the whole job.)
    {
        let mut conn = site.world.conn().await;
        orch_db::queries::access::testing::add_entry(
            &mut conn,
            &orch_db::queries::access::PageKey::new("node", site.thread, "live"),
            "any",
            "thread.view",
            ("user", Some(site.eve.id()), None),
            None,
            site.world.now(),
        )
        .await
        .unwrap();
    }
    let weak = site
        .app()
        .call(JobGet, &[&job.id])
        .token(&site.eve.token)
        .send()
        .await
        .ok();
    assert!(
        weak.result.is_none()
            && weak.error.is_none()
            && weak.log.is_none()
            && weak.created_by.is_none()
    );
    assert_eq!(weak.progress.percent, 100);
    assert_eq!(weak.progress.phase, "commit");
    assert_eq!(weak.progress.files_done, 0);
    assert_eq!(weak.progress.bytes_done, 0);
    assert!(weak.progress.message.is_none());
    assert!(!serde_json::to_string(&weak).unwrap().contains("PRIVATE"));
    let full = site
        .app()
        .call(JobGet, &[&job.id])
        .token(&site.ada.token)
        .send()
        .await
        .ok();
    assert!(
        full.result
            .unwrap()
            .to_string()
            .contains("PRIVATE_FILE_RESULT")
    );
    assert!(full.log.unwrap().contains("PRIVATE_PROGRAM_LOG"));
}

#[tokio::test]
async fn tree_topic_and_finder_show_independent_nullable_flags_and_true_only_counts() {
    let site = Site::live().await;
    let now = Time::new(site.world.now());
    let mut conn = site.world.conn().await;
    sqlx::query("UPDATE threads SET work_observed = $2 WHERE node_id = $1").bind(site.thread)
        .bind(json!({ "uncommitted": true, "unpushed": null, "changed_files": 1, "ahead": null, "as_of": now, "stale": false, "reason": "upstream_unknown" }))
        .execute(&mut *conn).await.unwrap();
    drop(conn);
    let thread = site
        .app()
        .call(ThreadSyncGet, &[&site.thread])
        .token(&site.ada.token)
        .query(&ThreadSyncGetQuery::default())
        .send()
        .await
        .ok()
        .thread;
    assert_eq!(thread.attention.uncommitted, Some(true));
    assert_eq!(thread.attention.unpushed, None);
    let tree = site
        .app()
        .call(OrgTreeGet, &[&site.org])
        .token(&site.ada.token)
        .query(&OrgTreeGetQuery::default())
        .send()
        .await
        .ok();
    let row = tree
        .nodes
        .iter()
        .filter_map(|node| node.as_visible())
        .find(|node| node.id == site.thread)
        .unwrap();
    assert_eq!(row.thread.as_ref().unwrap().attention, thread.attention);
    let topic = site
        .app()
        .call(NodeGet, &[&site.topic])
        .token(&site.ada.token)
        .send()
        .await
        .ok();
    let Node::Topic(topic) = topic else {
        panic!("a topic was expected")
    };
    assert_eq!(topic.work_attention.uncommitted, 1);
    assert_eq!(topic.work_attention.unpushed, 0);
    assert_eq!(topic.work_attention.failed, 0);
    let names = site
        .app()
        .call(NameSearchList, &[])
        .token(&site.ada.token)
        .query(&NameSearchListQuery {
            starred: None,
            participating: None,
            q: Some(thread.base.name.clone()),
            kinds: Some("thread".to_owned()),
            org_id: Some(site.org),
            limit: None,
        })
        .send()
        .await
        .ok();
    let hit = names.iter().find(|hit| hit.id == site.thread).unwrap();
    assert_eq!(hit.thread_attention.as_ref().unwrap(), &thread.attention);
    // The runtime can still say running after its machine disconnects. Every projection
    // retains these independent last-known flags and marks their source stale immediately.
    site.power_off().await;
    let offline = site
        .app()
        .call(ThreadSyncGet, &[&site.thread])
        .token(&site.ada.token)
        .query(&ThreadSyncGetQuery::default())
        .send()
        .await
        .ok()
        .thread;
    assert!(offline.attention.stale);
    assert_eq!(offline.attention.reason.as_deref(), Some("source_offline"));
    assert_eq!(offline.attention.uncommitted, Some(true));
    assert_eq!(offline.attention.unpushed, None);
    let names = site
        .app()
        .call(NameSearchList, &[])
        .token(&site.ada.token)
        .query(&NameSearchListQuery {
            starred: None,
            participating: None,
            q: Some(thread.base.name.clone()),
            kinds: Some("thread".to_owned()),
            org_id: Some(site.org),
            limit: None,
        })
        .send()
        .await
        .ok();
    assert_eq!(
        names
            .iter()
            .find(|hit| hit.id == site.thread)
            .unwrap()
            .thread_attention
            .as_ref(),
        Some(&offline.attention)
    );
}
