//! Native feed member bodies, exact scopes and frozen publication selections.

use super::*;
use orch_api_contract::{
    common::{ErrorCode, Problem},
    environment::{EnvironmentSyncGet, EnvironmentSyncGetQuery, EnvironmentSyncGetReply},
    resource_facts::DependentRelation,
};
use orch_db::queries::jobs::{self, NewJob, kind};

const OPERATION: &str = "EnvironmentSyncGet";
fn rules(s: &Scene, threads: bool) -> Vec<Json> {
    let mut sources = s.sources(OPERATION);
    if !threads {
        sources["resources"]
            .as_array_mut()
            .unwrap()
            .retain(|value| value != "thread");
    }
    vec![
        rule(
            OPERATION,
            "environment",
            s.first.base.id,
            "self",
            &["environment"],
            &["read"],
        ),
        sources,
    ]
}
async fn call(s: &Scene, token: &str, since: Option<i64>) -> RawReply {
    let call = s.world.app.call(EnvironmentSyncGet, &[&s.first.base.id]);
    call.token(token)
        .query(&EnvironmentSyncGetQuery { since })
        .send_bytes()
        .await
}
fn body(reply: RawReply) -> EnvironmentSyncGetReply {
    assert_eq!(
        reply.status,
        StatusCode::OK,
        "{}",
        String::from_utf8_lossy(&reply.raw)
    );
    serde_json::from_slice(&reply.raw).unwrap()
}
async fn job(s: &Scene, machine: Option<Id>) -> Id {
    let mut tx = s.world.db.pool.begin().await.unwrap();
    let plan = machine.map_or_else(
        || json!({}),
        |machine| json!({"active_command":{"machine":machine}}),
    );
    let row = jobs::insert(
        &mut tx,
        &NewJob {
            id: new_id(),
            kind: kind::ENVIRONMENT_RECREATE,
            node_id: None,
            environment_id: Some(s.first.base.id),
            provider_id: None,
            plan: &plan,
            progress: &json!({}),
            created_by: Some(s.team.c.admin.id()),
        },
        s.world.now(),
    )
    .await
    .unwrap();
    tx.commit().await.unwrap();
    row.id
}
async fn activity_wait(s: &Scene, pid: i32, request: &tokio::task::JoinHandle<RawReply>) {
    tokio::time::timeout(std::time::Duration::from_secs(30),async {
        loop {
            assert!(!request.is_finished(),"the actual sync publication must reach activity INSERT");
            let waiting:bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM pg_stat_activity WHERE datname=current_database() AND wait_event_type='Lock' AND $1=ANY(pg_blocking_pids(pid)) AND query LIKE '%INSERT INTO api_token_activity%')")
                .bind(pid).fetch_one(&s.world.db.pool).await.unwrap();
            if waiting {break;} tokio::task::yield_now().await;
        }
    }).await.expect("the actual environment feed bookkeeping wait");
}

#[tokio::test]
async fn sync_keeps_native_members_with_exact_operation_and_foreign_sources_and_bounds_waiting_names()
 {
    let s = Scene::new().await;
    let machine = orch_db::queries::resources::access_fixtures::insert_persons_machine(
        &mut *s.world.conn().await,
        s.team.c.admin.id(),
        "native-sync-extra-machine-marker",
    )
    .await
    .unwrap();
    job(&s, Some(machine)).await;
    let mut full = rules(&s, true);
    full.push(rule(
        OPERATION,
        "machine",
        machine,
        "self",
        &["machine"],
        &["read"],
    ));
    let reader = key(&s.world, s.team.c.admin.id(), full).await;
    let native = body(call(&s, &s.team.c.admin.token, None).await);
    assert!(
        serde_json::to_string(&native)
            .unwrap()
            .contains("native-sync-extra-machine-marker")
    );
    assert_eq!(body(call(&s, &reader.secret, None).await), native);
    let limited = key(&s.world, s.team.c.admin.id(), rules(&s, true)).await;
    let narrow = body(call(&s, &limited.secret, None).await);
    assert!(
        !serde_json::to_string(&narrow)
            .unwrap()
            .contains("native-sync-extra-machine-marker")
    );
    assert_eq!(narrow.environment, native.environment);
    for policies in [
        vec![
            rule(
                "EnvironmentGet.read",
                "environment",
                s.first.base.id,
                "self",
                &["environment"],
                &["read"],
            ),
            s.sources("EnvironmentGet.read"),
        ],
        vec![
            rule(
                OPERATION,
                "environment",
                s.first.base.id,
                "self",
                &["environment"],
                &["manage"],
            ),
            s.sources(OPERATION),
        ],
        vec![rule(
            OPERATION,
            "environment",
            s.first.base.id,
            "self",
            &["environment"],
            &["read"],
        )],
    ] {
        let denied = key(&s.world, s.team.c.admin.id(), policies).await;
        assert_eq!(
            call(&s, &denied.secret, None).await.status,
            StatusCode::FORBIDDEN
        );
    }
    let stranger = key(&s.world, s.team.c.stranger.id(), rules(&s, true)).await;
    assert_eq!(
        call(&s, &stranger.secret, None).await.status,
        StatusCode::FORBIDDEN
    );
}

#[tokio::test]
async fn sync_library_names_use_the_actual_binding_thread_scope_without_changing_native_counts() {
    let s = Scene::new().await;
    let thread = new_id();
    let mut conn = s.world.conn().await;
    orch_db::queries::hierarchy::testing::insert_node(
        &mut conn,
        thread,
        "thread",
        Some(s.team.topic),
        "sync-private-library-thread-marker",
        s.team.c.admin.id(),
    )
    .await
    .unwrap();
    sqlx::query("UPDATE threads SET environment_id=$2 WHERE node_id=$1")
        .bind(thread)
        .bind(s.first.base.id)
        .execute(&mut *conn)
        .await
        .unwrap();
    drop(conn);
    let native = body(call(&s, &s.team.c.admin.token, None).await);
    let native = native
        .library
        .as_ref()
        .unwrap()
        .dependents
        .iter()
        .find(|group| group.relation == DependentRelation::Binding)
        .unwrap();
    assert!(
        native
            .visible
            .iter()
            .any(|edge| edge.name == "sync-private-library-thread-marker")
    );
    let limited = key(&s.world, s.team.c.admin.id(), rules(&s, false)).await;
    let narrow = body(call(&s, &limited.secret, None).await);
    let narrow = narrow
        .library
        .as_ref()
        .unwrap()
        .dependents
        .iter()
        .find(|group| group.relation == DependentRelation::Binding)
        .unwrap();
    assert_eq!(narrow.count, native.count);
    assert!(narrow.visible.is_empty() && narrow.hidden >= 1);
    let full = key(&s.world, s.team.c.admin.id(), rules(&s, true)).await;
    assert!(
        serde_json::to_string(&body(call(&s, &full.secret, None).await))
            .unwrap()
            .contains("sync-private-library-thread-marker")
    );
}

#[tokio::test]
async fn sync_preserves_native_sequence_complete_since_and_overflow_semantics() {
    let s = Scene::new().await;
    for _ in 0..205 {
        job(&s, None).await;
    }
    let reader = key(&s.world, s.team.c.admin.id(), rules(&s, true)).await;
    for since in [None, Some(0)] {
        assert_eq!(
            body(call(&s, &reader.secret, since).await),
            body(call(&s, &s.team.c.admin.token, since).await)
        );
    }
    let current = body(call(&s, &reader.secret, None).await);
    assert_eq!(current.seq, current.environment.seq);
    let overflow = body(call(&s, &reader.secret, Some(0)).await);
    assert!(!overflow.complete && overflow.jobs.is_empty());
    let ahead = body(call(&s, &reader.secret, Some(current.seq + 100)).await);
    assert!(!ahead.complete && ahead.jobs.is_empty());
    for token in [&reader.secret, &s.team.c.admin.token] {
        let invalid = call(&s, token, Some(-1)).await;
        assert_eq!(invalid.status, StatusCode::BAD_REQUEST);
        let problem: Problem = serde_json::from_slice(&invalid.raw).unwrap();
        assert_eq!(problem.code, ErrorCode::RequestInvalid);
        assert_eq!(problem.extra["errors"][0]["path"], "since");
    }
}

#[tokio::test]
async fn sync_keeps_native_through_thread_progress_and_no_library_door() {
    use orch_db::queries::access::{PageKey, RowWrite, write_row};
    let s = Scene::new().await;
    let mut conn = s.world.conn().await;
    let page = PageKey::new("environment", s.first.base.id, "live");
    for permission in [
        "environment.view",
        "environment.manage",
        "files.view",
        "files.manage",
        "terminal.view",
        "terminal.manage",
        "browser.view",
        "browser.manage",
        "vnc.view",
        "vnc.manage",
        "ports.view",
        "ports.manage",
    ] {
        write_row(
            &mut conn,
            &RowWrite {
                page: &page,
                profile: "any",
                permission,
                inherit: false,
                locked: false,
            },
            Some(s.team.c.admin.id()),
            s.world.now(),
        )
        .await
        .unwrap();
    }
    let thread = new_id();
    orch_db::queries::hierarchy::testing::insert_node(
        &mut conn,
        thread,
        "thread",
        Some(s.team.topic),
        "native sync through-thread",
        s.team.c.admin.id(),
    )
    .await
    .unwrap();
    sqlx::query("UPDATE threads SET environment_id=$2 WHERE node_id=$1")
        .bind(thread)
        .bind(s.first.base.id)
        .execute(&mut *conn)
        .await
        .unwrap();
    drop(conn);
    resources::name_person(
        &s.world,
        "node",
        thread,
        "thread.view",
        s.team.c.stranger.id(),
    )
    .await;
    job(&s, None).await;
    let native = body(call(&s, &s.team.c.stranger.token, None).await);
    assert_eq!(native.environment.generation, 0);
    assert!(native.library.is_none());
    assert!(native.environment.host.is_none() && native.environment.template.is_none());
    assert!(native.jobs.iter().all(|job| matches!(
        job,
        orch_api_contract::environment::EnvironmentSyncJob::Reference(_)
    )));
    let reader = key(
        &s.world,
        s.team.c.stranger.id(),
        vec![
            rule(
                OPERATION,
                "environment",
                s.first.base.id,
                "self",
                &["environment"],
                &["read"],
            ),
            rule(
                OPERATION,
                "org",
                s.team.c.acme,
                "subtree",
                &["org", "project", "topic", "thread", "group"],
                &["read"],
            ),
        ],
    )
    .await;
    assert_eq!(body(call(&s, &reader.secret, None).await), native);
}

#[tokio::test]
async fn sync_publication_keeps_original_jobs_when_a_new_window_overflows_and_refuses_source_scope_loss()
 {
    for mode in ["unchanged", "new_jobs", "scope_closed", "expired"] {
        let s = Scene::new().await;
        job(&s, None).await;
        let reader = key(&s.world, s.team.c.admin.id(), rules(&s, true)).await;
        let original = body(call(&s, &reader.secret, Some(0)).await);
        assert!(original.complete);
        let mut blocker = s.world.db.pool.begin().await.unwrap();
        sqlx::query("LOCK TABLE api_token_activity IN SHARE MODE")
            .execute(&mut *blocker)
            .await
            .unwrap();
        let pid: i32 = sqlx::query_scalar("SELECT pg_backend_pid()")
            .fetch_one(&mut *blocker)
            .await
            .unwrap();
        let app = s.world.app.clone();
        let id = s.first.base.id;
        let token = reader.secret.clone();
        let request = tokio::spawn(async move {
            let call = app.call(EnvironmentSyncGet, &[&id]);
            call.token(&token)
                .query(&EnvironmentSyncGetQuery { since: Some(0) })
                .send_bytes()
                .await
        });
        activity_wait(&s, pid, &request).await;
        match mode {
            "new_jobs" => {
                for _ in 0..205 {
                    job(&s, None).await;
                }
            }
            "scope_closed" => {
                let scope:KeyScope=serde_json::from_value(json!({"rules":[
                    rule(OPERATION,"environment",s.second.base.id,"self",&["environment"],&["read"]),s.sources(OPERATION)
                ]})).unwrap();
                assert!(scope.validate().is_empty());
                sqlx::query("UPDATE api_tokens SET operation_scope=$2 WHERE id=$1")
                    .bind(reader.id)
                    .bind(json!(scope))
                    .execute(&s.world.db.pool)
                    .await
                    .unwrap();
            }
            "expired" => s.world.advance(chrono::Duration::hours(2)),
            _ => {}
        }
        blocker.commit().await.unwrap();
        let reply = tokio::time::timeout(std::time::Duration::from_secs(30), request)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(
            reply.status,
            match mode {
                "scope_closed" => StatusCode::FORBIDDEN,
                "expired" => StatusCode::UNAUTHORIZED,
                _ => StatusCode::OK,
            }
        );
        if mode == "unchanged" || mode == "new_jobs" {
            assert_eq!(
                body(reply),
                original,
                "a current replacement window cannot lend or discard original jobs"
            );
        }
        if mode == "new_jobs" {
            let fresh = body(call(&s, &reader.secret, Some(0)).await);
            assert!(!fresh.complete && fresh.jobs.is_empty());
        }
    }
}

#[tokio::test]
async fn sync_publication_cannot_lend_another_readable_waiting_machine_to_the_original_job_body() {
    let s = Scene::new().await;
    let mut machines = Vec::new();
    for name in ["original-sync-wait-marker", "replacement-sync-wait-marker"] {
        machines.push(
            orch_db::queries::resources::access_fixtures::insert_persons_machine(
                &mut *s.world.conn().await,
                s.team.c.admin.id(),
                name,
            )
            .await
            .unwrap(),
        );
    }
    let job = job(&s, Some(machines[0])).await;
    let mut policies = rules(&s, true);
    for machine in &machines {
        policies.push(rule(
            OPERATION,
            "machine",
            *machine,
            "self",
            &["machine"],
            &["read"],
        ));
    }
    let reader = key(&s.world, s.team.c.admin.id(), policies).await;
    assert!(
        serde_json::to_string(&body(call(&s, &reader.secret, None).await))
            .unwrap()
            .contains("original-sync-wait-marker")
    );
    let mut blocker = s.world.db.pool.begin().await.unwrap();
    sqlx::query("LOCK TABLE api_token_activity IN SHARE MODE")
        .execute(&mut *blocker)
        .await
        .unwrap();
    let pid: i32 = sqlx::query_scalar("SELECT pg_backend_pid()")
        .fetch_one(&mut *blocker)
        .await
        .unwrap();
    let app = s.world.app.clone();
    let id = s.first.base.id;
    let token = reader.secret.clone();
    let request = tokio::spawn(async move {
        let call = app.call(EnvironmentSyncGet, &[&id]);
        call.token(&token)
            .query(&EnvironmentSyncGetQuery { since: None })
            .send_bytes()
            .await
    });
    activity_wait(&s, pid, &request).await;
    sqlx::query("UPDATE jobs SET plan=jsonb_set(plan,'{active_command,machine}',$2) WHERE id=$1")
        .bind(job)
        .bind(json!(machines[1]))
        .execute(&s.world.db.pool)
        .await
        .unwrap();
    blocker.commit().await.unwrap();
    let reply = tokio::time::timeout(std::time::Duration::from_secs(30), request)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(reply.status, StatusCode::FORBIDDEN);
    assert!(
        serde_json::to_string(&body(call(&s, &reader.secret, None).await))
            .unwrap()
            .contains("replacement-sync-wait-marker")
    );
}
