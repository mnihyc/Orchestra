//! Native Job subjects retain alternatives, creator custody and their original progress cut.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "test assertions fail by panicking"
)]

#[path = "workspace_jobs_scopes/cancellation.rs"]
mod cancellation;
#[path = "workspace_snapshots/scope_helpers.rs"]
mod keys;
mod places;
mod support;
mod workspace;

use http::StatusCode;
use orch_api_contract::workspace::JobGet;
use orch_db::queries::jobs::{self, NewJob};
use orch_types::{Id, Json, new_id};
use serde_json::json;
use workspace::Site;

fn read_job(id: Id) -> Json {
    keys::rule("JobGet.ordinary", "job", id, "self", &["read"])
}

fn subject(operation: &str, kind: &str, id: Id, action: &str) -> Json {
    keys::rule(operation, kind, id, "self", &[action])
}

async fn site() -> Site {
    let site = Site::new().await;
    site.power_off().await;
    site
}

async fn seed(
    site: &Site,
    kind: &str,
    thread: Option<Id>,
    environment: Option<Id>,
    provider: Option<Id>,
    plan: Json,
) -> Id {
    let id = new_id();
    let progress = serde_json::to_value(orch_engine::jobs::runner::initial_progress(
        &orch_engine::jobs::measurement::PHASES,
    ))
    .unwrap();
    jobs::insert(
        &mut *site.world.conn().await,
        &NewJob {
            id,
            kind,
            node_id: thread,
            environment_id: environment,
            provider_id: provider,
            plan: &plan,
            progress: &progress,
            created_by: Some(site.ada.id()),
        },
        site.world.now(),
    )
    .await
    .unwrap();
    sqlx::query("UPDATE jobs SET result=$2 WHERE id=$1")
        .bind(id)
        .bind(json!({"native_result":"Earlier native Job result"}))
        .execute(&site.world.db.pool)
        .await
        .unwrap();
    id
}

async fn get(site: &Site, id: Id, token: &str, status: StatusCode) -> Json {
    let reply = site
        .app()
        .call(JobGet, &[&id])
        .token(token)
        .send_bytes()
        .await;
    assert_eq!(
        reply.status,
        status,
        "{}",
        String::from_utf8_lossy(&reply.raw)
    );
    serde_json::from_slice(&reply.raw).unwrap()
}

async fn add_permission(site: &Site, object: &str, id: Id, permission: &str, actor: Id) -> Id {
    orch_db::queries::access::testing::add_entry(
        &mut *site.world.conn().await,
        &orch_db::queries::access::PageKey::new(object, id, "live"),
        "any",
        permission,
        ("user", Some(actor), None),
        None,
        site.world.now(),
    )
    .await
    .unwrap()
}

#[tokio::test]
async fn ordinary_jobs_use_one_real_native_alternative_without_requiring_every_nonnull_link() {
    let site = site().await;
    for kind in [jobs::kind::ENVIRONMENT_RESTART, jobs::kind::SNAPSHOT] {
        let id = seed(
            &site,
            kind,
            Some(site.thread),
            Some(site.environment),
            Some(site.provider),
            json!({}),
        )
        .await;
        let native = get(&site, id, &site.ada.token, StatusCode::OK).await;
        for (kind, source) in [("environment", site.environment), ("thread", site.thread)] {
            let key = keys::scoped(
                &site,
                site.ada.id(),
                vec![
                    read_job(id),
                    subject("JobGet.ordinary", kind, source, "read"),
                ],
            )
            .await;
            assert_eq!(get(&site, id, &key.secret, StatusCode::OK).await, native);
        }
        for rules in [
            vec![read_job(id)],
            vec![subject(
                "JobGet.ordinary",
                "environment",
                site.environment,
                "read",
            )],
            vec![
                read_job(id),
                subject("NodeGet.read", "environment", site.environment, "read"),
            ],
            vec![
                subject("JobGet.ordinary", "job", id, "manage"),
                subject("JobGet.ordinary", "environment", site.environment, "read"),
            ],
        ] {
            let key = keys::scoped(&site, site.ada.id(), rules).await;
            get(&site, id, &key.secret, StatusCode::FORBIDDEN).await;
        }
    }
}

#[tokio::test]
async fn provider_job_visibility_retains_its_actual_manage_variant_and_ordinary_authority() {
    let site = site().await;
    let id = seed(
        &site,
        jobs::kind::PROVIDER_PROVISION,
        None,
        None,
        Some(site.provider),
        json!({}),
    )
    .await;
    let native = get(&site, id, &site.ada.token, StatusCode::OK).await;
    for operation in ["JobGet.ordinary", "JobGet.provider"] {
        let key = keys::scoped(
            &site,
            site.ada.id(),
            vec![
                subject(operation, "job", id, "read"),
                subject(operation, "provider", site.provider, "read"),
            ],
        )
        .await;
        get(&site, id, &key.secret, StatusCode::FORBIDDEN).await;
    }
    let key = keys::scoped(
        &site,
        site.ada.id(),
        vec![
            subject("JobGet.provider", "job", id, "read"),
            subject("JobGet.provider", "provider", site.provider, "manage"),
        ],
    )
    .await;
    assert_eq!(get(&site, id, &key.secret, StatusCode::OK).await, native);
    let stranger = keys::scoped(
        &site,
        site.eve.id(),
        vec![
            subject("JobGet.provider", "job", id, "read"),
            subject("JobGet.provider", "provider", site.provider, "manage"),
        ],
    )
    .await;
    let body = get(&site, id, &stranger.secret, StatusCode::FORBIDDEN).await;
    assert!(!body.to_string().contains("Earlier native Job result"));
}

#[tokio::test]
async fn creator_only_job_metadata_is_intrinsic_to_the_actual_job_and_stays_maker_only() {
    let site = site().await;
    for kind in [
        orch_engine::jobs::cleanup::KIND,
        orch_engine::resource_pause::KIND,
        jobs::kind::EXPORT,
        jobs::kind::IMPORT,
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
        let native = get(&site, id, &site.ada.token, StatusCode::OK).await;
        let key = keys::scoped(&site, site.ada.id(), vec![read_job(id)]).await;
        assert_eq!(get(&site, id, &key.secret, StatusCode::OK).await, native);
        let other = keys::scoped(&site, site.dev.id(), vec![read_job(id)]).await;
        get(&site, id, &other.secret, StatusCode::NOT_FOUND).await;
    }
}

#[tokio::test]
async fn through_thread_progress_never_becomes_a_full_body_by_selecting_a_different_scope_witness()
{
    let site = site().await;
    add_permission(&site, "node", site.thread, "thread.view", site.eve.id()).await;
    let id = seed(
        &site,
        jobs::kind::ENVIRONMENT_PROVISION,
        Some(site.thread),
        Some(site.environment),
        Some(site.provider),
        json!({}),
    )
    .await;
    let native = get(&site, id, &site.eve.token, StatusCode::OK).await;
    assert!(native["result"].is_null());
    assert!(native["node_id"].is_null());
    for (kind, source) in [("environment", site.environment), ("thread", site.thread)] {
        let key = keys::scoped(
            &site,
            site.eve.id(),
            vec![
                read_job(id),
                subject("JobGet.ordinary", kind, source, "read"),
            ],
        )
        .await;
        assert_eq!(get(&site, id, &key.secret, StatusCode::OK).await, native);
    }
    let older = seed(
        &site,
        jobs::kind::ENVIRONMENT_PROVISION,
        Some(site.thread),
        Some(site.environment),
        None,
        json!({}),
    )
    .await;
    // Both are open; native ordering, not scope, chooses which one has the progress door.
    let current = jobs::open_of_environment(&mut *site.world.conn().await, site.environment)
        .await
        .unwrap()[0]
        .id;
    let hidden = if current == id { older } else { id };
    let key = keys::scoped(
        &site,
        site.eve.id(),
        vec![
            read_job(hidden),
            subject("JobGet.ordinary", "thread", site.thread, "read"),
        ],
    )
    .await;
    get(&site, hidden, &key.secret, StatusCode::NOT_FOUND).await;
}

#[tokio::test]
async fn creator_job_read_keeps_its_real_place_ceiling_without_inventing_parent_read() {
    let site = site().await;
    let id = seed(
        &site,
        jobs::kind::EXPORT,
        Some(site.thread),
        Some(site.environment),
        Some(site.provider),
        json!({}),
    )
    .await;
    let native = get(&site, id, &site.ada.token, StatusCode::OK).await;
    for (place, allowed) in [(site.org, true), (site.ada.person.personal_org, false)] {
        let key = keys::scoped(&site, site.ada.id(), vec![read_job(id)]).await;
        let restriction = vec![vec![keys::rule(
            "JobGet.ordinary",
            "org",
            place,
            "subtree",
            &["read"],
        )]];
        sqlx::query("UPDATE api_tokens SET operation_scope=jsonb_set(operation_scope,'{constraints}',$2) WHERE id=$1")
            .bind(key.id).bind(json!(restriction)).execute(&site.world.db.pool).await.unwrap();
        let body = get(
            &site,
            id,
            &key.secret,
            if allowed {
                StatusCode::OK
            } else {
                StatusCode::FORBIDDEN
            },
        )
        .await;
        if allowed {
            assert_eq!(body, native);
        }
    }
}

#[tokio::test]
async fn job_measurement_and_notice_sources_come_from_their_native_subjects_not_nearby_columns() {
    let site = site().await;
    let measurement = seed(
        &site,
        orch_engine::jobs::measurement::KIND,
        None,
        None,
        None,
        json!({"subject":{"environment_id":site.environment}}),
    )
    .await;
    let native = get(&site, measurement, &site.ada.token, StatusCode::OK).await;
    let key = keys::scoped(
        &site,
        site.ada.id(),
        vec![
            read_job(measurement),
            subject("JobGet.ordinary", "environment", site.environment, "read"),
        ],
    )
    .await;
    assert_eq!(
        get(&site, measurement, &key.secret, StatusCode::OK).await,
        native
    );
    use orch_api_contract::documents::{NodeDocumentCreate, NodeDocumentCreateBody};
    let document = site
        .app()
        .call(NodeDocumentCreate, &[&site.topic])
        .token(&site.ada.token)
        .body(&NodeDocumentCreateBody {
            title: "Job's actual notice source".into(),
            blocks: None,
            markdown: None,
            access: None,
        })
        .send()
        .await
        .ok()
        .into_full()
        .expect("the ordinary notice fixture receives its full created document");
    let notice = seed(
        &site,
        orch_engine::jobs::document_notices::KIND,
        Some(site.thread),
        Some(site.environment),
        Some(site.provider),
        json!({"notice":{"document":document.base.id}}),
    )
    .await;
    let key = keys::scoped(
        &site,
        site.ada.id(),
        vec![
            read_job(notice),
            subject("JobGet.ordinary", "document", document.base.id, "read"),
        ],
    )
    .await;
    assert_eq!(
        get(&site, notice, &key.secret, StatusCode::OK).await,
        get(&site, notice, &site.ada.token, StatusCode::OK).await
    );
    let wrong = keys::scoped(
        &site,
        site.ada.id(),
        vec![
            read_job(notice),
            subject("JobGet.ordinary", "environment", site.environment, "read"),
        ],
    )
    .await;
    get(&site, notice, &wrong.secret, StatusCode::FORBIDDEN).await;
}

#[tokio::test]
async fn job_live_machine_labels_need_the_same_operation_source_read() {
    let site = site().await;
    let id = seed(
        &site,
        jobs::kind::ENVIRONMENT_PROVISION,
        None,
        Some(site.environment),
        None,
        json!({"active_command":{"machine":site.machine}}),
    )
    .await;
    let native = get(&site, id, &site.ada.token, StatusCode::OK).await;
    let physical = orch_db::queries::resources::hosting::host_of_machine(
        &mut *site.world.conn().await,
        site.machine,
    )
    .await
    .unwrap();
    let (machine_kind, machine_id) =
        physical.map_or(("machine", site.machine), |host| ("host", host));
    assert_eq!(
        native["progress"]["activity"]["waiting_for"]["name"],
        "workspace-machine"
    );
    for extra in [
        Vec::new(),
        vec![subject("JobGet.provider", machine_kind, machine_id, "read")],
    ] {
        let key = keys::scoped(
            &site,
            site.ada.id(),
            vec![
                read_job(id),
                subject("JobGet.ordinary", "environment", site.environment, "read"),
            ]
            .into_iter()
            .chain(extra)
            .collect(),
        )
        .await;
        let body = get(&site, id, &key.secret, StatusCode::OK).await;
        assert!(body["progress"]["activity"]["waiting_for"].is_null());
        assert_eq!(body["result"], native["result"]);
    }
    let key = keys::scoped(
        &site,
        site.ada.id(),
        vec![
            read_job(id),
            subject("JobGet.ordinary", "environment", site.environment, "read"),
            subject("JobGet.ordinary", machine_kind, machine_id, "read"),
        ],
    )
    .await;
    assert_eq!(get(&site, id, &key.secret, StatusCode::OK).await, native);
}

#[tokio::test]
async fn job_publication_preserves_the_native_full_versus_through_thread_cut() {
    let site = site().await;
    add_permission(&site, "node", site.thread, "thread.view", site.eve.id()).await;
    let held = add_permission(
        &site,
        "environment",
        site.environment,
        "files.view",
        site.eve.id(),
    )
    .await;
    let id = seed(
        &site,
        jobs::kind::ENVIRONMENT_PROVISION,
        Some(site.thread),
        Some(site.environment),
        None,
        json!({}),
    )
    .await;
    let key = keys::scoped(
        &site,
        site.eve.id(),
        vec![
            read_job(id),
            subject("JobGet.ordinary", "environment", site.environment, "read"),
        ],
    )
    .await;
    let control = get(&site, id, &key.secret, StatusCode::OK).await;
    assert_eq!(
        control["result"]["native_result"],
        "Earlier native Job result"
    );
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
    let request = tokio::spawn(async move {
        let call = app.call(JobGet, &[&id]).token(&key.secret);
        call.send_bytes().await
    });
    activity_wait(&site, pid, &request).await;
    let mut change = site.world.db.pool.begin().await.unwrap();
    assert!(
        orch_db::queries::access::remove_entry(&mut change, held, site.ada.id(), site.world.now())
            .await
            .unwrap()
    );
    orch_db::queries::access::bump_epoch_in_root(&mut change, site.org)
        .await
        .unwrap();
    change.commit().await.unwrap();
    let current = get(&site, id, &site.eve.token, StatusCode::OK).await;
    assert!(
        current["result"].is_null(),
        "native standing, not key scope, now requires the reduced body"
    );
    blocker.commit().await.unwrap();
    let reply = tokio::time::timeout(WAIT, request).await.unwrap().unwrap();
    assert_eq!(reply.status, StatusCode::FORBIDDEN);
    assert!(!String::from_utf8_lossy(&reply.raw).contains("Earlier native Job result"));
}

const WAIT: std::time::Duration = std::time::Duration::from_secs(30);

async fn activity_wait(
    site: &Site,
    pid: i32,
    request: &tokio::task::JoinHandle<orch_api_core::testing::RawReply>,
) {
    tokio::time::timeout(WAIT, async {
        loop {
            assert!(!request.is_finished(), "the actual Job response must wait at activity append");
            let blocked: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM pg_stat_activity WHERE datname=current_database() AND wait_event_type='Lock' AND $1=ANY(pg_blocking_pids(pid)) AND query LIKE '%INSERT INTO api_token_activity%')")
                .bind(pid).fetch_one(&site.world.db.pool).await.unwrap();
            if blocked { break; } tokio::task::yield_now().await;
        }
    }).await.expect("observe the literal native Job publication wait");
}

#[tokio::test]
async fn job_publication_cannot_replace_the_original_subject_at_the_same_job_identity() {
    let site = site().await;
    let id = seed(
        &site,
        jobs::kind::ENVIRONMENT_PROVISION,
        Some(site.thread),
        Some(site.environment),
        None,
        json!({}),
    )
    .await;
    let key = keys::scoped(
        &site,
        site.ada.id(),
        vec![
            read_job(id),
            subject("JobGet.ordinary", "environment", site.environment, "read"),
        ],
    )
    .await;
    let control = get(&site, id, &key.secret, StatusCode::OK).await;
    assert_eq!(
        control["result"]["native_result"],
        "Earlier native Job result"
    );
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
    let request = tokio::spawn(async move {
        let call = app.call(JobGet, &[&id]).token(&key.secret);
        call.send_bytes().await
    });
    activity_wait(&site, pid, &request).await;
    sqlx::query("UPDATE jobs SET node_id=NULL WHERE id=$1")
        .bind(id)
        .execute(&site.world.db.pool)
        .await
        .unwrap();
    blocker.commit().await.unwrap();
    let reply = tokio::time::timeout(WAIT, request).await.unwrap().unwrap();
    assert_eq!(reply.status, StatusCode::FORBIDDEN);
    assert!(!String::from_utf8_lossy(&reply.raw).contains("Earlier native Job result"));
}

#[tokio::test]
async fn job_publication_checks_live_key_expiry_after_the_actual_activity_wait() {
    let site = site().await;
    let id = seed(
        &site,
        jobs::kind::ENVIRONMENT_PROVISION,
        None,
        Some(site.environment),
        None,
        json!({}),
    )
    .await;
    let key = keys::scoped(
        &site,
        site.ada.id(),
        vec![
            read_job(id),
            subject("JobGet.ordinary", "environment", site.environment, "read"),
        ],
    )
    .await;
    get(&site, id, &key.secret, StatusCode::OK).await;
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
    let request = tokio::spawn(async move {
        let call = app.call(JobGet, &[&id]).token(&key.secret);
        call.send_bytes().await
    });
    activity_wait(&site, pid, &request).await;
    site.world.advance(chrono::Duration::hours(2));
    blocker.commit().await.unwrap();
    let reply = tokio::time::timeout(WAIT, request).await.unwrap().unwrap();
    assert_eq!(reply.status, StatusCode::UNAUTHORIZED);
    assert!(!String::from_utf8_lossy(&reply.raw).contains("Earlier native Job result"));
}
