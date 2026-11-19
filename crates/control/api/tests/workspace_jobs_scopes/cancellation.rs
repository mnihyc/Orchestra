//! Actual cancellation effects, native source alternatives, waits and optional response content.
use super::*;
use orch_api_contract::workspace::JobCancel;
use orch_api_core::testing::RawReply;
use orch_db::queries::link;

fn manage(id: Id) -> Json {
    subject("JobCancel.lifecycle", "job", id, "manage")
}
fn environment(site: &Site) -> Json {
    subject(
        "JobCancel.lifecycle",
        "environment",
        site.environment,
        "manage",
    )
}
fn read(id: Id) -> Json {
    subject("JobCancel.lifecycle", "job", id, "read")
}

async fn cancel(site: &Site, id: Id, token: &str) -> RawReply {
    let call = site.app().call(JobCancel, &[&id]).token(token);
    call.send_bytes().await
}
fn body(reply: RawReply, status: StatusCode) -> Json {
    assert_eq!(
        reply.status,
        status,
        "{}",
        String::from_utf8_lossy(&reply.raw)
    );
    serde_json::from_slice(&reply.raw).unwrap()
}
fn receipt(reply: RawReply, id: Id, asked: bool) {
    assert_eq!(
        body(reply, StatusCode::OK),
        json!({"projection":"receipt","job_id":id,"cancel_requested":asked})
    );
}
async fn row(site: &Site, id: Id) -> jobs::JobRow {
    jobs::job(&mut *site.world.conn().await, id)
        .await
        .unwrap()
        .unwrap()
}
async fn stamp(site: &Site, id: Id) -> String {
    sqlx::query_scalar("SELECT xmin::text FROM jobs WHERE id=$1")
        .bind(id)
        .fetch_one(&site.world.db.pool)
        .await
        .unwrap()
}
async fn command(site: &Site, job: Id) -> Id {
    let id = new_id();
    link::insert_command(
        &mut *site.world.conn().await,
        &link::NewCommand {
            id,
            machine_id: site.machine,
            kind: "fs.copy",
            body: &json!({"work":{"job_id":job}}),
            serial_key: None,
            deadline_at: site.world.now() + chrono::Duration::hours(1),
        },
        site.world.now(),
    )
    .await
    .unwrap();
    id
}
async fn wait(site: &Site, pid: i32, pattern: &str, request: &tokio::task::JoinHandle<RawReply>) {
    tokio::time::timeout(WAIT, async {
        loop {
            assert!(!request.is_finished(), "cancellation must reach its actual native wait");
            let blocked: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM pg_stat_activity WHERE datname=current_database() AND wait_event_type='Lock' AND $1=ANY(pg_blocking_pids(pid)) AND query LIKE $2)")
                .bind(pid).bind(pattern).fetch_one(&site.world.db.pool).await.unwrap();
            if blocked { break; } tokio::task::yield_now().await;
        }
    }).await.expect("observe the literal cancellation lock and blocking PID");
}

#[tokio::test]
async fn lifecycle_cancellation_uses_its_native_subject_and_never_borrows_job_get_read() {
    let site = site().await;
    let id = seed(
        &site,
        jobs::kind::ENVIRONMENT_RESTART,
        Some(site.thread),
        Some(site.environment),
        Some(site.provider),
        json!({}),
    )
    .await;
    let original = stamp(&site, id).await;
    for rules in [
        vec![manage(id)],
        vec![
            manage(id),
            subject("JobCancel.lifecycle", "thread", site.thread, "manage"),
        ],
        vec![
            subject("JobCancel.lifecycle", "job", id, "write"),
            environment(&site),
        ],
    ] {
        let key = keys::scoped(&site, site.ada.id(), rules).await;
        body(cancel(&site, id, &key.secret).await, StatusCode::FORBIDDEN);
        assert_eq!(stamp(&site, id).await, original);
    }
    let stranger = keys::scoped(&site, site.vic.id(), vec![manage(id), environment(&site)]).await;
    body(
        cancel(&site, id, &stranger.secret).await,
        StatusCode::FORBIDDEN,
    );
    assert!(!row(&site, id).await.cancel_requested());
    let key = keys::scoped(
        &site,
        site.ada.id(),
        vec![
            manage(id),
            environment(&site),
            read_job(id),
            subject("JobGet.ordinary", "environment", site.environment, "read"),
        ],
    )
    .await;
    receipt(cancel(&site, id, &key.secret).await, id, true);
    let stopped = row(&site, id).await;
    assert_eq!(stopped.state, "queued");
    assert_eq!(stopped.plan["cancel"]["by"], site.ada.id().to_string());
    let full = keys::scoped(
        &site,
        site.ada.id(),
        vec![manage(id), environment(&site), read(id)],
    )
    .await;
    let keyed = body(cancel(&site, id, &full.secret).await, StatusCode::OK);
    let ordinary = body(cancel(&site, id, &site.ada.token).await, StatusCode::OK);
    assert_eq!(keyed, ordinary);
    assert_eq!(
        keyed["result"]["native_result"],
        "Earlier native Job result"
    );
    assert_eq!(
        row(&site, id).await.plan["cancel"],
        stopped.plan["cancel"],
        "a repeated request keeps the original cancellation author/time"
    );
}

#[tokio::test]
async fn creator_cancellation_needs_no_invented_parent_manage_and_keeps_native_ownership() {
    let site = site().await;
    for kind in [
        orch_engine::jobs::cleanup::KIND,
        orch_engine::resource_pause::KIND,
        jobs::kind::EXPORT,
        jobs::kind::IMPORT,
        orch_engine::jobs::document_notices::KIND,
    ] {
        let id = seed(
            &site,
            kind,
            Some(site.thread),
            Some(site.environment),
            Some(site.provider),
            json!({}),
        )
        .await;
        let alien = keys::scoped(&site, site.eve.id(), vec![manage(id)]).await;
        body(
            cancel(&site, id, &alien.secret).await,
            StatusCode::NOT_FOUND,
        );
        assert!(!row(&site, id).await.cancel_requested());
        let key = keys::scoped(&site, site.ada.id(), vec![manage(id)]).await;
        receipt(cancel(&site, id, &key.secret).await, id, true);
    }
}

#[tokio::test]
async fn cancellation_machine_names_require_its_own_optional_source_read() {
    let site = site().await;
    let id = seed(
        &site,
        jobs::kind::ENVIRONMENT_RESTART,
        None,
        Some(site.environment),
        None,
        json!({"active_command":{"machine":site.machine}}),
    )
    .await;
    let host = orch_db::queries::resources::hosting::host_of_machine(
        &mut *site.world.conn().await,
        site.machine,
    )
    .await
    .unwrap();
    let (kind, machine) = host.map_or(("machine", site.machine), |host| ("host", host));
    for extra in [
        Vec::new(),
        vec![subject("JobGet.ordinary", kind, machine, "read")],
        vec![subject("JobCancel.lifecycle", kind, machine, "read")],
    ] {
        let named = extra
            .first()
            .is_some_and(|rule| rule["allow"][0]["operation"] == "JobCancel.lifecycle");
        let key = keys::scoped(
            &site,
            site.ada.id(),
            vec![manage(id), environment(&site), read(id)]
                .into_iter()
                .chain(extra)
                .collect(),
        )
        .await;
        let shown = body(cancel(&site, id, &key.secret).await, StatusCode::OK);
        assert_eq!(
            shown["result"]["native_result"],
            "Earlier native Job result"
        );
        if named {
            assert_eq!(
                shown["progress"]["activity"]["waiting_for"]["name"],
                "workspace-machine"
            );
        } else {
            assert!(shown["progress"]["activity"]["waiting_for"].is_null());
        }
    }
}

#[tokio::test]
async fn continuation_cancellation_selects_stored_intent_and_requires_real_interact_without_new_provider_use()
 {
    let site = site().await;
    let id = seed(
        &site,
        orch_db::queries::thread_continuations::JOB_KIND,
        Some(site.thread),
        Some(site.environment),
        Some(site.provider),
        json!({}),
    )
    .await;
    let job = subject("JobCancel.continuation", "job", id, "manage");
    for rules in [
        vec![manage(id), environment(&site)],
        vec![
            job.clone(),
            subject("JobCancel.continuation", "thread", site.thread, "manage"),
        ],
        vec![job.clone()],
    ] {
        let key = keys::scoped(&site, site.ada.id(), rules).await;
        body(cancel(&site, id, &key.secret).await, StatusCode::FORBIDDEN);
        assert!(!row(&site, id).await.cancel_requested());
    }
    let rules = vec![
        job,
        subject("JobCancel.continuation", "thread", site.thread, "write"),
    ];
    let viewer = keys::scoped(&site, site.vic.id(), rules.clone()).await;
    body(
        cancel(&site, id, &viewer.secret).await,
        StatusCode::FORBIDDEN,
    );
    let key = keys::scoped(&site, site.ada.id(), rules).await;
    receipt(cancel(&site, id, &key.secret).await, id, true);
}

#[tokio::test]
async fn cancellation_preserves_ended_noop_committed_and_removal_boundaries() {
    let site = site().await;
    for (kind, ended, committed) in [
        (jobs::kind::ENVIRONMENT_RESTART, true, false),
        (jobs::kind::ENVIRONMENT_RESTART, false, true),
        (jobs::kind::ENVIRONMENT_REMOVE, false, false),
        (jobs::retained::PURGE, false, false),
    ] {
        let id = seed(
            &site,
            kind,
            None,
            Some(site.environment),
            None,
            json!({"committed":committed}),
        )
        .await;
        if ended {
            sqlx::query("UPDATE jobs SET state='succeeded',finished_at=$2 WHERE id=$1")
                .bind(id)
                .bind(site.world.now())
                .execute(&site.world.db.pool)
                .await
                .unwrap();
        }
        let before = stamp(&site, id).await;
        let key = keys::scoped(&site, site.ada.id(), vec![manage(id), environment(&site)]).await;
        if ended {
            receipt(cancel(&site, id, &key.secret).await, id, false);
        } else {
            assert_eq!(
                body(cancel(&site, id, &key.secret).await, StatusCode::CONFLICT)["code"],
                "plan.blocked"
            );
        }
        assert_eq!(stamp(&site, id).await, before);
        assert!(!row(&site, id).await.cancel_requested());
    }
}

#[tokio::test]
async fn cancellation_checks_expiry_after_actual_thread_job_and_command_waits_before_any_stamp() {
    for stage in ["thread", "job", "command"] {
        for expired in [false, true] {
            let site = site().await;
            let kind = if stage == "thread" {
                jobs::kind::SNAPSHOT
            } else {
                jobs::kind::ENVIRONMENT_RESTART
            };
            let id = seed(
                &site,
                kind,
                Some(site.thread),
                Some(site.environment),
                None,
                json!({}),
            )
            .await;
            let command = command(&site, id).await;
            let key =
                keys::scoped(&site, site.ada.id(), vec![manage(id), environment(&site)]).await;
            let before = stamp(&site, id).await;
            let mut blocker = site.world.db.pool.begin().await.unwrap();
            let (sql, target, pattern) = match stage {
                "thread" => (
                    "SELECT node_id FROM threads WHERE node_id=$1 FOR UPDATE",
                    site.thread,
                    "%FROM threads%FOR UPDATE%",
                ),
                "job" => (
                    "SELECT id FROM jobs WHERE id=$1 FOR UPDATE",
                    id,
                    "%FROM jobs%FOR UPDATE%",
                ),
                _ => (
                    "SELECT id FROM commands WHERE id=$1 FOR UPDATE",
                    command,
                    "%FROM commands%FOR UPDATE%",
                ),
            };
            sqlx::query(sql)
                .bind(target)
                .execute(&mut *blocker)
                .await
                .unwrap();
            let pid: i32 = sqlx::query_scalar("SELECT pg_backend_pid()")
                .fetch_one(&mut *blocker)
                .await
                .unwrap();
            let app = site.app().clone();
            let request = tokio::spawn(async move {
                let call = app.call(JobCancel, &[&id]).token(&key.secret);
                call.send_bytes().await
            });
            wait(&site, pid, pattern, &request).await;
            assert_eq!(stamp(&site, id).await, before);
            assert_eq!(
                link::command(&mut *site.world.conn().await, command)
                    .await
                    .unwrap()
                    .unwrap()
                    .state,
                "queued"
            );
            if expired {
                site.world.advance(chrono::Duration::hours(2));
            }
            blocker.commit().await.unwrap();
            let reply = tokio::time::timeout(WAIT, request).await.unwrap().unwrap();
            if expired {
                body(reply, StatusCode::UNAUTHORIZED);
                assert_eq!(stamp(&site, id).await, before);
                assert_eq!(
                    link::command(&mut *site.world.conn().await, command)
                        .await
                        .unwrap()
                        .unwrap()
                        .state,
                    "queued"
                );
            } else {
                receipt(reply, id, true);
                assert!(row(&site, id).await.cancel_requested());
                assert_eq!(
                    link::command(&mut *site.world.conn().await, command)
                        .await
                        .unwrap()
                        .unwrap()
                        .state,
                    "cancelled"
                );
            }
        }
    }
}

#[tokio::test]
async fn cancellation_reloads_the_current_workspace_binding_after_its_real_thread_wait() {
    let site = site().await;
    let id = seed(
        &site,
        jobs::kind::SNAPSHOT,
        Some(site.thread),
        Some(site.environment),
        None,
        json!({}),
    )
    .await;
    let key = keys::scoped(&site, site.ada.id(), vec![manage(id), environment(&site)]).await;
    let mut blocker = site.world.db.pool.begin().await.unwrap();
    sqlx::query("SELECT node_id FROM threads WHERE node_id=$1 FOR UPDATE")
        .bind(site.thread)
        .execute(&mut *blocker)
        .await
        .unwrap();
    let pid: i32 = sqlx::query_scalar("SELECT pg_backend_pid()")
        .fetch_one(&mut *blocker)
        .await
        .unwrap();
    let app = site.app().clone();
    let request = tokio::spawn(async move {
        let call = app.call(JobCancel, &[&id]).token(&key.secret);
        call.send_bytes().await
    });
    wait(&site, pid, "%FROM threads%FOR UPDATE%", &request).await;
    sqlx::query("UPDATE threads SET environment_id=NULL WHERE node_id=$1")
        .bind(site.thread)
        .execute(&mut *blocker)
        .await
        .unwrap();
    blocker.commit().await.unwrap();
    let reply = tokio::time::timeout(WAIT, request).await.unwrap().unwrap();
    assert!(matches!(
        reply.status,
        StatusCode::NOT_FOUND | StatusCode::FORBIDDEN
    ));
    assert!(
        !row(&site, id).await.cancel_requested(),
        "stored job.environment_id cannot lend the Thread's old Files authority"
    );
}

#[tokio::test]
async fn cancellation_rechecks_current_operation_scope_after_correlated_command_wait() {
    let site = site().await;
    let id = seed(
        &site,
        jobs::kind::ENVIRONMENT_RESTART,
        None,
        Some(site.environment),
        None,
        json!({}),
    )
    .await;
    let command = command(&site, id).await;
    let key = keys::scoped(&site, site.ada.id(), vec![manage(id), environment(&site)]).await;
    let mut blocker = site.world.db.pool.begin().await.unwrap();
    sqlx::query("SELECT id FROM commands WHERE id=$1 FOR UPDATE")
        .bind(command)
        .execute(&mut *blocker)
        .await
        .unwrap();
    let pid: i32 = sqlx::query_scalar("SELECT pg_backend_pid()")
        .fetch_one(&mut *blocker)
        .await
        .unwrap();
    let app = site.app().clone();
    let token = key.secret.clone();
    let request = tokio::spawn(async move {
        let call = app.call(JobCancel, &[&id]).token(&token);
        call.send_bytes().await
    });
    wait(&site, pid, "%FROM commands%FOR UPDATE%", &request).await;
    sqlx::query("UPDATE api_tokens SET operation_scope=$2 WHERE id=$1").bind(key.id).bind(json!({"rules":[manage(id),subject("JobCancel.lifecycle","environment",site.environment,"read")]})).execute(&site.world.db.pool).await.unwrap();
    blocker.commit().await.unwrap();
    body(
        tokio::time::timeout(WAIT, request).await.unwrap().unwrap(),
        StatusCode::FORBIDDEN,
    );
    assert!(!row(&site, id).await.cancel_requested());
    assert_eq!(
        link::command(&mut *site.world.conn().await, command)
            .await
            .unwrap()
            .unwrap()
            .state,
        "queued"
    );
}

#[tokio::test]
async fn continuation_snapshot_trigger_is_locked_before_cancellation_authority_cut() {
    use orch_db::queries::{thread_continuations as continuations, threads};
    for expired in [false, true] {
        let site = site().await;
        let id = seed(
            &site,
            continuations::JOB_KIND,
            Some(site.thread),
            Some(site.environment),
            None,
            json!({}),
        )
        .await;
        let thread = threads::thread(&mut *site.world.conn().await, site.thread)
            .await
            .unwrap()
            .unwrap();
        let progress = row(&site, id).await.progress;
        let snapshot = json!({"id":id,"kind":continuations::JOB_KIND,"state":"queued","phase":progress["phase"].as_str().unwrap_or("preparing"),"percent":progress["percent"].as_i64().unwrap_or(0)});
        continuations::create_operation(
            &mut *site.world.conn().await,
            &continuations::OperationRow {
                id,
                job_id: Some(id),
                job_snapshot: snapshot.clone(),
                thread_id: site.thread,
                actor_id: site.ada.id(),
                client_id: id.to_string(),
                body_hash: orch_types::Hash::of(b"native cancellation trigger control")
                    .as_bytes()
                    .to_vec(),
                kind: "activate".into(),
                source_continuation_id: thread.active_continuation_id,
                target_continuation_id: thread.active_continuation_id,
                input_id: None,
                segment_id: thread.current_segment_id.unwrap_or_else(new_id),
                expected_active_continuation_id: thread.active_continuation_id,
                expected_execution_revision: thread.execution_revision,
                expected_queue_revision: thread.queue_revision,
                status: "preparing".into(),
                activation_seq: None,
                created_at: site.world.now(),
            },
        )
        .await
        .unwrap();
        let key = keys::scoped(
            &site,
            site.ada.id(),
            vec![
                subject("JobCancel.continuation", "job", id, "manage"),
                subject("JobCancel.continuation", "thread", site.thread, "write"),
            ],
        )
        .await;
        let before = stamp(&site, id).await;
        let mut blocker = site.world.db.pool.begin().await.unwrap();
        sqlx::query("SELECT id FROM thread_continuation_operations WHERE id=$1 FOR UPDATE")
            .bind(id)
            .execute(&mut *blocker)
            .await
            .unwrap();
        let pid: i32 = sqlx::query_scalar("SELECT pg_backend_pid()")
            .fetch_one(&mut *blocker)
            .await
            .unwrap();
        let app = site.app().clone();
        let request = tokio::spawn(async move {
            let call = app.call(JobCancel, &[&id]).token(&key.secret);
            call.send_bytes().await
        });
        wait(
            &site,
            pid,
            "%FROM thread_continuation_operations%FOR UPDATE%",
            &request,
        )
        .await;
        assert_eq!(stamp(&site, id).await, before);
        if expired {
            site.world.advance(chrono::Duration::hours(2));
        }
        blocker.commit().await.unwrap();
        let reply = tokio::time::timeout(WAIT, request).await.unwrap().unwrap();
        if expired {
            body(reply, StatusCode::UNAUTHORIZED);
            assert_eq!(stamp(&site, id).await, before);
        } else {
            receipt(reply, id, true);
            assert!(row(&site, id).await.cancel_requested());
        }
        assert_eq!(
            continuations::operation(&mut *site.world.conn().await, id)
                .await
                .unwrap()
                .unwrap()
                .job_snapshot,
            snapshot
        );
    }
}

#[tokio::test]
async fn completed_cancellation_keeps_its_receipt_or_current_read_after_actual_activity_wait() {
    for change in ["manage", "read", "expiry", "association"] {
        let site = site().await;
        let id = seed(
            &site,
            jobs::kind::ENVIRONMENT_RESTART,
            Some(site.thread),
            Some(site.environment),
            None,
            json!({}),
        )
        .await;
        let command = command(&site, id).await;
        let key = keys::scoped(
            &site,
            site.ada.id(),
            vec![manage(id), environment(&site), read(id)],
        )
        .await;
        let mut blocker = site.world.db.pool.begin().await.unwrap();
        sqlx::query("LOCK TABLE api_token_activity IN SHARE MODE")
            .execute(&mut *blocker)
            .await
            .unwrap();
        let pid: i32 = sqlx::query_scalar("SELECT pg_backend_pid()")
            .fetch_one(&mut *blocker)
            .await
            .unwrap();
        let app = site.app().clone();
        let token = key.secret.clone();
        let request = tokio::spawn(async move {
            let call = app.call(JobCancel, &[&id]).token(&token);
            call.send_bytes().await
        });
        activity_wait(&site, pid, &request).await;
        let accepted = row(&site, id).await;
        assert!(accepted.cancel_requested());
        let before = stamp(&site, id).await;
        assert_eq!(
            link::command(&mut *site.world.conn().await, command)
                .await
                .unwrap()
                .unwrap()
                .state,
            "cancelled"
        );
        match change {
            "expiry" => site.world.advance(chrono::Duration::hours(2)),
            "association" => {
                sqlx::query("UPDATE jobs SET node_id=NULL WHERE id=$1")
                    .bind(id)
                    .execute(&site.world.db.pool)
                    .await
                    .unwrap();
            }
            _ => {
                let rules = if change == "manage" {
                    vec![read(id)]
                } else {
                    vec![manage(id), environment(&site)]
                };
                sqlx::query("UPDATE api_tokens SET operation_scope=$2 WHERE id=$1")
                    .bind(key.id)
                    .bind(json!({"rules":rules}))
                    .execute(&site.world.db.pool)
                    .await
                    .unwrap();
            }
        }
        let after_change = stamp(&site, id).await;
        blocker.commit().await.unwrap();
        let reply = tokio::time::timeout(WAIT, request).await.unwrap().unwrap();
        if change == "manage" {
            assert_eq!(
                body(reply, StatusCode::OK)["result"]["native_result"],
                "Earlier native Job result",
                "completed Manage is never re-admitted for a current same-operation Read"
            );
        } else {
            receipt(reply, id, true);
        }
        assert_eq!(
            stamp(&site, id).await,
            after_change,
            "publication never replays the writer"
        );
        if change != "association" {
            assert_eq!(after_change, before);
        }
        assert_eq!(row(&site, id).await.plan["cancel"], accepted.plan["cancel"]);
    }
}
